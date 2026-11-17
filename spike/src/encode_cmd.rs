//! `encode`: runs the real encoder (`telinhas-codec`) on the live screen or a
//! synthetic picture, and checks the things the sender will depend on: the
//! bitrate ceiling following congestion control, keyframes on demand, the time
//! each frame takes, and the software fallback (M3c in `docs/PLANO-M3.md`).

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use telinhas_capture::{self as capture, Options as CaptureOptions};
use telinhas_codec::{Backend, Format, Frame, Options, Rate};

use crate::capture_cmd::parse_source;

pub struct Args {
    /// `synthetic`, `monitor:N` or `window:TEXT`.
    pub source: String,
    pub seconds: u64,
    pub fps: u32,
    /// Size of the synthetic picture.
    pub width: u32,
    pub height: u32,
    pub noise: u8,
    pub quality: u32,
    pub max_kbps: u32,
    pub backend: String,
    /// `SECONDS:KBPS`: change the ceiling this many seconds in.
    pub cap_change: Option<String>,
    /// Ask for a keyframe this many seconds in.
    pub keyframe_at: Option<u64>,
}

#[derive(Default)]
struct Latest {
    width: u32,
    height: u32,
    bgra: Vec<u8>,
}

fn percentile(values: &[f64], q: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    sorted[((sorted.len() - 1) as f64 * q).round() as usize]
}

/// A moving gradient and square, with optional noise, as BGRA.
fn synthetic(width: u32, height: u32, index: u64, noise: u8, rng: &mut u32, out: &mut Vec<u8>) {
    let (w, h) = (width as usize, height as usize);
    out.clear();
    out.resize(w * h * 4, 255);
    let mask: u8 = if noise == 0 {
        0
    } else {
        (1u8 << noise.min(7)) - 1
    };
    let t = index as usize;
    let side = h / 6;
    let (sx, sy) = ((t * 7) % (w - side), h / 3 + (t * 3) % (h / 3));
    for y in 0..h {
        for x in 0..w {
            let mut v = 48 + (((x + y / 2 + t * 3) / 4) % 64) as u8 * 2;
            if x >= sx && x < sx + side && y >= sy && y < sy + side {
                v = 235;
            }
            if mask != 0 {
                *rng ^= *rng << 13;
                *rng ^= *rng >> 17;
                *rng ^= *rng << 5;
                v = v.saturating_add((*rng as u8) & mask);
            }
            let at = (y * w + x) * 4;
            out[at] = v;
            out[at + 1] = v;
            out[at + 2] = v;
        }
    }
}

