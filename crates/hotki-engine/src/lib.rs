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
//!   1) `config: RwLock<Option<DynamicConfig>>` (read guard), 2) `runtime: Mutex<RuntimeState>`,
//!   3) `binding_manager: Mutex<KeyBindingManager>`. Avoid holding a write
//!      guard across any call that can block or `await`.
//! - `focus_ctx` uses `parking_lot::Mutex` for synchronous PID access by Repeater.
//!   Never hold this guard across an `.await`. Clone/copy values out and drop
//!   the guard before awaiting.
//! - Service calls (`world`, `repeater`, `relay`, `notifier`) must
//!   not be awaited while any of the async engine mutexes are held. Acquire,
//!   compute, drop guards, then perform async work.
//! - `set_config_path` acquires a write guard, replaces the config, drops the guard,
//!   then triggers a rebind. Do not re-enter config while a write guard is held.
#![warn(missing_docs)]
#![warn(unsafe_op_in_unsafe_fn)]

/// Test support utilities exported for the test suite.
pub mod test_support;
use std::{path::PathBuf, sync::Arc};

mod actions;
mod deps;
mod dispatch;
mod error;
mod key_binding;
mod key_state;
mod notification;
mod refresh;
mod relay;
mod repeater;
mod runtime;
mod selector;
mod selector_controller;
mod ticker;
mod world_sync;

// Timing constants for warning thresholds
const BIND_UPDATE_WARN_MS: u64 = 10;
const KEY_PROC_WARN_MS: u64 = 5;

#[derive(Debug, Clone, Copy, Default)]
struct DispatchOutcome {
    is_nav: bool,
    entered_mode: bool,
}

#[derive(Debug, Clone)]
struct DispatchContext {
    app: String,
    title: String,
    pid: i32,
}

impl DispatchContext {
    fn mode_ctx(&self, rt: &RuntimeState) -> dyn_engine::ModeCtx {
        rt.focus.mode_ctx(rt.hud_visible, rt.depth())
    }

    fn into_focus(self) -> FocusInfo {
        FocusInfo {
            app: self.app,
            title: self.title,
            pid: self.pid,
        }
    }
}

use config::script::engine as dyn_engine;
use deps::RealHotkeyApi;
pub use error::{Error, Result};
use hotki_protocol::{DisplaysSnapshot, MsgToUI};
use hotki_world::{WorldView, WorldWindow};
use key_binding::KeyBindingManager;
use key_state::KeyStateTracker;
use notification::NotificationDispatcher;
use parking_lot::Mutex;
use relay::RelayHandler;
use repeater::Repeater;
pub use repeater::{OnRelayRepeat, OnShellRepeat, RepeatSpec};

use crate::runtime::{FocusInfo, RuntimeState};

