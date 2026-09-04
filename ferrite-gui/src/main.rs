//! The Ferrite desktop application entry point. See `app` for the actual
//! UI and scan-control logic, `theme` for the visual style applied here.

mod app;
mod theme;

/// Raw RGBA8 pixels (no header), pre-processed to exactly 64x64 - see the
/// vault's `progress-log.md` for how this and `assets/ferrite.ico` (the
/// exe's own embedded icon, set in `build.rs`) were both generated from
/// the same source artwork. Embedding pre-decoded pixels avoids needing
/// the `image` crate as a runtime dependency just to decode a PNG at
/// startup.
const ICON_RGBA_64: &[u8] = include_bytes!("../assets/icon_64.rgba");
const ICON_SIZE: u32 = 64;

fn main() -> eframe::Result {
    let icon = eframe::egui::IconData {
        rgba: ICON_RGBA_64.to_vec(),
        width: ICON_SIZE,
        height: ICON_SIZE,
    };

    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([1024.0, 700.0])
            .with_icon(icon),
        ..Default::default()
    };

    eframe::run_native(
        "Ferrite",
        options,
        Box::new(|creation_context| {
            // Force dark mode regardless of the system preference, so the
            // custom theme always applies consistently rather than
            // occasionally being overridden by a light-mode default.
            creation_context
                .egui_ctx
                .set_theme(eframe::egui::ThemePreference::Dark);
            // Fonts before style: the style names font families
            // (`archivo-extrabold` and the rest), and egui panics on a
            // family nothing is bound to.
            creation_context.egui_ctx.set_fonts(theme::fonts());
            creation_context
                .egui_ctx
                .set_style_of(eframe::egui::Theme::Dark, theme::style());
            Ok(Box::new(app::FerriteApp::new()))
        }),
    )
}
