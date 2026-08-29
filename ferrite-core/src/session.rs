//! Attach to a running process, as a precursor to reading/writing its memory.
//!
//! Ownership model: a [`ProcessSession`] wraps exactly one open process
//! handle and closes it automatically on drop (RAII); it is intentionally
//! not `Clone`. See the concurrency model in the vault's `v1-plan.md`.

use core::ffi::c_void;

use windows::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, ERROR_INVALID_PARAMETER, HANDLE, STILL_ACTIVE,
};
use windows::Win32::System::Diagnostics::Debug::{ReadProcessMemory, WriteProcessMemory};
use windows::Win32::System::Threading::{
    GetExitCodeProcess, OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_VM_OPERATION,
    PROCESS_VM_READ, PROCESS_VM_WRITE,
};
use windows::core::HRESULT;

/// Why a read or write against attached process memory failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryError {
    /// The OS refused the read/write outright — e.g. the address isn't
    /// mapped, or isn't mapped with the needed protection. Carries the raw
    /// Win32 error code.
    Os(u32),
    /// The OS reported success but transferred fewer bytes than requested,
    /// without an error code explaining why (can happen at the edge of a
    /// mapped region).
    Partial {
        requested: usize,
        transferred: usize,
    },
}

impl std::fmt::Display for MemoryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Os(code) => write!(f, "OS error {code}"),
            Self::Partial {
                requested,
                transferred,
            } => write!(
                f,
                "only {transferred} of {requested} requested bytes were transferred"
            ),
        }
    }
}

impl std::error::Error for MemoryError {}

impl From<windows::core::Error> for MemoryError {
    fn from(err: windows::core::Error) -> Self {
        Self::Os(err.code().0 as u32)
    }
}

/// Why [`ProcessSession::attach`] failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachError {
    /// No process with that PID exists — it may have already exited, or
    /// never existed.
    ProcessNotFound,
    /// The process exists, but we don't have permission to open it with the
    /// access rights we need. Running Ferrite as Administrator is the usual
    /// fix.
    AccessDenied,
    /// Some other OS error; carries the raw Win32 error code.
    Other(u32),
}

impl std::fmt::Display for AttachError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ProcessNotFound => write!(f, "no process with that PID exists"),
            Self::AccessDenied => {
                write!(f, "access denied — try running Ferrite as Administrator")
            }
            Self::Other(code) => write!(f, "OS error {code}"),
        }
    }
}

impl std::error::Error for AttachError {}

impl From<windows::core::Error> for AttachError {
    fn from(err: windows::core::Error) -> Self {
        let code = err.code();
        if code == HRESULT::from_win32(ERROR_ACCESS_DENIED.0) {
            Self::AccessDenied
        } else if code == HRESULT::from_win32(ERROR_INVALID_PARAMETER.0) {
            // OpenProcess reports a nonexistent PID as ERROR_INVALID_PARAMETER,
            // not a dedicated "not found" code.
            Self::ProcessNotFound
        } else {
            Self::Other(code.0 as u32)
        }
    }
}

/// An open handle to a running process. Closes the handle automatically on
/// drop; not `Clone` — only one session is meant to be active at a time (see
/// the concurrency model in the vault's `v1-plan.md`).
pub struct ProcessSession {
    handle: HANDLE,
    pid: u32,
}

// SAFETY: `HANDLE` wraps a raw pointer, so `ProcessSession` isn't Send/Sync
// by default. The only operations it performs on that handle -
// ReadProcessMemory, WriteProcessMemory, GetExitCodeProcess - are documented
// safe to call concurrently, from any thread, on the same handle. This lets
// the freeze thread (`freeze.rs`) share one session with the GUI thread via
// `Arc<ProcessSession>` (`Arc<T>: Send` requires `T: Send + Sync`) - see the
// concurrency model in the vault's `v1-plan.md`.
unsafe impl Send for ProcessSession {}
unsafe impl Sync for ProcessSession {}

impl ProcessSession {
    /// Opens `pid` with exactly the access rights v1 needs — not
    /// `PROCESS_ALL_ACCESS` — per the vault's `v1-scope.md`.
    pub fn attach(pid: u32) -> Result<Self, AttachError> {
        let access =
            PROCESS_VM_READ | PROCESS_VM_WRITE | PROCESS_VM_OPERATION | PROCESS_QUERY_INFORMATION;

        // SAFETY: OpenProcess is a plain FFI call with no preconditions
        // beyond a valid access-rights value, which `access` is. The
        // returned HANDLE becomes owned by this ProcessSession and is
        // closed exactly once, in `Drop`.
        let handle = unsafe { OpenProcess(access, false, pid) }?;
        Ok(Self { handle, pid })
    }

    /// The PID this session is attached to.
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// The raw handle, for other `ferrite-core` modules (e.g. region
    /// enumeration) that need to call Win32 APIs this struct doesn't wrap
    /// itself. Deliberately not `pub` — callers outside this crate only ever
    /// see `ProcessSession`'s own methods.
    pub(crate) fn handle(&self) -> HANDLE {
        self.handle
    }

