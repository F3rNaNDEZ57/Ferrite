//! Exercises `scan_region_exact` against a real separate process, with a
//! chunk size small enough to force the value across a chunk boundary. This
//! is the end-to-end proof that the chunk-overlap logic actually works
//! through real `ReadProcessMemory` calls, not just against a synthetic
//! byte buffer.

mod common;

use common::Victim;
use ferrite_core::{
    MemoryRegion, ProcessSession, ScanOptions, ScanValue, first_scan_exact, scan_region_exact,
};

#[test]
fn full_process_scan_with_default_options_finds_hp() {
    // The "normal" path: first_scan_exact with real region enumeration and
    // production-sized chunks (1 MiB), not the boundary edge case below.
    // With a small process like the victim, this should complete quickly
    // and confirm the public API works end to end at realistic settings.
    let victim = Victim::spawn();
    let session = ProcessSession::attach(victim.pid()).expect("attaching to the victim process");
    let hp_address = victim.address_of("HP");

    let result = first_scan_exact(&session, ScanValue::I32(100), ScanOptions::default());

    assert!(
        result
            .matches
            .iter()
            .any(|m| m.address == hp_address && m.value == ScanValue::I32(100)),
        "expected HP's address {hp_address:#x} among the matches (got {} matches)",
        result.matches.len()
    );
    assert!(!result.capped);
}

#[test]
fn finds_hp_with_a_chunk_size_smaller_than_the_value_itself() {
    let victim = Victim::spawn();
    let session = ProcessSession::attach(victim.pid()).expect("attaching to the victim process");
    let hp_address = victim.address_of("HP");

    // A small synthetic region tightly bounding HP, rather than a real
    // (possibly huge) writable region: scanning an entire real region with
    // a 3-byte chunk size would mean hundreds of thousands of individual
    // ReadProcessMemory calls just to reach HP. That's exactly what the
    // chunking needs to be correct for in production, but this test only
    // needs to prove the boundary-overlap logic works against real memory —
    // it doesn't need to also prove region enumeration or walk unrelated
    // memory to do that, and doing so would make the test dramatically
    // slower for no added coverage.
    let region = MemoryRegion {
        base_address: hp_address - 16,
        size: 32,
    };

    // chunk_size (3) is smaller than an i32 (4 bytes), so HP straddles a
    // chunk boundary on essentially every possible alignment — if the
    // overlap logic were broken, this would reliably miss it.
    let options = ScanOptions {
        chunk_size: 3,
        ..Default::default()
    };
    let matches = scan_region_exact(&session, region, ScanValue::I32(100), options, 100);

    assert!(
        matches
            .iter()
            .any(|m| m.address == hp_address && m.value == ScanValue::I32(100)),
        "expected HP's address {hp_address:#x} among the matches (got {} matches)",
        matches.len()
    );
}
