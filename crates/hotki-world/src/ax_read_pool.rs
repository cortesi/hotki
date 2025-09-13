use std::{collections::HashMap, sync::OnceLock, thread, time::Duration};

use crossbeam_channel::{self as chan, Receiver, Sender};
use mac_winops::{AxProps, WindowId};
use super::TEST_OVERRIDES;
use parking_lot::RwLock;

// This module introduces a minimal per‑PID AX read worker pool.
// It provides non‑blocking getters that return the last cached value (if any)
// and schedule a background read when missing. On update, the world actor is
// nudged via `Command::HintRefresh` to reconcile promptly.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct Pid(i32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct Key {
    pid: i32,
    id: WindowId,
}

enum Job {
    FocusForPid { pid: i32 },
    TitleForId { pid: i32, id: WindowId },
    PropsForId { pid: i32, id: WindowId },
}

#[derive(Default)]
struct Cache {
    focus_by_pid: HashMap<i32, WindowId>,
    title_by_key: HashMap<Key, String>,
    props_by_key: HashMap<Key, AxProps>,
}

impl Cache {
    fn get_focus(&self, pid: i32) -> Option<WindowId> {
        self.focus_by_pid.get(&pid).copied()
    }
    fn set_focus(&mut self, pid: i32, id: WindowId) {
        self.focus_by_pid.insert(pid, id);
    }
    fn get_title(&self, pid: i32, id: WindowId) -> Option<String> {
        self.title_by_key.get(&Key { pid, id }).cloned()
    }
    fn set_title(&mut self, pid: i32, id: WindowId, title: String) {
        self.title_by_key.insert(Key { pid, id }, title);
    }
    fn get_props(&self, pid: i32, id: WindowId) -> Option<AxProps> {
        self.props_by_key.get(&Key { pid, id }).cloned()
    }
    fn set_props(&mut self, pid: i32, id: WindowId, props: AxProps) {
        self.props_by_key.insert(Key { pid, id }, props);
    }
}

struct Worker {
    _handle: thread::JoinHandle<()>,
    tx: Sender<Job>,
}

pub struct AxReadPool {
    // cache mutated by workers; reads are lock‑free via RwLock read guard
    cache: RwLock<Cache>,
    // one worker per PID created lazily
    workers: RwLock<HashMap<Pid, Worker>>,
    // world actor command sender: used to nudge a reconcile
    world_tx: tokio::sync::mpsc::UnboundedSender<super::Command>,
}

static POOL: OnceLock<AxReadPool> = OnceLock::new();

impl AxReadPool {
    fn get() -> &'static AxReadPool {
        POOL.get().expect("AxReadPool not initialized")
    }

    fn ensure_worker(&self, pid: i32) -> Sender<Job> {
        // fast path: check read lock
        if let Some(w) = self.workers.read().get(&Pid(pid)) {
            return w.tx.clone();
        }
        // slow path: create under write lock if still absent
        let mut wlock = self.workers.write();
        if let Some(w) = wlock.get(&Pid(pid)) {
            return w.tx.clone();
        }
        let (tx, rx): (Sender<Job>, Receiver<Job>) = chan::unbounded();
        let world_tx = self.world_tx.clone();
        let handle = thread::Builder::new()
            .name(format!("ax-read-pid-{}", pid))
            .spawn(move || worker_loop(pid, rx, world_tx))
            .expect("spawn ax read worker");
        wlock.insert(
            Pid(pid),
            Worker {
                _handle: handle,
                tx: tx.clone(),
            },
        );
        tx
    }
}

fn worker_loop(
    pid: i32,
    rx: Receiver<Job>,
    world_tx: tokio::sync::mpsc::UnboundedSender<super::Command>,
) {
    // Backoff to avoid busy loops if a client floods requests
    let min_nudge_gap = Duration::from_millis(16);
    let mut last_nudge = std::time::Instant::now() - min_nudge_gap;
    while let Ok(job) = rx.recv() {
        match job {
            Job::FocusForPid { pid } => {
                if let Some(id) = super::ax_focused_window_id_for_pid(pid) {
                    let pool = AxReadPool::get();
                    let mut c = pool.cache.write();
                    c.set_focus(pid, id);
                }
            }
            Job::TitleForId { pid, id } => {
                if let Some(title) = super::ax_title_for_window_id(id) {
                    let pool = AxReadPool::get();
                    let mut c = pool.cache.write();
                    c.set_title(pid, id, title);
                }
            }
            Job::PropsForId { pid, id } => {
                if let Ok(props) = mac_winops::ax_props_for_window_id(id) {
                    let pool = AxReadPool::get();
                    let mut c = pool.cache.write();
                    c.set_props(pid, id, props);
                }
            }
        }
        // Nudge world to refresh, lightly throttled
        let now = std::time::Instant::now();
        if now.saturating_duration_since(last_nudge) >= min_nudge_gap {
            let _ = world_tx.send(super::Command::HintRefresh);
            last_nudge = now;
        }
    }
    tracing::debug!("ax-read worker exiting for pid={}", pid);
}

// ===== Public API =====

/// Initialize the AX read pool. Idempotent.
pub fn init(world_tx: tokio::sync::mpsc::UnboundedSender<super::Command>) {
    let _ = POOL.set(AxReadPool {
        cache: RwLock::new(Cache::default()),
        workers: RwLock::new(HashMap::new()),
        world_tx,
    });
}

/// Get last cached focused window id for pid; schedule a background refresh if missing.
pub fn focused_id(pid: i32) -> Option<WindowId> {
    let pool = AxReadPool::get();
    if let Some(id) = pool.cache.read().get_focus(pid) {
        return Some(id);
    }
    // In tests, when an override is set, resolve synchronously on the caller thread
    // so that assertions about immediate focus/title precedence remain true.
    let has_test_override = TEST_OVERRIDES.with(|o| o.lock().ax_focus.is_some());
    if has_test_override {
        if let Some(id) = super::ax_focused_window_id_for_pid(pid) {
            let mut c = pool.cache.write();
            c.set_focus(pid, id);
            return Some(id);
        }
    }
    let tx = pool.ensure_worker(pid);
    let _ = tx.send(Job::FocusForPid { pid });
    None
}

/// Get last cached AX title for (pid, id); schedule background refresh if missing.
pub fn title(pid: i32, id: WindowId) -> Option<String> {
    let pool = AxReadPool::get();
    if let Some(t) = pool.cache.read().get_title(pid, id) {
        return Some(t);
    }
    // Respect test overrides synchronously when present.
    let has_test_override = TEST_OVERRIDES.with(|o| o.lock().ax_title.is_some());
    if has_test_override {
        if let Some(t) = super::ax_title_for_window_id(id) {
            let mut c = pool.cache.write();
            c.set_title(pid, id, t.clone());
            return Some(t);
        }
    }
    let tx = pool.ensure_worker(pid);
    let _ = tx.send(Job::TitleForId { pid, id });
    None
}

/// Get last cached `AxProps` for (pid, id); schedule background refresh if missing.
pub fn props(pid: i32, id: WindowId) -> Option<AxProps> {
    let pool = AxReadPool::get();
    if let Some(p) = pool.cache.read().get_props(pid, id) {
        return Some(p);
    }
    let tx = pool.ensure_worker(pid);
    let _ = tx.send(Job::PropsForId { pid, id });
    None
}
