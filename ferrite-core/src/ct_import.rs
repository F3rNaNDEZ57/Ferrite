//! Imports a Cheat Engine `.CT` table (plain XML) into our own [`CheatEntry`]
//! list. Schema verified against real tables and Cheat Engine's own source
//! (`MemoryRecordUnit.pas`/`CEFuncProc.pas` — see the vault's `v1-plan.md`
//! for the full research), not guessed.
//!
//! Entries this can't represent (Lua scripts, structure dissect, multi-level
//! pointer chains, unrecognized types, symbol/pointer-expression addresses)
//! go into a visible [`ImportReport`] rather than being silently dropped or
//! mis-imported, per the vault's `v1-scope.md`.

use std::path::Path;

use serde::Deserialize;

use crate::scan_value::ScanValue;
use crate::table::{CheatEntry, EntryValue, parse_address_expr, parse_hex_usize};

#[derive(Debug, Deserialize)]
struct CheatTableXml {
    #[serde(rename = "CheatEntries", default)]
    cheat_entries: Option<CheatEntriesXml>,
}

#[derive(Debug, Deserialize, Default)]
struct CheatEntriesXml {
    #[serde(rename = "CheatEntry", default)]
    entry: Vec<CheatEntryXml>,
}

#[derive(Debug, Deserialize, Default)]
struct CheatEntryXml {
    #[serde(rename = "Description", default)]
    description: Option<String>,
    #[serde(rename = "VariableType", default)]
    variable_type: Option<String>,
    #[serde(rename = "Address", default)]
    address: Option<String>,
    #[serde(rename = "Offsets", default)]
    offsets: Option<OffsetsXml>,
    #[serde(rename = "LastState", default)]
    last_state: Option<LastStateXml>,
    // A group entry has its own nested `<CheatEntries>` - structurally
    // distinct from a leaf, not just tagged as one (see `is_group` below).
    #[serde(rename = "CheatEntries", default)]
    nested: Option<CheatEntriesXml>,
}

#[derive(Debug, Deserialize, Default)]
struct OffsetsXml {
    #[serde(rename = "Offset", default)]
    offset: Vec<String>,
}

/// `<LastState Activated="1"/>` is a self-closing element with an
/// *attribute*, not a child element - the `@` prefix is required for
/// quick-xml's serde support to see it at all. Getting this wrong fails
/// silently (the field just stays `None`), which would make every imported
/// entry look like it was never frozen in the source table - asserted
/// against a real shape in this module's tests, not just compiled once and
/// trusted.
#[derive(Debug, Deserialize, Default)]
struct LastStateXml {
    #[serde(rename = "@Activated", default)]
    activated: Option<String>,
}

/// One entry that couldn't be imported, with a human-readable reason - the
/// visible report `v1-scope.md` requires, not just a log line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedEntry {
    pub description: String,
    pub reason: String,
}

/// The result of importing a `.CT` file.
#[derive(Debug, Default)]
pub struct ImportReport {
    pub imported: Vec<CheatEntry>,
    pub skipped: Vec<SkippedEntry>,
    /// Descriptions of entries that were frozen (`Activated="1"`) in the
    /// source table. Imported with `frozen: false` regardless - the source
    /// file never carries a real captured value alongside `Activated` (both
    /// real sample tables checked had `Value=""`), so honoring it as-is
    /// would freeze live game memory to a placeholder zero the moment the
    /// table resolves against an attached process. That's a destructive
    /// default, not a cosmetic one - see the vault's `v1-plan.md`. This list
    /// is purely informational, for the caller to surface.
    pub was_active_in_source: Vec<String>,
}

/// Why importing a `.CT` file failed outright (the whole file, not one
/// entry - a single bad entry goes in [`ImportReport::skipped`] instead).
#[derive(Debug)]
pub enum CtImportError {
    Io(std::io::Error),
    Xml(quick_xml::DeError),
}

