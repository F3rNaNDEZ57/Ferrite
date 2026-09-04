//! Enumerates a process's loaded modules and resolves a module's base
//! address by name — what `"module.exe"+offset`-style symbolic addressing
//! (the form nearly every real Cheat Engine table uses, per the vault's
//! `v0.1-plan.md`) needs to turn into an absolute address.

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
/// exclusively (see the vault's `v0.1-scope.md`), and a 32-bit process
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

/// An address expressed relative to the module that contains it — the
/// `module.exe+1C58DA0` form a saved table uses, recovered from a bare
/// address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleOffset {
    pub module: String,
    pub offset: usize,
}

impl std::fmt::Display for ModuleOffset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}+{:X}", self.module, self.offset)
    }
}

/// A snapshot of the target's modules, sorted by base address, for turning
/// an address back into `module+offset`.
///
/// Built once and reused, rather than resolved per address: the results
/// table asks this question for every visible row on a 100 ms tick, and
/// [`list_modules`] is a full `EnumProcessModulesEx` walk plus a
/// `GetModuleFileNameExW` per module. Doing that per row per tick would be
/// hundreds of process queries a second to answer a question whose answer
/// only changes when a module is loaded or unloaded.
///
/// It is a *snapshot*, deliberately: it goes stale if the target loads a
/// DLL, and the caller decides when to rebuild (the GUI does so on attach
/// and on a new scan). A stale map resolves an address in a newly-loaded
/// module to `None` rather than to something wrong.
#[derive(Debug, Clone, Default)]
pub struct ModuleMap {
    /// Sorted by `base`, ascending. Module images don't overlap, so the
    /// containing module is always the last one whose base is at or below
    /// the address.
    sorted: Vec<ModuleInfo>,
}

impl ModuleMap {
    /// Takes a snapshot of the attached process's modules.
    pub fn build(session: &ProcessSession) -> Result<Self, MemoryError> {
        let mut sorted = list_modules(session)?;
        sorted.sort_unstable_by_key(|m| m.base);
        Ok(Self { sorted })
    }

    /// An empty map, which resolves nothing. What the GUI holds while
    /// detached.
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.sorted.is_empty()
    }

    pub fn len(&self) -> usize {
        self.sorted.len()
    }

    /// Resolves `address` to the module containing it, or `None` if it falls
    /// in no module's image — which is the common case for a heap or stack
    /// address, not an error.
    pub fn resolve(&self, address: usize) -> Option<ModuleOffset> {
        // The last module whose base is <= address; binary search rather
        // than a scan, since this runs per visible row per tick.
        let index = self.sorted.partition_point(|m| m.base <= address);
        let module = self.sorted.get(index.checked_sub(1)?)?;
        // partition_point only established base <= address. The address can
        // still sit in the gap past this module's image and before the next
        // one, so the upper bound has to be checked explicitly.
        (address < module.base.checked_add(module.size)?).then(|| ModuleOffset {
            module: module.name.clone(),
            offset: address - module.base,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A map built from literal values, so the resolution logic is testable
    /// without depending on where Windows happened to load anything.
    fn map_of(modules: &[(&str, usize, usize)]) -> ModuleMap {
        let mut sorted: Vec<ModuleInfo> = modules
            .iter()
            .map(|(name, base, size)| ModuleInfo {
                name: (*name).to_string(),
                base: *base,
                size: *size,
            })
            .collect();
        sorted.sort_unstable_by_key(|m| m.base);
        ModuleMap { sorted }
    }

    #[test]
    fn resolves_an_address_to_the_module_that_contains_it() {
        let map = map_of(&[
            ("game.exe", 0x1000, 0x1000),
            ("unityplayer.dll", 0x5000, 0x2000),
        ]);
        assert_eq!(
            map.resolve(0x1000),
            Some(ModuleOffset {
                module: "game.exe".to_string(),
                offset: 0,
            }),
            "the base address itself is inside the module"
        );
        assert_eq!(map.resolve(0x1234).unwrap().to_string(), "game.exe+234");
        assert_eq!(
            map.resolve(0x6000).unwrap().to_string(),
            "unityplayer.dll+1000"
        );
    }

    #[test]
    fn an_address_in_the_gap_between_two_modules_resolves_to_nothing() {
        // The trap this guards: a binary search only establishes
        // `base <= address`. Without checking the module's size too, every
        // heap and stack address would be reported as an offset into
        // whichever module happens to sit below it - a plausible-looking,
        // entirely wrong `module+offset`.
        let map = map_of(&[
            ("game.exe", 0x1000, 0x1000),
            ("unityplayer.dll", 0x5000, 0x2000),
        ]);
        assert_eq!(map.resolve(0x2000), None, "one past game.exe's last byte");
        assert_eq!(map.resolve(0x3FFF), None, "the gap between the two");
        assert_eq!(map.resolve(0x7000), None, "past the last module");
        assert_eq!(map.resolve(0x0), None, "below the first module");
    }

    #[test]
    fn an_empty_map_resolves_nothing_rather_than_panicking() {
        assert_eq!(ModuleMap::empty().resolve(0x1000), None);
        assert!(ModuleMap::empty().is_empty());
    }

    #[test]
    fn a_real_map_resolves_an_address_inside_our_own_exe() {
        let session =
            ProcessSession::attach(std::process::id()).expect("attaching to our own process");
        let map = ModuleMap::build(&session).expect("building a module map");
        assert!(!map.is_empty());

        // A function pointer in this very binary must land inside some
        // loaded module - proof the map works against real module layout,
        // not just synthetic bases.
        let somewhere_in_our_exe =
            a_real_map_resolves_an_address_inside_our_own_exe as fn() as usize;
        let resolved = map
            .resolve(somewhere_in_our_exe)
            .expect("a code address should fall inside a loaded module");
        assert!(
            resolved.module.ends_with(".exe") || resolved.module.ends_with(".dll"),
            "unexpected module {resolved:?}"
        );
    }

    #[test]
    fn a_stack_address_falls_in_no_module() {
        let session =
            ProcessSession::attach(std::process::id()).expect("attaching to our own process");
        let map = ModuleMap::build(&session).expect("building a module map");
        let local = 0u64;
        assert_eq!(map.resolve(&raw const local as usize), None);
    }

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
