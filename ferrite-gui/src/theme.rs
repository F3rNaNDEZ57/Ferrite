// A design token that nothing references yet is not dead code: this module is
// the palette and the metric set as a whole, and a role defined but unused
// still documents the system (and is what the next screen reaches for). The
// allow is scoped to this file so the rest of the crate keeps the lint.
#![allow(dead_code)]

//! Ferrite's visual theme, implementing the v1.0 design specification.
//!
//! The palette was **replaced** rather than adjusted at v1.0: the cool
//! blue-grey ground moved to a warm neutral axis, and the blue accent became
//! a single oxide red. That red carries exactly one meaning — attention. A
//! red *fill* is an action you may take; red *type* or a red *rule* is a
//! problem you must read. Nothing else in the interface is coloured: there is
//! no green, no amber, no second accent.
//!
//! Two rules from the spec shape everything here:
//!
//! - **Corner radius is zero on every class** — button, field, dropdown,
//!   checkbox, panel, dialog, tag. Uniform, so nothing needs per-corner work.
//! - **No shadow anywhere.** Regions separate by a 2 px rule, and a dialog
//!   separates from the page by a stroke and an opaque scrim, never by
//!   elevation. `egui`'s shadows are switched off explicitly rather than left
//!   at their defaults.
//!
//! Every wash is a pre-composited opaque hex, not a translucent fill: the
//! table redraws over live content every frame, and alpha over a striped row
//! would give two different results on alternating rows.

use eframe::egui;

// ---------------------------------------------------------------------------
// Colour
// ---------------------------------------------------------------------------

/// Window fill, panel fill.
pub const GROUND: egui::Color32 = egui::Color32::from_rgb(0x14, 0x13, 0x12);
/// Rail and dock interior, table header.
pub const SURFACE: egui::Color32 = egui::Color32::from_rgb(0x1C, 0x1A, 0x19);
/// Top bar, striped row, secondary button idle.
pub const SURFACE_RAISED: egui::Color32 = egui::Color32::from_rgb(0x24, 0x22, 0x21);
/// Text fields and the script viewer.
pub const INPUT_INTERIOR: egui::Color32 = egui::Color32::from_rgb(0x0E, 0x0D, 0x0D);
/// 1 px widget bounds and table row rules.
pub const STROKE: egui::Color32 = egui::Color32::from_rgb(0x2A, 0x28, 0x27);
/// 2 px structural rules between regions.
pub const DIVIDER: egui::Color32 = egui::Color32::from_rgb(0x38, 0x35, 0x34);
/// Primary ink: all values and addresses.
pub const TEXT: egui::Color32 = egui::Color32::from_rgb(0xF3, 0xF2, 0xF2);
/// Labels, column headers, units.
pub const TEXT_DIM: egui::Color32 = egui::Color32::from_rgb(0x9B, 0x97, 0x97);
/// Inactive widget text, and the leading zeros of a hex address.
pub const TEXT_FAINT: egui::Color32 = egui::Color32::from_rgb(0x60, 0x5D, 0x5D);
/// Primary action fill, freeze-on, error rule.
pub const ACCENT: egui::Color32 = egui::Color32::from_rgb(0xEC, 0x30, 0x13);
/// Hover fill — and error text at body size, where [`ACCENT`] alone falls
/// under 4.5:1 against the ground.
pub const ACCENT_LIFT: egui::Color32 = egui::Color32::from_rgb(0xFF, 0x56, 0x3C);
/// Active (mouse-down) fill only.
pub const ACCENT_PRESS: egui::Color32 = egui::Color32::from_rgb(0xDD, 0x2B, 0x0F);
/// Selected row, error strip ground, frozen row. Opaque, not alpha.
pub const ACCENT_WASH: egui::Color32 = egui::Color32::from_rgb(0x2B, 0x13, 0x10);
/// 1 px bound on any [`ACCENT_WASH`] surface.
pub const ACCENT_WASH_STROKE: egui::Color32 = egui::Color32::from_rgb(0x4D, 0x17, 0x0E);
/// Hex padding inside a selected row — dimmer than [`TEXT_FAINT`] would be
/// against the wash, so leading zeros stay quiet instead of glowing.
pub const HEX_PAD_ON_WASH: egui::Color32 = egui::Color32::from_rgb(0x8A, 0x5A, 0x50);
/// Hovered row fill, full row width.
pub const ROW_HOVER: egui::Color32 = egui::Color32::from_rgb(0x2E, 0x2B, 0x2A);
/// The scrim behind a modal dialog. Opaque, so nothing shows through.
pub const SCRIM: egui::Color32 = egui::Color32::from_rgb(0x0A, 0x0A, 0x09);
/// Disabled text-field interior.
pub const INPUT_DISABLED: egui::Color32 = egui::Color32::from_rgb(0x16, 0x15, 0x14);

