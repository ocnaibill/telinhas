//! Video between peers.
//!
//! The broadcaster captures and encodes once, and sends the same H.264 to each
//! viewer over its own WebRTC connection. Each viewer decodes into pictures
//! the interface draws. Negotiation travels through the signaling server as
//! `signal` messages: the broadcaster sends the offer, the viewer answers, and
//! each side waits for ICE gathering so one description carries everything
//! (no trickle).
//!
//! Like `telinhas-net`, this is driven by commands and events from any thread:
//! the work happens on its own runtime.

mod actor;
mod audio;
mod link;
mod picture;
mod scale;
mod send;
mod stamp;
mod stats;

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

pub use audio::{AudioBuffer, AudioInput};
pub use link::LinkConfig;
pub use picture::Picture;
pub use send::{BroadcastSource, BroadcastSpec};
pub use stats::{AudioReceiving, Path, Receiving, Sending, Stats};
pub use telinhas_protocol::{PeerId, SignalKind, Value};
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};

/// What the application asks of the media layer.
#[derive(Debug)]
pub enum Command {
    /// Start capturing and encoding, sending to these viewers.
    StartBroadcast {
        spec: BroadcastSpec,
        viewers: Vec<PeerId>,
    },
    StopBroadcast,
    /// Someone entered the room: they get the broadcast, if there is one.
    PeerJoined(PeerId),
    /// Someone left or dropped: forget its connections.
    PeerLeft(PeerId),
    /// A peer stopped broadcasting: close what we receive from it.
    StreamEnded(PeerId),
    /// Negotiation that came from the signaling server.
    Signal {
        from: PeerId,
        kind: SignalKind,
        payload: Value,
    },
    Shutdown,
}

/// What the media layer tells the application.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// Negotiation to relay through the signaling server.
    Signal {
        to: PeerId,
        kind: SignalKind,
        payload: Value,
    },
    /// The encoder is running.
    Encoder {
        name: String,
        hardware: bool,
        /// Why software was used when hardware was wanted.
        fallback_reason: Option<String>,
        width: u32,
        height: u32,
    },
    /// The broadcast could not start or stopped on its own.
    BroadcastFailed(String),
    /// The sound could not be captured or stopped on its own; the video goes on.
    AudioFailed(String),
    /// A peer's pictures are coming (the first one arrived).
    Receiving(PeerId),
}

pub(crate) type AudioBuffers = Arc<Mutex<HashMap<PeerId, Arc<AudioBuffer>>>>;

type Latest = Arc<Mutex<HashMap<PeerId, Arc<Picture>>>>;

/// Handle to the media layer. Dropping it shuts everything down.
pub struct Media {
    commands: UnboundedSender<Command>,
    events: Mutex<Receiver<Event>>,
    latest: Latest,
    stats: Arc<Mutex<Stats>>,
    audio: AudioBuffers,
}

impl Media {
    /// Starts the media thread. `wake` is called whenever there is something
    /// new to look at (an event or a picture), from another thread.
    pub fn spawn(link: LinkConfig, wake: impl Fn() + Send + Sync + 'static) -> Self {
        let (commands, inbox) = unbounded_channel();
        let (events_tx, events) = channel();
        let latest: Latest = Arc::default();
        let stats: Arc<Mutex<Stats>> = Arc::default();
        let audio: AudioBuffers = Arc::default();
        let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(wake);
        let shared = actor::Shared {
            link,
            events: events_tx,
            latest: Arc::clone(&latest),
            stats: Arc::clone(&stats),
            audio: Arc::clone(&audio),
            wake,
        };
        std::thread::Builder::new()
            .name("telinhas-media".to_owned())
            .spawn(move || {
                let Ok(runtime) = tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .enable_all()
                    .build()
                else {
                    return;
                };
                runtime.block_on(actor::run(shared, inbox));
            })
            .ok();
        Self {
            commands,
            events: Mutex::new(events),
            latest,
            stats,
            audio,
        }
    }

    pub fn send(&self, command: Command) {
        let _ = self.commands.send(command);
    }

    pub fn try_event(&self) -> Option<Event> {
        self.events.lock().ok()?.try_recv().ok()
    }

    /// A copy of the measurements so far.
    pub fn stats(&self) -> Stats {
        self.stats.lock().map(|s| s.clone()).unwrap_or_default()
    }

    /// The decoded sound of what `peer` broadcasts, for the playback to pull from.
    pub fn audio(&self, peer: PeerId) -> Option<Arc<AudioBuffer>> {
        self.audio.lock().ok()?.get(&peer).cloned()
    }

    /// The newest decoded picture of what `peer` is broadcasting.
    pub fn latest(&self, peer: PeerId) -> Option<Arc<Picture>> {
        self.latest.lock().ok()?.get(&peer).cloned()
    }
}

impl Drop for Media {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Shutdown);
    }
}

pub(crate) type EventSender = Sender<Event>;
