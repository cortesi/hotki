use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

static REGISTRY: OnceLock<Mutex<HashSet<i32>>> = OnceLock::new();

fn reg() -> &'static Mutex<HashSet<i32>> {
    REGISTRY.get_or_init(|| Mutex::new(HashSet::new()))
}

pub fn register(pid: i32) {
    if let Ok(mut g) = reg().lock() {
        g.insert(pid);
    }
}

pub fn unregister(pid: i32) {
    if let Ok(mut g) = reg().lock() {
        g.remove(&pid);
    }
}

pub fn snapshot() -> Vec<i32> {
    reg()
        .lock()
        .map(|g| g.iter().copied().collect())
        .unwrap_or_default()
}

pub fn kill_all() {
    for pid in snapshot() {
        unsafe {
            let _ = libc::kill(pid as libc::pid_t, libc::SIGKILL);
        }
    }
}
