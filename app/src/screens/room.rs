//! The room: top bar, the grid or focus stage with the videos, the side
//! panel of streams and people, and the floating controls (DEC-20, DEC-33).

use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, FontId, Layout, Pos2, Rect, RichText, Sense,
    Stroke, StrokeKind, Vec2,
};

use crate::app::TelinhasApp;
use crate::model::{
    ConnectionState, Layout as RoomLayout, Quality, Room, StreamInfo, target_kbps, upload_kbps,
};
use crate::theme;
use crate::video::{self, VideoCallback};

/// What the room asked for while it was drawn; applied once drawing is done.
#[derive(Default)]
struct Actions {
    leave: bool,
    open_broadcast: bool,
    stop_broadcast: bool,
    copy_code: bool,
    fullscreen: bool,
    fill: bool,
}

/// Capacity the room is sized for (DEC-03, DEC-24): above it, only a warning.
const COMFORTABLE_PEOPLE: usize = 12;

fn nice_rate(kbps: u32) -> String {
    if kbps >= 1000 {
        format!("{:.1} Mbps", f64::from(kbps) / 1000.0).replace('.', ",")
    } else {
        format!("{kbps} kbps")
    }
}

impl TelinhasApp {
    pub(crate) fn room_ui(&mut self, ui: &mut egui::Ui) {
        let Some(mut room) = self.room.take() else {
            self.screen = crate::model::Screen::Home;
            return;
        };
        let mut actions = Actions::default();
        let time = self.started.elapsed().as_secs_f32();

        let ctx = ui.ctx().clone();
        let toggle = ctx.input(|i| {
            i.key_pressed(egui::Key::F11) || (i.modifiers.command && i.key_pressed(egui::Key::F))
        }) || (self.fullscreen && ctx.input(|i| i.key_pressed(egui::Key::Escape)));
        // F3 and F11 are media keys on a Mac, so there are letter shortcuts too.
        if ctx.input(|i| {
            i.key_pressed(egui::Key::F3) || (i.modifiers.command && i.key_pressed(egui::Key::D))
        }) {
            self.diagnostics = !self.diagnostics;
        }
        if toggle {
            self.fullscreen = !self.fullscreen;
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.fullscreen));
        }
        if !self.fullscreen {
            self.top_bar(ui, &mut room, &mut actions, time);
            self.banners(ui, &room);
            self.side_panel(ui, &mut room, time);
        }
        self.stage(ui, &mut room, &mut actions);
        self.floating_bar(&ui.ctx().clone(), &mut room, &mut actions);
        if self.diagnostics {
            self.diagnostics_ui(&ui.ctx().clone(), &room);
        }
        self.join_request_popup(&ui.ctx().clone(), &mut room);

        if actions.fill
            || (!ctx.egui_wants_keyboard_input() && ctx.input(|i| i.key_pressed(egui::Key::Z)))
        {
            self.fill_video = !self.fill_video;
        }
        if actions.fullscreen {
            self.fullscreen = !self.fullscreen;
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.fullscreen));
        }
        if actions.copy_code {
            ui.ctx().copy_text(room.code.clone());
            self.show_toast("Código copiado");
        }
        if actions.open_broadcast {
            self.open_broadcast_modal();
        }
        if actions.stop_broadcast {
            room.broadcasting = false;
            if let Some(media) = &self.media {
                media.send(telinhas_media::Command::StopBroadcast);
            }
            if let Some(index) = room.streams.iter().position(|s| s.owner == self.nickname) {
                room.streams.remove(index);
                if index < self.mock.len() {
                    self.mock.remove(index);
                }
                room.focus = 0;
            }
            self.send(telinhas_net::Command::AnnounceStream {
                stream_id: "screen".to_owned(),
                kind: telinhas_protocol::StreamKind::Screen,
                live: false,
                meta: telinhas_protocol::Value::Null,
            });
            self.show_toast("Transmissão encerrada");
        }
        self.room = Some(room);
        if actions.leave {
            self.leave_room();
        }
    }

    // ----- top bar ------------------------------------------------------------------

    fn top_bar(&mut self, ui: &mut egui::Ui, room: &mut Room, actions: &mut Actions, time: f32) {
        egui::Panel::top("room_top")
            .frame(
                egui::Frame::new()
                    .fill(theme::PANEL)
                    .inner_margin(egui::Margin::symmetric(16, 10)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    let code = format!("{}  {}", theme::icons::COPY, room.code);
                    if theme::secondary_button(ui, code)
                        .on_hover_text("Copiar o código da sala")
                        .clicked()
                    {
                        actions.copy_code = true;
                    }
                    if room.i_am_admin() {
                        theme::pill(
                            ui,
                            format!("{}  Administrador", theme::icons::CROWN),
                            theme::AMBER,
                            theme::AMBER_DIM,
                        )
                        .on_hover_text("Você aprova as entradas e passa o papel se sair");
                        if !self.is_demo() {
                            let mut on = room.require_approval;
                            if theme::toggle(ui, &mut on).changed() {
                                self.send(telinhas_net::Command::SetApproval(on));
                            }
                            ui.label(RichText::new("Aprovar entradas").small().color(theme::TEXT_DIM));
                        }
                    }
                    // Demonstration data, or the delay measured on the video path.
                    let delay = if self.is_demo() {
                        Some(room.average_latency() as f32 + (time * 1.7).sin() * 2.0)
                    } else {
                        self.video_delay()
                    };
                    let text = match delay {
                        Some(ms) => format!("{}  {:.0} ms", theme::icons::LIGHTNING, ms.max(0.0)),
                        None => format!("{}  — ms", theme::icons::LIGHTNING),
                    };
                    theme::pill(ui, text, theme::GREEN, theme::GREEN_DIM)
                    .on_hover_text("Atraso do vídeo, da captura até aparecer na tela do outro (F3 ou Cmd/Ctrl+D mostram os detalhes)");
                    theme::pill(
                        ui,
                        format!(
                            "{} ao vivo · {} conectados",
                            room.live_count(),
                            room.participants.len()
                        ),
                        theme::TEXT,
                        theme::CARD,
                    );
                    if room.broadcasting {
                        let viewers = room.participants.len().saturating_sub(1) as u32;
                        let own = room.streams.iter().find(|s| s.owner == self.nickname);
                        if let Some(stream) = own {
                            let per = target_kbps(stream.width, stream.height, stream.fps);
                            let total = upload_kbps(per, viewers);
                            theme::pill(
                                ui,
                                format!("{}  Upload até {}", theme::icons::ARROWS_CLOCKWISE, nice_rate(total)),
                                theme::TEXT,
                                theme::CARD,
                            )
                            .on_hover_text(format!(
                                "{} por espectador, com 25% de folga, para {viewers} espectadores",
                                nice_rate(per)
                            ));
                        }
                    }
                    if room.participants.len() > COMFORTABLE_PEOPLE {
                        theme::pill(
                            ui,
                            format!(
                                "{}  Acima de {COMFORTABLE_PEOPLE} pessoas: a banda e o desempenho podem não bastar",
                                theme::icons::WARNING
                            ),
                            theme::AMBER,
                            theme::AMBER_DIM,
                        );
                    }

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if self.is_demo() {
                            self.sim_menu(ui, room);
                        }
                        let mut layout = room.layout;
                        if theme::segmented(
                            ui,
                            &mut layout,
                            &[
                                (RoomLayout::Focus, "Foco"),
                                (RoomLayout::Grid, "Grade"),
                            ],
                        ) {
                            room.layout = layout;
                        }
                    });
                });
            });
    }

    /// Switches to see states the interface cannot reach on its own yet.
    fn sim_menu(&mut self, ui: &mut egui::Ui, room: &mut Room) {
        ui.menu_button("Simular", |ui| {
            if ui.button("Pedido de entrada").clicked() {
                self.sim.pending_join = Some("Beto".to_owned());
                ui.close();
            }
            ui.checkbox(&mut self.sim.reconnecting, "Reconectando");
            ui.checkbox(&mut self.sim.software_encoder, "Encode por software");
            ui.checkbox(&mut self.sim.not_admin, "Não sou o administrador");
            if ui.button("Entram mais 3 pessoas").clicked() {
                for name in ["Rafa", "Duda", "Gui"] {
                    room.participants.push(crate::model::Participant {
                        peer_id: None,
                        name: name.to_owned(),
                        admin: false,
                        latency_ms: 25,
                        state: ConnectionState::Connected,
                        is_me: false,
                    });
                }
                ui.close();
            }
        });
        if self.sim.not_admin {
            for p in &mut room.participants {
                if p.is_me {
                    p.admin = false;
                }
            }
        }
    }

    /// The average delay of the video on the paths this app takes part in.
    fn video_delay(&self) -> Option<f32> {
        let stats = self.media.as_ref()?.stats();
        let values: Vec<f32> = stats
            .viewers
            .values()
            .map(|p| p.total_ms)
            .chain(stats.receiving.values().map(|r| r.path.total_ms))
            .filter(|ms| *ms > 0.0)
            .collect();
        (!values.is_empty()).then(|| values.iter().sum::<f32>() / values.len() as f32)
    }

    /// Numbers over the stage: where the time goes, and how the video flows.
    fn diagnostics_ui(&self, ctx: &egui::Context, room: &Room) {
        let Some(media) = &self.media else {
            return;
        };
        let stats = media.stats();
        let name = |peer| room.name_of(peer).unwrap_or("?").to_owned();
        let path_line = |p: &telinhas_media::Path| {
            format!(
                "atraso {:.0} ms = codificação {:.0} + rede {:.0} + decodificação do espectador {:.0}   (banda estimada {:.1} Mbps)",
                p.total_ms,
                p.encode_ms,
                p.network_ms,
                p.viewer_ms,
                p.estimate_kbps / 1000.0
            )
        };
        let mut lines: Vec<String> = Vec::new();
        if let Some(s) = &stats.sending {
            lines.push(format!(
                "ENVIANDO  {} ({})  {}x{}  captura {:.0} fps  envio {:.0} fps  {:.1} Mbps  cópia {:.1} ms  codifica em {:.1} ms",
                s.encoder,
                if s.hardware { "hardware" } else { "software" },
                s.width,
                s.height,
                s.capture_fps,
                s.fps,
                s.kbps / 1000.0,
                s.copy_ms,
                s.encode_ms
            ));
            for (peer, path) in &stats.viewers {
                lines.push(format!("  para {}: {}", name(*peer), path_line(path)));
            }
        }
        for (peer, r) in &stats.receiving {
            lines.push(format!(
                "RECEBENDO de {}  {}x{}  {:.0} fps  {:.1} Mbps  decodifica em {:.1} ms  fila {}",
                name(*peer),
                r.width,
                r.height,
                r.fps,
                r.kbps / 1000.0,
                r.decode_ms,
                r.backlog
            ));
            if r.audio.packets > 0 {
                lines.push(format!(
                    "  som: {} pacotes, {} perdidos, {:.0} ms no buffer",
                    r.audio.packets, r.audio.lost, r.audio.buffered_ms
                ));
            }
            if r.path.total_ms > 0.0 {
                lines.push(format!("  {}", path_line(&r.path)));
            }
        }
        if lines.is_empty() {
            lines.push("Sem vídeo ainda.".to_owned());
        }
        egui::Area::new(theme::id("diagnostics"))
            .anchor(
                Align2::LEFT_TOP,
                [20.0, if self.fullscreen { 20.0 } else { 70.0 }],
            )
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(Color32::from_black_alpha(200))
                    .corner_radius(CornerRadius::same(10))
                    .inner_margin(10.0)
                    .show(ui, |ui| {
                        for line in lines {
                            ui.label(
                                RichText::new(line)
                                    .monospace()
                                    .small()
                                    .color(Color32::WHITE),
                            );
                        }
                    });
            });
    }

    fn banners(&mut self, ui: &mut egui::Ui, room: &Room) {
        let software = room.broadcasting && self.sim.software_encoder;
        let reconnecting = self.sim.reconnecting || self.reconnecting;
        if !reconnecting && !software {
            return;
        }
        egui::Panel::top("banners")
            .frame(
                egui::Frame::new()
                    .fill(theme::AMBER_DIM)
                    .inner_margin(egui::Margin::symmetric(16, 8)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if reconnecting {
                        ui.spinner();
                        ui.label(
                            RichText::new("Reconectando à sala. Você volta com o mesmo código, sem perder o lugar.")
                                .color(theme::AMBER),
                        );
                    }
                    if software {
                        ui.label(
                            RichText::new(format!(
                                "{}  Esta transmissão usa codificação por software e pode ficar quebrada.",
                                theme::icons::WARNING
                            ))
                            .color(theme::AMBER),
                        );
                    }
                });
            });
    }

    // ----- side panel -----------------------------------------------------------------

    fn side_panel(&mut self, ui: &mut egui::Ui, room: &mut Room, time: f32) {
        egui::Panel::right("room_side")
            .default_size(310.0)
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(theme::PANEL)
                    .inner_margin(egui::Margin::same(14)),
            )
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    theme::section_title(ui, &format!("Transmissões ({})", room.streams.len()));
                    ui.add_space(2.0);
                    let me = self.nickname.clone();
                    let mut focus_request = None;
                    for (index, stream) in room.streams.iter_mut().enumerate() {
                        if stream_card(ui, stream, stream.owner == me, index == room.focus) {
                            focus_request = Some(index);
                        }
                    }
                    if let Some(index) = focus_request {
                        room.focus = index;
                        room.layout = RoomLayout::Focus;
                    }
                    if room.streams.is_empty() {
                        ui.label(
                            RichText::new("Ninguém está transmitindo agora.")
                                .color(theme::TEXT_DIM),
                        );
                    }
                    ui.add_space(8.0);
                    theme::section_title(
                        ui,
                        &format!("Participantes ({})", room.participants.len()),
                    );
                    for p in &room.participants {
                        participant_row(ui, p, self.sim.reconnecting && p.is_me, time);
                    }
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Codec").color(theme::TEXT_FAINT));
                        ui.label(RichText::new("H.264").monospace());
                    });
                });
            });
    }

    // ----- stage ------------------------------------------------------------------------

    fn stage(&mut self, ui: &mut egui::Ui, room: &mut Room, actions: &mut Actions) {
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(theme::BG)
                    .inner_margin(egui::Margin::same(if self.fullscreen { 0 } else { 14 })),
            )
            .show(ui, |ui| {
                let visible: Vec<usize> = room
                    .streams
                    .iter()
                    .enumerate()
                    .filter(|(_, s)| s.visible)
                    .map(|(i, _)| i)
                    .collect();
                let area = ui.available_rect_before_wrap();
                if visible.is_empty() {
                    self.empty_stage(ui, room, area, actions);
                    return;
                }
                let gap = 12.0;
                let mut rects: Vec<(usize, Rect)> = Vec::new();
                match room.layout {
                    RoomLayout::Grid => {
                        let n = visible.len();
                        let cols = (n as f32).sqrt().ceil() as usize;
                        let rows = n.div_ceil(cols);
                        let cell_w = (area.width() - gap * (cols as f32 - 1.0)) / cols as f32;
                        let cell_h = (area.height() - gap * (rows as f32 - 1.0)) / rows as f32;
                        for (slot, &index) in visible.iter().enumerate() {
                            let (col, row) = (slot % cols, slot / cols);
                            rects.push((
                                index,
                                Rect::from_min_size(
                                    area.min
                                        + Vec2::new(
                                            col as f32 * (cell_w + gap),
                                            row as f32 * (cell_h + gap),
                                        ),
                                    Vec2::new(cell_w, cell_h),
                                ),
                            ));
                        }
                    }
                    RoomLayout::Focus => {
                        let focus = if visible.contains(&room.focus) {
                            room.focus
                        } else {
                            visible[0]
                        };
                        let others: Vec<usize> =
                            visible.iter().copied().filter(|i| *i != focus).collect();
                        let strip = if others.is_empty() { 0.0 } else { 250.0 };
                        let main_w = area.width() - strip - if strip > 0.0 { gap } else { 0.0 };
                        rects.push((
                            focus,
                            Rect::from_min_size(area.min, Vec2::new(main_w, area.height())),
                        ));
                        let thumb_h = ((area.height()
                            - gap * (others.len() as f32 - 1.0).max(0.0))
                            / others.len().max(1) as f32)
                            .min(150.0);
                        for (slot, &index) in others.iter().enumerate() {
                            rects.push((
                                index,
                                Rect::from_min_size(
                                    Pos2::new(
                                        area.min.x + main_w + gap,
                                        area.min.y + slot as f32 * (thumb_h + gap),
                                    ),
                                    Vec2::new(strip, thumb_h),
                                ),
                            ));
                        }
                    }
                }
                let big = rects.first().map(|(i, _)| *i);
                for (index, rect) in rects {
                    let response = ui.allocate_rect(rect, Sense::click());
                    let large = room.layout == RoomLayout::Grid || Some(index) == big;
                    self.paint_tile(ui, rect, room, index, large);
                    if response.double_clicked() {
                        room.focus = index;
                        room.layout = if room.layout == RoomLayout::Grid {
                            RoomLayout::Focus
                        } else {
                            RoomLayout::Grid
                        };
                    } else if response.clicked() && room.layout == RoomLayout::Focus {
                        room.focus = index;
                    }
                }
            });
    }

    fn empty_stage(&mut self, ui: &mut egui::Ui, room: &Room, area: Rect, actions: &mut Actions) {
        let painter = ui.painter();
        painter.rect_filled(area, CornerRadius::same(20), theme::PANEL);
        ui.scope_builder(egui::UiBuilder::new().max_rect(area), |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(area.height() * 0.3);
                ui.label(
                    RichText::new(theme::icons::MONITOR_PLAY)
                        .size(44.0)
                        .color(theme::INDIGO),
                );
                ui.label(
                    RichText::new("Ninguém está transmitindo")
                        .size(22.0)
                        .strong(),
                );
                ui.label(
                    RichText::new(format!(
                        "Compartilhe o código {} para convidar quem vai assistir.",
                        room.code
                    ))
                    .color(theme::TEXT_DIM),
                );
                ui.add_space(8.0);
                if theme::primary_button(
                    ui,
                    format!("{}  Transmitir minha tela", theme::icons::MONITOR_PLAY),
                )
                .clicked()
                {
                    actions.open_broadcast = true;
                }
            });
        });
    }

    fn paint_tile(&self, ui: &egui::Ui, rect: Rect, room: &Room, index: usize, large: bool) {
        let stream = &room.streams[index];
        let painter = ui.painter_at(rect.expand(3.0));
        let is_mine = stream.owner == self.nickname;
        // Full screen with one picture in front: nothing but the picture, no
        // frame, border or rounded corners, so it uses every pixel.
        let immersive = self.fullscreen
            && large
            && (room.layout == RoomLayout::Focus || room.streams.len() == 1);
        let (radius, video_radius) = if immersive { (0, 0.0) } else { (16, 13.0) };
        painter.rect_filled(
            rect,
            CornerRadius::same(radius),
            if immersive {
                Color32::BLACK
            } else {
                theme::CARD
            },
        );
        let inner = if immersive { rect } else { rect.shrink(3.0) };
        let frame = match &self.media {
            // What others send comes from the network; my own is not echoed back.
            Some(media) => stream.owner_id.and_then(|id| media.latest(id)),
            None => self.mock.get(index).map(|source| source.frame()),
        };
        if let Some(frame) = frame {
            // Keep the picture's proportions: the tile may be any shape, and
            // stretching it to fit squashes the video.
            let (shown, crop) = if self.fill_video {
                (inner, cover(inner, frame.width, frame.height))
            } else {
                (fit(inner, frame.width, frame.height), [0.0, 0.0, 1.0, 1.0])
            };
            painter.add(video::shape(VideoCallback {
                id: index,
                frame,
                rect: shown,
                radius: video_radius,
                crop,
            }));
        } else if self.media.is_some() {
            let text = if is_mine {
                "Você está transmitindo"
            } else {
                "Aguardando o vídeo…"
            };
            painter.text(
                inner.center(),
                egui::Align2::CENTER_CENTER,
                text,
                egui::FontId::proportional(15.0),
                theme::TEXT_DIM,
            );
        }
        let is_focus = room.layout == RoomLayout::Focus && index == room.focus;
        let border = if is_focus || is_mine {
            theme::GREEN
        } else {
            theme::STROKE
        };
        if !immersive {
            painter.rect_stroke(
                rect,
                CornerRadius::same(16),
                Stroke::new(2.0, border),
                StrokeKind::Inside,
            );
        }

        let mut cursor = inner.min + Vec2::new(12.0, 12.0);
        let live = format!("●  AO VIVO  {}p{}", stream.height, stream.fps);
        let r = pill_at(
            &painter,
            cursor,
            &live,
            Color32::from_black_alpha(160),
            Color32::WHITE,
        );
        cursor.x = r.max.x + 6.0;
        if !stream.hardware && large {
            pill_at(
                &painter,
                cursor,
                &format!("{}  software", theme::icons::WARNING),
                theme::AMBER_DIM,
                theme::AMBER,
            );
        }
        if large {
            let plate = Rect::from_min_size(
                Pos2::new(inner.min.x + 12.0, inner.max.y - 52.0),
                Vec2::new(230.0, 40.0),
            );
            painter.rect_filled(
                plate,
                CornerRadius::same(20),
                Color32::from_black_alpha(175),
            );
            theme::avatar(
                &painter,
                plate.left_center() + Vec2::new(20.0, 0.0),
                14.0,
                &stream.owner,
                theme::color_for(&stream.owner),
            );
            painter.text(
                plate.left_center() + Vec2::new(42.0, -8.0),
                Align2::LEFT_CENTER,
                if is_mine {
                    format!("{} (você)", stream.owner)
                } else {
                    stream.owner.clone()
                },
                FontId::proportional(14.0),
                Color32::WHITE,
            );
            painter.text(
                plate.left_center() + Vec2::new(42.0, 9.0),
                Align2::LEFT_CENTER,
                &stream.title,
                FontId::proportional(11.5),
                Color32::from_gray(195),
            );
        } else {
            painter.text(
                Pos2::new(inner.min.x + 12.0, inner.max.y - 14.0),
                Align2::LEFT_CENTER,
                &stream.owner,
                FontId::proportional(12.5),
                Color32::WHITE,
            );
        }
        if let (true, Some(webcam)) = (stream.has_webcam && large, &self.mock_webcam) {
            {
                let size = Vec2::new(inner.width() * 0.24, inner.width() * 0.24 * 9.0 / 16.0);
                let cam = Rect::from_min_size(inner.max - size - Vec2::new(12.0, 12.0), size);
                painter.add(video::shape(VideoCallback {
                    id: 100 + index,
                    frame: webcam.frame(),
                    rect: cam,
                    radius: 10.0,
                    crop: [0.0, 0.0, 1.0, 1.0],
                }));
                painter.rect_stroke(
                    cam,
                    CornerRadius::same(10),
                    Stroke::new(1.5, Color32::from_white_alpha(100)),
                    StrokeKind::Outside,
                );
            }
        }
    }

    // ----- floating bar ----------------------------------------------------------------

    fn floating_bar(&mut self, ctx: &egui::Context, room: &mut Room, actions: &mut Actions) {
        egui::Area::new(theme::id("floating_bar"))
            .anchor(
                Align2::CENTER_BOTTOM,
                [if self.fullscreen { 0.0 } else { -155.0 }, -20.0],
            )
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(Color32::from_rgba_unmultiplied(18, 19, 26, 240))
                    .stroke(Stroke::new(1.0, theme::STROKE))
                    .corner_radius(CornerRadius::same(32))
                    .inner_margin(egui::Margin::symmetric(14, 10))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 10.0;
                            if room.broadcasting {
                                if theme::danger_button(
                                    ui,
                                    format!("{}  Parar", theme::icons::MONITOR),
                                )
                                .clicked()
                                {
                                    actions.stop_broadcast = true;
                                }
                            } else if theme::primary_button(
                                ui,
                                format!("{}  Transmitir", theme::icons::MONITOR_PLAY),
                            )
                            .clicked()
                            {
                                actions.open_broadcast = true;
                            }
                            let camera = if room.camera_on {
                                theme::icons::VIDEO_CAMERA
                            } else {
                                theme::icons::VIDEO_CAMERA_SLASH
                            };
                            if theme::icon_button(ui, camera, "Câmera", room.camera_on).clicked() {
                                room.camera_on = !room.camera_on;
                            }
                            let mic = if room.mic_on {
                                theme::icons::MICROPHONE
                            } else {
                                theme::icons::MICROPHONE_SLASH
                            };
                            if theme::icon_button(ui, mic, "Microfone", room.mic_on).clicked() {
                                room.mic_on = !room.mic_on;
                            }
                            if theme::icon_button(
                                ui,
                                theme::icons::GEAR,
                                "Configurar transmissão",
                                false,
                            )
                            .clicked()
                            {
                                actions.open_broadcast = true;
                            }
                            let (icon, hint) = if self.fullscreen {
                                (theme::icons::CORNERS_IN, "Sair da tela cheia (Esc)")
                            } else {
                                (theme::icons::CORNERS_OUT, "Tela cheia (F11 ou Cmd/Ctrl+F)")
                            };
                            if theme::icon_button(ui, icon, hint, self.fullscreen).clicked() {
                                actions.fullscreen = true;
                            }
                            let (fill_hint, fill_icon) = if self.fill_video {
                                ("Manter a proporção do vídeo (Z)", theme::icons::CROP)
                            } else {
                                (
                                    "Preencher a tela, cortando as bordas (Z)",
                                    theme::icons::CROP,
                                )
                            };
                            if theme::icon_button(ui, fill_icon, fill_hint, self.fill_video)
                                .clicked()
                            {
                                actions.fill = true;
                            }
                            if theme::danger_button(ui, format!("{}  Sair", theme::icons::SIGN_OUT))
                                .clicked()
                            {
                                actions.leave = true;
                            }
                        });
                    });
            });
    }
}

