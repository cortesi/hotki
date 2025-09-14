use std::{collections::HashSet, sync::OnceLock};

use parking_lot::Mutex;

/// Global registry of helper process IDs for best-effort cleanup.
static REGISTRY: OnceLock<Mutex<HashSet<i32>>> = OnceLock::new();

/// Access the global registry, initializing if needed.
fn reg() -> &'static Mutex<HashSet<i32>> {
    REGISTRY.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Register a new process ID in the global registry.
pub fn register(pid: i32) {
    let mut g = reg().lock();
    g.insert(pid);
}

/// Remove a process ID from the global registry.
pub fn unregister(pid: i32) {
    let mut g = reg().lock();
    g.remove(&pid);
}

/// Snapshot the current set of registered PIDs.
pub fn snapshot() -> Vec<i32> {
    reg().lock().iter().copied().collect()
}

/// Kill all registered processes (best-effort cleanup).
pub fn kill_all() {
    for pid in snapshot() {
        unsafe {
            let _ = libc::kill(pid as libc::pid_t, libc::SIGKILL);
        }
    }
}
