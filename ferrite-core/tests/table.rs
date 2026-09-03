//! Exercises module resolution, pointer resolution, and `CheatEntry`
//! address resolution against a real separate process (`ferrite-victim`),
//! not just the pure-function unit tests in `table.rs`/`modules.rs`/
//! `pointer.rs` themselves.

mod common;

use common::Victim;
use ferrite_core::{
    AddressExpr, CheatEntry, EntryValue, ProcessSession, ResolveError, ScanValue, list_modules,
    module_base, resolve_address, resolve_pointer,
};

#[test]
fn victims_own_exe_module_resolves_with_a_real_base_address() {
    let victim = Victim::spawn();
    let session = ProcessSession::attach(victim.pid()).expect("attaching to the victim process");

    let modules = list_modules(&session).expect("listing the victim's modules");
    assert!(
        modules
            .iter()
            .any(|m| m.name.eq_ignore_ascii_case("ferrite-victim.exe")),
        "expected ferrite-victim.exe among its own modules, got: {modules:?}"
    );

    let base = module_base(&session, "ferrite-victim.exe")
        .expect("resolving ferrite-victim.exe's own base address");
    assert_ne!(base, 0);
}

#[test]
fn resolving_an_unloaded_module_name_fails_with_not_found() {
    let victim = Victim::spawn();
    let session = ProcessSession::attach(victim.pid()).expect("attaching to the victim process");

    match module_base(&session, "definitely-not-loaded.dll") {
        Err(ferrite_core::ModuleError::NotFound) => {}
        other => panic!("expected NotFound, got {other:?}"),
    }
}

#[test]
fn resolve_pointer_dereferences_a_real_pointer_in_the_victim() {
    let victim = Victim::spawn();
    let session = ProcessSession::attach(victim.pid()).expect("attaching to the victim process");

    let ptr_address = victim.address_of("PTR");
    let hp_address = victim.address_of("HP");

    let resolved =
        resolve_pointer(&session, ptr_address, 0).expect("resolving PTR in the victim process");
    assert_eq!(resolved, hp_address, "PTR should point at HP");

    let bytes = session
        .read_bytes(resolved, 4)
        .expect("reading HP through the resolved pointer");
    assert_eq!(i32::from_le_bytes(bytes.try_into().unwrap()), 100);
}

#[test]
fn cheat_entry_with_pointer_offset_resolves_to_hp() {
    let victim = Victim::spawn();
    let session = ProcessSession::attach(victim.pid()).expect("attaching to the victim process");

    let entry = CheatEntry {
        description: "HP via PTR".to_string(),
        base: AddressExpr::Absolute(victim.address_of("PTR")),
        pointer_offsets: vec![0],
        value: EntryValue::Scalar(ScanValue::I32(100)),
        frozen: false,
        show_as_hex: false,
    };

    let resolved = resolve_address(&entry, &session).expect("resolving the entry");
    assert_eq!(resolved, victim.address_of("HP"));
}

#[test]
fn cheat_entry_with_a_two_level_chain_resolves_through_ptr2_to_hp() {
    // The whole point of the schema change, against a real chain in a real
    // process: PTR2 -> PTR -> HP, two offsets, two dereferences.
    let victim = Victim::spawn();
    let session = ProcessSession::attach(victim.pid()).expect("attaching to the victim process");

    let entry = CheatEntry {
        description: "HP via PTR2".to_string(),
        base: AddressExpr::Absolute(victim.address_of("PTR2")),
        pointer_offsets: vec![0, 0],
        value: EntryValue::Scalar(ScanValue::I32(100)),
        frozen: false,
        show_as_hex: false,
    };

    let resolved = resolve_address(&entry, &session).expect("resolving the two-level entry");
    assert_eq!(resolved, victim.address_of("HP"));

    // One offset short lands on PTR instead - the check that the hop count
    // is what decides where the chain ends, not luck.
    let one_level = CheatEntry {
        pointer_offsets: vec![0],
        ..entry.clone()
    };
    assert_eq!(
        resolve_address(&one_level, &session).expect("resolving one level"),
        victim.address_of("PTR")
    );
}

#[test]
fn an_entry_with_no_offsets_resolves_to_its_base_address_untouched() {
    // The direct-address case after the schema change: an empty chain must
    // return the base, not dereference it once. HP's own address holds the
    // number 100, so a stray dereference would land on 100, not on HP.
    let victim = Victim::spawn();
    let session = ProcessSession::attach(victim.pid()).expect("attaching to the victim process");

    let entry = CheatEntry {
        description: "HP directly".to_string(),
        base: AddressExpr::Absolute(victim.address_of("HP")),
        pointer_offsets: Vec::new(),
        value: EntryValue::Scalar(ScanValue::I32(100)),
        frozen: false,
        show_as_hex: false,
    };

    assert_eq!(
        resolve_address(&entry, &session).expect("resolving a direct entry"),
        victim.address_of("HP")
    );
}

#[test]
fn cheat_entry_with_missing_module_reports_which_one() {
    let victim = Victim::spawn();
    let session = ProcessSession::attach(victim.pid()).expect("attaching to the victim process");

    let entry = CheatEntry {
        description: "wrong game".to_string(),
        base: AddressExpr::ModuleRelative {
            module: "not-loaded.exe".to_string(),
            offset: 0x1000,
        },
        pointer_offsets: Vec::new(),
        value: EntryValue::Scalar(ScanValue::I32(0)),
        frozen: false,
        show_as_hex: false,
    };

    match resolve_address(&entry, &session) {
        Err(ResolveError::ModuleNotFound(module)) => assert_eq!(module, "not-loaded.exe"),
        other => panic!("expected ModuleNotFound, got {other:?}"),
    }
}
