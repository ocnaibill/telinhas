//! What the signaling client reports, turned into changes of what the
//! interface shows.

use telinhas_net::{Command, Event, Failure, RoomSnapshot};
use telinhas_protocol::{ConnectionState as WireState, LeaveReason};

use crate::app::{JoinRequest, Link, TelinhasApp};
use crate::mockvideo::MockSource;
use crate::model::{ConnectionState, Participant, Room, Screen};

impl TelinhasApp {
    /// Reads every event the client has for us.
    pub(crate) fn poll_net(&mut self) {
        while let Some(event) = self.net.as_ref().and_then(|net| net.try_event()) {
            self.on_event(event);
        }
    }

    pub(crate) fn on_event(&mut self, event: Event) {
        match event {
            Event::Connecting => self.link = Link::Connecting,
            Event::ApprovalPending => self.link = Link::WaitingApproval,
            Event::Joined(snapshot) => self.on_joined(snapshot),
            Event::JoinDenied => {
                self.link = Link::Idle;
                self.notice = Some(
                    "O administrador não aceitou sua entrada, ou não respondeu a tempo.".to_owned(),
                );
            }
            Event::ApprovalRequest {
                request_id,
                nickname,
            } => {
                self.requests.push(JoinRequest {
                    request_id,
                    nickname,
                });
            }
            Event::PeerJoined(info) => {
                let name = info.nickname.clone();
                if let Some(room) = self.room.as_mut() {
                    room.participants.push(Participant {
                        peer_id: Some(info.peer_id),
                        name: info.nickname,
                        admin: false,
                        latency_ms: 0,
                        state: match info.state {
                            WireState::Connected => ConnectionState::Connected,
                            WireState::Reconnecting => ConnectionState::Reconnecting,
                        },
                        is_me: false,
                    });
                }
                self.show_toast(format!("{name} entrou na sala"));
            }
            Event::PeerLeft { peer_id, reason } => {
                let mut name = None;
                if let Some(room) = self.room.as_mut() {
                    name = room.name_of(peer_id).map(str::to_owned);
                    room.participants.retain(|p| p.peer_id != Some(peer_id));
                    let before = room.streams.len();
                    let kept: Vec<bool> = room
                        .streams
                        .iter()
                        .map(|s| s.owner_id != Some(peer_id))
                        .collect();
                    let mut index = 0;
                    room.streams.retain(|_| {
                        index += 1;
                        kept[index - 1]
                    });
                    // The test pictures follow the streams that stay.
                    if room.streams.len() != before {
                        let mut index = 0;
                        self.mock.retain(|_| {
                            index += 1;
                            kept.get(index - 1).copied().unwrap_or(true)
                        });
                        room.focus = 0;
                    }
                }
                if let Some(name) = name {
                    self.show_toast(match reason {
                        LeaveReason::Left => format!("{name} saiu da sala"),
                        LeaveReason::Timeout => format!("{name} caiu e não voltou"),
                    });
                }
            }
            Event::PeerConnection { peer_id, state } => {
                if let Some(room) = self.room.as_mut()
                    && let Some(person) = room
                        .participants
                        .iter_mut()
                        .find(|p| p.peer_id == Some(peer_id))
                {
                    person.state = match state {
                        WireState::Connected => ConnectionState::Connected,
                        WireState::Reconnecting => ConnectionState::Reconnecting,
                    };
                }
            }
            Event::PeerRenamed { peer_id, nickname } => {
                if let Some(room) = self.room.as_mut() {
                    for p in room
                        .participants
                        .iter_mut()
                        .filter(|p| p.peer_id == Some(peer_id))
                    {
                        p.name.clone_from(&nickname);
                    }
                    for s in room
                        .streams
                        .iter_mut()
                        .filter(|s| s.owner_id == Some(peer_id))
                    {
                        s.owner.clone_from(&nickname);
                    }
                }
            }
            Event::AdminChanged(peer_id) => {
                let mut me = false;
                if let Some(room) = self.room.as_mut() {
                    for p in &mut room.participants {
                        p.admin = p.peer_id == Some(peer_id);
                        me |= p.admin && p.is_me;
                    }
                }
                if me {
                    self.show_toast("Você agora é o administrador da sala");
                }
            }
            Event::ApprovalChanged(on) => {
                if let Some(room) = self.room.as_mut() {
                    room.require_approval = on;
                }
            }
            Event::StreamState {
                from,
                stream_id,
                kind,
                live,
                meta,
            } => {
                if let Some(room) = self.room.as_mut() {
                    let before = room.streams.len();
                    room.apply_stream(from, &stream_id, kind, live, &meta);
                    if room.streams.len() > before {
                        self.mock
                            .push(MockSource::start(1280, 720, 30, (before % 6) as u8));
                    } else if room.streams.len() < before && !self.mock.is_empty() {
                        self.mock.pop();
                    }
                }
            }
            Event::Reconnecting { .. } => self.reconnecting = true,
            Event::Failure {
                kind,
                message,
                fatal,
            } => self.on_failure(kind, &message, fatal),
            Event::Left => {}
            // Media negotiation and latency belong to the video milestone.
            Event::Signal { .. }
            | Event::LayerRequest { .. }
            | Event::LatencyReport { .. }
            | Event::OverCapacity(_) => {}
        }
    }

