#![warn(missing_docs)]
#![deny(clippy::disallowed_methods)]
//! Hotki Engine
//!
//! The Hotki Engine crate coordinates side effects for hotkeys:
//! - executes shell commands (first-run + repeats)
//! - relays key chords to the focused app or one exact running application
//! - manages repeat timing and focus/context updates
//! - emits HUD/notification messages to the UI layer
//!
//! This crate is macOS-only by design. It exposes a minimal, documented API:
//! - [`Engine`]: the primary type you construct and drive
//! - [`RepeatSpec`]: repeat timing overrides
//!
//! All other modules are crate-private implementation details.
//!
//! World Read Path
//! - The engine reads focus/window state exclusively from `hotki-world`.
//! - There is no FocusWatcher and no CoreGraphics/AX fallback path.
//! - Dispatch waits for a fresh world generation before selecting contextual
//!   bindings; other actions operate on the event-maintained focus cache.
//! - Early startup policy: if the world snapshot is empty, focus-driven
//!   actions are a no-op with a debug log.
//! - Focused relays use the global HID path. Application-name relays resolve
//!   once at gesture start and remain pinned through repeat and key-up.
//!
//! Concurrency and Lock Ordering
//! - The engine uses a handful of locks. To avoid deadlocks and priority
//!   inversions, follow this order when multiple guards are needed:
//!   1) `config_transaction`, 2) `config: Mutex<Option<ConfigRuntime>>`,
//!   3) `runtime: Mutex<RuntimeState>`, 4) `binding_manager: Mutex<KeyBindingManager>`.
//!      Avoid holding a write guard across any call that can block or `await`.
//! - `focus_ctx` uses `parking_lot::Mutex` for the event-maintained focus cache.
//!   Never hold this guard across an `.await`. Clone/copy values out and drop
//!   the guard before awaiting.
//! - Service calls (`world`, `repeater`, `relay`, `notifier`) must
//!   not be awaited while any of the async engine mutexes are held. Acquire,
//!   compute, drop guards, then perform async work.
//! - Config installation prepares a candidate before taking active-state locks, then
//!   commits config, runtime, bindings, path, and UI snapshot together.
//! - Selector opening resolves items under `config`, drops that guard, installs selector
//!   state under `runtime`, then publishes UI after guards are released.
#![warn(unsafe_op_in_unsafe_fn)]

#[cfg(test)]
mod test_support;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Weak},
};

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

#[cfg(test)]
#[path = "../tests/integration_tests.rs"]
mod integration_tests;

// Timing constants for warning thresholds
pub(crate) const BIND_UPDATE_WARN_MS: u64 = 10;
const KEY_DISPATCH_WARN_MS: u64 = 100;

/// Post-dispatch behavior requested by a binding or action.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum DispatchResult {
    /// No suppressing behavior occurred; exit the current transient mode.
    #[default]
    AutoExit,
    /// Stay in the current mode without navigation.
    Stay,
    /// Navigation changed HUD visibility or stack state without entering a child mode.
    Navigation,
    /// Dispatch entered a child mode.
    EnteredMode,
    /// Dispatch opened the selector UI.
    SelectorOpened,
}

impl DispatchResult {
    /// Return true when dispatch should auto-exit after executing the binding.
    pub(crate) fn should_auto_exit(self) -> bool {
        matches!(self, Self::AutoExit)
    }

    /// Apply an explicit stay request without masking stronger navigation outcomes.
    pub(crate) fn with_stay(self, stay: bool) -> Self {
        if stay && self.should_auto_exit() {
            Self::Stay
        } else {
            self
        }
    }

    /// Merge two outcomes, preserving the strongest post-dispatch behavior.
    pub(crate) fn combine(self, other: Self) -> Self {
        if other.priority() > self.priority() {
            other
        } else {
            self
        }
    }

    /// Ordering used when several effects request different post-dispatch behavior.
    fn priority(self) -> u8 {
        match self {
            Self::AutoExit => 0,
            Self::Stay => 1,
            Self::Navigation => 2,
            Self::EnteredMode => 3,
            Self::SelectorOpened => 4,
        }
    }
}

use config::runtime as dyn_engine;
use deps::RealHotkeyApi;
pub use error::{Error, Result};
use hotki_protocol::{DisplaysSnapshot, MsgToUI};
use hotki_world::WorldView;
use key_binding::KeyBindingManager;
use key_state::KeyStateTracker;
use notification::NotificationDispatcher;
use parking_lot::Mutex;
use relay::RelayHandler;
#[cfg(test)]
pub(crate) use repeater::OnRelayRepeat;
pub use repeater::RepeatSpec;
use repeater::Repeater;
use ticker::Ticker;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::runtime::RuntimeState;

