//! Exercises `next_scan`'s four filters against a real separate process,
//! using `write_bytes` to drive value changes under our own control.
//!
//! One assertion shape throughout: check HP's specific presence/absence in
//! the results, never a result-set count. Other addresses that happen to
//! hold the same value as HP can independently change on their own, so an
//! exact-count assertion would flake.

mod common;

use common::Victim;
use ferrite_core::{
    ProcessSession, ScanFilter, ScanOptions, ScanValue, first_scan_exact, next_scan,
};

fn hp_survives(matches: &[ferrite_core::ScanMatch], hp_address: usize) -> bool {
    matches.iter().any(|m| m.address == hp_address)
}

#[test]
fn increased_keeps_hp_after_a_write_that_raises_it_decreased_and_unchanged_drop_it() {
    let victim = Victim::spawn();
    let session = ProcessSession::attach(victim.pid()).expect("attaching to the victim process");
    let hp_address = victim.address_of("HP");

    let first = first_scan_exact(&session, ScanValue::I32(100), ScanOptions::default());
    assert!(
        hp_survives(&first.matches, hp_address),
        "HP missing from first scan"
    );

    session
        .write_bytes(hp_address, &150i32.to_le_bytes())
        .expect("raising HP");

    let increased = next_scan(&session, &first.matches, ScanFilter::Increased);
    assert!(
        hp_survives(&increased, hp_address),
        "Increased should keep HP after 100 -> 150"
    );

    let decreased = next_scan(&session, &first.matches, ScanFilter::Decreased);
    assert!(
        !hp_survives(&decreased, hp_address),
        "Decreased should drop HP after 100 -> 150"
    );

    let unchanged = next_scan(&session, &first.matches, ScanFilter::Unchanged);
    assert!(
        !hp_survives(&unchanged, hp_address),
        "Unchanged should drop HP after 100 -> 150"
    );

    let changed = next_scan(&session, &first.matches, ScanFilter::Changed);
    assert!(
        hp_survives(&changed, hp_address),
        "Changed should keep HP after 100 -> 150"
    );
}

#[test]
fn unchanged_keeps_hp_when_nothing_wrote_to_it() {
    let victim = Victim::spawn();
    let session = ProcessSession::attach(victim.pid()).expect("attaching to the victim process");
    let hp_address = victim.address_of("HP");

    let first = first_scan_exact(&session, ScanValue::I32(100), ScanOptions::default());
    assert!(
        hp_survives(&first.matches, hp_address),
        "HP missing from first scan"
    );

    // No write in between - HP should still read as 100.
    let unchanged = next_scan(&session, &first.matches, ScanFilter::Unchanged);
    assert!(
        hp_survives(&unchanged, hp_address),
        "Unchanged should keep HP when nothing wrote to it"
    );
}

#[test]
fn a_second_changed_scan_compares_against_the_updated_value_not_the_stale_original() {
    // The staleness trap: after the first Changed scan keeps HP, its
    // recorded value must become the *new* value (150), not stay at the
    // original 100. If next_scan forgot to update it, a second Changed scan
    // -- with no further write -- would wrongly compare the live value (150)
    // against the stale original (100), see a "change", and wrongly keep HP
    // again. HP should actually be dropped: nothing changed between the
    // first and second Changed scans.
    let victim = Victim::spawn();
    let session = ProcessSession::attach(victim.pid()).expect("attaching to the victim process");
    let hp_address = victim.address_of("HP");

    let first = first_scan_exact(&session, ScanValue::I32(100), ScanOptions::default());
    assert!(
        hp_survives(&first.matches, hp_address),
        "HP missing from first scan"
    );

    session
        .write_bytes(hp_address, &150i32.to_le_bytes())
        .expect("raising HP");

    let first_changed = next_scan(&session, &first.matches, ScanFilter::Changed);
    assert!(
        hp_survives(&first_changed, hp_address),
        "first Changed scan should keep HP (100 -> 150)"
    );
    let hp_match = first_changed
        .iter()
        .find(|m| m.address == hp_address)
        .expect("just asserted HP survives");
    assert_eq!(
        hp_match.value,
        ScanValue::I32(150),
        "the surviving match's value must be updated to the new value, not left at the original"
    );

    // No write here - HP is still 150 in the victim.
    let second_changed = next_scan(&session, &first_changed, ScanFilter::Changed);
    assert!(
        !hp_survives(&second_changed, hp_address),
        "second Changed scan should drop HP: nothing changed between the two Changed scans"
    );
}
