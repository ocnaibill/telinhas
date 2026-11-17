//! `measure`: captures the real screen, feeds it to the NVENC encoder (through
//! the `ffmpeg` command line) at a steady frame rate, and reports the bitrate
//! the content needs. This is M3e in `docs/PLANO-M3.md`: the real numbers that
//! replace the "about 8 Mbps per viewer" guess.
//!
//! The ffmpeg pipe is for measuring only. The app will encode through the
//! FFmpeg libraries, with the picture staying on the GPU.

use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use telinhas_capture::{self as capture, Options};

use crate::capture_cmd::parse_source;

/// The newest captured picture, as tightly packed BGRA.
#[derive(Default)]
struct Latest {
    width: u32,
    height: u32,
    bgra: Vec<u8>,
}

pub struct Args {
    pub source: String,
    pub seconds: u64,
    pub fps: u32,
    /// `cq:N` (constant quality: what the content asks for) or `cbr:KBPS`.
    pub rate: String,
    pub scale: Option<(u32, u32)>,
    pub cursor: bool,
}

fn rate_args(rate: &str) -> Result<Vec<String>> {
    let (kind, value) = rate.split_once(':').context("use cq:N or cbr:KBPS")?;
    let number: u32 = value.parse().context("the value must be a number")?;
    Ok(match kind {
        "cq" => vec![
            "-rc".into(),
            "vbr".into(),
            "-cq".into(),
            number.to_string(),
            "-b:v".into(),
            "0".into(),
        ],
        "cbr" => vec![
            "-rc".into(),
            "cbr".into(),
            "-b:v".into(),
            format!("{number}k"),
        ],
        other => bail!("unknown rate control {other:?}; use cq or cbr"),
    })
}

fn percentile(values: &[f64], q: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    sorted[((sorted.len() - 1) as f64 * q).round() as usize]
}

fn nvidia_snapshot() -> String {
    Command::new("nvidia-smi")
        .args([
            "--query-gpu=utilization.gpu,encoder.stats.averageFps,encoder.stats.averageLatency",
            "--format=csv,noheader",
        ])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_owned())
        .unwrap_or_else(|| "unavailable".to_owned())
}