#[derive(Clone)]
struct HeldBinding {
    identifier: String,
    chord: mac_keycode::Chord,
}

struct EngineLifecycle {
    cancel: CancellationToken,
    tasks: Mutex<Vec<JoinHandle<()>>>,
}

impl EngineLifecycle {
    fn new() -> Self {
        Self {
            cancel: CancellationToken::new(),
            tasks: Mutex::new(Vec::new()),
        }
    }

    fn shutdown(&self) {
        self.cancel.cancel();
        for task in self.tasks.lock().drain(..) {
            task.abort();
        }
    }
}

impl Drop for EngineLifecycle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[derive(Clone)]
enum EngineLifecycleHandle {
    Owner(Arc<EngineLifecycle>),
    Task(Weak<EngineLifecycle>),
}

impl EngineLifecycleHandle {
    fn new() -> Self {
        Self::Owner(Arc::new(EngineLifecycle::new()))
    }

    fn for_background(&self) -> Self {
        match self {
            Self::Owner(lifecycle) => Self::Task(Arc::downgrade(lifecycle)),
            Self::Task(lifecycle) => Self::Task(lifecycle.clone()),
        }
    }

    fn cancellation_token(&self) -> Option<CancellationToken> {
        match self {
            Self::Owner(lifecycle) => Some(lifecycle.cancel.clone()),
            Self::Task(lifecycle) => lifecycle
                .upgrade()
                .map(|lifecycle| lifecycle.cancel.clone()),
        }
    }

    fn register(&self, task: JoinHandle<()>) {
        let Self::Owner(lifecycle) = self else {
            task.abort();
            return;
        };
        if lifecycle.cancel.is_cancelled() {
            task.abort();
        } else {
            lifecycle.tasks.lock().push(task);
        }
    }

    fn is_last_owner(&self) -> bool {
        matches!(self, Self::Owner(lifecycle) if Arc::strong_count(lifecycle) == 1)
    }

    fn shutdown(&self) {
        if let Self::Owner(lifecycle) = self {
            lifecycle.shutdown();
        }
    }

    #[cfg(test)]
    fn weak(&self) -> Weak<EngineLifecycle> {
        match self {
            Self::Owner(lifecycle) => Arc::downgrade(lifecycle),
            Self::Task(lifecycle) => lifecycle.clone(),
        }
    }

    #[cfg(test)]
    fn task_count(&self) -> usize {
        match self {
            Self::Owner(lifecycle) => lifecycle.tasks.lock().len(),
            Self::Task(lifecycle) => lifecycle
                .upgrade()
                .map_or(0, |lifecycle| lifecycle.tasks.lock().len()),
        }
    }
}

/// Engine coordinates hotkey state, focus context, relays, notifications and repeats.
///
/// Construct via [`Engine::new`], then feed focus events and hotkey events via
/// [`Engine::dispatch`]. Use [`Engine::set_config_path`] to install a configuration.
///
/// # Focus Context
///
/// The engine caches world focus snapshots in `focus_ctx`.
/// This uses `parking_lot::Mutex` for fast, synchronous focus reads. Do not hold
/// this guard across `.await` points.
#[derive(Clone)]
pub struct Engine {
    /// Owner for background work spawned by this engine.
    lifecycle: EngineLifecycleHandle,
    /// Stack-based runtime state (mode stack + focus + rendered style).
    runtime: Arc<tokio::sync::Mutex<RuntimeState>>,
    /// Key binding manager
    binding_manager: Arc<tokio::sync::Mutex<KeyBindingManager>>,
    /// Key state tracker (tracks which keys are held down)
    key_tracker: KeyStateTracker,
    /// Binding identities retained until their matching registration ID is released.
    held_bindings: Arc<Mutex<HashMap<u32, HeldBinding>>>,
    /// Configuration
    config: Arc<tokio::sync::Mutex<Option<dyn_engine::ConfigRuntime>>>,
    /// Optional path used for `ctx:reload_config()`.
    config_path: Arc<tokio::sync::RwLock<Option<PathBuf>>>,
    /// Cached focus snapshot from World events.
    focus_ctx: Arc<Mutex<Option<hotki_protocol::FocusSnapshot>>>,
    /// If true, refresh world state before dispatch; else trust cached context.
    sync_on_dispatch: bool,
    /// Last displays snapshot sent to the UI.
    display_snapshot: Arc<tokio::sync::Mutex<DisplaysSnapshot>>,
    /// Key relay handler for destination-pinned gestures.
    relay: RelayHandler,
    /// Notification dispatcher for UI messages.
    notifier: NotificationDispatcher,
    /// Coalesced wakeups used to refresh selector snapshots during matching.
    selector_notify: Arc<tokio::sync::Notify>,
    /// Unified repeater for shell commands and key relays.
    repeater: Repeater,
    /// Repeater for Luau action closures created by `ctx:until_keyup`.
    action_repeater: Ticker,
    /// Serializes candidate preparation and committed refreshes.
    config_transaction: Arc<tokio::sync::Mutex<()>>,
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

