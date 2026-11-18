//! Linux audio capture through PipeWire.
//!
//! Every program that plays sound is a node of class `Stream/Output/Audio`.
//! A capture stream aimed at such a node (`target.object`) receives what that
//! program plays, without taking it away from the speakers. That gives the
//! same design as on Windows (DEC-16, DEC-41): one capture per program that is
//! wanted, mixed together, following the programs that start and stop.
//!
//! - [`AudioSource::Process`]: the programs that are that process or were
//!   started by it.
//! - [`AudioSource::System`]: every program except Telinhas itself and the
//!   ones named in `exclude`.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use pipewire as pw;
use pw::properties::properties;
use pw::spa;

use crate::audio::{
    AudioApp, AudioSource, AudioStats, CHANNELS, FRAME_SAMPLES, FrameSink, SAMPLE_RATE, TapStats,
};
use crate::{Error, Result};

const FRAME_LEN: usize = FRAME_SAMPLES * CHANNELS;
/// Audio queued beyond this is dropped from the front: two streams never run
/// at exactly the same speed, and a queue that only grows is delay.
const MAX_QUEUED: usize = SAMPLE_RATE as usize * CHANNELS / 5; // 200 ms
const KEEP_QUEUED: usize = SAMPLE_RATE as usize * CHANNELS * 3 / 50; // 60 ms
const FRAME_PERIOD: Duration = Duration::from_millis(20);
const APP_CLASS: &str = "Stream/Output/Audio";

fn os(error: impl std::fmt::Display) -> Error {
    Error::Os(error.to_string())
}

// ----- who plays what ----------------------------------------------------------

/// A program that plays sound, as PipeWire describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Player {
    /// The node's object id (what `global_remove` reports).
    id: u32,
    /// What `target.object` takes: the serial, which never repeats.
    serial: String,
    /// The process that owns the stream, if the client said so (a sandbox may not).
    pid: Option<u32>,
    /// `application.process.binary`, `application.name`, then `node.name`.
    names: Vec<String>,
}

impl Player {
    fn from_props(id: u32, props: &spa::utils::dict::DictRef) -> Option<Self> {
        if props.get(*pw::keys::MEDIA_CLASS) != Some(APP_CLASS) {
            return None;
        }
        let mut names = Vec::new();
        for key in [
            *pw::keys::APP_PROCESS_BINARY,
            *pw::keys::APP_NAME,
            *pw::keys::NODE_NAME,
        ] {
            if let Some(value) = props.get(key) {
                names.push(value.to_owned());
            }
        }
        Some(Self {
            id,
            serial: props
                .get("object.serial")
                .map_or_else(|| id.to_string(), str::to_owned),
            pid: props
                .get(*pw::keys::APP_PROCESS_ID)
                .and_then(|pid| pid.parse().ok()),
            names,
        })
    }

    fn label(&self) -> String {
        let name = self.names.first().map_or("?", String::as_str);
        match self.pid {
            Some(pid) => format!("{name} ({pid})"),
            None => name.to_owned(),
        }
    }
}

/// `Discord`, `discord` and `discord.exe` name the same program.
fn normalize(name: &str) -> String {
    let lower = name.trim().to_lowercase();
    lower.strip_suffix(".exe").unwrap_or(&lower).to_owned()
}

/// The parent and the name of a process, from `/proc`.
trait Processes {
    fn parent(&self, pid: u32) -> Option<u32>;
    fn name(&self, pid: u32) -> Option<String>;
}

struct Proc;

impl Processes for Proc {
    fn parent(&self, pid: u32) -> Option<u32> {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        parent_from_stat(&stat)
    }

    fn name(&self, pid: u32) -> Option<String> {
        let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
        Some(comm.trim().to_owned())
    }
}

/// `/proc/PID/stat` is `pid (name) state ppid ...`, and the name may hold
/// spaces and parentheses, so the fields start after the last `)`.
fn parent_from_stat(stat: &str) -> Option<u32> {
    let rest = &stat[stat.rfind(')')? + 1..];
    rest.split_whitespace().nth(1)?.parse().ok()
}

