//! Focus watcher: observe focused app and window title changes on macOS.
//!
//! - Produces coalesced `FocusSnapshot { app, title, pid }` updates.
//! - Combines CGWindowList polling and Accessibility (AX) notifications,
//!   debounced to avoid double updates on fast app switches.
//! - Exposed from `mac-winops` to avoid a separate crate.

mod ax;
mod ns;
mod watcher;

pub use ns::{install_ns_workspace_observer, post_user_event, set_main_proxy};

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use thiserror::Error;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

/// Snapshot of the current foreground application and focused window title.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FocusSnapshot {
    pub app: String,
    pub title: String,
    pub pid: i32,
}

impl Default for FocusSnapshot {
    fn default() -> Self {
        Self {
            app: String::new(),
            title: String::new(),
            pid: -1,
        }
    }
}

/// Errors that can occur when interacting with focus watcher public APIs.
#[derive(Debug, Error)]
pub enum Error {
    #[error("NS main proxy not set; call set_main_proxy() on the main thread first")]
    MainProxyNotSet,
    #[error("NS main proxy mutex poisoned")]
    MainProxyPoisoned,
    #[error("Failed to post install request to main thread")]
    PostEventFailed,
    #[error("NS observer state mutex poisoned")]
    NsObserverPoisoned,
}

/// Public APIs:
///   - `set_main_proxy`: provide the Tao `EventLoopProxy<()>` for main-thread work.
///   - `post_user_event`: wake the event loop (used to request installs).
///   - `install_ns_workspace_observer`: idempotent main-thread install.
///   - `start_watcher_snapshots`: spawn the background watcher and emit `FocusSnapshot`s.
///
/// Starts the snapshot-based focus watcher.
///
/// Emits `FocusSnapshot` updates when any of app/title/pid changes. Relies on
/// CG polling and AX notifications. NS observer installation is still requested
/// so other main-thread operations can be scheduled safely.
pub fn start_watcher_snapshots(tx: UnboundedSender<FocusSnapshot>) -> Result<(), Error> {
    // We do not set an NS sink here because the snapshot watcher path does not
    // translate NS notifications directly at this stage.
    ns::request_ns_observer_install()?;
    watcher::start_watcher_snapshots(tx);
    Ok(())
}

/// Poll the current focus synchronously and return a `FocusSnapshot`.
///
/// Priority:
/// 1) AX system focus (app + focused window title + pid) if Accessibility is granted.
/// 2) Fallback to CoreGraphics frontmost window info (title may be empty).
/// 3) Default snapshot (pid = -1) if neither is available.
pub fn poll_now() -> FocusSnapshot {
    if let Some((app, title, pid)) = ax::system_focus_snapshot() {
        FocusSnapshot { app, title, pid }
    } else if let Some(w) = crate::frontmost_window() {
        FocusSnapshot {
            app: w.app,
            title: w.title,
            pid: w.pid,
        }
    } else {
        FocusSnapshot::default()
    }
}

/// Engine-friendly watcher handle that owns lifecycle and broadcasts snapshots.
pub struct FocusWatcher {
    proxy: tao::event_loop::EventLoopProxy<()>,
    started: AtomicBool,
    last: Arc<Mutex<FocusSnapshot>>,
    subscribers: Arc<Mutex<Vec<UnboundedSender<FocusSnapshot>>>>,
}

impl FocusWatcher {
    /// Create a new watcher handle with a Tao proxy for main-thread installs.
    pub fn new(proxy: tao::event_loop::EventLoopProxy<()>) -> Self {
        Self {
            proxy,
            started: AtomicBool::new(false),
            last: Arc::new(Mutex::new(FocusSnapshot::default())),
            subscribers: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Start the internal snapshot watcher and NS observer install (idempotent).
    pub fn start(&self) -> Result<(), Error> {
        if self.started.swap(true, Ordering::SeqCst) {
            return Ok(());
        }

        // Ensure NS observer install is requested on the main loop (kept for parity).
        set_main_proxy(self.proxy.clone());
        let _ = post_user_event();

        let (tx_snap, mut rx_snap): (
            UnboundedSender<FocusSnapshot>,
            UnboundedReceiver<FocusSnapshot>,
        ) = unbounded_channel();
        // Spawn background watcher thread
        watcher::start_watcher_snapshots(tx_snap);

        // Seed `last` immediately with a best-effort snapshot so early
        // consumers have a consistent view before the watcher emits.
        {
            let initial = if let Some((app, title, pid)) = ax::system_focus_snapshot() {
                FocusSnapshot { app, title, pid }
            } else if let Some(w) = crate::frontmost_window() {
                FocusSnapshot {
                    app: w.app,
                    title: w.title,
                    pid: w.pid,
                }
            } else {
                FocusSnapshot::default()
            };
            if let Ok(mut g) = self.last.lock() {
                *g = initial;
            }
        }

        // Forwarder thread: update last + broadcast to subscribers
        let last = self.last.clone();
        let subs = self.subscribers.clone();
        std::thread::spawn(move || {
            loop {
                // Non-blocking drain to avoid busy wait
                let mut had = false;
                while let Ok(snap) = rx_snap.try_recv() {
                    had = true;
                    if let Ok(mut g) = last.lock() {
                        *g = snap.clone();
                    }
                    if let Ok(mut v) = subs.lock() {
                        // retain only live subscribers
                        v.retain(|tx| tx.send(snap.clone()).is_ok());
                    }
                }
                if !had {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
            }
        });

        Ok(())
    }

    /// Subscribe to receive coalesced focus snapshots.
    pub fn subscribe(&self) -> UnboundedReceiver<FocusSnapshot> {
        let (tx, rx) = unbounded_channel();
        if let Ok(v) = self.subscribers.lock() {
            // push outside borrow to avoid holding lock long
            drop(v);
        }
        if let Ok(mut v) = self.subscribers.lock() {
            v.push(tx);
        }
        rx
    }

    /// Return the last known snapshot.
    pub fn current(&self) -> FocusSnapshot {
        self.last.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}