impl std::fmt::Display for CtImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(err) => write!(f, "{err}"),
            Self::Xml(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for CtImportError {}

impl From<std::io::Error> for CtImportError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}

impl From<quick_xml::DeError> for CtImportError {
    fn from(err: quick_xml::DeError) -> Self {
        Self::Xml(err)
    }
}

/// Reads and imports a `.CT` file from disk.
pub fn import_ct_file(path: &Path) -> Result<ImportReport, CtImportError> {
    let xml = std::fs::read_to_string(path)?;
    import_ct_xml(&xml)
}

/// Imports a `.CT` file's already-read XML content. Split from
/// [`import_ct_file`] so the parser itself is testable against inline XML,
/// no file needed.
pub fn import_ct_xml(xml: &str) -> Result<ImportReport, CtImportError> {
    let table: CheatTableXml = quick_xml::de::from_str(xml)?;
    let mut report = ImportReport::default();
    if let Some(entries) = &table.cheat_entries {
        import_entries(&entries.entry, "", &mut report);
    }
    Ok(report)
}

/// Walks a `<CheatEntries>` list, recursing into group entries and
/// importing (or reporting) every leaf. `group_path` accumulates enclosing
/// group descriptions ("Addresses / xspeed pointer 1") so flattening groups
/// for v1 doesn't destroy that information.
fn import_entries(entries: &[CheatEntryXml], group_path: &str, report: &mut ImportReport) {
    for entry in entries {
        let description = strip_one_quote_pair(entry.description.as_deref().unwrap_or(""));
        let full_description = if group_path.is_empty() {
            description.to_string()
        } else {
            format!("{group_path} / {description}")
        };

        if is_group(entry) {
            if let Some(nested) = &entry.nested {
                import_entries(&nested.entry, &full_description, report);
            }
            continue;
        }

        match import_leaf_entry(entry) {
            Ok(cheat_entry) => {
                if was_activated(entry) {
                    report.was_active_in_source.push(full_description.clone());
                }
                report.imported.push(CheatEntry {
                    description: full_description,
                    ..cheat_entry
                });
            }
            Err(reason) => report.skipped.push(SkippedEntry {
                description: full_description,
                reason,
            }),
        }
    }
}

/// A group (folder/category) entry, not a leaf - presence of its own
/// nested `<CheatEntries>` is the primary signal (a real group always has
/// this); `VariableType == "Grouped"` is a secondary signal from Cheat
/// Engine's own recognized type strings, in case some version marks a group
/// that way instead of (or in addition to) nesting.
fn is_group(entry: &CheatEntryXml) -> bool {
    entry.nested.as_ref().is_some_and(|n| !n.entry.is_empty())
        || entry.variable_type.as_deref() == Some("Grouped")
}

fn was_activated(entry: &CheatEntryXml) -> bool {
    entry
        .last_state
        .as_ref()
        .and_then(|ls| ls.activated.as_deref())
        == Some("1")
}

