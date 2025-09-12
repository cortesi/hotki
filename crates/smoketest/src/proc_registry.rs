use std::{collections::HashSet, sync::OnceLock};

use parking_lot::Mutex;

static REGISTRY: OnceLock<Mutex<HashSet<i32>>> = OnceLock::new();

fn reg() -> &'static Mutex<HashSet<i32>> {
    REGISTRY.get_or_init(|| Mutex::new(HashSet::new()))
}

pub fn register(pid: i32) {
    let mut g = reg().lock();
    g.insert(pid);
}

pub fn unregister(pid: i32) {
    let mut g = reg().lock();
    g.remove(&pid);
}

pub fn snapshot() -> Vec<i32> {
    reg().lock().iter().copied().collect()
}

pub fn kill_all() {
    for pid in snapshot() {
        unsafe {
            let _ = libc::kill(pid as libc::pid_t, libc::SIGKILL);
        }
    }
}
