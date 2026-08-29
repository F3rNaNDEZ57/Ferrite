//! The Ferrite desktop application. Pre-alpha: the process picker is real,
//! but the scan panel and results table (the actual product) land in the
//! next commit, once this window itself is proven to work.

use eframe::egui;
use ferrite_core::ProcessInfo;

struct FerriteApp {
    processes: Vec<ProcessInfo>,
}

impl FerriteApp {
    fn new() -> Self {
        Self {
            processes: sorted_processes(),
        }
    }
}

fn sorted_processes() -> Vec<ProcessInfo> {
    let mut processes = ferrite_core::list_processes();
    processes.sort_by_key(|p| p.pid);
    processes
}

impl eframe::App for FerriteApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ui, |ui| {
            ui.heading("Ferrite");
            ui.label("Pre-alpha — process picker only for now.");
            ui.separator();

            if ui.button("Refresh process list").clicked() {
                self.processes = sorted_processes();
            }

            egui::ScrollArea::vertical().show(ui, |ui| {
                egui::Grid::new("process_list")
                    .striped(true)
                    .num_columns(2)
                    .show(ui, |ui| {
                        ui.strong("PID");
                        ui.strong("Name");
                        ui.end_row();

                        for process in &self.processes {
                            ui.label(process.pid.to_string());
                            ui.label(&process.name);
                            ui.end_row();
                        }
                    });
            });
        });
    }
}

fn main() -> eframe::Result {
    eframe::run_native(
        "Ferrite",
        eframe::NativeOptions::default(),
        Box::new(|_creation_context| Ok(Box::new(FerriteApp::new()))),
    )
}
