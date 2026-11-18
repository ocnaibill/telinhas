//! Colors, fonts and the small widgets the screens are built from, after the
//! Stitch mockup (dark, indigo for actions, green for "live", rounded cards).

use std::sync::Arc;

use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontData, FontFamily, FontId, Pos2, Rect, Response,
    RichText, Sense, Stroke, StrokeKind, Vec2,
};

pub use egui_phosphor::regular as icons;

pub const BG: Color32 = Color32::from_rgb(15, 16, 22);
pub const PANEL: Color32 = Color32::from_rgb(22, 23, 31);
pub const CARD: Color32 = Color32::from_rgb(30, 31, 41);
pub const CARD_HI: Color32 = Color32::from_rgb(40, 42, 56);
pub const STROKE: Color32 = Color32::from_rgb(50, 52, 68);
pub const INDIGO: Color32 = Color32::from_rgb(88, 101, 242);
pub const INDIGO_DIM: Color32 = Color32::from_rgb(46, 52, 120);
pub const GREEN: Color32 = Color32::from_rgb(34, 197, 94);
pub const GREEN_DIM: Color32 = Color32::from_rgb(20, 60, 40);
pub const RED: Color32 = Color32::from_rgb(220, 38, 38);
pub const AMBER: Color32 = Color32::from_rgb(245, 158, 11);
pub const AMBER_DIM: Color32 = Color32::from_rgb(70, 48, 12);
pub const TEXT: Color32 = Color32::from_rgb(236, 237, 243);
pub const TEXT_DIM: Color32 = Color32::from_rgb(160, 163, 180);
pub const TEXT_FAINT: Color32 = Color32::from_rgb(110, 113, 132);

/// A stable id for areas and modals created from a name.
pub fn id(name: &str) -> egui::Id {
    #[allow(deprecated)]
    egui::Id::new(name)
}

/// Sets visuals, fonts (with the icon font as a fallback) and spacing.
pub fn install(ctx: &egui::Context) {
    egui_extras::install_image_loaders(ctx);
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "phosphor".to_owned(),
        Arc::new(FontData::from_static(egui_phosphor::bytes::regular::FONT)),
    );
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        if let Some(list) = fonts.families.get_mut(&family) {
            list.push("phosphor".to_owned());
        }
    }
    ctx.set_fonts(fonts);

    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = BG;
    visuals.window_fill = PANEL;
    visuals.extreme_bg_color = Color32::from_rgb(10, 11, 16);
    visuals.faint_bg_color = CARD;
    visuals.window_corner_radius = CornerRadius::same(18);
    visuals.window_stroke = Stroke::new(1.0, STROKE);
    visuals.selection.bg_fill = INDIGO;
    visuals.selection.stroke = Stroke::new(1.0, Color32::WHITE);
    visuals.override_text_color = Some(TEXT);
    let rounding = CornerRadius::same(10);
    for widget in [
        &mut visuals.widgets.noninteractive,
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        widget.corner_radius = rounding;
    }
    visuals.widgets.noninteractive.bg_fill = PANEL;
    visuals.widgets.inactive.bg_fill = CARD;
    visuals.widgets.inactive.weak_bg_fill = CARD;
    visuals.widgets.hovered.bg_fill = CARD_HI;
    visuals.widgets.hovered.weak_bg_fill = CARD_HI;
    visuals.widgets.active.bg_fill = INDIGO_DIM;
    visuals.widgets.active.weak_bg_fill = INDIGO_DIM;
    // Always dark, whatever the system's app mode is: the palette is ours.
    ctx.set_theme(egui::ThemePreference::Dark);
    ctx.set_visuals_of(egui::Theme::Dark, visuals);

    ctx.global_style_mut(|style| {
        style.spacing.item_spacing = Vec2::new(10.0, 10.0);
        style.spacing.button_padding = Vec2::new(14.0, 8.0);
        style
            .text_styles
            .insert(egui::TextStyle::Body, FontId::proportional(14.0));
        style
            .text_styles
            .insert(egui::TextStyle::Button, FontId::proportional(14.0));
        style
            .text_styles
            .insert(egui::TextStyle::Heading, FontId::proportional(24.0));
        style
            .text_styles
            .insert(egui::TextStyle::Small, FontId::proportional(11.5));
    });
}

/// A rounded card, as in the mockup.
pub fn card() -> egui::Frame {
    egui::Frame::new()
        .fill(CARD)
        .corner_radius(CornerRadius::same(16))
        .inner_margin(16.0)
}

pub fn section_title(ui: &mut egui::Ui, text: &str) {
    ui.label(
        RichText::new(text.to_uppercase())
            .size(11.5)
            .color(TEXT_FAINT)
            .strong(),
    );
}

/// An inline pill: rounded, small, colored.
pub fn pill(ui: &mut egui::Ui, text: impl Into<String>, fg: Color32, bg: Color32) -> Response {
    let galley = ui
        .painter()
        .layout_no_wrap(text.into(), FontId::proportional(12.5), fg);
    let size = galley.size() + Vec2::new(20.0, 10.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(255), bg);
    ui.painter()
        .galley(rect.min + Vec2::new(10.0, 5.0), galley, fg);
    response
}