/// The process and everything above it, nearest first.
fn lineage(processes: &dyn Processes, pid: u32) -> Vec<u32> {
    let mut out = vec![pid];
    let mut at = pid;
    while let Some(parent) = processes.parent(at) {
        if parent == 0 || out.contains(&parent) || out.len() > 32 {
            break;
        }
        out.push(parent);
        at = parent;
    }
    out
}

/// Whether a capture of this program is wanted.
fn wanted(source: &AudioSource, player: &Player, own_pid: u32, processes: &dyn Processes) -> bool {
    let chain = player.pid.map(|pid| lineage(processes, pid));
    let chain_names = || -> Vec<String> {
        chain
            .iter()
            .flatten()
            .filter_map(|pid| processes.name(*pid))
            .map(|name| normalize(&name))
            .collect()
    };
    match source {
        AudioSource::Process { pid } => chain.as_ref().is_some_and(|chain| chain.contains(pid)),
        AudioSource::System { exclude } => {
            // The app's own sound would loop back to the people who listen.
            if chain.as_ref().is_some_and(|chain| chain.contains(&own_pid)) {
                return false;
            }
            if exclude.is_empty() {
                return true;
            }
            let excluded: Vec<String> = exclude.iter().map(|name| normalize(name)).collect();
            let own_names = player.names.iter().map(|name| normalize(name));
            let all_names: Vec<String> = own_names.chain(chain_names()).collect();
            !all_names.iter().any(|name| excluded.contains(name))
        }
    }
}

// ----- one capture stream --------------------------------------------------------

#[derive(Default)]
struct Shared {
    packets: AtomicU64,
    frames: AtomicU64,
    peak_bits: AtomicU32,
    dropped_frames: AtomicU64,
}

/// What the mixer reads from a stream, and what the statistics show.
struct Tap {
    label: String,
    /// The node it listens to; the microphone has none.
    node: Option<u32>,
    queue: Arc<Mutex<VecDeque<f32>>>,
    shared: Arc<Shared>,
}

impl Tap {
    fn stats(&self) -> TapStats {
        TapStats {
            label: self.label.clone(),
            packets: self.shared.packets.load(Ordering::Relaxed),
            frames: self.shared.frames.load(Ordering::Relaxed),
            peak: f32::from_bits(self.shared.peak_bits.load(Ordering::Relaxed)),
            // PipeWire delivers a buffer on every cycle, silent or not.
            max_gap_ms: 0,
            dropped_frames: self.shared.dropped_frames.load(Ordering::Relaxed),
        }
    }
}

/// What the stream's `process` callback needs.
struct Feed {
    queue: Arc<Mutex<VecDeque<f32>>>,
    shared: Arc<Shared>,
    channels: usize,
}

type Taps = Arc<Mutex<Vec<Tap>>>;

/// A running stream and the listener that keeps its callbacks alive.
struct Running {
    stream: pw::stream::StreamRc,
    _listener: pw::stream::StreamListener<Feed>,
}

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.stream.disconnect();
    }
}

/// The format request: 32-bit float, 48 kHz, stereo. The stream converts
/// whatever the program plays.
fn format_request() -> Result<Vec<u8>> {
    let mut info = spa::param::audio::AudioInfoRaw::new();
    info.set_format(spa::param::audio::AudioFormat::F32LE);
    info.set_rate(SAMPLE_RATE);
    info.set_channels(CHANNELS as u32);
    let mut position = [0u32; spa::sys::SPA_AUDIO_MAX_CHANNELS as usize];
    position[0] = spa::sys::SPA_AUDIO_CHANNEL_FL;
    position[1] = spa::sys::SPA_AUDIO_CHANNEL_FR;
    info.set_position(position);
    let object = spa::pod::Object {
        type_: spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
        id: spa::param::ParamType::EnumFormat.as_raw(),
        properties: info.into(),
    };
    spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &spa::pod::Value::Object(object),
    )
    .map(|(cursor, _)| cursor.into_inner())
    .map_err(os)
}

