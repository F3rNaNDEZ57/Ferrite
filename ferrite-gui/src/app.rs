//! The Ferrite application: process picker, scan panel, and results table.

use eframe::egui;
use ferrite_core::{
    AttachError, ProcessInfo, ProcessSession, ScanFilter, ScanMatch, ScanOptions, ScanValue,
    first_scan_exact, next_scan,
};

/// How many result rows the table actually renders. Independent of
/// `ScanOptions::max_results` (the scan engine's own cap): even a scan
/// capped at a few thousand matches would still make egui build thousands of
/// rows every frame, which crawls. The table shows this many and reports the
/// true total separately.
const MAX_RENDERED_ROWS: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ValueTypeChoice {
    I8,
    I16,
    I32,
    I64,
    F32,
    F64,
}

impl ValueTypeChoice {
    const ALL: [Self; 6] = [
        Self::I8,
        Self::I16,
        Self::I32,
        Self::I64,
        Self::F32,
        Self::F64,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::I8 => "i8",
            Self::I16 => "i16",
            Self::I32 => "i32",
            Self::I64 => "i64",
            Self::F32 => "f32",
            Self::F64 => "f64",
        }
    }

    fn parse(self, text: &str) -> Result<ScanValue, String> {
        let text = text.trim();
        let parsed = match self {
            Self::I8 => text.parse().ok().map(ScanValue::I8),
            Self::I16 => text.parse().ok().map(ScanValue::I16),
            Self::I32 => text.parse().ok().map(ScanValue::I32),
            Self::I64 => text.parse().ok().map(ScanValue::I64),
            Self::F32 => text.parse().ok().map(ScanValue::F32),
            Self::F64 => text.parse().ok().map(ScanValue::F64),
        };
        parsed.ok_or_else(|| format!("'{text}' isn't a valid {}", self.label()))
    }
}

fn format_value(value: ScanValue) -> String {
    match value {
        ScanValue::I8(v) => v.to_string(),
        ScanValue::I16(v) => v.to_string(),
        ScanValue::I32(v) => v.to_string(),
        ScanValue::I64(v) => v.to_string(),
        ScanValue::F32(v) => v.to_string(),
        ScanValue::F64(v) => v.to_string(),
    }
}

/// An active attach: the session plus everything scoped to it. Dropping this
/// (via Detach) closes the process handle, per `ProcessSession`'s RAII Drop.
struct Attached {
    session: ProcessSession,
    process_name: String,
    pid: u32,
    matches: Vec<ScanMatch>,
    has_scanned: bool,
    capped: bool,
}

pub struct FerriteApp {
    processes: Vec<ProcessInfo>,
    attached: Option<Attached>,
    attach_error: Option<String>,
    value_type: ValueTypeChoice,
    input_text: String,
    input_error: Option<String>,
    filter: ScanFilter,
}

impl FerriteApp {
    pub fn new() -> Self {
        Self {
            processes: sorted_processes(),
            attached: None,
            attach_error: None,
            value_type: ValueTypeChoice::I32,
            input_text: String::new(),
            input_error: None,
            filter: ScanFilter::Changed,
        }
    }

    fn attach(&mut self, process: &ProcessInfo) {
        match ProcessSession::attach(process.pid) {
            Ok(session) => {
                self.attached = Some(Attached {
                    session,
                    process_name: process.name.clone(),
                    pid: process.pid,
                    matches: Vec::new(),
                    has_scanned: false,
                    capped: false,
                });
                self.attach_error = None;
            }
            Err(err) => {
                self.attach_error = Some(match err {
                    AttachError::AccessDenied => {
                        "Access denied — try running Ferrite as Administrator.".to_string()
                    }
                    AttachError::ProcessNotFound => "That process no longer exists.".to_string(),
                    AttachError::Other(code) => format!("Couldn't attach (OS error {code})."),
                });
            }
        }
    }