/// Imports one leaf entry, or explains why it can't be. `description` is
/// filled in by the caller (`import_entries`) once this succeeds - kept out
/// of the returned `CheatEntry` here so this function stays focused on the
/// type/address/offset logic.
fn import_leaf_entry(entry: &CheatEntryXml) -> Result<CheatEntry, String> {
    let variable_type = entry
        .variable_type
        .as_deref()
        .ok_or_else(|| "no VariableType".to_string())?;
    let value = map_variable_type(variable_type)?;

    let address_text = entry
        .address
        .as_deref()
        .ok_or_else(|| "no Address".to_string())?;
    // Bracket/symbol expressions (`[players]`, `[Ns::Sym+18]`) would
    // otherwise slip past `parse_address_expr` as a mangled but
    // technically-parseable module name (e.g. module `"[game.exe"`) that
    // can never resolve - an honest report line beats a perpetual, cryptic
    // "module not found" for an address form v1 was never going to support
    // anyway. See the vault's `v1-plan.md`.
    if address_text.contains('[') || address_text.contains(']') {
        return Err(format!(
            "unsupported address expression {address_text:?} (symbol/pointer-expression addressing isn't supported)"
        ));
    }
    let base = parse_address_expr(address_text)
        .map_err(|err| format!("unsupported address {address_text:?}: {err}"))?;

    let offsets: &[String] = entry
        .offsets
        .as_ref()
        .map(|o| o.offset.as_slice())
        .unwrap_or(&[]);
    let pointer_offset = match offsets.len() {
        0 => None,
        1 => Some(
            parse_hex_usize(&offsets[0])
                .ok_or_else(|| format!("invalid offset {:?}", offsets[0]))?,
        ),
        n => {
            return Err(format!(
                "multi-level pointer chain ({n} offsets) isn't supported"
            ));
        }
    };

    Ok(CheatEntry {
        description: String::new(), // filled in by the caller
        base,
        pointer_offset,
        value,
        frozen: false, // see ImportReport::was_active_in_source
    })
}

/// Maps a `.CT` `<VariableType>` string to a zero-valued [`EntryValue`] of
/// the matching shape (the real value is filled in on first live resolve,
/// same as any other saved-list entry). Authoritative string table from
/// Cheat Engine's own `VariableTypeToString`/`StringToVariableType`
/// (`CEFuncProc.pas`) - see the vault's `v1-plan.md`. Unrecognized is
/// always reported, never silently guessed - unlike Cheat Engine's own
/// `StringToVariableType`, which falls back to `vtByte` for anything it
/// doesn't recognize.
fn map_variable_type(s: &str) -> Result<EntryValue, String> {
    match s {
        "Byte" => Ok(EntryValue::Scalar(ScanValue::I8(0))),
        "2 Bytes" => Ok(EntryValue::Scalar(ScanValue::I16(0))),
        "4 Bytes" => Ok(EntryValue::Scalar(ScanValue::I32(0))),
        "8 Bytes" => Ok(EntryValue::Scalar(ScanValue::I64(0))),
        "Float" => Ok(EntryValue::Scalar(ScanValue::F32(0.0))),
        "Double" => Ok(EntryValue::Scalar(ScanValue::F64(0.0))),
        "String" | "Unicode String" => {
            Err(format!("{s} entries aren't supported yet (deferred to v1.1)"))
        }
        // Unlike the fixed-width numeric types above, these two are only
        // confirmed *names* from Cheat Engine's source - never seen in a
        // real table, so their exact on-disk structure (does "Array of
        // byte" carry a length field? what does "Pointer" need beyond an
        // address?) isn't verified. Reported rather than guessed.
        "Array of byte" => Err(
            "Array of byte entries aren't supported yet (byte-length encoding not verified against a real table)"
                .to_string(),
        ),
        "Pointer" => Err(
            "Pointer-typed entries aren't supported yet (structure not verified against a real table)"
                .to_string(),
        ),
        "Auto Assembler Script" => {
            Err("Auto Assembler Script (Lua) entries aren't supported".to_string())
        }
        "Custom" => Err("Custom-type entries aren't supported (require Lua conversion functions)".to_string()),
        "Binary" => Err("Binary (bit-field) entries aren't supported".to_string()),
        "All" | "Grouped" => Err(format!("unexpected VariableType {s:?} on a leaf entry")),
        other => Err(format!("unrecognized VariableType {other:?}")),
    }
}

