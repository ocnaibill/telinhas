//! Windows audio capture through WASAPI process loopback.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use sysinfo::{Pid, ProcessesToUpdate, System};
use wasapi::{
    AudioClient, DeviceEnumerator, Direction, SampleType, SessionState, StreamMode, WaveFormat,
    initialize_mta,
};

use crate::audio::{
    AudioApp, AudioSource, AudioStats, CHANNELS, FRAME_SAMPLES, FrameSink, SAMPLE_RATE, TapStats,
};
use crate::{Error, Result};

const FRAME_LEN: usize = FRAME_SAMPLES * CHANNELS;
/// Audio queued beyond this is dropped from the front: two captures never run
/// at exactly the same speed, and a queue that only grows is delay.
const MAX_QUEUED: usize = SAMPLE_RATE as usize * CHANNELS / 5; // 200 ms
const KEEP_QUEUED: usize = SAMPLE_RATE as usize * CHANNELS * 3 / 50; // 60 ms
const FRAME_PERIOD: Duration = Duration::from_millis(20);
const DISCOVERY_PERIOD: Duration = Duration::from_millis(500);

fn os(error: impl std::fmt::Display) -> Error {
    Error::Os(error.to_string())
}

/// `chrome.exe` and `Chrome` name the same program.
fn normalize(name: &str) -> String {
    let lower = name.trim().to_lowercase();
    lower.strip_suffix(".exe").unwrap_or(&lower).to_owned()
}

fn processes() -> System {
    let mut system = System::new();
    system.refresh_processes(ProcessesToUpdate::All, true);
    system
}

