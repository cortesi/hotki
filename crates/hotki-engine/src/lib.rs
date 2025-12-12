#![deny(clippy::disallowed_methods)]
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
//! - [`RepeatSpec`], [`OnRelayRepeat`], [`OnShellRepeat`]: instrumentation hooks
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
//!
//! Concurrency and Lock Ordering
//! - The engine uses a handful of locks. To avoid deadlocks and priority
//!   inversions, follow this order when multiple guards are needed:
//!   1) `config: RwLock<Config>` (read guard), 2) `state: Mutex<State>`,
//!   3) `binding_manager: Mutex<KeyBindingManager>`. Avoid holding a write
//!      guard across any call that can block or `await`.
//! - `focus_ctx` uses `parking_lot::Mutex` for synchronous PID access by Repeater.
//!   Never hold this guard across an `.await`. Clone/copy values out and drop
//!   the guard before awaiting.
//! - Service calls (`world`, `repeater`, `relay`, `notifier`) must
//!   not be awaited while any of the async engine mutexes are held. Acquire,
//!   compute, drop guards, then perform async work.
//! - `set_config` acquires a write guard, replaces the config, drops the guard,
//!   then triggers a rebind. Do not re-enter config while a write guard is held.
#![warn(missing_docs)]
#![warn(unsafe_op_in_unsafe_fn)]

