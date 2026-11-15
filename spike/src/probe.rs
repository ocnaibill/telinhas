//! `probe`: what can this machine capture, and which H.264 encoders really
//! open? Feeds the decisions of the M3 plan (`docs/PLANO-M3.md`).
//!
//! Encoders are tested through the `ffmpeg` command line, so the probe needs
//! no FFmpeg libraries to build. Only a working `ffmpeg` on the PATH.

use std::process::Command;
use std::time::Instant;

use telinhas_capture as capture;

/// Frames encoded per test; enough to get past start-up costs.
const TEST_FRAMES: u32 = 300;

pub fn run() {
    capture::init_dpi_awareness();
    println!("== system");
    system();
    println!("\n== capture API");
    match capture::capture_support() {
        Ok(support) => {
            println!("graphics capture : {}", yes_no(support.graphics_capture));
            println!("border can be off: {}", yes_no(support.border_toggle));
            println!("cursor toggle    : {}", yes_no(support.cursor_toggle));
        }
        Err(error) => println!("not available: {error}"),
    }
    println!("\n== GPUs");
    match capture::list_gpus() {
        Ok(gpus) => {
            for gpu in gpus {
                println!("{} ({:?}, {} MB)", gpu.name, gpu.vendor, gpu.vram_mb);
            }
        }
        Err(error) => println!("not available: {error}"),
    }
    println!("\n== monitors");
    match capture::list_monitors() {
        Ok(monitors) => {
            for m in monitors {
                println!(
                    "#{} {} {}x{} @ {} Hz at ({},{}){}",
                    m.index,
                    m.device,
                    m.width,
                    m.height,
                    m.refresh_hz,
                    m.x,
                    m.y,
                    if m.primary { " [primary]" } else { "" }
                );
            }
        }
        Err(error) => println!("not available: {error}"),
    }
    println!("\n== windows");
    match capture::list_windows() {
        Ok(windows) => {
            for w in windows {
                println!(
                    "{:>10} {:>5}x{:<5} {:<22} {}{}",
                    w.handle,
                    w.width,
                    w.height,
                    w.process,
                    truncate(&w.title, 60),
                    if w.minimized { " [minimized]" } else { "" }
                );
            }
        }
        Err(error) => println!("not available: {error}"),
    }
    println!("\n== encoders (through ffmpeg)");
    encoders();
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_owned()
    } else {
        let head: String = text.chars().take(max - 1).collect();
        format!("{head}…")
    }
}

fn system() {
    println!("os: {} {}", std::env::consts::OS, std::env::consts::ARCH);
    if cfg!(windows)
        && let Ok(out) = Command::new("cmd").args(["/C", "ver"]).output()
    {
        println!("{}", String::from_utf8_lossy(&out.stdout).trim());
    }
    // The driver version matters for NVENC.
    if let Ok(out) = Command::new("nvidia-smi")
        .args(["--query-gpu=name,driver_version", "--format=csv,noheader"])
        .output()
        && out.status.success()
    {
        println!(
            "nvidia-smi: {}",
            String::from_utf8_lossy(&out.stdout).trim()
        );
    }
}

struct Candidate {
    name: &'static str,
    /// Low-latency options, as the app would use them.
    options: &'static [&'static str],
}

const CANDIDATES: &[Candidate] = &[
    Candidate {
        name: "h264_nvenc",
        options: &[
            "-preset", "p1", "-tune", "ll", "-bf", "0", "-rc", "cbr", "-b:v", "8M",
        ],
    },
    Candidate {
        name: "h264_qsv",
        options: &[
            "-preset",
            "veryfast",
            "-look_ahead",
            "0",
            "-bf",
            "0",
            "-b:v",
            "8M",
        ],
    },
    Candidate {
        name: "h264_amf",
        options: &[
            "-usage",
            "ultralowlatency",
            "-bf",
            "0",
            "-rc",
            "cbr",
            "-b:v",
            "8M",
        ],
    },
    Candidate {
        name: "h264_mf",
        options: &["-b:v", "8M"],
    },
];

fn encoders() {
    let version = Command::new("ffmpeg")
        .args(["-hide_banner", "-version"])
        .output();
    let Ok(version) = version else {
        println!("ffmpeg was not found on the PATH.");
        println!(
            "Install it (for example `winget install Gyan.FFmpeg`), open a new terminal and run again."
        );
        return;
    };
    let first_line = String::from_utf8_lossy(&version.stdout)
        .lines()
        .next()
        .unwrap_or_default()
        .to_owned();
    println!("{first_line}");

    let listed = Command::new("ffmpeg")
        .args(["-hide_banner", "-encoders"])
        .output()
        .map(|out| String::from_utf8_lossy(&out.stdout).into_owned())
        .unwrap_or_default();

    for candidate in CANDIDATES {
        if !listed.contains(candidate.name) {
            println!("{:<11} not built into this ffmpeg", candidate.name);
            continue;
        }
        for (width, height, fps) in [(1920, 1080, 60), (2560, 1440, 60)] {
            print!("{:<11} {width}x{height}@{fps}: ", candidate.name);
            match test_encode(candidate, width, height, fps) {
                Ok(speed) => println!("works, encodes {speed:.0} fps"),
                Err(reason) => println!("FAILED: {reason}"),
            }
        }
    }
}

/// Encodes a synthetic picture and returns how many frames per second the
/// encoder sustained, or the last lines of ffmpeg's complaint.
fn test_encode(candidate: &Candidate, width: u32, height: u32, fps: u32) -> Result<f64, String> {
    let source = format!("testsrc2=size={width}x{height}:rate={fps}");
    let started = Instant::now();
    let out = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            &source,
        ])
        .args(["-frames:v", &TEST_FRAMES.to_string(), "-pix_fmt", "yuv420p"])
        .args(["-c:v", candidate.name])
        .args(candidate.options)
        .args(["-f", "null", "-"])
        .output()
        .map_err(|error| error.to_string())?;
    let elapsed = started.elapsed().as_secs_f64();
    if out.status.success() {
        Ok(f64::from(TEST_FRAMES) / elapsed)
    } else {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let tail: Vec<&str> = stderr.lines().rev().take(2).collect();
        Err(tail.into_iter().rev().collect::<Vec<_>>().join(" | "))
    }
}