/// Engine coordinates hotkey state, focus context, relays, notifications and repeats.
///
/// Construct via [`Engine::new`], then feed focus events and hotkey events via
/// [`Engine::dispatch`]. Use [`Engine::set_config_path`] to install a configuration.
///
/// # Focus Context
///
/// The engine caches world focus snapshots in `focus_ctx`.
/// This uses `parking_lot::Mutex` for fast, synchronous access by the Repeater's PID
/// lookup. Do not hold this guard across `.await` points.
#[derive(Clone)]
pub struct Engine {
    /// Stack-based runtime state (mode stack + focus + theme/user-style).
    runtime: Arc<tokio::sync::Mutex<RuntimeState>>,
    /// Key binding manager
    binding_manager: Arc<tokio::sync::Mutex<KeyBindingManager>>,
    /// Key state tracker (tracks which keys are held down)
    key_tracker: KeyStateTracker,
    /// Configuration
    config: Arc<tokio::sync::RwLock<Option<dyn_engine::DynamicConfig>>>,
    /// Optional path used for `action.reload_config`.
    config_path: Arc<tokio::sync::RwLock<Option<PathBuf>>>,
    /// Cached focus snapshot from World events.
    focus_ctx: Arc<Mutex<Option<hotki_protocol::FocusSnapshot>>>,
    /// If true, hint world refresh on dispatch; else trust cached context.
    sync_on_dispatch: bool,
    /// Last displays snapshot sent to the UI.
    display_snapshot: Arc<tokio::sync::Mutex<DisplaysSnapshot>>,
    /// Key relay handler for forwarding keys to focused app.
    relay: RelayHandler,
    /// Notification dispatcher for UI messages.
    notifier: NotificationDispatcher,
    /// Coalesced wakeups used to refresh selector snapshots during matching.
    selector_notify: Arc<tokio::sync::Notify>,
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
    pub(crate) fn new_with_api_and_world(
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
        let selector_notify = Arc::new(tokio::sync::Notify::new());
        let repeater = Repeater::new_with_ctx(focus_ctx.clone(), relay.clone(), notifier.clone());
        let config_arc = Arc::new(tokio::sync::RwLock::new(None));

        let eng = Self {
            runtime: Arc::new(tokio::sync::Mutex::new(RuntimeState::empty())),
            binding_manager: binding_manager_arc,
            key_tracker: KeyStateTracker::new(),
            config: config_arc,
            config_path: Arc::new(tokio::sync::RwLock::new(None)),
            focus_ctx,
            sync_on_dispatch,
            display_snapshot: Arc::new(tokio::sync::Mutex::new(DisplaysSnapshot::default())),
            relay,
            notifier,
            selector_notify,
            repeater,
            world,
        };
        eng.spawn_world_focus_subscription();
        eng.spawn_selector_notify_task();
        eng
    }

    /// Load and install a dynamic configuration from `path`.
    pub async fn set_config_path(&self, path: PathBuf) -> Result<()> {
        let dyn_cfg = dyn_engine::load_dynamic_config(&path).map_err(|e| Error::Msg(e.pretty()))?;
        let root = dyn_cfg.root();
        let theme_name = dyn_cfg.active_theme().to_string();

        // LOCK ORDER: config (write) must be released before rebind_current_context.
        {
            let mut g = self.config.write().await;
            *g = Some(dyn_cfg);
        }
        {
            let mut g = self.config_path.write().await;
            *g = Some(path);
        }
        {
            let ctx = self.current_dispatch_context();
            let mut rt = self.runtime.lock().await;
            rt.hud_visible = false;
            rt.theme_name = theme_name;
            rt.focus = ctx.into_focus();
            rt.reset_to_root(root);
        }
        self.rebind_current_context().await
    }

    /// Set the active theme by name and re-render the stack.
    pub async fn set_theme(&self, name: &str) -> Result<()> {
        let cfg_guard = self.config.read().await;
        let Some(cfg) = cfg_guard.as_ref() else {
            return Err(Error::Msg("No config loaded; cannot set theme".to_string()));
        };
        let exists = cfg.theme_exists(name);
        drop(cfg_guard);

        if !exists {
            return Err(Error::Msg(format!("Unknown theme: {}", name)));
        }
        {
            let mut rt = self.runtime.lock().await;
            rt.theme_name = name.to_string();
        }
        self.rebind_current_context().await
    }

    // (No legacy focus snapshot hook; engine relies solely on world.)

    /// Get the current depth (0 = root) if state is initialized.
    pub async fn get_depth(&self) -> usize {
        self.runtime.lock().await.depth()
    }

    /// Get a read-only snapshot of currently bound keys as (identifier, chord) pairs.
    pub async fn bindings_snapshot(&self) -> Vec<(String, mac_keycode::Chord)> {
        self.binding_manager.lock().await.bindings_snapshot()
    }

    /// Re-export: current world snapshot of windows.
    pub async fn world_snapshot(&self) -> Vec<WorldWindow> {
        self.world.snapshot().await
    }

    /// Re-export: subscribe to world events (FocusChanged/DisplaysChanged).
    pub fn world_events(&self) -> hotki_world::EventCursor {
        self.world.subscribe()
    }

    /// Diagnostics: world status snapshot (counts, timings, permissions).
    pub async fn world_status(&self) -> hotki_world::WorldStatus {
        self.world.status().await
    }
}
