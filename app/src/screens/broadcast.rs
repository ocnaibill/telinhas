//! Modals and pop-ups: broadcast setup (DEC-36), nickname change, join
//! requests (RF-19) and short notices.

use std::time::Duration;

use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, Layout, Pos2, Rect, RichText, Sense, Stroke, Vec2,
};

use crate::app::{Modal, TelinhasApp};
use crate::model::{
    BroadcastDraft, ConnectionState, Participant, Quality, Room, SourceTab, StreamInfo,
    target_kbps, upload_kbps,
};
use crate::theme;

const MODAL_WIDTH: f32 = 760.0;

enum Outcome {
    Keep,
    Cancel,
    Start,
}

fn nice_rate(kbps: u32) -> String {
    if kbps >= 1000 {
        format!("{:.1} Mbps", f64::from(kbps) / 1000.0).replace('.', ",")
    } else {
        format!("{kbps} kbps")
    }
}

impl TelinhasApp {
    pub(crate) fn modals_ui(&mut self, ctx: &egui::Context) {
        let mut modal = std::mem::replace(&mut self.modal, Modal::None);
        let mut keep = true;
        match &mut modal {
            Modal::None => keep = false,
            Modal::Nickname => {
                let response = egui::Modal::new(theme::id("nickname_modal"))
                    .show(ctx, |ui| self.nickname_card(ui, "Trocar apelido", "Salvar"));
                if response.inner || response.should_close() {
                    keep = false;
                }
            }
            Modal::Broadcast(draft) => {
                let viewers = self
                    .room
                    .as_ref()
                    .map_or(0, |r| r.participants.len().saturating_sub(1))
                    as u32;
                let already = self.room.as_ref().is_some_and(|r| r.broadcasting);
                let mut outcome = Outcome::Keep;
                let response = egui::Modal::new(theme::id("broadcast_modal")).show(ctx, |ui| {
                    outcome = self.broadcast_form(ui, draft, viewers, already);
                });
                if response.should_close() {
                    outcome = Outcome::Cancel;
                }
                match outcome {
                    Outcome::Keep => {}
                    Outcome::Cancel => keep = false,
                    Outcome::Start => {
                        self.start_broadcast(draft);
                        keep = false;
                    }
                }
            }
        }
        if keep {
            self.modal = modal;
        }
    }

    pub(crate) fn start_broadcast(&mut self, draft: &BroadcastDraft) {
        let sources = self.sources.clone();
        let (width, height) = draft.output_size(&sources);
        let title = match draft.tab {
            SourceTab::Screens => format!("Tela {}", draft.monitor + 1),
            SourceTab::Windows => sources
                .windows
                .get(draft.window)
                .map_or("Janela".to_owned(), |w| w.title.clone()),
        };
        let me = self.nickname.clone();
        let software = self.sim.software_encoder;
        let Some(room) = self.room.as_mut() else {
            return;
        };
        let title_for_meta = title.clone();
        let info = StreamInfo {
            owner_id: room.me_id(),
            stream_id: "screen".to_owned(),
            owner: me.clone(),
            title,
            width,
            height,
            fps: draft.fps,
            hardware: !software,
            volume: 1.0,
            visible: true,
            quality: Quality::Original,
            has_webcam: draft.webcam,
        };
        let switching = room.broadcasting;
        if let Some(index) = room.streams.iter().position(|s| s.owner == me) {
            // Changing the source while live does not drop the stream (DEC-11).
            room.streams[index] = info;
        } else {
            room.streams.push(info);
            if self.media.is_none() {
                self.mock
                    .push(crate::mockvideo::MockSource::start(1280, 720, 30, 4));
            }
        }
        room.broadcasting = true;
        if let Some(net) = &self.net {
            net.send(telinhas_net::Command::AnnounceStream {
                stream_id: "screen".to_owned(),
                kind: telinhas_protocol::StreamKind::Screen,
                live: true,
                meta: crate::model::stream_meta(
                    &title_for_meta,
                    width,
                    height,
                    draft.fps,
                    !software,
                    draft.webcam,
                ),
            });
        }
        if let Some(media) = &self.media {
            use telinhas_media::{BroadcastSource, BroadcastSpec};
            let source = match (self.sources.is_demo, draft.tab) {
                (true, _) => BroadcastSource::Synthetic {
                    width: 1280,
                    height: 720,
                },
                (false, SourceTab::Screens) => self
                    .sources
                    .monitors
                    .get(draft.monitor)
                    .map_or(BroadcastSource::Monitor(0), |m| {
                        BroadcastSource::Monitor(m.index)
                    }),
                (false, SourceTab::Windows) => match self.sources.windows.get(draft.window) {
                    Some(w) => BroadcastSource::Window(w.handle),
                    None => BroadcastSource::Monitor(0),
                },
            };
            // Sound: a window brings its own program's, a screen brings the
            // system's minus what the person left out. Without capture support
            // (the demonstration sources) a tone stands in, to hear the path.
            let audio = if !draft.system_audio {
                None
            } else if self.sources.is_demo {
                Some(telinhas_media::AudioInput::Synthetic)
            } else {
                match draft.tab {
                    SourceTab::Screens => Some(telinhas_media::AudioInput::System {
                        exclude: draft.excluded_apps.clone(),
                    }),
                    SourceTab::Windows => self
                        .sources
                        .windows
                        .get(draft.window)
                        .filter(|w| w.pid != 0)
                        .map(|w| telinhas_media::AudioInput::Process { pid: w.pid }),
                }
            };
            let viewers = room
                .participants
                .iter()
                .filter(|p| !p.is_me)
                .filter_map(|p| p.peer_id)
                .collect();
            media.send(telinhas_media::Command::StartBroadcast {
                spec: BroadcastSpec {
                    // What the person chose is the most it will ever send.
                    max_height: Some(height),
                    audio,
                    mic: draft.mic,
                    ..BroadcastSpec::new(
                        source,
                        draft.fps,
                        crate::model::target_kbps(width, height, draft.fps),
                    )
                },
                viewers,
            });
        }
        if draft.webcam && self.mock_webcam.is_none() && self.media.is_none() {
            self.mock_webcam = Some(crate::mockvideo::MockSource::start(640, 360, 24, 3));
        }
        self.show_toast(if switching {
            "Fonte trocada sem derrubar a transmissão"
        } else {
            "Transmissão iniciada"
        });
    }

