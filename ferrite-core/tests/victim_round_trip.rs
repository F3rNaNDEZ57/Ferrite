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

/// Mirrors `ferrite-victim`'s own `VICTIM_TEXT`/`STR_BUFFER_LEN`. A test
/// binary can't import constants from a separate binary crate, so these are
/// restated here the same way `HP`'s initial `100` is above.
const VICTIM_TEXT: &str = "FerriteVictim";
const STR_BUFFER_LEN: usize = 32;

#[test]
fn the_string_buffers_live_in_writable_memory_where_a_scan_can_reach_them() {
    // The point of the victim's string buffers is to be *scannable*, and
    // `writable_regions` — what every scan walks — excludes read-only
    // memory. Reading the bytes back proves nothing about that on its own,
    // since `ReadProcessMemory` is happy to read a read-only page, so assert
    // the containing region directly.
    let victim = Victim::spawn();
    let session = ProcessSession::attach(victim.pid()).expect("attaching to the victim process");

    let regions = session.writable_regions();
    for name in ["STR_ASCII", "STR_UNICODE"] {
        let address = victim.address_of(name);
        assert!(
            regions
                .iter()
                .any(|r| address >= r.base_address && address - r.base_address < r.size),
            "{name} (0x{address:X}) must live in a writable region, or an AOB scan \
             would never see it"
        );
    }
}

#[test]
fn the_string_buffers_hold_their_text_nul_padded_to_the_full_buffer() {
    let victim = Victim::spawn();
    let session = ProcessSession::attach(victim.pid()).expect("attaching to the victim process");

    let ascii = session
        .read_bytes(victim.address_of("STR_ASCII"), STR_BUFFER_LEN)
        .expect("reading STR_ASCII from the victim");
    let (text, padding) = ascii.split_at(VICTIM_TEXT.len());
    assert_eq!(text, VICTIM_TEXT.as_bytes());
    assert!(
        padding.iter().all(|&b| b == 0),
        "STR_ASCII should be NUL-padded to its full length, got: {padding:?}"
    );

    let expected_utf16: Vec<u8> = VICTIM_TEXT
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    let unicode = session
        .read_bytes(victim.address_of("STR_UNICODE"), STR_BUFFER_LEN)
        .expect("reading STR_UNICODE from the victim");
    let (text, padding) = unicode.split_at(expected_utf16.len());
    assert_eq!(text, expected_utf16);
    assert!(
        padding.iter().all(|&b| b == 0),
        "STR_UNICODE should be NUL-padded to its full length, got: {padding:?}"
    );
}

#[test]
fn ptr2_is_a_genuine_two_hop_chain_ending_at_hp() {
    // Walked one hop at a time with the existing single-level resolver:
    // this asserts the victim's *fixture* is a real two-level chain, which
    // has to be true before `resolve_pointer_chain` can be tested against
    // it. Getting the hop count wrong is easy to do and lands on PTR's
    // address rather than HP's, so both hops are asserted.
    let victim = Victim::spawn();
    let session = ProcessSession::attach(victim.pid()).expect("attaching to the victim process");

    let first_hop = ferrite_core::resolve_pointer(&session, victim.address_of("PTR2"), 0)
        .expect("dereferencing PTR2");
    assert_eq!(
        first_hop,
        victim.address_of("PTR"),
        "PTR2 should point at PTR"
    );

    let second_hop =
        ferrite_core::resolve_pointer(&session, first_hop, 0).expect("dereferencing PTR");
    assert_eq!(
        second_hop,
        victim.address_of("HP"),
        "PTR should point at HP"
    );

    let bytes = session
        .read_bytes(second_hop, 4)
        .expect("reading HP through the two-hop chain");
    assert_eq!(i32::from_le_bytes(bytes.try_into().unwrap()), 100);
}
