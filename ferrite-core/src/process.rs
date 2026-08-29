//! Process enumeration for the process picker.
//!
//! This is a thin wrapper over `sysinfo`, converting to our own
//! [`ProcessInfo`] at the boundary so `sysinfo`'s types don't leak into the
//! rest of `ferrite-core`'s public API.

use std::path::PathBuf;

use sysinfo::{ProcessesToUpdate, System};

/// A running process, as shown in the process picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessInfo {
    pub pid: u32,
    pub name: String,
    /// The process's own executable path, if known - `None` for
    /// pseudo-processes with no backing file (`[System Process]`,
    /// `Registry`, `Secure System`) or when access is denied. Used to look
    /// up the process's icon (see `crate::icon`); nothing in `ferrite-core`
    /// itself needs it.
    pub exe: Option<PathBuf>,
}

/// Lists currently running processes.
///
/// Takes a full `sysinfo` refresh internally; not meant to be called on a
/// hot path — only when the process picker opens or the user hits refresh.
pub fn list_processes() -> Vec<ProcessInfo> {
    let mut system = System::new();
    system.refresh_processes(ProcessesToUpdate::All, true);

    system
        .processes()
        .values()
        .map(|process| ProcessInfo {
            pid: process.pid().as_u32(),
            name: process.name().to_string_lossy().into_owned(),
            exe: process.exe().map(|path| path.to_path_buf()),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

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
