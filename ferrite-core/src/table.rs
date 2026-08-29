//! The entry model for a saved "cheat table" (see the vault's
//! `v1-scope.md`) and its plain-JSON save/load — our own portable format,
//! not a Cheat Engine `.CT` file (that's a separate importer, not built
//! here; see the vault's `v1-plan.md`).

use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::modules::{ModuleError, module_base};
use crate::pointer::resolve_pointer;
use crate::scan_value::ScanValue;
use crate::session::{MemoryError, ProcessSession};

/// How a saved entry's address is expressed.
///
/// Verified against real Cheat Engine tables (see the vault's `v1-plan.md`):
/// nearly every real-world entry is module-relative, not absolute — an
/// absolute address only really survives across the same process's own
/// relaunches within a single boot (ASLR bias is chosen once per image per
/// boot, not per launch), and not at all across a reboot or on a different
/// machine. Both forms are kept as first-class here rather than treating
/// absolute as the default and module-relative as an add-on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AddressExpr {
    Absolute(usize),
    ModuleRelative { module: String, offset: usize },
}

/// A saved entry's value: either a numeric [`ScanValue`], or a fixed-length
/// byte array (an AOB-shaped entry — mirrors [`crate::aob::AobMatch`]'s
/// shape rather than pretending to be numeric).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum EntryValue {
    Scalar(ScanValue),
    Bytes(Vec<u8>),
}

impl EntryValue {
    /// This value's little-endian byte representation — what freeze pins an
    /// address to.
    pub fn to_le_bytes(&self) -> Vec<u8> {
        match self {
            Self::Scalar(v) => v.to_le_bytes(),
            Self::Bytes(bytes) => bytes.clone(),
        }
    }
}

/// One entry in a saved cheat table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CheatEntry {
    pub description: String,
    pub base: AddressExpr,
    /// `Some(offset)`: dereference `base` as a pointer-sized value, add
    /// `offset` — the single-level pointer chain v1 supports (see the
    /// vault's `v1-plan.md` for why this is exactly one dereference, not a
    /// chain of any length).
    pub pointer_offset: Option<usize>,
    /// The last-known/saved value. Also what a frozen entry is pinned to
    /// immediately on load — deterministic "restore this cheat" behavior,
    /// not a re-read of whatever's currently at the address (a decided
    /// choice, see the vault's `v1-plan.md`).
    pub value: EntryValue,
    pub frozen: bool,
}

/// Why [`resolve_address`] failed to resolve an entry's live address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveError {
    /// A `ModuleRelative` entry's module isn't loaded in the attached
    /// process — e.g. the table was saved against a different game, or the
    /// wrong process is currently attached. Carries the module name that
    /// wasn't found, so a caller can show it rather than a bare failure.
    ModuleNotFound(String),
    Memory(MemoryError),
}

impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ModuleNotFound(module) => write!(f, "module {module:?} isn't loaded"),
            Self::Memory(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for ResolveError {}

/// Resolves a saved entry's current address in the attached process.
///
/// `Absolute` entries always attempt resolution regardless of which process
/// is attached (they might be nonsense if it's the wrong process, but
/// that's the caller's call to make, not this function's — matches Cheat
/// Engine's own permissiveness here, see the vault's `v1-plan.md`).
pub fn resolve_address(
    entry: &CheatEntry,
    session: &ProcessSession,
) -> Result<usize, ResolveError> {
    let base = match &entry.base {
        AddressExpr::Absolute(address) => *address,
        AddressExpr::ModuleRelative { module, offset } => {
            let base = module_base(session, module).map_err(|err| match err {
                ModuleError::NotFound => ResolveError::ModuleNotFound(module.clone()),
                ModuleError::Memory(mem_err) => ResolveError::Memory(mem_err),
            })?;
            base + offset
        }
    };

    match entry.pointer_offset {
        Some(offset) => resolve_pointer(session, base, offset).map_err(ResolveError::Memory),
        None => Ok(base),
    }
}

/// Parses an address expression as typed by a user (or, later, an imported
/// `.CT` file — this parser is written to be reused there): either a bare
/// hex address (`"7FF6A8EA7000"`, an optional `0x`/`0X` prefix accepted for
/// how people naturally type addresses, though real `.CT` files never
/// include one), or `module.exe+HEX`/`"module.exe"+HEX` (surrounding quotes
/// on the module name optional — both forms appear in real Cheat Engine
/// tables).
pub fn parse_address_expr(text: &str) -> Result<AddressExpr, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("enter an address, e.g. 7FF6A8EA7000 or \"game.exe\"+1000".to_string());
    }

    if let Some((module, offset_text)) = text.rsplit_once('+') {
        let module = module.trim().trim_matches('"').trim();
        if module.is_empty() {
            return Err("missing a module name before '+'".to_string());
        }
        let offset = parse_hex_usize(offset_text)
            .ok_or_else(|| format!("'{offset_text}' isn't a valid hex offset"))?;
        return Ok(AddressExpr::ModuleRelative {
            module: module.to_string(),
            offset,
        });
    }

    let absolute = parse_hex_usize(text)
        .ok_or_else(|| format!("'{text}' isn't a valid address or module+offset expression"))?;
    Ok(AddressExpr::Absolute(absolute))
}