/// The largest rectangle with the picture's proportions that fits in `area`,
/// centered: bars on the sides or above and below instead of stretching.
fn fit(area: Rect, width: u32, height: u32) -> Rect {
    if width == 0 || height == 0 {
        return area;
    }
    let aspect = width as f32 / height as f32;
    let size = if area.width() / area.height().max(1.0) > aspect {
        Vec2::new(area.height() * aspect, area.height())
    } else {
        Vec2::new(area.width(), area.width() / aspect)
    };
    Rect::from_center_size(area.center(), size)
}

/// The part of a picture that fills `area` with no bars, centered, as
/// `[x, y, width, height]` fractions of the picture: the edges that do not fit
/// are cut.
fn cover(area: Rect, width: u32, height: u32) -> [f32; 4] {
    if width == 0 || height == 0 || area.height() < 1.0 {
        return [0.0, 0.0, 1.0, 1.0];
    }
    let picture = width as f32 / height as f32;
    let tile = area.width() / area.height();
    if tile > picture {
        // A wider tile: show a band of the picture's height.
        let h = picture / tile;
        [0.0, (1.0 - h) / 2.0, 1.0, h]
    } else {
        let w = tile / picture;
        [(1.0 - w) / 2.0, 0.0, w, 1.0]
    }
}