/// Opens a capture stream on `target` (a node serial), or on the default
/// microphone when it is `None`, and registers its tap.
fn open_stream(
    core: &pw::core::CoreRc,
    taps: &Taps,
    target: Option<&Player>,
    label: String,
) -> Result<Running> {
    let mut props = properties! {
        *pw::keys::MEDIA_TYPE => "Audio",
        *pw::keys::MEDIA_CATEGORY => "Capture",
        *pw::keys::MEDIA_ROLE => if target.is_some() { "Music" } else { "Communication" },
        *pw::keys::NODE_NAME => "telinhas-capture",
        // Our own streams must not be taken for a program to capture.
        *pw::keys::APP_NAME => "telinhas",
    };
    if let Some(player) = target {
        props.insert("target.object", player.serial.clone());
        // A program that ends ends our stream with it, not a reconnection to
        // some other thing.
        props.insert("node.dont-reconnect", "true");
    }
    let stream = pw::stream::StreamRc::new(core.clone(), "telinhas-audio", props).map_err(os)?;

    let tap = Tap {
        label,
        node: target.map(|player| player.id),
        queue: Arc::default(),
        shared: Arc::default(),
    };
    let feed = Feed {
        queue: Arc::clone(&tap.queue),
        shared: Arc::clone(&tap.shared),
        channels: CHANNELS,
    };
    let listener = stream
        .add_local_listener_with_user_data(feed)
        .param_changed(|_, feed, id, param| {
            let Some(param) = param else { return };
            if id != spa::param::ParamType::Format.as_raw() {
                return;
            }
            let mut info = spa::param::audio::AudioInfoRaw::new();
            if info.parse(param).is_ok() && info.channels() > 0 {
                feed.channels = info.channels() as usize;
            }
        })
        .process(|stream, feed| {
            let Some(mut buffer) = stream.dequeue_buffer() else {
                return;
            };
            let Some(data) = buffer.datas_mut().first_mut() else {
                return;
            };
            let (offset, size) = (data.chunk().offset() as usize, data.chunk().size() as usize);
            let Some(bytes) = data.data() else { return };
            let Some(bytes) = bytes.get(offset..offset + size) else {
                return;
            };
            push_samples(feed, bytes);
        })
        .register()
        .map_err(os)?;

    let bytes = format_request()?;
    let mut params = [spa::pod::Pod::from_bytes(&bytes)
        .ok_or_else(|| Error::Os("could not build the audio format request".to_owned()))?];
    stream
        .connect(
            spa::utils::Direction::Input,
            None,
            pw::stream::StreamFlags::AUTOCONNECT
                | pw::stream::StreamFlags::MAP_BUFFERS
                | pw::stream::StreamFlags::RT_PROCESS,
            &mut params,
        )
        .map_err(os)?;

    taps.lock().unwrap().push(tap);
    Ok(Running {
        stream,
        _listener: listener,
    })
}

/// Little-endian 32-bit float samples into the tap's queue, stereo whatever
/// the stream's channel count turned out to be.
fn push_samples(feed: &Feed, bytes: &[u8]) {
    let channels = feed.channels.max(1);
    let mut peak = f32::from_bits(feed.shared.peak_bits.load(Ordering::Relaxed));
    let mut queue = feed.queue.lock().unwrap();
    let mut frames = 0u64;
    for frame in bytes.chunks_exact(4 * channels) {
        let sample = |channel: usize| {
            let at = channel.min(channels - 1) * 4;
            f32::from_le_bytes([frame[at], frame[at + 1], frame[at + 2], frame[at + 3]])
        };
        let (left, right) = (sample(0), sample(1));
        peak = peak.max(left.abs()).max(right.abs());
        queue.push_back(left);
        queue.push_back(right);
        frames += 1;
    }
    if queue.len() > MAX_QUEUED {
        let drop = queue.len() - KEEP_QUEUED;
        queue.drain(..drop);
        feed.shared.dropped_frames.fetch_add(
            (drop / CHANNELS) as u64 / FRAME_SAMPLES as u64 + 1,
            Ordering::Relaxed,
        );
    }
    drop(queue);
    feed.shared.packets.fetch_add(1, Ordering::Relaxed);
    feed.shared.frames.fetch_add(frames, Ordering::Relaxed);
    feed.shared
        .peak_bits
        .store(peak.to_bits(), Ordering::Relaxed);
}

// ----- the PipeWire thread -----------------------------------------------------------

