//! Process enumeration for the process picker.
//!
//! This is a thin wrapper over `sysinfo`, converting to our own
//! [`ProcessInfo`] at the boundary so `sysinfo`'s types don't leak into the
//! rest of `ferrite-core`'s public API.

use std::path::PathBuf;

use sysinfo::{ProcessesToUpdate, System};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::SystemInformation::{IMAGE_FILE_MACHINE, IMAGE_FILE_MACHINE_UNKNOWN};
use windows::Win32::System::Threading::{
    IsWow64Process2, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
};

/// A target's pointer width, which decides whether Ferrite can attach to it
/// at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arch {
    /// 64-bit: attachable.
    X64,
    /// 32-bit (running under WOW64): **not** attachable. Ferrite is 64-bit
    /// only, so this is surfaced in the picker rather than left to fail as a
    /// confusing error after the user clicks Attach.
    X86,
    /// Couldn't be determined — the process refused even a limited-
    /// information handle, which is normal for protected and system
    /// processes. Reported as unknown rather than guessed as x64.
    Unknown,
}

impl Arch {
    /// Whether Ferrite can attach to a target of this architecture.
    /// `Unknown` is attachable: it usually means a privileged process that
    /// would fail on the attach itself, and refusing to try would hide a
    /// process the user may legitimately have rights to.
    pub fn is_attachable(self) -> bool {
        !matches!(self, Self::X86)
    }

    /// The short label the picker shows.
    pub fn label(self) -> &'static str {
        match self {
            Self::X64 => "x64",
            Self::X86 => "x86",
            Self::Unknown => "—",
        }
    }
}

/// A running process, as shown in the process picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessInfo {
    pub pid: u32,
    pub name: String,
    /// 64- or 32-bit. See [`Arch`].
    pub arch: Arch,
    /// The process's own executable path, if known - `None` for
    /// pseudo-processes with no backing file (`[System Process]`,
    /// `Registry`, `Secure System`) or when access is denied. Used to look
    /// up the process's icon (see `crate::icon`); nothing in `ferrite-core`
    /// itself needs it.
    pub exe: Option<PathBuf>,
}

/// Lists currently running processes.
///
/// Takes a full `sysinfo` refresh internally, and one `OpenProcess` /
/// `IsWow64Process2` pair per process to determine its architecture. Not
/// meant to be called on a hot path — only when the process picker opens or
/// the user hits refresh.
pub fn list_processes() -> Vec<ProcessInfo> {
    let mut system = System::new();
    system.refresh_processes(ProcessesToUpdate::All, true);

    system
        .processes()
        .values()
        .map(|process| {
            let pid = process.pid().as_u32();
            ProcessInfo {
                pid,
                name: process.name().to_string_lossy().into_owned(),
                arch: process_arch(pid),
                exe: process.exe().map(|path| path.to_path_buf()),
            }
        })
        .collect()
}

/// Determines a process's architecture.
///
/// `IsWow64Process2` reports the machine a process is *emulated* as: a value
/// of `IMAGE_FILE_MACHINE_UNKNOWN` means it isn't being emulated and so runs
/// natively — 64-bit on the x64 host Ferrite targets. Anything else means
/// WOW64, i.e. a 32-bit process.
///
/// Opened with `PROCESS_QUERY_LIMITED_INFORMATION` rather than the broader
/// query right: it is the least privilege that answers this question, and it
/// succeeds against many processes a full query handle would be refused for,
/// so fewer rows fall back to [`Arch::Unknown`].
fn process_arch(pid: u32) -> Arch {
    // SAFETY: OpenProcess with a pid that may not exist (or may be refused)
    // is well-defined - it returns an error rather than an invalid handle.
    let handle: HANDLE = match unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }
    {
        Ok(handle) => handle,
        Err(_) => return Arch::Unknown,
    };

    let mut process_machine = IMAGE_FILE_MACHINE(0);
    // SAFETY: `handle` is a live process handle for the duration of this
    // call, and `process_machine` is a valid out-parameter. The native
    // machine out-parameter is optional and not needed here.
    let queried = unsafe { IsWow64Process2(handle, &raw mut process_machine, None) };
    // SAFETY: `handle` came from OpenProcess above and hasn't been closed.
    let _ = unsafe { CloseHandle(handle) };

    match queried {
        Ok(()) if process_machine == IMAGE_FILE_MACHINE_UNKNOWN => Arch::X64,
        Ok(()) => Arch::X86,
        Err(_) => Arch::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn our_own_process_reports_itself_as_64_bit() {
        // Ferrite is a 64-bit binary, so this is a fixed expectation rather
        // than whatever the API happens to say - if this ever reports x86,
        // the UNKNOWN-means-native reading is inverted.
        assert_eq!(process_arch(std::process::id()), Arch::X64);
    }

    #[test]
    fn an_unknown_pid_reports_unknown_rather_than_guessing() {
        // 0 is the System Idle pseudo-process, which never yields a handle.
        assert_eq!(process_arch(0), Arch::Unknown);
    }

    #[test]
    fn only_32_bit_targets_are_unattachable() {
        assert!(Arch::X64.is_attachable());
        assert!(
            Arch::Unknown.is_attachable(),
            "unknown usually means a privileged process, not a 32-bit one - \
             refusing to try would hide a process the user may have rights to"
        );
        assert!(!Arch::X86.is_attachable());
    }

    #[test]
    fn current_process_appears_in_the_list() {
        let current_pid = std::process::id();
        let processes = list_processes();

        assert!(
            processes.iter().any(|p| p.pid == current_pid),
            "expected current process (pid {current_pid}) to appear in the process list"
        );
    }
}
