//! Room state and message handling.
//!
//! All state lives in memory behind one mutex. Handlers are synchronous and
//! never await while holding the lock; delayed work (reconnect grace,
//! approval timeout, room expiry) is queued as [`Timer`]s and spawned right
//! after the lock is released.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde_json::Value;
use tokio::sync::mpsc::UnboundedSender;
use uuid::Uuid;

use crate::code::RoomCode;
use crate::limiter::RateLimiter;
use crate::protocol::{
    ClientMessage, ConnectionState, ErrorCode, IceServer, LatencySample, LeaveReason,
    PROTOCOL_VERSION, PeerId, PeerInfo, ServerMessage, StreamInfo, StreamKind,
};

const MAX_NICKNAME_CHARS: usize = 24;
const MAX_STREAM_ID_CHARS: usize = 64;
const MAX_LATENCY_SAMPLES: usize = 64;

/// Tunable limits. Defaults follow `docs/PROTOCOLO.md`.
#[derive(Debug, Clone)]
pub struct Config {
    /// How long a dropped participant stays "reconnecting" (DEC-50).
    pub reconnect_grace: Duration,
    /// How long an empty room keeps its code (DEC-23).
    pub empty_room_grace: Duration,
    /// How long the admin has to answer a join request (DEC-47).
    pub approval_timeout: Duration,
    /// WebSocket ping interval.
    pub ping_interval: Duration,
    /// Connections silent for this long are dropped.
    pub idle_timeout: Duration,
    /// Failed joins per `failed_join_window` before an IP is blocked (DEC-48).
    pub max_failed_joins: usize,
    pub failed_join_window: Duration,
    pub join_block: Duration,
    /// Messages per second per connection before it is closed.
    pub max_messages_per_second: u32,
    /// Largest accepted message (DEC-46 / `PROTOCOLO.md` §9).
    pub max_message_bytes: usize,
    /// Above this many participants the room gets a warning, not a block (DEC-24).
    pub capacity_warning: usize,
    /// Peers whose `CF-Connecting-IP` header is trusted (the local tunnel).
    pub trusted_proxies: Vec<IpAddr>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            reconnect_grace: Duration::from_secs(60),
            empty_room_grace: Duration::from_secs(300),
            approval_timeout: Duration::from_secs(60),
            ping_interval: Duration::from_secs(15),
            idle_timeout: Duration::from_secs(45),
            max_failed_joins: 10,
            failed_join_window: Duration::from_secs(60),
            join_block: Duration::from_secs(300),
            max_messages_per_second: 100,
            max_message_bytes: 16 * 1024,
            capacity_warning: 12,
            trusted_proxies: vec![
                IpAddr::from([127, 0, 0, 1]),
                IpAddr::from([0, 0, 0, 0, 0, 0, 0, 1]),
            ],
        }
    }
}

