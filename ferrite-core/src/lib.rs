//! Core, GUI-free logic for Ferrite: process attach, memory scanning, and
//! read/write primitives.
//!
//! This crate intentionally has no GUI dependencies, so it stays unit-testable
//! against a small helper "victim" process rather than requiring the real
//! application. See the project's planning vault (kept alongside this repo)
//! for the full v1 scope and build plan.

pub mod process;
pub mod regions;
pub mod scan;
pub mod scan_value;
pub mod session;

pub use process::{ProcessInfo, list_processes};
pub use regions::MemoryRegion;
pub use scan::{FirstScanResult, ScanMatch, ScanOptions, first_scan_exact, scan_region_exact};
pub use scan_value::{ScanFilter, ScanValue, bytes_match_exact, passes_filter};
pub use session::{AttachError, MemoryError, ProcessSession};
