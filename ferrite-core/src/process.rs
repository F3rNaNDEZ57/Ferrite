//! Process enumeration for the process picker.
//!
//! This is a thin wrapper over `sysinfo`, converting to our own
//! [`ProcessInfo`] at the boundary so `sysinfo`'s types don't leak into the
//! rest of `ferrite-core`'s public API.

use sysinfo::{ProcessesToUpdate, System};

/// A running process, as shown in the process picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessInfo {
    pub pid: u32,
    pub name: String,
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