/// Test support utilities exported for the test suite.
pub mod test_support;
use std::{
    sync::Arc,
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

use config::keymode::{KeyResponse, State};
pub use deps::MockHotkeyApi;
use deps::RealHotkeyApi;
pub use error::{Error, Result};
use hotki_protocol::{DisplaysSnapshot, MsgToUI};
use hotki_world::{FocusChange, WorldView};
pub use hotki_world::{WorldEvent, WorldWindow};
use key_binding::KeyBindingManager;
use key_state::KeyStateTracker;
use mac_keycode::Chord;
pub use notification::NotificationDispatcher;
use parking_lot::Mutex;
pub use relay::RelayHandler;
use repeater::ExecSpec;
pub use repeater::{OnRelayRepeat, OnShellRepeat, RepeatSpec, Repeater};
use tracing::{debug, trace, warn};

/// Engine coordinates hotkey state, focus context, relays, notifications and repeats.
///
/// Construct via [`Engine::new`], then feed focus events and hotkey events via
/// [`Engine::dispatch`]. Use [`Engine::set_config`] to install a full configuration.
///
/// # Focus Context
///
/// The engine caches focus context `(app, title, pid)` from World events in `focus_ctx`.
/// This uses `parking_lot::Mutex` for fast, synchronous access by the Repeater's PID
/// lookup. Do not hold this guard across `.await` points.
#[derive(Clone)]
pub struct Engine {
    /// Keymode state (tracks only location). Always present.
    state: Arc<tokio::sync::Mutex<State>>,
    /// Key binding manager
    binding_manager: Arc<tokio::sync::Mutex<KeyBindingManager>>,
    /// Key state tracker (tracks which keys are held down)
    key_tracker: KeyStateTracker,
    /// Configuration
    config: Arc<tokio::sync::RwLock<config::Config>>,
    /// Cached focus context from World events: `(app, title, pid)`.
    focus_ctx: Arc<Mutex<Option<(String, String, i32)>>>,
    /// If true, hint world refresh on dispatch; else trust cached context.
    sync_on_dispatch: bool,
    /// Last displays snapshot sent to the UI.
    display_snapshot: Arc<tokio::sync::Mutex<DisplaysSnapshot>>,
    /// Key relay handler for forwarding keys to focused app.
    relay: RelayHandler,
    /// Notification dispatcher for UI messages.
    notifier: NotificationDispatcher,
    /// Unified repeater for shell commands and key relays.
    repeater: Repeater,
    /// World view for focus and display tracking.
    world: Arc<dyn WorldView>,
}

impl Engine {
    /// Create a new engine.
    ///
    /// - `manager`: platform hotkey manager used for key registration
    /// - `event_tx`: channel for sending UI messages (`MsgToUI`)
    pub fn new(
        manager: Arc<mac_hotkey::Manager>,
        event_tx: tokio::sync::mpsc::Sender<MsgToUI>,
    ) -> Self {
        let api = Arc::new(RealHotkeyApi::new(manager));
        let world = hotki_world::World::spawn_default_view(hotki_world::WorldCfg::default());
        Self::build(api, event_tx, true, true, world)
    }

    /// Custom constructor for tests and advanced scenarios.
    /// Allows injecting a `HotkeyApi`, relay enable flag, and an explicit world view.
    pub fn new_with_api_and_world(
        api: Arc<dyn deps::HotkeyApi>,
        event_tx: tokio::sync::mpsc::Sender<MsgToUI>,
        relay_enabled: bool,
        world: Arc<dyn WorldView>,
    ) -> Self {
        Self::build(api, event_tx, relay_enabled, false, world)
    }

    fn build(
        api: Arc<dyn deps::HotkeyApi>,
        event_tx: tokio::sync::mpsc::Sender<MsgToUI>,
        relay_enabled: bool,
        sync_on_dispatch: bool,
        world: Arc<dyn WorldView>,
    ) -> Self {
        let binding_manager_arc = Arc::new(tokio::sync::Mutex::new(
            KeyBindingManager::new_with_api(api),
        ));
        let focus_ctx = Arc::new(Mutex::new(None));
        let relay = RelayHandler::new_with_enabled(relay_enabled);
        let notifier = NotificationDispatcher::new(event_tx.clone());
        let repeater = Repeater::new_with_ctx(focus_ctx.clone(), relay.clone(), notifier.clone());
        let config_arc = Arc::new(tokio::sync::RwLock::new(config::Config::from_parts(
            config::Keys::default(),
            config::Style::default(),
        )));

        let eng = Self {
            state: Arc::new(tokio::sync::Mutex::new(State::new())),
            binding_manager: binding_manager_arc,
            key_tracker: KeyStateTracker::new(),
            config: config_arc,
            focus_ctx,
            sync_on_dispatch,
            display_snapshot: Arc::new(tokio::sync::Mutex::new(DisplaysSnapshot::default())),
            relay,
            notifier,
            repeater,
            world,
        };
        eng.spawn_world_focus_subscription();
        eng
    }

    /// Access the world view for event subscriptions and snapshots.
    pub fn world(&self) -> Arc<dyn WorldView> {
        self.world.clone()
    }

    fn spawn_world_focus_subscription(&self) {
        let world = self.world.clone();
        let engine = self.clone();
        tokio::spawn(async move {
            loop {
                let (mut cursor, seed) = world.subscribe_with_context().await;
                if let Err(err) = engine.apply_world_focus_context(seed).await {
                    warn!("World focus seed apply failed: {}", err);
                }

                let mut last_lost = cursor.lost_count;
                loop {
                    let deadline =
                        tokio::time::Instant::now() + tokio::time::Duration::from_secs(30);
                    match world.next_event_until(&mut cursor, deadline).await {
                        Some(event) => {
                            if cursor.lost_count > last_lost {
                                warn!(
                                    lost = cursor.lost_count - last_lost,
                                    "World focus subscription observed lost events; resubscribing"
                                );
                                break;
                            }
                            last_lost = cursor.lost_count;
                            if let hotki_world::WorldEvent::FocusChanged(change) = event {
                                let world_clone = world.clone();
                                let engine_clone = engine.clone();
                                tokio::spawn(async move {
                                    Engine::handle_focus_change_event(
                                        engine_clone,
                                        world_clone,
                                        change,
                                    )
                                    .await;
                                });
                            }
                            if let Err(err) = engine.refresh_displays_if_changed(&world).await {
                                warn!("Display refresh after world event failed: {}", err);
                            }
                        }
                        None => {
                            if cursor.is_closed() {
                                warn!("World focus subscription closed; exiting");
                                return;
                            }
                            if let Err(err) = engine.refresh_displays_if_changed(&world).await {
                                warn!("Display refresh after world timeout failed: {}", err);
                            }
                        }
                    }
                }
            }
        });
    }

    async fn apply_world_focus_context(&self, ctx: Option<(String, String, i32)>) -> Result<()> {
        let mut changed = false;
        // NOTE: focus_ctx uses parking_lot::Mutex. Guard must be dropped before await.
        {
            let mut guard = self.focus_ctx.lock();
            if guard.as_ref() != ctx.as_ref() {
                *guard = ctx.clone();
                changed = true;
            }
        }
        if !changed {
            trace!("World focus context unchanged; skipping rebind");
            return Ok(());
        }
        if let Some((ref app, ref title, pid)) = ctx {
            debug!(pid, app = %app, title = %title, "Engine: world focus context updated");
        } else {
            debug!("Engine: world focus context cleared");
        }
        self.rebind_current_context().await
    }

    async fn handle_focus_change_event(
        engine: Engine,
        world: Arc<dyn WorldView>,
        change: FocusChange,
    ) {
        let ctx =
            if let (Some(app), Some(title), Some(pid)) = (change.app, change.title, change.pid) {
                Some((app, title, pid))
            } else if let Some(key) = change.key {
                world.context_for_key(key).await
            } else {
                None
            };

        if let Some(ctx) = ctx {
            if let Err(err) = engine.apply_world_focus_context(Some(ctx)).await {
                warn!("World focus update failed: {}", err);
            }
        } else if change.key.is_none() {
            if let Err(err) = engine.apply_world_focus_context(None).await {
                warn!("World focus clear failed: {}", err);
            }
        } else {
            warn!(key = ?change.key, "World focus context unavailable after focus change");
        }
    }

    async fn rebind_current_context(&self) -> Result<()> {
        let (app, title, _pid) = self.current_context();
        debug!("Rebinding with context: app={}, title={}", app, title);
        self.rebind_and_refresh(&app, &title).await
    }

    async fn refresh_displays_if_changed(&self, world: &Arc<dyn WorldView>) -> Result<()> {
        let snapshot = world.displays().await;
        {
            let mut cache = self.display_snapshot.lock().await;
            if *cache == snapshot {
                return Ok(());
            }
            *cache = snapshot.clone();
        }

        let cursor = {
            let st = self.state.lock().await;
            st.current_cursor()
        };
        let cursor = self.cursor_with_current_app(cursor);
        self.publish_hud_with_displays(cursor, snapshot).await
    }

    async fn rebind_and_refresh(&self, app: &str, title: &str) -> Result<()> {
        tracing::debug!("start app={} title={}", app, title);
        // LOCK ORDER: config (read) -> state -> binding_manager.
        // The config read guard is held for the duration of this function because
        // we reference its data for key resolution. State and binding_manager
        // guards are acquired and released in separate scopes to avoid holding
        // multiple locks simultaneously. Async service calls happen outside all guards.
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
            let cursor = self.cursor_with_current_app(cursor);
            debug!("HUD update: cursor {:?}", cursor.path());
            let displays_snapshot = self.world.displays().await;
            self.publish_hud_with_displays(cursor, displays_snapshot)
                .await?;
        }

        // Determine capture policy via Config + Location
        let cur = {
            let st = self.state.lock().await;
            st.current_cursor()
        };
        let cur_with_app = self.cursor_with_current_app(cur.clone());
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
        // Update bindings without awaiting while the manager lock is held.
        let bindings_changed = {
            let mut manager = self.binding_manager.lock().await;
            manager.update_bindings(key_pairs)?
        };
        if bindings_changed {
            tracing::debug!("bindings updated, clearing repeater + relay");
            // Perform async work after dropping manager guard.
            self.repeater.clear_async().await;
            // Stop all active relays; each relay uses its original target PID.
            self.relay.stop_all();
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

    /// Update HUD with a new cursor + display snapshot and refresh the cache.
    async fn publish_hud_with_displays(
        &self,
        cursor: hotki_protocol::Cursor,
        snapshot: DisplaysSnapshot,
    ) -> Result<()> {
        {
            let mut cache = self.display_snapshot.lock().await;
            *cache = snapshot.clone();
        }
        self.notifier.send_hud_update_cursor(cursor, snapshot)?;
        Ok(())
    }

    // set_mode removed; use set_config with a full Config instead.

    /// Set full configuration (keys + style) and rebind while preserving UI state.
    ///
    /// We intentionally do not reset the engine `State` here so that the current
    /// HUD location (depth/path) remains stable across theme or config updates.
    /// Path invalidation is handled by `Config::ensure_context` during rebind.
    pub async fn set_config(&self, cfg: config::Config) -> Result<()> {
        // LOCK ORDER: config (write) must be released before rebind_current_context
        // which acquires config (read) -> state -> binding_manager.
        {
            let mut g = self.config.write().await;
            *g = cfg;
        }
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
    pub fn world_events(&self) -> hotki_world::EventCursor {
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
        if self.sync_on_dispatch {
            self.world.hint_refresh();
        }
        let (app_ctx, title_ctx, _pid) = self.current_context();

        trace!(
            "Key event received: {} (app: {}, title: {})",
            identifier, app_ctx, title_ctx
        );

        // LOCK ORDER: config (read) -> state. Response handling and rebind happen
        // after releasing state lock to avoid holding guards across async calls.
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
            }) => {
                self.handle_action_relay(&identifier, target, &attrs)
                    .await?
            }
            Ok(KeyResponse::ShellAsync {
                command,
                ok_notify,
                err_notify,
                repeat,
            }) => {
                self.handle_action_shell(&identifier, command, ok_notify, err_notify, repeat)
                    .await?
            }
            Ok(other) => {
                trace!("Key response: {:?}", other);
                self.notifier.handle_key_response(other)?;
            }
            Err(e) => {
                warn!("Key handler error for {}: {}", identifier, e);
                self.notifier.send_error("Key", e.to_string())?;
            }
        };

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
        attrs: &config::KeysAttrs,
    ) -> Result<()> {
        debug!(
            "Relay action {} -> {} (noexit={})",
            identifier,
            target,
            attrs.noexit()
        );
        let pid = self.current_context().2;
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
            self.relay
                .start_relay(identifier.to_string(), target.clone(), pid, false);
            let _ = self.relay.stop_relay(identifier, pid);
        }
        Ok(())
    }

    async fn handle_action_shell(
        &self,
        id: &str,
        command: String,
        ok_notify: config::NotifyKind,
        err_notify: config::NotifyKind,
        repeat: Option<config::keymode::ShellRepeatConfig>,
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

    /// Handle a key up event
    fn handle_key_up(&self, identifier: &str) {
        let pid = self.current_context().2;
        self.repeater.stop_sync(identifier);
        if self.relay.stop_relay(identifier, pid) {
            debug!("Stopped relay for {}", identifier);
        }
    }

    /// Handle a repeat key event for active relays
    fn handle_repeat(&self, identifier: &str) {
        let pid = self.current_context().2;
        // Forward OS repeat to active relay target, if any
        if self.relay.repeat_relay(identifier, pid) {
            // If a software ticker is active for this id, stop it to avoid double repeats.
            if self.repeater.is_ticking(identifier) {
                self.repeater.note_os_repeat(identifier);
            }
            debug!("Repeated relay for {}", identifier);
        }
    }

    /// Dispatch a hotkey event by id, handling all lookups and callback execution internally.
    /// This reduces the server's knowledge about engine internals and avoids repeated async locking.
    pub async fn dispatch(&self, id: u32, kind: mac_hotkey::EventKind, repeat: bool) -> Result<()> {
        // Resolve the registration to get identifier and chord
        let (ident, chord) = match self.binding_manager.lock().await.resolve(id) {
            Some((i, c)) => (i, c),
            None => {
                trace!("Dispatch called with unregistered id: {}", id);
                return Ok(());
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
                    return Ok(());
                }

                self.key_tracker.on_key_down(&ident);

                match self.handle_key_event(&chord, ident.clone()).await {
                    Ok(_depth_changed) => {}
                    Err(e) => {
                        warn!("Key handler failed: {}", e);
                        return Err(e);
                    }
                }
            }
            mac_hotkey::EventKind::KeyUp => {
                self.key_tracker.on_key_up(&ident);
                self.handle_key_up(&ident);
            }
        }
        Ok(())
    }
}

impl Engine {
    /// Resolve a registration id for an identifier (e.g., "cmd+k"). Intended for diagnostics/tests.
    pub async fn resolve_id_for_ident(&self, ident: &str) -> Option<u32> {
        self.binding_manager.lock().await.id_for_ident(ident)
    }
}

impl Engine {
    fn current_context(&self) -> (String, String, i32) {
        if let Some((a, t, p)) = &*self.focus_ctx.lock() {
            return (a.clone(), t.clone(), *p);
        }
        (String::new(), String::new(), -1)
    }

    fn cursor_with_current_app(&self, cursor: hotki_protocol::Cursor) -> hotki_protocol::Cursor {
        let (app, title, pid) = self.current_context();
        cursor.with_app(hotki_protocol::App { app, title, pid })
    }
}
