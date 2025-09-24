use std::{
    collections::{HashMap, VecDeque},
    sync::{
        Arc, OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use crossbeam_channel::{self as chan, Receiver, Sender, select};
use mac_winops::{AxProps, WindowId};
use parking_lot::{Mutex, RwLock};

use super::override_value;

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
    DropObservers,
}

#[derive(Debug, Clone, Copy)]
struct TimedJob {
    job: Job,
    deadline: Instant,
}

struct CacheEntry<T> {
    value: T,
    expires_at: Instant,
    order: u64,
}

impl<T> CacheEntry<T> {
    fn new(value: T, order: u64, now: Instant) -> Self {
        Self {
            value,
            expires_at: now + CACHE_TTL,
            order,
        }
    }

    fn is_expired(&self, now: Instant) -> bool {
        now >= self.expires_at
    }
}

#[derive(Default)]
struct Cache {
    focus_by_pid: HashMap<i32, WindowId>,
    title_by_key: HashMap<Key, CacheEntry<String>>,
    title_order: VecDeque<(Key, u64)>,
    props_by_key: HashMap<Key, CacheEntry<AxProps>>,
    props_order: VecDeque<(Key, u64)>,
    next_order: u64,
}

impl Cache {
    fn get_focus(&self, pid: i32) -> Option<WindowId> {
        self.focus_by_pid.get(&pid).copied()
    }

    fn set_focus(&mut self, pid: i32, id: WindowId) {
        self.focus_by_pid.insert(pid, id);
    }

    fn invalidate_focus(&mut self, pid: i32) {
        self.focus_by_pid.remove(&pid);
    }

    fn get_title(&mut self, pid: i32, id: WindowId, now: Instant) -> Option<String> {
        self.prune_titles(now);
        self.title_by_key
            .get(&Key { pid, id })
            .map(|entry| entry.value.clone())
    }

    fn set_title(&mut self, pid: i32, id: WindowId, title: String, now: Instant) {
        let key = Key { pid, id };
        let order = self.next_order;
        self.next_order = self.next_order.wrapping_add(1);
        self.title_by_key
            .insert(key, CacheEntry::new(title, order, now));
        self.title_order.push_back((key, order));
        self.prune_titles(now);
        self.enforce_title_capacity();
    }

    fn get_props(&mut self, pid: i32, id: WindowId, now: Instant) -> Option<AxProps> {
        self.prune_props(now);
        self.props_by_key
            .get(&Key { pid, id })
            .map(|entry| entry.value.clone())
    }

    fn set_props(&mut self, pid: i32, id: WindowId, props: AxProps, now: Instant) {
        let key = Key { pid, id };
        let order = self.next_order;
        self.next_order = self.next_order.wrapping_add(1);
        self.props_by_key
            .insert(key, CacheEntry::new(props, order, now));
        self.props_order.push_back((key, order));
        self.prune_props(now);
        self.enforce_props_capacity();
    }

    fn prune_titles(&mut self, now: Instant) {
        Self::prune_map(&mut self.title_by_key, &mut self.title_order, now);
    }

    fn prune_props(&mut self, now: Instant) {
        Self::prune_map(&mut self.props_by_key, &mut self.props_order, now);
    }

    fn enforce_title_capacity(&mut self) {
        Self::enforce_capacity(&mut self.title_by_key, &mut self.title_order);
    }

    fn enforce_props_capacity(&mut self) {
        Self::enforce_capacity(&mut self.props_by_key, &mut self.props_order);
    }

    fn prune_map<T>(
        map: &mut HashMap<Key, CacheEntry<T>>,
        order: &mut VecDeque<(Key, u64)>,
        now: Instant,
    ) {
        loop {
            let Some((key, token)) = order.front().copied() else {
                break;
            };
            let remove_front = match map.get(&key) {
                Some(entry) if entry.order == token => entry.is_expired(now),
                Some(_) => true,
                None => true,
            };
            if remove_front {
                order.pop_front();
                if let Some(entry) = map.get(&key)
                    && entry.order == token
                    && entry.is_expired(now)
                {
                    map.remove(&key);
                }
                continue;
            }
            break;
        }
    }

    fn enforce_capacity<T>(
        map: &mut HashMap<Key, CacheEntry<T>>,
        order: &mut VecDeque<(Key, u64)>,
    ) {
        while map.len() > MAX_CACHE_ENTRIES {
            let Some((key, token)) = order.pop_front() else {
                break;
            };
            if let Some(entry) = map.get(&key)
                && entry.order == token
            {
                map.remove(&key);
            }
        }
    }

    fn peek_title(&self, pid: i32, id: WindowId) -> Option<String> {
        self.title_by_key
            .get(&Key { pid, id })
            .map(|entry| entry.value.clone())
    }

    fn title_len(&self) -> usize {
        self.title_by_key.len()
    }

    fn props_len(&self) -> usize {
        self.props_by_key.len()
    }
}

#[derive(Clone)]
struct SharedWorldTx {
    inner: Arc<RwLock<tokio::sync::mpsc::UnboundedSender<super::Command>>>,
}

impl SharedWorldTx {
    fn new(tx: tokio::sync::mpsc::UnboundedSender<super::Command>) -> Self {
        Self {
            inner: Arc::new(RwLock::new(tx)),
        }
    }

    fn replace(&self, tx: tokio::sync::mpsc::UnboundedSender<super::Command>) {
        *self.inner.write() = tx;
    }

    fn send(&self, cmd: super::Command) {
        if let Err(err) = self.inner.read().send(cmd) {
            tracing::debug!("ax-read pool hint refresh send failed: {err:?}");
        }
    }
}

struct Worker {
    _handle: thread::JoinHandle<()>,
    tx: Sender<TimedJob>,
}

pub struct AxReadPool {
    // cache mutated by workers; reads are lock-free via RwLock read guard
    cache: RwLock<Cache>,
    // one worker per PID created lazily
    workers: RwLock<HashMap<Pid, Worker>>,
    // world actor command sender: used to nudge a reconcile
    world_tx: SharedWorldTx,
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

const CACHE_TTL: Duration = Duration::from_secs(3);
const MAX_CACHE_ENTRIES: usize = 2048;

// Global test overrides visible across worker threads (integration tests only use via test_api).
#[derive(Default, Clone)]
struct PoolTestOverrides {
    title: Option<(WindowId, String)>,
    title_delay_ms: Option<u64>,
    props: HashMap<Key, AxProps>,
}

static TEST_OVERRIDES_GLOBAL: OnceLock<Mutex<PoolTestOverrides>> = OnceLock::new();
fn test_overrides() -> &'static Mutex<PoolTestOverrides> {
    TEST_OVERRIDES_GLOBAL.get_or_init(|| Mutex::new(PoolTestOverrides::default()))
}

impl AxReadPool {
    fn get() -> &'static AxReadPool {
        POOL.get().expect("AxReadPool not initialized")
    }

    fn rebind_world_tx(&self, world_tx: tokio::sync::mpsc::UnboundedSender<super::Command>) {
        self.world_tx.replace(world_tx);
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

    fn reset(&self) {
        {
            let mut cache = self.cache.write();
            *cache = Cache::default();
        }
        let workers = self.workers.read();
        for worker in workers.values() {
            let _ = worker.tx.send(TimedJob {
                job: Job::DropObservers,
                deadline: Instant::now() + Duration::from_millis(READ_DEADLINE_MS),
            });
        }
        drop(workers);
        let deadline = Instant::now() + Duration::from_millis(READ_DEADLINE_MS);
        while mac_winops::active_ax_observer_count() > 0 && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
    }
}

fn worker_loop(
    pid: i32,
    rx: Receiver<TimedJob>,
    world_tx: SharedWorldTx,
    sem_tx: Sender<()>,
    sem_rx: Receiver<()>,
) {
    // Backoff to avoid busy loops if a client floods requests
    let min_nudge_gap = Duration::from_millis(16);
    let mut last_nudge = std::time::Instant::now() - min_nudge_gap;
    while let Ok(tj) = rx.recv() {
        if matches!(tj.job, Job::DropObservers) {
            let _ = mac_winops::remove_ax_observer(pid);
            continue;
        }
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
        struct Permit<'a> {
            tx: &'a Sender<()>,
        }
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
            if cur <= prev {
                break;
            }
            if PEAK_INFLIGHT
                .compare_exchange(prev, cur, Ordering::SeqCst, Ordering::Relaxed)
                .is_ok()
            {
                break;
            }
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
                    if let Some(ms) = o.title_delay_ms {
                        std::thread::sleep(Duration::from_millis(ms));
                    }
                    if let Some((tid, ref t)) = o.title
                        && tid == id
                    {
                        Some(t.clone())
                    } else {
                        super::ax_title_for_window_id(id)
                    }
                };
                if let Some(title) = title_opt {
                    if Instant::now() < tj.deadline {
                        let pool = AxReadPool::get();
                        let mut c = pool.cache.write();
                        c.set_title(pid, id, title, Instant::now());
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
                    c.set_props(pid, id, props, Instant::now());
                }
            }
            Job::DropObservers => {}
        }
        // Nudge world to refresh, lightly throttled
        let now = std::time::Instant::now();
        if now.saturating_duration_since(last_nudge) >= min_nudge_gap {
            world_tx.send(super::Command::HintRefresh);
            last_nudge = now;
        }
    }
    tracing::debug!("ax-read worker exiting for pid={}", pid);
}

