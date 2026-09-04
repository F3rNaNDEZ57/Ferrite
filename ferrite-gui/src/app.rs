//! The Ferrite application: process picker, scan panel, and results table.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui;

use crate::theme;
use ferrite_core::{
    AddressExpr, AobFilter, AobMatch, AttachError, CheatEntry, DEFAULT_FREEZE_INTERVAL, EntryValue,
    FreezeHandle, ImportReport, ModuleMap, ProcessInfo, ProcessSession, ResolveError, ScanFilter,
    ScanMatch, ScanOptions, ScanValue, ScriptKind, TextEncoding, decode_text, encode_text,
    extract_icon_rgba, first_scan_aob, first_scan_exact, format_pattern, import_ct_file,
    load_table, next_scan, next_scan_aob, parse_address_expr, parse_hex_pattern,
    parse_pointer_offsets, resolve_address, save_table,
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
    Text,
    UnicodeText,
}

impl ValueTypeChoice {
    const ALL: [Self; 9] = [
        Self::I8,
        Self::I16,
        Self::I32,
        Self::I64,
        Self::F32,
        Self::F64,
        Self::Aob,
        Self::Text,
        Self::UnicodeText,
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
            // Cheat Engine's own names for these two, so a user who knows
            // where a value came from recognizes the type here.
            Self::Text => "String",
            Self::UnicodeText => "Unicode String",
        }
    }

    /// The text encoding this type scans and displays in, or `None` for the
    /// types that aren't text.
    fn text_encoding(self) -> Option<TextEncoding> {
        match self {
            Self::Text => Some(TextEncoding::Latin1),
            Self::UnicodeText => Some(TextEncoding::Utf16Le),
            _ => None,
        }
    }

    /// Whether this type searches for a run of bytes rather than a
    /// fixed-width number — true for `Aob` and for both string types, since
    /// a string scan *is* a byte-pattern search (the encoded text is the
    /// pattern). That's why both route through the same scan engine rather
    /// than a second one.
    fn scans_as_bytes(self) -> bool {
        self == Self::Aob || self.text_encoding().is_some()
    }

    /// Turns the search box's text into the bytes to look for.
    fn parse_pattern(self, text: &str) -> Result<Vec<u8>, String> {
        match self.text_encoding() {
            Some(encoding) => encode_text(text, encoding),
            None => parse_hex_pattern(text),
        }
    }

    /// Renders matched bytes for the results table. A scan match is exactly
    /// as long as the pattern searched for, so there's no NUL padding to
    /// truncate here — unlike a saved entry, which has a declared buffer
    /// width and carries its own zero-terminate flag.
    fn format_bytes(self, bytes: &[u8]) -> String {
        match self.text_encoding() {
            Some(encoding) => format!("{:?}", decode_text(bytes, encoding, false)),
            None => format_pattern(bytes),
        }
    }

    /// The saved-list value a result of this type promotes to. A promoted
    /// string's buffer is the match itself, so `zero_terminated` starts
    /// `false`: there's nothing past the text to truncate at, and setting it
    /// would instead hide any NUL that later appears inside the buffer.
    fn entry_value_from_bytes(self, bytes: Vec<u8>) -> EntryValue {
        match self.text_encoding() {
            Some(encoding) => EntryValue::Text {
                bytes,
                encoding,
                zero_terminated: false,
            },
            None => EntryValue::Bytes(bytes),
        }
    }

    /// The helper line under the value field: what this type accepts. It
    /// occupies the message slot whenever there is no error to show, so the
    /// slot is never empty space that only fills up when something breaks.
    fn hint(self) -> &'static str {
        match self {
            Self::I8 => "Decimal. Signed 8-bit.",
            Self::I16 => "Decimal. Signed 16-bit.",
            Self::I32 => "Decimal. Signed 32-bit.",
            Self::I64 => "Decimal. Signed 64-bit.",
            Self::F32 => "Decimal, with a fractional part. 32-bit float.",
            Self::F64 => "Decimal, with a fractional part. 64-bit float.",
            Self::Aob => "Hex bytes, whitespace optional.",
            Self::Text => "Text, one byte per character.",
            Self::UnicodeText => "Text, two bytes per character (UTF-16).",
        }
    }

    /// Parses a numeric value. Never called for the byte-pattern types —
    /// those go through [`Self::parse_pattern`] instead, since neither a hex
    /// pattern nor a string is a `ScanValue`.
    fn parse(self, text: &str) -> Result<ScanValue, String> {
        let text = text.trim();
        let parsed = match self {
            Self::I8 => text.parse().ok().map(ScanValue::I8),
            Self::I16 => text.parse().ok().map(ScanValue::I16),
            Self::I32 => text.parse().ok().map(ScanValue::I32),
            Self::I64 => text.parse().ok().map(ScanValue::I64),
            Self::F32 => text.parse().ok().map(ScanValue::F32),
            Self::F64 => text.parse().ok().map(ScanValue::F64),
            Self::Aob | Self::Text | Self::UnicodeText => unreachable!(
                "{} is parsed via parse_pattern, not ValueTypeChoice::parse",
                self.label()
            ),
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
        // Encoding and zero-terminate are properties of the *entry*, not of
        // whatever bytes happen to be at the address this tick - only the
        // buffer contents are new. The buffer length comes along unchanged
        // too, since the caller reads `to_le_bytes().len()` bytes.
        EntryValue::Text {
            encoding,
            zero_terminated,
            ..
        } => EntryValue::Text {
            bytes: bytes.to_vec(),
            encoding: *encoding,
            zero_terminated: *zero_terminated,
        },
    }
}

/// Renders a saved entry's value for its row. `show_as_hex` only affects
/// numeric values - a byte pattern is already hex, and text has no
/// meaningful hex rendering that a user would want in place of the text.
fn format_entry_value(value: &EntryValue, show_as_hex: bool) -> String {
    match value {
        EntryValue::Scalar(v) if show_as_hex => format_scalar_hex(*v),
        EntryValue::Scalar(v) => format_value(*v),
        EntryValue::Bytes(bytes) => format_pattern(bytes),
        // Quoted, so trailing NUL padding on a non-zero-terminated buffer
        // is visible as width rather than looking like a shorter string.
        EntryValue::Text {
            bytes,
            encoding,
            zero_terminated,
        } => format!("{:?}", decode_text(bytes, *encoding, *zero_terminated)),
    }
}

