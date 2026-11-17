//! `capture`: captures a monitor or window and measures how well it does,
//! without encoding anything (M3b in `docs/PLANO-M3.md`).

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow, bail};
use telinhas_capture::{self as capture, Options, Source};

#[derive(Default)]
struct Acc {
    frames: u64,
    last_micros: Option<i64>,
    /// Gaps between consecutive frames as the system timestamped them.
    intervals_ms: Vec<f64>,
    copies_ms: Vec<f64>,
    window_intervals_ms: Vec<f64>,
    window_copies_ms: Vec<f64>,
    window_frames: u64,
    size: (u32, u32),
}

fn percentile(values: &mut [f64], q: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(f64::total_cmp);
    values[((values.len() - 1) as f64 * q).round() as usize]
}

fn mean(values: &[f64]) -> f64 {
    if values.is_empty() {
        0.0
    } else {
        values.iter().sum::<f64>() / values.len() as f64
    }
}

/// `monitor:N`, `window:HANDLE` or `window:TEXT` (matches the title or the
/// program name, ignoring case).
pub fn parse_source(text: &str) -> Result<Source> {
    let (kind, value) = text
        .split_once(':')
        .ok_or_else(|| anyhow!("use monitor:N or window:HANDLE-OR-TEXT"))?;
    match kind {
        "monitor" => Ok(Source::Monitor(value.parse()?)),
        "window" => {
            if let Ok(handle) = value.parse::<isize>() {
                return Ok(Source::Window(handle));
            }
            let wanted = value.to_lowercase();
            let matches: Vec<_> = capture::list_windows()?
                .into_iter()
                .filter(|w| {
                    w.title.to_lowercase().contains(&wanted)
                        || w.process.to_lowercase().contains(&wanted)
                })
                .collect();
            match matches.as_slice() {
                [] => bail!("no window matches {value:?}; run `probe` to see them"),
                [one] => {
                    println!(
                        "window: {} ({}x{}) {}",
                        one.process, one.width, one.height, one.title
                    );
                    Ok(Source::Window(one.handle))
                }
                many => {
                    for w in many {
                        println!("{:>10} {} {}", w.handle, w.process, w.title);
                    }
                    bail!("{} windows match {value:?}; use the handle", many.len())
                }
            }
        }
        _ => bail!("unknown source kind {kind:?}"),
    }
}

pub fn run(
    source: &str,
    seconds: u64,
    cursor: bool,
    border: bool,
    max_fps: Option<u32>,
) -> Result<()> {
    capture::init_dpi_awareness();
    let source = parse_source(source)?;
    let acc = Arc::new(Mutex::new(Acc::default()));
    let handler_acc = Arc::clone(&acc);

    let session = capture::start(
        source,
        Options {
            cursor,
            border,
            max_fps,
        },
        move |frame| {
            let mut acc = handler_acc.lock().unwrap();
            acc.size = (frame.width, frame.height);
            acc.frames += 1;
            acc.window_frames += 1;
            let copy_ms = f64::from(frame.copy_micros) / 1000.0;
            acc.copies_ms.push(copy_ms);
            acc.window_copies_ms.push(copy_ms);
            if let Some(last) = acc.last_micros {
                let gap = (frame.system_micros - last) as f64 / 1000.0;
                acc.intervals_ms.push(gap);
                acc.window_intervals_ms.push(gap);
            }
            acc.last_micros = Some(frame.system_micros);
        },
    )?;

    let (width, height) = session.size();
    println!("capturing {source:?} at {width}x{height}");
    println!(
        "cursor: {}",
        if session.cursor_applied() {
            "setting accepted"
        } else {
            "setting NOT accepted"
        }
    );
    println!("border: {}", session.border_status());
    println!("frame rate cap: {}", session.max_fps_status());

    let started = Instant::now();
    let cpu_start = capture::process_cpu_seconds().unwrap_or(0.0);
    let mut last_tick = Instant::now();
    let mut last_cpu = cpu_start;
    while started.elapsed() < Duration::from_secs(seconds) {
        std::thread::sleep(Duration::from_secs(2));
        let now = Instant::now();
        let cpu_now = capture::process_cpu_seconds().unwrap_or(0.0);
        let wall = (now - last_tick).as_secs_f64();
        let mut acc = acc.lock().unwrap();
        let mut gaps = std::mem::take(&mut acc.window_intervals_ms);
        let copies = std::mem::take(&mut acc.window_copies_ms);
        let frames = std::mem::take(&mut acc.window_frames);
        println!(
            "{:>6.1} fps | frame gap p50={:.1} p95={:.1} max={:.1} ms | copy avg={:.2} ms | cpu {:.0}% of a core | {}x{}",
            frames as f64 / wall,
            percentile(&mut gaps, 0.5),
            percentile(&mut gaps, 0.95),
            percentile(&mut gaps, 1.0),
            mean(&copies),
            (cpu_now - last_cpu) / wall * 100.0,
            acc.size.0,
            acc.size.1,
        );
        last_tick = now;
        last_cpu = cpu_now;
    }
    let paced_out = session.frames_paced_out();
    session.stop();

    let wall = started.elapsed().as_secs_f64();
    let cpu = capture::process_cpu_seconds().unwrap_or(0.0) - cpu_start;
    let mut acc = acc.lock().unwrap();
    let frames = acc.frames;
    let mut gaps = std::mem::take(&mut acc.intervals_ms);
    let mut copies = std::mem::take(&mut acc.copies_ms);
    println!(
        "FINAL: {frames} frames in {wall:.1}s = {:.1} fps | frame gap p50={:.1} p95={:.1} max={:.1} ms | copy avg={:.2} p95={:.2} ms | cpu {:.0}% of a core",
        frames as f64 / wall,
        percentile(&mut gaps, 0.5),
        percentile(&mut gaps, 0.95),
        percentile(&mut gaps, 1.0),
        mean(&copies),
        percentile(&mut copies, 0.95),
        cpu / wall * 100.0,
    );
    if max_fps.is_some() {
        println!(
            "pacer left out {paced_out} of {} frames the system produced",
            frames + paced_out
        );
    }
    Ok(())
}
