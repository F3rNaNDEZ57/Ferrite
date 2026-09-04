//! Imports a Cheat Engine `.CT` table (plain XML) into our own [`CheatEntry`]
//! list. Schema verified against real tables and Cheat Engine's own source
//! (`MemoryRecordUnit.pas`/`CEFuncProc.pas` — see the vault's `v0.1-plan.md`
//! for the full research), not guessed.
//!
//! Entries this can't represent (Lua scripts, structure dissect, bit-field
//! and custom types, codepage strings, over-deep pointer chains,
//! unrecognized types, symbol/pointer-expression addresses)
//! go into a visible [`ImportReport`] rather than being silently dropped or
//! mis-imported, per the vault's `v0.1-scope.md`.

use std::path::Path;

use serde::Deserialize;

use crate::pointer::MAX_POINTER_CHAIN_DEPTH;
use crate::scan_value::ScanValue;
use crate::script::{ScriptKind, parse_script};
use crate::table::{CheatEntry, EntryValue, parse_address_expr, parse_hex_usize};
use crate::text::TextEncoding;

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
    // The four `vtString` child elements Cheat Engine reads and writes
    // (`MemoryRecordUnit.pas`). Child *elements* with "1"/"0" text content,
    // not attributes - checked against CE's own read and write paths rather
    // than assumed, since the `@Activated` shape next door proves that
    // guessing this wrong fails silently as a `None` that looks like a
    // legitimate default.
    #[serde(rename = "Length", default)]
    length: Option<String>,
    #[serde(rename = "Unicode", default)]
    unicode: Option<String>,
    #[serde(rename = "ZeroTerminate", default)]
    zero_terminate: Option<String>,
    #[serde(rename = "CodePage", default)]
    code_page: Option<String>,
    // Parsed but never executed. Ferrite only ever touches data, never
    // code; this exists so a skipped script entry can be *read* by the
    // person deciding whether they trust it, which is exactly the decision
    // a downloaded table with an embedded script asks them to make. See the
    // vault's `v0.2-scope.md` for why executing these is out of scope and
    // not a planned follow-on.
    #[serde(rename = "AssemblerScript", default)]
    assembler_script: Option<String>,
    #[serde(rename = "ByteLength", default)]
    byte_length: Option<String>,
    #[serde(rename = "ShowAsHex", default)]
    show_as_hex: Option<String>,
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
/// visible report `v0.1-scope.md` requires, not just a log line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedEntry {
    pub description: String,
    pub reason: String,
    /// The entry's raw `<AssemblerScript>` text, for `Auto Assembler
    /// Script` entries only. Display-only: it exists so a user can read
    /// what a table's script *would* have done and judge it by hand.
    /// Nothing here executes it, and this is not a step toward that.
    pub script_text: Option<String>,
    /// What kind of script it is, where there is one — see
    /// [`crate::script`]. Present so the report can distinguish a script
    /// Ferrite could never run from one it merely doesn't run *yet*, which
    /// are very different things to tell someone.
    pub script_kind: Option<ScriptKind>,
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
    /// default, not a cosmetic one - see the vault's `v0.1-plan.md`. This list
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
            Err(reason) => {
                let script_text = script_text_of(entry);
                // Classify before reporting, so the reason can name what
                // the script actually is rather than only its type string.
                let script_kind = script_text
                    .as_deref()
                    .map(|text| parse_script(text).map(|s| s.kind()));
                // Where the script was classified, the classification *is*
                // the reason. Prefixing it with the generic "Auto Assembler
                // Script entries aren't supported" adds nothing and is
                // actively misleading for a data-only Lua entry, which is
                // not an assembler script in any sense that matters.
                let reason = match &script_kind {
                    Some(Ok(kind)) => {
                        let reason = kind.reason();
                        let mut chars = reason.chars();
                        match chars.next() {
                            Some(first) => {
                                first.to_uppercase().collect::<String>() + chars.as_str()
                            }
                            None => reason.to_string(),
                        }
                    }
                    // An unparseable script is worth saying so about: it
                    // means Cheat Engine would reject it too.
                    Some(Err(err)) => format!("The script couldn't be read — {err}."),
                    None => reason,
                };
                report.skipped.push(SkippedEntry {
                    description: full_description,
                    reason,
                    script_text,
                    script_kind: script_kind.and_then(Result::ok),
                });
            }
        }
    }
}

