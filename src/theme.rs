//! Visual language for KaraokeKit: dark studio panels with a violet accent.

use egui::{Color32, CornerRadius, Margin, Stroke};

pub const BG: Color32 = Color32::from_rgb(11, 13, 20);
pub const PANEL: Color32 = Color32::from_rgb(18, 21, 31);
pub const CARD: Color32 = Color32::from_rgb(24, 28, 41);
pub const CARD_HOVER: Color32 = Color32::from_rgb(32, 37, 54);
pub const BORDER: Color32 = Color32::from_rgb(42, 48, 67);
pub const TEXT: Color32 = Color32::from_rgb(231, 234, 243);
pub const MUTED: Color32 = Color32::from_rgb(143, 151, 176);
pub const ACCENT: Color32 = Color32::from_rgb(124, 92, 255);
pub const ACCENT_SOFT: Color32 = Color32::from_rgb(73, 58, 140);
pub const GOLD: Color32 = Color32::from_rgb(255, 213, 74);
pub const OK: Color32 = Color32::from_rgb(86, 214, 166);
pub const DANGER: Color32 = Color32::from_rgb(255, 110, 124);

pub fn apply(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.override_text_color = Some(TEXT);
    visuals.panel_fill = BG;
    visuals.window_fill = PANEL;
    visuals.extreme_bg_color = Color32::from_rgb(8, 10, 16);
    visuals.faint_bg_color = CARD;
    visuals.hyperlink_color = ACCENT;
    visuals.selection.bg_fill = ACCENT_SOFT;
    visuals.selection.stroke = Stroke::new(1.0, GOLD);
    visuals.widgets.inactive.bg_fill = CARD;
    visuals.widgets.inactive.weak_bg_fill = CARD;
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, BORDER);
    visuals.widgets.inactive.corner_radius = CornerRadius::same(8);
    visuals.widgets.hovered.bg_fill = CARD_HOVER;
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, ACCENT);
    visuals.widgets.hovered.corner_radius = CornerRadius::same(8);
    visuals.widgets.active.bg_fill = ACCENT_SOFT;
    visuals.widgets.active.corner_radius = CornerRadius::same(8);
    visuals.window_corner_radius = CornerRadius::same(14);
    visuals.menu_corner_radius = CornerRadius::same(10);
    ctx.set_visuals(visuals);

    ctx.all_styles_mut(|style| {
        style.spacing.item_spacing = egui::vec2(8.0, 8.0);
        style.spacing.button_padding = egui::vec2(10.0, 6.0);
        style.spacing.slider_width = 170.0;
        style.spacing.interact_size.y = 24.0;
        style.text_styles.insert(
            egui::TextStyle::Heading,
            egui::FontId::new(20.0, egui::FontFamily::Proportional),
        );
        style.text_styles.insert(
            egui::TextStyle::Body,
            egui::FontId::new(14.0, egui::FontFamily::Proportional),
        );
    });
}

pub fn card() -> egui::Frame {
    egui::Frame::NONE
        .fill(CARD)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::same(12))
}

pub fn section_label(ui: &mut egui::Ui, text: &str) {
    ui.label(
        egui::RichText::new(text.to_uppercase())
            .size(11.0)
            .strong()
            .color(MUTED),
    );
}
