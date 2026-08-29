//! The Ferrite desktop application entry point. See `app` for the actual
//! UI and scan-control logic, `theme` for the visual style applied here.

mod app;
mod theme;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([900.0, 700.0])
            .with_min_inner_size([700.0, 500.0]),
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
            creation_context
                .egui_ctx
                .set_style_of(eframe::egui::Theme::Dark, theme::style());
            Ok(Box::new(app::FerriteApp::new()))
        }),
    )
}