fn button_base(
    ui: &mut egui::Ui,
    text: impl Into<String>,
    fg: Color32,
    fill: Color32,
    hover: Color32,
    min_width: f32,
) -> Response {
    let galley = ui
        .painter()
        .layout_no_wrap(text.into(), FontId::proportional(14.0), fg);
    let size = Vec2::new(
        (galley.size().x + 32.0).max(min_width),
        galley.size().y + 18.0,
    );
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let color = if response.hovered() { hover } else { fill };
    ui.painter()
        .rect_filled(rect, CornerRadius::same(12), color);
    let at = rect.center() - galley.size() / 2.0;
    ui.painter().galley(at, galley, fg);
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

pub fn primary_button(ui: &mut egui::Ui, text: impl Into<String>) -> Response {
    button_base(
        ui,
        text,
        Color32::WHITE,
        INDIGO,
        Color32::from_rgb(105, 117, 250),
        0.0,
    )
}

pub fn wide_primary_button(ui: &mut egui::Ui, text: impl Into<String>, width: f32) -> Response {
    button_base(
        ui,
        text,
        Color32::WHITE,
        INDIGO,
        Color32::from_rgb(105, 117, 250),
        width,
    )
}

pub fn success_button(ui: &mut egui::Ui, text: impl Into<String>, width: f32) -> Response {
    button_base(
        ui,
        text,
        Color32::WHITE,
        Color32::from_rgb(22, 163, 74),
        Color32::from_rgb(34, 197, 94),
        width,
    )
}

pub fn secondary_button(ui: &mut egui::Ui, text: impl Into<String>) -> Response {
    button_base(ui, text, TEXT, CARD_HI, STROKE, 0.0)
}

pub fn danger_button(ui: &mut egui::Ui, text: impl Into<String>) -> Response {
    button_base(
        ui,
        text,
        Color32::WHITE,
        RED,
        Color32::from_rgb(239, 68, 68),
        0.0,
    )
}

/// A round icon button for the floating bar. `active` paints it as on.
pub fn icon_button(ui: &mut egui::Ui, icon: &str, tip: &str, active: bool) -> Response {
    let size = Vec2::splat(40.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let fill = match (active, response.hovered()) {
        (true, true) => Color32::from_rgb(105, 117, 250),
        (true, false) => INDIGO,
        (false, true) => CARD_HI,
        (false, false) => CARD,
    };
    ui.painter().circle_filled(rect.center(), 20.0, fill);
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        icon,
        FontId::proportional(19.0),
        TEXT,
    );
    response
        .on_hover_text(tip)
        .on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A switch. Returns the response; `on` is flipped when clicked.
pub fn toggle(ui: &mut egui::Ui, on: &mut bool) -> Response {
    let size = Vec2::new(42.0, 24.0);
    let (rect, mut response) = ui.allocate_exact_size(size, Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    let t = ui.ctx().animate_bool_responsive(response.id, *on);
    let track = if *on {
        GREEN
    } else {
        Color32::from_rgb(58, 60, 76)
    };
    ui.painter()
        .rect_filled(rect, CornerRadius::same(255), track);
    let x = egui::lerp(rect.left() + 12.0..=rect.right() - 12.0, t);
    ui.painter()
        .circle_filled(Pos2::new(x, rect.center().y), 9.0, Color32::WHITE);
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A row of options in a dark rounded track; the chosen one is filled.
/// Returns true when the choice changed.
pub fn segmented<T: PartialEq + Copy>(
    ui: &mut egui::Ui,
    current: &mut T,
    options: &[(T, &str)],
) -> bool {
    let mut changed = false;
    egui::Frame::new()
        .fill(Color32::from_rgb(12, 13, 19))
        .corner_radius(CornerRadius::same(14))
        .inner_margin(4.0)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = Vec2::new(4.0, 0.0);
            ui.horizontal(|ui| {
                for (value, label) in options {
                    let selected = *current == *value;
                    let galley = ui.painter().layout_no_wrap(
                        (*label).to_owned(),
                        FontId::proportional(13.5),
                        if selected { Color32::WHITE } else { TEXT_DIM },
                    );
                    let size = Vec2::new(galley.size().x + 28.0, galley.size().y + 14.0);
                    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
                    if selected {
                        ui.painter()
                            .rect_filled(rect, CornerRadius::same(10), INDIGO);
                    } else if response.hovered() {
                        ui.painter().rect_filled(rect, CornerRadius::same(10), CARD);
                    }
                    let color = if selected { Color32::WHITE } else { TEXT_DIM };
                    ui.painter().galley_with_override_text_color(
                        rect.center() - galley.size() / 2.0,
                        galley,
                        color,
                    );
                    if response.clicked() && !selected {
                        *current = *value;
                        changed = true;
                    }
                    response.on_hover_cursor(egui::CursorIcon::PointingHand);
                }
            });
        });
    changed
}

/// A round avatar with an initial.
pub fn avatar(painter: &egui::Painter, center: Pos2, radius: f32, name: &str, color: Color32) {
    painter.circle_filled(center, radius, color);
    let initial = name
        .chars()
        .next()
        .map_or("?".to_owned(), |c| c.to_uppercase().to_string());
    painter.text(
        center,
        Align2::CENTER_CENTER,
        initial,
        FontId::proportional(radius * 1.0),
        Color32::WHITE,
    );
}

/// A color per person, stable for a name.
pub fn color_for(name: &str) -> Color32 {
    const COLORS: [Color32; 6] = [
        Color32::from_rgb(88, 101, 242),
        Color32::from_rgb(34, 160, 94),
        Color32::from_rgb(217, 70, 140),
        Color32::from_rgb(234, 138, 30),
        Color32::from_rgb(20, 150, 190),
        Color32::from_rgb(150, 90, 220),
    ];
    let hash = name
        .bytes()
        .fold(7usize, |h, b| h.wrapping_mul(31).wrapping_add(b as usize));
    COLORS[hash % COLORS.len()]
}

/// A thin outline for focused fields and selected cards.
pub fn outline(ui: &egui::Ui, rect: Rect, color: Color32) {
    ui.painter().rect_stroke(
        rect,
        CornerRadius::same(14),
        Stroke::new(2.0, color),
        StrokeKind::Inside,
    );
}
