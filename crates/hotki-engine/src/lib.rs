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
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

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
const FOCUS_BIND_WARN_MS: u64 = 10;

use hotki_protocol::MsgToUI;
use keymode::{KeyResponse, State};
use mac_keycode::Chord;
use tracing::{debug, info, trace, warn};

pub use error::{Error, Result};
pub use focus::FocusHandler;
pub use notification::NotificationDispatcher;
pub use relay::RelayHandler;
pub use repeater::{RepeatObserver, RepeatSpec, Repeater};

use key_binding::KeyBindingManager;
use key_state::KeyStateTracker;
use repeater::ExecSpec;

use crate::focus::FocusState;

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
    /// Focus handler
    focus_handler: FocusHandler,
    /// Relay handler
    relay_handler: RelayHandler,
    /// Notification dispatcher
    notifier: NotificationDispatcher,
    /// Repeater for shell commands and relays
    repeater: Repeater,
    /// Configuration
    config: Arc<tokio::sync::RwLock<config::Config>>,
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
        let binding_manager_arc =
            Arc::new(tokio::sync::Mutex::new(KeyBindingManager::new(manager)));
        // Create shared focus/relay instances
        let focus_handler = FocusHandler::new();
        let relay_handler = RelayHandler::new();
        let notifier = NotificationDispatcher::new(event_tx.clone());
        let repeater = Repeater::new(
            focus_handler.clone(),
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
            focus_handler,
            relay_handler,
            notifier,
            repeater,
            config: config_arc,
        }
    }

    async fn rebind_current_context(&self) -> Result<()> {
        let fs = self.focus_handler.get_focus_state();
        info!(
            "Rebinding with current context: app={}, title={}",
            fs.app, fs.title
        );
        self.rebind_and_refresh(&fs.app, &fs.title).await
    }

    async fn rebind_and_refresh(&self, app: &str, title: &str) -> Result<()> {
        tracing::info!("update_context: start app={} title={}", app, title);
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
            debug!("HUD update: cursor {:?}", cursor.path());
            self.notifier
                .send_hud_update_cursor(cursor, app.to_string(), title.to_string())?;
        }
        tracing::debug!("update_context: hud updated");

        // Determine capture policy via Config + Location
        let cur = {
            let st = self.state.lock().await;
            st.current_cursor()
        };
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
        let detailed = cfg_guard.hud_keys(&cur, app, title);
        for (ch, _desc, _attrs, _is_mode) in detailed {
            let ident = ch.to_string();
            if dedup.insert(ident.clone()) {
                key_pairs.push((ident, ch));
            }
        }
        // Keep bind ordering stable for reduced churn and better diffs/logging
        key_pairs.sort_by(|a, b| a.0.cmp(&b.0));
        let key_count = key_pairs.len();
        tracing::debug!(
            "update_context: capture gating hud_visible={} capture={}",
            hud_visible,
            capture
        );
        let mut manager = self.binding_manager.lock().await;
        if manager.update_bindings(key_pairs)? {
            tracing::debug!("update_context: bindings updated, clearing repeater + relay");
            debug!("Bindings changed, clearing repeater and relay");
            // Async clear to avoid blocking the runtime thread
            self.repeater.clear_async().await;
            let pid = self.focus_handler.get_pid();
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
        tracing::info!("update_context: done for app={} title={}", app, title);
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

    /// Handle a focus event: update internal context and rebind if needed.
    pub async fn on_focus_event(&mut self, event: mac_focus_watcher::FocusEvent) -> Result<()> {
        let start = Instant::now();
        debug!("Focus event received: {:?}", event);

        self.focus_handler.handle_event(event);

        {
            let update_start = Instant::now();

            info!("Focus change: updating bindings synchronously");
            // Single entrypoint: ensure context, update HUD, rebind
            self.rebind_current_context().await?;

            let elapsed = update_start.elapsed();
            if elapsed > Duration::from_millis(FOCUS_BIND_WARN_MS) {
                warn!(
                    "Focus event binding update took {:?}, may cause key drops",
                    elapsed
                );
            } else {
                debug!("Focus event binding update completed in {:?}", elapsed);
            }
        }

        debug!("Focus event fully processed in {:?}", start.elapsed());
        Ok(())
    }

    /// Return the current focus context (app, title).
    pub fn get_context(&self) -> FocusState {
        self.focus_handler.get_focus_state()
    }

    /// Get the current depth (0 = root) if state is initialized.
    pub async fn get_depth(&self) -> usize {
        self.state.lock().await.depth()
    }

    /// Get a read-only snapshot of currently bound keys as (identifier, chord) pairs.
    pub async fn bindings_snapshot(&self) -> Vec<(String, mac_keycode::Chord)> {
        self.binding_manager.lock().await.bindings_snapshot()
    }

    /// Process a key event and return whether depth changed (requiring rebind)
    async fn handle_key_event(&self, chord: &Chord, identifier: String) -> Result<bool> {
        let start = Instant::now();
        let fs = self.focus_handler.get_focus_state();
        let pid = self.focus_handler.get_pid();

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

                    // Record whether OS repeat events should be acted upon
                    self.key_tracker
                        .set_repeat_allowed(&identifier, repeat.is_some());

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
                // Map Toggle -> mac_winops::Desired
                let d = match desired {
                    config::Toggle::On => mac_winops::Desired::On,
                    config::Toggle::Off => mac_winops::Desired::Off,
                    config::Toggle::Toggle => mac_winops::Desired::Toggle,
                };
                let pid = self.focus_handler.get_pid();
                let res = match kind {
                    config::FullscreenKind::Native => mac_winops::fullscreen_native(pid, d),
                    config::FullscreenKind::Nonnative => {
                        mac_winops::request_fullscreen_nonnative(pid, d)
                    }
                };
                if let Err(e) = res {
                    let _ = self.notifier.send_error("Fullscreen", format!("{}", e));
                }
                Ok(())
            }
            Ok(KeyResponse::Place {
                cols,
                rows,
                col,
                row,
            }) => {
                let pid = self.focus_handler.get_pid();
                if let Err(e) = mac_winops::request_place_grid(pid, cols, rows, col, row) {
                    let _ = self.notifier.send_error("Place", format!("{}", e));
                }
                Ok(())
            }
            Ok(resp) => {
                trace!("Key response: {:?}", resp);
                // Special-case ShellAsync to start shell repeater if configured
                match resp {
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
                self.notifier.send_error("Key", e)?;
                Ok(())
            }
        }?;

        let location_changed = loc_before != loc_after;
        if location_changed {
            info!(
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
        let pid = self.focus_handler.get_pid();
        self.repeater.stop_sync(identifier);
        if self.relay_handler.stop_relay(identifier, pid) {
            debug!("Stopped relay for {}", identifier);
        }
    }

    /// Handle a repeat key event for active relays
    fn handle_repeat(&self, identifier: &str) {
        let pid = self.focus_handler.get_pid();
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
