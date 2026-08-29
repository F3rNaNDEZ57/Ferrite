//! Spawns `ferrite-victim`, attaches to it via [`ProcessSession`], and
//! round-trips a read/write against its known values. This is the proof that
//! attach/read/write actually work against a *separate* process — everything
//! else in `ferrite-core` is built on this working.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::time::Duration;

use ferrite_core::ProcessSession;

/// Spawns `ferrite-victim`, parses the addresses it prints on startup, and
/// kills it on drop so a panicking test never leaves an orphaned process
/// behind.
struct Victim {
    child: Child,
    stdin: ChildStdin,
    addresses: HashMap<String, usize>,
}

impl Victim {
    fn spawn() -> Self {
        let binary_path = escargot::CargoBuild::new()
            .package("ferrite-victim")
            .bin("ferrite-victim")
            .run()
            .expect("building ferrite-victim")
            .path()
            .to_path_buf();

        let mut child = Command::new(binary_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("spawning ferrite-victim");

        let stdout = child.stdout.take().expect("victim stdout was piped");
        let mut lines = BufReader::new(stdout).lines();

        let mut addresses = HashMap::new();
        for line in &mut lines {
            let line = line.expect("reading victim stdout");
            if line == "READY" {
                break;
            }
            let (name, addr) = line
                .split_once('=')
                .unwrap_or_else(|| panic!("unexpected victim output line: {line:?}"));
            let addr = addr
                .strip_prefix("0x")
                .unwrap_or_else(|| panic!("expected a 0x-prefixed hex address, got: {addr:?}"));
            let addr = usize::from_str_radix(addr, 16)
                .unwrap_or_else(|_| panic!("invalid hex address: {addr:?}"));
            addresses.insert(name.to_string(), addr);
        }

        let stdin = child.stdin.take().expect("victim stdin was piped");
        Self {
            child,
            stdin,
            addresses,
        }
    }

    fn pid(&self) -> u32 {
        self.child.id()
    }

    fn address_of(&self, name: &str) -> usize {
        *self
            .addresses
            .get(name)
            .unwrap_or_else(|| panic!("victim didn't report an address for {name:?}"))
    }
}

impl Drop for Victim {
    fn drop(&mut self) {
        // Ask nicely first...
        let _ = writeln!(self.stdin, "exit");
        let _ = self.stdin.flush();

        // ...but don't let a failing test leave an orphaned process behind:
        // give it a moment to exit on its own, then kill unconditionally.
        for _ in 0..20 {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

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
