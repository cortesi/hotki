use std::fmt;
use std::mem;
use std::sync::{Arc, Mutex, MutexGuard};

use mac_keycode::Chord;
use rhai::{FnPtr, Position};

use crate::{Action, NotifyKind};

/// Unique identifier for a mode closure.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ModeId(u64);

impl ModeId {
    pub(crate) fn new(id: u64) -> Self {
        Self(id)
    }

    pub(crate) fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Debug for ModeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("ModeId").field(&self.0).finish()
    }
}

/// Opaque wrapper around a Rhai mode closure.
#[derive(Clone)]
pub struct ModeRef {
    pub(crate) id: ModeId,
    pub(crate) func: FnPtr,
    pub(crate) default_title: Option<String>,
}

impl fmt::Debug for ModeRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ModeRef")
            .field("id", &self.id)
            .field("default_title", &self.default_title)
            .finish_non_exhaustive()
    }
}

/// Opaque wrapper around a Rhai handler closure.
#[derive(Clone)]
pub struct HandlerRef {
    pub(crate) func: FnPtr,
}

impl fmt::Debug for HandlerRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HandlerRef").finish_non_exhaustive()
    }
}

/// Render-time context passed into mode closures.
#[derive(Debug, Clone)]
pub struct ModeCtx {
    pub(crate) app: String,
    pub(crate) title: String,
    pub(crate) pid: i64,
    pub(crate) hud: bool,
    pub(crate) depth: i64,
}

/// Effect emitted by handlers (and render warnings) for the engine to apply.
#[derive(Debug, Clone)]
pub enum Effect {
    /// Execute a primitive action.
    Exec(Action),
    /// Show a notification.
    Notify {
        kind: NotifyKind,
        title: String,
        body: String,
    },
}

/// Navigation request emitted by handlers or primitive actions.
#[derive(Debug, Clone)]
pub enum NavRequest {
    /// Push a mode onto the stack.
    Push { mode: ModeRef, title: Option<String> },
    /// Pop the current mode.
    Pop,
    /// Clear to root and hide HUD.
    Exit,
    /// Clear to root and show HUD.
    ShowRoot,
    /// Hide HUD without changing stack depth.
    HideHud,
}

/// Handler execution context passed into handler closures.
#[derive(Debug, Clone)]
pub struct ActionCtx {
    pub(crate) app: String,
    pub(crate) title: String,
    pub(crate) pid: i64,
    pub(crate) hud: bool,
    pub(crate) depth: i64,
    shared: Arc<Mutex<ActionCtxShared>>,
}

#[derive(Debug, Default)]
struct ActionCtxShared {
    effects: Vec<Effect>,
    nav: Option<NavRequest>,
    stay: bool,
}

fn lock_unpoisoned<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl ActionCtx {
    pub(crate) fn new(app: String, title: String, pid: i64, hud: bool, depth: i64) -> Self {
        Self {
            app,
            title,
            pid,
            hud,
            depth,
            shared: Arc::new(Mutex::new(ActionCtxShared::default())),
        }
    }

    pub(crate) fn push_effect(&self, effect: Effect) {
        lock_unpoisoned(&self.shared).effects.push(effect);
    }

    pub(crate) fn set_nav(&self, nav: NavRequest) {
        lock_unpoisoned(&self.shared).nav = Some(nav);
    }

    pub(crate) fn set_stay(&self) {
        lock_unpoisoned(&self.shared).stay = true;
    }

    pub(crate) fn take_effects(&self) -> Vec<Effect> {
        mem::take(&mut lock_unpoisoned(&self.shared).effects)
    }

    pub(crate) fn take_nav(&self) -> Option<NavRequest> {
        lock_unpoisoned(&self.shared).nav.take()
    }

    pub(crate) fn stay(&self) -> bool {
        lock_unpoisoned(&self.shared).stay
    }
}

/// Software repeat configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RepeatSpec {
    pub(crate) delay_ms: Option<u64>,
    pub(crate) interval_ms: Option<u64>,
}

/// Binding-level flags.
#[derive(Debug, Clone, Default)]
pub struct BindingFlags {
    pub(crate) hidden: bool,
    pub(crate) global: bool,
    pub(crate) stay: bool,
    pub(crate) repeat: Option<RepeatSpec>,
}

/// Opaque style overlay attached to modes and bindings.
#[derive(Clone)]
pub struct StyleOverlay {
    pub(crate) func: Option<FnPtr>,
    pub(crate) raw: Option<crate::raw::RawStyle>,
}

impl fmt::Debug for StyleOverlay {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StyleOverlay")
            .field("dynamic", &self.func.is_some())
            .field("static", &self.raw.is_some())
            .finish()
    }
}

/// The kind of binding produced by a mode closure.
#[derive(Debug, Clone)]
pub enum BindingKind {
    /// Primitive action binding.
    Action(Action),
    /// Handler binding.
    Handler(HandlerRef),
    /// Mode entry binding.
    Mode(ModeRef),
}

/// A rendered binding entry.
#[derive(Debug, Clone)]
pub struct Binding {
    pub(crate) chord: Chord,
    pub(crate) desc: String,
    pub(crate) kind: BindingKind,
    pub(crate) mode_id: Option<ModeId>,
    pub(crate) flags: BindingFlags,
    pub(crate) style: Option<StyleOverlay>,
    pub(crate) mode_style: Option<StyleOverlay>,
    pub(crate) mode_capture: bool,
    pub(crate) pos: Position,
}

/// A stack frame representing an active mode.
#[derive(Debug, Clone)]
pub struct ModeFrame {
    pub(crate) title: String,
    pub(crate) closure: ModeRef,
    pub(crate) entered_via: Option<(Chord, ModeId)>,
    pub(crate) rendered: Vec<Binding>,
    pub(crate) style: Option<StyleOverlay>,
    pub(crate) capture: bool,
}

/// One HUD row entry produced by rendering.
#[derive(Debug, Clone)]
pub struct HudRow {
    pub(crate) chord: Chord,
    pub(crate) desc: String,
    pub(crate) is_mode: bool,
    pub(crate) style: Option<HudRowStyle>,
}

/// Optional per-binding HUD style overrides after resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HudRowStyle {
    pub(crate) key_fg: (u8, u8, u8),
    pub(crate) key_bg: (u8, u8, u8),
    pub(crate) mod_fg: (u8, u8, u8),
    pub(crate) mod_bg: (u8, u8, u8),
    pub(crate) tag_fg: (u8, u8, u8),
}

/// Render output for the current runtime state.
#[derive(Debug, Clone)]
pub struct RenderedState {
    pub(crate) bindings: Vec<(Chord, Binding)>,
    pub(crate) hud_rows: Vec<HudRow>,
    pub(crate) style: crate::Style,
    pub(crate) capture: bool,
}
