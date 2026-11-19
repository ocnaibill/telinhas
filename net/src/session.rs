//! The connection loop: one room at a time, with reconnection.

use std::sync::Once;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use telinhas_protocol::{ClientMessage, ServerMessage, decode_server, encode_client};
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::time::Instant;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

use crate::{
    Command, Emitter, Event, Failure, GIVE_UP_AFTER, RoomSnapshot, backoff, failure_from_error,
};

/// A connection with no frame for this long is considered dead. The server
/// pings every 15 s, so a healthy one is never this quiet.
const IDLE_TIMEOUT: Duration = Duration::from_secs(45);

#[derive(Debug, Clone)]
pub struct Settings {
    /// WebSocket address of the signaling server, such as `wss://host/ws`.
    pub server: String,
    pub nickname: String,
}

enum Entry {
    Create { require_approval: bool },
    Join(String),
}

enum Exit {
    /// Back to waiting for the next room.
    Idle,
    Shutdown,
}

enum LoopEnd {
    Left,
    Shutdown,
    /// The room is over for us (denied, expired, refused): already reported.
    Over,
    /// The connection broke; the caller decides whether to retry.
    Dropped,
}

fn install_tls_provider() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

fn new_token() -> String {
    format!(
        "{:032x}{:032x}",
        rand::random::<u128>(),
        rand::random::<u128>()
    )
}

pub(crate) async fn run(
    settings: Settings,
    mut commands: UnboundedReceiver<Command>,
    emit: Emitter,
) {
    install_tls_provider();
    let mut nickname = settings.nickname.clone();
    loop {
        let Some(command) = commands.recv().await else {
            return;
        };
        let entry = match command {
            Command::CreateRoom { require_approval } => Entry::Create { require_approval },
            Command::JoinRoom { code } => Entry::Join(code),
            Command::SetNickname(name) => {
                nickname = name;
                continue;
            }
            Command::Shutdown => return,
            _ => continue,
        };
        match run_room(&settings.server, &mut nickname, entry, &mut commands, &emit).await {
            Exit::Idle => {}
            Exit::Shutdown => return,
        }
    }
}

/// Waits `delay`, but reacts to the person leaving meanwhile.
async fn wait_or_leave(
    delay: Duration,
    commands: &mut UnboundedReceiver<Command>,
) -> Option<LoopEnd> {
    let sleep = tokio::time::sleep(delay);
    tokio::pin!(sleep);
    loop {
        tokio::select! {
            () = &mut sleep => return None,
            command = commands.recv() => match command {
                None | Some(Command::Shutdown) => return Some(LoopEnd::Shutdown),
                Some(Command::Leave) => return Some(LoopEnd::Left),
                Some(_) => {}
            },
        }
    }
}

async fn run_room(
    server: &str,
    nickname: &mut String,
    entry: Entry,
    commands: &mut UnboundedReceiver<Command>,
    emit: &Emitter,
) -> Exit {
    let token = new_token();
    // Known once we are in; from then on a connection is always a rejoin.
    let mut code: Option<String> = None;
    let mut attempt = 0u32;
    let mut reconnecting_since: Option<Instant> = None;
    emit.emit(Event::Connecting);

    loop {
        let connection = connect_async(server).await;
        let ws = match connection {
            Ok((ws, _)) => ws,
            Err(error) => {
                if code.is_none() {
                    emit.emit(Event::Failure {
                        kind: Failure::Unreachable,
                        message: error.to_string(),
                        fatal: true,
                    });
                    return Exit::Idle;
                }
                match retry_or_give_up(&mut attempt, &mut reconnecting_since, commands, emit).await
                {
                    None => continue,
                    Some(end) => return finish(end, emit),
                }
            }
        };

        let first = match (&code, &entry) {
            (Some(code), _) | (None, Entry::Join(code)) => ClientMessage::JoinRoom {
                code: code.clone(),
                nickname: nickname.clone(),
                session_token: token.clone(),
            },
            (None, Entry::Create { require_approval }) => ClientMessage::CreateRoom {
                nickname: nickname.clone(),
                session_token: token.clone(),
                require_approval: *require_approval,
            },
        };
        let end = connection_loop(ws, first, nickname, &mut code, commands, emit, || {
            attempt = 0;
            reconnecting_since = None;
        })
        .await;
        match end {
            LoopEnd::Dropped => {
                if code.is_none() {
                    emit.emit(Event::Failure {
                        kind: Failure::Unreachable,
                        message: "the connection closed before we got into the room".to_owned(),
                        fatal: true,
                    });
                    return Exit::Idle;
                }
                match retry_or_give_up(&mut attempt, &mut reconnecting_since, commands, emit).await
                {
                    None => continue,
                    Some(end) => return finish(end, emit),
                }
            }
            other => return finish(other, emit),
        }
    }
}