// ---------------------------------------------------------------------------
// Spacing, shape and metrics
// ---------------------------------------------------------------------------

/// The spacing scale. Every gap in the interface is one of these six values.
pub mod space {
    /// Icon-to-label, checkbox inset.
    pub const XS: f32 = 4.0;
    /// `item_spacing.y`, label-to-field.
    pub const SM: f32 = 8.0;
    /// `item_spacing.x`, panel inner margin.
    pub const MD: f32 = 12.0;
    /// Between rail groups.
    pub const LG: f32 = 16.0;
    /// Above a group heading.
    pub const XL: f32 = 24.0;
    /// Empty-state block inset.
    pub const XXL: f32 = 32.0;
}

/// Fixed table row height. The whole table is virtualised against this, so it
/// is a hard number rather than a minimum.
pub const ROW_HEIGHT: f32 = 24.0;
/// Table header height.
pub const HEADER_HEIGHT: f32 = 26.0;
/// A skipped-import row carries a two-line reason, so that list is not the
/// virtualised table and uses its own taller row.
pub const REASON_ROW_HEIGHT: f32 = 44.0;
/// Text field height.
pub const FIELD_HEIGHT: f32 = 28.0;
/// The rail's preferred width, and the floor it shrinks to below 1200 px of
/// window width. It never collapses — it holds the primary action.
pub const RAIL_WIDTH: f32 = 340.0;
/// See [`RAIL_WIDTH`].
pub const RAIL_WIDTH_MIN: f32 = 300.0;
/// The saved-list dock's floor: its header, its column header, and three rows.
pub const DOCK_HEIGHT_MIN: f32 = 132.0;
/// Collapsed, the dock is its header alone.
pub const DOCK_HEIGHT_COLLAPSED: f32 = 26.0;
/// Every fallible field owns this much space beneath it, whether or not there
/// is a message, so a validation error never moves the layout.
pub const MESSAGE_SLOT_HEIGHT: f32 = 32.0;

/// Exact column widths for the results and saved-list tables.
pub mod col {
    /// Selection checkbox.
    pub const SELECT: f32 = 30.0;
    /// Freeze checkbox.
    pub const FREEZE: f32 = 52.0;
    /// A zero-padded 16-digit address, monospace.
    pub const ADDRESS: f32 = 156.0;
    /// A right-aligned numeric value, monospace.
    pub const VALUE: f32 = 140.0;
    /// One 20 px icon button.
    pub const ACTION: f32 = 36.0;
}

/// Window-width breakpoints at which columns drop. A dropped column's content
/// moves to the row's tooltip rather than being lost.
pub mod breakpoint {
    /// Below this, `module + offset` leaves the results table.
    pub const DROP_MODULE: f32 = 1300.0;
    /// Below this, `previous` leaves it too.
    pub const DROP_PREVIOUS: f32 = 1120.0;
    /// Below this, the rail shrinks toward its floor.
    pub const RAIL_SHRINK: f32 = 1200.0;
    /// Below this, top-bar buttons drop their labels for icons.
    pub const TOPBAR_ICONS: f32 = 1150.0;
    /// At or above this, the import report splits side by side; below, it
    /// stacks.
    pub const REPORT_SPLIT: f32 = 1200.0;
}

