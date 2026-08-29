//! Exercises the AOB (byte-pattern) scan path against a real separate
//! process: first scan, chunk-boundary handling with a pattern longer than
//! the chunk size, and next-scan Changed/Unchanged filtering.

mod common;

use common::Victim;
use ferrite_core::{
    AobFilter, MemoryRegion, ProcessSession, ScanOptions, first_scan_aob, next_scan_aob,
    scan_region_aob,
};

fn hp_survives(matches: &[ferrite_core::AobMatch], hp_address: usize) -> bool {
    matches.iter().any(|m| m.address == hp_address)
}

#[test]
fn full_process_scan_with_default_options_finds_hp() {
    let victim = Victim::spawn();
    let session = ProcessSession::attach(victim.pid()).expect("attaching to the victim process");
    let hp_address = victim.address_of("HP");

    // HP is a little-endian i32 == 100.
    let pattern = 100i32.to_le_bytes();
    let result = first_scan_aob(&session, &pattern, ScanOptions::default());

    assert!(
        hp_survives(&result.matches, hp_address),
        "expected HP's address {hp_address:#x} among the matches (got {} matches)",
        result.matches.len()
    );
}

#[test]
fn finds_hp_with_a_chunk_size_smaller_than_the_pattern_itself() {
    let victim = Victim::spawn();
    let session = ProcessSession::attach(victim.pid()).expect("attaching to the victim process");
    let hp_address = victim.address_of("HP");

    // Same tightly-bounded synthetic region trick as scan.rs's numeric
    // equivalent - proves the overlap logic without walking a whole region.
    let region = MemoryRegion {
        base_address: hp_address - 16,
        size: 32,
    };

    let pattern = 100i32.to_le_bytes();
    // chunk_size (3) is smaller than the 4-byte pattern, so it straddles a
    // chunk boundary on essentially every possible alignment.
    let options = ScanOptions {
        chunk_size: 3,
        ..Default::default()
    };
    let matches = scan_region_aob(&session, region, &pattern, options, 100);

    assert!(
        matches.iter().any(|m| m.address == hp_address),
        "expected HP's address {hp_address:#x} among the matches (got {} matches)",
        matches.len()
    );
}

#[test]
fn changed_and_unchanged_track_a_write_to_the_matched_bytes() {
    let victim = Victim::spawn();
    let session = ProcessSession::attach(victim.pid()).expect("attaching to the victim process");
    let hp_address = victim.address_of("HP");

    let pattern = 100i32.to_le_bytes();
    let first = first_scan_aob(&session, &pattern, ScanOptions::default());
    assert!(
        hp_survives(&first.matches, hp_address),
        "HP missing from first scan"
    );

    session
        .write_bytes(hp_address, &150i32.to_le_bytes())
        .expect("changing HP's bytes");

    let unchanged = next_scan_aob(&session, &first.matches, AobFilter::Unchanged);
    assert!(
        !hp_survives(&unchanged, hp_address),
        "Unchanged should drop HP after its bytes changed"
    );

    let changed = next_scan_aob(&session, &first.matches, AobFilter::Changed);
    assert!(
        hp_survives(&changed, hp_address),
        "Changed should keep HP after its bytes changed"
    );

    // Second Changed scan with no further write - proves the stored bytes
    // were updated to the new value, not left stale at the original
    // pattern (same staleness trap as the numeric next_scan).
    let second_changed = next_scan_aob(&session, &changed, AobFilter::Changed);
    assert!(
        !hp_survives(&second_changed, hp_address),
        "second Changed scan should drop HP: nothing changed since the first Changed scan"
    );
}
