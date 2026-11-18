//! M5 spike: audio capture on Windows. Answers the questions of
//! `docs/PLANO-M5.md`: can one program be captured, can the system be captured
//! without several programs, and does a program that starts making sound in
//! the middle get picked up.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use telinhas_capture::audio::{self, AudioSource, CHANNELS, SAMPLE_RATE};

pub fn run(mode: &str, seconds: u64, exclude: &[String], mic: bool, out: &str) -> Result<()> {
    if mode == "list" {
        let apps = audio::list_audio_apps().context("listing the audio sessions")?;
        println!(
            "{} program(s) with an audio session on any output device:",
            apps.len()
        );
        for app in apps {
            println!(
                "  pid {:>6}  {:<26} {:<8} on {}",
                app.pid,
                app.process,
                if app.active { "playing" } else { "quiet" },
                app.device
            );
        }
        return Ok(());
    }
    let source = match mode {
        "system" => AudioSource::System {
            exclude: exclude.to_vec(),
        },
        other => match other.strip_prefix("app:").and_then(|pid| pid.parse().ok()) {
            Some(pid) => AudioSource::Process { pid },
            None => bail!("mode must be `list`, `system` or `app:PID`, not {other:?}"),
        },
    };

    // The samples go to memory first, so writing the file never delays a frame.
    let samples: Arc<Mutex<Vec<f32>>> = Arc::default();
    let sink = Arc::clone(&samples);
    let capture = audio::start_audio(Some(source), mic, move |frame| {
        sink.lock().unwrap().extend_from_slice(frame);
    })
    .context(
        "starting the audio capture (Windows 10 version 2004 or newer, or PipeWire on Linux)",
    )?;
    println!("capturing {mode} for {seconds} s; play something now");
    if !exclude.is_empty() {
        println!("leaving out: {}", exclude.join(", "));
    }

    let cpu_before = telinhas_capture::process_cpu_seconds();
    let started = Instant::now();
    let mut printed = 0;
    while started.elapsed() < Duration::from_secs(seconds) {
        std::thread::sleep(Duration::from_millis(250));
        let elapsed = started.elapsed().as_secs();
        if elapsed >= printed + 2 {
            printed = elapsed;
            let stats = capture.stats();
            println!(
                "  {elapsed:>3} s  frames out {}  captures: {}",
                stats.frames_out,
                stats
                    .taps
                    .iter()
                    .map(|t| format!("{} peak {:.2}", t.label, t.peak))
                    .collect::<Vec<_>>()
                    .join(" | ")
            );
        }
    }
    let stats = capture.stats();
    let cpu_after = telinhas_capture::process_cpu_seconds();
    let wall = started.elapsed().as_secs_f64();
    drop(capture);

    if let (Some(before), Some(after)) = (cpu_before, cpu_after) {
        println!(
            "\nCPU used by the capture and mix: {:.1}% of one core",
            (after - before) / wall * 100.0
        );
    }
    let expected = seconds * 50;
    println!(
        "\n20 ms frames delivered: {} (expected about {expected})",
        stats.frames_out
    );
    println!("{} capture(s) were running at the end:", stats.taps.len());
    for tap in &stats.taps {
        println!(
            "  {:<32} packets {:>6}  frames {:>9}  peak {:.3}  longest silence {} ms  dropped {}",
            tap.label, tap.packets, tap.frames, tap.peak, tap.max_gap_ms, tap.dropped_frames
        );
    }

    let samples = samples.lock().unwrap();
    write_wav(out, &samples).with_context(|| format!("writing {out}"))?;
    println!(
        "wrote {out} ({:.1} s)",
        samples.len() as f64 / (SAMPLE_RATE as f64 * CHANNELS as f64)
    );
    Ok(())
}

/// A 32-bit float WAV, which every player opens.
fn write_wav(path: &str, samples: &[f32]) -> Result<()> {
    let mut file = BufWriter::new(File::create(path)?);
    let data_len = (samples.len() * 4) as u32;
    let channels = CHANNELS as u16;
    file.write_all(b"RIFF")?;
    file.write_all(&(36 + data_len).to_le_bytes())?;
    file.write_all(b"WAVEfmt ")?;
    file.write_all(&16u32.to_le_bytes())?;
    file.write_all(&3u16.to_le_bytes())?; // IEEE float
    file.write_all(&channels.to_le_bytes())?;
    file.write_all(&SAMPLE_RATE.to_le_bytes())?;
    file.write_all(&(SAMPLE_RATE * u32::from(channels) * 4).to_le_bytes())?;
    file.write_all(&(channels * 4).to_le_bytes())?;
    file.write_all(&32u16.to_le_bytes())?;
    file.write_all(b"data")?;
    file.write_all(&data_len.to_le_bytes())?;
    for sample in samples {
        file.write_all(&sample.to_le_bytes())?;
    }
    file.flush()?;
    Ok(())
}