/// A live value that changed on the last tick flashes its cell ground and
/// decays back to the row ground over this long.
pub const FLASH_DECAY: f32 = 0.4;

// ---------------------------------------------------------------------------
// Type
// ---------------------------------------------------------------------------

/// Font family names registered in [`fonts`]. `Proportional` and `Monospace`
/// carry the 400-weight faces; the rest are separate families because `egui`
/// selects a face by family, not by weight.
pub mod family {
    /// Archivo SemiBold — widget labels and body text.
    pub const SEMIBOLD: &str = "archivo-semibold";
    /// Archivo ExtraBold — section labels and empty-state headlines.
    pub const EXTRABOLD: &str = "archivo-extrabold";
    /// JetBrains Mono Bold — a live value.
    pub const MONO_BOLD: &str = "mono-bold";
}

/// Named text styles from the specification's type scale, in addition to
/// `egui`'s built-in ones.
pub mod text_style {
    /// 11 / 800 / +0.18 em / caps — a section label such as `SCAN`.
    pub const SECTION_LABEL: &str = "section-label";
    /// 11 / 700 / +0.10 em / caps — a table column header.
    pub const TABLE_HEADER: &str = "table-header";
    /// 13 / 400 — secondary and helper text.
    pub const SECONDARY: &str = "secondary";
    /// mono 12 / 400 — an address or a byte pattern.
    pub const MONO_VALUE: &str = "mono-value";
    /// mono 12 / 700 — a live value.
    pub const MONO_LIVE: &str = "mono-live";
    /// 20 / 800 — an empty-state headline.
    pub const EMPTY_HEADLINE: &str = "empty-headline";
}

/// Letter-spacing is not a `TextStyle` property in `egui`, so the two tracked
/// styles apply it by hand. Returns the extra space, in points, to insert
/// between characters.
pub const fn tracking(text_style: &str) -> f32 {
    match text_style.as_bytes() {
        b"section-label" => 11.0 * 0.18,
        b"table-header" => 11.0 * 0.10,
        _ => 0.0,
    }
}

/// Registers Archivo and JetBrains Mono, replacing `egui`'s default stack.
///
/// Both are OFL-licensed and embedded in the binary (`assets/fonts`, with the
/// licence text alongside), so the interface renders identically on a machine
/// with neither installed.
pub fn fonts() -> egui::FontDefinitions {
    let mut fonts = egui::FontDefinitions::empty();

    let faces: [(&str, &[u8]); 5] = [
        (
            "archivo-regular",
            include_bytes!("../assets/fonts/Archivo-Regular.ttf"),
        ),
        (
            family::SEMIBOLD,
            include_bytes!("../assets/fonts/Archivo-SemiBold.ttf"),
        ),
        (
            family::EXTRABOLD,
            include_bytes!("../assets/fonts/Archivo-ExtraBold.ttf"),
        ),
        (
            "mono-regular",
            include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf"),
        ),
        (
            family::MONO_BOLD,
            include_bytes!("../assets/fonts/JetBrainsMono-Bold.ttf"),
        ),
    ];
    for (name, bytes) in faces {
        fonts.font_data.insert(
            name.to_owned(),
            std::sync::Arc::new(egui::FontData::from_static(bytes)),
        );
    }

    // Each weight is its own family. A family's list is a fallback chain, so
    // the mono faces are appended to every proportional family too: a glyph
    // Archivo lacks still renders rather than showing a tofu box.
    let mut family_of = |family: egui::FontFamily, primary: &str| {
        fonts
            .families
            .insert(family, vec![primary.to_owned(), "mono-regular".to_owned()]);
    };
    family_of(egui::FontFamily::Proportional, "archivo-regular");
    family_of(egui::FontFamily::Monospace, "mono-regular");
    for name in [family::SEMIBOLD, family::EXTRABOLD, family::MONO_BOLD] {
        fonts.families.insert(
            egui::FontFamily::Name(name.into()),
            vec![name.to_owned(), "mono-regular".to_owned()],
        );
    }

    fonts
}

