//! The home screen: create a room or enter one (RF-01, RF-02, DEC-35).

use eframe::egui::{self, Align, Color32, Layout, RichText, Sense, Vec2};

use crate::app::{Modal, TelinhasApp};
use crate::logo;
use crate::model::{EntryMode, looks_like_code};
use crate::theme;

impl TelinhasApp {
    pub(crate) fn home_ui(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("home_top")
            .frame(
                egui::Frame::new()
                    .fill(theme::PANEL)
                    .inner_margin(egui::Margin::symmetric(20, 12)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.add(logo::image(26.0));
                    ui.label(RichText::new("Telinhas").size(18.0).strong());
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let chip = format!("{}  {}", theme::icons::USER, self.nickname);
                        if theme::secondary_button(ui, chip)
                            .on_hover_text("Trocar apelido")
                            .clicked()
                        {
                            self.nick_draft = self.nickname.clone();
                            self.modal = Modal::Nickname;
                        }
                    });
                });
            });

        egui::CentralPanel::default().show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space((ui.available_height() * 0.12).max(24.0));
                ui.label(
                    RichText::new("Transmita sua tela para quem estiver na sala.")
                        .size(30.0)
                        .strong(),
                );
                ui.add_space(6.0);
                ui.label(
                    RichText::new(
                        "Sem conta e sem gravação. O vídeo vai direto entre os participantes.",
                    )
                    .color(theme::TEXT_DIM),
                );
                ui.add_space(28.0);
                let width = ui.available_width().min(940.0);
                ui.allocate_ui(Vec2::new(width, 0.0), |ui| {
                    ui.columns(2, |columns| {
                        self.create_card(&mut columns[0]);
                        self.join_card(&mut columns[1]);
                    });
                    // Both cards fill up to the taller one's content next frame.
                    self.home.target = self.home.natural[0].max(self.home.natural[1]);
                });
            });
        });
    }

    fn create_card(&mut self, ui: &mut egui::Ui) {
        let shown = theme::card().show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(theme::icons::PLUS)
                        .size(20.0)
                        .color(theme::INDIGO),
                );
                ui.label(RichText::new("Criar nova sala").size(20.0).strong());
            });
            ui.add_space(6.0);
            egui::Frame::new()
                .fill(Color32::from_rgb(10, 11, 16))
                .corner_radius(12)
                .inner_margin(14.0)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    theme::section_title(ui, "Código da sala");
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(&self.home.preview_code)
                                .size(24.0)
                                .monospace()
                                .strong(),
                        );
                    });
                    ui.label(
                        RichText::new("Expira 5 min depois que todos saírem")
                            .small()
                            .color(theme::TEXT_FAINT),
                    );
                });
            ui.add_space(16.0);
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(RichText::new("Exigir aprovação para entrar").strong());
                    ui.label(
                        RichText::new("Você decide quem entra, como administrador")
                            .small()
                            .color(theme::TEXT_DIM),
                    );
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    theme::toggle(ui, &mut self.home.require_approval);
                });
            });
            // The button sits at the bottom, level with the other card's: the
            // slack of the shorter card goes above it.
            ui.add_space(10.0);
            let slack = (self.home.target - self.home.natural[0]).max(0.0);
            ui.add_space(slack);
            if theme::wide_primary_button(
                ui,
                format!("{}  Criar sala", theme::icons::LIGHTNING),
                ui.available_width(),
            )
            .clicked()
            {
                let code = self.home.preview_code.clone();
                self.enter_room(&code, true);
            }
            self.home.natural[0] = ui.min_rect().height() - slack;
        });
        self.home.measured[0] = shown.response.rect.height();
    }

    fn join_card(&mut self, ui: &mut egui::Ui) {
        let shown = theme::card().show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(theme::icons::SIGN_IN)
                        .size(20.0)
                        .color(theme::GREEN),
                );
                ui.label(RichText::new("Entrar em uma sala").size(20.0).strong());
            });
            ui.add_space(6.0);
            theme::section_title(ui, "Código da sala");
            let response = egui::TextEdit::singleline(&mut self.home.join_code)
                .hint_text("XXXX-XXXX")
                .font(egui::TextStyle::Monospace)
                .char_limit(9)
                .desired_width(f32::INFINITY)
                .margin(Vec2::new(12.0, 10.0))
                .show(ui)
                .response;
            ui.add_space(4.0);
            theme::section_title(ui, "Como você entra");
            for (mode, title, hint) in [
                (
                    EntryMode::Watching,
                    "Assistindo",
                    "Microfone e tela desligados",
                ),
                (
                    EntryMode::Broadcasting,
                    "Transmitindo",
                    "Abre a configuração de transmissão ao entrar",
                ),
            ] {
                let selected = self.home.mode == mode;
                let frame = egui::Frame::new()
                    .fill(if selected {
                        theme::INDIGO_DIM
                    } else {
                        Color32::from_rgb(10, 11, 16)
                    })
                    .corner_radius(12)
                    .inner_margin(10.0);
                let outer = frame.show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new(title).strong());
                    ui.label(RichText::new(hint).small().color(theme::TEXT_DIM));
                });
                let rect = outer.response.rect;
                let click = ui.interact(rect, ui.scope_id().with(title), Sense::click());
                if selected {
                    theme::outline(ui, rect, theme::INDIGO);
                }
                if click.clicked() {
                    self.home.mode = mode;
                }
            }
            let ready = looks_like_code(&self.home.join_code);
            let mut enter =
                response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && ready;
            ui.add_space(10.0);
            let slack = (self.home.target - self.home.natural[1]).max(0.0);
            ui.add_space(slack);
            enter |= ui
                .add_enabled_ui(ready, |ui| {
                    theme::success_button(
                        ui,
                        format!("{}  Entrar na sala", theme::icons::BROADCAST),
                        ui.available_width(),
                    )
                })
                .inner
                .clicked();
            self.home.natural[1] = ui.min_rect().height() - slack;
            if enter && ready {
                let code = self.home.join_code.to_uppercase();
                self.enter_room(&code, false);
            }
        });
        self.home.measured[1] = shown.response.rect.height();
    }
}
