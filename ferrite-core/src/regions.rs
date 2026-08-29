//! Enumerates a process's committed, writable memory regions — the address
//! space the scan engine walks.

use core::ffi::c_void;
use std::mem::size_of;

use windows::Win32::System::Memory::{
    MEM_COMMIT, MEMORY_BASIC_INFORMATION, PAGE_EXECUTE_READWRITE, PAGE_EXECUTE_WRITECOPY,
    PAGE_GUARD, PAGE_NOACCESS, PAGE_READWRITE, PAGE_WRITECOPY, VirtualQueryEx,
};

use crate::session::ProcessSession;

/// A single committed, writable, non-guard memory region in the target
/// process, as reported by `VirtualQueryEx`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryRegion {
    pub base_address: usize,
    pub size: usize,
}

impl ProcessSession {
    /// Lists the process's committed, writable memory regions — the region
    /// set the scan engine scans over.
    ///
    /// Excludes guard pages and no-access regions even though their base
    /// protection value might otherwise look writable: `PAGE_GUARD` is a
    /// modifier bit OR'd into `Protect`, not a distinct protection value, so
    /// it has to be masked out rather than compared with equality.
    pub fn writable_regions(&self) -> Vec<MemoryRegion> {
        const WRITABLE_PROTECTIONS: [u32; 4] = [
            PAGE_READWRITE.0,
            PAGE_WRITECOPY.0,
            PAGE_EXECUTE_READWRITE.0,
            PAGE_EXECUTE_WRITECOPY.0,
        ];

        let mut regions = Vec::new();
        let mut address: usize = 0;

        loop {
            let mut info = MEMORY_BASIC_INFORMATION::default();
            // SAFETY: `info` is a valid, correctly-sized out-parameter for
            // the duration of this call.
            let written = unsafe {
                VirtualQueryEx(
                    self.handle(),
                    Some(address as *const c_void),
                    &raw mut info,
                    size_of::<MEMORY_BASIC_INFORMATION>(),
                )
            };
            if written == 0 {
                break; // no more regions - reached the end of the address space
            }

            let protect = info.Protect.0;
            let is_guarded_or_noaccess =
                (protect & PAGE_GUARD.0) != 0 || (protect & !PAGE_GUARD.0) == PAGE_NOACCESS.0;
            let base_protect = protect & !PAGE_GUARD.0;
            let is_writable = WRITABLE_PROTECTIONS.contains(&base_protect);

            if info.State == MEM_COMMIT && is_writable && !is_guarded_or_noaccess {
                regions.push(MemoryRegion {
                    base_address: info.BaseAddress as usize,
                    size: info.RegionSize,
                });
            }

            // Advance past this region. Guard against a zero-size or
            // non-advancing report, which would otherwise infinite-loop.
            let Some(next) = (info.BaseAddress as usize).checked_add(info.RegionSize) else {
                break;
            };
            if next <= address {
                break;
            }
            address = next;
        }

        regions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn our_own_process_has_at_least_one_writable_region() {
        // Every process has writable memory (its stack, if nothing else),
        // so this should never come back empty.
        let session =
            ProcessSession::attach(std::process::id()).expect("attaching to our own process");
        let regions = session.writable_regions();
        assert!(
            !regions.is_empty(),
            "expected at least one writable region in our own process"
        );
    }
}