/// A [`egui::FontId`] for one of the named styles in [`text_style`].
pub fn font(style: &str) -> egui::FontId {
    let named = |name: &str| egui::FontFamily::Name(name.into());
    match style {
        text_style::SECTION_LABEL => egui::FontId::new(11.0, named(family::EXTRABOLD)),
        text_style::TABLE_HEADER => egui::FontId::new(11.0, named(family::SEMIBOLD)),
        text_style::SECONDARY => egui::FontId::new(13.0, egui::FontFamily::Proportional),
        text_style::MONO_VALUE => egui::FontId::new(12.0, egui::FontFamily::Monospace),
        text_style::MONO_LIVE => egui::FontId::new(12.0, named(family::MONO_BOLD)),
        text_style::EMPTY_HEADLINE => egui::FontId::new(20.0, named(family::EXTRABOLD)),
        _ => egui::FontId::new(14.0, named(family::SEMIBOLD)),
    }
}

// ---------------------------------------------------------------------------
// Style
// ---------------------------------------------------------------------------

/// The full style: the type scale, spacing and [`visuals`] together, so a
/// caller sets one thing.
pub fn style() -> egui::Style {
    let mut style = egui::Style::default();

    style.spacing.item_spacing = egui::vec2(space::MD, space::SM);
    style.spacing.button_padding = egui::vec2(space::MD, 6.0);
    style.spacing.indent = space::LG;
    style.spacing.interact_size.y = FIELD_HEIGHT;
    style.spacing.icon_width = 14.0; // checkbox box
    style.spacing.icon_width_inner = 9.0;
    style.spacing.scroll.bar_width = 10.0;

    // The built-in styles, retuned to the spec's scale. Body and widget label
    // share one style at 14/600 — the spec treats them as the same thing.
    let semibold = egui::FontFamily::Name(family::SEMIBOLD.into());
    style.text_styles = [
        (
            egui::TextStyle::Heading,
            egui::FontId::new(20.0, egui::FontFamily::Name(family::EXTRABOLD.into())),
        ),
        (
            egui::TextStyle::Body,
            egui::FontId::new(14.0, semibold.clone()),
        ),
        (
            egui::TextStyle::Button,
            egui::FontId::new(14.0, semibold.clone()),
        ),
        (
            egui::TextStyle::Small,
            egui::FontId::new(13.0, egui::FontFamily::Proportional),
        ),
        (
            egui::TextStyle::Monospace,
            egui::FontId::new(12.0, egui::FontFamily::Monospace),
        ),
    ]
    .into();
    for name in [
        text_style::SECTION_LABEL,
        text_style::TABLE_HEADER,
        text_style::SECONDARY,
        text_style::MONO_VALUE,
        text_style::MONO_LIVE,
        text_style::EMPTY_HEADLINE,
    ] {
        style
            .text_styles
            .insert(egui::TextStyle::Name(name.into()), font(name));
    }

    style.visuals = visuals();
    style
}

