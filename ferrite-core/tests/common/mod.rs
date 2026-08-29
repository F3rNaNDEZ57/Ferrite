//! Shared test-support code: spawning `ferrite-victim` for integration
//! tests that need a real, separate process to attach to.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::time::Duration;

/// Spawns `ferrite-victim`, parses the addresses it prints on startup, and
/// kills it on drop so a panicking test never leaves an orphaned process
/// behind.
pub struct Victim {
    child: Child,
    stdin: ChildStdin,
    addresses: HashMap<String, usize>,
}

impl Victim {
    pub fn spawn() -> Self {
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

    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    pub fn address_of(&self, name: &str) -> usize {
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
