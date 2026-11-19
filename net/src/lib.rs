//! The signaling client: connects to the server, keeps the person in a room
//! and turns what happens into [`Event`]s for the interface.
//!
//! It runs on its own thread with its own async runtime. The interface sends
//! [`Command`]s and reads [`Event`]s without waiting on the network, and is
//! woken (to redraw) whenever an event arrives.
//!
//! Reconnection follows DEC-10, DEC-42 and DEC-50: when the connection drops
//! inside a room, the client keeps trying with the same room code and session
//! token, so the server recognizes the same participant.

use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use telinhas_protocol::{
    ConnectionState, ErrorCode, LatencySample, Layer, LeaveReason, PeerId, PeerInfo, SignalKind,
    StreamInfo, StreamKind, Value,
};
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};

mod session;

pub use session::Settings;

/// What the interface asks the client to do.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    CreateRoom {
        require_approval: bool,
    },
    JoinRoom {
        code: String,
    },
    /// Leaves for good. The room does not keep a place for who leaves on purpose.
    Leave,
    SetNickname(String),
    SetApproval(bool),
    ApprovalDecision {
        request_id: String,
        accept: bool,
    },
    /// Relays WebRTC negotiation to a participant.
    Signal {
        to: PeerId,
        kind: SignalKind,
        payload: Value,
    },
    AnnounceStream {
        stream_id: String,
        kind: StreamKind,
        live: bool,
        meta: Value,
    },
    RequestLayer {
        to: PeerId,
        stream_id: String,
        layer: Layer,
    },
    ReportLatency(Vec<LatencySample>),
    /// Closes the socket without leaving, as a network drop would. For tests.
    DropConnectionForTesting,
    Shutdown,
}

/// The room as the server described it when we got in.
#[derive(Debug, Clone, PartialEq)]
pub struct RoomSnapshot {
    pub code: String,
    pub you: PeerInfo,
    pub peers: Vec<PeerInfo>,
    pub admin: PeerId,
    pub require_approval: bool,
    pub streams: Vec<StreamInfo>,
    /// Relays for connections that cannot go direct.
    pub ice_servers: Vec<telinhas_protocol::IceServer>,
}

/// What the client tells the interface.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// Opening the connection to the server.
    Connecting,
    /// In the room: first entry or after a reconnection.
    Joined(RoomSnapshot),
    /// Waiting for the admin to accept us in.
    ApprovalPending,
    /// The admin was refused or did not answer; we are out.
    JoinDenied,
    /// For the admin: someone asks to come in.
    ApprovalRequest {
        request_id: String,
        nickname: String,
    },
    PeerJoined(PeerInfo),
    PeerLeft {
        peer_id: PeerId,
        reason: LeaveReason,
    },
    PeerConnection {
        peer_id: PeerId,
        state: ConnectionState,
    },
    PeerRenamed {
        peer_id: PeerId,
        nickname: String,
    },
    AdminChanged(PeerId),
    ApprovalChanged(bool),
    OverCapacity(usize),
    StreamState {
        from: PeerId,
        stream_id: String,
        kind: StreamKind,
        live: bool,
        meta: Value,
    },
    Signal {
        from: PeerId,
        kind: SignalKind,
        payload: Value,
    },
    LayerRequest {
        from: PeerId,
        stream_id: String,
        layer: Layer,
    },
    LatencyReport {
        from: PeerId,
        samples: Vec<LatencySample>,
    },
    /// The connection dropped inside a room; trying again (attempt number).
    Reconnecting {
        attempt: u32,
    },
    /// A problem to tell the person about. `fatal` means we are out of the room.
    Failure {
        kind: Failure,
        message: String,
        fatal: bool,
    },
    /// Out of the room, back to idle.
    Left,
}

/// Kinds of problem, in terms the interface can word.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    /// The server could not be reached.
    Unreachable,
    /// The code does not match a live room.
    InvalidCode,
    /// Too many wrong codes from this address.
    RateLimited { retry_after_s: u64 },
    /// The nickname was refused.
    InvalidNickname,
    /// Only the admin may do that.
    NotAdmin,
    /// The room is gone: its time to come back ran out.
    RoomExpired,
    /// Anything else the server or the protocol complained about.
    Other,
}

/// The two ends the interface holds.
pub struct Handle {
    commands: UnboundedSender<Command>,
    events: mpsc::Receiver<Event>,
}

impl Handle {
    pub fn send(&self, command: Command) {
        let _ = self.commands.send(command);
    }

    /// The next event, if one is waiting.
    pub fn try_event(&self) -> Option<Event> {
        self.events.try_recv().ok()
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Shutdown);
    }
}

/// Starts the client thread. `wake` is called after every event, so the
/// interface can ask for a redraw.
pub fn spawn(settings: Settings, wake: impl Fn() + Send + Sync + 'static) -> Handle {
    let (command_tx, command_rx) = unbounded_channel();
    let (event_tx, event_rx) = mpsc::channel();
    let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(wake);
    std::thread::Builder::new()
        .name("telinhas-net".to_owned())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .enable_all()
                .build()
                .expect("a runtime for the signaling client");
            runtime.block_on(session::run(
                settings,
                command_rx,
                Emitter { tx: event_tx, wake },
            ));
        })
        .expect("the signaling client thread");
    Handle {
        commands: command_tx,
        events: event_rx,
    }
}

/// Sends events to the interface and wakes it.
#[derive(Clone)]
pub(crate) struct Emitter {
    tx: mpsc::Sender<Event>,
    wake: Arc<dyn Fn() + Send + Sync>,
}

impl Emitter {
    pub(crate) fn emit(&self, event: Event) {
        let _ = self.tx.send(event);
        (self.wake)();
    }
}

/// How long to wait before reconnection attempt `attempt` (counting from 1):
/// quick at first, then calmer, never beyond a few seconds.
pub fn backoff(attempt: u32) -> Duration {
    const STEPS_MS: [u64; 6] = [300, 700, 1500, 3000, 5000, 5000];
    Duration::from_millis(STEPS_MS[(attempt.saturating_sub(1) as usize).min(STEPS_MS.len() - 1)])
}

/// How long the client keeps trying to get back into a room: the server holds
/// a dropped participant for 60 s (DEC-50) and an empty room for 5 min (DEC-23).
pub const GIVE_UP_AFTER: Duration = Duration::from_secs(6 * 60);

pub(crate) fn failure_from_error(code: ErrorCode, retry_after_s: Option<u64>) -> Failure {
    match code {
        ErrorCode::InvalidCode => Failure::InvalidCode,
        ErrorCode::RateLimited => Failure::RateLimited {
            retry_after_s: retry_after_s.unwrap_or(60),
        },
        ErrorCode::InvalidNickname => Failure::InvalidNickname,
        ErrorCode::NotAdmin => Failure::NotAdmin,
        ErrorCode::JoinDenied | ErrorCode::UnsupportedVersion | ErrorCode::BadMessage => {
            Failure::Other
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_grows_then_levels_off() {
        assert!(backoff(1) < backoff(3));
        assert_eq!(backoff(6), backoff(20));
        assert!(backoff(20) <= Duration::from_secs(5));
    }

    #[test]
    fn errors_become_failures() {
        assert_eq!(
            failure_from_error(ErrorCode::RateLimited, Some(30)),
            Failure::RateLimited { retry_after_s: 30 }
        );
        assert_eq!(
            failure_from_error(ErrorCode::InvalidCode, None),
            Failure::InvalidCode
        );
    }
}
