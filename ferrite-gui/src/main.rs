//! The Ferrite desktop application entry point. See `app` for the actual
//! UI and scan-control logic.

mod app;

fn main() -> eframe::Result {
    eframe::run_native(
        "Ferrite",
        eframe::NativeOptions::default(),
        Box::new(|_creation_context| Ok(Box::new(app::FerriteApp::new()))),
    )
}
