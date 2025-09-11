//! Hotki Engine
//!
//! The Hotki Engine crate coordinates side effects for hotkeys:
//! - executes shell commands (first-run + repeats)
//! - relays key chords to the focused app
//! - manages repeat timing and focus/context updates
//! - emits HUD/notification messages to the UI layer
//!
//! This crate is macOS-only by design. It exposes a minimal, documented API:
//! - [`Engine`]: the primary type you construct and drive
//! - [`RepeatSpec`] and [`RepeatObserver`]: instrumentation hooks
//!
//! All other modules are crate-private implementation details.
#![warn(missing_docs)]
#![warn(unsafe_op_in_unsafe_fn)]
use std::{
    sync::atomic::{AtomicU64, Ordering},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

mod deps;
mod error;
mod key_binding;
mod key_state;
mod notification;
mod relay;
mod repeater;
mod ticker;

// Timing constants for warning thresholds
const BIND_UPDATE_WARN_MS: u64 = 10;
const KEY_PROC_WARN_MS: u64 = 5;

use hotki_protocol::MsgToUI;
use keymode::{KeyResponse, State};
use mac_keycode::Chord;
use tracing::{debug, trace, warn};

pub use deps::MockHotkeyApi;
pub use error::{Error, Result};
pub use hotki_world::{WorldEvent, WorldWindow};
pub use notification::NotificationDispatcher;
pub use relay::RelayHandler;
pub use repeater::{RepeatObserver, RepeatSpec, Repeater};

use deps::RealHotkeyApi;
use key_binding::KeyBindingManager;
use key_state::KeyStateTracker;
use mac_winops::ops::{RealWinOps, WinOps};
use repeater::ExecSpec;

#[inline]
fn to_desired(t: config::Toggle) -> mac_winops::Desired {
    match t {
        config::Toggle::On => mac_winops::Desired::On,
        config::Toggle::Off => mac_winops::Desired::Off,
        config::Toggle::Toggle => mac_winops::Desired::Toggle,
    }
}

#[inline]
fn to_move_dir(d: config::Dir) -> mac_winops::MoveDir {
    match d {
        config::Dir::Left => mac_winops::MoveDir::Left,
        config::Dir::Right => mac_winops::MoveDir::Right,
        config::Dir::Up => mac_winops::MoveDir::Up,
        config::Dir::Down => mac_winops::MoveDir::Down,
    }
}

/// Engine coordinates hotkey state, focus context, relays, notifications and repeats.
///
/// Construct via [`Engine::new`], then feed focus events and hotkey events via
/// [`Engine::dispatch`]. Use [`Engine::set_mode`] to install a `Keys` configuration.
#[derive(Clone)]
pub struct Engine {
    /// Keymode state (tracks only location). Always present.
    state: Arc<tokio::sync::Mutex<State>>,
    /// Key binding manager
    binding_manager: Arc<tokio::sync::Mutex<KeyBindingManager>>,
    /// Key state tracker (tracks which keys are held down)
    key_tracker: KeyStateTracker,
    /// Relay handler
    relay_handler: RelayHandler,
    /// Notification dispatcher
    notifier: NotificationDispatcher,
    /// Repeater for shell commands and relays
    repeater: Repeater,
    /// Configuration
    config: Arc<tokio::sync::RwLock<config::Config>>,
    /// Last known focus snapshot (app/title/pid) when using engine-owned watcher
    focus_snapshot: Arc<Mutex<mac_winops::focus::FocusSnapshot>>,
    /// Monotonic token to cancel pending Raise debounces when a new Raise occurs
    raise_nonce: Arc<AtomicU64>,
    /// Last pid explicitly targeted by a Raise action (used as a hint for subsequent Place).
    last_target_pid: Arc<Mutex<Option<i32>>>,
    winops: Arc<dyn WinOps>,
    /// Window world service handle
    world: hotki_world::WorldHandle,
    /// If true, poll focus snapshot synchronously at dispatch; else trust last snapshot.
    sync_focus_on_dispatch: bool,
}

impl Engine {
    fn sync_focus_now(&self) {
        let snap = mac_winops::focus::poll_now();
        if let Ok(mut g) = self.focus_snapshot.lock() {
            *g = snap;
        }
    }
    fn current_snapshot(&self) -> mac_winops::focus::FocusSnapshot {
        self.focus_snapshot
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    fn current_pid(&self) -> i32 {
        self.focus_snapshot.lock().map(|g| g.pid).unwrap_or(-1)
    }
    /// Create a new engine.
    ///
    /// - `manager`: platform hotkey manager used for key registration
    /// - `event_tx`: channel for sending UI messages (`MsgToUI`)
    pub fn new(
        manager: Arc<mac_hotkey::Manager>,
        event_tx: tokio::sync::mpsc::UnboundedSender<MsgToUI>,
    ) -> Self {
        let binding_manager_arc = Arc::new(tokio::sync::Mutex::new(
            KeyBindingManager::new_with_api(Arc::new(RealHotkeyApi::new(manager))),
        ));
        // Create shared focus/relay instances
        let focus_snapshot_arc = Arc::new(Mutex::new(mac_winops::focus::FocusSnapshot::default()));
        let relay_handler = RelayHandler::new();
        let notifier = NotificationDispatcher::new(event_tx.clone());
        let repeater = Repeater::new(
            focus_snapshot_arc.clone(),
            relay_handler.clone(),
            notifier.clone(),
        );
        let config_arc = Arc::new(tokio::sync::RwLock::new(config::Config::from_parts(
            keymode::Keys::default(),
            config::Style::default(),
        )));

        // Prepare shared winops and world before constructing Self
        let winops: Arc<dyn WinOps> = Arc::new(RealWinOps);
        let world = hotki_world::World::spawn(winops.clone(), hotki_world::WorldCfg::default());

        Self {
            state: Arc::new(tokio::sync::Mutex::new(State::new())),
            binding_manager: binding_manager_arc,
            key_tracker: KeyStateTracker::new(),
            relay_handler,
            notifier,
            repeater,
            config: config_arc,
            focus_snapshot: focus_snapshot_arc,
            raise_nonce: Arc::new(AtomicU64::new(0)),
            last_target_pid: Arc::new(Mutex::new(None)),
            winops,
            world,
            sync_focus_on_dispatch: true,
        }
    }

    /// Create a new engine with a custom window-ops implementation (useful for tests).
    pub fn new_with_ops(
        manager: Arc<mac_hotkey::Manager>,
        event_tx: tokio::sync::mpsc::UnboundedSender<MsgToUI>,
        winops: Arc<dyn WinOps>,
    ) -> Self {
        let binding_manager_arc = Arc::new(tokio::sync::Mutex::new(
            KeyBindingManager::new_with_api(Arc::new(RealHotkeyApi::new(manager))),
        ));
        // Create shared focus/relay instances
        let focus_snapshot_arc = Arc::new(Mutex::new(mac_winops::focus::FocusSnapshot::default()));
        let relay_handler = RelayHandler::new();
        let notifier = NotificationDispatcher::new(event_tx.clone());
        let repeater = Repeater::new(
            focus_snapshot_arc.clone(),
            relay_handler.clone(),
            notifier.clone(),
        );
        let config_arc = Arc::new(tokio::sync::RwLock::new(config::Config::from_parts(
            keymode::Keys::default(),
            config::Style::default(),
        )));

        let world = hotki_world::World::spawn(winops.clone(), hotki_world::WorldCfg::default());

        Self {
            state: Arc::new(tokio::sync::Mutex::new(State::new())),
            binding_manager: binding_manager_arc,
            key_tracker: KeyStateTracker::new(),
            relay_handler,
            notifier,
            repeater,
            config: config_arc,
            focus_snapshot: focus_snapshot_arc,
            raise_nonce: Arc::new(AtomicU64::new(0)),
            last_target_pid: Arc::new(Mutex::new(None)),
            winops,
            world,
            sync_focus_on_dispatch: true,
        }
    }

    /// Custom constructor for tests and advanced scenarios.
    /// Allows injecting a `HotkeyApi`, `WinOps`, relay enable flag, and an explicit World handle.
    pub fn new_with_api_and_ops(
        api: Arc<dyn deps::HotkeyApi>,
        event_tx: tokio::sync::mpsc::UnboundedSender<MsgToUI>,
        winops: Arc<dyn WinOps>,
        relay_enabled: bool,
        world: hotki_world::WorldHandle,
    ) -> Self {
        let binding_manager_arc = Arc::new(tokio::sync::Mutex::new(
            KeyBindingManager::new_with_api(api),
        ));
        let focus_snapshot_arc = Arc::new(Mutex::new(mac_winops::focus::FocusSnapshot::default()));
        let relay_handler = RelayHandler::new_with_enabled(relay_enabled);
        let notifier = NotificationDispatcher::new(event_tx.clone());
        let repeater = Repeater::new(
            focus_snapshot_arc.clone(),
            relay_handler.clone(),
            notifier.clone(),
        );
        let config_arc = Arc::new(tokio::sync::RwLock::new(config::Config::from_parts(
            keymode::Keys::default(),
            config::Style::default(),
        )));

        Self {
            state: Arc::new(tokio::sync::Mutex::new(State::new())),
            binding_manager: binding_manager_arc,
            key_tracker: KeyStateTracker::new(),
            relay_handler,
            notifier,
            repeater,
            config: config_arc,
            focus_snapshot: focus_snapshot_arc,
            raise_nonce: Arc::new(AtomicU64::new(0)),
            last_target_pid: Arc::new(Mutex::new(None)),
            winops,
            world,
            sync_focus_on_dispatch: false,
        }
    }

    /// Create an Engine that will own and drive the focus watcher using a Tao EventLoopProxy.
    /// Server migration will switch to this constructor; legacy `new` remains for compatibility.
    pub fn new_with_proxy(
        manager: Arc<mac_hotkey::Manager>,
        event_tx: tokio::sync::mpsc::UnboundedSender<MsgToUI>,
        proxy: tao::event_loop::EventLoopProxy<()>,
    ) -> Self {
        let eng = Self::new(manager, event_tx);

        // Start engine-owned focus watcher and subscribe to snapshots
        let watcher = mac_winops::focus::FocusWatcher::new(proxy);
        let _ = watcher.start();
        let mut rx = watcher.subscribe();

        // Seed current snapshot immediately (best-effort).
        // Prefer the watcher's last snapshot; if it's uninitialized, fall back to
        // querying the current frontmost window via CG to provide a consistent
        // initial focus view before any key handling occurs.
        {
            let eng_clone = eng.clone();
            let winops = eng.winops.clone();
            let mut snap = watcher.current();
            if snap.pid <= 0
                && let Some(w) = winops.frontmost_window()
            {
                snap = mac_winops::focus::FocusSnapshot {
                    app: w.app,
                    title: w.title,
                    pid: w.pid,
                };
            }
            tokio::spawn(async move {
                let _ = eng_clone.on_focus_snapshot(snap).await;
            });
        }

        let eng_clone = eng.clone();
        tokio::spawn(async move {
            while let Some(snap) = rx.recv().await {
                let _ = eng_clone.on_focus_snapshot(snap).await;
            }
        });

        eng
    }

    /// Access the world service handle for event subscriptions and snapshots.
    pub fn world_handle(&self) -> hotki_world::WorldHandle {
        self.world.clone()
    }

    async fn rebind_current_context(&self) -> Result<()> {
        let fs = self.current_snapshot();
        debug!("Rebinding with context: app={}, title={}", fs.app, fs.title);
        self.rebind_and_refresh(&fs.app, &fs.title).await
    }

    async fn rebind_and_refresh(&self, app: &str, title: &str) -> Result<()> {
        tracing::debug!("start app={} title={}", app, title);
        let cfg_guard = self.config.read().await;

        // Ensure valid context first
        {
            let mut st = self.state.lock().await;
            let _ = st.ensure_context(&cfg_guard, app, title);
        }

        // Update HUD next to reflect current state
        {
            trace!("Updating HUD for app={}, title={}", app, title);
            let cursor = {
                let st = self.state.lock().await;
                st.current_cursor()
            };
            // Attach focus context to the cursor we send to the UI
            let cursor = cursor.with_app(hotki_protocol::App {
                app: app.to_string(),
                title: title.to_string(),
                pid: self.current_pid(),
            });
            debug!("HUD update: cursor {:?}", cursor.path());
            self.notifier.send_hud_update_cursor(cursor)?;
        }

        // Determine capture policy via Config + Location
        let cur = {
            let st = self.state.lock().await;
            st.current_cursor()
        };
        // Carry app/title on the cursor for downstream consumers
        let cur_with_app = cur.clone().with_app(hotki_protocol::App {
            app: app.to_string(),
            title: title.to_string(),
            pid: self.current_pid(),
        });
        let hud_visible = cfg_guard.hud_visible(&cur);
        let capture = cfg_guard.mode_requests_capture(&cur);
        {
            let mut mgr = self.binding_manager.lock().await;
            mgr.set_capture_all(hud_visible && capture);
        }

        // Bind keys and perform cleanup on change
        let start = Instant::now();
        let mut key_pairs: Vec<(String, Chord)> = Vec::new();
        let mut dedup = std::collections::HashSet::new();
        let detailed = cfg_guard.hud_keys_ctx(&cur_with_app);
        for (ch, _desc, _attrs, _is_mode) in detailed {
            let ident = ch.to_string();
            if dedup.insert(ident.clone()) {
                key_pairs.push((ident, ch));
            }
        }
        // Keep bind ordering stable for reduced churn and better diffs/logging
        key_pairs.sort_by(|a, b| a.0.cmp(&b.0));
        let key_count = key_pairs.len();
        let mut manager = self.binding_manager.lock().await;
        if manager.update_bindings(key_pairs)? {
            tracing::debug!("bindings updated, clearing repeater + relay");
            // Async clear to avoid blocking the runtime thread
            self.repeater.clear_async().await;
            let pid = self.current_pid();
            self.relay_handler.stop_all(pid);
        }

        let elapsed = start.elapsed();
        if elapsed > Duration::from_millis(BIND_UPDATE_WARN_MS) {
            warn!(
                "Context update bind step took {:?} for {} keys",
                elapsed, key_count
            );
        } else {
            trace!(
                "Context update bind step completed in {:?} for {} keys",
                elapsed, key_count
            );
        }
        Ok(())
    }

    // set_mode removed; use set_config with a full Config instead.

    /// Set full configuration (keys + style) and rebind while preserving UI state.
    ///
    /// We intentionally do not reset the engine `State` here so that the current
    /// HUD location (depth/path) remains stable across theme or config updates.
    /// Path invalidation is handled by `Config::ensure_context` during rebind.
    pub async fn set_config(&mut self, cfg: config::Config) -> Result<()> {
        *self.config.write().await = cfg;
        self.rebind_current_context().await
    }

    // Legacy on_focus_event removed; use on_focus_snapshot instead.

    /// Handle a coalesced focus snapshot produced by an engine-owned watcher.
    pub async fn on_focus_snapshot(&self, snap: mac_winops::focus::FocusSnapshot) -> Result<()> {
        // Update cached snapshot
        if let Ok(mut g) = self.focus_snapshot.lock() {
            *g = snap.clone();
        }
        let start = Instant::now();
        debug!(
            "Focus changed: app='{}' title='{}' pid={}",
            snap.app, snap.title, snap.pid
        );

        // Update HUD and bindings with new context
        self.rebind_and_refresh(&snap.app, &snap.title).await?;
        debug!("Focus snapshot processed in {:?}", start.elapsed());
        Ok(())
    }

    /// Get the current depth (0 = root) if state is initialized.
    pub async fn get_depth(&self) -> usize {
        self.state.lock().await.depth()
    }

    /// Get a read-only snapshot of currently bound keys as (identifier, chord) pairs.
    pub async fn bindings_snapshot(&self) -> Vec<(String, mac_keycode::Chord)> {
        self.binding_manager.lock().await.bindings_snapshot()
    }

    /// Re-export: current world snapshot of windows.
    pub async fn world_snapshot(&self) -> Vec<WorldWindow> {
        self.world.snapshot().await
    }

    /// Re-export: subscribe to world events (Added/Updated/Removed/FocusChanged).
    pub fn world_events(&self) -> tokio::sync::broadcast::Receiver<WorldEvent> {
        self.world.subscribe()
    }

    /// Diagnostics: world status snapshot (counts, timings, permissions).
    pub async fn world_status(&self) -> hotki_world::WorldStatus {
        self.world.status().await
    }

    /// Process a key event and return whether depth changed (requiring rebind)
    async fn handle_key_event(&self, chord: &Chord, identifier: String) -> Result<bool> {
        let start = Instant::now();
        // Ensure our snapshot is fresh before computing context or acting on windows
        // unless explicitly disabled for tests.
        if self.sync_focus_on_dispatch {
            self.sync_focus_now();
        }
        let fs = self.current_snapshot();
        let pid = self.current_pid();

        trace!(
            "Key event received: {} (app: {}, title: {})",
            identifier, fs.app, fs.title
        );

        // CRITICAL: Single lock acquisition to avoid race conditions
        let cfg_for_handle = self.config.read().await;
        let (loc_before, loc_after, response) = {
            let mut st = self.state.lock().await;
            let loc_before = st.current_cursor();
            let resp = st.handle_key_with_context(&cfg_for_handle, chord, &fs.app, &fs.title);
            let loc_after = st.current_cursor();
            (loc_before, loc_after, resp)
        };

        let processing_time = start.elapsed();
        if processing_time > Duration::from_millis(KEY_PROC_WARN_MS) {
            warn!(
                "Key processing took {:?} for {}",
                processing_time, identifier
            );
        }

        // Handle the response (outside the lock for better concurrency)
        match response {
            Ok(KeyResponse::Relay {
                chord: target,
                attrs,
            }) => {
                debug!(
                    "Relay action {} -> {} (noexit={})",
                    identifier,
                    target,
                    attrs.noexit()
                );
                if attrs.noexit() {
                    // Conservative default: avoid repeating Command/Option chords
                    let mods = &target.modifiers;
                    let cmd_like = mods.contains(&mac_keycode::Modifier::Command)
                        || mods.contains(&mac_keycode::Modifier::RightCommand)
                        || mods.contains(&mac_keycode::Modifier::Option)
                        || mods.contains(&mac_keycode::Modifier::RightOption);

                    let repeat = if attrs.repeat_effective() && !cmd_like {
                        Some(RepeatSpec {
                            initial_delay_ms: attrs.repeat_delay,
                            interval_ms: attrs.repeat_interval,
                        })
                    } else {
                        None
                    };

                    // Prefer software repeats when custom timings are provided; otherwise allow OS repeat.
                    // This ensures `repeat_delay`/`repeat_interval` are honored for relay actions.
                    let has_custom_timing =
                        attrs.repeat_delay.is_some() || attrs.repeat_interval.is_some();
                    let allow_os_repeat = repeat.is_some() && !has_custom_timing;
                    self.key_tracker
                        .set_repeat_allowed(&identifier, allow_os_repeat);

                    self.repeater.start(
                        identifier.clone(),
                        ExecSpec::Relay { chord: target },
                        repeat,
                    );
                } else {
                    // Ensure repeats are not acted upon in single-shot case
                    self.key_tracker.set_repeat_allowed(&identifier, false);
                    // Single-shot: Down then Up
                    self.relay_handler
                        .start_relay(identifier.clone(), target.clone(), pid, false);
                    let _ = self.relay_handler.stop_relay(&identifier, pid);
                }
                Ok(())
            }
            Ok(KeyResponse::Fullscreen { desired, kind }) => {
                tracing::debug!(
                    "Engine: fullscreen action received: desired={:?} kind={:?}",
                    desired,
                    kind
                );
                // Map Toggle -> mac_winops::Desired
                let d = to_desired(desired);
                let pid = self.current_pid();
                let res = match kind {
                    config::FullscreenKind::Native => {
                        tracing::debug!(
                            "Engine: queueing fullscreen_native on main thread pid={} d={:?}",
                            pid,
                            d
                        );
                        self.winops.request_fullscreen_native(pid, d)
                    }
                    config::FullscreenKind::Nonnative => {
                        tracing::debug!(
                            "Engine: queueing fullscreen_nonnative pid={} d={:?}",
                            pid,
                            d
                        );
                        self.winops.request_fullscreen_nonnative(pid, d)
                    }
                };
                if let Err(e) = res {
                    let _ = self.notifier.send_error("Fullscreen", format!("{}", e));
                }
                Ok(())
            }
            Ok(KeyResponse::Raise { app, title }) => {
                use hotki_protocol::NotifyKind;
                use regex::Regex;
                // Compile regexes if present; on error, notify and abort this action.
                tracing::debug!("Raise action: app={:?} title={:?}", app, title);
                // Invalidate any pending debounce tasks from previous raise actions
                let nonce = self.raise_nonce.fetch_add(1, Ordering::SeqCst) + 1;
                let mut invalid = false;
                let app_re = if let Some(s) = app.as_ref() {
                    match Regex::new(s) {
                        Ok(r) => Some(r),
                        Err(e) => {
                            self.notifier
                                .send_error("Raise", format!("Invalid app regex: {}", e))?;
                            invalid = true;
                            None
                        }
                    }
                } else {
                    None
                };
                let title_re = if let Some(s) = title.as_ref() {
                    match Regex::new(s) {
                        Ok(r) => Some(r),
                        Err(e) => {
                            self.notifier
                                .send_error("Raise", format!("Invalid title regex: {}", e))?;
                            invalid = true;
                            None
                        }
                    }
                } else {
                    None
                };
                if !invalid {
                    let all = self.winops.list_windows();
                    tracing::debug!("Raise: list_windows count={}", all.len());
                    if all.is_empty() {
                        let _ = self.notifier.send_notification(
                            NotifyKind::Info,
                            "Raise".into(),
                            "No windows on screen".into(),
                        );
                    } else {
                        let cur = self.winops.frontmost_window();
                        tracing::debug!(
                            "Raise: current frontmost={:?}",
                            cur.as_ref().map(|w| (&w.app, &w.title, w.pid, w.id))
                        );

                        let matches = |w: &mac_winops::WindowInfo| -> bool {
                            let aok = app_re.as_ref().map(|r| r.is_match(&w.app)).unwrap_or(true);
                            let tok = title_re
                                .as_ref()
                                .map(|r| r.is_match(&w.title))
                                .unwrap_or(true);
                            aok && tok
                        };
                        let mut idx_match: Vec<usize> = Vec::new();
                        for (i, w) in all.iter().enumerate() {
                            if matches(w) {
                                idx_match.push(i);
                            }
                        }
                        tracing::debug!("Raise: matched count={}", idx_match.len());
                        if idx_match.is_empty() {
                            // Debounce: retry multiple times before notifying; if a match appears, activate it.
                            let app_re2 = app_re.clone();
                            let title_re2 = title_re.clone();
                            let raise_nonce = self.raise_nonce.clone();
                            let last_pid_hint = self.last_target_pid.clone();
                            let winops = self.winops.clone();
                            tokio::spawn(async move {
                                let mut activated = false;
                                for _ in 0..24 {
                                    // Cancel if a new Raise action started
                                    if raise_nonce.load(Ordering::SeqCst) != nonce {
                                        return;
                                    }
                                    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                                    let all2 = winops.list_windows();
                                    if let Some(target) = all2.iter().find(|w| {
                                        let aok = app_re2
                                            .as_ref()
                                            .map(|r| r.is_match(&w.app))
                                            .unwrap_or(true);
                                        let tok = title_re2
                                            .as_ref()
                                            .map(|r| r.is_match(&w.title))
                                            .unwrap_or(true);
                                        aok && tok
                                    }) {
                                        if let Ok(mut g) = last_pid_hint.lock() {
                                            *g = Some(target.pid);
                                        }
                                        let _ = winops.request_activate_pid(target.pid);
                                        activated = true;
                                        break;
                                    }
                                }
                                if !activated {
                                    if raise_nonce.load(Ordering::SeqCst) != nonce {
                                        return;
                                    }
                                    // Quiet fallback: avoid user-facing notification to reduce noise during fast app launches.
                                    tracing::debug!(
                                        "Raise: no matching window after debounce; suppressing notification"
                                    );
                                }
                            });
                        } else {
                            let target_idx = if let Some(c) = &cur {
                                if matches(c) {
                                    // Find the next match behind the current one
                                    let cur_index =
                                        all.iter().position(|w| w.id == c.id && w.pid == c.pid);
                                    if let Some(ci) = cur_index {
                                        idx_match.into_iter().find(|&i| i > ci).unwrap_or(ci)
                                    } else {
                                        idx_match[0]
                                    }
                                } else {
                                    idx_match[0]
                                }
                            } else {
                                idx_match[0]
                            };
                            let target = &all[target_idx];
                            tracing::debug!(
                                "Raise: target pid={} id={} app='{}' title='{}'",
                                target.pid,
                                target.id,
                                target.app,
                                target.title
                            );
                            if let Ok(mut g) = self.last_target_pid.lock() {
                                *g = Some(target.pid);
                            }
                            if let Err(e) = self.winops.request_activate_pid(target.pid) {
                                if let mac_winops::Error::MainThread = e {
                                    tracing::warn!(
                                        "Raise requires main thread; scheduling failed: {}",
                                        e
                                    );
                                }
                                let _ = self.notifier.send_error("Raise", format!("{}", e));
                            }
                        }
                    }
                }
                Ok(())
            }
            Ok(KeyResponse::Place {
                cols,
                rows,
                col,
                row,
            }) => {
                // Prefer pid targeted by the most recent Raise; fall back to CG frontmost (skips our UI) then snapshot pid.
                let (pid, pid_src) = if let Ok(mut g) = self.last_target_pid.lock() {
                    if let Some(p) = *g {
                        *g = None;
                        (p, "raise")
                    } else if let Some(w) = self.winops.frontmost_window() {
                        (w.pid, "frontmost")
                    } else {
                        (self.current_pid(), "snapshot")
                    }
                } else if let Some(w) = self.winops.frontmost_window() {
                    (w.pid, "frontmost")
                } else {
                    (self.current_pid(), "snapshot")
                };

                // Log current focus/frontmost context for diagnostics
                let snap = self.current_snapshot();
                let front = self.winops.frontmost_window();
                if let Some(ref f) = front {
                    tracing::debug!(
                        "Place: chosen pid={} (src={}) | focus app='{}' title='{}' pid={} | frontmost pid={} id={} app='{}' title='{}' cols={} rows={} col={} row={}",
                        pid,
                        pid_src,
                        snap.app,
                        snap.title,
                        snap.pid,
                        f.pid,
                        f.id,
                        f.app,
                        f.title,
                        cols,
                        rows,
                        col,
                        row
                    );
                } else {
                    tracing::debug!(
                        "Place: chosen pid={} (src={}) | focus app='{}' title='{}' pid={} | frontmost=<none> cols={} rows={} col={} row={}",
                        pid,
                        pid_src,
                        snap.app,
                        snap.title,
                        snap.pid,
                        cols,
                        rows,
                        col,
                        row
                    );
                }

                if let Err(e) = self
                    .winops
                    .request_place_grid_focused(pid, cols, rows, col, row)
                {
                    let _ = self.notifier.send_error("Place", format!("{}", e));
                }
                Ok(())
            }
            Ok(KeyResponse::PlaceMove { cols, rows, dir }) => {
                let mdir = to_move_dir(dir);
                if let Some(w) = self.winops.frontmost_window_for_pid(self.current_pid()) {
                    if let Err(e) = self.winops.request_place_move_grid(w.id, cols, rows, mdir) {
                        let _ = self.notifier.send_error("Move", format!("{}", e));
                    }
                } else {
                    let _ = self
                        .notifier
                        .send_error("Move", "No focused window to move".to_string());
                }
                Ok(())
            }
            Ok(KeyResponse::Hide { desired }) => {
                // Event log including current window info
                let snap = self.current_snapshot();
                let front = self.winops.frontmost_window();
                if let Some(ref f) = front {
                    tracing::info!(
                        "Hide: request desired={:?}; focus app='{}' title='{}' pid={}; frontmost pid={} id={} app='{}' title='{}'",
                        desired,
                        snap.app,
                        snap.title,
                        snap.pid,
                        f.pid,
                        f.id,
                        f.app,
                        f.title
                    );
                } else {
                    tracing::info!(
                        "Hide: request desired={:?}; focus app='{}' title='{}' pid={}; frontmost=<none>",
                        desired,
                        snap.app,
                        snap.title,
                        snap.pid
                    );
                }
                tracing::debug!("Hide action received: desired={:?}", desired);
                let d = to_desired(desired);
                // Perform inline to avoid depending on main-thread queueing for smoketest reliability.
                let pid = self.current_pid();
                tracing::debug!("Hide: perform right now for pid={} desired={:?}", pid, d);
                if let Err(e) = self.winops.hide_bottom_left(pid, d) {
                    let _ = self.notifier.send_error("Hide", format!("{}", e));
                }
                Ok(())
            }
            Ok(resp) => {
                trace!("Key response: {:?}", resp);
                // Special-case ShellAsync to start shell repeater if configured
                match resp {
                    KeyResponse::Focus { dir } => {
                        tracing::info!("Engine: focus(dir={:?})", dir);
                        if let Err(e) = self.winops.request_focus_dir(to_move_dir(dir)) {
                            if let mac_winops::Error::MainThread = e {
                                tracing::warn!(
                                    "Focus requires main thread; scheduling failed: {}",
                                    e
                                );
                            }
                            let _ = self.notifier.send_error("Focus", format!("{}", e));
                        }
                        Ok(())
                    }
                    KeyResponse::ShellAsync {
                        command,
                        ok_notify,
                        err_notify,
                        repeat,
                    } => {
                        let exec = ExecSpec::Shell {
                            command,
                            ok_notify,
                            err_notify,
                        };
                        let rep = repeat.map(|r| RepeatSpec {
                            initial_delay_ms: r.initial_delay_ms,
                            interval_ms: r.interval_ms,
                        });
                        self.repeater.start(identifier.clone(), exec, rep);
                        Ok(())
                    }
                    other => self.notifier.handle_key_response(other),
                }
            }
            Err(e) => {
                warn!("Key handler error for {}: {}", identifier, e);
                self.notifier.send_error("Key", e.to_string())?;
                Ok(())
            }
        }?;

        let location_changed = loc_before != loc_after;
        if location_changed {
            debug!(
                "Location changed: {:?} -> {:?} (triggered by key: {})",
                loc_before.path(),
                loc_after.path(),
                identifier
            );
            // Invoke hook to rebind and refresh HUD using current context
            self.rebind_and_refresh(&fs.app, &fs.title).await?;
        }
        trace!(
            "Key event completed in {:?}: {} (location_changed: {})",
            start.elapsed(),
            identifier,
            location_changed
        );
        Ok(location_changed)
    }

    /// Handle a key up event
    fn handle_key_up(&self, identifier: &str) {
        let pid = self.current_pid();
        self.repeater.stop_sync(identifier);
        if self.relay_handler.stop_relay(identifier, pid) {
            debug!("Stopped relay for {}", identifier);
        }
    }

    /// Handle a repeat key event for active relays
    fn handle_repeat(&self, identifier: &str) {
        let pid = self.current_pid();
        // Forward OS repeat to active relay target, if any
        if self.relay_handler.repeat_relay(identifier, pid) {
            // If a software ticker is active for this id, stop it to avoid double repeats.
            if self.repeater.is_ticking(identifier) {
                self.repeater.note_os_repeat(identifier);
            }
            debug!("Repeated relay for {}", identifier);
        }
    }

    /// Dispatch a hotkey event by id, handling all lookups and callback execution internally.
    /// This reduces the server's knowledge about engine internals and avoids repeated async locking.
    pub async fn dispatch(&self, id: u32, kind: mac_hotkey::EventKind, repeat: bool) {
        // Resolve the registration to get identifier and chord
        let (ident, chord) = match self.binding_manager.lock().await.resolve(id) {
            Some((i, c)) => (i, c),
            None => {
                trace!("Dispatch called with unregistered id: {}", id);
                return;
            }
        };

        trace!("Key event: {} {:?} (repeat: {})", ident, kind, repeat);

        // Handle the event based on its kind
        match kind {
            mac_hotkey::EventKind::KeyDown => {
                if repeat {
                    if self.key_tracker.is_down(&ident)
                        && self.key_tracker.is_repeat_allowed(&ident)
                    {
                        self.handle_repeat(&ident);
                    }
                    return;
                }

                self.key_tracker.on_key_down(&ident);

                match self.handle_key_event(&chord, ident.clone()).await {
                    Ok(_depth_changed) => {}
                    Err(e) => {
                        warn!("Key handler failed: {}", e);
                    }
                }
            }
            mac_hotkey::EventKind::KeyUp => {
                self.key_tracker.on_key_up(&ident);
                self.handle_key_up(&ident);
            }
        }
    }
}

impl Engine {
    /// Resolve a registration id for an identifier (e.g., "cmd+k"). Intended for diagnostics/tests.
    pub async fn resolve_id_for_ident(&self, ident: &str) -> Option<u32> {
        self.binding_manager.lock().await.id_for_ident(ident)
    }
}
