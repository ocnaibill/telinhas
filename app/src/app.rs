//! The application state and the main loop: which screen is up, and the
//! pieces every screen shares.

use std::time::{Duration, Instant};

use eframe::egui;

use telinhas_net::{Command, Handle, Settings};

use crate::mockvideo::MockSource;
use crate::model::{
    BroadcastDraft, EntryMode, Room, Screen, Sources, preview_code, valid_nickname,
};
use crate::theme;
use crate::video::VideoGpu;

pub(crate) const NICKNAME_KEY: &str = "nickname";
pub(crate) const SERVER_KEY: &str = "server";
pub(crate) const FILL_KEY: &str = "fill_video";

pub(crate) enum Modal {
    None,
    Broadcast(Box<BroadcastDraft>),
    Nickname,
}

/// Switches that make the interface show states it cannot reach on its own
/// yet (a join request, a dropped connection, software encoding).
/// Where we are on the way into a room, before the room screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Link {
    #[default]
    Idle,
    Connecting,
    WaitingApproval,
}

/// A person asking to come in, for the admin to answer (RF-19).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct JoinRequest {
    pub request_id: String,
    pub nickname: String,
}

#[derive(Default)]
pub(crate) struct Sim {
    pub pending_join: Option<String>,
    pub reconnecting: bool,
    pub software_encoder: bool,
    pub not_admin: bool,
}

pub(crate) struct HomeState {
    pub join_code: String,
    pub mode: EntryMode,
    pub require_approval: bool,
    pub preview_code: String,
    /// The height the content needs, button included, of the taller card.
    pub target: f32,
    /// What each card's content needed on the last frame, [create, join].
    pub natural: [f32; 2],
    /// Each card's full height as drawn, [create, join].
    pub measured: [f32; 2],
}

pub struct TelinhasApp {
    pub(crate) screen: Screen,
    pub(crate) nickname: String,
    pub(crate) nick_draft: String,
    pub(crate) home: HomeState,
    pub(crate) room: Option<Room>,
    pub(crate) sources: Sources,
    pub(crate) modal: Modal,
    pub(crate) sim: Sim,
    /// Test pictures for the room's streams, while in a room.
    pub(crate) mock: Vec<MockSource>,
    pub(crate) mock_webcam: Option<MockSource>,
    pub(crate) toast: Option<(String, Instant)>,
    pub(crate) started: Instant,
    /// Address of the signaling server; empty means demonstration mode.
    pub(crate) server: String,
    pub(crate) server_draft: String,
    pub(crate) net: Option<Handle>,
    pub(crate) media: Option<telinhas_media::Media>,
    pub(crate) link: Link,
    /// A problem to show on the home screen (a wrong code, no server...).
    pub(crate) notice: Option<String>,
    pub(crate) requests: Vec<JoinRequest>,
    /// The connection dropped inside the room and the client is retrying.
    pub(crate) reconnecting: bool,
    /// Only the video and the control bar, in a full-screen window.
    pub(crate) fullscreen: bool,
    /// The numbers panel over the video (F3).
    pub(crate) diagnostics: bool,
    /// Fill the tile and cut the edges, instead of keeping the proportions
    /// (DEC-62). A local choice of whoever watches.
    pub(crate) fill_video: bool,
    pub(crate) wake: Option<egui::Context>,
}