fn process_name(system: &System, pid: u32) -> String {
    system
        .process(Pid::from_u32(pid))
        .map(|p| p.name().to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The process and everything above it, nearest first.
fn lineage(system: &System, pid: u32) -> Vec<u32> {
    let mut out = vec![pid];
    let mut at = Pid::from_u32(pid);
    while let Some(parent) = system.process(at).and_then(|p| p.parent()) {
        let parent_id = parent.as_u32();
        if out.contains(&parent_id) || out.len() > 32 {
            break;
        }
        out.push(parent_id);
        at = parent;
    }
    out
}

/// One audio session: which program, where it plays, whether it makes sound now.
struct Session {
    pid: u32,
    device: String,
    active: bool,
}

/// The audio sessions on every output device, not only the default one: a
/// virtual mixer (such as Fifine Genie) gives each kind of sound its own
/// output, and a program may play on any of them.
fn sessions() -> Result<Vec<Session>> {
    let enumerator = DeviceEnumerator::new().map_err(os)?;
    let devices = enumerator
        .get_device_collection(Direction::Render)
        .map_err(os)?;
    let mut out = Vec::new();
    for device in &devices {
        let Ok(device) = device else {
            continue;
        };
        let name = device.get_friendlyname().unwrap_or_default();
        let Ok(manager) = device.get_iaudiosessionmanager() else {
            continue;
        };
        let Ok(list) = manager.get_audiosessionenumerator() else {
            continue;
        };
        let Ok(count) = list.get_count() else {
            continue;
        };
        for i in 0..count {
            let Ok(control) = list.get_session(i) else {
                continue;
            };
            let Ok(state) = control.get_state() else {
                continue;
            };
            if state == SessionState::Expired {
                continue;
            }
            let Ok(pid) = control.get_process_id() else {
                continue;
            };
            // Process 0 is the system's own sounds, which cannot be targeted.
            if pid != 0 {
                out.push(Session {
                    pid,
                    device: name.clone(),
                    active: state == SessionState::Active,
                });
            }
        }
    }
    Ok(out)
}

pub fn list_apps() -> Result<Vec<AudioApp>> {
    initialize_mta().ok().map_err(os)?;
    let system = processes();
    let mut apps: Vec<AudioApp> = sessions()?
        .into_iter()
        .map(|session| AudioApp {
            pid: session.pid,
            process: process_name(&system, session.pid),
            device: session.device,
            active: session.active,
        })
        .collect();
    apps.sort_by(|a, b| a.process.cmp(&b.process).then(a.pid.cmp(&b.pid)));
    Ok(apps)
}

// ----- one capture ---------------------------------------------------------

#[derive(Default)]
struct Shared {
    packets: AtomicU64,
    frames: AtomicU64,
    peak_bits: AtomicU32,
    max_gap_ms: AtomicU32,
    dropped_frames: AtomicU64,
    /// The capture thread ended, for whatever reason.
    ended: AtomicBool,
}

struct Tap {
    label: String,
    pid: Option<u32>,
    queue: Arc<Mutex<VecDeque<f32>>>,
    shared: Arc<Shared>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Tap {
    fn stats(&self) -> TapStats {
        TapStats {
            label: self.label.clone(),
            packets: self.shared.packets.load(Ordering::Relaxed),
            frames: self.shared.frames.load(Ordering::Relaxed),
            peak: f32::from_bits(self.shared.peak_bits.load(Ordering::Relaxed)),
            max_gap_ms: self.shared.max_gap_ms.load(Ordering::Relaxed),
            dropped_frames: self.shared.dropped_frames.load(Ordering::Relaxed),
        }
    }
}

impl Drop for Tap {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// What a capture listens to.
#[derive(Clone, Copy)]
enum Target {
    /// Everything except this process and its children.
    SystemExceptUs,
    Include(u32),
    /// The default microphone.
    Microphone,
}

fn start_tap(target: Target, label: String) -> Result<Tap> {
    let queue: Arc<Mutex<VecDeque<f32>>> = Arc::default();
    let shared: Arc<Shared> = Arc::default();
    let stop = Arc::new(AtomicBool::new(false));
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<()>>();
    let thread = {
        let (queue, shared, stop) = (Arc::clone(&queue), Arc::clone(&shared), Arc::clone(&stop));
        std::thread::Builder::new()
            .name("telinhas-audio-tap".to_owned())
            .spawn(move || {
                let result = run_tap(target, &queue, &shared, &stop, &ready_tx);
                if let Err(error) = result {
                    let _ = ready_tx.send(Err(error));
                }
                shared.ended.store(true, Ordering::Relaxed);
            })
            .map_err(os)?
    };
    // Starting can fail (an old Windows, a process that is gone): say so now.
    match ready_rx.recv_timeout(Duration::from_secs(5)) {
        Ok(Ok(())) => {}
        Ok(Err(error)) => return Err(error),
        Err(_) => {
            return Err(Error::Os(
                "the audio capture did not start in 5 s".to_owned(),
            ));
        }
    }
    Ok(Tap {
        label,
        pid: match target {
            Target::Include(pid) => Some(pid),
            Target::SystemExceptUs | Target::Microphone => None,
        },
        queue,
        shared,
        stop,
        thread: Some(thread),
    })
}

fn run_tap(
    target: Target,
    queue: &Mutex<VecDeque<f32>>,
    shared: &Shared,
    stop: &AtomicBool,
    ready: &std::sync::mpsc::Sender<Result<()>>,
) -> Result<()> {
    initialize_mta().ok().map_err(os)?;
    let mut client = match target {
        Target::SystemExceptUs => {
            AudioClient::new_application_loopback_client(std::process::id(), false).map_err(os)?
        }
        Target::Include(pid) => {
            AudioClient::new_application_loopback_client(pid, true).map_err(os)?
        }
        Target::Microphone => {
            let enumerator = DeviceEnumerator::new().map_err(os)?;
            let device = enumerator
                .get_default_device(Direction::Capture)
                .map_err(os)?;
            device.get_iaudioclient().map_err(os)?
        }
    };
    let format = WaveFormat::new(
        32,
        32,
        SampleType::Float,
        SAMPLE_RATE as usize,
        CHANNELS,
        None,
    );
    let mode = StreamMode::EventsShared {
        autoconvert: true,
        buffer_duration_hns: 0,
    };
    client
        .initialize_client(&format, Direction::Capture, mode, None)
        .map_err(os)?;
    let event = client.set_get_eventhandle().map_err(os)?;
    let capture = client.get_audiocaptureclient().map_err(os)?;
    client.start_stream().map_err(os)?;
    let _ = ready.send(Ok(()));

    let mut bytes: VecDeque<u8> = VecDeque::new();
    let mut last_packet = Instant::now();
    while !stop.load(Ordering::Relaxed) {
        // A timeout is silence, not a failure.
        if event.wait_for_event(100).is_err() {
            continue;
        }
        loop {
            let frames = capture.get_next_packet_size().map_err(os)?.unwrap_or(0);
            if frames == 0 {
                break;
            }
            let info = capture.read_from_device_to_deque(&mut bytes).map_err(os)?;
            shared.packets.fetch_add(1, Ordering::Relaxed);
            shared
                .frames
                .fetch_add(u64::from(frames), Ordering::Relaxed);
            let gap = last_packet.elapsed().as_millis() as u32;
            shared.max_gap_ms.fetch_max(gap, Ordering::Relaxed);
            last_packet = Instant::now();
            // Windows marks packets of silence: there is nothing to queue, and
            // the mixer treats an empty queue as silence. Most captures of a
            // mix of many programs are silent most of the time.
            if info.flags.silent {
                bytes.clear();
                continue;
            }

            let raw = bytes.make_contiguous();
            let mut peak = f32::from_bits(shared.peak_bits.load(Ordering::Relaxed));
            let mut queue = queue.lock().unwrap();
            for chunk in raw.as_chunks::<4>().0 {
                let sample = f32::from_le_bytes(*chunk);
                peak = peak.max(sample.abs());
                queue.push_back(sample);
            }
            if queue.len() > MAX_QUEUED {
                let drop = queue.len() - KEEP_QUEUED;
                queue.drain(..drop);
                shared.dropped_frames.fetch_add(
                    (drop / CHANNELS) as u64 / FRAME_SAMPLES as u64 + 1,
                    Ordering::Relaxed,
                );
            }
            drop(queue);
            shared.peak_bits.store(peak.to_bits(), Ordering::Relaxed);
            bytes.clear();
        }
    }
    let _ = client.stop_stream();
    Ok(())
}

// ----- the mixer and the discovery of applications ----------------------------

pub struct Capture {
    taps: Arc<Mutex<Vec<Tap>>>,
    frames_out: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
}

impl Capture {
    pub fn stats(&self) -> AudioStats {
        AudioStats {
            taps: self.taps.lock().unwrap().iter().map(Tap::stats).collect(),
            frames_out: self.frames_out.load(Ordering::Relaxed),
        }
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
        self.taps.lock().unwrap().clear();
    }
}

pub fn start(source: Option<AudioSource>, mic: bool, mut on_frame: FrameSink) -> Result<Capture> {
    let taps: Arc<Mutex<Vec<Tap>>> = Arc::default();
    let stop = Arc::new(AtomicBool::new(false));
    let frames_out = Arc::new(AtomicU64::new(0));
    let mut threads = Vec::new();

    if mic {
        taps.lock()
            .unwrap()
            .push(start_tap(Target::Microphone, "microphone".to_owned())?);
    }
    match source {
        None => {}
        Some(AudioSource::Process { pid }) => {
            let label = format!("{} ({pid})", process_name(&processes(), pid));
            taps.lock()
                .unwrap()
                .push(start_tap(Target::Include(pid), label)?);
        }
        Some(AudioSource::System { exclude }) if exclude.is_empty() => {
            taps.lock()
                .unwrap()
                .push(start_tap(Target::SystemExceptUs, "system".to_owned())?);
        }
        Some(AudioSource::System { exclude }) => {
            // Windows cannot leave out several programs at once, so every other
            // program is captured on its own, as they appear and go.
            let (taps, stop) = (Arc::clone(&taps), Arc::clone(&stop));
            threads.push(
                std::thread::Builder::new()
                    .name("telinhas-audio-discovery".to_owned())
                    .spawn(move || discover(&exclude, &taps, &stop))
                    .map_err(os)?,
            );
        }
    }

    // The mixer: every 20 ms, one frame from whatever each capture has.
    {
        let (taps, stop, frames_out) = (
            Arc::clone(&taps),
            Arc::clone(&stop),
            Arc::clone(&frames_out),
        );
        threads.push(
            std::thread::Builder::new()
                .name("telinhas-audio-mixer".to_owned())
                .spawn(move || {
                    let mut frame = vec![0f32; FRAME_LEN];
                    let mut next = Instant::now();
                    while !stop.load(Ordering::Relaxed) {
                        frame.fill(0.0);
                        for tap in taps.lock().unwrap().iter() {
                            let mut queue = tap.queue.lock().unwrap();
                            let take = queue.len().min(FRAME_LEN);
                            for (out, sample) in frame.iter_mut().zip(queue.drain(..take)) {
                                *out += sample;
                            }
                        }
                        for sample in &mut frame {
                            *sample = sample.clamp(-1.0, 1.0);
                        }
                        on_frame(&frame);
                        frames_out.fetch_add(1, Ordering::Relaxed);
                        next += FRAME_PERIOD;
                        let now = Instant::now();
                        if next > now {
                            std::thread::sleep(next - now);
                        } else if now - next > FRAME_PERIOD * 5 {
                            next = now;
                        }
                    }
                })
                .map_err(os)?,
        );
    }
    Ok(Capture {
        taps,
        frames_out,
        stop,
        threads,
    })
}

/// Keeps one capture per program that is not excluded, following the programs
/// that start and stop making sound.
fn discover(exclude: &[String], taps: &Mutex<Vec<Tap>>, stop: &AtomicBool) {
    if initialize_mta().ok().is_err() {
        return;
    }
    let excluded: HashSet<String> = exclude.iter().map(|n| normalize(n)).collect();
    let own = std::process::id();
    let mut system = processes();
    let mut refreshed = Instant::now();
    while !stop.load(Ordering::Relaxed) {
        if let Ok(found) = sessions() {
            // Reading every process is the costly part: do it only when a
            // program shows up that is not in the table yet, or now and then
            // to follow the processes that ended.
            let unknown = found
                .iter()
                .any(|s| system.process(Pid::from_u32(s.pid)).is_none());
            if unknown || refreshed.elapsed() >= Duration::from_secs(5) {
                system = processes();
                refreshed = Instant::now();
            }
            let mut wanted: HashMap<u32, String> = HashMap::new();
            'session: for Session { pid, .. } in found {
                let lineage = lineage(&system, pid);
                // The app's own sound, and any program the person left out
                // (or one of its children).
                for id in &lineage {
                    if *id == own || excluded.contains(&normalize(&process_name(&system, *id))) {
                        continue 'session;
                    }
                }
                // Already covered by a capture of one of its parents.
                if lineage[1..].iter().any(|id| wanted.contains_key(id)) {
                    continue;
                }
                // A parent that has a session replaces its children's.
                wanted.retain(|other, _| !lineage_contains(&system, *other, pid));
                wanted.insert(pid, format!("{} ({pid})", process_name(&system, pid)));
            }

            let mut taps = taps.lock().unwrap();
            // Captures whose program is gone, or no longer wanted, or whose
            // thread ended on an error.
            // (The microphone has no pid: it is not ours to manage here.)
            taps.retain(|tap| {
                tap.pid.is_none_or(|pid| wanted.contains_key(&pid))
                    && !tap.shared.ended.load(Ordering::Relaxed)
            });
            for (pid, label) in wanted {
                if taps.iter().any(|tap| tap.pid == Some(pid)) {
                    continue;
                }
                // A program that cannot be captured is skipped, not fatal.
                if let Ok(tap) = start_tap(Target::Include(pid), label) {
                    taps.push(tap);
                }
            }
        }
        std::thread::sleep(DISCOVERY_PERIOD);
    }
}

fn lineage_contains(system: &System, pid: u32, ancestor: u32) -> bool {
    lineage(system, pid)[1..].contains(&ancestor)
}