// ===== Public API =====

/// Initialize the AX read pool. Idempotent.
pub fn init(world_tx: tokio::sync::mpsc::UnboundedSender<super::Command>) {
    if let Some(pool) = POOL.get() {
        pool.rebind_world_tx(world_tx);
        return;
    }
    // Create a bounded channel and prefill it with MAX_PARALLEL tokens.
    let (sem_tx, sem_rx) = chan::bounded::<()>(MAX_PARALLEL);
    for _ in 0..MAX_PARALLEL {
        let _ = sem_tx.send(());
    }

    let world_tx = SharedWorldTx::new(world_tx);
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
    let has_override = override_value(|o| o.ax_focus).is_some();
    let force_async = override_value(|o| o.ax_async_only).unwrap_or(false);
    if has_override
        && !force_async
        && let Some(id) = super::ax_focused_window_id_for_pid(pid)
    {
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

/// Drop any cached focus entry for the process and nudge the world to refresh.
pub fn invalidate_focus(pid: i32) {
    invalidate_focus_inner(pid, true);
}

/// Drop any cached focus entry for the process without hinting.
pub fn invalidate_focus_silent(pid: i32) {
    invalidate_focus_inner(pid, false);
}

fn invalidate_focus_inner(pid: i32, send_hint: bool) {
    let pool = AxReadPool::get();
    {
        let mut cache = pool.cache.write();
        cache.invalidate_focus(pid);
    }
    if send_hint {
        pool.world_tx.send(super::Command::HintRefresh);
    }
}

/// Get last cached AX title for (pid, id); schedule background refresh if missing.
pub fn title(pid: i32, id: WindowId) -> Option<String> {
    let pool = AxReadPool::get();
    {
        let now = Instant::now();
        let mut cache = pool.cache.write();
        if let Some(t) = cache.get_title(pid, id, now) {
            return Some(t);
        }
    }
    // Respect test overrides synchronously when present, unless forced async.
    let has_override = override_value(|o| o.ax_title.clone()).is_some();
    let force_async = override_value(|o| o.ax_async_only).unwrap_or(false);
    if has_override
        && !force_async
        && let Some(t) = super::ax_title_for_window_id(id)
    {
        let mut c = pool.cache.write();
        c.set_title(pid, id, t.clone(), Instant::now());
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
    {
        let now = Instant::now();
        let mut cache = pool.cache.write();
        if let Some(p) = cache.get_props(pid, id, now) {
            return Some(p);
        }
    }
    let force_async = override_value(|o| o.ax_async_only).unwrap_or(false);
    let props_override = if force_async {
        None
    } else {
        test_overrides().lock().props.get(&Key { pid, id }).cloned()
    };
    if let Some(props) = props_override {
        let mut cache = pool.cache.write();
        cache.set_props(pid, id, props.clone(), Instant::now());
        return Some(props);
    }
    let tx = pool.ensure_worker(pid);
    let _ = tx.send(TimedJob {
        job: Job::PropsForId { pid, id },
        deadline: Instant::now() + Duration::from_millis(READ_DEADLINE_MS),
    });
    None
}

/// Reset the worker pool, clearing caches and dropping AX observers.
pub fn reset() {
    if let Some(pool) = POOL.get() {
        pool.reset();
    }
}

// ===== Test helpers (exposed via crate::test_api) =====

/// Get current and peak in-flight counts observed since last reset.
pub fn _test_inflight_metrics() -> (usize, usize) {
    (
        INFLIGHT.load(Ordering::SeqCst),
        PEAK_INFLIGHT.load(Ordering::SeqCst),
    )
}

/// Reset in-flight metrics and clear the internal caches for deterministic tests.
pub fn _test_reset_metrics_and_cache() {
    while INFLIGHT.load(Ordering::SeqCst) != 0 {
        thread::sleep(Duration::from_millis(1));
    }
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
    AxReadPool::get().cache.read().peek_title(pid, id)
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

pub fn _test_set_props_override(pid: i32, id: WindowId, props: AxProps) {
    let mut o = test_overrides().lock();
    o.props.insert(Key { pid, id }, props);
}

pub fn _test_clear_overrides() {
    let mut o = test_overrides().lock();
    *o = PoolTestOverrides::default();
}

pub fn _test_cache_usage() -> (usize, usize) {
    let pool = AxReadPool::get();
    let mut cache = pool.cache.write();
    let now = Instant::now();
    cache.prune_titles(now);
    cache.prune_props(now);
    (cache.title_len(), cache.props_len())
}