impl TelinhasApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::install(&cc.egui_ctx);
        if let Some(state) = cc.wgpu_render_state.as_ref() {
            state
                .renderer
                .write()
                .callback_resources
                .insert(VideoGpu::new(&state.device, state.target_format));
        }
        // `--first-run` shows the nickname screen again without touching what
        // is saved: the saved nickname only changes when a new one is confirmed.
        let force_first_run = std::env::args().any(|a| a == "--first-run");
        let nickname = if force_first_run {
            String::new()
        } else {
            cc.storage
                .and_then(|s| s.get_string(NICKNAME_KEY))
                .filter(|n| valid_nickname(n))
                .unwrap_or_default()
        };
        let server = server_setting(cc.storage);
        let mut app = Self::build(nickname, Sources::load());
        app.wake = Some(cc.egui_ctx.clone());
        app.fill_video = cc
            .storage
            .and_then(|s| s.get_string(FILL_KEY))
            .is_some_and(|v| v == "true");
        app.set_server(&server);
        app
    }

    /// Points the client at a signaling server. Empty means demonstration mode.
    pub(crate) fn set_server(&mut self, server: &str) {
        self.server = server.trim().to_owned();
        self.server_draft = self.server.clone();
        self.net = None;
        self.media = None;
        if self.server.is_empty() {
            return;
        }
        let ctx = self.wake.clone();
        let media_ctx = self.wake.clone();
        // A server on this machine means a test: no STUN, no outside network.
        let local = ["//127.0.0.1", "//localhost", "//[::1]"]
            .iter()
            .any(|host| self.server.contains(host));
        self.media = Some(telinhas_media::Media::spawn(
            if local {
                telinhas_media::LinkConfig::loopback()
            } else {
                telinhas_media::LinkConfig::internet()
            },
            move || {
                if let Some(ctx) = &media_ctx {
                    ctx.request_repaint();
                }
            },
        ));
        self.net = Some(telinhas_net::spawn(
            Settings {
                server: self.server.clone(),
                nickname: self.nickname.clone(),
            },
            move || {
                if let Some(ctx) = &ctx {
                    ctx.request_repaint();
                }
            },
        ));
    }

    /// Whether the room is the made-up one of the demonstration.
    pub(crate) fn is_demo(&self) -> bool {
        self.net.is_none()
    }

    pub(crate) fn send(&self, command: Command) {
        if let Some(net) = &self.net {
            net.send(command);
        }
    }

    fn build(nickname: String, sources: Sources) -> Self {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(1, |d| d.as_nanos() as u64);
        Self {
            screen: if nickname.is_empty() {
                Screen::FirstRun
            } else {
                Screen::Home
            },
            nick_draft: nickname.clone(),
            nickname,
            home: HomeState {
                join_code: String::new(),
                mode: EntryMode::Watching,
                require_approval: false,
                preview_code: preview_code(seed),
                target: 0.0,
                natural: [0.0; 2],
                measured: [0.0; 2],
            },
            room: None,
            sources,
            modal: Modal::None,
            sim: Sim::default(),
            mock: Vec::new(),
            mock_webcam: None,
            toast: None,
            started: Instant::now(),
            server: String::new(),
            server_draft: String::new(),
            net: None,
            media: None,
            link: Link::Idle,
            notice: None,
            requests: Vec::new(),
            reconnecting: false,
            fullscreen: false,
            diagnostics: false,
            fill_video: false,
            wake: None,
        }
    }

    pub(crate) fn show_toast(&mut self, text: impl Into<String>) {
        self.toast = Some((text.into(), Instant::now()));
    }

    /// Enters a test room, with a few people already in it.
    /// Create or join: through the server when there is one, otherwise the
    /// demonstration room.
    pub(crate) fn enter_room(&mut self, code: &str, create: bool) {
        if self.net.is_some() {
            let command = if create {
                Command::CreateRoom {
                    require_approval: self.home.require_approval,
                }
            } else {
                Command::JoinRoom {
                    code: code.to_owned(),
                }
            };
            self.start_connecting(command);
            return;
        }
        let mut room = Room::demo(code, &self.nickname);
        if create {
            // A new room starts with only me, as its admin.
            room.participants.retain(|p| p.is_me);
            if let Some(me) = room.participants.first_mut() {
                me.admin = true;
            }
            room.streams.clear();
            room.require_approval = self.home.require_approval;
        }
        self.mock = (0..room.streams.len())
            .map(|i| MockSource::start(1280, 720, 30, i as u8))
            .collect();
        self.mock_webcam = room
            .streams
            .iter()
            .any(|s| s.has_webcam)
            .then(|| MockSource::start(640, 360, 24, 3));
        let open_broadcast = self.home.mode == EntryMode::Broadcasting;
        self.room = Some(room);
        self.screen = Screen::Room;
        self.sim = Sim::default();
        if open_broadcast {
            let mut draft = BroadcastDraft::new();
            draft.clamp(&self.sources);
            self.modal = Modal::Broadcast(Box::new(draft));
        }
    }

    pub(crate) fn leave_room(&mut self) {
        if self.net.is_some() {
            self.send(Command::Leave);
        }
        self.reset_room_state();
    }

    /// Back to the home screen, whatever the reason.
    pub(crate) fn reset_room_state(&mut self) {
        if let (Some(media), Some(room)) = (&self.media, &self.room) {
            media.send(telinhas_media::Command::StopBroadcast);
            for peer in room.participants.iter().filter_map(|p| p.peer_id) {
                media.send(telinhas_media::Command::PeerLeft(peer));
            }
        }
        self.room = None;
        self.requests.clear();
        if self.fullscreen {
            self.fullscreen = false;
            if let Some(ctx) = &self.wake {
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
            }
        }
        self.reconnecting = false;
        self.link = Link::Idle;
        self.mock.clear();
        self.mock_webcam = None;
        self.modal = Modal::None;
        self.screen = Screen::Home;
        self.home.preview_code = preview_code(self.started.elapsed().as_nanos() as u64 + 17);
    }
}