/// Strips one leading/trailing literal `"` pair from a `.CT` description -
/// confirmed from real files that Cheat Engine stores `Description` text
/// with the quotes as part of the value itself
/// (`<Description>"Enable +info"</Description>`), not XML attribute
/// quoting. Strips exactly one pair, not every quote (`trim_matches`
/// would over-strip a description that itself starts and ends with `"`).
fn strip_one_quote_pair(s: &str) -> &str {
    let s = s.trim();
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        &s[1..s.len() - 1]
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::table::AddressExpr;

    /// These fixtures are synthetic, hand-written to the schema verified in
    /// this session (see the vault's `v1-plan.md`) - not copies of the real
    /// downloaded tables used for that research, which are third-party
    /// files of unclear license and don't belong in this repo.
    fn load_fixture(name: &str) -> String {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name);
        std::fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("reading fixture {path:?}: {err}"))
    }

    #[test]
    fn lua_only_table_imports_nothing_and_reports_the_script() {
        let report = import_ct_xml(&load_fixture("lua_only.CT")).expect("parsing lua_only.CT");
        assert_eq!(report.imported.len(), 0);
        assert_eq!(report.skipped.len(), 1);
        assert_eq!(report.skipped[0].description, "Enable +info");
        assert!(report.skipped[0].reason.contains("Auto Assembler Script"));
    }

    #[test]
    fn basic_table_imports_supported_entries_and_reports_the_rest() {
        let report =
            import_ct_xml(&load_fixture("basic_table.CT")).expect("parsing basic_table.CT");

        // Imported: the two grouped entries, the absolute byte, the
        // single-level pointer, and the was-activated entry = 5.
        assert_eq!(
            report.imported.len(),
            5,
            "unexpected imported set: {:#?}",
            report.imported
        );
        // Skipped: multi-level chain, String, bracket address, unknown
        // type = 4.
        assert_eq!(
            report.skipped.len(),
            4,
            "unexpected skipped set: {:#?}",
            report.skipped
        );

        let grouped = report
            .imported
            .iter()
            .find(|e| e.description == "Addresses / HP (quoted module)")
            .expect("group path should prefix the flattened description");
        assert_eq!(
            grouped.base,
            AddressExpr::ModuleRelative {
                module: "game.exe".to_string(),
                offset: 0x1000,
            }
        );

        let unquoted = report
            .imported
            .iter()
            .find(|e| e.description == "Addresses / Ammo (unquoted module)")
            .expect("unquoted module+offset form should import too");
        assert_eq!(
            unquoted.base,
            AddressExpr::ModuleRelative {
                module: "game.exe".to_string(),
                offset: 0x2000,
            }
        );

        let absolute = report
            .imported
            .iter()
            .find(|e| e.description == "Byte at an absolute address")
            .expect("a bare absolute address should import");
        assert_eq!(absolute.base, AddressExpr::Absolute(0x7FF6_A8EA_7000));

        let single_pointer = report
            .imported
            .iter()
            .find(|e| e.description == "Single-level pointer")
            .expect("exactly one Offset should import as pointer_offset");
        assert_eq!(single_pointer.pointer_offset, Some(0x10));

        let was_frozen = report
            .imported
            .iter()
            .find(|e| e.description == "Was frozen in the source table")
            .expect("an Activated=1 entry should still import");
        assert!(
            !was_frozen.frozen,
            "import must never auto-freeze from LastState - see ImportReport::was_active_in_source"
        );
        assert_eq!(
            report.was_active_in_source,
            vec!["Was frozen in the source table".to_string()]
        );

        let multi_level = report
            .skipped
            .iter()
            .find(|s| s.description == "Multi-level pointer chain")
            .expect("a 3-offset entry should be skipped, not truncated to 1");
        assert!(
            multi_level.reason.contains('3'),
            "reason should mention the real offset count (3), got: {}",
            multi_level.reason
        );

        let bracket = report
            .skipped
            .iter()
            .find(|s| s.description == "Injected pointer expression")
            .expect("a bracket address should be reported, not mis-imported");
        assert!(bracket.reason.contains("symbol/pointer-expression"));

        assert!(
            report
                .skipped
                .iter()
                .any(|s| s.description == "Player name" && s.reason.contains("String"))
        );
        assert!(
            report.skipped.iter().any(
                |s| s.description == "Unknown future type" && s.reason.contains("unrecognized")
            )
        );
    }
}