pub fn run(args: &Args) -> Result<()> {
    capture::init_dpi_awareness();
    let backend = match args.backend.as_str() {
        "auto" => Backend::Auto,
        "hardware" => Backend::Hardware,
        "software" => Backend::Software,
        other => bail!("unknown backend {other:?}; use auto, hardware or software"),
    };
    let cap_change = args
        .cap_change
        .as_ref()
        .map(|text| {
            let (at, kbps) = text.split_once(':').context("use SECONDS:KBPS")?;
            anyhow::Ok((at.parse::<u64>()?, kbps.parse::<u32>()?))
        })
        .transpose()?;

    // The source of pictures: the live screen, or a generated one.
    let latest = Arc::new(Mutex::new(Latest::default()));
    let mut session = None;
    let (width, height) = if args.source == "synthetic" {
        (args.width, args.height)
    } else {
        let source = parse_source(&args.source)?;
        let handler_latest = Arc::clone(&latest);
        let started = capture::start(
            source,
            CaptureOptions {
                cursor: true,
                border: true,
                max_fps: Some(args.fps),
            },
            move |frame| {
                let mut latest = handler_latest.lock().unwrap();
                latest.width = frame.width;
                latest.height = frame.height;
                latest.bgra.clear();
                let row = frame.width as usize * 4;
                for line in frame.bgra.chunks(frame.stride).take(frame.height as usize) {
                    latest.bgra.extend_from_slice(&line[..row]);
                }
            },
        )?;
        let size = started.size();
        session = Some(started);
        let waited = Instant::now();
        while latest.lock().unwrap().bgra.is_empty() {
            if waited.elapsed() > Duration::from_secs(5) {
                bail!("no frame arrived in 5 s");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        size
    };

    let options = Options {
        format: Format {
            width,
            height,
            fps: args.fps,
        },
        rate: Rate {
            quality: args.quality,
            max_kbps: args.max_kbps,
        },
        keyframe_every: 0,
    };
    let created = telinhas_codec::create(backend, options)?;
    let mut encoder = created.encoder;
    let info = encoder.info();
    println!(
        "encoder: {} ({}), {width}x{height} at {} fps, quality {} ceiling {} kbps",
        info.name,
        if info.hardware {
            "hardware"
        } else {
            "software"
        },
        args.fps,
        args.quality,
        args.max_kbps
    );
    if let Some(reason) = created.fallback_reason {
        println!("WARNING: using the software encoder, the stream may stutter. Reason: {reason}");
    }

    let period = Duration::from_secs_f64(1.0 / f64::from(args.fps));
    let started = Instant::now();
    let mut next = started;
    let mut index = 0u64;
    let mut rng = 0x9E37_79B9u32;
    let mut picture = Vec::new();

    let mut second_bytes = 0u64;
    let mut second_packets = 0u32;
    let mut second_keys = 0u32;
    let mut second_encode_ms: Vec<f64> = Vec::new();
    let mut all_encode_ms: Vec<f64> = Vec::new();
    let mut per_second_kbps: Vec<f64> = Vec::new();
    let mut total_keys = 0u32;
    let mut next_report = started + Duration::from_secs(1);
    let mut changed = false;
    let mut keyed = false;
    let mut event_note = String::new();

    while started.elapsed() < Duration::from_secs(args.seconds) {
        let elapsed = started.elapsed().as_secs();
        if let Some((at, kbps)) = cap_change
            && !changed
            && elapsed >= at
        {
            changed = true;
            let before = Instant::now();
            encoder.set_max_kbps(kbps)?;
            event_note = format!(
                "[ceiling -> {kbps} kbps, took {:.1} ms]",
                before.elapsed().as_secs_f64() * 1000.0
            );
        }
        if let Some(at) = args.keyframe_at
            && !keyed
            && elapsed >= at
        {
            keyed = true;
            encoder.request_keyframe();
            event_note.push_str(" [keyframe requested]");
        }

        let encode_started;
        let packets = if args.source == "synthetic" {
            synthetic(width, height, index, args.noise, &mut rng, &mut picture);
            let frame = Frame {
                width,
                height,
                stride: width as usize * 4,
                bgra: &picture,
            };
            encode_started = Instant::now();
            encoder.encode(&frame)?
        } else {
            let latest = latest.lock().unwrap();
            let frame = Frame {
                width: latest.width,
                height: latest.height,
                stride: latest.width as usize * 4,
                bgra: &latest.bgra,
            };
            encode_started = Instant::now();
            encoder.encode(&frame)?
        };
        let took = encode_started.elapsed().as_secs_f64() * 1000.0;
        second_encode_ms.push(took);
        all_encode_ms.push(took);
        for packet in &packets {
            second_bytes += packet.data.len() as u64;
            second_packets += 1;
            if packet.keyframe {
                second_keys += 1;
                total_keys += 1;
            }
        }
        index += 1;

        if Instant::now() >= next_report {
            let kbps = second_bytes as f64 * 8.0 / 1000.0;
            per_second_kbps.push(kbps);
            println!(
                "{:>4}s: {kbps:>8.0} kbps | packets {second_packets:>3} keyframes {second_keys} | encode avg {:.2} max {:.2} ms {event_note}",
                per_second_kbps.len(),
                second_encode_ms.iter().sum::<f64>() / second_encode_ms.len().max(1) as f64,
                percentile(&second_encode_ms, 1.0),
            );
            second_bytes = 0;
            second_packets = 0;
            second_keys = 0;
            second_encode_ms.clear();
            event_note.clear();
            next_report += Duration::from_secs(1);
        }

        next += period;
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        } else if now - next > period * 5 {
            next = now;
        }
    }
    if let Some(session) = session {
        session.stop();
    }

    let steady = if per_second_kbps.len() > 3 {
        &per_second_kbps[2..]
    } else {
        &per_second_kbps[..]
    };
    println!(
        "FINAL: {} frames, {total_keys} keyframes | bitrate avg={:.0} p95={:.0} peak={:.0} kbps | encode time p50={:.2} p95={:.2} max={:.2} ms",
        index,
        steady.iter().sum::<f64>() / steady.len().max(1) as f64,
        percentile(steady, 0.95),
        percentile(steady, 1.0),
        percentile(&all_encode_ms, 0.5),
        percentile(&all_encode_ms, 0.95),
        percentile(&all_encode_ms, 1.0),
    );
    Ok(())
}
