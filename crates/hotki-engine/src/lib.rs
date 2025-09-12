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
//!
//! World Read Path
//! - The engine reads focus/window state exclusively from `hotki-world`.
//! - There is no FocusWatcher and no CoreGraphics/AX fallback path.
//! - Actions call `world.hint_refresh()` to nudge refresh but operate on the
//!   cached world context; dispatch paths are free of synchronous focus reads.
//! - Early startup policy: if the world snapshot is empty, focus-driven
//!   actions are a no-op with a debug log.
//! - Repeat/relay targets follow the world-backed PID cache and hand off
//!   seamlessly when focus changes.
#![warn(missing_docs)]
#![warn(unsafe_op_in_unsafe_fn)]
use std::{
    sync::Arc,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

mod deps;
mod error;
mod focus;
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
use focus::FocusState;
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
    /// Focus-related state (context, pid, last-target, policy)
    focus: FocusState,
    /// Monotonic token to cancel pending Raise debounces when a new Raise occurs
    raise_nonce: Arc<AtomicU64>,
    winops: Arc<dyn WinOps>,
    /// Window world service handle
    world: hotki_world::WorldHandle,
}

impl Engine {
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
        let focus = FocusState::new(true);
        let relay_handler = RelayHandler::new();
        let notifier = NotificationDispatcher::new(event_tx.clone());
        let repeater = Repeater::new(focus.pid.clone(), relay_handler.clone(), notifier.clone());
        let config_arc = Arc::new(tokio::sync::RwLock::new(config::Config::from_parts(
            keymode::Keys::default(),
            config::Style::default(),
        )));

        // Prepare shared winops and world before constructing Self
        let winops: Arc<dyn WinOps> = Arc::new(RealWinOps);
        let world = hotki_world::World::spawn(winops.clone(), hotki_world::WorldCfg::default());

        let eng = Self {
            state: Arc::new(tokio::sync::Mutex::new(State::new())),
            binding_manager: binding_manager_arc,
            key_tracker: KeyStateTracker::new(),
            relay_handler,
            notifier,
            repeater,
            config: config_arc,
            focus,
            raise_nonce: Arc::new(AtomicU64::new(0)),
            winops,
            world,
        };
        eng.spawn_world_focus_subscription();
        eng
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
        let focus = FocusState::new(true);
        let relay_handler = RelayHandler::new();
        let notifier = NotificationDispatcher::new(event_tx.clone());
        let repeater = Repeater::new(focus.pid.clone(), relay_handler.clone(), notifier.clone());
        let config_arc = Arc::new(tokio::sync::RwLock::new(config::Config::from_parts(
            keymode::Keys::default(),
            config::Style::default(),
        )));

        let world = hotki_world::World::spawn(winops.clone(), hotki_world::WorldCfg::default());

        let eng = Self {
            state: Arc::new(tokio::sync::Mutex::new(State::new())),
            binding_manager: binding_manager_arc,
            key_tracker: KeyStateTracker::new(),
            relay_handler,
            notifier,
            repeater,
            config: config_arc,
            focus,
            raise_nonce: Arc::new(AtomicU64::new(0)),
            winops,
            world,
        };
        eng.spawn_world_focus_subscription();
        eng
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
        let focus = FocusState::new(false);
        let relay_handler = RelayHandler::new_with_enabled(relay_enabled);
        let notifier = NotificationDispatcher::new(event_tx.clone());
        let repeater = Repeater::new(focus.pid.clone(), relay_handler.clone(), notifier.clone());
        let config_arc = Arc::new(tokio::sync::RwLock::new(config::Config::from_parts(
            keymode::Keys::default(),
            config::Style::default(),
        )));

