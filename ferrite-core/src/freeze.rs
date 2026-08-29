//! Freeze: a background thread that repeatedly rewrites a set of addresses,
//! pinning their values against whatever the target process does to them.
//! See the concurrency model in the vault's `v1-plan.md`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::session::ProcessSession;

/// How often the freeze thread re-writes every frozen address, by default.
pub const DEFAULT_FREEZE_INTERVAL: Duration = Duration::from_millis(50);

/// Handle to a running freeze thread for one [`ProcessSession`]. The GUI (or
/// any caller) mutates the frozen-address set only through this handle's
/// methods - it never writes memory directly for a frozen entry, so there's
/// exactly one writer.
pub struct FreezeHandle {
    entries: Arc<Mutex<HashMap<usize, Vec<u8>>>>,
    exited: Arc<AtomicBool>,
    running: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl FreezeHandle {
    /// Freezes `address` at exactly `bytes` - the freeze thread rewrites
    /// this value there every tick until [`Self::unfreeze`] is called.
    /// Overwrites any previous freeze at the same address.
    pub fn freeze(&self, address: usize, bytes: Vec<u8>) {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        entries.insert(address, bytes);
    }

    /// Stops rewriting `address`. A no-op if it wasn't frozen.
    pub fn unfreeze(&self, address: usize) {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        entries.remove(&address);
    }

    /// Whether `address` is currently frozen.
    pub fn is_frozen(&self, address: usize) -> bool {
        let entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        entries.contains_key(&address)
    }

    /// Whether the freeze thread has detected that the target process
    /// exited. Once true, it stays true - the thread has already stopped
    /// itself; callers should treat the session as dead (e.g. force a
    /// detach) rather than keep polling.
    pub fn target_exited(&self) -> bool {
        self.exited.load(Ordering::Relaxed)
    }
}

impl Drop for FreezeHandle {
    fn drop(&mut self) {
        // Stop the thread and join it here, before this handle (and, once
        // every other Arc<ProcessSession> clone is gone, the process handle
        // itself) goes away - so a write can never race a CloseHandle.
        self.running.store(false, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl ProcessSession {
    /// Starts a background thread that repeatedly rewrites every frozen
    /// address at `interval`. Takes `&Arc<Self>` (not `&self`) because the
    /// thread needs to keep this session - and its process handle - alive
    /// for as long as it runs, independent of whatever else holds it.
    pub fn start_freeze_thread(self: &Arc<Self>, interval: Duration) -> FreezeHandle {
        let entries: Arc<Mutex<HashMap<usize, Vec<u8>>>> = Arc::new(Mutex::new(HashMap::new()));
        let exited = Arc::new(AtomicBool::new(false));
        let running = Arc::new(AtomicBool::new(true));

        let thread = {
            let session = Arc::clone(self);
            let entries = Arc::clone(&entries);
            let exited = Arc::clone(&exited);
            let running = Arc::clone(&running);

            std::thread::spawn(move || {
                while running.load(Ordering::Relaxed) {
                    std::thread::sleep(interval);
                    if !running.load(Ordering::Relaxed) {
                        break;
                    }

                    // Snapshot under the lock, then write outside it - a
                    // WriteProcessMemory call must never hold the lock the
                    // GUI thread needs for a quick freeze()/unfreeze().
                    let snapshot: Vec<(usize, Vec<u8>)> = {
                        let entries = entries.lock().unwrap_or_else(|e| e.into_inner());
                        entries
                            .iter()
                            .map(|(&address, bytes)| (address, bytes.clone()))
                            .collect()
                    };
                    if snapshot.is_empty() {
                        continue;
                    }

                    let mut any_succeeded = false;
                    for (address, bytes) in &snapshot {
                        if session.write_bytes(*address, bytes).is_ok() {
                            any_succeeded = true;
                        }
                    }

                    // One bad address among several failing isn't process
                    // exit - only check for that (an extra syscall, so only
                    // paid on the failure path) when *everything* failed.
                    if !any_succeeded && session.has_exited() {
                        exited.store(true, Ordering::Relaxed);
                        running.store(false, Ordering::Relaxed);
                        break;
                    }
                }
            })
        };

        FreezeHandle {
            entries,
            exited,
            running,
            thread: Some(thread),
        }
    }
}