/// Parses a hex offset as typed by a user: `"70"`, `"0x70"`, `"BD0"` — real
/// Cheat Engine `<Offset>` values never carry a `0x` prefix, but a
/// hand-typed one might.
pub fn parse_hex_usize(text: &str) -> Option<usize> {
    let text = text.trim();
    let text = text
        .strip_prefix("0x")
        .or_else(|| text.strip_prefix("0X"))
        .unwrap_or(text);
    usize::from_str_radix(text, 16).ok()
}

/// Why saving or loading a table failed.
#[derive(Debug)]
pub enum TableError {
    Io(std::io::Error),
    Json(serde_json::Error),
}

impl std::fmt::Display for TableError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(err) => write!(f, "{err}"),
            Self::Json(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for TableError {}

impl From<std::io::Error> for TableError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}

impl From<serde_json::Error> for TableError {
    fn from(err: serde_json::Error) -> Self {
        Self::Json(err)
    }
}

/// Saves `entries` to `path` as pretty-printed JSON — diffable and
/// human-readable, per the vault's `v1-scope.md`, not a new binary format.
pub fn save_table(path: &Path, entries: &[CheatEntry]) -> Result<(), TableError> {
    let file = File::create(path)?;
    serde_json::to_writer_pretty(BufWriter::new(file), entries)?;
    Ok(())
}

/// Loads a previously-saved table. Never resolves addresses itself — that's
/// [`resolve_address`]'s job, called per-entry once (if) a process is
/// attached, so a load always succeeds independent of attach state (see the
/// vault's `v1-plan.md` for why loading while detached, or attached to the
/// wrong process, must never silently drop entries).
pub fn load_table(path: &Path) -> Result<Vec<CheatEntry>, TableError> {
    let file = File::open(path)?;
    Ok(serde_json::from_reader(file)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_entries() -> Vec<CheatEntry> {
        vec![
            CheatEntry {
                description: "absolute i32".to_string(),
                base: AddressExpr::Absolute(0x7FF6_A8EA_7000),
                pointer_offset: None,
                value: EntryValue::Scalar(ScanValue::I32(100)),
                frozen: false,
            },
            CheatEntry {
                description: "module-relative pointer".to_string(),
                base: AddressExpr::ModuleRelative {
                    module: "ferrite-victim.exe".to_string(),
                    offset: 0x1000,
                },
                pointer_offset: Some(0x8),
                value: EntryValue::Scalar(ScanValue::F32(1.5)),
                frozen: true,
            },
            CheatEntry {
                description: "byte pattern".to_string(),
                base: AddressExpr::Absolute(0x1234),
                pointer_offset: None,
                value: EntryValue::Bytes(vec![0xDE, 0xAD, 0xBE, 0xEF]),
                frozen: false,
            },
        ]
    }

    #[test]
    fn a_saved_table_loads_back_identical() {
        let entries = sample_entries();
        let path = std::env::temp_dir().join(format!(
            "ferrite-table-roundtrip-{}.json",
            std::process::id()
        ));

        save_table(&path, &entries).expect("saving the table");
        let loaded = load_table(&path).expect("loading the table back");
        std::fs::remove_file(&path).expect("cleaning up the temp file");

        assert_eq!(loaded, entries);
    }

    #[test]
    fn parses_a_bare_hex_address() {
        assert_eq!(
            parse_address_expr("7FF6A8EA7000"),
            Ok(AddressExpr::Absolute(0x7FF6_A8EA_7000))
        );
        assert_eq!(
            parse_address_expr("0x1000"),
            Ok(AddressExpr::Absolute(0x1000))
        );
    }

    #[test]
    fn parses_a_module_relative_address_quoted_or_not() {
        assert_eq!(
            parse_address_expr("\"GTA5.exe\"+01C58DA0"),
            Ok(AddressExpr::ModuleRelative {
                module: "GTA5.exe".to_string(),
                offset: 0x01C5_8DA0,
            })
        );
        assert_eq!(
            parse_address_expr("GTA5.exe+24BB438"),
            Ok(AddressExpr::ModuleRelative {
                module: "GTA5.exe".to_string(),
                offset: 0x24BB438,
            })
        );
    }

    #[test]
    fn rejects_garbage_input() {
        assert!(parse_address_expr("").is_err());
        assert!(parse_address_expr("not an address").is_err());
        assert!(parse_address_expr("+1000").is_err());
    }
}