/// A pill drawn directly with a painter, returning its rectangle.
fn pill_at(painter: &egui::Painter, at: Pos2, text: &str, fill: Color32, color: Color32) -> Rect {
    let galley = painter.layout_no_wrap(text.to_owned(), FontId::proportional(12.0), color);
    let rect = Rect::from_min_size(at, galley.size() + Vec2::new(18.0, 9.0));
    painter.rect_filled(rect, CornerRadius::same(255), fill);
    painter.galley(rect.min + Vec2::new(9.0, 4.5), galley, color);
    rect
}

/// One stream in the side panel: who, what, volume, and the quality choice
/// (DEC-04, DEC-34). Returns true when the card was clicked to be focused.
fn stream_card(ui: &mut egui::Ui, stream: &mut StreamInfo, mine: bool, focused: bool) -> bool {
    let mut clicked = false;
    let outer = theme::card()
        .corner_radius(CornerRadius::same(14))
        .inner_margin(12.0)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(Vec2::splat(34.0), Sense::hover());
                theme::avatar(
                    ui.painter(),
                    rect.center(),
                    16.0,
                    &stream.owner,
                    theme::color_for(&stream.owner),
                );
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    ui.label(
                        RichText::new(if mine {
                            format!("{} (você)", stream.owner)
                        } else {
                            stream.owner.clone()
                        })
                        .strong(),
                    );
                    ui.label(RichText::new(&stream.title).small().color(theme::TEXT_DIM));
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let eye = if stream.visible {
                        theme::icons::EYE
                    } else {
                        theme::icons::EYE_SLASH
                    };
                    let button = ui.add(
                        egui::Button::new(RichText::new(eye).size(17.0).color(theme::TEXT_DIM))
                            .frame(false),
                    );
                    if button.on_hover_text("Mostrar ou ocultar").clicked() {
                        stream.visible = !stream.visible;
                    }
                });
            });
            if !mine {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(theme::icons::SPEAKER_HIGH).color(theme::TEXT_DIM));
                    ui.spacing_mut().slider_width = ui.available_width() - 50.0;
                    ui.add(egui::Slider::new(&mut stream.volume, 0.0..=1.0).show_value(false));
                    ui.label(
                        RichText::new(format!("{:.0}%", stream.volume * 100.0))
                            .small()
                            .color(theme::TEXT_DIM),
                    );
                });
                theme::segmented(
                    ui,
                    &mut stream.quality,
                    &[
                        (Quality::Original, "Original"),
                        (Quality::Economy, "Econômica"),
                    ],
                );
            }
            ui.horizontal(|ui| {
                let (w, h, fps) = match stream.quality {
                    Quality::Original => (stream.width, stream.height, stream.fps),
                    Quality::Economy => (1280, 720, stream.fps.min(30)),
                };
                ui.label(
                    RichText::new(format!("{w}×{h} · {fps} fps"))
                        .small()
                        .color(theme::TEXT_FAINT),
                );
                if !stream.hardware {
                    ui.label(RichText::new("software").small().color(theme::AMBER));
                }
            });
        });
    let rect = outer.response.rect;
    if focused {
        theme::outline(ui, rect, theme::INDIGO);
    }
    if ui
        .interact(
            rect,
            ui.scope_id().with(("stream", &stream.owner)),
            Sense::click(),
        )
        .double_clicked()
    {
        clicked = true;
    }
    ui.add_space(4.0);
    clicked
}