/// Runs until `stop` fires: follows the programs that play and keeps a stream
/// on each one that is wanted.
fn run(
    source: Option<AudioSource>,
    mic: bool,
    taps: Taps,
    ready: Sender<Result<()>>,
    stop: pw::channel::Receiver<()>,
) -> Result<()> {
    pw::init();
    let mainloop = pw::main_loop::MainLoopRc::new(None).map_err(os)?;
    let context = pw::context::ContextRc::new(&mainloop, None).map_err(os)?;
    let core = context.connect_rc(None).map_err(os)?;

    let stop_loop = mainloop.clone();
    let _stop = stop.attach(mainloop.loop_(), move |()| stop_loop.quit());
    let error_loop = mainloop.clone();
    let _core_listener = core
        .add_listener_local()
        .error(move |id, _, _, _| {
            // Errors on the core itself (id 0) mean the daemon went away.
            if id == 0 {
                error_loop.quit();
            }
        })
        .register();

    let running: Rc<RefCell<HashMap<u32, Running>>> = Rc::default();
    let mut microphone = None;
    if mic {
        microphone = Some(open_stream(&core, &taps, None, "microphone".to_owned())?);
    }

    let registry = core.get_registry_rc().map_err(os)?;
    let _registry_listener = source.map(|source| {
        let (core, taps, running) = (core.clone(), Arc::clone(&taps), Rc::clone(&running));
        let removed_taps = Arc::clone(&taps);
        let removed = Rc::clone(&running);
        let own = std::process::id();
        registry
            .add_listener_local()
            .global(move |object| {
                if object.type_ != pw::types::ObjectType::Node {
                    return;
                }
                let Some(player) = object
                    .props
                    .and_then(|props| Player::from_props(object.id, props))
                else {
                    return;
                };
                if !wanted(&source, &player, own, &Proc) {
                    return;
                }
                // A program that cannot be captured is skipped, not fatal.
                if let Ok(stream) = open_stream(&core, &taps, Some(&player), player.label()) {
                    running.borrow_mut().insert(player.id, stream);
                }
            })
            .global_remove(move |id| {
                if removed.borrow_mut().remove(&id).is_some() {
                    removed_taps
                        .lock()
                        .unwrap()
                        .retain(|tap| tap.node != Some(id));
                }
            })
            .register()
    });

    // Tell the caller the daemon answered: one round trip.
    let ready_loop = Rc::new(RefCell::new(Some(ready)));
    let announce = Rc::clone(&ready_loop);
    let _sync_listener = core
        .add_listener_local()
        .done(move |_, _| {
            if let Some(ready) = announce.borrow_mut().take() {
                let _ = ready.send(Ok(()));
            }
        })
        .register();
    core.sync(0).map_err(os)?;

    mainloop.run();
    // Streams go before the loop and the core.
    running.borrow_mut().clear();
    drop(microphone);
    Ok(())
}

// ----- listing the programs -------------------------------------------------------------

/// The programs that play sound right now (or hold a stream open), one row
/// per process. PipeWire does not say which output device a stream uses without
/// following its links, so `device` is the stream's own title.
pub fn list_apps() -> Result<Vec<AudioApp>> {
    pw::init();
    let mainloop = pw::main_loop::MainLoopRc::new(None).map_err(os)?;
    let context = pw::context::ContextRc::new(&mainloop, None).map_err(os)?;
    let core = context.connect_rc(None).map_err(os)?;

    let found: Rc<RefCell<Vec<(Player, String)>>> = Rc::default();
    let registry = core.get_registry_rc().map_err(os)?;
    let sink = Rc::clone(&found);
    let _registry_listener = registry
        .add_listener_local()
        .global(move |object| {
            if object.type_ != pw::types::ObjectType::Node {
                return;
            }
            let Some(props) = object.props else { return };
            if let Some(player) = Player::from_props(object.id, props) {
                let title = props.get(*pw::keys::MEDIA_NAME).unwrap_or("").to_owned();
                sink.borrow_mut().push((player, title));
            }
        })
        .register();

    let done_loop = mainloop.clone();
    let _core_listener = core
        .add_listener_local()
        .done(move |_, _| done_loop.quit())
        .register();
    core.sync(0).map_err(os)?;
    mainloop.run();

    let processes = Proc;
    let mut apps: Vec<AudioApp> = Vec::new();
    for (player, title) in found.borrow().iter() {
        let Some(pid) = player.pid else { continue };
        if apps.iter().any(|app| app.pid == pid) {
            continue;
        }
        apps.push(AudioApp {
            pid,
            process: player
                .names
                .first()
                .cloned()
                .or_else(|| processes.name(pid))
                .unwrap_or_default(),
            device: title.clone(),
            active: true,
        });
    }
    apps.sort_by(|a, b| a.process.cmp(&b.process).then(a.pid.cmp(&b.pid)));
    Ok(apps)
}