/// Parses a comma-separated list of IP addresses, such as
/// `10.0.0.5,fd00::5`. Empty items are ignored.
pub fn parse_ip_list(list: &str) -> Result<Vec<IpAddr>, String> {
    list.split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(|item| {
            item.parse()
                .map_err(|_| format!("not an IP address: {item:?}"))
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ConnId(u64);

/// What a connection task should do next.
#[derive(Debug)]
pub enum Out {
    Message(ServerMessage),
    Close,
}

#[derive(Debug, Clone)]
enum Binding {
    Unbound,
    Pending { code: RoomCode, request_id: String },
    Member { code: RoomCode, peer: PeerId },
}

#[derive(Debug)]
struct Conn {
    tx: UnboundedSender<Out>,
    ip: IpAddr,
    binding: Binding,
}

#[derive(Debug)]
struct Participant {
    peer_id: PeerId,
    nickname: String,
    token: String,
    /// Join order; lowest is the oldest (DEC-22).
    seq: u64,
    conn: Option<ConnId>,
    /// Bumped on every disconnect/reconnect so stale timers do nothing.
    drop_gen: u64,
}

#[derive(Debug)]
struct Pending {
    request_id: String,
    conn: ConnId,
    nickname: String,
    token: String,
}

#[derive(Debug, Clone)]
struct StreamEntry {
    from: PeerId,
    stream_id: String,
    kind: StreamKind,
    meta: Value,
}

#[derive(Debug)]
struct Room {
    participants: Vec<Participant>,
    /// Participants removed after their reconnect grace. They can return with
    /// their token for as long as the room lives (DEC-42).
    dormant: Vec<Participant>,
    admin: Option<PeerId>,
    require_approval: bool,
    pending: Vec<Pending>,
    streams: Vec<StreamEntry>,
    /// Bumped whenever the room (re)fills or empties so stale expiry timers do nothing.
    expiry_gen: u64,
}

impl Room {
    fn info(&self, peer: PeerId) -> Option<PeerInfo> {
        self.participants
            .iter()
            .find(|p| p.peer_id == peer)
            .map(participant_info)
    }

    fn unique_nickname(&self, base: &str, exclude: Option<PeerId>) -> String {
        let taken = |name: &str| {
            let name = name.to_lowercase();
            self.participants
                .iter()
                .any(|p| Some(p.peer_id) != exclude && p.nickname.to_lowercase() == name)
        };
        if !taken(base) {
            return base.to_owned();
        }
        (2..)
            .map(|n| format!("{base} ({n})"))
            .find(|candidate| !taken(candidate))
            .expect("an unused suffix always exists")
    }

    fn joined_message(&self, you: PeerId) -> ServerMessage {
        ServerMessage::RoomJoined {
            code: String::new(),
            you: self.info(you).expect("participant is in the room"),
            peers: self
                .participants
                .iter()
                .filter(|p| p.peer_id != you)
                .map(participant_info)
                .collect(),
            admin: self.admin.unwrap_or(you),
            require_approval: self.require_approval,
            streams: self
                .streams
                .iter()
                .map(|s| StreamInfo {
                    from: s.from,
                    stream_id: s.stream_id.clone(),
                    kind: s.kind,
                    meta: s.meta.clone(),
                })
                .collect(),
            // Filled in by the hub, which knows the relays.
            ice_servers: Vec::new(),
        }
    }
}

fn participant_info(p: &Participant) -> PeerInfo {
    PeerInfo {
        peer_id: p.peer_id,
        nickname: p.nickname.clone(),
        state: if p.conn.is_some() {
            ConnectionState::Connected
        } else {
            ConnectionState::Reconnecting
        },
    }
}

fn clean_nickname(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    let chars = trimmed.chars().count();
    ((1..=MAX_NICKNAME_CHARS).contains(&chars) && !trimmed.chars().any(char::is_control))
        .then(|| trimmed.to_owned())
}

fn valid_token(token: &str) -> bool {
    (16..=128).contains(&token.len())
        && token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn send(conns: &HashMap<ConnId, Conn>, id: ConnId, message: ServerMessage) {
    if let Some(conn) = conns.get(&id) {
        let _ = conn.tx.send(Out::Message(message));
    }
}

fn send_error(conns: &HashMap<ConnId, Conn>, id: ConnId, code: ErrorCode, message: &str) {
    send(
        conns,
        id,
        ServerMessage::Error {
            code,
            message: message.to_owned(),
            retry_after_s: None,
        },
    );
}

fn broadcast(
    conns: &HashMap<ConnId, Conn>,
    room: &Room,
    except: Option<PeerId>,
    message: &ServerMessage,
) {
    for p in &room.participants {
        if Some(p.peer_id) == except {
            continue;
        }
        if let Some(conn) = p.conn {
            send(conns, conn, message.clone());
        }
    }
}

/// Delayed work queued by handlers and spawned after the lock is released.
#[derive(Debug)]
enum Timer {
    Drop {
        code: RoomCode,
        peer: PeerId,
        epoch: u64,
    },
    Approval {
        code: RoomCode,
        request_id: String,
    },
    Expire {
        code: RoomCode,
        epoch: u64,
    },
}

impl Timer {
    fn delay(&self, cfg: &Config) -> Duration {
        match self {
            Timer::Drop { .. } => cfg.reconnect_grace,
            Timer::Approval { .. } => cfg.approval_timeout,
            Timer::Expire { .. } => cfg.empty_room_grace,
        }
    }
}

struct Inner {
    cfg: Config,
    conns: HashMap<ConnId, Conn>,
    rooms: HashMap<RoomCode, Room>,
    limiter: RateLimiter,
    next_conn: u64,
    next_seq: u64,
    next_request: u64,
    timers: Vec<Timer>,
    /// Relays handed to everyone who joins a room (see `relay.rs`).
    ice_servers: Vec<IceServer>,
}

impl Inner {
    fn new(cfg: Config) -> Self {
        let limiter =
            RateLimiter::new(cfg.max_failed_joins, cfg.failed_join_window, cfg.join_block);
        Self {
            cfg,
            conns: HashMap::new(),
            rooms: HashMap::new(),
            limiter,
            next_conn: 0,
            next_seq: 0,
            next_request: 0,
            timers: Vec::new(),
            ice_servers: Vec::new(),
        }
    }

    fn error(&self, conn: ConnId, code: ErrorCode, message: &str) {
        send_error(&self.conns, conn, code, message);
    }

    fn require_unbound(&self, conn: ConnId) -> bool {
        match self.conns.get(&conn).map(|c| &c.binding) {
            Some(Binding::Unbound) => true,
            Some(_) => {
                self.error(
                    conn,
                    ErrorCode::BadMessage,
                    "already in a room or waiting for approval",
                );
                false
            }
            None => false,
        }
    }

    fn require_member(&self, conn: ConnId) -> Option<(RoomCode, PeerId)> {
        match self.conns.get(&conn).map(|c| &c.binding) {
            Some(Binding::Member { code, peer }) => Some((code.clone(), *peer)),
            Some(_) => {
                self.error(conn, ErrorCode::BadMessage, "not in a room");
                None
            }
            None => None,
        }
    }

    fn set_binding(&mut self, conn: ConnId, binding: Binding) {
        if let Some(c) = self.conns.get_mut(&conn) {
            c.binding = binding;
        }
    }

    fn new_participant(&mut self, nickname: String, token: String) -> Participant {
        self.next_seq += 1;
        Participant {
            peer_id: Uuid::new_v4(),
            nickname,
            token,
            seq: self.next_seq,
            conn: None,
            drop_gen: 0,
        }
    }

    fn handle(&mut self, conn: ConnId, message: ClientMessage) {
        match message {
            ClientMessage::CreateRoom {
                nickname,
                session_token,
                require_approval,
            } => self.create_room(conn, &nickname, session_token, require_approval),
            ClientMessage::JoinRoom {
                code,
                nickname,
                session_token,
            } => self.join_room(conn, &code, &nickname, session_token),
            ClientMessage::SetNickname { nickname } => self.set_nickname(conn, &nickname),
            ClientMessage::SetApproval { require_approval } => {
                self.set_approval(conn, require_approval);
            }
            ClientMessage::ApprovalDecision { request_id, accept } => {
                self.approval_decision(conn, &request_id, accept);
            }
            ClientMessage::LeaveRoom => self.leave_room(conn),
            ClientMessage::Signal { to, kind, payload } => {
                self.relay_to(conn, to, |from| ServerMessage::Signal {
                    from,
                    kind,
                    payload,
                });
            }
            ClientMessage::StreamState {
                stream_id,
                kind,
                live,
                meta,
            } => self.stream_state(conn, stream_id, kind, live, meta),
            ClientMessage::LayerRequest {
                to,
                stream_id,
                layer,
            } => {
                self.relay_to(conn, to, |from| ServerMessage::LayerRequest {
                    from,
                    stream_id,
                    layer,
                });
            }
            ClientMessage::LatencyReport { samples } => self.latency_report(conn, samples),
        }
    }

    // ----- joining ---------------------------------------------------------

    fn create_room(&mut self, conn: ConnId, nickname: &str, token: String, require_approval: bool) {
        if !self.require_unbound(conn) {
            return;
        }
        let Some(nickname) = clean_nickname(nickname) else {
            self.error(
                conn,
                ErrorCode::InvalidNickname,
                "nickname must have 1 to 24 characters",
            );
            return;
        };
        if !valid_token(&token) {
            self.error(conn, ErrorCode::BadMessage, "invalid session_token");
            return;
        }
        let code = loop {
            let candidate = RoomCode::generate();
            if !self.rooms.contains_key(&candidate) {
                break candidate;
            }
        };
        self.rooms.insert(
            code.clone(),
            Room {
                participants: Vec::new(),
                dormant: Vec::new(),
                admin: None,
                require_approval,
                pending: Vec::new(),
                streams: Vec::new(),
                expiry_gen: 0,
            },
        );
        let participant = self.new_participant(nickname, token);
        self.seat(&code, conn, participant, true);
    }

    fn join_room(&mut self, conn: ConnId, code_text: &str, nickname: &str, token: String) {
        if !self.require_unbound(conn) {
            return;
        }
        let Some(ip) = self.conns.get(&conn).map(|c| c.ip) else {
            return;
        };
        let now = Instant::now();
        if let Err(retry) = self.limiter.check(ip, now) {
            send(
                &self.conns,
                conn,
                ServerMessage::Error {
                    code: ErrorCode::RateLimited,
                    message: "too many failed attempts".to_owned(),
                    retry_after_s: Some(retry.as_secs().max(1)),
                },
            );
            return;
        }
        let Some(nickname) = clean_nickname(nickname) else {
            self.error(
                conn,
                ErrorCode::InvalidNickname,
                "nickname must have 1 to 24 characters",
            );
            return;
        };
        if !valid_token(&token) {
            self.error(conn, ErrorCode::BadMessage, "invalid session_token");
            return;
        }
        let Some(code) = RoomCode::parse(code_text).filter(|c| self.rooms.contains_key(c)) else {
            self.limiter.record_failure(ip, now);
            self.error(conn, ErrorCode::InvalidCode, "unknown or expired room code");
            return;
        };

        enum Path {
            Reconnect(usize),
            Restore(usize),
            Fresh { admin: bool, approval: bool },
        }
        let path = {
            let room = &self.rooms[&code];
            if let Some(i) = room.participants.iter().position(|p| p.token == token) {
                Path::Reconnect(i)
            } else if let Some(i) = room.dormant.iter().position(|p| p.token == token) {
                Path::Restore(i)
            } else {
                Path::Fresh {
                    admin: room.participants.is_empty(),
                    approval: room.require_approval && !room.participants.is_empty(),
                }
            }
        };

        match path {
            Path::Reconnect(index) => self.reconnect(&code, index, conn),
            Path::Restore(index) => {
                let room = self.rooms.get_mut(&code).expect("room checked above");
                let participant = room.dormant.remove(index);
                let admin = room.participants.is_empty();
                self.seat(&code, conn, participant, admin);
            }
            Path::Fresh { admin, approval } => {
                if approval {
                    self.queue_for_approval(&code, conn, nickname, token);
                } else {
                    let participant = self.new_participant(nickname, token);
                    self.seat(&code, conn, participant, admin);
                }
            }
        }
    }

    /// Puts `participant` in the room, tells everyone and answers `conn`.
    fn seat(&mut self, code: &RoomCode, conn: ConnId, mut participant: Participant, admin: bool) {
        let Some(room) = self.rooms.get_mut(code) else {
            return;
        };
        participant.conn = Some(conn);
        participant.nickname = room.unique_nickname(&participant.nickname, None);
        let peer = participant.peer_id;
        room.participants.push(participant);
        room.participants.sort_by_key(|p| p.seq);
        room.expiry_gen += 1;
        if admin {
            room.admin = Some(peer);
        }
        if let Some(info) = room.info(peer) {
            broadcast(
                &self.conns,
                room,
                Some(peer),
                &ServerMessage::PeerJoined { peer: info },
            );
        }
        let mut joined = room.joined_message(peer);
        if let ServerMessage::RoomJoined {
            code: shown,
            ice_servers,
            ..
        } = &mut joined
        {
            *shown = code.to_string();
            ice_servers.clone_from(&self.ice_servers);
        }
        send(&self.conns, conn, joined);
        if room.participants.len() > self.cfg.capacity_warning {
            broadcast(
                &self.conns,
                room,
                None,
                &ServerMessage::RoomOverCapacity {
                    count: room.participants.len(),
                },
            );
        }
        self.set_binding(
            conn,
            Binding::Member {
                code: code.clone(),
                peer,
            },
        );
    }

    fn reconnect(&mut self, code: &RoomCode, index: usize, conn: ConnId) {
        let Some(room) = self.rooms.get_mut(code) else {
            return;
        };
        let participant = &mut room.participants[index];
        let peer = participant.peer_id;
        let previous = participant.conn.replace(conn);
        participant.drop_gen += 1;
        match previous {
            Some(old) if old != conn => {
                // Same session opened twice: the newer connection wins.
                if let Some(c) = self.conns.get_mut(&old) {
                    c.binding = Binding::Unbound;
                    let _ = c.tx.send(Out::Close);
                }
            }
            Some(_) => {}
            None => broadcast(
                &self.conns,
                room,
                Some(peer),
                &ServerMessage::PeerConnection {
                    peer_id: peer,
                    state: ConnectionState::Connected,
                },
            ),
        }
        let mut joined = room.joined_message(peer);
        if let ServerMessage::RoomJoined {
            code: shown,
            ice_servers,
            ..
        } = &mut joined
        {
            *shown = code.to_string();
            ice_servers.clone_from(&self.ice_servers);
        }
        send(&self.conns, conn, joined);
        if room.admin == Some(peer) {
            for p in &room.pending {
                send(
                    &self.conns,
                    conn,
                    ServerMessage::ApprovalRequest {
                        request_id: p.request_id.clone(),
                        nickname: p.nickname.clone(),
                    },
                );
            }
        }
        self.set_binding(
            conn,
            Binding::Member {
                code: code.clone(),
                peer,
            },
        );
    }

    fn queue_for_approval(
        &mut self,
        code: &RoomCode,
        conn: ConnId,
        nickname: String,
        token: String,
    ) {
        self.next_request += 1;
        let request_id = format!("req-{}", self.next_request);
        let Some(room) = self.rooms.get_mut(code) else {
            return;
        };
        if let Some(admin_conn) = room
            .admin
            .and_then(|a| room.participants.iter().find(|p| p.peer_id == a))
            .and_then(|p| p.conn)
        {
            send(
                &self.conns,
                admin_conn,
                ServerMessage::ApprovalRequest {
                    request_id: request_id.clone(),
                    nickname: nickname.clone(),
                },
            );
        }
        room.pending.push(Pending {
            request_id: request_id.clone(),
            conn,
            nickname,
            token,
        });
        send(&self.conns, conn, ServerMessage::ApprovalPending);
        self.timers.push(Timer::Approval {
            code: code.clone(),
            request_id: request_id.clone(),
        });
        self.set_binding(
            conn,
            Binding::Pending {
                code: code.clone(),
                request_id,
            },
        );
    }

    // ----- admin actions ---------------------------------------------------

    fn require_admin(&self, conn: ConnId) -> Option<RoomCode> {
        let (code, peer) = self.require_member(conn)?;
        if self.rooms.get(&code)?.admin == Some(peer) {
            Some(code)
        } else {
            self.error(conn, ErrorCode::NotAdmin, "only the room admin can do this");
            None
        }
    }

    fn approval_decision(&mut self, conn: ConnId, request_id: &str, accept: bool) {
        let Some(code) = self.require_admin(conn) else {
            return;
        };
        let pending = {
            let room = self.rooms.get_mut(&code).expect("room exists for admin");
            let Some(index) = room.pending.iter().position(|p| p.request_id == request_id) else {
                self.error(conn, ErrorCode::BadMessage, "unknown request_id");
                return;
            };
            room.pending.remove(index)
        };
        self.resolve_pending(&code, pending, accept);
    }

    fn resolve_pending(&mut self, code: &RoomCode, pending: Pending, accept: bool) {
        if accept {
            let participant = self.new_participant(pending.nickname, pending.token);
            self.seat(code, pending.conn, participant, false);
        } else {
            self.deny(pending.conn);
        }
    }

    fn deny(&mut self, conn: ConnId) {
        self.error(conn, ErrorCode::JoinDenied, "join request was not accepted");
        self.set_binding(conn, Binding::Unbound);
        if let Some(c) = self.conns.get(&conn) {
            let _ = c.tx.send(Out::Close);
        }
    }

    fn set_approval(&mut self, conn: ConnId, require_approval: bool) {
        let Some(code) = self.require_admin(conn) else {
            return;
        };
        let admitted = {
            let room = self.rooms.get_mut(&code).expect("room exists for admin");
            room.require_approval = require_approval;
            broadcast(
                &self.conns,
                room,
                None,
                &ServerMessage::ApprovalChanged { require_approval },
            );
            if require_approval {
                Vec::new()
            } else {
                std::mem::take(&mut room.pending)
            }
        };
        // Turning approval off lets everyone who was waiting in.
        for pending in admitted {
            self.resolve_pending(&code, pending, true);
        }
    }

    // ----- leaving ---------------------------------------------------------

    fn leave_room(&mut self, conn: ConnId) {
        let binding = self.conns.get(&conn).map(|c| c.binding.clone());
        match binding {
            Some(Binding::Member { code, peer }) => {
                self.remove_participant(&code, peer, LeaveReason::Left, false);
            }
            Some(Binding::Pending { code, request_id }) => {
                if let Some(room) = self.rooms.get_mut(&code) {
                    room.pending.retain(|p| p.request_id != request_id);
                }
            }
            _ => {}
        }
        self.set_binding(conn, Binding::Unbound);
    }

    fn remove_participant(
        &mut self,
        code: &RoomCode,
        peer: PeerId,
        reason: LeaveReason,
        keep: bool,
    ) {
        let Some(room) = self.rooms.get_mut(code) else {
            return;
        };
        let Some(index) = room.participants.iter().position(|p| p.peer_id == peer) else {
            return;
        };
        let mut participant = room.participants.remove(index);

        let (ended, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut room.streams)
            .into_iter()
            .partition(|s| s.from == peer);
        room.streams = kept;
        for stream in ended {
            broadcast(
                &self.conns,
                room,
                None,
                &ServerMessage::StreamState {
                    from: peer,
                    stream_id: stream.stream_id,
                    kind: stream.kind,
                    live: false,
                    meta: Value::Null,
                },
            );
        }
        broadcast(
            &self.conns,
            room,
            None,
            &ServerMessage::PeerLeft {
                peer_id: peer,
                reason,
            },
        );

        if keep {
            participant.conn = None;
            room.dormant.push(participant);
        }
        if room.participants.is_empty() {
            room.admin = None;
            room.expiry_gen += 1;
            self.timers.push(Timer::Expire {
                code: code.clone(),
                epoch: room.expiry_gen,
            });
        } else if room.admin == Some(peer) {
            Self::transfer_admin(&self.conns, room);
        }
    }

    /// DEC-22: the oldest connected participant takes over.
    fn transfer_admin(conns: &HashMap<ConnId, Conn>, room: &mut Room) {
        let next = room
            .participants
            .iter()
            .find(|p| p.conn.is_some())
            .or_else(|| room.participants.first())
            .map(|p| (p.peer_id, p.conn));
        let Some((peer, admin_conn)) = next else {
            room.admin = None;
            return;
        };
        room.admin = Some(peer);
        broadcast(
            conns,
            room,
            None,
            &ServerMessage::AdminChanged { peer_id: peer },
        );
        if let Some(admin_conn) = admin_conn {
            for p in &room.pending {
                send(
                    conns,
                    admin_conn,
                    ServerMessage::ApprovalRequest {
                        request_id: p.request_id.clone(),
                        nickname: p.nickname.clone(),
                    },
                );
            }
        }
    }

    fn disconnect(&mut self, conn: ConnId) {
        let Some(closed) = self.conns.remove(&conn) else {
            return;
        };
        match closed.binding {
            Binding::Unbound => {}
            Binding::Pending { code, request_id } => {
                if let Some(room) = self.rooms.get_mut(&code) {
                    room.pending.retain(|p| p.request_id != request_id);
                }
            }
            Binding::Member { code, peer } => {
                let Some(room) = self.rooms.get_mut(&code) else {
                    return;
                };
                let Some(p) = room
                    .participants
                    .iter_mut()
                    .find(|p| p.peer_id == peer && p.conn == Some(conn))
                else {
                    return;
                };
                p.conn = None;
                p.drop_gen += 1;
                let epoch = p.drop_gen;
                broadcast(
                    &self.conns,
                    room,
                    Some(peer),
                    &ServerMessage::PeerConnection {
                        peer_id: peer,
                        state: ConnectionState::Reconnecting,
                    },
                );
                self.timers.push(Timer::Drop { code, peer, epoch });
            }
        }
    }

    // ----- timers ----------------------------------------------------------

    fn fire(&mut self, timer: Timer) {
        match timer {
            Timer::Drop { code, peer, epoch } => {
                let stale = self
                    .rooms
                    .get(&code)
                    .and_then(|r| r.participants.iter().find(|p| p.peer_id == peer))
                    .is_none_or(|p| p.drop_gen != epoch || p.conn.is_some());
                if !stale {
                    self.remove_participant(&code, peer, LeaveReason::Timeout, true);
                }
            }
            Timer::Approval { code, request_id } => {
                let pending = self.rooms.get_mut(&code).and_then(|room| {
                    let index = room
                        .pending
                        .iter()
                        .position(|p| p.request_id == request_id)?;
                    Some(room.pending.remove(index))
                });
                if let Some(pending) = pending {
                    self.deny(pending.conn);
                }
            }
            Timer::Expire { code, epoch } => {
                let expired = self
                    .rooms
                    .get(&code)
                    .is_some_and(|r| r.participants.is_empty() && r.expiry_gen == epoch);
                if expired {
                    self.rooms.remove(&code);
                }
            }
        }
    }

    // ----- room traffic ----------------------------------------------------

    fn set_nickname(&mut self, conn: ConnId, nickname: &str) {
        let Some((code, peer)) = self.require_member(conn) else {
            return;
        };
        let Some(nickname) = clean_nickname(nickname) else {
            self.error(
                conn,
                ErrorCode::InvalidNickname,
                "nickname must have 1 to 24 characters",
            );
            return;
        };
        let Some(room) = self.rooms.get_mut(&code) else {
            return;
        };
        let nickname = room.unique_nickname(&nickname, Some(peer));
        if let Some(p) = room.participants.iter_mut().find(|p| p.peer_id == peer) {
            p.nickname.clone_from(&nickname);
        }
        broadcast(
            &self.conns,
            room,
            None,
            &ServerMessage::PeerRenamed {
                peer_id: peer,
                nickname,
            },
        );
    }

    fn relay_to(&self, conn: ConnId, to: PeerId, build: impl FnOnce(PeerId) -> ServerMessage) {
        let Some((code, from)) = self.require_member(conn) else {
            return;
        };
        let target = self
            .rooms
            .get(&code)
            .and_then(|r| r.participants.iter().find(|p| p.peer_id == to));
        match target {
            Some(p) if p.conn.is_some() => {
                if let Some(target_conn) = p.conn {
                    send(&self.conns, target_conn, build(from));
                }
            }
            Some(_) => self.error(conn, ErrorCode::BadMessage, "peer is reconnecting"),
            None => self.error(conn, ErrorCode::BadMessage, "unknown peer"),
        }
    }

    fn stream_state(
        &mut self,
        conn: ConnId,
        stream_id: String,
        kind: StreamKind,
        live: bool,
        meta: Value,
    ) {
        let Some((code, peer)) = self.require_member(conn) else {
            return;
        };
        if stream_id.is_empty() || stream_id.chars().count() > MAX_STREAM_ID_CHARS {
            self.error(conn, ErrorCode::BadMessage, "invalid stream_id");
            return;
        }
        let Some(room) = self.rooms.get_mut(&code) else {
            return;
        };
        room.streams
            .retain(|s| !(s.from == peer && s.stream_id == stream_id));
        if live {
            room.streams.push(StreamEntry {
                from: peer,
                stream_id: stream_id.clone(),
                kind,
                meta: meta.clone(),
            });
        }
        broadcast(
            &self.conns,
            room,
            Some(peer),
            &ServerMessage::StreamState {
                from: peer,
                stream_id,
                kind,
                live,
                meta,
            },
        );
    }

    fn latency_report(&self, conn: ConnId, samples: Vec<LatencySample>) {
        let Some((code, peer)) = self.require_member(conn) else {
            return;
        };
        if samples.len() > MAX_LATENCY_SAMPLES {
            self.error(conn, ErrorCode::BadMessage, "too many latency samples");
            return;
        }
        if let Some(room) = self.rooms.get(&code) {
            broadcast(
                &self.conns,
                room,
                Some(peer),
                &ServerMessage::LatencyReport {
                    from: peer,
                    samples,
                },
            );
        }
    }
}

/// Shared handle used by every connection task.
pub struct Hub {
    cfg: Config,
    inner: Mutex<Inner>,
}

impl Hub {
    pub fn new(cfg: Config) -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(Inner::new(cfg.clone())),
            cfg,
        })
    }

    pub fn config(&self) -> &Config {
        &self.cfg
    }

    /// Sets the relays that new joiners receive. Credentials expire, so the
    /// server replaces them now and then.
    pub fn set_ice_servers(&self, servers: Vec<IceServer>) {
        let mut inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        inner.ice_servers = servers;
    }

    /// Registers a new, unbound connection.
    pub fn register(&self, ip: IpAddr, tx: UnboundedSender<Out>) -> ConnId {
        let mut inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        inner.next_conn += 1;
        let id = ConnId(inner.next_conn);
        inner.conns.insert(
            id,
            Conn {
                tx,
                ip,
                binding: Binding::Unbound,
            },
        );
        id
    }

    /// Parses and handles one text frame.
    pub fn handle_text(self: &Arc<Self>, conn: ConnId, text: &str) {
        let value: Value = match serde_json::from_str(text) {
            Ok(value) => value,
            Err(_) => return self.reject(conn, ErrorCode::BadMessage, "invalid JSON"),
        };
        match value.get("v").and_then(Value::as_u64) {
            Some(v) if v == u64::from(PROTOCOL_VERSION) => {}
            Some(_) => {
                return self.reject(
                    conn,
                    ErrorCode::UnsupportedVersion,
                    "unsupported protocol version",
                );
            }
            None => return self.reject(conn, ErrorCode::BadMessage, "missing protocol version"),
        }
        match serde_json::from_value::<ClientMessage>(value) {
            Ok(message) => self.with_inner(|inner| inner.handle(conn, message)),
            Err(error) => self.reject(conn, ErrorCode::BadMessage, &error.to_string()),
        }
    }

    /// Sends an error to one connection.
    pub fn reject(&self, conn: ConnId, code: ErrorCode, message: &str) {
        let inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        inner.error(conn, code, message);
    }

    /// Asks the connection task to close the socket.
    pub fn close(&self, conn: ConnId) {
        let inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(c) = inner.conns.get(&conn) {
            let _ = c.tx.send(Out::Close);
        }
    }

    /// Called when the socket is gone.
    pub fn disconnect(self: &Arc<Self>, conn: ConnId) {
        self.with_inner(|inner| inner.disconnect(conn));
    }

    fn with_inner<R>(self: &Arc<Self>, f: impl FnOnce(&mut Inner) -> R) -> R {
        let (result, timers) = {
            let mut inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
            let result = f(&mut inner);
            (result, std::mem::take(&mut inner.timers))
        };
        self.schedule(timers);
        result
    }

    /// Not generic on purpose: timers spawn tasks that call back into the hub.
    fn schedule(self: &Arc<Self>, timers: Vec<Timer>) {
        for timer in timers {
            let hub = Arc::clone(self);
            let delay = timer.delay(&self.cfg);
            tokio::spawn(async move {
                tokio::time::sleep(delay).await;
                let timers = {
                    let mut inner = hub.inner.lock().unwrap_or_else(PoisonError::into_inner);
                    inner.fire(timer);
                    std::mem::take(&mut inner.timers)
                };
                hub.schedule(timers);
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ip_lists() {
        let ips = parse_ip_list(" 10.0.0.5, fd00::5 ,,127.0.0.1").unwrap();
        assert_eq!(ips.len(), 3);
        assert_eq!(ips[0], IpAddr::from([10, 0, 0, 5]));
        assert!(parse_ip_list("").unwrap().is_empty());
    }

    #[test]
    fn rejects_bad_ip_lists() {
        assert!(parse_ip_list("10.0.0.5, nope").is_err());
        assert!(parse_ip_list("10.0.0.0/24").is_err());
    }
}
