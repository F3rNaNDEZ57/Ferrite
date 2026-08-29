//! The Ferrite application: process picker, scan panel, and results table.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui;
use ferrite_core::{
    AddressExpr, AobFilter, AobMatch, AttachError, CheatEntry, DEFAULT_FREEZE_INTERVAL, EntryValue,
    FreezeHandle, ProcessInfo, ProcessSession, ResolveError, ScanFilter, ScanMatch, ScanOptions,
    ScanValue, first_scan_aob, first_scan_exact, format_pattern, load_table, next_scan,
    next_scan_aob, parse_address_expr, parse_hex_pattern, parse_hex_usize, resolve_address,
    save_table,
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

/// Reinterprets freshly-read `bytes` as the same shape `previous` was -
/// mirrors `ScanValue::from_le_bytes_like`'s "keep the type, take the new
/// bits" contract for `EntryValue`'s extra `Bytes` variant too.
fn reinterpret_entry_value(previous: &EntryValue, bytes: &[u8]) -> EntryValue {
    match previous {
        EntryValue::Scalar(v) => EntryValue::Scalar(v.from_le_bytes_like(bytes)),
        EntryValue::Bytes(_) => EntryValue::Bytes(bytes.to_vec()),
    }
}

fn format_entry_value(value: &EntryValue) -> String {
    match value {
        EntryValue::Scalar(v) => format_value(*v),
        EntryValue::Bytes(bytes) => format_pattern(bytes),
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

/// A saved-list row's live resolution state — GUI-only display state, never
/// persisted (only the `CheatEntry` itself is saved/loaded). Recomputed each
/// throttled refresh tick so the saved list always shows *some* status per
/// entry, independent of whether a process is attached at all (see the
/// vault's `v1-plan.md`: loading while detached, or attached to the wrong
/// process, must never silently drop entries).
#[derive(Clone)]
enum RowStatus {
    NotAttached,
    Resolved,
    ModuleNotFound(String),
    Unreadable,
}

impl RowStatus {
    fn label(&self) -> String {
        match self {
            Self::NotAttached => "—".to_string(),
            Self::Resolved => String::new(),
            Self::ModuleNotFound(module) => format!("module {module:?} not found"),
            Self::Unreadable => "unreadable".to_string(),
        }
    }
}

/// One row in the saved-list panel: the persisted [`CheatEntry`] plus its
/// live, GUI-only resolution state.
struct SavedRow {
    entry: CheatEntry,
    resolved_address: Option<usize>,
    status: RowStatus,
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
    saved: Vec<SavedRow>,
    last_saved_refresh: Instant,
    manual_description: String,
    manual_address_text: String,
    manual_pointer_offset_text: String,
    manual_value_type: ValueTypeChoice,
    manual_value_text: String,
    manual_add_error: Option<String>,
    table_path_text: String,
    table_status: Option<String>,
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
            saved: Vec::new(),
            last_saved_refresh: Instant::now(),
            manual_description: String::new(),
            manual_address_text: String::new(),
            manual_pointer_offset_text: String::new(),
            manual_value_type: ValueTypeChoice::I32,
            manual_value_text: String::new(),
            manual_add_error: None,
            table_path_text: String::new(),
            table_status: None,
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
        self.mark_saved_entries_unattached();
    }

    /// Resets every saved-list row's live status to `NotAttached` — called
    /// whenever the active session goes away (Detach, exit detection),
    /// since a stale "Resolved" status must not linger once there's no
    /// session left to have resolved it against.
    fn mark_saved_entries_unattached(&mut self) {
        for row in &mut self.saved {
            row.resolved_address = None;
            row.status = RowStatus::NotAttached;
        }
    }

    /// Re-resolves every saved entry's address against the attached
    /// session, throttled like `refresh_live_values`. Runs independently of
    /// whether a scan has ever happened - the saved list is its own thing,
    /// not derived from scan results (see the vault's `v1-plan.md`).
    fn refresh_saved_entries(&mut self) {
        let Some(attached) = &self.attached else {
            return;
        };
        if self.last_saved_refresh.elapsed() < LIVE_REFRESH_INTERVAL {
            return;
        }
        self.last_saved_refresh = Instant::now();

        for row in &mut self.saved {
            let was_resolved = matches!(row.status, RowStatus::Resolved);
            match resolve_address(&row.entry, &attached.session) {
                Ok(address) => {
                    row.resolved_address = Some(address);

                    // Freezing pins to the *saved* value, not whatever's
                    // currently in memory - do this once, right on the
                    // transition into "resolved" (covers both "table loaded
                    // while detached, attach happens later" and "loaded
                    // while already attached"), before the live-read below
                    // would otherwise overwrite it. See the vault's
                    // `v1-plan.md`.
                    if !was_resolved && row.entry.frozen && !attached.freeze.is_frozen(address) {
                        attached
                            .freeze
                            .freeze(address, row.entry.value.to_le_bytes());
                    }

                    let len = row.entry.value.to_le_bytes().len();
                    match attached.session.read_bytes(address, len) {
                        Ok(bytes) => {
                            row.entry.value = reinterpret_entry_value(&row.entry.value, &bytes);
                            row.status = RowStatus::Resolved;
                        }
                        Err(_) => row.status = RowStatus::Unreadable,
                    }
                }
                Err(ResolveError::ModuleNotFound(module)) => {
                    row.resolved_address = None;
                    row.status = RowStatus::ModuleNotFound(module);
                }
                Err(ResolveError::Memory(_)) => {
                    row.resolved_address = None;
                    row.status = RowStatus::Unreadable;
                }
            }
        }
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
            self.mark_saved_entries_unattached();
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
                    self.mark_saved_entries_unattached();
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
                .id_salt("process_list_scroll")
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

        let mut to_promote: Option<SavedRow> = None;

        match results {
            Results::Numeric(matches) => {
                let shown = render_result_summary(ui, matches.len(), *capped);
                egui::ScrollArea::vertical()
                    .id_salt("results_table_scroll")
                    .show(ui, |ui| {
                        egui::Grid::new("results_table")
                            .striped(true)
                            .num_columns(5)
                            .show(ui, |ui| {
                                ui.strong("");
                                ui.strong("Frozen");
                                ui.strong("Address");
                                ui.strong("Value");
                                ui.strong("");
                                ui.end_row();

                                for m in matches.iter().take(shown) {
                                    show_selection_checkbox(ui, selected, m.address);
                                    show_frozen_checkbox(ui, freeze, m.address, || {
                                        m.value.to_le_bytes()
                                    });
                                    ui.label(format!("{:#x}", m.address));
                                    ui.label(format_value(m.value));
                                    if ui.button("Add to saved list").clicked() {
                                        to_promote = Some(SavedRow {
                                            entry: CheatEntry {
                                                description: format!(
                                                    "{} @ {:#x}",
                                                    format_value(m.value),
                                                    m.address
                                                ),
                                                base: AddressExpr::Absolute(m.address),
                                                pointer_offset: None,
                                                value: EntryValue::Scalar(m.value),
                                                frozen: false,
                                            },
                                            resolved_address: Some(m.address),
                                            status: RowStatus::Resolved,
                                        });
                                    }
                                    ui.end_row();
                                }
                            });
                    });
            }
            Results::Aob(matches) => {
                let shown = render_result_summary(ui, matches.len(), *capped);
                egui::ScrollArea::vertical()
                    .id_salt("results_table_scroll")
                    .show(ui, |ui| {
                        egui::Grid::new("results_table")
                            .striped(true)
                            .num_columns(5)
                            .show(ui, |ui| {
                                ui.strong("");
                                ui.strong("Frozen");
                                ui.strong("Address");
                                ui.strong("Bytes");
                                ui.strong("");
                                ui.end_row();

                                for m in matches.iter().take(shown) {
                                    show_selection_checkbox(ui, selected, m.address);
                                    show_frozen_checkbox(ui, freeze, m.address, || m.bytes.clone());
                                    ui.label(format!("{:#x}", m.address));
                                    ui.label(format_pattern(&m.bytes));
                                    if ui.button("Add to saved list").clicked() {
                                        to_promote = Some(SavedRow {
                                            entry: CheatEntry {
                                                description: format!(
                                                    "{} @ {:#x}",
                                                    format_pattern(&m.bytes),
                                                    m.address
                                                ),
                                                base: AddressExpr::Absolute(m.address),
                                                pointer_offset: None,
                                                value: EntryValue::Bytes(m.bytes.clone()),
                                                frozen: false,
                                            },
                                            resolved_address: Some(m.address),
                                            status: RowStatus::Resolved,
                                        });
                                    }
                                    ui.end_row();
                                }
                            });
                    });
            }
        }

        if let Some(row) = to_promote {
            self.saved.push(row);
        }
    }

    /// Manual "add address" form — works regardless of attach state (the
    /// row simply shows `NotAttached` until a matching process is attached
    /// and the next refresh tick resolves it). See the vault's
    /// `v1-plan.md`.
    fn show_manual_add_form(&mut self, ui: &mut egui::Ui) {
        ui.separator();
        ui.heading("Add address manually");
        ui.horizontal(|ui| {
            ui.label("Description:");
            ui.text_edit_singleline(&mut self.manual_description);
        });
        ui.horizontal(|ui| {
            ui.label("Address:");
            ui.text_edit_singleline(&mut self.manual_address_text);
            ui.label("Pointer offset (optional):");
            ui.text_edit_singleline(&mut self.manual_pointer_offset_text);
        });
        ui.horizontal(|ui| {
            ui.label("Type:");
            egui::ComboBox::new("manual_value_type", "")
                .selected_text(self.manual_value_type.label())
                .show_ui(ui, |ui| {
                    for choice in ValueTypeChoice::ALL {
                        ui.selectable_value(&mut self.manual_value_type, choice, choice.label());
                    }
                });
            ui.label("Value:");
            ui.text_edit_singleline(&mut self.manual_value_text);

            if ui.button("Add").clicked() {
                match self.build_manual_entry() {
                    Ok(entry) => {
                        self.saved.push(SavedRow {
                            entry,
                            resolved_address: None,
                            status: RowStatus::NotAttached,
                        });
                        self.manual_description.clear();
                        self.manual_address_text.clear();
                        self.manual_pointer_offset_text.clear();
                        self.manual_value_text.clear();
                        self.manual_add_error = None;
                    }
                    Err(err) => self.manual_add_error = Some(err),
                }
            }
        });
        if let Some(err) = &self.manual_add_error {
            ui.colored_label(egui::Color32::RED, err);
        }
    }

    fn build_manual_entry(&self) -> Result<CheatEntry, String> {
        let base = parse_address_expr(&self.manual_address_text)?;
        let pointer_offset = if self.manual_pointer_offset_text.trim().is_empty() {
            None
        } else {
            Some(
                parse_hex_usize(&self.manual_pointer_offset_text).ok_or_else(|| {
                    format!(
                        "'{}' isn't a valid hex offset",
                        self.manual_pointer_offset_text
                    )
                })?,
            )
        };
        let value = parse_entry_value(self.manual_value_type, &self.manual_value_text)?;
        let description = if self.manual_description.trim().is_empty() {
            self.manual_address_text.trim().to_string()
        } else {
            self.manual_description.trim().to_string()
        };

        Ok(CheatEntry {
            description,
            base,
            pointer_offset,
            value,
            frozen: false,
        })
    }

    /// Save/load controls: a plain path text field plus two buttons - no
    /// native file dialog for v1 (a decided simplification, see the vault's
    /// `v1-plan.md`).
    fn show_persistence_controls(&mut self, ui: &mut egui::Ui) {
        ui.separator();
        ui.horizontal(|ui| {
            ui.label("Table file:");
            ui.text_edit_singleline(&mut self.table_path_text);

            if ui.button("Save").clicked() {
                let entries: Vec<CheatEntry> = self.saved.iter().map(|r| r.entry.clone()).collect();
                match save_table(&PathBuf::from(self.table_path_text.trim()), &entries) {
                    Ok(()) => self.table_status = Some(format!("Saved {} entries.", entries.len())),
                    Err(err) => self.table_status = Some(format!("Save failed: {err}")),
                }
            }
            if ui.button("Load").clicked() {
                match load_table(&PathBuf::from(self.table_path_text.trim())) {
                    Ok(entries) => {
                        self.table_status = Some(format!("Loaded {} entries.", entries.len()));
                        self.saved = entries
                            .into_iter()
                            .map(|entry| SavedRow {
                                entry,
                                resolved_address: None,
                                status: RowStatus::NotAttached,
                            })
                            .collect();
                    }
                    Err(err) => self.table_status = Some(format!("Load failed: {err}")),
                }
            }
        });
        if let Some(status) = &self.table_status {
            ui.label(status);
        }
    }

    /// The saved-list panel: shows every saved entry with its live status,
    /// independent of scan results and of attach state (see the vault's
    /// `v1-plan.md`).
    fn show_saved_list_table(&mut self, ui: &mut egui::Ui) {
        if self.saved.is_empty() {
            return;
        }
        ui.separator();
        ui.heading("Saved list");

        let freeze_handle = self.attached.as_ref().map(|a| &a.freeze);
        let mut to_remove: Option<usize> = None;

        egui::ScrollArea::vertical()
            .id_salt("saved_list_scroll")
            .show(ui, |ui| {
                egui::Grid::new("saved_list")
                    .striped(true)
                    .num_columns(5)
                    .show(ui, |ui| {
                        ui.strong("Description");
                        ui.strong("Address");
                        ui.strong("Value");
                        ui.strong("Frozen");
                        ui.strong("");
                        ui.end_row();

                        for (index, row) in self.saved.iter_mut().enumerate() {
                            ui.text_edit_singleline(&mut row.entry.description);

                            let address_text = match (row.resolved_address, &row.status) {
                                (Some(address), RowStatus::Resolved) => format!("{address:#x}"),
                                (_, status) => status.label(),
                            };
                            ui.label(address_text);
                            ui.label(format_entry_value(&row.entry.value));

                            match freeze_handle {
                                Some(freeze) => show_saved_frozen_checkbox(ui, freeze, row),
                                None => {
                                    let mut is_frozen = row.entry.frozen;
                                    ui.add_enabled(false, egui::Checkbox::new(&mut is_frozen, ""));
                                }
                            }

                            if ui.button("Remove").clicked() {
                                to_remove = Some(index);
                            }
                            ui.end_row();
                        }
                    });
            });

        if let Some(index) = to_remove {
            let removed = self.saved.remove(index);
            if let (Some(freeze), Some(address)) = (
                self.attached.as_ref().map(|a| &a.freeze),
                removed.resolved_address,
            ) {
                freeze.unfreeze(address);
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

/// Renders a saved-list row's "Frozen" checkbox. Unlike
/// `show_frozen_checkbox` (results-table rows, which have no persisted
/// frozen flag of their own), this also keeps `row.entry.frozen` in sync so
/// the state round-trips through save/load. Disabled (read-only) when the
/// row hasn't resolved to a live address - there's no `FreezeHandle` entry
/// to toggle without one.
fn show_saved_frozen_checkbox(ui: &mut egui::Ui, freeze: &FreezeHandle, row: &mut SavedRow) {
    let Some(address) = row.resolved_address else {
        let mut is_frozen = row.entry.frozen;
        ui.add_enabled(false, egui::Checkbox::new(&mut is_frozen, ""));
        return;
    };
    let mut is_frozen = freeze.is_frozen(address);
    if ui.checkbox(&mut is_frozen, "").changed() {
        if is_frozen {
            freeze.freeze(address, row.entry.value.to_le_bytes());
        } else {
            freeze.unfreeze(address);
        }
        row.entry.frozen = is_frozen;
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

/// Parses a manual-add form's value text into an [`EntryValue`] of the
/// chosen type — the same split `parse_write_bytes` makes, but keeping the
/// typed `ScanValue`/byte-pattern rather than flattening straight to bytes,
/// since a saved entry needs to remember its own shape (for live-refresh
/// re-interpretation and for display).
fn parse_entry_value(value_type: ValueTypeChoice, text: &str) -> Result<EntryValue, String> {
    if value_type == ValueTypeChoice::Aob {
        parse_hex_pattern(text).map(EntryValue::Bytes)
    } else {
        value_type.parse(text).map(EntryValue::Scalar)
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
        self.refresh_saved_entries();
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
            self.show_manual_add_form(ui);
            self.show_persistence_controls(ui);
            self.show_saved_list_table(ui);
        });
    }
}
