//! Core, GUI-free logic for Ferrite: process attach, memory scanning, and
//! read/write primitives.
//!
//! This crate intentionally has no GUI dependencies, so it stays unit-testable
//! against a small helper "victim" process rather than requiring the real
//! application. See the project's planning vault (kept alongside this repo)
//! for the full v1 scope and build plan.

pub mod aob;
pub mod freeze;
pub mod modules;
pub mod pointer;
pub mod process;
pub mod regions;
pub mod scan;
pub mod scan_value;
pub mod session;
pub mod table;

pub use aob::{
    AobFilter, AobMatch, AobScanResult, first_scan_aob, format_pattern, next_scan_aob,
    parse_hex_pattern, scan_region_aob,
};
pub use freeze::{DEFAULT_FREEZE_INTERVAL, FreezeHandle};
pub use modules::{ModuleError, ModuleInfo, list_modules, module_base};
pub use pointer::resolve_pointer;
pub use process::{ProcessInfo, list_processes};
pub use regions::MemoryRegion;
pub use scan::{
    FirstScanResult, ScanMatch, ScanOptions, first_scan_exact, next_scan, scan_region_exact,
};
pub use scan_value::{ScanFilter, ScanValue, bytes_match_exact, passes_filter};
pub use session::{AttachError, MemoryError, ProcessSession};
pub use table::{
    AddressExpr, CheatEntry, EntryValue, ResolveError, TableError, load_table, parse_address_expr,
    parse_hex_usize, resolve_address, save_table,
};
