//! Test-only helper process for `ferrite-core`'s integration tests.
//!
//! On startup, prints the address of each known static value to stdout as
//! `NAME=0xADDRESS`, one per line, followed by a `READY` sentinel line, then
//! blocks reading stdin lines. Sending `exit` (or closing stdin) makes it
//! exit cleanly. Not part of the shipped product.
//!
//! Only addresses are printed — the test harness (`tests/common/mod.rs`)
//! hex-parses everything to the right of the `=`, so the *contents* of these
//! values are hardcoded in the tests instead, the same way `HP`'s initial
//! `100` always has been.

use std::io::{self, BufRead, Write};
use std::sync::atomic::{AtomicI32, AtomicI64, AtomicU8, AtomicUsize, Ordering};

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

// A second hop on top of PTR, giving a genuine two-level chain
// `PTR2 -> PTR -> HP` for v0.2.0's multi-level pointer resolution: with
// offsets `[0, 0]` that's dereference-then-add twice, landing on HP's
// address — not something unit-test arithmetic alone can prove.
static PTR2: AtomicUsize = AtomicUsize::new(0);

/// The text held by both string buffers below. Deliberately ASCII-only (so
/// the `String` buffer is a faithful ASCII/Latin-1 encoding) and distinctive
/// enough not to collide with unrelated bytes in the process. Its ASCII form
/// is not a substring of its UTF-16LE form, so a scan for one never matches
/// the other.
const VICTIM_TEXT: &str = "FerriteVictim";

/// Both string buffers are deliberately longer than the text they hold, with
/// the remainder left as NULs — the fixed-length-buffer shape a `.CT`
/// `<Length>` declares, and the only shape in which v0.2.0's display-only
/// `ZeroTerminate` truncation rule means anything.
const STR_BUFFER_LEN: usize = 32;

// Both encodings must fit with room left over for at least one NUL, or the
// zip-fill below would silently truncate. `len() * 2` is an upper bound on
// the UTF-16LE encoding of any `&str`, and exact for ASCII text.
const _: () = assert!(VICTIM_TEXT.len() * 2 < STR_BUFFER_LEN);

// Arrays of `AtomicU8` (size 1, align 1) are `STR_BUFFER_LEN` contiguous
// bytes, and — like `PTR` above — being written at runtime is what puts them
// in writable memory. That matters: `ProcessSession::writable_regions` only
// yields `PAGE_READWRITE`-family regions, so a plain immutable `static` byte
// array could land in a read-only section where an AOB scan would never see
// it, however readable it is via `ReadProcessMemory`.
static STR_ASCII: [AtomicU8; STR_BUFFER_LEN] = [const { AtomicU8::new(0) }; STR_BUFFER_LEN];
static STR_UNICODE: [AtomicU8; STR_BUFFER_LEN] = [const { AtomicU8::new(0) }; STR_BUFFER_LEN];

/// Copies `bytes` into `buffer`, leaving the rest of it as the NULs it was
/// initialized with.
fn fill(buffer: &[AtomicU8], bytes: impl IntoIterator<Item = u8>) {
    for (slot, byte) in buffer.iter().zip(bytes) {
        slot.store(byte, Ordering::Relaxed);
    }
}

fn main() {
    PTR.store(&raw const HP as usize, Ordering::Relaxed);
    PTR2.store(&raw const PTR as usize, Ordering::Relaxed);

    fill(&STR_ASCII, VICTIM_TEXT.bytes());
    fill(
        &STR_UNICODE,
        VICTIM_TEXT.encode_utf16().flat_map(u16::to_le_bytes),
    );

    let stdout = io::stdout();
    let mut out = stdout.lock();

    writeln!(out, "HP=0x{:X}", &raw const HP as usize).unwrap();
    writeln!(out, "SCORE=0x{:X}", &raw const SCORE as usize).unwrap();
    writeln!(out, "PTR=0x{:X}", &raw const PTR as usize).unwrap();
    writeln!(out, "PTR2=0x{:X}", &raw const PTR2 as usize).unwrap();
    writeln!(
        out,
        "STR_ASCII=0x{:X}",
        (&raw const STR_ASCII).cast::<u8>() as usize
    )
    .unwrap();
    writeln!(
        out,
        "STR_UNICODE=0x{:X}",
        (&raw const STR_UNICODE).cast::<u8>() as usize
    )
    .unwrap();
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