fn finish(end: LoopEnd, emit: &Emitter) -> Exit {
    match end {
        LoopEnd::Left => {
            emit.emit(Event::Left);
            Exit::Idle
        }
        LoopEnd::Shutdown => Exit::Shutdown,
        LoopEnd::Over | LoopEnd::Dropped => Exit::Idle,
    }
}

/// Announces the attempt and waits before the next one. `None` means try
/// again; `Some` ends the room (the person left, or the time ran out).
async fn retry_or_give_up(
    attempt: &mut u32,
    since: &mut Option<Instant>,
    commands: &mut UnboundedReceiver<Command>,
    emit: &Emitter,
) -> Option<LoopEnd> {
    let started = *since.get_or_insert_with(Instant::now);
    if started.elapsed() > GIVE_UP_AFTER {
        emit.emit(Event::Failure {
            kind: Failure::RoomExpired,
            message: "could not get back into the room in time".to_owned(),
            fatal: true,
        });
        return Some(LoopEnd::Over);
    }
    *attempt += 1;
    emit.emit(Event::Reconnecting { attempt: *attempt });
    wait_or_leave(backoff(*attempt), commands).await
}

fn event_for(message: ServerMessage) -> Option<Event> {
    Some(match message {
        ServerMessage::ApprovalPending => Event::ApprovalPending,
        ServerMessage::ApprovalRequest {
            request_id,
            nickname,
        } => Event::ApprovalRequest {
            request_id,
            nickname,
        },
        ServerMessage::PeerJoined { peer } => Event::PeerJoined(peer),
        ServerMessage::PeerLeft { peer_id, reason } => Event::PeerLeft { peer_id, reason },
        ServerMessage::PeerConnection { peer_id, state } => {
            Event::PeerConnection { peer_id, state }
        }
        ServerMessage::PeerRenamed { peer_id, nickname } => {
            Event::PeerRenamed { peer_id, nickname }
        }
        ServerMessage::AdminChanged { peer_id } => Event::AdminChanged(peer_id),
        ServerMessage::ApprovalChanged { require_approval } => {
            Event::ApprovalChanged(require_approval)
        }
        ServerMessage::Signal {
            from,
            kind,
            payload,
        } => Event::Signal {
            from,
            kind,
            payload,
        },
        ServerMessage::StreamState {
            from,
            stream_id,
            kind,
            live,
            meta,
        } => Event::StreamState {
            from,
            stream_id,
            kind,
            live,
            meta,
        },
        ServerMessage::LayerRequest {
            from,
            stream_id,
            layer,
        } => Event::LayerRequest {
            from,
            stream_id,
            layer,
        },
        ServerMessage::LatencyReport { from, samples } => Event::LatencyReport { from, samples },
        ServerMessage::RoomOverCapacity { count } => Event::OverCapacity(count),
        ServerMessage::RoomJoined { .. } | ServerMessage::Error { .. } => return None,
    })
}

