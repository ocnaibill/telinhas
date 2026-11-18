//! What the signaling client reports, turned into changes of what the
//! interface shows.

use telinhas_net::{Command, Event, Failure, RoomSnapshot};
use telinhas_protocol::{ConnectionState as WireState, LeaveReason};

use crate::app::{JoinRequest, Link, TelinhasApp};
use crate::model::{ConnectionState, Participant, Room, Screen};

impl TelinhasApp {
    /// Reads every event the client has for us.
    pub(crate) fn poll_net(&mut self) {
        while let Some(event) = self.net.as_ref().and_then(|net| net.try_event()) {
            self.on_event(event);
        }
        while let Some(event) = self.media.as_ref().and_then(|media| media.try_event()) {
            self.on_media_event(event);
        }
    }

    /// Gives the sound output the broadcasters' sound, with each one's volume.
    /// The output opens when the first sound arrives, not before.
    pub(crate) fn update_playback(&mut self) {
        let Some(media) = &self.media else {
            return;
        };
        let wanted: Vec<_> = self
            .room
            .iter()
            .flat_map(|room| {
                let me = room.me_id();
                room.streams
                    .iter()
                    .filter(move |s| s.owner_id.is_some() && s.owner_id != me)
            })
            .filter_map(|s| {
                let id = s.owner_id?;
                Some((id, media.audio(id)?, s.volume))
            })
            .collect();
        if self.playback.is_none() {
            let sound_arrived = wanted
                .iter()
                .any(|(_, buffer, _)| buffer.buffered_ms() > 0.0);
            if !sound_arrived || self.playback_failed {
                return;
            }
            match crate::playback::Playback::start() {
                Ok(playback) => self.playback = Some(playback),
                Err(reason) => {
                    self.playback_failed = true;
                    self.show_toast(format!("Sem saída de áudio: {reason}"));
                    return;
                }
            }
        }
        if let Some(playback) = &self.playback {
            playback.set_sources(wanted);
        }
    }

    fn on_media_event(&mut self, event: telinhas_media::Event) {
        use telinhas_media::Event as Media;
        match event {
            Media::Signal { to, kind, payload } => {
                self.send(Command::Signal { to, kind, payload });
            }
            Media::Encoder { hardware, .. } => self.sim.software_encoder = !hardware,
            Media::BroadcastFailed(reason) => {
                if let Some(room) = self.room.as_mut() {
                    room.broadcasting = false;
                    let me = room.me_id();
                    room.streams.retain(|s| s.owner_id != me);
                }
                self.send(Command::AnnounceStream {
                    stream_id: "screen".to_owned(),
                    kind: telinhas_protocol::StreamKind::Screen,
                    live: false,
                    meta: telinhas_protocol::Value::Null,
                });
                self.show_toast(format!("A transmissão parou: {reason}"));
            }
            Media::AudioFailed(reason) => {
                self.show_toast(format!("O áudio parou, o vídeo continua: {reason}"));
            }
            Media::Receiving(_) => {}
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
                if let Some(media) = &self.media {
                    media.send(telinhas_media::Command::PeerJoined(info.peer_id));
                }
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
                if let Some(media) = &self.media {
                    media.send(telinhas_media::Command::PeerLeft(peer_id));
                }
                let mut name = None;
                if let Some(room) = self.room.as_mut() {
                    name = room.name_of(peer_id).map(str::to_owned);
                    room.participants.retain(|p| p.peer_id != Some(peer_id));
                    room.streams.retain(|s| s.owner_id != Some(peer_id));
                    room.focus = 0;
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
                if !live && let Some(media) = &self.media {
                    media.send(telinhas_media::Command::StreamEnded(from));
                }
                if let Some(room) = self.room.as_mut() {
                    room.apply_stream(from, &stream_id, kind, live, &meta);
                    room.focus = room.focus.min(room.streams.len().saturating_sub(1));
                }
            }
            Event::Reconnecting { .. } => self.reconnecting = true,
            Event::Failure {
                kind,
                message,
                fatal,
            } => self.on_failure(kind, &message, fatal),
            Event::Left => {}
            Event::Signal {
                from,
                kind,
                payload,
            } => {
                if let Some(media) = &self.media {
                    media.send(telinhas_media::Command::Signal {
                        from,
                        kind,
                        payload,
                    });
                }
            }
            // Layers and latency belong to later milestones.
            Event::LayerRequest { .. } | Event::LatencyReport { .. } | Event::OverCapacity(_) => {}
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
        let open_broadcast = self.home.mode == crate::model::EntryMode::Broadcasting;
        self.room = Some(room);
        self.screen = Screen::Room;
        self.sim = crate::app::Sim::default();
        self.reconnecting = false;
        if open_broadcast {
            self.open_broadcast_modal();
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