/// Renders a numeric value's raw little-endian bytes as a hex integer of
/// that type's width. Floats included: Cheat Engine treats a hex-displayed
/// value as an integer "even for the float types", so an `f32` shows its
/// bit pattern rather than a decimal that hex couldn't represent anyway.
fn format_scalar_hex(value: ScanValue) -> String {
    let bytes = value.to_le_bytes();
    let mut widened = [0u8; 8];
    widened[..bytes.len()].copy_from_slice(&bytes);
    format!("{:#X}", u64::from_le_bytes(widened))
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
/// vault's `v0.1-plan.md`: loading while detached, or attached to the wrong
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
            Self::NotAttached => "not attached".to_string(),
            Self::Resolved => "resolved".to_string(),
            Self::ModuleNotFound(module) => format!("module {module:?} not found"),
            Self::Unreadable => "unreadable · page freed or protected".to_string(),
        }
    }

    fn is_resolved(&self) -> bool {
        matches!(self, Self::Resolved)
    }

    /// Row state is carried by ink weight and a left rule, never by hue —
    /// red means attention here, and an entry that simply isn't resolved yet
    /// is not something to alarm anyone about.
    fn ink(&self) -> egui::Color32 {
        if self.is_resolved() {
            theme::TEXT
        } else {
            theme::TEXT_FAINT
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
    manual_pointer_offsets_text: String,
    manual_value_type: ValueTypeChoice,
    manual_value_text: String,
    manual_add_error: Option<String>,
    table_path_text: String,
    table_status: Option<String>,
    import_report: Option<ImportReport>,
    /// Icon textures keyed by exe path, `None` cached for a path that
    /// failed extraction (no icon, or a pseudo-process with no exe at
    /// all) so a bad path isn't retried every frame. Extraction is lazy
    /// (first time a path is drawn) and never cleared - an exe's icon
    /// doesn't change mid-session.
    icon_cache: HashMap<PathBuf, Option<egui::TextureHandle>>,
    /// Free-text filter over the process list — name, PID and path.
    process_filter: String,
    /// Hide targets Ferrite can't attach to, on by default: an unattachable
    /// row is noise until you're looking for it.
    hide_32_bit: bool,
    /// A snapshot of the target's modules, for showing a result's address as
    /// `module+offset`. Rebuilt on attach and on each first scan, never per
    /// row — see [`ferrite_core::ModuleMap`].
    module_map: ModuleMap,
    /// Committed writable bytes and region count in the attached target,
    /// shown in the rail so a scan's scope is visible before it runs.
    scan_region_summary: Option<(u64, usize)>,
    /// Match counts, one per scan in the current chain: `18402 → 412 → 6`.
    scan_history: Vec<usize>,
    /// Whether the manual-add modal is open.
    manual_add_open: bool,
    /// Index into the import report's skipped list whose script is being
    /// read, if any. The list keeps its scroll position while you read one
    /// script after another, which is what a person actually does with a
    /// downloaded table.
    selected_skip: Option<usize>,
    /// Whether the script pane soft-wraps. Off by default: assembly and Lua
    /// are line-oriented, and wrapping makes a long line look like two.
    script_wrap: bool,
    /// When each address last changed, for the flash-on-change decay. Keyed
    /// by address, cleared with the results it belongs to.
    changed_at: HashMap<usize, f64>,
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
            manual_pointer_offsets_text: String::new(),
            manual_value_type: ValueTypeChoice::I32,
            manual_value_text: String::new(),
            manual_add_error: None,
            table_path_text: String::new(),
            table_status: None,
            import_report: None,
            icon_cache: HashMap::new(),
            process_filter: String::new(),
            hide_32_bit: true,
            module_map: ModuleMap::empty(),
            scan_region_summary: None,
            scan_history: Vec::new(),
            manual_add_open: false,
            selected_skip: None,
            script_wrap: false,
            changed_at: HashMap::new(),
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

                // Both are snapshots taken once per attach rather than per
                // frame: the module map answers a per-row question on a
                // 100 ms tick, and the region summary walks the whole
                // address space.
                if let Some(attached) = &self.attached {
                    self.module_map =
                        ModuleMap::build(&attached.session).unwrap_or_else(|_| ModuleMap::empty());
                    let regions = attached.session.writable_regions();
                    self.scan_region_summary =
                        Some((regions.iter().map(|r| r.size as u64).sum(), regions.len()));
                }
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
    /// not derived from scan results (see the vault's `v0.1-plan.md`).
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
                    // `v0.1-plan.md`.
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
    /// call - see the "Known scope limit" note in the vault's `v0.1-plan.md`.
    fn refresh_live_values(&mut self, now: f64) {
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
        let changed_at = &mut self.changed_at;

        match results {
            Results::Numeric(matches) => {
                for m in matches.iter_mut().take(MAX_RENDERED_ROWS) {
                    if let Ok(bytes) = session.read_bytes(m.address, m.value.size()) {
                        let fresh = m.value.from_le_bytes_like(&bytes);
                        // Note *when* it changed, not that it did: the row
                        // flashes and decays from this timestamp.
                        if fresh != m.value {
                            changed_at.insert(m.address, now);
                        }
                        m.value = fresh;
                    }
                }
            }
            Results::Aob(matches) => {
                for m in matches.iter_mut().take(MAX_RENDERED_ROWS) {
                    if let Ok(bytes) = session.read_bytes(m.address, m.bytes.len()) {
                        if bytes != m.bytes {
                            changed_at.insert(m.address, now);
                        }
                        m.bytes = bytes;
                    }
                }
            }
        }
    }

    /// Looks up (extracting and caching on first use) the icon texture for
    /// an exe path. `None` covers both "no exe path at all" (pseudo-
    /// processes) and "extraction failed" - either way, the caller just
    /// reserves blank space instead of an image.
    fn icon_texture(&mut self, ctx: &egui::Context, path: &Path) -> Option<egui::TextureHandle> {
        self.icon_cache
            .entry(path.to_path_buf())
            .or_insert_with(|| {
                let icon = extract_icon_rgba(path)?;
                let image = egui::ColorImage::from_rgba_unmultiplied(
                    [icon.width as usize, icon.height as usize],
                    &icon.rgba,
                );
                Some(ctx.load_texture(
                    path.display().to_string(),
                    image,
                    egui::TextureOptions::default(),
                ))
            })
            .clone()
    }

    /// The process picker, which is the whole central region until
    /// something is attached.
    ///
    /// A filter and an architecture column, because the two things that make
    /// this list hard are that it is several hundred rows long and that a
    /// third of it can't be attached to at all.
    fn show_process_picker(&mut self, ui: &mut egui::Ui) {
        if let Some(err) = &self.attach_error {
            ui.label(
                egui::RichText::new(err)
                    .font(theme::font(theme::text_style::SECONDARY))
                    .color(theme::ACCENT_LIFT),
            );
            ui.add_space(theme::space::SM);
        }
        if let Some(msg) = &self.process_exited_message {
            ui.label(
                egui::RichText::new(msg)
                    .font(theme::font(theme::text_style::SECONDARY))
                    .color(theme::ACCENT_LIFT),
            );
            ui.add_space(theme::space::SM);
        }

        ui.horizontal(|ui| {
            ui.add_sized(
                [260.0, theme::FIELD_HEIGHT],
                egui::TextEdit::singleline(&mut self.process_filter)
                    .hint_text("Filter by name, PID or path"),
            );
            ui.checkbox(&mut self.hide_32_bit, "Hide 32-bit");
            if ui.button("Refresh").clicked() {
                self.processes = sorted_processes();
            }
        });
        ui.add_space(theme::space::SM);

        let needle = self.process_filter.trim().to_lowercase();
        let visible: Vec<ProcessInfo> = self
            .processes
            .iter()
            .filter(|p| {
                if self.hide_32_bit && !p.arch.is_attachable() {
                    return false;
                }
                if needle.is_empty() {
                    return true;
                }
                p.name.to_lowercase().contains(&needle)
                    || p.pid.to_string().contains(&needle)
                    || p.exe
                        .as_ref()
                        .is_some_and(|e| e.to_string_lossy().to_lowercase().contains(&needle))
            })
            .cloned()
            .collect();

        let hidden_32 = self
            .processes
            .iter()
            .filter(|p| !p.arch.is_attachable())
            .count();
        let mut summary = format!("{} of {} processes", visible.len(), self.processes.len());
        if !needle.is_empty() {
            summary.push_str(&format!(" matching \"{}\"", self.process_filter.trim()));
        }
        if self.hide_32_bit && hidden_32 > 0 {
            summary.push_str(&format!(" · {hidden_32} hidden as 32-bit"));
        }
        ui.label(
            egui::RichText::new(summary)
                .font(theme::font(theme::text_style::SECONDARY))
                .color(theme::TEXT_DIM),
        );
        ui.add_space(theme::space::SM);

        let mut to_attach: Option<ProcessInfo> = None;
        let ctx = ui.ctx().clone();
        let mut icons: Vec<Option<egui::TextureHandle>> = Vec::with_capacity(visible.len());
        for process in &visible {
            icons.push(
                process
                    .exe
                    .as_deref()
                    .and_then(|path| self.icon_texture(&ctx, path)),
            );
        }

        egui_extras::TableBuilder::new(ui)
            .id_salt("process_table")
            .striped(true)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(egui_extras::Column::exact(24.0))
            .column(egui_extras::Column::exact(72.0))
            .column(egui_extras::Column::exact(220.0))
            .column(egui_extras::Column::remainder())
            .column(egui_extras::Column::exact(56.0))
            .column(egui_extras::Column::exact(96.0))
            .header(theme::HEADER_HEIGHT, |mut header| {
                header.col(|ui| {
                    theme::column_header(ui, "");
                });
                header.col(|ui| {
                    theme::column_header(ui, "pid");
                });
                header.col(|ui| {
                    theme::column_header(ui, "name");
                });
                header.col(|ui| {
                    theme::column_header(ui, "path");
                });
                header.col(|ui| {
                    theme::column_header(ui, "arch");
                });
                header.col(|ui| {
                    theme::column_header(ui, "");
                });
            })
            .body(|body| {
                body.rows(theme::ROW_HEIGHT, visible.len(), |mut row| {
                    let index = row.index();
                    let process = &visible[index];
                    let attachable = process.arch.is_attachable();

                    row.col(|ui| {
                        match &icons[index] {
                            Some(texture) => {
                                ui.add(
                                    egui::Image::new(texture)
                                        .fit_to_exact_size(egui::vec2(16.0, 16.0)),
                                );
                            }
                            None => {
                                ui.allocate_space(egui::vec2(16.0, 16.0));
                            }
                        };
                    });
                    row.col(|ui| {
                        ui.label(
                            egui::RichText::new(process.pid.to_string())
                                .font(theme::font(theme::text_style::MONO_VALUE))
                                .color(theme::TEXT_DIM),
                        );
                    });
                    row.col(|ui| {
                        ui.add(
                            egui::Label::new(egui::RichText::new(&process.name).color(
                                if attachable {
                                    theme::TEXT
                                } else {
                                    theme::TEXT_FAINT
                                },
                            ))
                            .truncate(),
                        );
                    });
                    row.col(|ui| {
                        let path = process
                            .exe
                            .as_ref()
                            .map(|e| e.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(&path)
                                    .font(theme::font(theme::text_style::SECONDARY))
                                    .color(theme::TEXT_FAINT),
                            )
                            .truncate(),
                        )
                        .on_hover_text(path);
                    });
                    row.col(|ui| {
                        ui.label(
                            egui::RichText::new(process.arch.label())
                                .font(theme::font(theme::text_style::MONO_VALUE))
                                .color(if attachable {
                                    theme::TEXT_DIM
                                } else {
                                    theme::ACCENT_LIFT
                                }),
                        );
                    });
                    row.col(|ui| {
                        if attachable {
                            if ui.button("Attach").clicked() {
                                to_attach = Some(process.clone());
                            }
                        } else {
                            // Named rather than merely disabled: the reason
                            // is the useful part, and it is a property of
                            // Ferrite, not of the process.
                            ui.label(
                                egui::RichText::new("64-bit only")
                                    .font(theme::font(theme::text_style::SECONDARY))
                                    .color(theme::TEXT_FAINT),
                            );
                        }
                    });
                });
            });

        if let Some(process) = to_attach {
            self.attach(&process);
        }
    }

    /// The SCAN group: what to look for, and the button that starts it.
    fn show_scan_group(&mut self, ui: &mut egui::Ui) {
        let attached = self.attached.is_some();
        ui.horizontal(|ui| {
            theme::section_label(ui, "scan");
            if !attached {
                ui.label(
                    egui::RichText::new("available once attached")
                        .font(theme::font(theme::text_style::SECONDARY))
                        .color(theme::TEXT_FAINT),
                );
            }
        });
        ui.add_space(theme::space::SM);

        // Rendered inactive rather than hidden while detached: the controls
        // are the explanation of what this program does.
        ui.add_enabled_ui(attached, |ui| {
            ui.label(
                egui::RichText::new("Value type")
                    .font(theme::font(theme::text_style::SECONDARY))
                    .color(theme::TEXT_DIM),
            );
            let previous_type = self.value_type;
            egui::ComboBox::new("value_type", "")
                .selected_text(self.value_type.label())
                .width(ui.available_width())
                .show_ui(ui, |ui| {
                    for choice in ValueTypeChoice::ALL {
                        ui.selectable_value(&mut self.value_type, choice, choice.label());
                    }
                });
            if self.value_type != previous_type
                && let Some(attached) = &mut self.attached
            {
                // A filter for one kind of result is meaningless applied to
                // the other, so switching type clears them rather than
                // leaving them to be misread.
                attached.results = None;
                attached.capped = false;
                attached.selected.clear();
                self.scan_history.clear();
                self.changed_at.clear();
                self.input_error = None;
            }

            ui.add_space(theme::space::SM);
            ui.label(
                egui::RichText::new(match self.value_type.text_encoding() {
                    Some(_) => "Text",
                    None if self.value_type.scans_as_bytes() => "Pattern",
                    None => "Value",
                })
                .font(theme::font(theme::text_style::SECONDARY))
                .color(theme::TEXT_DIM),
            );
            ui.add_sized(
                [ui.available_width(), theme::FIELD_HEIGHT],
                egui::TextEdit::singleline(&mut self.input_text)
                    .font(egui::TextStyle::Name(theme::text_style::MONO_VALUE.into())),
            );
            // Validated as you type, not on submit. First Scan is disabled
            // while the field doesn't parse, so a submit-time error would
            // never be reachable — the slot has to carry the reason instead.
            let live_error = self.scan_input_error();
            message_slot(
                ui,
                live_error.as_deref().or(self.input_error.as_deref()),
                self.value_type.hint(),
            );

            ui.horizontal(|ui| {
                // Inactive until the field parses, so an invalid scan is
                // impossible rather than merely reported afterwards.
                let parses = self.scan_input_parses();
                if ui
                    .add_enabled_ui(parses, |ui| theme::primary(ui, "First Scan"))
                    .inner
                    .clicked()
                {
                    self.run_first_scan();
                }
                if self.attached.as_ref().is_some_and(|a| a.results.is_some())
                    && ui.button("New Scan").clicked()
                {
                    if let Some(attached) = &mut self.attached {
                        attached.results = None;
                        attached.capped = false;
                        attached.selected.clear();
                    }
                    self.scan_history.clear();
                    self.changed_at.clear();
                }
            });
        });
    }

    /// Why the scan field's contents can't be scanned, or `None` if they
    /// can. An empty field is not an error — it is the starting state, and
    /// shouting at someone who hasn't typed anything yet is noise.
    fn scan_input_error(&self) -> Option<String> {
        if self.input_text.trim().is_empty() {
            return None;
        }
        if self.value_type.scans_as_bytes() {
            match self.value_type.parse_pattern(&self.input_text) {
                Ok(p) if p.is_empty() => Some("Enter at least one byte.".to_string()),
                Ok(_) => None,
                Err(err) => Some(err),
            }
        } else {
            self.value_type.parse(&self.input_text).err()
        }
    }

    /// Whether the scan field currently holds something scannable. Drives the
    /// disabled state of First Scan, and is the same parse the scan itself
    /// performs.
    fn scan_input_parses(&self) -> bool {
        if self.value_type.scans_as_bytes() {
            self.value_type
                .parse_pattern(&self.input_text)
                .is_ok_and(|p| !p.is_empty())
        } else {
            self.value_type.parse(&self.input_text).is_ok()
        }
    }

    /// Runs a first scan with whatever the field holds.
    fn run_first_scan(&mut self) {
        let Some(attached) = &mut self.attached else {
            return;
        };
        if self.value_type.scans_as_bytes() {
            match self.value_type.parse_pattern(&self.input_text) {
                Ok(pattern) => {
                    let result =
                        first_scan_aob(&attached.session, &pattern, ScanOptions::default());
                    attached.capped = result.capped;
                    self.scan_history = vec![result.matches.len()];
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
                    self.scan_history = vec![result.matches.len()];
                    attached.results = Some(Results::Numeric(result.matches));
                    self.input_error = None;
                }
                Err(err) => self.input_error = Some(err),
            }
        }
        self.changed_at.clear();
        // The module map is a snapshot; a fresh scan is the natural moment to
        // retake it, since that is when new addresses appear.
        if let Some(attached) = &self.attached {
            self.module_map =
                ModuleMap::build(&attached.session).unwrap_or_else(|_| ModuleMap::empty());
        }
    }

    /// The RESCAN group. Absent rather than greyed while there are no
    /// results: it has nothing to filter.
    fn show_rescan_group(&mut self, ui: &mut egui::Ui) {
        theme::section_label(ui, "rescan");
        ui.add_space(theme::space::SM);

        let is_aob = matches!(
            self.attached.as_ref().and_then(|a| a.results.as_ref()),
            Some(Results::Aob(_))
        );
        ui.horizontal_wrapped(|ui| {
            if is_aob {
                for filter in [AobFilter::Changed, AobFilter::Unchanged] {
                    ui.selectable_value(&mut self.aob_filter, filter, aob_filter_label(filter));
                }
            } else {
                for filter in [
                    ScanFilter::Changed,
                    ScanFilter::Unchanged,
                    ScanFilter::Increased,
                    ScanFilter::Decreased,
                ] {
                    ui.selectable_value(&mut self.filter, filter, filter_label(filter));
                }
            }
        });
        ui.add_space(theme::space::SM);

        if theme::primary(ui, "Next Scan").clicked() {
            let (filter, aob_filter) = (self.filter, self.aob_filter);
            if let Some(attached) = &mut self.attached {
                match &attached.results {
                    Some(Results::Numeric(matches)) => {
                        let updated = next_scan(&attached.session, matches, filter);
                        self.scan_history.push(updated.len());
                        attached.results = Some(Results::Numeric(updated));
                    }
                    Some(Results::Aob(matches)) => {
                        let updated = next_scan_aob(&attached.session, matches, aob_filter);
                        self.scan_history.push(updated.len());
                        attached.results = Some(Results::Aob(updated));
                    }
                    None => {}
                }
                attached.capped = false; // next_scan only narrows, never re-caps
            }
        }

        if self.scan_history.len() > 1 {
            ui.add_space(theme::space::SM);
            let chain = self
                .scan_history
                .iter()
                .map(|n| n.to_string())
                .collect::<Vec<_>>()
                .join(" → ");
            ui.label(
                egui::RichText::new(chain)
                    .font(theme::font(theme::text_style::MONO_VALUE))
                    .color(theme::TEXT_DIM),
            );
        }
    }

    /// The WRITE group: what to set the selected addresses to.
    fn show_write_group(&mut self, ui: &mut egui::Ui) {
        let selected = self
            .attached
            .as_ref()
            .map(|a| a.selected.len())
            .unwrap_or(0);
        ui.horizontal(|ui| {
            theme::section_label(ui, "write");
            ui.label(
                egui::RichText::new(format!("— {selected} selected"))
                    .font(theme::font(theme::text_style::SECONDARY))
                    .color(theme::TEXT_DIM),
            );
        });
        ui.add_space(theme::space::SM);

        ui.add_enabled_ui(selected > 0, |ui| {
            ui.add_sized(
                [ui.available_width(), theme::FIELD_HEIGHT],
                egui::TextEdit::singleline(&mut self.edit_input_text)
                    .font(egui::TextStyle::Name(theme::text_style::MONO_VALUE.into())),
            );
            message_slot(ui, self.edit_input_error.as_deref(), "");
            ui.horizontal(|ui| {
                if theme::primary(ui, "Set value").clicked() {
                    self.write_selected();
                }
                if ui.button("Freeze selected").clicked() {
                    self.freeze_selected();
                }
            });
        });
    }

    /// Writes the edit field's value to every selected address.
    fn write_selected(&mut self) {
        match parse_write_bytes(self.value_type, &self.edit_input_text) {
            Ok(bytes) => {
                if let Some(attached) = &mut self.attached {
                    for &address in &attached.selected {
                        // One address failing to write shouldn't stop the
                        // rest of the batch.
                        let _ = attached.session.write_bytes(address, &bytes);
                        // A frozen address's pin has to move too, or the
                        // freeze thread reverts this write on its next tick.
                        if attached.freeze.is_frozen(address) {
                            attached.freeze.freeze(address, bytes.clone());
                        }
                    }
                }
                self.edit_input_error = None;
            }
            Err(err) => self.edit_input_error = Some(err),
        }
    }

    /// Freezes every selected address at whatever it currently holds.
    fn freeze_selected(&mut self) {
        let Some(attached) = &mut self.attached else {
            return;
        };
        let addresses: Vec<usize> = attached.selected.iter().copied().collect();
        for address in addresses {
            let size = match &attached.results {
                Some(Results::Numeric(m)) => m
                    .iter()
                    .find(|m| m.address == address)
                    .map(|m| m.value.size()),
                Some(Results::Aob(m)) => m
                    .iter()
                    .find(|m| m.address == address)
                    .map(|m| m.bytes.len()),
                None => None,
            };
            if let Some(size) = size
                && let Ok(bytes) = attached.session.read_bytes(address, size)
            {
                attached.freeze.freeze(address, bytes);
            }
        }
    }

    /// The central results region: a summary line, then one virtualised
    /// table.
    ///
    /// Every visible row re-reads target memory on a 100 ms tick, and a scan
    /// can return tens of thousands of addresses, so only the rows actually
    /// on screen are built at all — `TableBuilder::rows` gives the callback a
    /// row index and nothing else is touched.
    fn show_results_table(&mut self, ui: &mut egui::Ui, window_width: f32) {
        let Some(attached) = &mut self.attached else {
            return;
        };
        let Some(results) = &attached.results else {
            empty_state(
                ui,
                "Attached. No scan yet.",
                "Type the value you can see in the target — health, ammo, a score — then \
                 run a first scan. Change it, come back, and narrow the results down.",
            );
            return;
        };

        let total = match results {
            Results::Numeric(m) => m.len(),
            Results::Aob(m) => m.len(),
        };
        let capped = attached.capped;
        let selected_count = attached.selected.len();
        let frozen_count = match results {
            Results::Numeric(m) => m
                .iter()
                .filter(|m| attached.freeze.is_frozen(m.address))
                .count(),
            Results::Aob(m) => m
                .iter()
                .filter(|m| attached.freeze.is_frozen(m.address))
                .count(),
        };

        // Which optional columns fit. A dropped column's content moves to the
        // row tooltip rather than being lost.
        let show_module = window_width >= theme::breakpoint::DROP_MODULE;
        let show_previous = window_width >= theme::breakpoint::DROP_PREVIOUS;

        ui.horizontal(|ui| {
            theme::section_label(ui, "results");
            ui.add_space(theme::space::SM);
            ui.label(
                egui::RichText::new(format!("{total} addresses"))
                    .font(theme::font(theme::text_style::MONO_LIVE))
                    .color(theme::TEXT),
            );
            let mut note = Vec::new();
            if capped {
                note.push("capped — narrow your search".to_string());
            }
            if selected_count > 0 {
                note.push(format!("{selected_count} selected"));
            }
            if frozen_count > 0 {
                note.push(format!("{frozen_count} frozen"));
            }
            if !note.is_empty() {
                ui.label(
                    egui::RichText::new(note.join(" · "))
                        .font(theme::font(theme::text_style::SECONDARY))
                        .color(if capped {
                            theme::ACCENT_LIFT
                        } else {
                            theme::TEXT_DIM
                        }),
                );
            }
        });
        ui.add_space(theme::space::SM);

        let value_type = self.value_type;
        let module_map = &self.module_map;
        let changed_at = &self.changed_at;
        let now = ui.input(|i| i.time);
        let mut to_promote: Option<SavedRow> = None;

        let Attached {
            freeze,
            selected,
            results,
            ..
        } = attached;
        let results = results.as_ref().expect("checked above");

        let mut builder = egui_extras::TableBuilder::new(ui)
            .id_salt("results_table")
            .striped(true)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(egui_extras::Column::exact(theme::col::SELECT))
            .column(egui_extras::Column::exact(theme::col::FREEZE))
            .column(egui_extras::Column::exact(theme::col::ADDRESS))
            .column(egui_extras::Column::exact(theme::col::VALUE));
        if show_previous {
            builder = builder.column(egui_extras::Column::exact(theme::col::VALUE));
        }
        if show_module {
            builder = builder.column(egui_extras::Column::remainder());
        }
        builder = builder.column(egui_extras::Column::exact(theme::col::ACTION));

        builder
            .header(theme::HEADER_HEIGHT, |mut header| {
                header.col(|ui| {
                    theme::column_header(ui, "");
                });
                header.col(|ui| {
                    theme::column_header(ui, "frz");
                });
                header.col(|ui| {
                    theme::column_header(ui, "address");
                });
                header.col(|ui| {
                    theme::column_header(ui, "value");
                });
                if show_previous {
                    header.col(|ui| {
                        theme::column_header(ui, "previous");
                    });
                }
                if show_module {
                    header.col(|ui| {
                        theme::column_header(ui, "module + offset");
                    });
                }
                header.col(|ui| {
                    theme::column_header(ui, "");
                });
            })
            .body(|body| {
                body.rows(theme::ROW_HEIGHT, total, |mut row| {
                    let index = row.index();
                    let (address, value_text, previous_text, live_bytes) = match results {
                        Results::Numeric(m) => {
                            let m = &m[index];
                            (
                                m.address,
                                format_value(m.value),
                                format_value(m.previous),
                                m.value.to_le_bytes(),
                            )
                        }
                        Results::Aob(m) => {
                            let m = &m[index];
                            (
                                m.address,
                                value_type.format_bytes(&m.bytes),
                                value_type.format_bytes(&m.previous),
                                m.bytes.clone(),
                            )
                        }
                    };
                    let is_selected = selected.contains(&address);
                    row.set_selected(is_selected);

                    row.col(|ui| {
                        let mut checked = is_selected;
                        if ui.checkbox(&mut checked, "").changed() {
                            if checked {
                                selected.insert(address);
                            } else {
                                selected.remove(&address);
                            }
                        }
                    });
                    row.col(|ui| {
                        let mut frozen = freeze.is_frozen(address);
                        if ui.checkbox(&mut frozen, "").changed() {
                            if frozen {
                                freeze.freeze(address, live_bytes.clone());
                            } else {
                                freeze.unfreeze(address);
                            }
                        }
                    });
                    row.col(|ui| {
                        ui.add(egui::Label::new(hex_address_job(address, is_selected)).extend());
                    });
                    row.col(|ui| {
                        // A value that changed on the last tick flashes and
                        // decays back to the row ground, so a change is
                        // visible even if you were looking elsewhere.
                        let flash = changed_at
                            .get(&address)
                            .map(|t| ((now - t) / theme::FLASH_DECAY as f64).clamp(0.0, 1.0))
                            .unwrap_or(1.0);
                        if flash < 1.0 {
                            let rect = ui.max_rect();
                            ui.painter().rect_filled(
                                rect,
                                0.0,
                                theme::ACCENT_WASH.gamma_multiply(1.0 - flash as f32),
                            );
                        }
                        ui.label(
                            egui::RichText::new(value_text)
                                .font(theme::font(theme::text_style::MONO_LIVE))
                                .color(theme::TEXT),
                        );
                    });
                    if show_previous {
                        row.col(|ui| {
                            ui.label(
                                egui::RichText::new(previous_text)
                                    .font(theme::font(theme::text_style::MONO_VALUE))
                                    .color(theme::TEXT_DIM),
                            );
                        });
                    }
                    if show_module {
                        row.col(|ui| {
                            let text = module_map
                                .resolve(address)
                                .map(|m| m.to_string())
                                .unwrap_or_else(|| "—".to_string());
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(text)
                                        .font(theme::font(theme::text_style::MONO_VALUE))
                                        .color(theme::TEXT_DIM),
                                )
                                .truncate(),
                            );
                        });
                    }
                    row.col(|ui| {
                        if ui
                            .add(egui::Button::new("→").frame(false))
                            .on_hover_text("Send to the saved list")
                            .clicked()
                        {
                            to_promote = Some(SavedRow {
                                entry: CheatEntry {
                                    description: format!("{address:X}"),
                                    base: AddressExpr::Absolute(address),
                                    pointer_offsets: Vec::new(),
                                    value: match results {
                                        Results::Numeric(m) => EntryValue::Scalar(m[index].value),
                                        Results::Aob(m) => value_type
                                            .entry_value_from_bytes(m[index].bytes.clone()),
                                    },
                                    frozen: false,
                                    show_as_hex: false,
                                },
                                resolved_address: Some(address),
                                status: RowStatus::Resolved,
                            });
                        }
                    });
                });
            });

        if let Some(row) = to_promote {
            self.saved.push(row);
        }
    }

    /// The manual-add form's body, inside its modal.
    ///
    /// A form with its own validation, so each fallible field owns a message
    /// slot and the whole thing shows what the address it is building
    /// actually resolves to before anything is added.
    fn show_manual_add_form(&mut self, ui: &mut egui::Ui) {
        ui.set_width(596.0);

        if self.attached.is_none() {
            ui.label(
                egui::RichText::new(
                    "Nothing is attached, so this entry will read \"not attached\" until a \
                     matching process is.",
                )
                .font(theme::font(theme::text_style::SECONDARY))
                .color(theme::TEXT_FAINT),
            );
            ui.add_space(theme::space::SM);
        }

        ui.label(
            egui::RichText::new("Description")
                .font(theme::font(theme::text_style::SECONDARY))
                .color(theme::TEXT_DIM),
        );
        ui.add_sized(
            [ui.available_width(), theme::FIELD_HEIGHT],
            egui::TextEdit::singleline(&mut self.manual_description),
        );
        message_slot(ui, None, "Optional. Defaults to the address you type.");

        ui.label(
            egui::RichText::new("Address")
                .font(theme::font(theme::text_style::SECONDARY))
                .color(theme::TEXT_DIM),
        );
        ui.add_sized(
            [ui.available_width(), theme::FIELD_HEIGHT],
            egui::TextEdit::singleline(&mut self.manual_address_text)
                .font(egui::TextStyle::Name(theme::text_style::MONO_VALUE.into())),
        );
        let address_error = (!self.manual_address_text.trim().is_empty())
            .then(|| parse_address_expr(&self.manual_address_text).err())
            .flatten();
        message_slot(
            ui,
            address_error.as_deref(),
            "Absolute hex (7FF6A41C58DA) or module+offset (game.exe+1C58DA0).",
        );

        ui.label(
            egui::RichText::new("Pointer offsets")
                .font(theme::font(theme::text_style::SECONDARY))
                .color(theme::TEXT_DIM),
        );
        ui.add_sized(
            [ui.available_width(), theme::FIELD_HEIGHT],
            egui::TextEdit::singleline(&mut self.manual_pointer_offsets_text)
                .font(egui::TextStyle::Name(theme::text_style::MONO_VALUE.into())),
        );
        let offsets = parse_pointer_offsets(&self.manual_pointer_offsets_text);
        message_slot(
            ui,
            offsets.as_ref().err().map(String::as_str),
            "Optional. Hex, separated by commas or spaces, outermost first.",
        );

        // The expression the two fields above add up to, written the way a
        // saved table writes it. Shown before anything is added, because a
        // pointer chain is exactly the kind of thing that is easy to get one
        // level wrong and hard to notice afterwards.
        if let (Ok(base), Ok(offsets)) = (
            parse_address_expr(&self.manual_address_text),
            offsets.as_ref(),
        ) && !offsets.is_empty()
        {
            let base_text = match &base {
                AddressExpr::Absolute(a) => format!("{a:X}"),
                AddressExpr::ModuleRelative { module, offset } => {
                    format!("{module}+{offset:X}")
                }
            };
            let mut expr = format!("[{base_text}]");
            for offset in offsets.iter().rev().skip(1).rev() {
                expr = format!("[{expr}+{offset:X}]");
            }
            if let Some(last) = offsets.last() {
                expr = format!("{expr}+{last:X}");
            }
            ui.label(
                egui::RichText::new(expr)
                    .font(theme::font(theme::text_style::MONO_VALUE))
                    .color(theme::TEXT_DIM),
            );
            ui.add_space(theme::space::SM);
        }

        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(
                    egui::RichText::new("Type")
                        .font(theme::font(theme::text_style::SECONDARY))
                        .color(theme::TEXT_DIM),
                );
                egui::ComboBox::new("manual_value_type", "")
                    .selected_text(self.manual_value_type.label())
                    .width(160.0)
                    .show_ui(ui, |ui| {
                        for choice in ValueTypeChoice::ALL {
                            ui.selectable_value(
                                &mut self.manual_value_type,
                                choice,
                                choice.label(),
                            );
                        }
                    });
            });
            ui.add_space(theme::space::MD);
            ui.vertical(|ui| {
                ui.label(
                    egui::RichText::new("Value")
                        .font(theme::font(theme::text_style::SECONDARY))
                        .color(theme::TEXT_DIM),
                );
                ui.add_sized(
                    [220.0, theme::FIELD_HEIGHT],
                    egui::TextEdit::singleline(&mut self.manual_value_text)
                        .font(egui::TextStyle::Name(theme::text_style::MONO_VALUE.into())),
                );
            });
        });
        let value_error = (!self.manual_value_text.trim().is_empty())
            .then(|| parse_entry_value(self.manual_value_type, &self.manual_value_text).err())
            .flatten();
        message_slot(
            ui,
            value_error.as_deref(),
            "What a freeze would pin. Not written to the target on Add.",
        );

        ui.add_space(theme::space::SM);
        ui.horizontal(|ui| {
            // Enabled only once the whole form builds, so Add can't produce
            // an error - the fields have already said what is wrong.
            let ready = self.build_manual_entry().is_ok();
            if ui
                .add_enabled_ui(ready, |ui| theme::primary(ui, "Add to saved list"))
                .inner
                .clicked()
            {
                match self.build_manual_entry() {
                    Ok(entry) => {
                        self.saved.push(SavedRow {
                            entry,
                            resolved_address: None,
                            status: RowStatus::NotAttached,
                        });
                        self.manual_description.clear();
                        self.manual_address_text.clear();
                        self.manual_pointer_offsets_text.clear();
                        self.manual_value_text.clear();
                        self.manual_add_error = None;
                        self.manual_add_open = false;
                    }
                    Err(err) => self.manual_add_error = Some(err),
                }
            }
            if ui.button("Cancel").clicked() {
                self.manual_add_open = false;
                self.manual_add_error = None;
            }
        });
        if let Some(err) = &self.manual_add_error {
            ui.label(
                egui::RichText::new(err)
                    .font(theme::font(theme::text_style::SECONDARY))
                    .color(theme::ACCENT_LIFT),
            );
        }
    }

    fn build_manual_entry(&self) -> Result<CheatEntry, String> {
        let base = parse_address_expr(&self.manual_address_text)?;
        let pointer_offsets = parse_pointer_offsets(&self.manual_pointer_offsets_text)?;
        let value = parse_entry_value(self.manual_value_type, &self.manual_value_text)?;
        let description = if self.manual_description.trim().is_empty() {
            self.manual_address_text.trim().to_string()
        } else {
            self.manual_description.trim().to_string()
        };

        Ok(CheatEntry {
            description,
            base,
            pointer_offsets,
            value,
            frozen: false,
            show_as_hex: false,
        })
    }

    /// Save/load controls: a plain path text field plus two buttons - no
    /// native file dialog for v1 (a decided simplification, see the vault's
    /// `v0.1-plan.md`).
    /// The table actions in the top bar. Labels drop to icons on a narrow
    /// window, but the process identity and Detach always stay in words.
    ///
    /// Declared Import, Load, Save because the top bar lays this group out
    /// right-to-left, which renders them Save, Load, Import from the left.
    fn show_table_actions(&mut self, ui: &mut egui::Ui, icons_only: bool) {
        {
            {
                let label =
                    |full: &str, icon: &str| if icons_only { icon } else { full }.to_string();

                if ui.button(label("Import .CT", ".CT")).clicked()
                    && let Some(path) = rfd::FileDialog::new()
                        .add_filter("Cheat Engine table", &["CT", "ct"])
                        .pick_file()
                {
                    match import_ct_file(&path) {
                        Ok(report) => {
                            self.table_status = Some(format!(
                                "Imported {} entries ({} skipped — see below).",
                                report.imported.len(),
                                report.skipped.len()
                            ));
                            self.table_path_text = path.display().to_string();
                            for entry in &report.imported {
                                self.saved.push(SavedRow {
                                    entry: entry.clone(),
                                    resolved_address: None,
                                    status: RowStatus::NotAttached,
                                });
                            }
                            self.import_report = Some(report);
                        }
                        Err(err) => self.table_status = Some(format!("Import failed: {err}")),
                    }
                }
                if ui.button(label("Load table", "↑")).clicked()
                    && let Some(path) = rfd::FileDialog::new()
                        .add_filter("Ferrite table", &["json"])
                        .pick_file()
                {
                    match load_table(&path) {
                        Ok(entries) => {
                            self.table_status = Some(format!("Loaded {} entries.", entries.len()));
                            self.table_path_text = path.display().to_string();
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
                if ui.button(label("Save table", "↓")).clicked()
                    && let Some(path) = rfd::FileDialog::new()
                        .set_file_name("cheat_table.json")
                        .add_filter("Ferrite table", &["json"])
                        .save_file()
                {
                    let entries: Vec<CheatEntry> =
                        self.saved.iter().map(|r| r.entry.clone()).collect();
                    match save_table(&path, &entries) {
                        Ok(()) => {
                            self.table_status = Some(format!("Saved {} entries.", entries.len()));
                            self.table_path_text = path.display().to_string();
                        }
                        Err(err) => self.table_status = Some(format!("Save failed: {err}")),
                    }
                }
            }
        }
    }

    /// The visible unsupported-entries report `.CT` import must show, per
    /// the vault's `v0.1-scope.md` - every skipped entry with its description
    /// and reason, not just a log line, plus the informational note about
    /// entries that were frozen in the source table (see
    /// `ImportReport::was_active_in_source` in `ferrite-core::ct_import`).
    /// The import report.
    ///
    /// A first-class screen rather than an error dump: it exists so someone
    /// can read what a downloaded table would have done *before* trusting
    /// it, which is the decision an embedded script actually asks of them.
    /// Ferrite never assembles, injects or runs any of it — the script pane
    /// is text on a page.
    ///
    /// Split side by side when there is room, stacked below 1200 px. Split
    /// rather than modal so the list keeps its scroll position while you
    /// read one script after another.
    fn show_import_report(&mut self, ui: &mut egui::Ui, window_width: f32) {
        let Some(report) = &self.import_report else {
            return;
        };
        let imported = report.imported.len();
        let skipped = report.skipped.len();
        let with_script = report
            .skipped
            .iter()
            .filter(|s| s.script_text.is_some())
            .count();
        let was_active = report.was_active_in_source.len();

        ui.horizontal(|ui| {
            theme::section_label(ui, "import report");
            ui.add_space(theme::space::SM);
            ui.label(
                egui::RichText::new(format!(
                    "{imported} of {} entries imported",
                    imported + skipped
                ))
                .font(theme::font(theme::text_style::MONO_LIVE))
                .color(theme::TEXT),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Dismiss").clicked() {
                    self.import_report = None;
                    self.selected_skip = None;
                }
            });
        });
        if self.import_report.is_none() {
            return;
        }
        ui.add_space(theme::space::XS);

        let mut facts = vec![format!("{skipped} skipped")];
        if with_script > 0 {
            facts.push(format!("{with_script} of them carry a script"));
        }
        if was_active > 0 {
            facts.push(format!("{was_active} were frozen in the source"));
        }
        ui.label(
            egui::RichText::new(format!(
                "{}. Nothing was executed to produce this report.",
                facts.join(" · ")
            ))
            .font(theme::font(theme::text_style::SECONDARY))
            .color(theme::TEXT_DIM),
        );
        if was_active > 0 {
            ui.label(
                egui::RichText::new(
                    "Freeze is off on every imported entry — re-check the ones you want.",
                )
                .font(theme::font(theme::text_style::SECONDARY))
                .color(theme::TEXT_FAINT),
            );
        }
        ui.add_space(theme::space::MD);

        if skipped == 0 {
            return;
        }

        let split = window_width >= theme::breakpoint::REPORT_SPLIT;
        if split {
            let available = ui.available_width();
            ui.horizontal_top(|ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(available * 0.52, ui.available_height()),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| self.show_skipped_list(ui),
                );
                ui.painter().vline(
                    ui.cursor().left(),
                    ui.max_rect().y_range(),
                    theme::divider_stroke(),
                );
                ui.add_space(theme::space::MD);
                ui.vertical(|ui| self.show_script_pane(ui));
            });
        } else {
            self.show_skipped_list(ui);
            ui.add_space(theme::space::MD);
            self.show_script_pane(ui);
        }
    }

    /// The skipped entries, each with its reason.
    ///
    /// Not the virtualised table: a row carries a two-line reason, and
    /// sixteen skipped entries out of forty-seven is a long list but not a
    /// large one, so it is a list rather than a table pretending to scale.
    fn show_skipped_list(&mut self, ui: &mut egui::Ui) {
        let Some(report) = &self.import_report else {
            return;
        };
        ui.horizontal(|ui| {
            theme::column_header(ui, "skipped entry");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                theme::column_header(ui, "script");
            });
        });
        ui.add_space(theme::space::XS);

        let mut newly_selected = None;
        egui::ScrollArea::vertical()
            .id_salt("skipped_list")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for (index, entry) in report.skipped.iter().enumerate() {
                    let is_selected = self.selected_skip == Some(index);
                    let lines = entry
                        .script_text
                        .as_ref()
                        .map(|s| s.lines().count())
                        .unwrap_or(0);

                    let response = ui.allocate_ui(
                        egui::vec2(ui.available_width(), theme::REASON_ROW_HEIGHT),
                        |ui| {
                            if is_selected {
                                ui.painter()
                                    .rect_filled(ui.max_rect(), 0.0, theme::ACCENT_WASH);
                            }
                            ui.horizontal(|ui| {
                                ui.vertical(|ui| {
                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(&entry.description)
                                                .color(theme::TEXT),
                                        )
                                        .truncate()
                                        .selectable(false),
                                    );
                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(&entry.reason)
                                                .font(theme::font(theme::text_style::SECONDARY))
                                                .color(theme::TEXT_DIM),
                                        )
                                        .truncate()
                                        .selectable(false),
                                    );
                                });
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        // A row with a script is the only
                                        // kind you can open, so only it gets
                                        // an affordance.
                                        if lines > 0 {
                                            ui.add_space(theme::space::SM);
                                            script_kind_chip(ui, entry.script_kind);
                                            if ui
                                                .selectable_label(
                                                    is_selected,
                                                    format!("{lines} lines"),
                                                )
                                                .clicked()
                                            {
                                                newly_selected = Some(index);
                                            }
                                        } else {
                                            ui.label(
                                                egui::RichText::new("—").color(theme::TEXT_FAINT),
                                            );
                                        }
                                    },
                                );
                            });
                        },
                    );
                    let _ = response;
                    ui.painter().hline(
                        ui.max_rect().x_range(),
                        ui.cursor().top(),
                        egui::Stroke::new(1.0, theme::STROKE),
                    );
                }
            });

        if let Some(index) = newly_selected {
            self.selected_skip = Some(index);
        }
    }

    /// The selected entry's script, in full.
    fn show_script_pane(&mut self, ui: &mut egui::Ui) {
        let Some(report) = &self.import_report else {
            return;
        };
        let Some(entry) = self
            .selected_skip
            .and_then(|index| report.skipped.get(index))
        else {
            // The count has to come from the report; an earlier draft
            // hardcoded "Nine", which was simply wrong for any other table.
            let with_script = report
                .skipped
                .iter()
                .filter(|s| s.script_text.is_some())
                .count();
            empty_state(
                ui,
                "Pick an entry to read its script",
                &format!(
                    "{with_script} of these carry an Auto Assembler or Lua script. Ferrite \
                     runs none of them — the text is here so you can read what the table \
                     would have done and decide for yourself."
                ),
            );
            return;
        };
        let Some(script) = entry.script_text.clone() else {
            empty_state(
                ui,
                "No script on this entry",
                "This one was skipped for the reason beside it, not because it carries \
                 code.",
            );
            return;
        };

        let lines = script.lines().count();
        ui.horizontal(|ui| {
            ui.add(
                egui::Label::new(egui::RichText::new(&entry.description).color(theme::TEXT))
                    .truncate(),
            );
            ui.label(
                egui::RichText::new(format!("{lines} lines"))
                    .font(theme::font(theme::text_style::SECONDARY))
                    .color(theme::TEXT_DIM),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.checkbox(&mut self.script_wrap, "Wrap");
                if ui.button("Copy").clicked() {
                    ui.ctx().copy_text(script.clone());
                }
            });
        });
        ui.label(
            egui::RichText::new(
                "Read-only. Ferrite never assembles, injects or runs this — it is text \
                 on a page.",
            )
            .font(theme::font(theme::text_style::SECONDARY))
            .color(theme::TEXT_FAINT),
        );
        ui.add_space(theme::space::SM);

        // A real read-only multiline field, not a painted block: selectable,
        // copyable, and a single node in the accessibility tree.
        egui::ScrollArea::both()
            .id_salt("script_pane")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let mut text = script.as_str();
                ui.add(
                    egui::TextEdit::multiline(&mut text)
                        .font(egui::TextStyle::Name(theme::text_style::MONO_VALUE.into()))
                        .desired_width(if self.script_wrap {
                            ui.available_width()
                        } else {
                            f32::INFINITY
                        })
                        .desired_rows(lines.clamp(8, 40))
                        .interactive(true),
                );
            });
    }

    fn show_saved_list_table(&mut self, ui: &mut egui::Ui) {
        let mut to_remove: Option<usize> = None;
        // Split the borrows explicitly: the table body needs `&mut
        // self.saved` while still reading the freeze handle, and
        // FreezeHandle is not Clone.
        let freeze = self.attached.as_ref().map(|a| &a.freeze);
        let rows = &mut self.saved;

        egui_extras::TableBuilder::new(ui)
            .id_salt("saved_table")
            .striped(true)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(egui_extras::Column::exact(theme::col::FREEZE))
            .column(egui_extras::Column::remainder().at_least(160.0))
            .column(egui_extras::Column::exact(theme::col::ADDRESS))
            .column(egui_extras::Column::exact(theme::col::VALUE))
            .column(egui_extras::Column::remainder().at_least(120.0))
            .column(egui_extras::Column::exact(theme::col::ACTION))
            .header(theme::HEADER_HEIGHT, |mut header| {
                header.col(|ui| {
                    theme::column_header(ui, "frz");
                });
                header.col(|ui| {
                    theme::column_header(ui, "description");
                });
                header.col(|ui| {
                    theme::column_header(ui, "address");
                });
                header.col(|ui| {
                    theme::column_header(ui, "value");
                });
                header.col(|ui| {
                    theme::column_header(ui, "state");
                });
                header.col(|ui| {
                    theme::column_header(ui, "");
                });
            })
            .body(|body| {
                body.rows(theme::ROW_HEIGHT, rows.len(), |mut row| {
                    let index = row.index();
                    let entry = &mut rows[index];
                    let resolved = entry.status.is_resolved();
                    let ink = entry.status.ink();

                    row.col(|ui| {
                        // Freeze needs both an attached session and an
                        // address to pin; without either the box is shown
                        // but inert, so the column doesn't jump around.
                        match (freeze, entry.resolved_address, resolved) {
                            (Some(freeze), Some(address), true) => {
                                let mut frozen = entry.entry.frozen;
                                if ui.checkbox(&mut frozen, "").changed() {
                                    if frozen {
                                        freeze.freeze(address, entry.entry.value.to_le_bytes());
                                    } else {
                                        freeze.unfreeze(address);
                                    }
                                    entry.entry.frozen = frozen;
                                }
                            }
                            _ => {
                                let mut frozen = entry.entry.frozen;
                                ui.add_enabled(false, egui::Checkbox::new(&mut frozen, ""));
                            }
                        }
                    });
                    row.col(|ui| {
                        // Frameless: the description is editable in place,
                        // but a bordered box on every row would turn the
                        // column into a stack of form fields rather than a
                        // column of names. The field still takes focus and
                        // still shows a caret when clicked.
                        ui.add(
                            egui::TextEdit::singleline(&mut entry.entry.description)
                                .desired_width(ui.available_width())
                                .frame(egui::Frame::NONE)
                                .text_color(ink),
                        );
                    });
                    row.col(|ui| match entry.resolved_address {
                        Some(address) if resolved => {
                            ui.add(egui::Label::new(hex_address_job(address, false)).extend());
                        }
                        _ => {
                            ui.label(
                                egui::RichText::new("—")
                                    .font(theme::font(theme::text_style::MONO_VALUE))
                                    .color(theme::TEXT_FAINT),
                            );
                        }
                    });
                    row.col(|ui| {
                        let text = if resolved {
                            format_entry_value(&entry.entry.value, entry.entry.show_as_hex)
                        } else {
                            "—".to_string()
                        };
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(text)
                                    .font(theme::font(theme::text_style::MONO_LIVE))
                                    .color(ink),
                            )
                            .truncate(),
                        );
                    });
                    row.col(|ui| {
                        // The reason names the module in quotes, so a
                        // wrong-target attach is obvious rather than just
                        // "unresolved".
                        let text = if resolved && entry.entry.frozen {
                            "frozen".to_string()
                        } else {
                            entry.status.label()
                        };
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(text)
                                    .font(theme::font(theme::text_style::SECONDARY))
                                    .color(if resolved {
                                        theme::TEXT_DIM
                                    } else {
                                        theme::TEXT_FAINT
                                    }),
                            )
                            .truncate(),
                        );
                    });
                    row.col(|ui| {
                        if ui
                            .add(egui::Button::new("×").frame(false))
                            .on_hover_text("Remove from the saved list")
                            .clicked()
                        {
                            to_remove = Some(index);
                        }
                    });
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

/// Parses the edit box's text into raw bytes to write: a hex pattern for
/// `Aob`, or a numeric value's little-endian bytes otherwise. Shared by
/// "Set Value" - `First Scan` has its own parse step because it also needs
/// the typed `ScanValue`/pattern, not just bytes.
fn parse_write_bytes(value_type: ValueTypeChoice, text: &str) -> Result<Vec<u8>, String> {
    if value_type.scans_as_bytes() {
        value_type.parse_pattern(text)
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
    if value_type.scans_as_bytes() {
        value_type
            .parse_pattern(text)
            .map(|bytes| value_type.entry_value_from_bytes(bytes))
    } else {
        value_type.parse(text).map(EntryValue::Scalar)
    }
}

/// A short label for what kind of script a skipped entry carries.
///
/// The reason line already spells this out; the chip exists so sixteen
/// skipped rows can be scanned at a glance for the one thing that decides
/// what a reader can do about them. Deliberately not colour-coded green /
/// red: in this palette red means attention, and none of these is an error
/// — they are facts about the table.
fn script_kind_chip(ui: &mut egui::Ui, kind: Option<ScriptKind>) {
    let (text, hover) = match kind {
        Some(ScriptKind::DataOnlyLua) => (
            "data-only Lua",
            "Reads and writes values only. This is the kind Ferrite could run.",
        ),
        Some(ScriptKind::GenerativeLua) => (
            "generates code",
            "Its Lua returns assembly, so running only the Lua would leave the              target partly modified.",
        ),
        Some(ScriptKind::Assembler) => (
            "Auto Assembler",
            "Assembles code, allocates memory inside the target and patches its              execution. Ferrite does none of that.",
        ),
        Some(ScriptKind::Empty) => ("no code", "The script has no statements in either half."),
        None => (
            "unreadable",
            "The script couldn't be parsed — Cheat Engine would reject it too.",
        ),
    };
    ui.add(
        egui::Label::new(
            egui::RichText::new(text)
                .font(theme::font(theme::text_style::SECONDARY))
                .color(theme::TEXT_FAINT),
        )
        .selectable(false),
    )
    .on_hover_text(hover);
}

/// A byte count at a scale that stays readable: a small target reported as
/// "0.00 GB" tells the user nothing, so the unit follows the magnitude.
fn format_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.2} GB", b / GB)
    } else if b >= MB {
        format!("{:.1} MB", b / MB)
    } else {
        format!("{:.0} KB", b / KB)
    }
}