/// The colour, stroke and shape half of the theme.
pub fn visuals() -> egui::Visuals {
    let mut visuals = egui::Visuals::dark();

    visuals.override_text_color = Some(TEXT);
    visuals.weak_text_color = Some(TEXT_DIM);
    visuals.hyperlink_color = ACCENT_LIFT;
    visuals.panel_fill = GROUND;
    visuals.window_fill = SURFACE;
    visuals.faint_bg_color = SURFACE_RAISED; // striped rows
    visuals.extreme_bg_color = INPUT_INTERIOR;
    visuals.window_stroke = egui::Stroke::new(2.0, DIVIDER);

    // Zero radius on every class, and no elevation anywhere.
    visuals.window_corner_radius = egui::CornerRadius::ZERO;
    visuals.menu_corner_radius = egui::CornerRadius::ZERO;
    visuals.window_shadow = egui::epaint::Shadow::NONE;
    visuals.popup_shadow = egui::epaint::Shadow::NONE;

    // Selection: the accent wash, bounded, never a translucent highlight.
    // The stroke is the accent rather than plain ink because egui draws a
    // focused text field's frame with it — the spec's focus ring is 2 px of
    // accent, and a white ring here would be the one un-themed edge in the
    // interface.
    visuals.selection.bg_fill = ACCENT_WASH;
    visuals.selection.stroke = egui::Stroke::new(2.0, ACCENT);

    // The five states the spec specifies, mapped onto egui's five.
    // `noninteractive` is what `add_enabled(false)` renders, so it carries the
    // disabled treatment rather than a merely quieter idle one.
    let w = &mut visuals.widgets;

    w.noninteractive.bg_fill = SURFACE_RAISED;
    w.noninteractive.weak_bg_fill = SURFACE_RAISED;
    w.noninteractive.bg_stroke = egui::Stroke::new(1.0, STROKE);
    w.noninteractive.fg_stroke = egui::Stroke::new(1.0, TEXT_FAINT);

    w.inactive.bg_fill = SURFACE_RAISED;
    w.inactive.weak_bg_fill = SURFACE_RAISED;
    w.inactive.bg_stroke = egui::Stroke::new(1.0, STROKE);
    w.inactive.fg_stroke = egui::Stroke::new(1.0, TEXT);

    w.hovered.bg_fill = ROW_HOVER;
    w.hovered.weak_bg_fill = ROW_HOVER;
    w.hovered.bg_stroke = egui::Stroke::new(1.0, DIVIDER);
    w.hovered.fg_stroke = egui::Stroke::new(1.0, TEXT);

    w.active.bg_fill = ACCENT_WASH;
    w.active.weak_bg_fill = ACCENT_WASH;
    w.active.bg_stroke = egui::Stroke::new(1.0, ACCENT);
    w.active.fg_stroke = egui::Stroke::new(1.0, TEXT);

    w.open.bg_fill = SURFACE_RAISED;
    w.open.weak_bg_fill = SURFACE_RAISED;
    w.open.bg_stroke = egui::Stroke::new(1.0, ACCENT);
    w.open.fg_stroke = egui::Stroke::new(1.0, TEXT);

    for widget in [
        &mut w.noninteractive,
        &mut w.inactive,
        &mut w.hovered,
        &mut w.active,
        &mut w.open,
    ] {
        widget.corner_radius = egui::CornerRadius::ZERO;
        // egui grows a hovered/active widget by a couple of points by
        // default. At zero radius that reads as a wobble, so it is off.
        widget.expansion = 0.0;
    }

    visuals
}

/// A panel frame: flat fill, 12 px inner margin, no outer margin. Panels meet
/// on a 2 px rule rather than floating apart, so there is deliberately no
/// gap between them.
pub fn panel(fill: egui::Color32) -> egui::Frame {
    egui::Frame::new()
        .fill(fill)
        .inner_margin(egui::Margin::same(space::MD as i8))
}

/// The 2 px structural rule drawn between two regions.
pub fn divider_stroke() -> egui::Stroke {
    egui::Stroke::new(2.0, DIVIDER)
}

/// A primary action: accent fill, ground-coloured ink — unless the
/// surrounding `Ui` is disabled, in which case it takes the disabled
/// treatment instead.
///
/// Takes the `Ui` rather than returning a bare `Button` because an explicit
/// `.fill()` overrides egui's own disabled visuals: a greyed-out button
/// would otherwise keep its full accent fill and still read as the thing to
/// press.
pub fn primary(ui: &mut egui::Ui, text: impl Into<String>) -> egui::Response {
    let enabled = ui.is_enabled();
    let (fill, ink) = if enabled {
        (ACCENT, GROUND)
    } else {
        (SURFACE_RAISED, TEXT_FAINT)
    };
    ui.add(
        egui::Button::new(egui::RichText::new(text).color(ink))
            .fill(fill)
            .corner_radius(egui::CornerRadius::ZERO),
    )
}

/// A section label — `SCAN`, `RESULTS`, `SAVED LIST`. Caps and tracked by
/// hand, since `egui` has no letter-spacing property.
pub fn section_label(ui: &mut egui::Ui, text: &str) -> egui::Response {
    tracked_label(ui, text, text_style::SECTION_LABEL, TEXT_DIM)
}

