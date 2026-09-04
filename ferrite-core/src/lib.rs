//! Core, GUI-free logic for Ferrite: process attach, memory scanning, and
//! read/write primitives.
//!
//! This crate intentionally has no GUI dependencies, so it stays unit-testable
//! against a small helper "victim" process rather than requiring the real
//! application. See the project's planning vault (kept alongside this repo)
//! for the full v1 scope and build plan.

pub mod aob;
pub mod ct_import;
pub mod freeze;
pub mod icon;
pub mod modules;
pub mod pointer;
pub mod process;
pub mod regions;
pub mod scan;
pub mod scan_value;
pub mod script;
pub mod session;
pub mod table;
pub mod text;

pub use aob::{
    AobFilter, AobMatch, AobScanResult, first_scan_aob, format_pattern, next_scan_aob,
    parse_hex_pattern, scan_region_aob,
};
pub use ct_import::{CtImportError, ImportReport, SkippedEntry, import_ct_file, import_ct_xml};
pub use freeze::{DEFAULT_FREEZE_INTERVAL, FreezeHandle};
pub use icon::{IconRgba, extract_icon_rgba};
pub use modules::{ModuleError, ModuleInfo, ModuleMap, ModuleOffset, list_modules, module_base};
pub use pointer::{MAX_POINTER_CHAIN_DEPTH, resolve_pointer, resolve_pointer_chain};
pub use process::{Arch, ProcessInfo, list_processes};
pub use regions::MemoryRegion;
pub use scan::{
    FirstScanResult, ScanMatch, ScanOptions, first_scan_exact, next_scan, scan_region_exact,
};
pub use scan_value::{ScanFilter, ScanValue, bytes_match_exact, passes_filter};
pub use script::{Block, Script, ScriptError, ScriptKind, Section, parse_script};
pub use session::{AttachError, MemoryError, ProcessSession};
pub use table::{
    AddressExpr, CheatEntry, EntryValue, ResolveError, TableError, load_table, parse_address_expr,
    parse_hex_usize, parse_pointer_offsets, resolve_address, save_table,
};
pub use text::{TextEncoding, decode_text, encode_text};
