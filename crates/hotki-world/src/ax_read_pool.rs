use std::{
    collections::HashMap,
    sync::{OnceLock, atomic::{AtomicUsize, Ordering}},
    thread,
    time::{Duration, Instant},
};

use crossbeam_channel::{self as chan, Receiver, Sender, select};
use mac_winops::{AxProps, WindowId};
use parking_lot::{RwLock, Mutex};

use super::TEST_OVERRIDES;

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

#[derive(Debug, Clone, Copy)]
enum Job {
    FocusForPid { pid: i32 },
    TitleForId { pid: i32, id: WindowId },
    PropsForId { pid: i32, id: WindowId },
}

#[derive(Debug, Clone, Copy)]
struct TimedJob {
    job: Job,
    deadline: Instant,
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
    tx: Sender<TimedJob>,
}

pub struct AxReadPool {
    // cache mutated by workers; reads are lock‑free via RwLock read guard
    cache: RwLock<Cache>,
    // one worker per PID created lazily
    workers: RwLock<HashMap<Pid, Worker>>,
    // world actor command sender: used to nudge a reconcile
    world_tx: tokio::sync::mpsc::UnboundedSender<super::Command>,
    // global semaphore implemented via a bounded channel pre-filled with MAX_PARALLEL tokens
    sem_tx: Sender<()>,
    sem_rx: Receiver<()>,
}

static POOL: OnceLock<AxReadPool> = OnceLock::new();

const MAX_PARALLEL: usize = 4;
const READ_DEADLINE_MS: u64 = 200;
// Test metrics: track current and peak in-flight jobs that acquired a permit.
static INFLIGHT: AtomicUsize = AtomicUsize::new(0);
static PEAK_INFLIGHT: AtomicUsize = AtomicUsize::new(0);
static STALE_DROPS: AtomicUsize = AtomicUsize::new(0);

// Global test overrides visible across worker threads (integration tests only use via test_api).
#[derive(Default, Clone)]
struct PoolTestOverrides {
    title: Option<(WindowId, String)>,
    title_delay_ms: Option<u64>,
}

static TEST_OVERRIDES_GLOBAL: OnceLock<Mutex<PoolTestOverrides>> = OnceLock::new();
fn test_overrides() -> &'static Mutex<PoolTestOverrides> {
    TEST_OVERRIDES_GLOBAL.get_or_init(|| Mutex::new(PoolTestOverrides::default()))
}