/// The raw script text of an `Auto Assembler Script` entry, if it has any.
///
/// Restricted to that one type on purpose rather than surfacing whatever
/// `<AssemblerScript>` happens to be present: it's the type whose *whole
/// content* is the script, so it's the only one where showing the text
/// explains the skip rather than adding noise to it.
fn script_text_of(entry: &CheatEntryXml) -> Option<String> {
    // Case-insensitively, like every other type comparison since v0.3.0 —
    // this one was left exact by mistake, so a table writing
    // `auto assembler script` in lower case was classified as a script
    // entry and then had its script text dropped.
    let is_script = entry
        .variable_type
        .as_deref()
        .is_some_and(|t| t.trim().eq_ignore_ascii_case("auto assembler script"));
    if !is_script {
        return None;
    }
    entry
        .assembler_script
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
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
    // Matched case-insensitively and trimmed, exactly as Cheat Engine's own
    // `StringToVariableType` does (`s := trim(lowercase(s))`). Only the
    // *matching* is normalized - messages keep the original text, so an
    // unrecognized type is reported as it was actually written.
    let type_key = variable_type.trim().to_ascii_lowercase();

    // These two are resolved here rather than in `map_variable_type`: their
    // size comes from a sibling element (`<Length>`/`<Unicode>`,
    // `<ByteLength>`) that a function seeing only the type string can't
    // reach.
    let value = match type_key.as_str() {
        "string" | "unicode string" => import_string_value(entry, variable_type)?,
        "array of byte" => import_byte_array_value(entry)?,
        _ => map_variable_type(&type_key, variable_type)?,
    };

    // `<ShowAsHex>` is read before `<VariableType>` is applied in Cheat
    // Engine's own loader, and `setVarType` then forces it on for a
    // `Pointer` entry - so the type wins over the element, not the other
    // way round.
    let show_as_hex = type_key == "pointer" || element_flag(&entry.show_as_hex) == Some(true);

    let address_text = entry
        .address
        .as_deref()
        .ok_or_else(|| "no Address".to_string())?;
    // Bracket/symbol expressions (`[players]`, `[Ns::Sym+18]`) would
    // otherwise slip past `parse_address_expr` as a mangled but
    // technically-parseable module name (e.g. module `"[game.exe"`) that
    // can never resolve - an honest report line beats a perpetual, cryptic
    // "module not found" for an address form v1 was never going to support
    // anyway. See the vault's `v0.1-plan.md`.
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
    if offsets.len() > MAX_POINTER_CHAIN_DEPTH {
        return Err(format!(
            "pointer chain of {} offsets is over the {MAX_POINTER_CHAIN_DEPTH}-level limit",
            offsets.len()
        ));
    }
    // Kept in document order - the order Cheat Engine both reads and writes
    // them in. The last-to-first walk lives in `resolve_pointer_chain`, not
    // in how they're stored, so there's nothing to reverse here.
    let pointer_offsets = offsets
        .iter()
        .map(|text| parse_hex_usize(text).ok_or_else(|| format!("invalid offset {text:?}")))
        .collect::<Result<Vec<_>, _>>()?;

    Ok(CheatEntry {
        description: String::new(), // filled in by the caller
        base,
        pointer_offsets,
        value,
        frozen: false, // see ImportReport::was_active_in_source
        show_as_hex,
    })
}

/// The longest buffer this will import from a declared length - `<Length>`
/// in characters for a string, `<ByteLength>` in bytes for an array of
/// byte. Real entries are names, labels and short signatures: tens of
/// bytes, not thousands. A cap keeps a malformed or adversarial `.CT` from
/// turning one declared length into a multi-gigabyte allocation and a read
/// of the same size on every refresh tick; same reasoning as the
/// pointer-chain depth cap.
const MAX_BUFFER_LENGTH: usize = 4096;

