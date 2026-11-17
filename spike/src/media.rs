//! The sender's encode loop (through `telinhas-codec`: hardware with a
//! software fallback) and the receiver's decoder (`openh264`), each on its
//! own thread.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::Receiver;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use openh264::decoder::Decoder;
use telinhas_codec::{Backend, Format, Frame, Options, Rate};
use tokio::sync::mpsc::UnboundedSender;

use crate::frames::{FrameSource, FrameSpec, paint_stamp_bgra};
use crate::source::{elapsed_ms, now_micros, read_stamp};
use crate::stats::Stats;

#[derive(Debug, Clone)]
pub struct StreamConfig {
    /// `synthetic`, `monitor:N`, `window:HANDLE` or `window:TEXT`.
    pub source: String,
    /// Size of the synthetic picture; a real source uses its own size.
    pub width: usize,
    pub height: usize,
    pub fps: u32,
    /// Ceiling for the encoder, whatever congestion control says.
    pub max_bps: u32,
    /// Constant quality, lower is better.
    pub quality: u32,
    /// `auto`, `hardware` or `software`.
    pub backend: String,
    /// A keyframe at least this often, as a safety net. The spike does not
    /// read picture-loss requests (PLI) from the receiver yet.
    pub keyframe_every: Duration,
    /// 0 to 5: how noisy the synthetic picture is, and so how hard to encode.
    pub noise_level: u8,
    pub cursor: bool,
    /// Leave the system's capture border on (the yellow outline).
    pub border: bool,
    /// The encoder ceiling as a share of the congestion estimate. `None`
    /// picks a default per encoder: NVENC overshoots its ceiling, so it gets
    /// more room than the software encoder, which holds it strictly.
    pub headroom: Option<f64>,
}

impl Default for StreamConfig {
    fn default() -> Self {
        Self {
            source: "synthetic".to_owned(),
            width: 1280,
            height: 720,
            fps: 30,
            max_bps: 4_000_000,
            quality: 23,
            backend: "auto".to_owned(),
            keyframe_every: Duration::from_secs(5),
            noise_level: 3,
            cursor: true,
            border: true,
            headroom: None,
        }
    }
}

/// What the sending side did with its frames, so encoder skips, deliberate
/// drops and network loss can be told apart.
#[derive(Default)]
pub struct SendCounters {
    /// Frames the source produced.
    pub generated: AtomicU64,
    /// Frames the encoder emitted (the rest it skipped to hold its bitrate).
    pub encoded: AtomicU64,
    /// Frames handed to the network.
    pub written: AtomicU64,
    /// Frames dropped on purpose because too many were still in flight.
    pub dropped: AtomicU64,
}

impl std::fmt::Display for SendCounters {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let get = |c: &AtomicU64| c.load(Ordering::Relaxed);
        write!(
            f,
            "generated={} encoded={} (encoder skipped {}) written={} dropped_backlog={}",
            get(&self.generated),
            get(&self.encoded),
            get(&self.generated).saturating_sub(get(&self.encoded)),
            get(&self.written),
            get(&self.dropped),
        )
    }
}

pub struct EncodedFrame {
    pub data: Vec<u8>,
    pub counter: u32,
    pub captured_at: Instant,
}

/// A congestion estimate must move this much to change the encoder's ceiling,
/// and not more often than `MIN_CEILING_INTERVAL`: NVENC settles over several
/// seconds, so following every report would only churn.
const CEILING_STEP_PERCENT: u32 = 25;
const MIN_CEILING_INTERVAL: Duration = Duration::from_secs(3);

fn parse_backend(text: &str) -> Result<Backend> {
    Ok(match text {
        "auto" => Backend::Auto,
        "hardware" => Backend::Hardware,
        "software" => Backend::Software,
        other => bail!("unknown backend {other:?}; use auto, hardware or software"),
    })
}

