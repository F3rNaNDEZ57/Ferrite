//! The victim's known values live on its own stack — this confirms
//! `writable_regions` actually finds the region that contains them, since
//! the scan engine only ever looks inside regions this returns.

mod common;

use common::Victim;
use ferrite_core::ProcessSession;

#[test]
fn victim_hp_address_falls_inside_an_enumerated_writable_region() {
    let victim = Victim::spawn();
    let session = ProcessSession::attach(victim.pid()).expect("attaching to the victim process");
    let hp_address = victim.address_of("HP");

    let regions = session.writable_regions();
    let containing_region = regions
        .iter()
        .find(|r| hp_address >= r.base_address && hp_address < r.base_address + r.size);

    assert!(
        containing_region.is_some(),
        "expected HP's address {hp_address:#x} to fall inside one of {} writable regions",
        regions.len()
    );
}