// ----- the capture handle and the mixer ----------------------------------------------------

pub struct Capture {
    taps: Taps,
    frames_out: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    mixer: Option<JoinHandle<()>>,
    pipewire: Option<(pw::channel::Sender<()>, JoinHandle<()>)>,
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
        if let Some((stop, thread)) = self.pipewire.take() {
            let _ = stop.send(());
            let _ = thread.join();
        }
        if let Some(mixer) = self.mixer.take() {
            let _ = mixer.join();
        }
    }
}

pub fn start(source: Option<AudioSource>, mic: bool, mut on_frame: FrameSink) -> Result<Capture> {
    let taps: Taps = Arc::default();
    let stop = Arc::new(AtomicBool::new(false));
    let frames_out = Arc::new(AtomicU64::new(0));

    let (ready_tx, ready_rx) = channel::<Result<()>>();
    let (stop_tx, stop_rx) = pw::channel::channel::<()>();
    let thread = {
        let taps = Arc::clone(&taps);
        let failed = ready_tx.clone();
        std::thread::Builder::new()
            .name("telinhas-audio-pipewire".to_owned())
            .spawn(move || {
                if let Err(error) = run(source, mic, taps, ready_tx, stop_rx) {
                    let _ = failed.send(Err(error));
                }
            })
            .map_err(os)?
    };
    // No PipeWire daemon (or no permission to reach it) is said right away.
    match ready_rx.recv_timeout(Duration::from_secs(5)) {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            let _ = stop_tx.send(());
            let _ = thread.join();
            return Err(error);
        }
        Err(_) => {
            let _ = stop_tx.send(());
            let _ = thread.join();
            return Err(Error::Os("PipeWire did not answer in 5 s".to_owned()));
        }
    }

    // The mixer: every 20 ms, one frame from whatever each stream has.
    let mixer = {
        let (taps, stop, frames_out) = (
            Arc::clone(&taps),
            Arc::clone(&stop),
            Arc::clone(&frames_out),
        );
        std::thread::Builder::new()
            .name("telinhas-audio-mixer".to_owned())
            .spawn(move || {
                let mut frame = vec![0f32; FRAME_LEN];
                let mut next = Instant::now();
                while !stop.load(Ordering::Relaxed) {
                    mix(&taps.lock().unwrap(), &mut frame);
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
            .map_err(os)?
    };
    Ok(Capture {
        taps,
        frames_out,
        stop,
        mixer: Some(mixer),
        pipewire: Some((stop_tx, thread)),
    })
}

/// One 20 ms frame: what every tap has queued, summed and limited to ±1.
fn mix(taps: &[Tap], frame: &mut [f32]) {
    frame.fill(0.0);
    for tap in taps {
        let mut queue = tap.queue.lock().unwrap();
        let take = queue.len().min(frame.len());
        for (out, sample) in frame.iter_mut().zip(queue.drain(..take)) {
            *out += sample;
        }
    }
    for sample in frame {
        *sample = sample.clamp(-1.0, 1.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A table of parents and names standing in for `/proc`.
    struct Fake(Vec<(u32, u32, &'static str)>);

    impl Processes for Fake {
        fn parent(&self, pid: u32) -> Option<u32> {
            self.0.iter().find(|p| p.0 == pid).map(|p| p.1)
        }
        fn name(&self, pid: u32) -> Option<String> {
            self.0.iter().find(|p| p.0 == pid).map(|p| p.2.to_owned())
        }
    }

    fn player(id: u32, pid: Option<u32>, name: &str) -> Player {
        Player {
            id,
            serial: id.to_string(),
            pid,
            names: vec![name.to_owned()],
        }
    }

    fn table() -> Fake {
        Fake(vec![
            (1, 0, "systemd"),
            (100, 1, "telinhas"),
            (200, 1, "firefox"),
            (201, 200, "firefox-child"),
            (300, 1, "Discord"),
            (301, 300, "Discord"),
        ])
    }

    fn system(exclude: &[&str]) -> AudioSource {
        AudioSource::System {
            exclude: exclude.iter().map(|name| (*name).to_owned()).collect(),
        }
    }

    #[test]
    fn the_parent_is_read_after_the_last_parenthesis() {
        assert_eq!(parent_from_stat("42 (a b) c) S 7 42 42"), Some(7));
        assert_eq!(parent_from_stat("nonsense"), None);
    }

    #[test]
    fn the_lineage_stops_at_the_root_and_at_loops() {
        assert_eq!(lineage(&table(), 201), vec![201, 200, 1]);
        let looped = Fake(vec![(5, 6, "a"), (6, 5, "b")]);
        assert_eq!(lineage(&looped, 5), vec![5, 6]);
    }

    #[test]
    fn the_system_leaves_out_this_app_and_what_is_excluded() {
        let table = table();
        let source = system(&["discord.exe"]);
        assert!(wanted(
            &source,
            &player(1, Some(200), "firefox"),
            100,
            &table
        ));
        assert!(!wanted(
            &source,
            &player(2, Some(100), "telinhas"),
            100,
            &table
        ));
        assert!(!wanted(
            &source,
            &player(3, Some(301), "Discord"),
            100,
            &table
        ));
        // Without a pid the names still tell.
        assert!(!wanted(&source, &player(4, None, "Discord"), 100, &table));
        assert!(wanted(&source, &player(5, None, "mpv"), 100, &table));
    }

    #[test]
    fn the_system_without_exclusions_takes_everything_but_this_app() {
        let table = table();
        let source = system(&[]);
        assert!(wanted(
            &source,
            &player(1, Some(300), "Discord"),
            100,
            &table
        ));
        assert!(!wanted(
            &source,
            &player(2, Some(100), "telinhas"),
            100,
            &table
        ));
    }

    #[test]
    fn one_process_takes_its_children_and_nothing_else() {
        let table = table();
        let source = AudioSource::Process { pid: 200 };
        assert!(wanted(
            &source,
            &player(1, Some(200), "firefox"),
            100,
            &table
        ));
        assert!(wanted(&source, &player(2, Some(201), "tab"), 100, &table));
        assert!(!wanted(
            &source,
            &player(3, Some(300), "Discord"),
            100,
            &table
        ));
        assert!(!wanted(&source, &player(4, None, "firefox"), 100, &table));
    }

    #[test]
    fn taps_are_summed_and_limited() {
        let tap = |samples: &[f32]| Tap {
            label: String::new(),
            node: None,
            queue: Arc::new(Mutex::new(samples.iter().copied().collect())),
            shared: Arc::default(),
        };
        let taps = [tap(&[0.5, -0.5, 0.9]), tap(&[0.25, -0.75, 0.9])];
        let mut frame = vec![9.0; 4];
        mix(&taps, &mut frame);
        assert_eq!(frame, vec![0.75, -1.0, 1.0, 0.0]);
        // What was taken is gone.
        assert!(taps[0].queue.lock().unwrap().is_empty());
    }

    #[test]
    fn samples_are_queued_as_stereo_whatever_the_channel_count() {
        let feed = |channels| Feed {
            queue: Arc::default(),
            shared: Arc::default(),
            channels,
        };
        let bytes =
            |samples: &[f32]| -> Vec<u8> { samples.iter().flat_map(|s| s.to_le_bytes()).collect() };
        let mono = feed(1);
        push_samples(&mono, &bytes(&[0.5, 0.25]));
        assert_eq!(
            mono.queue
                .lock()
                .unwrap()
                .iter()
                .copied()
                .collect::<Vec<_>>(),
            vec![0.5, 0.5, 0.25, 0.25]
        );
        let stereo = feed(2);
        push_samples(&stereo, &bytes(&[0.5, -0.5]));
        assert_eq!(
            stereo
                .queue
                .lock()
                .unwrap()
                .iter()
                .copied()
                .collect::<Vec<_>>(),
            vec![0.5, -0.5]
        );
        assert_eq!(
            f32::from_bits(stereo.shared.peak_bits.load(Ordering::Relaxed)),
            0.5
        );
    }
}