async fn connection_loop(
    ws: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    first: ClientMessage,
    nickname: &mut String,
    code: &mut Option<String>,
    commands: &mut UnboundedReceiver<Command>,
    emit: &Emitter,
    mut on_joined: impl FnMut(),
) -> LoopEnd {
    let (mut sink, mut stream) = ws.split();
    let send = |message: ClientMessage| Message::Text(encode_client(&message).into());
    if sink.send(send(first)).await.is_err() {
        return LoopEnd::Dropped;
    }

    loop {
        tokio::select! {
            frame = tokio::time::timeout(IDLE_TIMEOUT, stream.next()) => {
                let frame = match frame {
                    Err(_) | Ok(None | Some(Err(_))) => return LoopEnd::Dropped,
                    Ok(Some(Ok(frame))) => frame,
                };
                let text = match frame {
                    Message::Text(text) => text,
                    Message::Close(_) => return LoopEnd::Dropped,
                    _ => continue, // pings and pongs: the library answers them
                };
                let message = match decode_server(text.as_str()) {
                    Ok(message) => message,
                    Err(error) => {
                        emit.emit(Event::Failure {
                            kind: Failure::Other,
                            message: error.to_string(),
                            fatal: false,
                        });
                        continue;
                    }
                };
                match message {
                    ServerMessage::RoomJoined { code: room_code, you, peers, admin, require_approval, streams, ice_servers } => {
                        *code = Some(room_code.clone());
                        on_joined();
                        emit.emit(Event::Joined(RoomSnapshot {
                            code: room_code,
                            you,
                            peers,
                            admin,
                            require_approval,
                            streams,
                            ice_servers,
                        }));
                    }
                    ServerMessage::Error { code: error_code, message, retry_after_s } => {
                        let joined = code.is_some();
                        if error_code == telinhas_protocol::ErrorCode::JoinDenied {
                            emit.emit(Event::JoinDenied);
                            return LoopEnd::Over;
                        }
                        let kind = if error_code == telinhas_protocol::ErrorCode::InvalidCode && joined {
                            Failure::RoomExpired
                        } else {
                            failure_from_error(error_code, retry_after_s)
                        };
                        // Problems before we are in end the attempt; once in,
                        // only an expired room does.
                        let fatal = !joined || kind == Failure::RoomExpired;
                        emit.emit(Event::Failure { kind, message, fatal });
                        if fatal {
                            return LoopEnd::Over;
                        }
                    }
                    other => {
                        if let Some(event) = event_for(other) {
                            emit.emit(event);
                        }
                    }
                }
            }
            command = commands.recv() => {
                let Some(command) = command else {
                    return LoopEnd::Shutdown;
                };
                let outgoing = match command {
                    Command::Leave => {
                        let _ = sink.send(send(ClientMessage::LeaveRoom)).await;
                        let _ = sink.close().await;
                        return LoopEnd::Left;
                    }
                    Command::Shutdown => return LoopEnd::Shutdown,
                    Command::DropConnectionForTesting => return LoopEnd::Dropped,
                    Command::SetNickname(name) => {
                        *nickname = name.clone();
                        ClientMessage::SetNickname { nickname: name }
                    }
                    Command::SetApproval(require_approval) => ClientMessage::SetApproval { require_approval },
                    Command::ApprovalDecision { request_id, accept } => ClientMessage::ApprovalDecision { request_id, accept },
                    Command::Signal { to, kind, payload } => ClientMessage::Signal { to, kind, payload },
                    Command::AnnounceStream { stream_id, kind, live, meta } => ClientMessage::StreamState { stream_id, kind, live, meta },
                    Command::RequestLayer { to, stream_id, layer } => ClientMessage::LayerRequest { to, stream_id, layer },
                    Command::ReportLatency(samples) => ClientMessage::LatencyReport { samples },
                    // Already in a room.
                    Command::CreateRoom { .. } | Command::JoinRoom { .. } => continue,
                };
                if sink.send(send(outgoing)).await.is_err() {
                    return LoopEnd::Dropped;
                }
            }
        }
    }
}
