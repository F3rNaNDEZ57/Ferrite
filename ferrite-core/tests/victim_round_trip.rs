//! Spawns `ferrite-victim`, attaches to it via [`ProcessSession`], and
//! round-trips a read/write against its known values. This is the proof that
//! attach/read/write actually work against a *separate* process — everything
//! else in `ferrite-core` is built on this working.

mod common;

use common::Victim;
use ferrite_core::ProcessSession;

#[test]
fn read_and_write_round_trip_against_a_separate_process() {
    let victim = Victim::spawn();
    let session = ProcessSession::attach(victim.pid()).expect("attaching to the victim process");

    let hp_address = victim.address_of("HP");

    // HP starts at 100 (i32, little-endian on x86/x64).
    let initial = session
        .read_bytes(hp_address, 4)
        .expect("reading HP from the victim");
    assert_eq!(initial, 100i32.to_le_bytes());

    // Write a new value, then read it back — proof the write landed in the
    // victim's actual memory, not just that the OS call returned success.
    session
        .write_bytes(hp_address, &42i32.to_le_bytes())
        .expect("writing HP in the victim");

    let updated = session
        .read_bytes(hp_address, 4)
        .expect("reading HP back after writing it");
    assert_eq!(updated, 42i32.to_le_bytes());
}