    /// Construct an engine around test-owned platform and world adapters.
    #[cfg(test)]
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
        let notifier = NotificationDispatcher::new(event_tx);
        let selector_notify = Arc::new(tokio::sync::Notify::new());
        let repeater = Repeater::new(notifier.clone());
        let action_repeater = Ticker::default();
        let config_arc = Arc::new(tokio::sync::Mutex::new(None));

        let engine = Self {
            lifecycle: EngineLifecycleHandle::new(),
            runtime: Arc::new(tokio::sync::Mutex::new(RuntimeState::empty())),
            binding_manager: binding_manager_arc,
            key_tracker: KeyStateTracker::new(),
            held_bindings: Arc::new(Mutex::new(HashMap::new())),
            config: config_arc,
            config_path: Arc::new(tokio::sync::RwLock::new(None)),
            focus_ctx,
            sync_on_dispatch,
            display_snapshot: Arc::new(tokio::sync::Mutex::new(DisplaysSnapshot::default())),
            relay,
            notifier,
            selector_notify,
            repeater,
            action_repeater,
            config_transaction: Arc::new(tokio::sync::Mutex::new(())),
            world,
        };
        engine.spawn_world_focus_subscription();
        engine.spawn_selector_notify_task();
        engine
    }

    fn clone_for_background(&self) -> Self {
        let mut engine = self.clone();
        engine.lifecycle = self.lifecycle.for_background();
        engine
    }

    fn background_cancellation_token(&self) -> CancellationToken {
        self.lifecycle
            .cancellation_token()
            .expect("engine lifecycle must exist while spawning background work")
    }

    fn register_background_task(&self, task: JoinHandle<()>) {
        self.lifecycle.register(task);
    }

    /// Load and install a dynamic configuration from `path`.
    pub async fn set_config_path(&self, path: PathBuf) -> Result<()> {
        self.install_config(&path, ConfigInstall::ResetFocus).await
    }

    /// Load config from `path` into runtime state.
    ///
    /// `ConfigInstall::ResetFocus` also resets HUD visibility and focus for a fresh install.
    /// `ConfigInstall::KeepFocus` only replaces the mode stack root (config reload).
    pub(crate) async fn install_config(&self, path: &Path, mode: ConfigInstall) -> Result<()> {
        let _transaction = self.config_transaction.lock().await;
        let prepared = self.prepare_config(path, mode).await?;
        self.commit_config(prepared).await
    }

    /// Get the current depth (0 = root) if state is initialized.
    pub async fn get_depth(&self) -> usize {
        self.runtime.lock().await.depth()
    }

    /// Get a read-only snapshot of currently bound keys as (identifier, chord) pairs.
    pub async fn bindings_snapshot(&self) -> Vec<(String, mac_keycode::Chord)> {
        self.binding_manager.lock().await.bindings_snapshot()
    }

    /// Diagnostics: world status snapshot (counts, timings, permissions).
    pub async fn world_status(&self) -> hotki_world::WorldStatus {
        self.world.status().await
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        if self.lifecycle.is_last_owner() {
            self.lifecycle.shutdown();
            self.repeater.abort_all();
            self.action_repeater.abort_all();
            self.relay.stop_all();
        }
    }
}

/// How [`Engine::install_config`] should treat existing focus and HUD state.
#[derive(Debug, Clone, Copy)]
pub(crate) enum ConfigInstall {
    /// Fresh install: clear HUD and refresh focus from the world cache.
    ResetFocus,
    /// Reload in place: keep current focus and HUD visibility.
    KeepFocus,
}
