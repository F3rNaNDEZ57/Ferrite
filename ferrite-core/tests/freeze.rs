//! Exercises the freeze thread against a real separate process: pinning a
//! frozen address against an external write while leaving an unfrozen
//! address alone, unfreezing actually stopping the rewrites, detecting
//! that the target process exited, and pinning a whole fixed-width string
//! buffer rather than just the part of it that currently holds text.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::Victim;
use ferrite_core::{EntryValue, ProcessSession, TextEncoding, decode_text};

/// Fast enough to keep the tests quick, slow enough that a single sleep
/// comfortably spans several ticks.
const TEST_FREEZE_INTERVAL: Duration = Duration::from_millis(15);

fn wait_for_a_few_ticks() {
    std::thread::sleep(TEST_FREEZE_INTERVAL * 6);
}

#[test]
fn freeze_pins_a_value_and_leaves_unfrozen_addresses_alone() {
    let victim = Victim::spawn();
    let session =
        Arc::new(ProcessSession::attach(victim.pid()).expect("attaching to the victim process"));
    let hp_address = victim.address_of("HP");
    let score_address = victim.address_of("SCORE");

    let freeze = session.start_freeze_thread(TEST_FREEZE_INTERVAL);
    freeze.freeze(hp_address, 999i32.to_le_bytes().to_vec());

    // Simulate the target process (or something else) writing over both
    // addresses "from outside" the freeze mechanism.
    session
        .write_bytes(hp_address, &5i32.to_le_bytes())
        .expect("external write to HP");
    session
        .write_bytes(score_address, &42i64.to_le_bytes())
        .expect("external write to SCORE");

    wait_for_a_few_ticks();

    let hp_bytes = session.read_bytes(hp_address, 4).expect("reading HP back");
    assert_eq!(
        hp_bytes,
        999i32.to_le_bytes(),
        "frozen HP should have been rewritten back to 999"
    );

    let score_bytes = session
        .read_bytes(score_address, 8)
        .expect("reading SCORE back");
    assert_eq!(
        score_bytes,
        42i64.to_le_bytes(),
        "unfrozen SCORE should be left exactly as the external write set it"
    );
}

#[test]
fn unfreezing_stops_the_rewrites() {
    let victim = Victim::spawn();
    let session =
        Arc::new(ProcessSession::attach(victim.pid()).expect("attaching to the victim process"));
    let hp_address = victim.address_of("HP");

    let freeze = session.start_freeze_thread(TEST_FREEZE_INTERVAL);
    freeze.freeze(hp_address, 999i32.to_le_bytes().to_vec());
    wait_for_a_few_ticks();
    assert!(freeze.is_frozen(hp_address));

    freeze.unfreeze(hp_address);
    assert!(!freeze.is_frozen(hp_address));

    session
        .write_bytes(hp_address, &5i32.to_le_bytes())
        .expect("external write to HP after unfreezing");
    wait_for_a_few_ticks();

    let hp_bytes = session.read_bytes(hp_address, 4).expect("reading HP back");
    assert_eq!(
        hp_bytes,
        5i32.to_le_bytes(),
        "HP should stay at the externally-written value once unfrozen"
    );
}

#[test]
fn freeze_detects_the_target_process_exiting() {
    let victim = Victim::spawn();
    let session =
        Arc::new(ProcessSession::attach(victim.pid()).expect("attaching to the victim process"));
    let hp_address = victim.address_of("HP");

    let freeze = session.start_freeze_thread(TEST_FREEZE_INTERVAL);
    freeze.freeze(hp_address, 999i32.to_le_bytes().to_vec());
    assert!(!freeze.target_exited());

    // Victim's Drop normally kills the child on scope exit, but that's too
    // late for this test - kill it now and give the freeze thread a chance
    // to notice on its next tick.
    drop(victim);

    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while !freeze.target_exited() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }

    assert!(
        freeze.target_exited(),
        "freeze thread should have detected the target process exiting within the timeout"
    );
}

#[test]
fn freezing_a_string_entry_pins_its_whole_fixed_width_buffer() {
    // A `Text` entry freezes on `EntryValue::to_le_bytes()`, which is the
    // full declared buffer - so an external write of *shorter* text must be
    // undone across the entire width, not just the bytes the new text
    // happened to touch. Nothing else exercises freeze on a Text value:
    // the GUI's own live click-through covered display and refresh, not
    // this.
    let victim = Victim::spawn();
    let session =
        Arc::new(ProcessSession::attach(victim.pid()).expect("attaching to the victim process"));
    let address = victim.address_of("STR_ASCII");

    // The victim's buffer as it starts: "FerriteVictim" NUL-padded to 32.
    const BUFFER_LEN: usize = 32;
    let original = session
        .read_bytes(address, BUFFER_LEN)
        .expect("reading STR_ASCII");
    let entry = EntryValue::Text {
        bytes: original.clone(),
        encoding: TextEncoding::Latin1,
        zero_terminated: true,
    };
    assert_eq!(
        decode_text(&entry.to_le_bytes(), TextEncoding::Latin1, true),
        "FerriteVictim"
    );

    let freeze = session.start_freeze_thread(TEST_FREEZE_INTERVAL);
    freeze.freeze(address, entry.to_le_bytes());

    // An external write of shorter text: this leaves "iteVictim" trailing
    // in the buffer, so restoring only the written prefix would leave a
    // corrupted string behind and still look like the freeze worked.
    session
        .write_bytes(address, b"Zap\0")
        .expect("external write to STR_ASCII");

    wait_for_a_few_ticks();

    let restored = session
        .read_bytes(address, BUFFER_LEN)
        .expect("reading STR_ASCII back");
    assert_eq!(
        restored, original,
        "freeze should restore the whole {BUFFER_LEN}-byte buffer"
    );
    assert_eq!(
        decode_text(&restored, TextEncoding::Latin1, true),
        "FerriteVictim"
    );
}
