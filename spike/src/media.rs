//! H.264 encode and decode on dedicated threads (software, `openh264`).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::Receiver;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::Result;
use openh264::decoder::Decoder;
use openh264::encoder::{
    BitRate, Encoder, EncoderConfig, FrameRate, IntraFramePeriod, RateControlMode, UsageType,
};
use tokio::sync::mpsc::UnboundedSender;

use crate::source::{Source, elapsed_ms, now_micros, read_stamp};
use crate::stats::Stats;

#[derive(Debug, Clone)]
pub struct StreamConfig {
    pub width: usize,
    pub height: usize,
    pub fps: u32,
    /// Ceiling for the encoder, whatever congestion control says.
    pub max_bps: u32,
    /// A keyframe at least this often, so late joiners and losses recover
    /// (the spike does not handle PLI yet).
    pub keyframe_every: Duration,
    /// 0 to 5: how noisy the synthetic picture is, and so how hard to encode.
    pub noise_level: u8,
}

impl Default for StreamConfig {
    fn default() -> Self {
        Self {
            width: 1280,
            height: 720,
            fps: 30,
            max_bps: 4_000_000,
            keyframe_every: Duration::from_secs(2),
            noise_level: 3,
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

fn make_encoder(cfg: &StreamConfig, bps: u32) -> Result<Encoder> {
    let frames = (cfg.keyframe_every.as_secs_f64() * f64::from(cfg.fps)).round() as u32;
    let config = EncoderConfig::new()
        .bitrate(BitRate::from_bps(bps))
        .max_frame_rate(FrameRate::from_hz(cfg.fps as f32))
        .usage_type(UsageType::ScreenContentRealTime)
        .rate_control_mode(RateControlMode::Bitrate)
        .skip_frames(true)
        .intra_frame_period(IntraFramePeriod::from_num_frames(frames.max(1)));
    Ok(Encoder::with_api_config(
        openh264::OpenH264API::from_source(),
        config,
    )?)
}

/// Starts the capture-and-encode loop. `target_bps` holds the congestion
/// controller's estimate as `f64` bits; the encoder is rebuilt when it
/// drifts far from what it is using.
pub fn spawn_encoder(
    cfg: StreamConfig,
    out: UnboundedSender<EncodedFrame>,
    stop: Arc<AtomicBool>,
    target_bps: Arc<AtomicU64>,
    need_key: Arc<AtomicBool>,
    counters: Arc<SendCounters>,
) -> JoinHandle<Result<()>> {
    std::thread::spawn(move || {
        let mut source = Source::new(cfg.width, cfg.height, cfg.noise_level);
        let pick = |target: &AtomicU64| -> u32 {
            // Stay under the estimate, with a low floor: a floor above the
            // estimate piles up a queue whose delay grows without bound.
            let estimate = f64::from_bits(target.load(Ordering::Relaxed)) * 0.8;
            (estimate as u32).clamp(100_000, cfg.max_bps)
        };
        let mut bps = pick(&target_bps);
        let mut encoder = make_encoder(&cfg, bps)?;
        let interval = Duration::from_secs_f64(1.0 / f64::from(cfg.fps));
        let mut next = Instant::now();
        while !stop.load(Ordering::Relaxed) {
            let wanted = pick(&target_bps);
            if wanted.abs_diff(bps) * 100 / bps > 25 {
                bps = wanted;
                encoder = make_encoder(&cfg, bps)?;
            }
            if need_key.swap(false, Ordering::Relaxed) {
                encoder.force_intra_frame();
            }
            let captured_at = Instant::now();
            let frame = source.next_frame();
            counters.generated.fetch_add(1, Ordering::Relaxed);
            let stream = encoder.encode(&frame.yuv)?;
            let data = stream.to_vec();
            if !data.is_empty() {
                counters.encoded.fetch_add(1, Ordering::Relaxed);
            }
            if !data.is_empty()
                && out
                    .send(EncodedFrame {
                        data,
                        counter: frame.counter,
                        captured_at,
                    })
                    .is_err()
            {
                break;
            }
            next += interval;
            let now = Instant::now();
            if next > now {
                std::thread::sleep(next - now);
            } else if now - next > interval * 5 {
                next = now; // fell far behind; do not try to catch up
            }
        }
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