    fn broadcast_form(
        &mut self,
        ui: &mut egui::Ui,
        draft: &mut BroadcastDraft,
        viewers: u32,
        already: bool,
    ) -> Outcome {
        let mut outcome = Outcome::Keep;
        let sources = self.sources.clone();
        ui.set_width(MODAL_WIDTH);
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(
                    RichText::new("Configurar transmissão")
                        .size(24.0)
                        .strong(),
                );
                ui.label(
                    RichText::new("Dá para trocar a fonte e as opções com a transmissão ativa, sem desconectar.")
                        .color(theme::TEXT_DIM),
                );
                if sources.is_demo {
                    ui.label(
                        RichText::new("Lista de exemplo: a captura de telas e janelas só existe no Windows por enquanto.")
                            .small()
                            .color(theme::AMBER),
                    );
                }
            });
            ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                if ui
                    .add(egui::Button::new(RichText::new(theme::icons::X).size(18.0)).frame(false))
                    .clicked()
                {
                    outcome = Outcome::Cancel;
                }
            });
        });
        ui.add_space(8.0);

        egui::ScrollArea::vertical()
            .max_height((ui.ctx().content_rect().height() - 300.0).max(260.0))
            .show(ui, |ui| {
                ui.set_width(MODAL_WIDTH - 24.0);
                // ---- source
                theme::section_title(ui, "Origem da captura");
                let mut tab = draft.tab;
                let tabs = [
                    (
                        SourceTab::Screens,
                        format!("Telas inteiras ({})", sources.monitors.len()),
                    ),
                    (
                        SourceTab::Windows,
                        format!("Janelas ({})", sources.windows.len()),
                    ),
                ];
                let labels: Vec<(SourceTab, &str)> =
                    tabs.iter().map(|(t, l)| (*t, l.as_str())).collect();
                if theme::segmented(ui, &mut tab, &labels) {
                    draft.tab = tab;
                    draft.clamp(&sources);
                }
                match draft.tab {
                    SourceTab::Screens => {
                        ui.horizontal_wrapped(|ui| {
                            for (index, monitor) in sources.monitors.iter().enumerate() {
                                let selected = draft.monitor == index;
                                let title = format!(
                                    "Tela {}{}",
                                    index + 1,
                                    if monitor.primary { " (principal)" } else { "" }
                                );
                                let detail = format!(
                                    "{} × {} · {} Hz",
                                    monitor.width, monitor.height, monitor.refresh_hz
                                );
                                if source_card(
                                    ui,
                                    theme::icons::MONITOR,
                                    &title,
                                    &detail,
                                    selected,
                                    340.0,
                                    false,
                                ) {
                                    draft.monitor = index;
                                    draft.clamp(&sources);
                                }
                            }
                        });
                    }
                    SourceTab::Windows => {
                        ui.horizontal_wrapped(|ui| {
                            for (index, window) in sources.windows.iter().enumerate() {
                                let selected = draft.window == index;
                                let detail = format!(
                                    "{} · {} × {}{}",
                                    window.process,
                                    window.width,
                                    window.height,
                                    if window.minimized {
                                        " · minimizada"
                                    } else {
                                        ""
                                    }
                                );
                                if source_card(
                                    ui,
                                    theme::icons::APP_WINDOW,
                                    &window.title,
                                    &detail,
                                    selected,
                                    340.0,
                                    window.minimized,
                                ) && !window.minimized
                                {
                                    draft.window = index;
                                    draft.clamp(&sources);
                                }
                            }
                        });
                    }
                }

                // ---- quality
                ui.add_space(6.0);
                theme::section_title(ui, "Qualidade da transmissão");
                let (out_w, out_h) = draft.output_size(&sources);
                let heights = draft.height_options(&sources);
                let height_labels: Vec<(u32, String)> =
                    heights.iter().map(|h| (*h, format!("{h}p"))).collect();
                let labels: Vec<(u32, &str)> = height_labels
                    .iter()
                    .map(|(h, l)| (*h, l.as_str()))
                    .collect();
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Resolução").color(theme::TEXT_DIM));
                    theme::segmented(ui, &mut draft.height, &labels);
                    ui.label(
                        RichText::new(format!("{out_w}×{out_h}"))
                            .small()
                            .monospace()
                            .color(theme::TEXT_FAINT),
                    );
                });
                let rates = draft.fps_options(&sources);
                let rate_labels: Vec<(u32, String)> =
                    rates.iter().map(|f| (*f, format!("{f} fps"))).collect();
                let labels: Vec<(u32, &str)> =
                    rate_labels.iter().map(|(f, l)| (*f, l.as_str())).collect();
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Quadros por segundo").color(theme::TEXT_DIM));
                    theme::segmented(ui, &mut draft.fps, &labels);
                    if draft.source_refresh(&sources) > 60 {
                        ui.label(
                            RichText::new("limitado a 60")
                                .small()
                                .color(theme::TEXT_FAINT),
                        );
                    }
                });
                ui.label(
                    RichText::new("As opções vêm da tela ou janela escolhida e do seu hardware.")
                        .small()
                        .color(theme::TEXT_FAINT),
                );

                // ---- mouse and audio
                ui.add_space(6.0);
                theme::section_title(ui, "Mouse e áudio");
                toggle_row(
                    ui,
                    theme::icons::MOUSE_SIMPLE,
                    "Capturar o mouse",
                    match draft.tab {
                        SourceTab::Screens => "Ligado por padrão ao transmitir uma tela inteira",
                        SourceTab::Windows => "Desligado por padrão em janelas, exceto navegadores",
                    },
                    &mut draft.mouse,
                );
                toggle_row(
                    ui,
                    theme::icons::SPEAKER_HIGH,
                    "Áudio do sistema",
                    match draft.tab {
                        SourceTab::Screens => {
                            "Todo o áudio da tela, menos o dos aplicativos excluídos"
                        }
                        SourceTab::Windows => "Só o áudio da janela escolhida",
                    },
                    &mut draft.system_audio,
                );
                if draft.tab == SourceTab::Screens && draft.system_audio {
                    excluded_apps(ui, draft, &sources.audio_apps);
                }
                toggle_row(
                    ui,
                    theme::icons::MICROPHONE,
                    "Microfone",
                    "Desligado por padrão",
                    &mut draft.mic,
                );
                toggle_row(
                    ui,
                    theme::icons::VIDEO_CAMERA,
                    "Webcam em picture-in-picture",
                    "A webcam vai como um vídeo separado, que cada pessoa posiciona",
                    &mut draft.webcam,
                );
            });

        // ---- estimate (RF-06)
        ui.add_space(6.0);
        let (w, h) = draft.output_size(&sources);
        let per_viewer = target_kbps(w, h, draft.fps);
        let total = upload_kbps(per_viewer, viewers);
        egui::Frame::new()
            .fill(Color32::from_rgb(12, 13, 19))
            .corner_radius(CornerRadius::same(12))
            .inner_margin(12.0)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(RichText::new(theme::icons::ARROWS_CLOCKWISE).color(theme::TEXT_DIM));
                    ui.label(format!(
                        "Cada espectador na qualidade original usa até {} de upload.",
                        nice_rate(upload_kbps(per_viewer, 1))
                    ));
                });
                if viewers > 0 {
                    ui.label(
                        RichText::new(format!(
                            "Com {viewers} {} na sala: até {}.",
                            if viewers == 1 { "espectador" } else { "espectadores" },
                            nice_rate(total)
                        ))
                        .color(theme::TEXT_DIM),
                    );
                }
                if viewers + 1 > 12 {
                    ui.label(
                        RichText::new(format!(
                            "{}  Passa de 12 pessoas: a banda e o desempenho podem não bastar.",
                            theme::icons::WARNING
                        ))
                        .color(theme::AMBER),
                    );
                }
                if self.sim.software_encoder {
                    ui.label(
                        RichText::new(format!(
                            "{}  Sem codificação por hardware: usaremos software e a transmissão pode ficar quebrada.",
                            theme::icons::WARNING
                        ))
                        .color(theme::AMBER),
                    );
                }
            });

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let label = if already {
                    format!("{}  Trocar fonte e aplicar", theme::icons::BROADCAST)
                } else {
                    format!("{}  Iniciar transmissão", theme::icons::BROADCAST)
                };
                if theme::wide_primary_button(ui, label, 220.0).clicked() {
                    outcome = Outcome::Start;
                }
                if theme::secondary_button(ui, "Cancelar").clicked() {
                    outcome = Outcome::Cancel;
                }
            })
        });
        outcome
    }

    // ----- join request (RF-19) -----------------------------------------------------------

    pub(crate) fn join_request_popup(&mut self, ctx: &egui::Context, room: &mut Room) {
        let real = self.requests.first().cloned();
        let Some(name) = real
            .as_ref()
            .map(|r| r.nickname.clone())
            .or_else(|| self.sim.pending_join.clone())
        else {
            return;
        };
        if !room.i_am_admin() {
            return;
        }
        let mut decision = None;
        egui::Area::new(theme::id("join_request"))
            .anchor(Align2::RIGHT_TOP, [-340.0, 70.0])
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(theme::PANEL)
                    .stroke(Stroke::new(1.0, theme::STROKE))
                    .corner_radius(CornerRadius::same(16))
                    .inner_margin(14.0)
                    .show(ui, |ui| {
                        ui.set_width(290.0);
                        ui.horizontal(|ui| {
                            let (rect, _) =
                                ui.allocate_exact_size(Vec2::splat(36.0), Sense::hover());
                            theme::avatar(
                                ui.painter(),
                                rect.center(),
                                17.0,
                                &name,
                                theme::color_for(&name),
                            );
                            ui.vertical(|ui| {
                                ui.label(RichText::new(format!("{name} quer entrar")).strong());
                                ui.label(
                                    RichText::new("Só o administrador decide")
                                        .small()
                                        .color(theme::TEXT_DIM),
                                );
                            });
                        });
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            if theme::success_button(ui, "Aceitar", 110.0).clicked() {
                                decision = Some(true);
                            }
                            if theme::secondary_button(ui, "Recusar").clicked() {
                                decision = Some(false);
                            }
                        });
                    });
            });
        if let (Some(accept), Some(request)) = (decision, real) {
            self.requests.remove(0);
            self.send(telinhas_net::Command::ApprovalDecision {
                request_id: request.request_id,
                accept,
            });
            if !accept {
                self.show_toast(format!("Pedido de {name} recusado"));
            }
            return;
        }
        match decision {
            Some(true) => {
                room.participants.push(Participant {
                    peer_id: None,
                    name: name.clone(),
                    admin: false,
                    latency_ms: 28,
                    state: ConnectionState::Connected,
                    is_me: false,
                });
                self.sim.pending_join = None;
                self.show_toast(format!("{name} entrou na sala"));
            }
            Some(false) => {
                self.sim.pending_join = None;
                self.show_toast(format!("Pedido de {name} recusado"));
            }
            None => {}
        }
    }

    // ----- notices ------------------------------------------------------------------------------

    pub(crate) fn toast_ui(&mut self, ctx: &egui::Context) {
        let Some((text, at)) = self.toast.clone() else {
            return;
        };
        if at.elapsed() > Duration::from_millis(2600) {
            self.toast = None;
            return;
        }
        ctx.request_repaint_after(Duration::from_millis(100));
        egui::Area::new(theme::id("toast"))
            .anchor(Align2::CENTER_BOTTOM, [-155.0, -92.0])
            .order(egui::Order::Tooltip)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(Color32::from_rgba_unmultiplied(40, 42, 56, 245))
                    .corner_radius(CornerRadius::same(20))
                    .inner_margin(egui::Margin::symmetric(16, 9))
                    .show(ui, |ui| {
                        ui.label(RichText::new(text).color(theme::TEXT));
                    });
            });
    }
}

