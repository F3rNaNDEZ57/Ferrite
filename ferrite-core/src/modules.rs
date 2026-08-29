//! Enumerates a process's loaded modules and resolves a module's base
//! address by name — what `"module.exe"+offset`-style symbolic addressing
//! (the form nearly every real Cheat Engine table uses, per the vault's
//! `v1-plan.md`) needs to turn into an absolute address.

use std::mem::size_of;

use windows::Win32::Foundation::HMODULE;
use windows::Win32::System::ProcessStatus::{
    EnumProcessModulesEx, GetModuleFileNameExW, GetModuleInformation, LIST_MODULES_64BIT,
    MODULEINFO,
};

use crate::session::{MemoryError, ProcessSession};

/// One loaded module in the target process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleInfo {
    /// The module's file name only (e.g. `"GTA5.exe"`), not the full path —
    /// this is what a `"module.exe"+offset` address expression names.
    pub name: String,
    pub base: usize,
    pub size: usize,
}

/// Why [`module_base`] failed to resolve a name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModuleError {
    /// No loaded module's file name matched (case-insensitively).
    NotFound,
    /// The underlying module enumeration failed at the OS level.
    Memory(MemoryError),
}

impl From<MemoryError> for ModuleError {
    fn from(err: MemoryError) -> Self {
        Self::Memory(err)
    }
}

/// Lists every module currently loaded in the attached process (its main
/// exe plus every DLL it has mapped).
///
/// 64-bit modules only (`LIST_MODULES_64BIT`) — v1 targets 64-bit processes
/// exclusively (see the vault's `v1-scope.md`), and a 32-bit process
/// wouldn't attach successfully in the first place.
pub fn list_modules(session: &ProcessSession) -> Result<Vec<ModuleInfo>, MemoryError> {
    // A generous fixed cap rather than a dynamic two-pass query: real
    // processes rarely load more than a few hundred modules, and this list
    // is only walked to resolve one name, not rendered in bulk.
    const MAX_MODULES: usize = 1024;
    // SAFETY: a null HMODULE (all-zero) is a valid initial value to overwrite
    // with EnumProcessModulesEx's output below.
    let mut handles: Vec<HMODULE> = vec![HMODULE(std::ptr::null_mut()); MAX_MODULES];
    let mut bytes_needed: u32 = 0;

    // SAFETY: `handles` is a valid, uniquely-owned buffer of its stated byte
    // length for the duration of this call; `bytes_needed` is a valid,
    // uniquely-owned out-parameter.
    unsafe {
        EnumProcessModulesEx(
            session.handle(),
            handles.as_mut_ptr(),
            (handles.len() * size_of::<HMODULE>()) as u32,
            &raw mut bytes_needed,
            LIST_MODULES_64BIT,
        )
    }
    .map_err(MemoryError::from)?;

    let returned = (bytes_needed as usize / size_of::<HMODULE>()).min(handles.len());
    let mut modules = Vec::with_capacity(returned);

    for &handle in &handles[..returned] {
        // SAFETY: `MODULEINFO` is a plain C struct of integer/pointer
        // fields - all-zero is a valid bit pattern, overwritten by
        // GetModuleInformation below.
        let mut info: MODULEINFO = unsafe { std::mem::zeroed() };
        // SAFETY: `info` is a valid, correctly-sized out-parameter for the
        // duration of this call; `handle` came from the enumeration above.
        let info_result = unsafe {
            GetModuleInformation(
                session.handle(),
                handle,
                &raw mut info,
                size_of::<MODULEINFO>() as u32,
            )
        };
        if info_result.is_err() {
            continue; // a module that raced an unload between the two calls
        }

        let mut name_buf = [0u16; 260];
        // SAFETY: `name_buf` is a valid, uniquely-owned buffer for the
        // duration of this call.
        let name_len =
            unsafe { GetModuleFileNameExW(Some(session.handle()), Some(handle), &mut name_buf) };
        if name_len == 0 {
            continue;
        }

        let full_path = String::from_utf16_lossy(&name_buf[..name_len as usize]);
        let name = full_path
            .rsplit(['\\', '/'])
            .next()
            .unwrap_or(&full_path)
            .to_string();

        modules.push(ModuleInfo {
            name,
            base: info.lpBaseOfDll as usize,
            size: info.SizeOfImage as usize,
        });
    }

    Ok(modules)
}

/// Resolves a loaded module's base address by file name (e.g. `"GTA5.exe"`),
/// case-insensitively — matching how a `"module.exe"+offset` address
/// expression names it.
pub fn module_base(session: &ProcessSession, name: &str) -> Result<usize, ModuleError> {
    let modules = list_modules(session)?;
    modules
        .into_iter()
        .find(|m| m.name.eq_ignore_ascii_case(name))
        .map(|m| m.base)
        .ok_or(ModuleError::NotFound)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn our_own_process_lists_its_own_exe_module() {
        let session =
            ProcessSession::attach(std::process::id()).expect("attaching to our own process");
        let modules = list_modules(&session).expect("listing our own modules");
        assert!(!modules.is_empty(), "expected at least our own exe module");
        assert!(
            modules.iter().all(|m| m.base != 0 && m.size != 0),
            "every module should report a nonzero base and size"
        );
    }

    #[test]
    fn resolving_a_nonexistent_module_name_fails_with_not_found() {
        let session =
            ProcessSession::attach(std::process::id()).expect("attaching to our own process");
        match module_base(&session, "definitely-not-a-real-module.dll") {
            Err(ModuleError::NotFound) => {}
            other => panic!("expected NotFound, got {other:?}"),
        }
    }
}