        let eng = Self {
            state: Arc::new(tokio::sync::Mutex::new(State::new())),
            binding_manager: binding_manager_arc,
            key_tracker: KeyStateTracker::new(),
            relay_handler,
            notifier,
            repeater,
            config: config_arc,
            focus,
            raise_nonce: Arc::new(AtomicU64::new(0)),
            winops,
            world,
        };
        eng.spawn_world_focus_subscription();
        eng
    }

    /// Access the world service handle for event subscriptions and snapshots.
    pub fn world_handle(&self) -> hotki_world::WorldHandle {
        self.world.clone()
    }

    fn spawn_world_focus_subscription(&self) {
        let world = self.world.clone();
        let focus_ctx = self.focus.ctx.clone();
        let focus_pid = self.focus.pid.clone();
        tokio::spawn(async move {
            let (mut rx, seed) = world.subscribe_with_context().await;
            if let Some(ctx) = seed
                && let Ok(mut g) = focus_ctx.lock()
            {
                let p = ctx.2;
                *g = Some(ctx);
                if let Ok(mut pg) = focus_pid.lock() {
                    *pg = Some(p);
                }
            }
            loop {
                match rx.recv().await {
                    Ok(hotki_world::WorldEvent::FocusChanged(Some(key))) => {
                        if let Some(ctx) = world.context_for_key(key).await
                            && let Ok(mut g) = focus_ctx.lock()
                        {
                            let p = ctx.2;
                            *g = Some(ctx);
                            if let Ok(mut pg) = focus_pid.lock() {
                                *pg = Some(p);
                            }
                        }
                    }
                    Ok(hotki_world::WorldEvent::FocusChanged(None)) => {
                        if let Ok(mut g) = focus_ctx.lock() {
                            *g = None;
                        }
                        if let Ok(mut pg) = focus_pid.lock() {
                            *pg = None;
                        }
                    }
                    Ok(_) => {}
                    Err(_) => break,
                }
            }
        });
    }

    async fn rebind_current_context(&self) -> Result<()> {
        let (app, title, _pid) = self.current_context_tuple();
        debug!("Rebinding with context: app={}, title={}", app, title);
        self.rebind_and_refresh(&app, &title).await
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
                pid: self.current_pid_world_first(),
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
            pid: self.current_pid_world_first(),
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
            let pid = self.current_pid_world_first();
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
        {
            let mut g = self.config.write().await;
            *g = cfg;
        }
        // Write guard is dropped before we rebind to avoid nested lock access.
        self.rebind_current_context().await
    }

    // (No legacy focus snapshot hook; engine relies solely on world.)

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
        // On dispatch, nudge world to refresh and proceed with cached context
        if self.focus.sync_on_dispatch {
            self.world.hint_refresh();
        }
        let (app_ctx, title_ctx, _pid) = self.current_context_tuple();

        trace!(
            "Key event received: {} (app: {}, title: {})",
            identifier, app_ctx, title_ctx
        );

        // CRITICAL: Single lock acquisition to avoid race conditions
        let cfg_for_handle = self.config.read().await;
        let (loc_before, loc_after, response) = {
            let mut st = self.state.lock().await;
            let loc_before = st.current_cursor();
            let resp = st.handle_key_with_context(&cfg_for_handle, chord, &app_ctx, &title_ctx);
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
            }) => self.handle_action_relay(&identifier, target, &attrs).await,
            Ok(KeyResponse::Fullscreen { desired, kind }) => {
                self.handle_action_fullscreen(desired, kind)
            }
            Ok(KeyResponse::Raise { app, title }) => self.handle_action_raise(app, title).await,
            Ok(KeyResponse::Place {
                cols,
                rows,
                col,
                row,
            }) => {
                let raise_pid = self
                    .focus
                    .last_target_pid
                    .lock()
                    .ok()
                    .and_then(|mut g| g.take());
                self.handle_action_place_request(cols, rows, col, row, raise_pid)
                    .await
            }
            Ok(KeyResponse::PlaceMove { cols, rows, dir }) => {
                self.handle_action_place_move(cols, rows, dir).await
            }
            Ok(KeyResponse::Hide { desired }) => self.handle_action_hide(desired).await,
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
                        self.hint_refresh();
                        Ok(())
                    }
                    KeyResponse::ShellAsync {
                        command,
                        ok_notify,
                        err_notify,
                        repeat,
                    } => {
                        self.handle_action_shell(
                            &identifier,
                            command,
                            ok_notify,
                            err_notify,
                            repeat,
                        )
                        .await
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
            self.rebind_and_refresh(&app_ctx, &title_ctx).await?;
        }
        trace!(
            "Key event completed in {:?}: {} (location_changed: {})",
            start.elapsed(),
            identifier,
            location_changed
        );
        Ok(location_changed)
    }

    // Extracted action handlers for clarity and testability
    async fn handle_action_relay(
        &self,
        identifier: &str,
        target: Chord,
        attrs: &keymode::KeysAttrs,
    ) -> Result<()> {
        debug!(
            "Relay action {} -> {} (noexit={})",
            identifier,
            target,
            attrs.noexit()
        );
        let pid = self.current_pid_world_first();
        if attrs.noexit() {
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
            let has_custom_timing = attrs.repeat_delay.is_some() || attrs.repeat_interval.is_some();
            let allow_os_repeat = repeat.is_some() && !has_custom_timing;
            self.key_tracker
                .set_repeat_allowed(identifier, allow_os_repeat);
            self.repeater.start(
                identifier.to_string(),
                ExecSpec::Relay { chord: target },
                repeat,
            );
        } else {
            self.key_tracker.set_repeat_allowed(identifier, false);
            self.relay_handler
                .start_relay(identifier.to_string(), target.clone(), pid, false);
            let _ = self.relay_handler.stop_relay(identifier, pid);
        }
        Ok(())
    }

    fn handle_action_fullscreen(
        &self,
        desired: config::Toggle,
        kind: config::FullscreenKind,
    ) -> Result<()> {
        tracing::debug!(
            "Engine: fullscreen action received: desired={:?} kind={:?}",
            desired,
            kind
        );
        let d = to_desired(desired);
        let pid = self.current_pid_world_first();
        let res = match kind {
            config::FullscreenKind::Native => self.winops.request_fullscreen_native(pid, d),
            config::FullscreenKind::Nonnative => self.winops.request_fullscreen_nonnative(pid, d),
        };
        if let Err(e) = res {
            let _ = self.notifier.send_error("Fullscreen", format!("{}", e));
        }
        self.hint_refresh();
        Ok(())
    }

    async fn handle_action_shell(
        &self,
        id: &str,
        command: String,
        ok_notify: keymode::NotificationType,
        err_notify: keymode::NotificationType,
        repeat: Option<keymode::ShellRepeatConfig>,
    ) -> Result<()> {
        let exec = ExecSpec::Shell {
            command,
            ok_notify,
            err_notify,
        };
        let rep = repeat.map(|r| RepeatSpec {
            initial_delay_ms: r.initial_delay_ms,
            interval_ms: r.interval_ms,
        });
        self.repeater.start(id.to_string(), exec, rep);
        Ok(())
    }

    async fn handle_action_raise(&self, app: Option<String>, title: Option<String>) -> Result<()> {
        use regex::Regex;
        tracing::debug!("Raise action: app={:?} title={:?}", app, title);
        let _ = self.raise_nonce.fetch_add(1, Ordering::SeqCst) + 1;
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
            let mut wsnap = self.world.snapshot().await;
            wsnap.sort_by_key(|w| w.z);
            if wsnap.is_empty() {
                tracing::debug!("Raise(World): empty snapshot; no-op");
            } else {
                let focused = self.world.focused_window().await;
                let matches = |w: &hotki_world::WorldWindow| -> bool {
                    let aok = app_re.as_ref().map(|r| r.is_match(&w.app)).unwrap_or(true);
                    let tok = title_re
                        .as_ref()
                        .map(|r| r.is_match(&w.title))
                        .unwrap_or(true);
                    aok && tok
                };
                let mut idx_match: Vec<usize> = Vec::new();
                for (i, w) in wsnap.iter().enumerate() {
                    if matches(w) {
                        idx_match.push(i);
                    }
                }
                tracing::debug!("Raise(World): matched count={}", idx_match.len());
                if !idx_match.is_empty() {
                    let target_idx = if let Some(c) = focused.as_ref() {
                        if matches(c) {
                            let cur_index =
                                wsnap.iter().position(|w| w.id == c.id && w.pid == c.pid);
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
                    let target = &wsnap[target_idx];
                    tracing::debug!(
                        "Raise(World): target pid={} id={} app='{}' title='{}'",
                        target.pid,
                        target.id,
                        target.app,
                        target.title
                    );
                    if let Ok(mut g) = self.focus.last_target_pid.lock() {
                        *g = Some(target.pid);
                    }
                    if let Err(e) = self.winops.request_activate_pid(target.pid) {
                        if let mac_winops::Error::MainThread = e {
                            tracing::warn!("Raise requires main thread; scheduling failed: {}", e);
                        }
                        let _ = self.notifier.send_error("Raise", format!("{}", e));
                    }
                    self.hint_refresh();
                } else {
                    tracing::debug!("Raise(World): no match in snapshot; no-op");
                }
            }
        }
        Ok(())
    }

    async fn handle_action_place_request(
        &self,
        cols: u32,
        rows: u32,
        col: u32,
        row: u32,
        raise_pid: Option<i32>,
    ) -> Result<()> {
        let (pid, pid_src): (i32, &str) = if let Some(p) = raise_pid {
            (p, "raise")
        } else if let Some((_, _, p)) = self.focus.ctx.lock().ok().and_then(|g| g.clone()) {
            (p, "world")
        } else {
            let mut snap = self.world.snapshot().await;
            snap.sort_by_key(|w| w.z);
            if snap.is_empty() {
                tracing::debug!("Place(World): empty snapshot; no-op");
                return Ok(());
            } else if let Some(w) = self
                .world
                .focused_window()
                .await
                .or_else(|| snap.first().cloned())
            {
                (w.pid, "world_top")
            } else {
                tracing::debug!("Place(World): no top window; no-op");
                return Ok(());
            }
        };

        let (wapp, wtitle, wpid) = self.current_context_tuple();
        let mut snap = self.world.snapshot().await;
        snap.sort_by_key(|w| w.z);
        if let Some(top) = snap.first() {
            tracing::debug!(
                "Place: chosen pid={} (src={}) | world_focus app='{}' title='{}' pid={} | world_top pid={} id={} app='{}' title='{}' cols={} rows={} col={} row={}",
                pid,
                pid_src,
                wapp,
                wtitle,
                wpid,
                top.pid,
                top.id,
                top.app,
                top.title,
                cols,
                rows,
                col,
                row
            );
        } else {
            tracing::debug!(
                "Place: chosen pid={} (src={}) | world_focus app='{}' title='{}' pid={} | world_top=<none> cols={} rows={} col={} row={}",
                pid,
                pid_src,
                wapp,
                wtitle,
                wpid,
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
        self.hint_refresh();
        Ok(())
    }

    async fn handle_action_place_move(&self, cols: u32, rows: u32, dir: config::Dir) -> Result<()> {
        let mdir = to_move_dir(dir);
        let pid = self.current_pid_world_first();
        let mut snap = self.world.snapshot().await;
        snap.sort_by_key(|w| w.z);
        if !snap.is_empty() {
            let candidate = snap
                .iter()
                .filter(|w| w.pid == pid)
                .min_by_key(|w| (!w.focused, w.z))
                .cloned();
            if let Some(w) = candidate {
                if let Err(e) = self.winops.request_place_move_grid(w.id, cols, rows, mdir) {
                    let _ = self.notifier.send_error("Move", format!("{}", e));
                }
                self.hint_refresh();
            } else {
                let _ = self
                    .notifier
                    .send_error("Move", "No focused window to move".to_string());
            }
        } else {
            let _ = self
                .notifier
                .send_error("Move", "No focused window to move".to_string());
        }
        Ok(())
    }

    async fn handle_action_hide(&self, desired: config::Toggle) -> Result<()> {
        let (wapp, wtitle, wpid) = self.current_context_tuple();
        let mut snap = self.world.snapshot().await;
        snap.sort_by_key(|w| w.z);
        if let Some(top) = snap.first() {
            tracing::info!(
                "Hide: request desired={:?}; world_focus app='{}' title='{}' pid={}; world_top pid={} id={} app='{}' title='{}'",
                desired,
                wapp,
                wtitle,
                wpid,
                top.pid,
                top.id,
                top.app,
                top.title
            );
        } else {
            tracing::info!(
                "Hide: request desired={:?}; world_focus app='{}' title='{}' pid={}; world_top=<none>",
                desired,
                wapp,
                wtitle,
                wpid
            );
        }
        tracing::debug!("Hide action received: desired={:?}", desired);
        let d = to_desired(desired);
        let pid = self.current_pid_world_first();
        tracing::debug!("Hide: perform right now for pid={} desired={:?}", pid, d);
        if let Err(e) = self.winops.hide_bottom_left(pid, d) {
            let _ = self.notifier.send_error("Hide", format!("{}", e));
        }
        self.hint_refresh();
        Ok(())
    }

    /// Handle a key up event
    fn handle_key_up(&self, identifier: &str) {
        let pid = self.current_pid_world_first();
        self.repeater.stop_sync(identifier);
        if self.relay_handler.stop_relay(identifier, pid) {
            debug!("Stopped relay for {}", identifier);
        }
    }

    /// Handle a repeat key event for active relays
    fn handle_repeat(&self, identifier: &str) {
        let pid = self.current_pid_world_first();
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

impl Engine {
    fn current_pid_world_first(&self) -> i32 {
        if let Ok(g) = self.focus.ctx.lock()
            && let Some((_, _, p)) = &*g
        {
            return *p;
        }
        if let Ok(pg) = self.focus.pid.lock() {
            return (*pg).unwrap_or(-1);
        }
        -1
    }

    fn current_context_tuple(&self) -> (String, String, i32) {
        if let Ok(g) = self.focus.ctx.lock()
            && let Some((a, t, p)) = &*g
        {
            return (a.clone(), t.clone(), *p);
        }
        (String::new(), String::new(), -1)
    }

    fn hint_refresh(&self) {
        self.world.hint_refresh();
    }
}