/// A source choice: a preview area, a title and a detail line. Returns true
/// when clicked. `disabled` greys it out (a minimized window cannot be captured).
fn source_card(
    ui: &mut egui::Ui,
    icon: &str,
    title: &str,
    detail: &str,
    selected: bool,
    width: f32,
    disabled: bool,
) -> bool {
    let size = Vec2::new(width, 122.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let painter = ui.painter();
    let fill = if selected {
        theme::INDIGO_DIM
    } else {
        theme::CARD
    };
    painter.rect_filled(rect, CornerRadius::same(14), fill);
    let preview = Rect::from_min_size(rect.min + Vec2::splat(8.0), Vec2::new(width - 16.0, 66.0));
    painter.rect_filled(
        preview,
        CornerRadius::same(10),
        Color32::from_rgb(12, 13, 19),
    );
    painter.text(
        preview.center(),
        Align2::CENTER_CENTER,
        icon,
        egui::FontId::proportional(30.0),
        if disabled {
            theme::TEXT_FAINT
        } else {
            theme::TEXT_DIM
        },
    );
    painter.text(
        Pos2::new(rect.min.x + 12.0, rect.max.y - 30.0),
        Align2::LEFT_CENTER,
        truncate(title, 38),
        egui::FontId::proportional(14.0),
        if disabled {
            theme::TEXT_FAINT
        } else {
            theme::TEXT
        },
    );
    painter.text(
        Pos2::new(rect.min.x + 12.0, rect.max.y - 13.0),
        Align2::LEFT_CENTER,
        truncate(detail, 46),
        egui::FontId::proportional(11.5),
        theme::TEXT_FAINT,
    );
    if selected {
        painter.circle_filled(
            rect.right_top() + Vec2::new(-20.0, 20.0),
            11.0,
            theme::INDIGO,
        );
        painter.text(
            rect.right_top() + Vec2::new(-20.0, 20.0),
            Align2::CENTER_CENTER,
            theme::icons::CHECK,
            egui::FontId::proportional(13.0),
            Color32::WHITE,
        );
        painter.rect_stroke(
            rect,
            CornerRadius::same(14),
            Stroke::new(2.0, theme::INDIGO),
            egui::StrokeKind::Inside,
        );
    }
    response.clicked()
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_owned()
    } else {
        let head: String = text.chars().take(max.saturating_sub(1)).collect();
        format!("{head}…")
    }
}