fn participant_row(
    ui: &mut egui::Ui,
    p: &crate::model::Participant,
    reconnecting: bool,
    time: f32,
) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(26.0), Sense::hover());
        theme::avatar(
            ui.painter(),
            rect.center(),
            12.0,
            &p.name,
            theme::color_for(&p.name),
        );
        let mut name = p.name.clone();
        if p.is_me {
            name.push_str(" (você)");
        }
        ui.label(RichText::new(name));
        if p.admin {
            ui.label(RichText::new(theme::icons::CROWN).color(theme::AMBER))
                .on_hover_text("Administrador da sala");
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if reconnecting || p.state == ConnectionState::Reconnecting {
                ui.label(RichText::new("reconectando…").small().color(theme::AMBER));
            } else if p.is_me {
                ui.label(RichText::new("assistindo").small().color(theme::TEXT_FAINT));
            } else if p.latency_ms == 0 && p.peer_id.is_some() {
                ui.label(RichText::new("—").small().color(theme::TEXT_FAINT));
            } else {
                let ms = p.latency_ms as f32 + (time * 1.3 + p.name.len() as f32).sin() * 2.0;
                ui.label(
                    RichText::new(format!("{:.0} ms", ms.max(1.0)))
                        .small()
                        .monospace()
                        .color(theme::GREEN),
                );
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_picture_keeps_its_proportions_inside_any_tile() {
        let wide = Rect::from_min_size(Pos2::ZERO, Vec2::new(1000.0, 400.0));
        let shown = fit(wide, 1920, 1080);
        assert!((shown.width() / shown.height() - 16.0 / 9.0).abs() < 0.01);
        assert!(shown.height() <= 400.0 + 0.01 && shown.center() == wide.center());
        let tall = Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 1000.0));
        let shown = fit(tall, 1920, 1080);
        assert!((shown.width() / shown.height() - 16.0 / 9.0).abs() < 0.01);
        assert!(shown.width() <= 400.0 + 0.01);
        // Filling a wide tile with a 16:9 picture cuts the top and bottom.
        let crop = cover(wide, 1920, 1080);
        assert!(
            (crop[2] - 1.0).abs() < 1e-6
                && crop[3] < 1.0
                && (crop[1] * 2.0 + crop[3] - 1.0).abs() < 1e-5
        );
        let crop = cover(tall, 1920, 1080);
        assert!(crop[2] < 1.0 && (crop[3] - 1.0).abs() < 1e-6);
        assert_eq!(
            cover(
                Rect::from_min_size(Pos2::ZERO, Vec2::new(1920.0, 1080.0)),
                1920,
                1080
            ),
            [0.0, 0.0, 1.0, 1.0]
        );
        // A tile of the same shape is used whole.
        let same = Rect::from_min_size(Pos2::ZERO, Vec2::new(1920.0, 1080.0));
        assert_eq!(fit(same, 1920, 1080), same);
    }
}
