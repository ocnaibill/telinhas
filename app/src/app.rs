//! The application state and the main loop: which screen is up, and the
//! pieces every screen shares.

use std::time::{Duration, Instant};

use eframe::egui;

use crate::mockvideo::MockSource;
use crate::model::{
    BroadcastDraft, EntryMode, Room, Screen, Sources, preview_code, valid_nickname,
};
use crate::theme;
use crate::video::VideoGpu;

pub(crate) const NICKNAME_KEY: &str = "nickname";

pub(crate) enum Modal {
    None,
    Broadcast(Box<BroadcastDraft>),
    Nickname,
}

/// Switches that make the interface show states it cannot reach on its own
/// yet (a join request, a dropped connection, software encoding).
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
    /// Height both cards share: the taller one's, measured on the last frame.
    pub card_height: f32,
    /// What each card measured this frame, [create, join].
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
        let nickname = cc
            .storage
            .and_then(|s| s.get_string(NICKNAME_KEY))
            .filter(|n| valid_nickname(n))
            .unwrap_or_default();
        Self::build(nickname, Sources::load())
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
                card_height: 0.0,
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
        }
    }

    pub(crate) fn show_toast(&mut self, text: impl Into<String>) {
        self.toast = Some((text.into(), Instant::now()));
    }

    /// Enters a test room, with a few people already in it.
    pub(crate) fn enter_room(&mut self, code: &str, create: bool) {
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
        self.room = None;
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
    }
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
}
