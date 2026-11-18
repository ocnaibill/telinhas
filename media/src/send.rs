//! The broadcaster's loop: capture (or a generated picture), encode once, and
//! hand the H.264 to every viewer's link.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use bytes::Bytes;
use telinhas_capture::{self as capture, Options as CaptureOptions};
pub use telinhas_codec::Backend;
use telinhas_codec::{Format, Frame, Options, Rate};
use telinhas_protocol::PeerId;
use tokio::sync::broadcast;

use crate::Event;
use crate::link::BWE_INITIAL;
use crate::scale::{downscale, target_size};
use crate::stamp;
use crate::stats::{Sending, Stats, smooth};

/// What to broadcast.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BroadcastSource {
    /// A moving test pattern of this size, for tests and machines without capture.
    Synthetic { width: u32, height: u32 },
    /// A monitor, by the index of `telinhas_capture::list_monitors`.
    Monitor(usize),
    /// A window, by the handle of `telinhas_capture::list_windows`.
    Window(isize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BroadcastSpec {
    pub source: BroadcastSource,
    pub fps: u32,
    /// Constant quality, lower is better (23 is high quality).
    pub quality: u32,
    /// The most to send, in kbps, whatever the network estimate says.
    pub max_kbps: u32,
    pub backend: Backend,
    /// Send at most this many lines (shrinking, never enlarging).
    pub max_height: Option<u32>,
    /// The sound to send with the picture, if any.
    pub audio: Option<crate::AudioInput>,
    pub cursor: bool,
    /// Leave the system's capture border on.
    pub border: bool,
}

impl BroadcastSpec {
    pub fn new(source: BroadcastSource, fps: u32, max_kbps: u32) -> Self {
        Self {
            source,
            fps,
            // Lower is better. 23 gave about 8.8 Mbps at 1080p on a game and
            // looked soft; the ceiling was not the limit. `TELINHAS_CQ` is for
            // trying values without rebuilding.
            quality: std::env::var("TELINHAS_CQ")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(20),
            max_kbps,
            backend: Backend::Auto,
            max_height: None,
            audio: None,
            cursor: true,
            border: false,
        }
    }
}

/// One encoded picture on its way to every viewer.
pub(crate) struct Encoded {
    /// The access unit, with its frame number stamped in front.
    pub data: Bytes,
    pub duration: Duration,
    pub counter: u32,
    /// When the picture was taken from the screen.
    pub captured_at: Instant,
}

/// The congestion estimate of each viewer's path, for the encoder to follow.
pub(crate) type Targets = Arc<Mutex<HashMap<PeerId, Arc<AtomicU64>>>>;

/// The weakest path decides: the encoder runs once for everybody.
fn weakest(targets: &Targets) -> f64 {
    let targets = targets.lock().unwrap();
    let lowest = targets
        .values()
        .map(|t| f64::from_bits(t.load(Ordering::Relaxed)))
        .filter(|t| *t > 0.0)
        .fold(f64::INFINITY, f64::min);
    // Nobody connected yet: start from the initial guess.
    if lowest.is_finite() {
        lowest
    } else {
        BWE_INITIAL
    }
}

/// A ceiling must move this much to be changed, and not more often than
/// `MIN_CEILING_INTERVAL`: NVENC settles over several seconds, so following
/// every report would only churn.
const CEILING_STEP_PERCENT: u32 = 25;
const MIN_CEILING_INTERVAL: Duration = Duration::from_secs(3);

#[derive(Default)]
struct Latest {
    width: u32,
    height: u32,
    bgra: Vec<u8>,
}

enum Frames {
    Synthetic {
        width: u32,
        height: u32,
    },
    Capture {
        latest: Arc<Mutex<Latest>>,
        /// How many pictures the capture has delivered so far.
        delivered: Arc<AtomicU64>,
        session: capture::Session,
    },
}

impl Frames {
    fn open(spec: &BroadcastSpec) -> Result<Self> {
        let source = match spec.source {
            BroadcastSource::Synthetic { width, height } => {
                return Ok(Self::Synthetic {
                    width: width & !1,
                    height: height & !1,
                });
            }
            BroadcastSource::Monitor(index) => capture::Source::Monitor(index),
            BroadcastSource::Window(handle) => capture::Source::Window(handle),
        };
        capture::init_dpi_awareness();
        let latest: Arc<Mutex<Latest>> = Arc::default();
        let sink = Arc::clone(&latest);
        let delivered = Arc::new(AtomicU64::new(0));
        let counter = Arc::clone(&delivered);
        let session = capture::start(
            source,
            CaptureOptions {
                cursor: spec.cursor,
                border: spec.border,
                max_fps: Some(spec.fps),
            },
            move |frame| {
                // Even sizes only: H.264 needs them.
                counter.fetch_add(1, Ordering::Relaxed);
                let (w, h) = (frame.width & !1, frame.height & !1);
                let mut latest = sink.lock().unwrap();
                latest.width = w;
                latest.height = h;
                latest.bgra.clear();
                let row = w as usize * 4;
                for line in frame.bgra.chunks(frame.stride).take(h as usize) {
                    latest.bgra.extend_from_slice(&line[..row]);
                }
            },
        )?;
        let waited = Instant::now();
        while latest.lock().unwrap().bgra.is_empty() {
            if waited.elapsed() > Duration::from_secs(5) {
                bail!("no picture arrived in 5 s: the source is still or cannot be captured");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        Ok(Self::Capture {
            latest,
            delivered,
            session,
        })
    }

    fn delivered(&self) -> u64 {
        match self {
            Self::Synthetic { .. } => 0,
            Self::Capture { delivered, .. } => delivered.load(Ordering::Relaxed),
        }
    }

    fn size(&self) -> (u32, u32) {
        match self {
            Self::Synthetic { width, height } => (*width, *height),
            Self::Capture { latest, .. } => {
                let latest = latest.lock().unwrap();
                (latest.width, latest.height)
            }
        }
    }

    /// The current picture as BGRA. A still screen is repeated, as it delivers
    /// no frames.
    fn fill(&self, tick: u64, out: &mut Vec<u8>) -> (u32, u32) {
        match self {
            Self::Synthetic { width, height } => {
                synthetic(*width, *height, tick, out);
                (*width, *height)
            }
            Self::Capture { latest, .. } => {
                let latest = latest.lock().unwrap();
                out.clear();
                out.extend_from_slice(&latest.bgra);
                (latest.width, latest.height)
            }
        }
    }
}

/// A gradient with a bar sweeping across it, so motion is easy to see.
fn synthetic(width: u32, height: u32, tick: u64, out: &mut Vec<u8>) {
    let (w, h) = (width as usize, height as usize);
    out.clear();
    out.reserve(w * h * 4);
    let bar = (tick as usize * 8) % w.max(1);
    for y in 0..h {
        for x in 0..w {
            let on_bar = x >= bar && x < bar + 24;
            let (b, g, r) = if on_bar {
                (255, 255, 255)
            } else {
                ((x * 255 / w.max(1)) as u8, (y * 255 / h.max(1)) as u8, 96)
            };
            out.extend_from_slice(&[b, g, r, 255]);
        }
    }
}

/// Starts capturing and encoding. Encoded pictures go to `out`; the loop ends
/// when `stop` is set or nobody listens any more.
pub(crate) fn spawn_broadcast(
    spec: BroadcastSpec,
    out: broadcast::Sender<Arc<Encoded>>,
    stop: Arc<AtomicBool>,
    need_key: Arc<AtomicBool>,
    targets: Targets,
    stats: Arc<Mutex<Stats>>,
    tell: Arc<dyn Fn(Event) + Send + Sync>,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        if let Err(error) = run(spec, &out, &stop, &need_key, &targets, &stats, &*tell) {
            tell(Event::BroadcastFailed(error.to_string()));
        }
        stats.lock().unwrap().sending = None;
    })
}

fn run(
    spec: BroadcastSpec,
    out: &broadcast::Sender<Arc<Encoded>>,
    stop: &AtomicBool,
    need_key: &AtomicBool,
    targets: &Targets,
    stats: &Mutex<Stats>,
    tell: &dyn Fn(Event),
) -> Result<()> {
    let frames = Frames::open(&spec)?;
    // Stay under the estimate, with a low floor: a floor above the estimate
    // piles up a queue whose delay grows without bound.
    let pick = |headroom: f64| -> u32 {
        ((weakest(targets) * headroom / 1000.0) as u32).clamp(100, spec.max_kbps.max(100))
    };
    let options = |width, height, kbps| Options {
        format: Format {
            width,
            height,
            fps: spec.fps,
        },
        rate: Rate {
            quality: spec.quality,
            max_kbps: kbps,
        },
        keyframe_every: 0,
    };
    let (source_w, source_h) = frames.size();
    let (mut width, mut height) = target_size(source_w, source_h, spec.max_height);
    let mut kbps = pick(0.5);
    let created = telinhas_codec::create(spec.backend, options(width, height, kbps))?;
    let mut encoder = created.encoder;
    let info = encoder.info();
    // NVENC was measured to overshoot its ceiling by up to about 50%, and what
    // it sends beyond the estimate waits in the pacer's queue as latency
    // (0.8 gave about 380 ms in loopback, 0.5 gave 107 ms).
    let headroom = if info.hardware { 0.5 } else { 0.8 };
    if pick(headroom) != kbps {
        kbps = pick(headroom);
        encoder.set_max_kbps(kbps)?;
    }
    stats.lock().unwrap().sending = Some(Sending {
        encoder: info.name.clone(),
        hardware: info.hardware,
        width,
        height,
        ..Sending::default()
    });
    tell(Event::Encoder {
        name: info.name,
        hardware: info.hardware,
        fallback_reason: created.fallback_reason,
        width,
        height,
    });

    let period = Duration::from_secs_f64(1.0 / f64::from(spec.fps.max(1)));
    let mut next = Instant::now();
    let mut last_ceiling_change = Instant::now();
    let mut tick = 0u64;
    let mut picture = Vec::new();
    let mut scaled = Vec::new();
    let (mut window, mut window_frames, mut window_bytes) = (Instant::now(), 0u32, 0usize);
    let mut window_delivered = 0u64;
    while !stop.load(Ordering::Relaxed) {
        let wanted = pick(headroom);
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
        let (source_w, source_h) = frames.fill(tick, &mut picture);
        let (w, h) = target_size(source_w, source_h, spec.max_height);
        if (w, h) != (source_w, source_h) {
            downscale(
                &picture,
                source_w as usize,
                source_h as usize,
                w as usize,
                h as usize,
                &mut scaled,
            );
        }
        let pixels: &[u8] = if (w, h) == (source_w, source_h) {
            &picture
        } else {
            &scaled
        };
        let copy_ms = captured_at.elapsed().as_secs_f32() * 1000.0;
        if (w, h) != (width, height) {
            // The source changed size (a window was resized): open the
            // encoder again at the new size. It starts with a keyframe.
            (width, height) = (w, h);
            encoder = telinhas_codec::create(spec.backend, options(width, height, kbps))?.encoder;
        }
        let encode_started = Instant::now();
        let packets = encoder.encode(&Frame {
            width: w,
            height: h,
            stride: w as usize * 4,
            bgra: pixels,
        })?;
        let encode_ms = encode_started.elapsed().as_secs_f32() * 1000.0;
        let counter = tick as u32;
        for packet in packets {
            let data = Bytes::from(stamp::prepend(counter, &packet.data));
            window_bytes += data.len();
            // No receiver means nobody is connected yet: the picture is
            // dropped, and a keyframe is asked for when someone connects.
            let _ = out.send(Arc::new(Encoded {
                data,
                duration: period,
                counter,
                captured_at,
            }));
        }
        window_frames += 1;
        let elapsed = window.elapsed().as_secs_f32();
        let mut stats = stats.lock().unwrap();
        if let Some(sending) = stats.sending.as_mut() {
            sending.width = w;
            sending.height = h;
            sending.encode_ms = smooth(sending.encode_ms, encode_ms);
            sending.copy_ms = smooth(sending.copy_ms, copy_ms);
            if elapsed >= 1.0 {
                let delivered = frames.delivered();
                sending.capture_fps = (delivered - window_delivered) as f32 / elapsed;
                window_delivered = delivered;
                sending.fps = window_frames as f32 / elapsed;
                sending.kbps = window_bytes as f32 * 8.0 / 1000.0 / elapsed;
                (window, window_frames, window_bytes) = (Instant::now(), 0, 0);
            }
        }
        drop(stats);
        tick += 1;

        next += period;
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        } else if now - next > period * 5 {
            next = now; // fell far behind; do not try to catch up
        }
    }
    if let Frames::Capture { session, .. } = frames {
        session.stop();
    }
    Ok(())
}
