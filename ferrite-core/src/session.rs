//! Attach to a running process, as a precursor to reading/writing its memory.
//!
//! Ownership model: a [`ProcessSession`] wraps exactly one open process
//! handle and closes it automatically on drop (RAII); it is intentionally
//! not `Clone`. See the concurrency model in the vault's `v1-plan.md`.

use windows::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, ERROR_INVALID_PARAMETER, HANDLE,
};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_VM_OPERATION, PROCESS_VM_READ, PROCESS_VM_WRITE,
};
use windows::core::HRESULT;

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
}