/// A table column header.
pub fn column_header(ui: &mut egui::Ui, text: &str) -> egui::Response {
    tracked_label(ui, text, text_style::TABLE_HEADER, TEXT_DIM)
}

/// Renders `text` uppercased with letter-spacing, as one `Label` — one
/// widget, so it stays a single node in the accessibility tree rather than
/// one node per character.
fn tracked_label(
    ui: &mut egui::Ui,
    text: &str,
    style: &str,
    color: egui::Color32,
) -> egui::Response {
    let extra = tracking(style);
    let upper = text.to_uppercase();
    let mut job = egui::text::LayoutJob::default();
    let font = font(style);
    for ch in upper.chars() {
        job.append(
            &ch.to_string(),
            0.0,
            egui::TextFormat {
                font_id: font.clone(),
                color,
                extra_letter_spacing: extra,
                ..Default::default()
            },
        );
    }
    // The accessible name has to stay the readable text, not the tracked
    // glyph run, or the UI Automation driver can't find it by name.
    ui.add(egui::Label::new(job).selectable(false))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_named_text_style_resolves_to_a_font() {
        // A typo in a style name would otherwise fall through to the default
        // arm and silently render at the wrong size.
        for (name, size) in [
            (text_style::SECTION_LABEL, 11.0),
            (text_style::TABLE_HEADER, 11.0),
            (text_style::SECONDARY, 13.0),
            (text_style::MONO_VALUE, 12.0),
            (text_style::MONO_LIVE, 12.0),
            (text_style::EMPTY_HEADLINE, 20.0),
        ] {
            assert_eq!(font(name).size, size, "wrong size for {name}");
        }
    }

    #[test]
    fn the_style_registers_every_named_text_style() {
        let style = style();
        for name in [
            text_style::SECTION_LABEL,
            text_style::TABLE_HEADER,
            text_style::SECONDARY,
            text_style::MONO_VALUE,
            text_style::MONO_LIVE,
            text_style::EMPTY_HEADLINE,
        ] {
            assert!(
                style
                    .text_styles
                    .contains_key(&egui::TextStyle::Name(name.into())),
                "{name} missing from the style"
            );
        }
    }

    #[test]
    fn every_font_family_the_theme_names_is_registered() {
        let fonts = fonts();
        for name in [family::SEMIBOLD, family::EXTRABOLD, family::MONO_BOLD] {
            assert!(
                fonts
                    .families
                    .contains_key(&egui::FontFamily::Name(name.into())),
                "{name} family missing"
            );
        }
        assert!(fonts.families.contains_key(&egui::FontFamily::Proportional));
        assert!(fonts.families.contains_key(&egui::FontFamily::Monospace));
    }

    #[test]
    fn nothing_is_rounded_and_nothing_casts_a_shadow() {
        // Both are spec rules rather than taste, and both are easy to
        // reintroduce by accident when adding a widget later.
        let v = visuals();
        assert_eq!(v.window_corner_radius, egui::CornerRadius::ZERO);
        assert_eq!(v.menu_corner_radius, egui::CornerRadius::ZERO);
        assert_eq!(v.window_shadow, egui::epaint::Shadow::NONE);
        assert_eq!(v.popup_shadow, egui::epaint::Shadow::NONE);
        for widget in [
            &v.widgets.noninteractive,
            &v.widgets.inactive,
            &v.widgets.hovered,
            &v.widgets.active,
            &v.widgets.open,
        ] {
            assert_eq!(widget.corner_radius, egui::CornerRadius::ZERO);
            assert_eq!(widget.expansion, 0.0);
        }
    }

    #[test]
    fn only_the_tracked_styles_carry_letter_spacing() {
        assert!(tracking(text_style::SECTION_LABEL) > tracking(text_style::TABLE_HEADER));
        assert_eq!(tracking(text_style::SECONDARY), 0.0);
        assert_eq!(tracking(text_style::MONO_VALUE), 0.0);
    }
}
