//! Core, GUI-free logic for Ferrite: process attach, memory scanning, and
//! read/write primitives.
//!
//! This crate intentionally has no GUI dependencies, so it stays unit-testable
//! against a small helper "victim" process rather than requiring the real
//! application. See the project's planning vault (kept alongside this repo)
//! for the full v1 scope and build plan.

/// Abstracts over how a target process's memory is read and written, so a
/// platform other than Windows can be added later without reworking the scan
/// engine built on top of it.
///
/// Not yet implemented — this is the first thing M0 builds.
pub trait MemoryBackend {
    // TODO(M0): attach/detach, read, write.
}