pub fn run(args: &Args) -> Result<()> {
    capture::init_dpi_awareness();
    let source = parse_source(&args.source)?;

    let latest = Arc::new(Mutex::new(Latest::default()));
    let new_frames = Arc::new(AtomicU64::new(0));
    let (handler_latest, handler_new) = (Arc::clone(&latest), Arc::clone(&new_frames));
    let session = capture::start(
        source,
        Options {
            cursor: args.cursor,
            border: true,
            max_fps: Some(args.fps),
        },
        move |frame| {
            let mut latest = handler_latest.lock().unwrap();
            let row = frame.width as usize * 4;
            latest.width = frame.width;
            latest.height = frame.height;
            latest.bgra.clear();
            if frame.stride == row {
                latest
                    .bgra
                    .extend_from_slice(&frame.bgra[..row * frame.height as usize]);
            } else {
                for line in frame.bgra.chunks(frame.stride).take(frame.height as usize) {
                    latest.bgra.extend_from_slice(&line[..row]);
                }
            }
            handler_new.fetch_add(1, Ordering::Relaxed);
        },
    )?;
    let (width, height) = session.size();
    println!(
        "capturing {source:?} at {width}x{height}, encoding at {} fps",
        args.fps
    );

    // Wait for the first picture, so ffmpeg is started with the real size.
    let waited = Instant::now();
    while new_frames.load(Ordering::Relaxed) == 0 {
        if waited.elapsed() > Duration::from_secs(5) {
            bail!("no frame arrived in 5 s: the screen is still. Move something and run again");
        }
        std::thread::sleep(Duration::from_millis(20));
    }

    let mut command = Command::new("ffmpeg");
    command
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "bgra",
        ])
        .args(["-video_size", &format!("{width}x{height}")])
        .args(["-framerate", &args.fps.to_string(), "-i", "pipe:0"]);
    if let Some((w, h)) = args.scale {
        command.args(["-vf", &format!("scale={w}:{h}:flags=fast_bilinear")]);
    }
    command
        .args([
            "-c:v",
            "h264_nvenc",
            "-preset",
            "p1",
            "-tune",
            "ll",
            "-bf",
            "0",
        ])
        .args(["-g", &(args.fps * 2).to_string()])
        .args(rate_args(&args.rate)?)
        .args(["-f", "h264", "pipe:1"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .context("could not start ffmpeg; is it on the PATH?")?;
    let mut stdin = child.stdin.take().context("no stdin")?;
    let mut stdout = child.stdout.take().context("no stdout")?;
    let mut stderr = child.stderr.take().context("no stderr")?;

    let bytes_out = Arc::new(AtomicU64::new(0));
    let reader = {
        let bytes_out = Arc::clone(&bytes_out);
        std::thread::spawn(move || {
            let mut buffer = vec![0u8; 64 * 1024];
            while let Ok(n) = stdout.read(&mut buffer) {
                if n == 0 {
                    break;
                }
                bytes_out.fetch_add(n as u64, Ordering::Relaxed);
            }
        })
    };
    let errors = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stderr.read_to_string(&mut text);
        text
    });

    // Feeds the encoder at a fixed rate, repeating the last picture when the
    // screen did not change, as the real sender will have to.
    let stop = Arc::new(AtomicBool::new(false));
    let fed = Arc::new(AtomicU64::new(0));
    let feeder = {
        let (stop, fed, latest) = (Arc::clone(&stop), Arc::clone(&fed), Arc::clone(&latest));
        let period = Duration::from_secs_f64(1.0 / f64::from(args.fps));
        std::thread::spawn(move || {
            let mut next = Instant::now();
            let mut scratch: Vec<u8> = Vec::new();
            while !stop.load(Ordering::Relaxed) {
                {
                    let latest = latest.lock().unwrap();
                    scratch.clear();
                    scratch.extend_from_slice(&latest.bgra);
                }
                if stdin.write_all(&scratch).is_err() {
                    break;
                }
                fed.fetch_add(1, Ordering::Relaxed);
                next += period;
                let now = Instant::now();
                if next > now {
                    std::thread::sleep(next - now);
                } else if now - next > period * 5 {
                    next = now;
                }
            }
        })
    };

    let cpu_start = capture::process_cpu_seconds().unwrap_or(0.0);
    let started = Instant::now();
    let mut series_kbps: Vec<f64> = Vec::new();
    let mut last_bytes = 0u64;
    let mut last_new = 0u64;
    let mut gpu_mid = String::new();
    for second in 1..=args.seconds {
        std::thread::sleep(Duration::from_secs(1));
        let bytes = bytes_out.load(Ordering::Relaxed);
        let new = new_frames.load(Ordering::Relaxed);
        let kbps = (bytes - last_bytes) as f64 * 8.0 / 1000.0;
        series_kbps.push(kbps);
        println!(
            "{second:>4}s: {kbps:>8.0} kbps | new pictures {:>3}/s | fed {} total",
            new - last_new,
            fed.load(Ordering::Relaxed)
        );
        last_bytes = bytes;
        last_new = new;
        if second == args.seconds / 2 {
            gpu_mid = nvidia_snapshot();
        }
    }
    let wall = started.elapsed().as_secs_f64();
    let cpu = capture::process_cpu_seconds().unwrap_or(0.0) - cpu_start;

    stop.store(true, Ordering::Relaxed);
    session.stop();
    let _ = feeder.join();
    let _ = child.wait();
    let _ = reader.join();
    let complaints = errors.join().unwrap_or_default();

    let steady = if series_kbps.len() > 3 {
        &series_kbps[2..]
    } else {
        &series_kbps[..]
    };
    let average = steady.iter().sum::<f64>() / steady.len().max(1) as f64;
    println!(
        "FINAL {} at {} fps, {}: steady bitrate avg={:.0} kbps p50={:.0} p95={:.0} peak={:.0} kbps",
        args.source,
        args.fps,
        args.rate,
        average,
        percentile(steady, 0.5),
        percentile(steady, 0.95),
        percentile(steady, 1.0),
    );
    println!(
        "total {:.1} MB in {wall:.0}s; fed {} pictures ({:.1}/s), {} were new; our process cpu {:.0}% of a core",
        bytes_out.load(Ordering::Relaxed) as f64 / 1e6,
        fed.load(Ordering::Relaxed),
        fed.load(Ordering::Relaxed) as f64 / wall,
        new_frames.load(Ordering::Relaxed),
        cpu / wall * 100.0,
    );
    println!("gpu (mid-run: utilization %, encoder avg fps, avg latency us): {gpu_mid}");
    if !complaints.trim().is_empty() {
        println!("ffmpeg said:\n{}", complaints.trim());
    }
    Ok(())
}