/// An empty state: a headline and one sentence telling the user what to do
/// next. Inset from the region's edges so it reads as deliberate space
/// rather than as a region that failed to fill.
fn empty_state(ui: &mut egui::Ui, headline: &str, body: &str) {
    ui.add_space(theme::space::XXL);
    ui.horizontal(|ui| {
        ui.add_space(theme::space::XXL);
        ui.vertical(|ui| {
            ui.label(
                egui::RichText::new(headline)
                    .font(theme::font(theme::text_style::EMPTY_HEADLINE))
                    .color(theme::TEXT),
            );
            ui.add_space(theme::space::SM);
            ui.add(
                egui::Label::new(
                    egui::RichText::new(body)
                        .font(theme::font(theme::text_style::SECONDARY))
                        .color(theme::TEXT_DIM),
                )
                .wrap(),
            );
        });
    });
}

/// A fixed-height slot beneath a fallible field, holding either an error or
/// the field's hint.
///
/// Allocated whether or not there is a message, which is the whole point: an
/// error appearing must not move everything below it. Two lines' worth, and
/// a longer message ellipsises rather than growing the slot.
fn message_slot(ui: &mut egui::Ui, error: Option<&str>, hint: &str) {
    let (text, color) = match error {
        Some(err) => (err, theme::ACCENT_LIFT),
        None => (hint, theme::TEXT_FAINT),
    };
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), theme::MESSAGE_SLOT_HEIGHT),
        egui::Sense::hover(),
    );
    if text.is_empty() {
        return;
    }
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
    child.add(
        egui::Label::new(
            egui::RichText::new(text)
                .font(theme::font(theme::text_style::SECONDARY))
                .color(color),
        )
        .truncate(),
    );
}

