//! The Ferrite application: process picker, scan panel, and results table.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui;
use ferrite_core::{
    AobFilter, AobMatch, AttachError, DEFAULT_FREEZE_INTERVAL, FreezeHandle, ProcessInfo,
    ProcessSession, ScanFilter, ScanMatch, ScanOptions, ScanValue, first_scan_aob,
    first_scan_exact, format_pattern, next_scan, next_scan_aob, parse_hex_pattern,
};

/// How many result rows the table actually renders. Independent of
/// `ScanOptions::max_results` (the scan engine's own cap): even a scan
/// capped at a few thousand matches would still make egui build thousands of
/// rows every frame, which crawls. The table shows this many and reports the
/// true total separately.
const MAX_RENDERED_ROWS: usize = 500;

/// How often displayed rows are re-read from the target process. Throttled
/// rather than every frame - at 60fps, re-reading up to `MAX_RENDERED_ROWS`
/// rows every frame would mean tens of thousands of `ReadProcessMemory`
/// calls a second for no visible benefit at that refresh rate.
const LIVE_REFRESH_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ValueTypeChoice {
    I8,
    I16,
    I32,
    I64,
    F32,
    F64,
    Aob,
}

impl ValueTypeChoice {
    const ALL: [Self; 7] = [
        Self::I8,
        Self::I16,
        Self::I32,
        Self::I64,
        Self::F32,
        Self::F64,
        Self::Aob,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::I8 => "i8",
            Self::I16 => "i16",
            Self::I32 => "i32",
            Self::I64 => "i64",
            Self::F32 => "f32",
            Self::F64 => "f64",
            Self::Aob => "AOB (bytes)",
        }
    }

    /// Parses a numeric value. Never called for `Aob` — that variant goes
    /// through `parse_hex_pattern` instead, since a byte pattern isn't a
    /// `ScanValue`.
    fn parse(self, text: &str) -> Result<ScanValue, String> {
        let text = text.trim();
        let parsed = match self {
            Self::I8 => text.parse().ok().map(ScanValue::I8),
            Self::I16 => text.parse().ok().map(ScanValue::I16),
            Self::I32 => text.parse().ok().map(ScanValue::I32),
            Self::I64 => text.parse().ok().map(ScanValue::I64),
            Self::F32 => text.parse().ok().map(ScanValue::F32),
            Self::F64 => text.parse().ok().map(ScanValue::F64),
            Self::Aob => {
                unreachable!("Aob is parsed via parse_hex_pattern, not ValueTypeChoice::parse")
            }
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

/// A scan's results are one kind or the other, never both — an `Option` of
/// two parallel lists would let them disagree about which is current.
enum Results {
    Numeric(Vec<ScanMatch>),
    Aob(Vec<AobMatch>),
}

/// An active attach: the session plus everything scoped to it. Dropping this
/// (via Detach) closes the process handle, per `ProcessSession`'s RAII Drop -
/// and, per `FreezeHandle`'s Drop, stops and joins the freeze thread first.
struct Attached {
    session: Arc<ProcessSession>,
    freeze: FreezeHandle,
    process_name: String,
    pid: u32,
    results: Option<Results>,
    capped: bool,
    /// Addresses currently checked in the results table, for "Set Value" to
    /// act on. Keyed by address rather than table row, since rows move
    /// around under filtering/live-refresh but an address is stable.
    selected: HashSet<usize>,
    last_refresh: Instant,
}

pub struct FerriteApp {
    processes: Vec<ProcessInfo>,
    attached: Option<Attached>,
    attach_error: Option<String>,
    /// Set when a freeze thread detects its target process exited, and
    /// survives the resulting auto-detach so the user actually sees why the
    /// attach went away instead of it just silently disappearing.
    process_exited_message: Option<String>,
    value_type: ValueTypeChoice,
    input_text: String,
    input_error: Option<String>,
    filter: ScanFilter,
    aob_filter: AobFilter,
    edit_input_text: String,
    edit_input_error: Option<String>,
}

impl FerriteApp {
    pub fn new() -> Self {
        Self {
            processes: sorted_processes(),
            attached: None,
            attach_error: None,
            process_exited_message: None,
            value_type: ValueTypeChoice::I32,
            input_text: String::new(),
            input_error: None,
            filter: ScanFilter::Changed,
            aob_filter: AobFilter::Changed,
            edit_input_text: String::new(),
            edit_input_error: None,
        }
    }

    fn attach(&mut self, process: &ProcessInfo) {
        match ProcessSession::attach(process.pid) {
            Ok(session) => {
                let session = Arc::new(session);
                let freeze = session.start_freeze_thread(DEFAULT_FREEZE_INTERVAL);
                self.attached = Some(Attached {
                    session,
                    freeze,
                    process_name: process.name.clone(),
                    pid: process.pid,
                    results: None,
                    capped: false,
                    selected: HashSet::new(),
                    last_refresh: Instant::now(),
                });
                self.attach_error = None;
                self.process_exited_message = None;
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

    /// If the freeze thread has detected that the target process exited,
    /// surfaces that and force-detaches - a stale, unwritable session isn't
    /// useful to keep around, and its results/edit controls would silently
    /// do nothing if left attached.
    fn check_target_exited(&mut self) {
        let Some(attached) = &self.attached else {
            return;
        };
        if !attached.freeze.target_exited() {
            return;
        }
        self.process_exited_message = Some(format!(
            "{} (pid {}) exited while frozen values were active.",
            attached.process_name, attached.pid
        ));
        self.attached = None; // Drop closes the handle and joins the freeze thread.
    }

    /// Re-reads each displayed row's current bytes from the target process,
    /// throttled to `LIVE_REFRESH_INTERVAL`. Rows beyond `MAX_RENDERED_ROWS`
    /// aren't refreshed - they aren't drawn either, so there's nothing to
    /// keep current. A row whose address fails to read (e.g. the page was
    /// freed) is left showing its last known value rather than removed -
    /// unlike a scan filter, a live-refresh miss isn't a judgment that the
    /// row no longer matches.
    ///
    /// Also the general process-exit check, on the same throttle:
    /// `check_target_exited` only learns about an exit via the freeze
    /// thread, so it stays silent for a session with nothing frozen (an
    /// attach with no scan yet, or a scan with nothing checked). This method
    /// already touches the session on a timer regardless of freeze state,
    /// so it's the natural place to close that gap with one extra cheap
    /// call - see the "Known scope limit" note in the vault's `v1-plan.md`.
    fn refresh_live_values(&mut self) {
        let Some(attached) = &mut self.attached else {
            return;
        };
        if attached.last_refresh.elapsed() < LIVE_REFRESH_INTERVAL {
            return;
        }
        attached.last_refresh = Instant::now();

        if attached.session.has_exited() {
            let message = format!("{} (pid {}) exited.", attached.process_name, attached.pid);
            self.process_exited_message = Some(message);
            self.attached = None; // Drop closes the handle and joins the freeze thread.
            return;
        }

        let Attached {
            session, results, ..
        } = attached;
        let Some(results) = results else {
            return;
        };

        match results {
            Results::Numeric(matches) => {
                for m in matches.iter_mut().take(MAX_RENDERED_ROWS) {
                    if let Ok(bytes) = session.read_bytes(m.address, m.value.size()) {
                        m.value = m.value.from_le_bytes_like(&bytes);
                    }
                }
            }
            Results::Aob(matches) => {
                for m in matches.iter_mut().take(MAX_RENDERED_ROWS) {
                    if let Ok(bytes) = session.read_bytes(m.address, m.bytes.len()) {
                        m.bytes = bytes;
                    }
                }
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
        if let Some(msg) = &self.process_exited_message {
            ui.colored_label(egui::Color32::RED, msg);
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
            let previous_type = self.value_type;
            egui::ComboBox::new("value_type", "")
                .selected_text(self.value_type.label())
                .show_ui(ui, |ui| {
                    for choice in ValueTypeChoice::ALL {
                        ui.selectable_value(&mut self.value_type, choice, choice.label());
                    }
                });
            if self.value_type != previous_type {
                // Switching types mid-session invalidates the current
                // results (a filter for one kind is meaningless applied to
                // the other) - clear rather than let them go stale, and
                // drop selections along with them (frozen entries are left
                // alone - they're independent of what's currently shown).
                attached.results = None;
                attached.capped = false;
                attached.selected.clear();
            }

            let is_aob = self.value_type == ValueTypeChoice::Aob;
            ui.label(if is_aob { "Pattern:" } else { "Value:" });
            ui.text_edit_singleline(&mut self.input_text);

            if ui.button("First Scan").clicked() {
                if is_aob {
                    match parse_hex_pattern(&self.input_text) {
                        Ok(pattern) => {
                            let result =
                                first_scan_aob(&attached.session, &pattern, ScanOptions::default());
                            attached.capped = result.capped;
                            attached.results = Some(Results::Aob(result.matches));
                            self.input_error = None;
                        }
                        Err(err) => self.input_error = Some(err),
                    }
                } else {
                    match self.value_type.parse(&self.input_text) {
                        Ok(target) => {
                            let result =
                                first_scan_exact(&attached.session, target, ScanOptions::default());
                            attached.capped = result.capped;
                            attached.results = Some(Results::Numeric(result.matches));
                            self.input_error = None;
                        }
                        Err(err) => self.input_error = Some(err),
                    }
                }
            }

            if attached.results.is_some() && ui.button("New Scan").clicked() {
                attached.results = None;
                attached.capped = false;
                attached.selected.clear();
            }
        });

        if let Some(err) = &self.input_error {
            ui.colored_label(egui::Color32::RED, err);
        }

        let is_aob_results = matches!(attached.results, Some(Results::Aob(_)));
        if attached.results.is_some() {
            ui.horizontal(|ui| {
                ui.label("Next scan filter:");
                if is_aob_results {
                    egui::ComboBox::new("scan_filter", "")
                        .selected_text(aob_filter_label(self.aob_filter))
                        .show_ui(ui, |ui| {
                            for filter in [AobFilter::Changed, AobFilter::Unchanged] {
                                ui.selectable_value(
                                    &mut self.aob_filter,
                                    filter,
                                    aob_filter_label(filter),
                                );
                            }
                        });
                } else {
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
                }

                if ui.button("Next Scan").clicked() {
                    match &attached.results {
                        Some(Results::Numeric(matches)) => {
                            let updated = next_scan(&attached.session, matches, self.filter);
                            attached.results = Some(Results::Numeric(updated));
                        }
                        Some(Results::Aob(matches)) => {
                            let updated =
                                next_scan_aob(&attached.session, matches, self.aob_filter);
                            attached.results = Some(Results::Aob(updated));
                        }
                        None => {}
                    }
                    attached.capped = false; // next_scan only narrows, never re-caps
                }
            });
        }
    }

    fn show_results_table(&mut self, ui: &mut egui::Ui) {
        let Some(attached) = &mut self.attached else {
            return;
        };
        if attached.results.is_none() {
            return;
        }

        ui.separator();
        ui.horizontal(|ui| {
            ui.label("New value:");
            ui.text_edit_singleline(&mut self.edit_input_text);

            let selected_count = attached.selected.len();
            let set_clicked = ui
                .add_enabled(selected_count > 0, egui::Button::new("Set Value"))
                .clicked();
            if set_clicked {
                match parse_write_bytes(self.value_type, &self.edit_input_text) {
                    Ok(bytes) => {
                        for &address in &attached.selected {
                            // A single address failing to write (e.g. the
                            // page became unwritable) shouldn't stop the
                            // rest of the batch.
                            let _ = attached.session.write_bytes(address, &bytes);
                            // A frozen address' target must move too, or
                            // the freeze thread overwrites this write with
                            // the old pinned bytes on its very next tick -
                            // a silent revert the user has no way to see.
                            if attached.freeze.is_frozen(address) {
                                attached.freeze.freeze(address, bytes.clone());
                            }
                        }
                        self.edit_input_error = None;
                    }
                    Err(err) => self.edit_input_error = Some(err),
                }
            }
            ui.label(format!("({selected_count} selected)"));
        });
        if let Some(err) = &self.edit_input_error {
            ui.colored_label(egui::Color32::RED, err);
        }

        let Attached {
            freeze,
            results,
            selected,
            capped,
            ..
        } = attached;
        let Some(results) = results else {
            return;
        };

        match results {
            Results::Numeric(matches) => {
                let shown = render_result_summary(ui, matches.len(), *capped);
                egui::ScrollArea::vertical().show(ui, |ui| {
                    egui::Grid::new("results_table")
                        .striped(true)
                        .num_columns(4)
                        .show(ui, |ui| {
                            ui.strong("");
                            ui.strong("Frozen");
                            ui.strong("Address");
                            ui.strong("Value");
                            ui.end_row();

                            for m in matches.iter().take(shown) {
                                show_selection_checkbox(ui, selected, m.address);
                                show_frozen_checkbox(ui, freeze, m.address, || {
                                    m.value.to_le_bytes()
                                });
                                ui.label(format!("{:#x}", m.address));
                                ui.label(format_value(m.value));
                                ui.end_row();
                            }
                        });
                });
            }
            Results::Aob(matches) => {
                let shown = render_result_summary(ui, matches.len(), *capped);
                egui::ScrollArea::vertical().show(ui, |ui| {
                    egui::Grid::new("results_table")
                        .striped(true)
                        .num_columns(4)
                        .show(ui, |ui| {
                            ui.strong("");
                            ui.strong("Frozen");
                            ui.strong("Address");
                            ui.strong("Bytes");
                            ui.end_row();

                            for m in matches.iter().take(shown) {
                                show_selection_checkbox(ui, selected, m.address);
                                show_frozen_checkbox(ui, freeze, m.address, || m.bytes.clone());
                                ui.label(format!("{:#x}", m.address));
                                ui.label(format_pattern(&m.bytes));
                                ui.end_row();
                            }
                        });
                });
            }
        }
    }
}

/// Renders the leftmost "select this row" checkbox and keeps `selected` in
/// sync with it.
fn show_selection_checkbox(ui: &mut egui::Ui, selected: &mut HashSet<usize>, address: usize) {
    let mut is_selected = selected.contains(&address);
    if ui.checkbox(&mut is_selected, "").changed() {
        if is_selected {
            selected.insert(address);
        } else {
            selected.remove(&address);
        }
    }
}

/// Renders the "Frozen" checkbox and toggles the freeze thread's entry for
/// `address` to match. `current_bytes` is called only when the checkbox is
/// freshly checked - freezing pins whatever the row is currently showing.
fn show_frozen_checkbox(
    ui: &mut egui::Ui,
    freeze: &FreezeHandle,
    address: usize,
    current_bytes: impl FnOnce() -> Vec<u8>,
) {
    let mut is_frozen = freeze.is_frozen(address);
    if ui.checkbox(&mut is_frozen, "").changed() {
        if is_frozen {
            freeze.freeze(address, current_bytes());
        } else {
            freeze.unfreeze(address);
        }
    }
}

/// Parses the edit box's text into raw bytes to write: a hex pattern for
/// `Aob`, or a numeric value's little-endian bytes otherwise. Shared by
/// "Set Value" - `First Scan` has its own parse step because it also needs
/// the typed `ScanValue`/pattern, not just bytes.
fn parse_write_bytes(value_type: ValueTypeChoice, text: &str) -> Result<Vec<u8>, String> {
    if value_type == ValueTypeChoice::Aob {
        parse_hex_pattern(text)
    } else {
        value_type.parse(text).map(ScanValue::to_le_bytes)
    }
}

/// Renders the "N result(s) [capped / showing first M]" summary line shared
/// by both results kinds, and returns how many rows the caller should
/// actually render.
fn render_result_summary(ui: &mut egui::Ui, total: usize, capped: bool) -> usize {
    let shown = total.min(MAX_RENDERED_ROWS);
    let mut summary = format!("{total} result(s)");
    if capped {
        summary.push_str(" (scan stopped early — too many matches; narrow your search)");
    } else if total > shown {
        summary.push_str(&format!(" — showing first {shown}"));
    }
    ui.label(summary);
    shown
}

fn filter_label(filter: ScanFilter) -> &'static str {
    match filter {
        ScanFilter::Changed => "Changed",
        ScanFilter::Unchanged => "Unchanged",
        ScanFilter::Increased => "Increased",
        ScanFilter::Decreased => "Decreased",
    }
}

fn aob_filter_label(filter: AobFilter) -> &'static str {
    match filter {
        AobFilter::Changed => "Changed",
        AobFilter::Unchanged => "Unchanged",
    }
}

fn sorted_processes() -> Vec<ProcessInfo> {
    let mut processes = ferrite_core::list_processes();
    processes.sort_by_key(|p| p.pid);
    processes
}

impl eframe::App for FerriteApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.check_target_exited();
        self.refresh_live_values();
        if self.attached.is_some() {
            // eframe only repaints reactively (on input) by default - an
            // attached session needs a nudge to keep polling live values
            // even when the user isn't touching anything.
            ui.ctx().request_repaint_after(LIVE_REFRESH_INTERVAL);
        }

        egui::CentralPanel::default().show(ui, |ui| {
            self.show_process_picker(ui);
            self.show_scan_panel(ui);
            self.show_results_table(ui);
        });
    }
}
