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
    path::PathBuf,
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
mod runtime;
mod ticker;

// Timing constants for warning thresholds
const BIND_UPDATE_WARN_MS: u64 = 10;
const KEY_PROC_WARN_MS: u64 = 5;

#[derive(Debug, Clone, Copy, Default)]
struct DispatchOutcome {
    is_nav: bool,
    entered_mode: bool,
}

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

use crate::runtime::{FocusInfo, RuntimeState};

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
    /// Stack-based runtime state (mode stack + focus + theme/user-style).
    runtime: Arc<tokio::sync::Mutex<RuntimeState>>,
    /// Key binding manager
    binding_manager: Arc<tokio::sync::Mutex<KeyBindingManager>>,
    /// Key state tracker (tracks which keys are held down)
    key_tracker: KeyStateTracker,
    /// Configuration
    config: Arc<tokio::sync::RwLock<Option<config::dynamic::DynamicConfig>>>,
    /// Optional path used for `action.reload_config`.
    config_path: Arc<tokio::sync::RwLock<Option<PathBuf>>>,
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
        let (app, title, pid) = self.current_context();
        debug!("Rebinding with context: app={}, title={}", app, title);
        self.rebind_and_refresh(&app, &title, pid).await
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

        let cursor = self.cursor_for_ui().await;
        let cursor = self.cursor_with_current_app(cursor);
        self.publish_hud_with_displays(cursor, snapshot).await
    }

    async fn rebind_and_refresh(&self, app: &str, title: &str, pid: i32) -> Result<()> {
        tracing::debug!("start app={} title={}", app, title);

        let mut warnings = Vec::new();
        let mut key_pairs: Vec<(String, Chord)> = Vec::new();
        let mut capture_all = false;
        let cursor = {
            let cfg_guard = self.config.read().await;
            let mut rt = self.runtime.lock().await;
            rt.focus = FocusInfo {
                app: app.to_string(),
                title: title.to_string(),
                pid,
            };

            if let Some(cfg) = cfg_guard.as_ref() {
                if rt.stack.is_empty() {
                    rt.stack.push(config::dynamic::ModeFrame {
                        title: "root".to_string(),
                        closure: cfg.root(),
                        entered_via: None,
                        rendered: Vec::new(),
                        style: None,
                        capture: false,
                    });
                }

                let theme = theme_name_for_index(rt.theme_index);
                let base_style = cfg.base_style(Some(theme), rt.user_style_enabled);

                let mut ctx = config::dynamic::ModeCtx {
                    app: rt.focus.app.clone(),
                    title: rt.focus.title.clone(),
                    pid: rt.focus.pid as i64,
                    hud: rt.hud_visible,
                    depth: rt.depth() as i64,
                };

                let output =
                    match config::dynamic::render_stack(cfg, &mut rt.stack, &ctx, &base_style) {
                        Ok(o) => Some(o),
                        Err(err) => {
                            self.notifier.send_error("Config", err.pretty())?;
                            rt.stack.truncate(1);
                            ctx.depth = 0;
                            match config::dynamic::render_stack(
                                cfg,
                                &mut rt.stack,
                                &ctx,
                                &base_style,
                            ) {
                                Ok(o) => Some(o),
                                Err(err) => {
                                    self.notifier.send_error("Config", err.pretty())?;
                                    rt.rendered = config::dynamic::RenderedState {
                                        bindings: Vec::new(),
                                        hud_rows: Vec::new(),
                                        style: base_style,
                                        capture: false,
                                    };
                                    None
                                }
                            }
                        }
                    };

                if let Some(output) = output {
                    warnings = output.warnings;
                    rt.rendered = output.rendered;

                    for (ch, _binding) in rt.rendered.bindings.iter() {
                        key_pairs.push((ch.to_string(), ch.clone()));
                    }
                    key_pairs.sort_by(|a, b| a.0.cmp(&b.0));

                    capture_all = rt.hud_visible && rt.rendered.capture;
                }
            } else {
                rt.hud_visible = false;
                rt.stack.clear();
                rt.rendered = config::dynamic::RenderedState {
                    bindings: Vec::new(),
                    hud_rows: Vec::new(),
                    style: config::Style::default(),
                    capture: false,
                };
            }

            cursor_for_ui_from_state(&rt)
        };

        for effect in warnings {
            if let config::dynamic::Effect::Notify { kind, title, body } = effect {
                self.notifier.send_notification(kind, title, body)?;
            }
        }

        let start = Instant::now();
        let key_count = key_pairs.len();
        let bindings_changed = {
            let mut manager = self.binding_manager.lock().await;
            manager.set_capture_all(capture_all);
            manager.update_bindings(key_pairs)?
        };
        if bindings_changed {
            tracing::debug!("bindings updated, clearing repeater + relay");
            self.repeater.clear_async().await;
            self.relay.stop_all();
        }

        let displays_snapshot = self.world.displays().await;
        let cursor = self.cursor_with_current_app(cursor);
        self.publish_hud_with_displays(cursor, displays_snapshot)
            .await?;

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

    /// Load and install a dynamic configuration from `path`.
    pub async fn set_config_path(&self, path: PathBuf) -> Result<()> {
        let dyn_cfg = config::load_dynamic_config(&path).map_err(|e| Error::Msg(e.pretty()))?;
        let root = dyn_cfg.root();
        let theme_index = theme_index_for_name(dyn_cfg.base_theme().unwrap_or("default"));

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
            let (app, title, pid) = self.current_context();
            let mut rt = self.runtime.lock().await;
            rt.hud_visible = false;
            rt.theme_index = theme_index;
            rt.user_style_enabled = true;
            rt.focus = FocusInfo { app, title, pid };
            rt.stack = vec![config::dynamic::ModeFrame {
                title: "root".to_string(),
                closure: root,
                entered_via: None,
                rendered: Vec::new(),
                style: None,
                capture: false,
            }];
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

    /// Process a key-down event for a bound chord.
    async fn handle_key_event(&self, chord: &Chord, identifier: String) -> Result<()> {
        let start = Instant::now();
        // On dispatch, nudge world to refresh and proceed with cached context
        if self.sync_on_dispatch {
            self.world.hint_refresh();
        }
        let (app_ctx, title_ctx, pid) = self.current_context();

        trace!(
            "Key event received: {} (app: {}, title: {})",
            identifier, app_ctx, title_ctx
        );

        let cfg_guard = self.config.read().await;
        let Some(cfg) = cfg_guard.as_ref() else {
            trace!("No dynamic config loaded; ignoring key");
            return Ok(());
        };

        let (binding, ctx) = {
            let rt = self.runtime.lock().await;
            let Some(binding) = config::dynamic::resolve_binding(&rt.rendered, chord).cloned()
            else {
                trace!("No binding for chord {}", chord);
                return Ok(());
            };
            let ctx = config::dynamic::ModeCtx {
                app: app_ctx.clone(),
                title: title_ctx.clone(),
                pid: pid as i64,
                hud: rt.hud_visible,
                depth: rt.depth() as i64,
            };
            (binding, ctx)
        };

        let mut stay = binding.flags.stay;
        let mut nav_occurred = false;
        let mut entered_mode = false;

        match binding.kind.clone() {
            config::dynamic::BindingKind::Mode(mode) => {
                entered_mode = true;
                let mut rt = self.runtime.lock().await;
                rt.hud_visible = true;
                rt.stack.push(config::dynamic::ModeFrame {
                    title: binding.desc.clone(),
                    closure: mode,
                    entered_via: binding.mode_id.map(|id| (binding.chord.clone(), id)),
                    rendered: Vec::new(),
                    style: binding.mode_style.clone(),
                    capture: binding.mode_capture,
                });
            }
            config::dynamic::BindingKind::Action(action) => {
                let outcome = self
                    .apply_action(&identifier, &action, binding.flags.repeat)
                    .await?;
                nav_occurred = outcome.is_nav;
                entered_mode = outcome.entered_mode;
            }
            config::dynamic::BindingKind::Handler(handler) => {
                let result = match config::dynamic::execute_handler(cfg, &handler, &ctx) {
                    Ok(r) => r,
                    Err(err) => {
                        self.notifier.send_error("Handler", err.pretty())?;
                        return Ok(());
                    }
                };

                stay = stay || result.stay;

                for effect in result.effects {
                    match effect {
                        config::dynamic::Effect::Exec(action) => {
                            let outcome = self.apply_action(&identifier, &action, None).await?;
                            nav_occurred |= outcome.is_nav;
                            entered_mode |= outcome.entered_mode;
                        }
                        config::dynamic::Effect::Notify { kind, title, body } => {
                            self.notifier.send_notification(kind, title, body)?;
                        }
                    }
                }

                if let Some(nav) = result.nav {
                    let outcome = self.apply_nav_request(nav).await;
                    nav_occurred |= outcome.is_nav;
                    entered_mode |= outcome.entered_mode;
                }
            }
        }

        if !stay && !nav_occurred && !entered_mode {
            self.auto_exit().await;
        }

        let processing_time = start.elapsed();
        if processing_time > Duration::from_millis(KEY_PROC_WARN_MS) {
            warn!(
                "Key processing took {:?} for {}",
                processing_time, identifier
            );
        }

        self.rebind_and_refresh(&app_ctx, &title_ctx, pid).await?;
        trace!(
            "Key event completed in {:?}: {}",
            start.elapsed(),
            identifier
        );
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

    async fn apply_action(
        &self,
        identifier: &str,
        action: &config::Action,
        repeat: Option<config::dynamic::RepeatSpec>,
    ) -> Result<DispatchOutcome> {
        // Default to ignoring OS repeats for non-relay actions.
        self.key_tracker.set_repeat_allowed(identifier, false);

        match action {
            config::Action::Shell(spec) => {
                let repeat = repeat.map(|r| RepeatSpec {
                    initial_delay_ms: r.delay_ms,
                    interval_ms: r.interval_ms,
                });

                self.repeater.start(
                    identifier.to_string(),
                    ExecSpec::Shell {
                        command: spec.command().to_string(),
                        ok_notify: spec.ok_notify(),
                        err_notify: spec.err_notify(),
                    },
                    repeat,
                );
                Ok(DispatchOutcome::default())
            }
            config::Action::Relay(spec) => {
                let Some(target) = Chord::parse(spec) else {
                    self.notifier
                        .send_error("Relay", format!("Invalid relay chord string: {}", spec))?;
                    return Ok(DispatchOutcome::default());
                };

                let repeat = repeat.map(|r| RepeatSpec {
                    initial_delay_ms: r.delay_ms,
                    interval_ms: r.interval_ms,
                });

                if let Some(repeat) = repeat {
                    let allow_os_repeat =
                        repeat.initial_delay_ms.is_none() && repeat.interval_ms.is_none();
                    self.key_tracker
                        .set_repeat_allowed(identifier, allow_os_repeat);
                    self.repeater.start(
                        identifier.to_string(),
                        ExecSpec::Relay { chord: target },
                        Some(repeat),
                    );
                    return Ok(DispatchOutcome::default());
                }

                let pid = self.current_context().2;
                self.relay
                    .start_relay(identifier.to_string(), target.clone(), pid, false);
                let _ = self.relay.stop_relay(identifier, pid);
                Ok(DispatchOutcome::default())
            }
            config::Action::Pop => Ok(self
                .apply_nav_request(config::dynamic::NavRequest::Pop)
                .await),
            config::Action::Exit => Ok(self
                .apply_nav_request(config::dynamic::NavRequest::Exit)
                .await),
            config::Action::ShowHudRoot | config::Action::ShowRoot => Ok(self
                .apply_nav_request(config::dynamic::NavRequest::ShowRoot)
                .await),
            config::Action::HideHud => Ok(self
                .apply_nav_request(config::dynamic::NavRequest::HideHud)
                .await),
            config::Action::ReloadConfig => {
                if let Err(err) = self.reload_dynamic_config().await {
                    self.notifier.send_error("Config", err.to_string())?;
                }
                Ok(DispatchOutcome::default())
            }
            config::Action::ClearNotifications => {
                self.notifier.send_ui(MsgToUI::ClearNotifications)?;
                Ok(DispatchOutcome::default())
            }
            config::Action::ShowDetails(arg) => {
                self.notifier.send_ui(MsgToUI::ShowDetails(*arg))?;
                Ok(DispatchOutcome::default())
            }
            config::Action::ThemeNext => {
                let mut rt = self.runtime.lock().await;
                rt.theme_index = theme_next_index(rt.theme_index);
                Ok(DispatchOutcome::default())
            }
            config::Action::ThemePrev => {
                let mut rt = self.runtime.lock().await;
                rt.theme_index = theme_prev_index(rt.theme_index);
                Ok(DispatchOutcome::default())
            }
            config::Action::ThemeSet(name) => {
                if config::themes::theme_exists(name.as_str()) {
                    let mut rt = self.runtime.lock().await;
                    rt.theme_index = theme_index_for_name(name.as_str());
                } else {
                    self.notifier.send_notification(
                        config::NotifyKind::Warn,
                        "Theme".to_string(),
                        format!("Unknown theme: {}", name),
                    )?;
                }
                Ok(DispatchOutcome::default())
            }
            config::Action::SetVolume(level) => {
                let repeat = repeat.map(|r| RepeatSpec {
                    initial_delay_ms: r.delay_ms,
                    interval_ms: r.interval_ms,
                });
                let script = format!("set volume output volume {}", (*level).min(100));
                self.repeater.start(
                    identifier.to_string(),
                    ExecSpec::Shell {
                        command: format!("osascript -e '{}'", script),
                        ok_notify: config::NotifyKind::Ignore,
                        err_notify: config::NotifyKind::Warn,
                    },
                    repeat,
                );
                Ok(DispatchOutcome::default())
            }
            config::Action::ChangeVolume(delta) => {
                let repeat = repeat.map(|r| RepeatSpec {
                    initial_delay_ms: r.delay_ms,
                    interval_ms: r.interval_ms,
                });
                let script = format!(
                    "set currentVolume to output volume of (get volume settings)\nset volume output volume (currentVolume + {})",
                    delta
                );
                self.repeater.start(
                    identifier.to_string(),
                    ExecSpec::Shell {
                        command: format!("osascript -e '{}'", script.replace('\n', "' -e '")),
                        ok_notify: config::NotifyKind::Ignore,
                        err_notify: config::NotifyKind::Warn,
                    },
                    repeat,
                );
                Ok(DispatchOutcome::default())
            }
            config::Action::Mute(arg) => {
                let script = match arg {
                    config::Toggle::On => "set volume output muted true".to_string(),
                    config::Toggle::Off => "set volume output muted false".to_string(),
                    config::Toggle::Toggle => {
                        "set curMuted to output muted of (get volume settings)\nset volume output muted not curMuted".to_string()
                    }
                };
                self.repeater.start(
                    identifier.to_string(),
                    ExecSpec::Shell {
                        command: format!("osascript -e '{}'", script.replace('\n', "' -e '")),
                        ok_notify: config::NotifyKind::Ignore,
                        err_notify: config::NotifyKind::Warn,
                    },
                    None,
                );
                Ok(DispatchOutcome::default())
            }
            config::Action::UserStyle(arg) => {
                let mut rt = self.runtime.lock().await;
                rt.user_style_enabled = apply_toggle(rt.user_style_enabled, *arg);
                Ok(DispatchOutcome::default())
            }
            config::Action::Rhai { .. } | config::Action::Keys(_) => {
                self.notifier.send_notification(
                    config::NotifyKind::Warn,
                    "Config".to_string(),
                    "Legacy actions are not supported in dynamic config".to_string(),
                )?;
                Ok(DispatchOutcome::default())
            }
        }
    }

    async fn apply_nav_request(&self, nav: config::dynamic::NavRequest) -> DispatchOutcome {
        let mut rt = self.runtime.lock().await;
        match nav {
            config::dynamic::NavRequest::Push { mode, title } => {
                rt.hud_visible = true;
                let title = title
                    .or_else(|| mode.default_title().map(|t| t.to_string()))
                    .unwrap_or_else(|| "mode".to_string());
                rt.stack.push(config::dynamic::ModeFrame {
                    title,
                    closure: mode,
                    entered_via: None,
                    rendered: Vec::new(),
                    style: None,
                    capture: false,
                });
                DispatchOutcome {
                    is_nav: true,
                    entered_mode: true,
                }
            }
            config::dynamic::NavRequest::Pop => {
                if rt.stack.len() > 1 {
                    rt.stack.pop();
                }
                if rt.stack.len() <= 1 {
                    rt.hud_visible = false;
                }
                DispatchOutcome {
                    is_nav: true,
                    entered_mode: false,
                }
            }
            config::dynamic::NavRequest::Exit => {
                if rt.stack.len() > 1 {
                    rt.stack.truncate(1);
                }
                rt.hud_visible = false;
                DispatchOutcome {
                    is_nav: true,
                    entered_mode: false,
                }
            }
            config::dynamic::NavRequest::ShowRoot => {
                if rt.stack.len() > 1 {
                    rt.stack.truncate(1);
                }
                rt.hud_visible = true;
                DispatchOutcome {
                    is_nav: true,
                    entered_mode: false,
                }
            }
            config::dynamic::NavRequest::HideHud => {
                rt.hud_visible = false;
                DispatchOutcome {
                    is_nav: true,
                    entered_mode: false,
                }
            }
        }
    }

    async fn auto_exit(&self) {
        let mut rt = self.runtime.lock().await;
        if rt.stack.len() > 1 {
            rt.stack.truncate(1);
        }
        rt.hud_visible = false;
    }

    async fn reload_dynamic_config(&self) -> Result<()> {
        let path = { self.config_path.read().await.clone() };
        let Some(path) = path else {
            return Err(Error::Msg(
                "No config path set; cannot reload config".to_string(),
            ));
        };

        let dyn_cfg = config::load_dynamic_config(&path).map_err(|e| Error::Msg(e.pretty()))?;
        let root = dyn_cfg.root();

        {
            let mut g = self.config.write().await;
            *g = Some(dyn_cfg);
        }
        {
            let mut rt = self.runtime.lock().await;
            rt.stack = vec![config::dynamic::ModeFrame {
                title: "root".to_string(),
                closure: root,
                entered_via: None,
                rendered: Vec::new(),
                style: None,
                capture: false,
            }];
        }

        Ok(())
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

                if let Err(e) = self.handle_key_event(&chord, ident.clone()).await {
                    warn!("Key handler failed: {}", e);
                    return Err(e);
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

    async fn cursor_for_ui(&self) -> hotki_protocol::Cursor {
        let rt = self.runtime.lock().await;
        cursor_for_ui_from_state(&rt)
    }

    fn cursor_with_current_app(&self, cursor: hotki_protocol::Cursor) -> hotki_protocol::Cursor {
        let (app, title, pid) = self.current_context();
        cursor.with_app(hotki_protocol::App { app, title, pid })
    }
}

fn cursor_for_ui_from_state(rt: &RuntimeState) -> hotki_protocol::Cursor {
    let mut cursor = hotki_protocol::Cursor::new(Vec::new(), rt.hud_visible);
    cursor.set_theme(Some(theme_name_for_index(rt.theme_index)));
    cursor.set_user_style_enabled(rt.user_style_enabled);
    cursor
}

fn theme_name_for_index(index: usize) -> &'static str {
    let themes = config::themes::list_themes();
    if themes.is_empty() {
        return "default";
    }
    themes[index % themes.len()]
}

fn theme_index_for_name(name: &str) -> usize {
    let themes = config::themes::list_themes();
    if let Some(idx) = themes.iter().position(|t| *t == name) {
        return idx;
    }
    themes.iter().position(|t| *t == "default").unwrap_or(0)
}

fn theme_next_index(index: usize) -> usize {
    let themes = config::themes::list_themes();
    if themes.is_empty() {
        return 0;
    }
    (index % themes.len() + 1) % themes.len()
}

fn theme_prev_index(index: usize) -> usize {
    let themes = config::themes::list_themes();
    if themes.is_empty() {
        return 0;
    }
    let idx = index % themes.len();
    if idx == 0 { themes.len() - 1 } else { idx - 1 }
}

fn apply_toggle(current: bool, toggle: config::Toggle) -> bool {
    match toggle {
        config::Toggle::On => true,
        config::Toggle::Off => false,
        config::Toggle::Toggle => !current,
    }
}