    fn on_joined(&mut self, snapshot: RoomSnapshot) {
        self.link = Link::Idle;
        self.notice = None;
        if self.screen == Screen::Room && self.room.is_some() {
            // Back after a dropped connection: same room, fresh state.
            let was_broadcasting = self.room.as_ref().is_some_and(|r| r.broadcasting);
            let mut room = Room::from_snapshot(&snapshot);
            room.broadcasting = was_broadcasting;
            if let Some(old) = self.room.as_ref() {
                room.layout = old.layout;
                room.mic_on = old.mic_on;
                room.camera_on = old.camera_on;
            }
            self.room = Some(room);
            self.reconnecting = false;
            self.show_toast("De volta à sala");
            return;
        }
        let room = Room::from_snapshot(&snapshot);
        self.mock = (0..room.streams.len())
            .map(|i| MockSource::start(1280, 720, 30, i as u8))
            .collect();
        self.mock_webcam = room
            .streams
            .iter()
            .any(|s| s.has_webcam)
            .then(|| MockSource::start(640, 360, 24, 3));
        let open_broadcast = self.home.mode == crate::model::EntryMode::Broadcasting;
        self.room = Some(room);
        self.screen = Screen::Room;
        self.sim = crate::app::Sim::default();
        self.reconnecting = false;
        if open_broadcast {
            let mut draft = crate::model::BroadcastDraft::new();
            draft.clamp(&self.sources);
            self.modal = crate::app::Modal::Broadcast(Box::new(draft));
        }
    }

    fn on_failure(&mut self, kind: Failure, message: &str, fatal: bool) {
        let text = match kind {
            Failure::Unreachable => {
                "Não consegui falar com o servidor. Confira o endereço e a sua conexão.".to_owned()
            }
            Failure::InvalidCode => "Esse código não existe ou a sala já expirou.".to_owned(),
            Failure::RateLimited { retry_after_s } => {
                format!("Muitas tentativas com código errado. Tente de novo em {retry_after_s} s.")
            }
            Failure::InvalidNickname => "O servidor recusou o apelido.".to_owned(),
            Failure::NotAdmin => "Só o administrador da sala pode fazer isso.".to_owned(),
            Failure::RoomExpired => {
                "A sala acabou enquanto você estava desconectado. O tempo para voltar passou."
                    .to_owned()
            }
            Failure::Other => message.to_owned(),
        };
        if fatal {
            let was_in_room = self.screen == Screen::Room;
            self.reset_room_state();
            self.notice = Some(text);
            if was_in_room {
                self.screen = Screen::Home;
            }
        } else {
            self.show_toast(text);
        }
    }

    /// Asks the client to take us into a room, and shows the wait.
    pub(crate) fn start_connecting(&mut self, command: Command) {
        self.notice = None;
        self.link = Link::Connecting;
        self.send(command);
    }
}