impl eframe::App for TelinhasApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll_net();
        // The room has moving pictures and live numbers.
        if self.screen == Screen::Room {
            ctx.request_repaint();
        } else {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
        match self.screen {
            Screen::FirstRun => self.first_run_ui(ui),
            Screen::Home => self.home_ui(ui),
            Screen::Room => self.room_ui(ui),
        }
        self.modals_ui(&ctx);
        self.toast_ui(&ctx);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        if valid_nickname(&self.nickname) {
            storage.set_string(NICKNAME_KEY, self.nickname.clone());
        }
        storage.set_string(SERVER_KEY, self.server.clone());
        storage.set_string(FILL_KEY, self.fill_video.to_string());
    }
}

/// The server address: `--server URL`, then `TELINHAS_SERVER`, then what was
/// saved last time.
fn server_setting(storage: Option<&dyn eframe::Storage>) -> String {
    let mut args = std::env::args();
    while let Some(arg) = args.next() {
        if arg == "--server"
            && let Some(value) = args.next()
        {
            return value;
        }
    }
    if let Ok(value) = std::env::var("TELINHAS_SERVER")
        && !value.trim().is_empty()
    {
        return value;
    }
    storage
        .and_then(|s| s.get_string(SERVER_KEY))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs the interface for a few frames, so layout mistakes panic here
    /// instead of on someone's screen.
    fn frames(app: &mut TelinhasApp, count: usize) -> egui::Context {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        for _ in 0..count {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1360.0, 820.0),
                )),
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| {
                match app.screen {
                    Screen::FirstRun => app.first_run_ui(ui),
                    Screen::Home => app.home_ui(ui),
                    Screen::Room => app.room_ui(ui),
                }
                app.modals_ui(&ui.ctx().clone());
                app.toast_ui(&ui.ctx().clone());
            });
            // Nothing renders in a test, so there is nothing to upload.
            output.textures_delta.clear();
        }
        ctx
    }

    fn app(nickname: &str) -> TelinhasApp {
        TelinhasApp::build(nickname.to_owned(), Sources::demo())
    }

    #[test]
    fn first_launch_asks_for_a_nickname() {
        let mut app = app("");
        assert_eq!(app.screen, Screen::FirstRun);
        let ctx = frames(&mut app, 3);
        // The form is as tall as its content and sits in the middle of the
        // window, not stretched to fill it.
        let area = ctx
            .memory(|m| m.area_rect(theme::id("first_run")))
            .expect("the form was drawn");
        assert!(area.height() < 460.0, "form is {} px tall", area.height());
        assert!(
            (area.center().y - 410.0).abs() < 4.0,
            "centered at {}",
            area.center().y
        );
        assert!(
            (area.center().x - 680.0).abs() < 4.0,
            "centered at {}",
            area.center().x
        );
    }

    #[test]
    fn home_and_nickname_modal_draw() {
        let mut app = app("Bianco");
        assert_eq!(app.screen, Screen::Home);
        frames(&mut app, 4);
        // The two cards are as tall as the taller one.
        let [create, join] = app.home.measured;
        assert!(create > 100.0 && join > 100.0);
        assert!(
            (create - join).abs() < 0.5,
            "create {create} px, join {join} px"
        );
        app.modal = Modal::Nickname;
        frames(&mut app, 3);
    }

    #[test]
    fn room_draws_in_every_layout_and_state() {
        let mut app = app("Bianco");
        app.enter_room("ABCD-2345", false);
        frames(&mut app, 3);
        if let Some(room) = app.room.as_mut() {
            room.layout = crate::model::Layout::Focus;
        }
        frames(&mut app, 3);
        app.sim.reconnecting = true;
        app.sim.software_encoder = true;
        app.sim.pending_join = Some("Beto".to_owned());
        if let Some(room) = app.room.as_mut() {
            room.participants[0].admin = false;
            room.participants[3].admin = true;
            room.broadcasting = true;
        }
        frames(&mut app, 3);
        // Full screen drops the frame around the picture.
        app.fullscreen = true;
        frames(&mut app, 3);
        app.fill_video = true;
        frames(&mut app, 3);
    }

    #[test]
    fn a_new_room_starts_empty_and_can_open_the_broadcast_setup() {
        let mut app = app("Bianco");
        app.home.mode = EntryMode::Broadcasting;
        app.enter_room("WXYZ-9876", true);
        assert!(matches!(app.modal, Modal::Broadcast(_)));
        frames(&mut app, 3);
        // Both tabs of the setup.
        if let Modal::Broadcast(draft) = &mut app.modal {
            draft.tab = crate::model::SourceTab::Windows;
            draft.clamp(&app.sources.clone());
        }
        frames(&mut app, 3);
    }

    #[test]
    fn starting_a_broadcast_adds_my_stream_and_switching_keeps_it() {
        let mut app = app("Bianco");
        app.enter_room("ABCD-2345", false);
        let before = app.room.as_ref().unwrap().streams.len();
        let draft = BroadcastDraft::new();
        app.start_broadcast(&draft);
        let room = app.room.as_ref().unwrap();
        assert_eq!(room.streams.len(), before + 1);
        assert!(room.broadcasting);
        // Changing the source while live keeps one stream.
        let mut other = BroadcastDraft::new();
        other.height = 720;
        app.start_broadcast(&other);
        let room = app.room.as_ref().unwrap();
        assert_eq!(room.streams.len(), before + 1);
        assert_eq!(room.streams.last().unwrap().height, 720);
        frames(&mut app, 2);
    }

    #[test]
    fn leaving_goes_back_home() {
        let mut app = app("Bianco");
        app.enter_room("ABCD-2345", false);
        app.leave_room();
        assert_eq!(app.screen, Screen::Home);
        assert!(app.room.is_none());
        assert!(app.mock.is_empty());
    }

    // ----- the signaling client --------------------------------------------------------

    /// A real server in this process, on its own runtime.
    fn server() -> (tokio::runtime::Runtime, String) {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let url = runtime.block_on(async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            tokio::spawn(telinhas_signaling::serve(
                listener,
                telinhas_signaling::Hub::new(telinhas_signaling::Config::default()),
            ));
            format!("ws://{addr}/ws")
        });
        (runtime, url)
    }

    /// Polls the app until the condition holds, or fails after a few seconds.
    fn wait(app: &mut TelinhasApp, what: &str, done: impl Fn(&TelinhasApp) -> bool) {
        let deadline = Instant::now() + std::time::Duration::from_secs(5);
        loop {
            app.poll_net();
            if done(app) {
                return;
            }
            assert!(Instant::now() < deadline, "timed out waiting for: {what}");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    #[test]
    fn without_a_server_the_app_stays_in_demonstration() {
        let mut app = app("Ana");
        assert!(app.is_demo());
        app.screen = Screen::Home;
        app.enter_room("ABCD-EFGH", true);
        assert_eq!(app.screen, Screen::Room);
    }

    #[test]
    fn a_wrong_code_shows_a_notice_on_the_home_screen() {
        let (_runtime, url) = server();
        let mut app = app("Ana");
        app.screen = Screen::Home;
        app.set_server(&url);
        app.enter_room("ABCD-EFGH", false);
        assert_eq!(app.link, Link::Connecting);
        wait(&mut app, "a notice", |a| a.notice.is_some());
        assert_eq!(app.screen, Screen::Home);
        assert_eq!(app.link, Link::Idle);
        frames(&mut app, 2);
    }

    #[test]
    fn two_apps_meet_in_a_room_and_see_each_other() {
        let (_runtime, url) = server();
        let mut ana = app("Ana");
        ana.screen = Screen::Home;
        ana.set_server(&url);
        ana.enter_room("", true);
        wait(&mut ana, "the room", |a| a.screen == Screen::Room);
        let code = ana.room.as_ref().unwrap().code.clone();
        assert!(ana.room.as_ref().unwrap().i_am_admin());

        let mut beto = app("Beto");
        beto.screen = Screen::Home;
        beto.set_server(&url);
        beto.enter_room(&code, false);
        wait(&mut beto, "the room", |a| a.screen == Screen::Room);
        assert_eq!(beto.room.as_ref().unwrap().participants.len(), 2);
        assert!(!beto.room.as_ref().unwrap().i_am_admin());

        wait(&mut ana, "Beto", |a| {
            a.room.as_ref().is_some_and(|r| r.participants.len() == 2)
        });
        frames(&mut ana, 3);
        frames(&mut beto, 3);

        beto.leave_room();
        assert!(beto.room.is_none());
        wait(&mut ana, "Beto leaving", |a| {
            a.room.as_ref().is_some_and(|r| r.participants.len() == 1)
        });
    }

    #[test]
    fn the_admin_answers_a_join_request() {
        let (_runtime, url) = server();
        let mut ana = app("Ana");
        ana.screen = Screen::Home;
        ana.home.require_approval = true;
        ana.set_server(&url);
        ana.enter_room("", true);
        wait(&mut ana, "the room", |a| a.screen == Screen::Room);
        let code = ana.room.as_ref().unwrap().code.clone();

        let mut beto = app("Beto");
        beto.screen = Screen::Home;
        beto.set_server(&url);
        beto.enter_room(&code, false);
        wait(&mut beto, "waiting", |a| a.link == Link::WaitingApproval);
        wait(&mut ana, "the request", |a| !a.requests.is_empty());
        frames(&mut ana, 2);

        let id = ana.requests[0].request_id.clone();
        ana.requests.remove(0);
        ana.send(Command::ApprovalDecision {
            request_id: id,
            accept: true,
        });
        wait(&mut beto, "the room", |a| a.screen == Screen::Room);
    }

    #[test]
    fn a_broadcast_reaches_the_other_app_as_real_video() {
        let (_runtime, url) = server();
        let mut ana = app("Ana");
        ana.screen = Screen::Home;
        ana.set_server(&url);
        ana.enter_room("", true);
        wait(&mut ana, "the room", |a| a.screen == Screen::Room);
        let code = ana.room.as_ref().unwrap().code.clone();
        let ana_id = ana.room.as_ref().unwrap().me_id().unwrap();

        let mut beto = app("Beto");
        beto.screen = Screen::Home;
        beto.set_server(&url);
        beto.enter_room(&code, false);
        wait(&mut beto, "the room", |a| a.screen == Screen::Room);
        wait(&mut ana, "Beto", |a| {
            a.room.as_ref().is_some_and(|r| r.participants.len() == 2)
        });

        let mut draft = BroadcastDraft::new();
        draft.clamp(&ana.sources);
        ana.start_broadcast(&draft);
        assert!(ana.room.as_ref().unwrap().broadcasting);

        // Beto learns of the stream and then receives its pictures. Ana's app
        // must keep polling too: it relays the negotiation.
        let deadline = Instant::now() + std::time::Duration::from_secs(30);
        loop {
            ana.poll_net();
            beto.poll_net();
            let got = beto
                .media
                .as_ref()
                .and_then(|m| m.latest(ana_id))
                .is_some_and(|p| p.seq >= 3);
            if got {
                break;
            }
            assert!(Instant::now() < deadline, "no video arrived at Beto");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(beto.room.as_ref().unwrap().streams.len(), 1);
        beto.diagnostics = true;
        frames(&mut beto, 3);

        ana.room.as_mut().unwrap().broadcasting = false;
        ana.media
            .as_ref()
            .unwrap()
            .send(telinhas_media::Command::StopBroadcast);
    }
}