/// Builds a zero-filled string buffer of the width a `.CT` entry declares.
///
/// Defaults here are Cheat Engine's own, taken from `setVarType`
/// (`MemoryRecordUnit.pas`) rather than from what looks natural: setting
/// either string type turns `ZeroTerminate` **on**, and `Unicode String`
/// additionally turns `unicode` on before folding itself into a plain
/// `vtString`. The XML read path then overrides each only when the element
/// is actually present - so an absent `<ZeroTerminate>` means *true*, not
/// false, and an explicit `<Unicode>0</Unicode>` demotes even a
/// `Unicode String` entry.
fn import_string_value(entry: &CheatEntryXml, variable_type: &str) -> Result<EntryValue, String> {
    // `<CodePage>1</CodePage>` is a third encoding (Windows codepage text,
    // which CE round-trips through `UTF8ToWinCP`), not a flavor of Latin-1.
    // Reported rather than decoded as something it isn't.
    if element_flag(&entry.code_page) == Some(true) {
        return Err(
            "String entries with <CodePage>1</CodePage> aren't supported (only Latin-1 and UTF-16)"
                .to_string(),
        );
    }

    let encoding = match element_flag(&entry.unicode) {
        Some(true) => TextEncoding::Utf16Le,
        Some(false) => TextEncoding::Latin1,
        None if variable_type == "Unicode String" => TextEncoding::Utf16Le,
        None => TextEncoding::Latin1,
    };
    let zero_terminated = element_flag(&entry.zero_terminate).unwrap_or(true);

    // Decimal, not hex, unlike `<Offset>` - CE parses it with `strtoint`.
    let length_text = entry
        .length
        .as_deref()
        .ok_or_else(|| format!("{variable_type} entry has no <Length>"))?;
    let length: usize = length_text
        .trim()
        .parse()
        .map_err(|_| format!("invalid <Length> {length_text:?}"))?;
    if length == 0 {
        return Err(format!("{variable_type} entry declares <Length>0</Length>"));
    }
    if length > MAX_BUFFER_LENGTH {
        return Err(format!(
            "{variable_type} entry declares <Length>{length}</Length>, over the {MAX_BUFFER_LENGTH}-character limit"
        ));
    }

    // `<Length>` counts characters; the buffer is twice that in bytes for
    // UTF-16 (CE's own `getByteSize`). Zero-filled, like every other
    // imported value - the real contents arrive on the first live resolve.
    Ok(EntryValue::Text {
        bytes: vec![0; length * encoding.bytes_per_char()],
        encoding,
        zero_terminated,
    })
}

/// Builds a zero-filled buffer of the width an `Array of byte` entry
/// declares in `<ByteLength>` (decimal, like `<Length>`).
///
/// Imported as [`EntryValue::Bytes`] rather than a new variant: it *is* the
/// AOB shape the scan side already produces, so it displays as a hex
/// pattern that can be pasted straight back into the AOB search box.
fn import_byte_array_value(entry: &CheatEntryXml) -> Result<EntryValue, String> {
    let text = entry
        .byte_length
        .as_deref()
        .ok_or_else(|| "Array of byte entry has no <ByteLength>".to_string())?;
    let length: usize = text
        .trim()
        .parse()
        .map_err(|_| format!("invalid <ByteLength> {text:?}"))?;
    if length == 0 {
        return Err("Array of byte entry declares <ByteLength>0</ByteLength>".to_string());
    }
    if length > MAX_BUFFER_LENGTH {
        return Err(format!(
            "Array of byte entry declares <ByteLength>{length}</ByteLength>, over the {MAX_BUFFER_LENGTH}-byte limit"
        ));
    }
    Ok(EntryValue::Bytes(vec![0; length]))
}

/// Reads one of Cheat Engine's `"1"`/`"0"` flag elements: `None` when the
/// element is absent (so the caller can apply CE's own default rather than
/// a made-up one), and otherwise true only for exactly `"1"`, matching CE's
/// `tempnode.TextContent='1'` comparison.
fn element_flag(value: &Option<String>) -> Option<bool> {
    value.as_deref().map(|v| v.trim() == "1")
}