fn toggle_row(ui: &mut egui::Ui, icon: &str, title: &str, hint: &str, on: &mut bool) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(icon).size(20.0).color(theme::TEXT_DIM));
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            ui.label(RichText::new(title).strong());
            ui.label(RichText::new(hint).small().color(theme::TEXT_DIM));
        });
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            theme::toggle(ui, on);
        });
    });
}

/// The programs left out of the audio when sharing a whole screen (DEC-16).
fn excluded_apps(ui: &mut egui::Ui, draft: &mut BroadcastDraft, apps: &[String]) {
    egui::Frame::new()
        .fill(Color32::from_rgb(12, 13, 19))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(10.0)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(
                RichText::new("Excluir aplicativos do áudio, por exemplo o Discord, para não repetir a voz da chamada")
                    .small()
                    .color(theme::TEXT_DIM),
            );
            ui.horizontal_wrapped(|ui| {
                let mut remove = None;
                for (index, app) in draft.excluded_apps.iter().enumerate() {
                    if theme::pill(
                        ui,
                        format!("{app}  {}", theme::icons::X),
                        theme::TEXT,
                        theme::CARD_HI,
                    )
                    .interact(Sense::click())
                    .clicked()
                    {
                        remove = Some(index);
                    }
                }
                if let Some(index) = remove {
                    draft.excluded_apps.remove(index);
                }
                egui::ComboBox::from_id_salt("exclude_app")
                    .selected_text(format!("{}  Adicionar aplicativo", theme::icons::PLUS))
                    .show_ui(ui, |ui| {
                        for app in apps {
                            if draft.excluded_apps.contains(app) {
                                continue;
                            }
                            if ui.selectable_label(false, app).clicked() {
                                draft.excluded_apps.push(app.clone());
                            }
                        }
                    });
            });
        });
}