    fn show_process_picker(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("Ferrite");
            if let Some(attached) = &self.attached {
                ui.label(format!(
                    "— attached to {} (pid {})",
                    attached.process_name, attached.pid
                ));
                if ui.button("Detach").clicked() {
                    self.attached = None; // Drop closes the handle.
                }
            }
        });

        if let Some(err) = &self.attach_error {
            ui.colored_label(egui::Color32::RED, err);
        }

        if self.attached.is_none() {
            ui.separator();
            if ui.button("Refresh process list").clicked() {
                self.processes = sorted_processes();
            }

            egui::ScrollArea::vertical()
                .max_height(300.0)
                .show(ui, |ui| {
                    egui::Grid::new("process_list")
                        .striped(true)
                        .num_columns(3)
                        .show(ui, |ui| {
                            ui.strong("PID");
                            ui.strong("Name");
                            ui.end_row();

                            // Clone the list up front: attaching inside the
                            // loop would otherwise borrow self.processes
                            // while also needing &mut self to attach.
                            for process in self.processes.clone() {
                                ui.label(process.pid.to_string());
                                ui.label(&process.name);
                                if ui.button("Attach").clicked() {
                                    self.attach(&process);
                                }
                                ui.end_row();
                            }
                        });
                });
        }
    }

    fn show_scan_panel(&mut self, ui: &mut egui::Ui) {
        let Some(attached) = &mut self.attached else {
            return;
        };

        ui.separator();
        ui.horizontal(|ui| {
            ui.label("Type:");
            egui::ComboBox::new("value_type", "")
                .selected_text(self.value_type.label())
                .show_ui(ui, |ui| {
                    for choice in ValueTypeChoice::ALL {
                        ui.selectable_value(&mut self.value_type, choice, choice.label());
                    }
                });

            ui.label("Value:");
            ui.text_edit_singleline(&mut self.input_text);

            if ui.button("First Scan").clicked() {
                match self.value_type.parse(&self.input_text) {
                    Ok(target) => {
                        let result =
                            first_scan_exact(&attached.session, target, ScanOptions::default());
                        attached.matches = result.matches;
                        attached.capped = result.capped;
                        attached.has_scanned = true;
                        self.input_error = None;
                    }
                    Err(err) => self.input_error = Some(err),
                }
            }

            if attached.has_scanned && ui.button("New Scan").clicked() {
                attached.matches.clear();
                attached.has_scanned = false;
                attached.capped = false;
            }
        });

        if let Some(err) = &self.input_error {
            ui.colored_label(egui::Color32::RED, err);
        }

        if attached.has_scanned {
            ui.horizontal(|ui| {
                ui.label("Next scan filter:");
                egui::ComboBox::new("scan_filter", "")
                    .selected_text(filter_label(self.filter))
                    .show_ui(ui, |ui| {
                        for filter in [
                            ScanFilter::Changed,
                            ScanFilter::Unchanged,
                            ScanFilter::Increased,
                            ScanFilter::Decreased,
                        ] {
                            ui.selectable_value(&mut self.filter, filter, filter_label(filter));
                        }
                    });

                if ui.button("Next Scan").clicked() {
                    attached.matches = next_scan(&attached.session, &attached.matches, self.filter);
                    attached.capped = false; // next_scan only narrows, never re-caps
                }
            });
        }
    }

    fn show_results_table(&self, ui: &mut egui::Ui) {
        let Some(attached) = &self.attached else {
            return;
        };
        if !attached.has_scanned {
            return;
        }

        ui.separator();
        let total = attached.matches.len();
        let shown = total.min(MAX_RENDERED_ROWS);
        let mut summary = format!("{total} result(s)");
        if attached.capped {
            summary.push_str(" (scan stopped early — too many matches; narrow your search)");
        } else if total > shown {
            summary.push_str(&format!(" — showing first {shown}"));
        }
        ui.label(summary);

        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::Grid::new("results_table")
                .striped(true)
                .num_columns(2)
                .show(ui, |ui| {
                    ui.strong("Address");
                    ui.strong("Value");
                    ui.end_row();

                    for m in attached.matches.iter().take(MAX_RENDERED_ROWS) {
                        ui.label(format!("{:#x}", m.address));
                        ui.label(format_value(m.value));
                        ui.end_row();
                    }
                });
        });
    }
}

fn filter_label(filter: ScanFilter) -> &'static str {
    match filter {
        ScanFilter::Changed => "Changed",
        ScanFilter::Unchanged => "Unchanged",
        ScanFilter::Increased => "Increased",
        ScanFilter::Decreased => "Decreased",
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
            self.show_process_picker(ui);
            self.show_scan_panel(ui);
            self.show_results_table(ui);
        });
    }
}