impl AxReadPool {
    fn get() -> &'static AxReadPool {
        POOL.get().expect("AxReadPool not initialized")
    }

    fn ensure_worker(&self, pid: i32) -> Sender<TimedJob> {
        // fast path: check read lock
        if let Some(w) = self.workers.read().get(&Pid(pid)) {
            return w.tx.clone();
        }
        // slow path: create under write lock if still absent
        let mut wlock = self.workers.write();
        if let Some(w) = wlock.get(&Pid(pid)) {
            return w.tx.clone();
        }
        let (tx, rx): (Sender<TimedJob>, Receiver<TimedJob>) = chan::unbounded();
        let world_tx = self.world_tx.clone();
        let sem_tx = self.sem_tx.clone();
        let sem_rx = self.sem_rx.clone();
        let handle = thread::Builder::new()
            .name(format!("ax-read-pid-{}", pid))
            .spawn(move || worker_loop(pid, rx, world_tx, sem_tx, sem_rx))
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
    rx: Receiver<TimedJob>,
    world_tx: tokio::sync::mpsc::UnboundedSender<super::Command>,
    sem_tx: Sender<()>,
    sem_rx: Receiver<()>,
) {
    // Backoff to avoid busy loops if a client floods requests
    let min_nudge_gap = Duration::from_millis(16);
    let mut last_nudge = std::time::Instant::now() - min_nudge_gap;
    while let Ok(tj) = rx.recv() {
        // Drop immediately if already stale
        if Instant::now() >= tj.deadline {
            STALE_DROPS.fetch_add(1, Ordering::SeqCst);
            continue;
        }
        // Acquire a global permit, but only until the job deadline
        let now = Instant::now();
        let left = tj.deadline.saturating_duration_since(now);
        let have_permit = if sem_rx.try_recv().is_ok() {
            true
        } else {
            // Timed wait until the deadline for a permit
            select! {
                recv(sem_rx) -> _ => true,
                recv(chan::after(left)) -> _ => false,
            }
        };
        if !have_permit {
            continue;
        }

        // Ensure we release the permit even if the job path early-returns.
        struct Permit<'a> { tx: &'a Sender<()> }
        impl<'a> Drop for Permit<'a> {
            fn drop(&mut self) {
                INFLIGHT.fetch_sub(1, Ordering::SeqCst);
                let _ = self.tx.send(());
            }
        }
        // Bump inflight and update peak under contention.
        let cur = INFLIGHT.fetch_add(1, Ordering::SeqCst) + 1;
        loop {
            let prev = PEAK_INFLIGHT.load(Ordering::Relaxed);
            if cur <= prev { break; }
            if PEAK_INFLIGHT.compare_exchange(prev, cur, Ordering::SeqCst, Ordering::Relaxed).is_ok() { break; }
        }
        let _permit = Permit { tx: &sem_tx };

        match tj.job {
            Job::FocusForPid { pid } => {
                if let Some(id) = super::ax_focused_window_id_for_pid(pid)
                    && Instant::now() < tj.deadline
                {
                    let pool = AxReadPool::get();
                    let mut c = pool.cache.write();
                    c.set_focus(pid, id);
                }
            }
            Job::TitleForId { pid, id } => {
                // Prefer global test override (cross-thread); otherwise defer to lib shim.
                let title_opt = {
                    let o = test_overrides().lock();
                    if let Some(ms) = o.title_delay_ms { std::thread::sleep(Duration::from_millis(ms)); }
                    if let Some((tid, ref t)) = o.title && tid == id { Some(t.clone()) } else { super::ax_title_for_window_id(id) }
                };
                if let Some(title) = title_opt {
                    if Instant::now() < tj.deadline {
                        let pool = AxReadPool::get();
                        let mut c = pool.cache.write();
                        c.set_title(pid, id, title);
                    } else {
                        STALE_DROPS.fetch_add(1, Ordering::SeqCst);
                    }
                }
            }
            Job::PropsForId { pid, id } => {
                if let Ok(props) = mac_winops::ax_props_for_window_id(id)
                    && Instant::now() < tj.deadline
                {
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
    // Create a bounded channel and prefill it with MAX_PARALLEL tokens.
    let (sem_tx, sem_rx) = chan::bounded::<()>(MAX_PARALLEL);
    for _ in 0..MAX_PARALLEL {
        let _ = sem_tx.send(());
    }

    let _ = POOL.set(AxReadPool {
        cache: RwLock::new(Cache::default()),
        workers: RwLock::new(HashMap::new()),
        world_tx,
        sem_tx,
        sem_rx,
    });
}

/// Get last cached focused window id for pid; schedule a background refresh if missing.
pub fn focused_id(pid: i32) -> Option<WindowId> {
    let pool = AxReadPool::get();
    if let Some(id) = pool.cache.read().get_focus(pid) {
        return Some(id);
    }
    // In tests, when an override is set, resolve synchronously on the caller thread
    // unless tests explicitly force async-only behavior.
    let (has_override, force_async) = TEST_OVERRIDES.with(|o| {
        let s = o.lock();
        (s.ax_focus.is_some(), s.ax_async_only.unwrap_or(false))
    });
    if has_override && !force_async && let Some(id) = super::ax_focused_window_id_for_pid(pid) {
        let mut c = pool.cache.write();
        c.set_focus(pid, id);
        return Some(id);
    }
    let tx = pool.ensure_worker(pid);
    let _ = tx.send(TimedJob {
        job: Job::FocusForPid { pid },
        deadline: Instant::now() + Duration::from_millis(READ_DEADLINE_MS),
    });
    None
}

/// Get last cached AX title for (pid, id); schedule background refresh if missing.
pub fn title(pid: i32, id: WindowId) -> Option<String> {
    let pool = AxReadPool::get();
    if let Some(t) = pool.cache.read().get_title(pid, id) {
        return Some(t);
    }
    // Respect test overrides synchronously when present, unless forced async.
    let (has_override, force_async) = TEST_OVERRIDES.with(|o| {
        let s = o.lock();
        (s.ax_title.is_some(), s.ax_async_only.unwrap_or(false))
    });
    if has_override && !force_async && let Some(t) = super::ax_title_for_window_id(id) {
        let mut c = pool.cache.write();
        c.set_title(pid, id, t.clone());
        return Some(t);
    }
    let tx = pool.ensure_worker(pid);
    let _ = tx.send(TimedJob {
        job: Job::TitleForId { pid, id },
        deadline: Instant::now() + Duration::from_millis(READ_DEADLINE_MS),
    });
    None
}

/// Get last cached `AxProps` for (pid, id); schedule background refresh if missing.
pub fn props(pid: i32, id: WindowId) -> Option<AxProps> {
    let pool = AxReadPool::get();
    if let Some(p) = pool.cache.read().get_props(pid, id) {
        return Some(p);
    }
    let tx = pool.ensure_worker(pid);
    let _ = tx.send(TimedJob {
        job: Job::PropsForId { pid, id },
        deadline: Instant::now() + Duration::from_millis(READ_DEADLINE_MS),
    });
    None
}

// ===== Test helpers (exposed via crate::test_api) =====

/// Get current and peak in-flight counts observed since last reset.
pub fn _test_inflight_metrics() -> (usize, usize) {
    (INFLIGHT.load(Ordering::SeqCst), PEAK_INFLIGHT.load(Ordering::SeqCst))
}

/// Reset in-flight metrics and clear the internal caches for deterministic tests.
pub fn _test_reset_metrics_and_cache() {
    INFLIGHT.store(0, Ordering::SeqCst);
    PEAK_INFLIGHT.store(0, Ordering::SeqCst);
    STALE_DROPS.store(0, Ordering::SeqCst);
    let pool = AxReadPool::get();
    let mut c = pool.cache.write();
    *c = Cache::default();
    // Clear global test overrides too.
    *test_overrides().lock() = PoolTestOverrides::default();
}

/// Peek cached title without scheduling a background job.
pub fn _test_peek_title(pid: i32, id: WindowId) -> Option<String> {
    AxReadPool::get().cache.read().get_title(pid, id)
}

/// Peek cached focus without scheduling a background job.
pub fn _test_peek_focus(pid: i32) -> Option<WindowId> {
    AxReadPool::get().cache.read().get_focus(pid)
}

pub fn _test_stale_drop_count() -> usize {
    STALE_DROPS.load(Ordering::SeqCst)
}

pub fn _test_set_title_override(id: WindowId, title: &str) {
    let mut o = test_overrides().lock();
    o.title = Some((id, title.to_string()));
}

pub fn _test_set_title_delay_ms(ms: u64) {
    let mut o = test_overrides().lock();
    o.title_delay_ms = Some(ms);
}