/// Starts the capture-and-encode loop. `target_bps` holds the congestion
/// controller's estimate as `f64` bits. The loop runs at a fixed rate and
/// repeats the last picture when the screen did not change.
pub fn spawn_encoder(
    cfg: StreamConfig,
    out: UnboundedSender<EncodedFrame>,
    stop: Arc<AtomicBool>,
    target_bps: Arc<AtomicU64>,
    need_key: Arc<AtomicBool>,
    counters: Arc<SendCounters>,
) -> JoinHandle<Result<()>> {
    std::thread::spawn(move || {
        let mut frames = FrameSource::open(&FrameSpec {
            source: cfg.source.clone(),
            fps: cfg.fps,
            width: cfg.width as u32,
            height: cfg.height as u32,
            noise: cfg.noise_level,
            cursor: cfg.cursor,
            border: cfg.border,
        })?;
        if cfg.source != "synthetic" {
            println!("source {}: {}", cfg.source, frames.note());
        }

        let pick = |target: &AtomicU64, headroom: f64| -> u32 {
            // Stay under the estimate, with a low floor: a floor above the
            // estimate piles up a queue whose delay grows without bound.
            let estimate = f64::from_bits(target.load(Ordering::Relaxed)) * headroom;
            ((estimate / 1000.0) as u32).clamp(100, cfg.max_bps / 1000)
        };
        let mut kbps = pick(&target_bps, 0.5);
        let (mut width, mut height) = frames.size();
        let options = |width, height, kbps| Options {
            format: Format {
                width,
                height,
                fps: cfg.fps,
            },
            rate: Rate {
                quality: cfg.quality,
                max_kbps: kbps,
            },
            keyframe_every: (cfg.keyframe_every.as_secs_f64() * f64::from(cfg.fps)) as u32,
        };
        let backend = parse_backend(&cfg.backend)?;
        let created = telinhas_codec::create(backend, options(width, height, kbps))?;
        let mut encoder = created.encoder;
        let info = encoder.info();
        // NVENC was measured to overshoot its ceiling by up to about 50%, and
        // what it sends beyond the congestion estimate waits in the pacer's
        // queue as latency (0.8 gave about 380 ms in loopback, 0.5 gave 107 ms).
        let headroom = cfg
            .headroom
            .unwrap_or(if info.hardware { 0.5 } else { 0.8 });
        if pick(&target_bps, headroom) != kbps {
            kbps = pick(&target_bps, headroom);
            encoder.set_max_kbps(kbps)?;
        }
        println!(
            "encoder: {} ({}), {width}x{height} at {} fps",
            info.name,
            if info.hardware {
                "hardware"
            } else {
                "software"
            },
            cfg.fps
        );
        if let Some(reason) = created.fallback_reason {
            println!(
                "WARNING: using the software encoder, the stream may stutter. Reason: {reason}"
            );
        }

        let period = Duration::from_secs_f64(1.0 / f64::from(cfg.fps));
        let mut next = Instant::now();
        let mut last_ceiling_change = Instant::now();
        let mut tick = 0u64;
        let mut picture = Vec::new();
        while !stop.load(Ordering::Relaxed) {
            let wanted = pick(&target_bps, headroom);
            if wanted.abs_diff(kbps) * 100 / kbps.max(1) > CEILING_STEP_PERCENT
                && last_ceiling_change.elapsed() >= MIN_CEILING_INTERVAL
            {
                kbps = wanted;
                encoder.set_max_kbps(kbps)?;
                last_ceiling_change = Instant::now();
            }
            if need_key.swap(false, Ordering::Relaxed) {
                encoder.request_keyframe();
            }

            let captured_at = Instant::now();
            let (w, h) = frames.fill(tick, &mut picture);
            if (w, h) != (width, height) {
                // The source changed size (a window was resized): open the
                // encoder again at the new size. It starts with a keyframe.
                (width, height) = (w, h);
                encoder = telinhas_codec::create(backend, options(width, height, kbps))?.encoder;
                println!("source resized to {width}x{height}");
            }
            let counter = tick as u32;
            paint_stamp_bgra(&mut picture, w, counter, now_micros());
            counters.generated.fetch_add(1, Ordering::Relaxed);
            let packets = encoder.encode(&Frame {
                width: w,
                height: h,
                stride: w as usize * 4,
                bgra: &picture,
            })?;
            if !packets.is_empty() {
                counters.encoded.fetch_add(1, Ordering::Relaxed);
            }
            for packet in packets {
                let frame = EncodedFrame {
                    data: packet.data,
                    counter,
                    captured_at,
                };
                if out.send(frame).is_err() {
                    frames.stop();
                    return Ok(());
                }
            }
            tick += 1;

            next += period;
            let now = Instant::now();
            if next > now {
                std::thread::sleep(next - now);
            } else if now - next > period * 5 {
                next = now; // fell far behind; do not try to catch up
            }
        }
        frames.stop();
        Ok(())
    })
}

/// One access unit handed to the decoder, with the time it was complete.
pub struct Unit {
    pub data: Vec<u8>,
    pub ready_at: Instant,
}

/// Decodes access units and records what the stamps say.
///
/// Each decoded frame's counter is also sent to `acks`, if given, so the
/// sender can time the round trip without relying on synchronized clocks.
pub fn spawn_decoder(
    units: Receiver<Unit>,
    stats: Arc<Stats>,
    acks: Option<UnboundedSender<u32>>,
) -> JoinHandle<Result<()>> {
    std::thread::spawn(move || {
        let mut decoder = Decoder::new()?;
        while let Ok(unit) = units.recv() {
            let len = unit.data.len();
            // Early units may fail until the first keyframe arrives.
            let Ok(Some(yuv)) = decoder.decode(&unit.data) else {
                continue;
            };
            let (counter, captured) = read_stamp(&yuv);
            stats.record(counter, elapsed_ms(captured, now_micros()), len);
            if let Some(acks) = &acks {
                let _ = acks.send(counter);
            }
        }
        Ok(())
    })
}