/// Renders a 64-bit address as a fixed-width 16-digit hex run, with the
/// leading zeros dimmed.
///
/// This is what makes a column of addresses line up. Printed with `{:#x}`,
/// `0x14a20` and `0x7ff6a41c58da` are different widths and read as ragged
/// text rather than as a column; zero-padded to the full 16 nibbles they
/// align exactly, and dimming the padding keeps the significant digits
/// dominant.
///
/// Built as one `LayoutJob` so the cell stays a single `Label` — one widget,
/// one node in the accessibility tree, rather than two runs the UI
/// Automation driver would have to stitch back together.
fn hex_address_job(address: usize, on_wash: bool) -> egui::text::LayoutJob {
    let full = format!("{address:016X}");
    let significant = full.len() - full.trim_start_matches('0').len();
    // An address of exactly zero is all padding; keep one digit significant
    // so the cell never renders as entirely dim.
    let split = significant.min(full.len() - 1);
    let font = theme::font(theme::text_style::MONO_VALUE);
    let pad_color = if on_wash {
        theme::HEX_PAD_ON_WASH
    } else {
        theme::TEXT_FAINT
    };

    let mut job = egui::text::LayoutJob::default();
    if split > 0 {
        job.append(
            &full[..split],
            0.0,
            egui::TextFormat {
                font_id: font.clone(),
                color: pad_color,
                ..Default::default()
            },
        );
    }
    job.append(
        &full[split..],
        0.0,
        egui::TextFormat {
            font_id: font,
            color: theme::TEXT,
            ..Default::default()
        },
    );
    job
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

impl FerriteApp {
    /// The top bar: identity on the left, table actions on the right.
    fn show_top_bar(&mut self, ui: &mut egui::Ui, window_width: f32) {
        let icons_only = window_width < theme::breakpoint::TOPBAR_ICONS;
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("FERRITE")
                    .font(theme::font(theme::text_style::SECTION_LABEL))
                    .color(theme::ACCENT),
            );
            ui.add_space(theme::space::MD);

            match &self.attached {
                Some(attached) => {
                    ui.label(
                        egui::RichText::new(&attached.process_name)
                            .font(theme::font(theme::text_style::SECONDARY))
                            .color(theme::TEXT),
                    );
                    ui.label(
                        egui::RichText::new(format!("pid {}", attached.pid))
                            .font(theme::font(theme::text_style::MONO_VALUE))
                            .color(theme::TEXT_DIM),
                    );
                    if ui.button("Detach").clicked() {
                        self.detach();
                    }
                }
                None => {
                    ui.label(
                        egui::RichText::new("Not attached")
                            .font(theme::font(theme::text_style::SECONDARY))
                            .color(theme::TEXT_DIM),
                    );
                }
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                self.show_table_actions(ui, icons_only);
            });
        });
    }

    /// Detaches, dropping the handle and the freeze thread with it.
    fn detach(&mut self) {
        self.attached = None;
        self.module_map = ModuleMap::empty();
        self.scan_region_summary = None;
        self.scan_history.clear();
        self.changed_at.clear();
        self.mark_saved_entries_unattached();
    }

    /// The left rail: what is attached, and every control that starts or
    /// narrows a scan. It never collapses, because it holds the primary
    /// action.
    fn show_rail(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical()
            .id_salt("rail_scroll")
            .show(ui, |ui| {
                theme::section_label(ui, "target");
                ui.add_space(theme::space::SM);
                match &self.attached {
                    None => {
                        ui.label(
                            egui::RichText::new("No process attached")
                                .font(theme::font(theme::text_style::EMPTY_HEADLINE)),
                        );
                        ui.add_space(theme::space::SM);
                        ui.label(
                            egui::RichText::new(
                                "Pick a 64-bit process from the list. Ferrite reads and \
                                 writes only data — it never injects or runs code in the \
                                 target.",
                            )
                            .font(theme::font(theme::text_style::SECONDARY))
                            .color(theme::TEXT_DIM),
                        );
                    }
                    Some(_) => {
                        if let Some((bytes, regions)) = self.scan_region_summary {
                            ui.label(
                                egui::RichText::new(format!(
                                    "{} across {regions} regions.",
                                    format_bytes(bytes)
                                ))
                                .font(theme::font(theme::text_style::SECONDARY))
                                .color(theme::TEXT_DIM),
                            );
                        }
                        if !self.module_map.is_empty() {
                            ui.label(
                                egui::RichText::new(format!(
                                    "{} modules loaded.",
                                    self.module_map.len()
                                ))
                                .font(theme::font(theme::text_style::SECONDARY))
                                .color(theme::TEXT_DIM),
                            );
                        }
                    }
                }

                ui.add_space(theme::space::XL);
                self.show_scan_group(ui);

                if self.attached.as_ref().is_some_and(|a| a.results.is_some()) {
                    ui.add_space(theme::space::XL);
                    self.show_rescan_group(ui);
                    ui.add_space(theme::space::XL);
                    self.show_write_group(ui);
                }

                ui.add_space(theme::space::XL);
                ui.label(
                    egui::RichText::new(
                        "Reads and writes process data only. No code execution, no \
                         network, no telemetry.",
                    )
                    .font(theme::font(theme::text_style::SECONDARY))
                    .color(theme::TEXT_FAINT),
                );
            });
    }

    /// The central region once something is attached.
    fn show_results_region(&mut self, ui: &mut egui::Ui, window_width: f32) {
        self.show_results_table(ui, window_width);
    }

    /// The saved-list dock. Docked rather than stacked, so nothing above it
    /// can push it off the bottom of the window.
    fn show_saved_dock(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            theme::section_label(ui, "saved list");
            ui.add_space(theme::space::SM);
            let frozen = self.saved.iter().filter(|r| r.entry.frozen).count();
            let summary = match (self.saved.len(), frozen) {
                (0, _) => "0 entries".to_string(),
                (n, 0) => format!("{n} entries"),
                (n, f) => format!("{n} entries · {f} frozen"),
            };
            ui.label(
                egui::RichText::new(summary)
                    .font(theme::font(theme::text_style::SECONDARY))
                    .color(theme::TEXT_DIM),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Add manually").clicked() {
                    self.manual_add_open = true;
                }
            });
        });
        ui.add_space(theme::space::SM);

        if self.saved.is_empty() {
            ui.label(
                egui::RichText::new("Nothing saved yet")
                    .font(theme::font(theme::text_style::SECONDARY))
                    .color(theme::TEXT_DIM),
            );
            return;
        }
        self.show_saved_list_table(ui);
    }

    /// The manual-add form, as a modal rather than a section in a scroll: it
    /// has its own validation, and it must not push the dock around.
    fn show_manual_add_modal(&mut self, ctx: &egui::Context) {
        if !self.manual_add_open {
            return;
        }
        let mut open = true;
        egui::Window::new("Add address manually")
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .frame(theme::panel(theme::SURFACE).stroke(theme::divider_stroke()))
            .default_width(620.0)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| self.show_manual_add_form(ui));
        self.manual_add_open &= open;
    }
}

