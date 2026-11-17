//! First launch: the person picks a nickname (DEC-14, RF-15).

use eframe::egui::{self, Align, Align2, Color32, Layout, RichText, Vec2};

use crate::app::TelinhasApp;
use crate::logo;
use crate::model::{Screen, valid_nickname};
use crate::theme;

impl TelinhasApp {
    pub(crate) fn first_run_ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        egui::CentralPanel::default().show(ui, |_| {});
        let mut done = false;
        egui::Area::new(theme::id("first_run"))
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .show(&ctx, |ui| {
                ui.set_width(420.0);
                ui.vertical_centered(|ui| {
                    ui.add(logo::image(86.0));
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new("Telinhas")
                            .size(22.0)
                            .color(theme::TEXT)
                            .strong(),
                    );
                });
                ui.add_space(18.0);
                done = self.nickname_card(ui, "Como devemos te chamar?", "Continuar");
            });
        if done {
            self.screen = Screen::Home;
        }
    }

    /// The nickname form, used at first launch and when changing it later.
    /// Returns true when the person confirmed a valid nickname.
    pub(crate) fn nickname_card(&mut self, ui: &mut egui::Ui, title: &str, confirm: &str) -> bool {
        let mut done = false;
        theme::card().show(ui, |ui| {
            ui.set_width(420.0);
            ui.label(RichText::new(title).size(22.0).strong());
            ui.add_space(4.0);
            ui.label(
                RichText::new("É o nome que as outras pessoas veem na sala. Você pode trocá-lo quando quiser.")
                    .color(theme::TEXT_DIM),
            );
            ui.add_space(14.0);
            let response = egui::TextEdit::singleline(&mut self.nick_draft)
                .hint_text("Seu apelido")
                .char_limit(24)
                .desired_width(f32::INFINITY)
                .margin(Vec2::new(12.0, 10.0))
                .show(ui)
                .response;
            let valid = valid_nickname(&self.nick_draft);
            ui.add_space(12.0);
            ui.horizontal(|ui| ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let button = ui.add_enabled_ui(valid, |ui| {
                    theme::wide_primary_button(ui, confirm, 140.0)
                });
                let submit = button.inner.clicked()
                    || (response.lost_focus()
                        && ui.input(|i| i.key_pressed(egui::Key::Enter))
                        && valid);
                if submit && valid {
                    self.nickname = self.nick_draft.trim().to_owned();
                    self.nick_draft = self.nickname.clone();
                    done = true;
                }
                if !valid && !self.nick_draft.is_empty() {
                    ui.label(
                        RichText::new("1 a 24 caracteres")
                            .small()
                            .color(Color32::from_rgb(245, 158, 11)),
                    );
                }
            }));
        });
        done
    }
}
