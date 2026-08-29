//! Test-only helper process for `ferrite-core`'s integration tests.
//!
//! On startup, prints the address of each known static value to stdout as
//! `NAME=0xADDRESS`, one per line, followed by a `READY` sentinel line, then
//! blocks reading stdin lines. Sending `exit` (or closing stdin) makes it
//! exit cleanly. Not part of the shipped product.

use std::io::{self, BufRead, Write};
use std::sync::atomic::{AtomicI32, AtomicI64, AtomicUsize, Ordering};

// `static` (not `static mut`) items have a fixed address for the entire
// process lifetime and can't be optimized into a register — no `unsafe`
// needed to guarantee that, `AtomicI32`/`AtomicI64` are `Sync` on their own.
static HP: AtomicI32 = AtomicI32::new(100);
static SCORE: AtomicI64 = AtomicI64::new(1_000_000);

// A real in-process pointer for `ferrite-core::pointer`'s integration test:
// PTR holds HP's own address, so resolving *(PTR) + 0 should equal HP's
// address, and *(PTR) + 0's contents should be the initial HP value. Set at
// runtime (not `const`), since a static's address isn't known until it's
// actually placed in memory.
static PTR: AtomicUsize = AtomicUsize::new(0);

fn main() {
    PTR.store(&raw const HP as usize, Ordering::Relaxed);

    let stdout = io::stdout();
    let mut out = stdout.lock();

    writeln!(out, "HP=0x{:X}", &raw const HP as usize).unwrap();
    writeln!(out, "SCORE=0x{:X}", &raw const SCORE as usize).unwrap();
    writeln!(out, "PTR=0x{:X}", &raw const PTR as usize).unwrap();
    writeln!(out, "READY").unwrap();
    out.flush().unwrap();

    // Block until told to exit (or stdin closes), so the values above stay
    // alive at a stable address for the parent test to read/write.
    for line in io::stdin().lock().lines() {
        match line {
            Ok(l) if l.trim() == "exit" => break,
            Ok(_) => continue,
            Err(_) => break,
        }
    }
}