impl eframe::App for FerriteApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.check_target_exited();
        self.refresh_live_values(ui.input(|i| i.time));
        self.refresh_saved_entries();
        if self.attached.is_some() {
            // eframe only repaints reactively (on input) by default - an
            // attached session needs a nudge to keep polling live values
            // even when the user isn't touching anything.
            ui.ctx().request_repaint_after(LIVE_REFRESH_INTERVAL);
        }

        // One window, three regions. Declared top -> bottom -> left ->
        // central because egui allocates panels in call order, and the top
        // bar and the dock both span the full width while the rail only
        // occupies what is left between them.
        //
        // Nothing here scrolls as a whole: each region owns its own scroll,
        // which is the point. The saved-list dock cannot be pushed off the
        // bottom of the window by anything above it, because nothing is
        // above it.
        // Captured before any panel is allocated, so it is the whole
        // window's width rather than whatever is left after the rail.
        let window_width = ui.available_width();

        egui::Panel::top("top_bar")
            .frame(theme::panel(theme::SURFACE_RAISED))
            .show_separator_line(false)
            .show(ui, |ui| self.show_top_bar(ui, window_width));
        divider(ui);

        egui::Panel::bottom("saved_dock")
            .frame(theme::panel(theme::SURFACE))
            .show_separator_line(false)
            .resizable(true)
            .min_size(theme::DOCK_HEIGHT_MIN)
            .default_size(theme::DOCK_HEIGHT_MIN * 1.6)
            .show(ui, |ui| self.show_saved_dock(ui));

        egui::Panel::left("rail")
            .frame(theme::panel(theme::SURFACE))
            .show_separator_line(false)
            .resizable(false)
            .exact_size(rail_width(window_width))
            .show(ui, |ui| self.show_rail(ui));

        egui::CentralPanel::default()
            .frame(theme::panel(theme::GROUND))
            .show(ui, |ui| {
                // The import report is its own view, not a banner above
                // another one: importing a table while detached is a normal
                // thing to do, and the report is the whole reason to look at
                // the screen when it happens. Dismiss returns to whichever
                // view the attach state calls for.
                if self.import_report.is_some() {
                    self.show_import_report(ui, window_width);
                } else if self.attached.is_some() {
                    self.show_results_region(ui, window_width);
                } else {
                    self.show_process_picker(ui);
                }
            });

        self.show_manual_add_modal(ui.ctx());
    }
}

/// The rail's width at the current window size: its preferred 340 px,
/// shrinking to a 300 px floor on a narrow window. It never collapses — it
/// holds the primary action.
fn rail_width(window_width: f32) -> f32 {
    if window_width < theme::breakpoint::RAIL_SHRINK {
        theme::RAIL_WIDTH_MIN
    } else {
        theme::RAIL_WIDTH
    }
}

/// The 2 px rule that separates two regions. Drawn by hand rather than left
/// to egui's own separator line, which is 1 px and takes its colour from the
/// widget palette rather than from the divider token.
fn divider(ui: &mut egui::Ui) {
    let rect = ui.available_rect_before_wrap();
    let y = rect.top();
    ui.painter()
        .hline(rect.x_range(), y, theme::divider_stroke());
    ui.add_space(2.0);
}