    /// Cheaply checks whether the attached process has actually terminated,
    /// via `GetExitCodeProcess` (needs only `PROCESS_QUERY_INFORMATION`,
    /// which `attach` already requests - unlike `WaitForSingleObject`,
    /// which needs `SYNCHRONIZE` and would otherwise silently fail on this
    /// session's handle). `STILL_ACTIVE` means it's still running; anything
    /// else means it has exited. Used by the freeze thread to distinguish
    /// "the target process exited" from "one write to one now-invalid
    /// address failed" before reporting the session as dead.
    pub(crate) fn has_exited(&self) -> bool {
        let mut exit_code = 0u32;
        // SAFETY: `self.handle` is a valid process handle and `exit_code`
        // is a valid, uniquely-owned `u32` for the duration of this call.
        let result = unsafe { GetExitCodeProcess(self.handle, &raw mut exit_code) };
        match result {
            Ok(()) => exit_code != STILL_ACTIVE.0 as u32,
            // Can't determine the exit code at all - don't claim the
            // process exited on the strength of an unrelated OS error.
            Err(_) => false,
        }
    }

    /// Reads `len` bytes from the attached process's memory at `address`.
    pub fn read_bytes(&self, address: usize, len: usize) -> Result<Vec<u8>, MemoryError> {
        let mut buffer = vec![0u8; len];
        let mut bytes_read = 0usize;

        // SAFETY: `buffer` is a valid, uniquely-owned allocation of exactly
        // `len` bytes for the duration of this call.
        unsafe {
            ReadProcessMemory(
                self.handle,
                address as *const c_void,
                buffer.as_mut_ptr().cast::<c_void>(),
                len,
                Some(&raw mut bytes_read),
            )
        }?;

        if bytes_read != len {
            return Err(MemoryError::Partial {
                requested: len,
                transferred: bytes_read,
            });
        }
        Ok(buffer)
    }

    /// Writes `data` to the attached process's memory at `address`.
    pub fn write_bytes(&self, address: usize, data: &[u8]) -> Result<(), MemoryError> {
        let mut bytes_written = 0usize;

        // SAFETY: `data` is valid for reads of `data.len()` bytes for the
        // duration of this call; we only pass it as the source buffer.
        unsafe {
            WriteProcessMemory(
                self.handle,
                address as *const c_void,
                data.as_ptr().cast::<c_void>(),
                data.len(),
                Some(&raw mut bytes_written),
            )
        }?;

        if bytes_written != data.len() {
            return Err(MemoryError::Partial {
                requested: data.len(),
                transferred: bytes_written,
            });
        }
        Ok(())
    }
}

impl Drop for ProcessSession {
    fn drop(&mut self) {
        // SAFETY: `self.handle` was obtained from `OpenProcess` in `attach`
        // and, since `ProcessSession` isn't `Clone`, is closed exactly once,
        // here.
        let _ = unsafe { CloseHandle(self.handle) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attach_to_current_process_succeeds() {
        let pid = std::process::id();
        let session = ProcessSession::attach(pid).expect("attaching to our own process");
        assert_eq!(session.pid(), pid);
    }

    #[test]
    fn attach_to_nonexistent_pid_fails_with_process_not_found() {
        // A PID this large is not a real process ID on Windows (max PID is
        // well under u32::MAX in practice), so this is a stable "does not
        // exist" case rather than a race against a real process exiting.
        // ProcessSession deliberately isn't Debug/PartialEq (it owns a raw
        // HANDLE), so match instead of assert_eq!.
        match ProcessSession::attach(u32::MAX) {
            Err(AttachError::ProcessNotFound) => {}
            Err(other) => panic!("expected ProcessNotFound, got {other}"),
            Ok(_) => panic!("expected attach to a bogus PID to fail"),
        }
    }

    #[test]
    #[ignore = "only meaningful run unelevated - see reasoning below"]
    fn attach_to_a_system_owned_process_is_denied_without_elevation() {
        // services.exe runs as SYSTEM; opening it for VM read/write without
        // elevation reliably fails with ERROR_ACCESS_DENIED (verified by
        // hand: also true of winlogon.exe, lsass.exe, csrss.exe, and PID 4).
        // Elevated, this same attach can succeed, which is exactly the
        // behavior we're confirming AttachError maps correctly either way -
        // so accept Ok too, rather than assert this suite always runs
        // unelevated.
        let target = crate::list_processes()
            .into_iter()
            .find(|p| p.name.eq_ignore_ascii_case("services.exe"));

        let Some(target) = target else {
            eprintln!("services.exe not found (unusual on Windows) - skipping");
            return;
        };

        match ProcessSession::attach(target.pid) {
            Err(AttachError::AccessDenied) => {}
            Ok(_) => {} // running elevated - also a valid, expected outcome
            Err(other) => panic!("expected AccessDenied (or Ok, if elevated), got {other}"),
        }
    }
}