/// Maps a `.CT` `<VariableType>` to a zero-valued [`EntryValue`] of the
/// matching shape (the real value is filled in on first live resolve, same
/// as any other saved-list entry). `key` is the trimmed, lowercased type
/// string; `original` is what the file actually said, used only in
/// messages so a report names the text a user would find if they searched
/// for it.
///
/// Authoritative string table from Cheat Engine's own
/// `VariableTypeToString`/`StringToVariableType` (`CEFuncProc.pas`) - see
/// the vault's `v0.1-plan.md`. Unrecognized is always reported, never
/// silently guessed - unlike `StringToVariableType`, which falls back to
/// `vtByte` for anything it doesn't recognize.
fn map_variable_type(key: &str, original: &str) -> Result<EntryValue, String> {
    match key {
        "byte" => Ok(EntryValue::Scalar(ScanValue::I8(0))),
        "2 bytes" => Ok(EntryValue::Scalar(ScanValue::I16(0))),
        "4 bytes" => Ok(EntryValue::Scalar(ScanValue::I32(0))),
        "8 bytes" => Ok(EntryValue::Scalar(ScanValue::I64(0))),
        "float" => Ok(EntryValue::Scalar(ScanValue::F32(0.0))),
        "double" => Ok(EntryValue::Scalar(ScanValue::F64(0.0))),
        // A pointer is an address-sized integer shown in hex - that's all
        // `vtPointer` is once Cheat Engine's own `setVarType` is done with
        // it (it rewrites the type to vtQword/vtDword and turns ShowAsHex
        // on). Ferrite is 64-bit only, so it's always the 8-byte form.
        "pointer" => Ok(EntryValue::Scalar(ScanValue::I64(0))),
        // Intercepted by `import_leaf_entry` before this is reached, since a
        // string's shape depends on sibling elements, not just its type
        // name. Kept as an explicit arm rather than left to fall into
        // `other` so that removing that interception surfaces as a
        // conspicuous internal error instead of reporting a real Cheat
        // Engine type as "unrecognized".
        "string" | "unicode string" | "array of byte" => Err(format!(
            "internal: {original} entries are handled by import_leaf_entry, not map_variable_type"
        )),
        "auto assembler script" => {
            Err("Auto Assembler Script (Lua) entries aren't supported".to_string())
        }
        "custom" => Err(
            "Custom-type entries aren't supported (require Lua conversion functions)".to_string(),
        ),
        "binary" => Err("Binary (bit-field) entries aren't supported".to_string()),
        "all" | "grouped" => Err(format!(
            "unexpected VariableType {original:?} on a leaf entry"
        )),
        _ => Err(format!("unrecognized VariableType {original:?}")),
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
    use crate::script::ScriptKind;
    use crate::table::AddressExpr;

    /// These fixtures are synthetic, hand-written to the schema verified in
    /// this session (see the vault's `v0.1-plan.md`) - not copies of the real
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
        // The reason is the classification, not the type name: this entry's
        // whole body is a `{$LUA}` block that only prints, so it is exactly
        // the shape a data-only interpreter could run.
        assert_eq!(
            report.skipped[0].script_kind,
            Some(ScriptKind::DataOnlyLua),
            "reason was: {}",
            report.skipped[0].reason
        );
        assert!(report.skipped[0].reason.contains("data-only Lua"));

        // The script text comes along so a user can read what the entry
        // would have done - display only, never executed.
        let script = report.skipped[0]
            .script_text
            .as_deref()
            .expect("an Auto Assembler Script skip should carry its script text");
        assert!(script.starts_with("{$LUA}"), "got: {script}");
        assert!(script.contains("print(\"hello\")"), "got: {script}");

        // ...and only that type carries one: a String skip is not a script.
        let no_script = import_ct_xml(&one_entry_table(
            "<VariableType>String</VariableType><CodePage>1</CodePage><Length>4</Length>",
        ))
        .expect("parsing");
        assert_eq!(no_script.skipped[0].script_text, None);
    }

    /// Wraps one leaf `<CheatEntry>` body in the minimum table around it,
    /// for the string cases that don't warrant their own fixture file.
    fn one_entry_table(body: &str) -> String {
        format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>
             <CheatTable><CheatEntries><CheatEntry>
             <Description>\"S\"</Description><Address>\"game.exe\"+1000</Address>
             {body}
             </CheatEntry></CheatEntries></CheatTable>"
        )
    }

    #[test]
    fn a_codepage_string_is_reported_rather_than_decoded_as_latin1() {
        let report = import_ct_xml(&one_entry_table(
            "<VariableType>String</VariableType><Length>10</Length><CodePage>1</CodePage>",
        ))
        .expect("parsing");
        assert_eq!(report.imported.len(), 0);
        assert!(
            report.skipped[0].reason.contains("CodePage"),
            "got: {}",
            report.skipped[0].reason
        );
    }

    #[test]
    fn a_string_without_a_length_is_reported_rather_than_imported_empty() {
        let report = import_ct_xml(&one_entry_table("<VariableType>String</VariableType>"))
            .expect("parsing");
        assert_eq!(report.imported.len(), 0);
        assert!(
            report.skipped[0].reason.contains("<Length>"),
            "got: {}",
            report.skipped[0].reason
        );
    }

    #[test]
    fn an_absurd_length_is_capped_rather_than_allocated() {
        // A malformed or adversarial table must not turn one declared
        // length into a multi-gigabyte buffer that every refresh tick then
        // re-reads.
        let report = import_ct_xml(&one_entry_table(
            "<VariableType>String</VariableType><Length>999999999</Length>",
        ))
        .expect("parsing");
        assert_eq!(report.imported.len(), 0);
        assert!(
            report.skipped[0].reason.contains("limit"),
            "got: {}",
            report.skipped[0].reason
        );
    }

    #[test]
    fn a_length_is_parsed_as_decimal_not_hex() {
        // Unlike <Offset>, which is hex. CE reads <Length> with strtoint,
        // so <Length>20</Length> is twenty characters, not thirty-two.
        let report = import_ct_xml(&one_entry_table(
            "<VariableType>String</VariableType><Length>20</Length>",
        ))
        .expect("parsing");
        assert_eq!(report.imported[0].value.to_le_bytes().len(), 20);
    }

    #[test]
    fn an_over_deep_pointer_chain_is_reported_rather_than_walked() {
        let offsets = "<Offset>10</Offset>".repeat(MAX_POINTER_CHAIN_DEPTH + 1);
        let report = import_ct_xml(&one_entry_table(&format!(
            "<VariableType>4 Bytes</VariableType><Offsets>{offsets}</Offsets>"
        )))
        .expect("parsing");
        assert_eq!(report.imported.len(), 0);
        assert!(
            report.skipped[0].reason.contains("limit"),
            "got: {}",
            report.skipped[0].reason
        );
    }

    #[test]
    fn a_chain_exactly_at_the_depth_limit_still_imports() {
        let offsets = "<Offset>10</Offset>".repeat(MAX_POINTER_CHAIN_DEPTH);
        let report = import_ct_xml(&one_entry_table(&format!(
            "<VariableType>4 Bytes</VariableType><Offsets>{offsets}</Offsets>"
        )))
        .expect("parsing");
        assert_eq!(
            report.imported[0].pointer_offsets.len(),
            MAX_POINTER_CHAIN_DEPTH
        );
    }

    #[test]
    fn an_array_of_byte_without_a_bytelength_is_reported() {
        let report = import_ct_xml(&one_entry_table(
            "<VariableType>Array of byte</VariableType>",
        ))
        .expect("parsing");
        assert_eq!(report.imported.len(), 0);
        assert!(
            report.skipped[0].reason.contains("<ByteLength>"),
            "got: {}",
            report.skipped[0].reason
        );
    }

    #[test]
    fn an_absurd_bytelength_is_capped_rather_than_allocated() {
        let report = import_ct_xml(&one_entry_table(
            "<VariableType>Array of byte</VariableType><ByteLength>999999999</ByteLength>",
        ))
        .expect("parsing");
        assert_eq!(report.imported.len(), 0);
        assert!(
            report.skipped[0].reason.contains("limit"),
            "got: {}",
            report.skipped[0].reason
        );
    }

    #[test]
    fn an_unrecognized_type_is_reported_with_the_text_the_file_used() {
        // Matching is case-insensitive, but the *message* has to quote the
        // original so a user can search their table for it.
        let report = import_ct_xml(&one_entry_table(
            "<VariableType>Something CE Added Later</VariableType>",
        ))
        .expect("parsing");
        assert!(
            report.skipped[0]
                .reason
                .contains("Something CE Added Later"),
            "got: {}",
            report.skipped[0].reason
        );
    }

    #[test]
    fn a_pointer_entry_stays_hex_even_with_showashex_switched_off() {
        // CE reads <ShowAsHex> *before* it applies <VariableType>, and
        // setVarType then forces hex on for a pointer - so the type wins.
        let report = import_ct_xml(&one_entry_table(
            "<VariableType>Pointer</VariableType><ShowAsHex>0</ShowAsHex>",
        ))
        .expect("parsing");
        assert!(report.imported[0].show_as_hex);
    }

    #[test]
    fn the_script_fixture_classifies_every_shape() {
        let report = import_ct_xml(&load_fixture("scripts.CT")).expect("parsing scripts.CT");

        // Nothing runs, and nothing imports: no interpreter exists yet, and
        // an Auto Assembler entry has no address to import as a value.
        assert_eq!(report.imported.len(), 0);
        assert_eq!(report.skipped.len(), 7, "{:#?}", report.skipped);

        let by = |description: &str| {
            report
                .skipped
                .iter()
                .find(|s| s.description == description)
                .unwrap_or_else(|| panic!("{description:?} should be skipped"))
        };

        let expected = [
            ("Infinite health (data only)", Some(ScriptKind::DataOnlyLua)),
            (
                "Damage multiplier (generates code)",
                Some(ScriptKind::GenerativeLua),
            ),
            ("God mode (no damage)", Some(ScriptKind::Assembler)),
            (
                "Ammo hook (Lua helper plus assembly)",
                Some(ScriptKind::Assembler),
            ),
            ("Placeholder (comments only)", Some(ScriptKind::Empty)),
            // Cheat Engine rejects two [ENABLE] sections outright, so this
            // one has no classification at all.
            ("Malformed (two enable sections)", None),
            ("Lower-case type name", Some(ScriptKind::DataOnlyLua)),
        ];
        for (description, kind) in expected {
            assert_eq!(
                by(description).script_kind,
                kind,
                "wrong classification for {description:?}"
            );
        }

        // Only the data-only ones could ever run, and the reason says which
        // is which rather than reporting them all as "not supported".
        assert!(
            by("God mode (no damage)")
                .reason
                .contains("patching its execution")
        );
        assert!(
            by("Damage multiplier (generates code)")
                .reason
                .contains("partly modified")
        );
        assert!(
            by("Malformed (two enable sections)")
                .reason
                .contains("more than one [ENABLE]")
        );

        // Every one of them keeps its script text, including the
        // lower-case-typed entry that used to lose it.
        for (description, _) in expected {
            assert!(
                by(description).script_text.is_some(),
                "{description:?} lost its script text"
            );
        }
    }

    #[test]
    fn basic_table_imports_supported_entries_and_reports_the_rest() {
        let report =
            import_ct_xml(&load_fixture("basic_table.CT")).expect("parsing basic_table.CT");

        // Imported: the two grouped entries, the absolute byte, the
        // single-level pointer, the was-activated entry, the four string
        // entries, the three-level pointer chain, the array of byte, the
        // Pointer, the ShowAsHex entry, and the lowercase-typed one = 14.
        assert_eq!(
            report.imported.len(),
            14,
            "unexpected imported set: {:#?}",
            report.imported
        );
        // Skipped: bracket address, unknown type = 2.
        assert_eq!(
            report.skipped.len(),
            2,
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
            .expect("one Offset should import as a one-element chain");
        assert_eq!(single_pointer.pointer_offsets, vec![0x10]);

        // Document order, not reversed on read: the fixture lists 10, 20,
        // 30 and that's how they're stored. The last-to-first *walk* is
        // `resolve_pointer_chain`'s business, and reversing here as well
        // would cancel it out into a chain that resolves silently to the
        // wrong address.
        let multi_level = report
            .imported
            .iter()
            .find(|e| e.description == "Multi-level pointer chain")
            .expect("a 3-offset entry should import now, not be skipped");
        assert_eq!(multi_level.pointer_offsets, vec![0x10, 0x20, 0x30]);

        let entry = |description: &str| {
            report
                .imported
                .iter()
                .find(|e| e.description == description)
                .unwrap_or_else(|| panic!("{description:?} should import"))
        };

        // `<ByteLength>` is a byte count, decimal - the AOB shape the scan
        // side already produces, so it displays as a pasteable hex pattern.
        assert_eq!(
            entry("Signature (array of byte)").value,
            EntryValue::Bytes(vec![0; 6])
        );

        // A Pointer is an address-sized integer shown in hex: that's all
        // CE's own setVarType leaves it as. The hex flag comes from the
        // *type*, with no <ShowAsHex> element present at all.
        let pointer = entry("Player base (pointer)");
        assert_eq!(pointer.value, EntryValue::Scalar(ScanValue::I64(0)));
        assert!(
            pointer.show_as_hex,
            "a Pointer entry is hex-displayed by its type, not by an element"
        );

        // ...and <ShowAsHex> works on an ordinary numeric entry too.
        let flags = entry("Flags (shown as hex)");
        assert_eq!(flags.value, EntryValue::Scalar(ScanValue::I32(0)));
        assert!(flags.show_as_hex);
        assert!(
            !entry("Byte at an absolute address").show_as_hex,
            "absent <ShowAsHex> means off"
        );

        // Type names match case-insensitively, as CE's own
        // StringToVariableType does (`s := trim(lowercase(s))`).
        assert_eq!(
            entry("Lowercase type name").value,
            EntryValue::Scalar(ScanValue::I32(0))
        );

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

        let bracket = report
            .skipped
            .iter()
            .find(|s| s.description == "Injected pointer expression")
            .expect("a bracket address should be reported, not mis-imported");
        assert!(bracket.reason.contains("symbol/pointer-expression"));

        // All four string shapes the fixture covers, each asserted for the
        // buffer width and flags it should end up with - not just for
        // having been imported. Widths are the decisive part: <Length> is a
        // character count, so a unicode entry's buffer is twice its
        // declared length, and getting that backwards would silently import
        // half a string and write that truncated buffer back on every
        // freeze tick.
        let string_entry = |description: &str| {
            report
                .imported
                .iter()
                .find(|e| e.description == description)
                .unwrap_or_else(|| panic!("{description:?} should import"))
                .value
                .clone()
        };

        // <Length>20</Length>, no <Unicode>, no <ZeroTerminate>: Latin-1,
        // 20 bytes, and zero-terminated by CE's own setVarType default -
        // the absent-element case, where a made-up `false` default would
        // have been wrong.
        assert_eq!(
            string_entry("Player name"),
            EntryValue::Text {
                bytes: vec![0; 20],
                encoding: TextEncoding::Latin1,
                zero_terminated: true,
            }
        );
        // <Length>16</Length> + <Unicode>1</Unicode>: 32 bytes, and an
        // explicit <ZeroTerminate>0</ZeroTerminate> overriding the default.
        assert_eq!(
            string_entry("Player name (unicode)"),
            EntryValue::Text {
                bytes: vec![0; 32],
                encoding: TextEncoding::Utf16Le,
                zero_terminated: false,
            }
        );
        assert_eq!(
            string_entry("Zone name (zero-terminated)"),
            EntryValue::Text {
                bytes: vec![0; 12],
                encoding: TextEncoding::Latin1,
                zero_terminated: true,
            }
        );
        // The distinct "Unicode String" VariableType, with neither element
        // present: CE's setVarType turns on both unicode and ZeroTerminate
        // before folding it into a plain vtString.
        assert_eq!(
            string_entry("Clan tag (Unicode String type)"),
            EntryValue::Text {
                bytes: vec![0; 16],
                encoding: TextEncoding::Utf16Le,
                zero_terminated: true,
            }
        );

        assert!(
            report.skipped.iter().any(
                |s| s.description == "Unknown future type" && s.reason.contains("unrecognized")
            )
        );
    }
}
