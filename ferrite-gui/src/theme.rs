//! Ferrite's visual theme: a dark, single-accent palette with softer
//! rounding and more breathing room than stock `egui` defaults. Applied
//! once at startup (see `main.rs`) rather than left as egui's out-of-the-box
//! look.

use eframe::egui;

pub const BG_WINDOW: egui::Color32 = egui::Color32::from_rgb(0x18, 0x1B, 0x20);
pub const BG_CARD: egui::Color32 = egui::Color32::from_rgb(0x20, 0x24, 0x2B);
pub const BG_INPUT: egui::Color32 = egui::Color32::from_rgb(0x12, 0x15, 0x1A);
pub const BORDER: egui::Color32 = egui::Color32::from_rgb(0x2E, 0x33, 0x3C);
pub const TEXT: egui::Color32 = egui::Color32::from_rgb(0xE8, 0xE9, 0xEC);
pub const TEXT_WEAK: egui::Color32 = egui::Color32::from_rgb(0x9A, 0xA0, 0xAC);
pub const ACCENT: egui::Color32 = egui::Color32::from_rgb(0x5B, 0x8D, 0xEF);
pub const ERROR: egui::Color32 = egui::Color32::from_rgb(0xE5, 0x48, 0x4D);

const WIDGET_CORNER_RADIUS: u8 = 6;
const WINDOW_CORNER_RADIUS: u8 = 8;
const CARD_CORNER_RADIUS: u8 = 10;

/// The full style: spacing/padding (more breathing room than egui's fairly
/// tight defaults) plus [`visuals`] baked in, so callers only need to set
/// one thing.
pub fn style() -> egui::Style {
    let mut style = egui::Style::default();
    style.spacing.item_spacing = egui::vec2(10.0, 8.0);
    style.spacing.button_padding = egui::vec2(10.0, 6.0);
    style.spacing.indent = 18.0;
    style.visuals = visuals();
    style
}

/// The color/rounding half of the theme.
pub fn visuals() -> egui::Visuals {
    let mut visuals = egui::Visuals::dark();

    visuals.override_text_color = Some(TEXT);
    visuals.weak_text_color = Some(TEXT_WEAK);
    visuals.hyperlink_color = ACCENT;
    visuals.panel_fill = BG_WINDOW;
    visuals.window_fill = BG_WINDOW;
    visuals.extreme_bg_color = BG_INPUT;
    visuals.faint_bg_color = BG_CARD;
    visuals.window_corner_radius = egui::CornerRadius::same(WINDOW_CORNER_RADIUS);
    visuals.menu_corner_radius = egui::CornerRadius::same(WINDOW_CORNER_RADIUS);

    visuals.selection.bg_fill = ACCENT;
    visuals.selection.stroke = egui::Stroke::new(1.0, TEXT);

    for widget in [
        &mut visuals.widgets.noninteractive,
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        widget.corner_radius = egui::CornerRadius::same(WIDGET_CORNER_RADIUS);
    }
    visuals.widgets.inactive.bg_fill = BG_CARD;
    visuals.widgets.inactive.weak_bg_fill = BG_CARD;
    visuals.widgets.hovered.bg_fill = ACCENT.gamma_multiply(0.35);
    visuals.widgets.active.bg_fill = ACCENT;

    visuals
}

/// A bordered, filled "card" frame — used to visually group each top-level
/// section instead of a bare `ui.separator()` line.
pub fn card() -> egui::Frame {
    egui::Frame::new()
        .fill(BG_CARD)
        .stroke(egui::Stroke::new(1.0, BORDER))
        .corner_radius(CARD_CORNER_RADIUS)
        .inner_margin(egui::Margin::same(12))
        .outer_margin(egui::Margin::symmetric(0, 6))
}
