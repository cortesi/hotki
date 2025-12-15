use std::{
    fmt, mem,
    sync::{Arc, Mutex},
};

use mac_keycode::Chord;
use rhai::{FnPtr, Position};

use super::util::lock_unpoisoned;
use crate::{Action, NotifyKind, Style, raw::RawStyle};

/// Unique identifier for a mode closure.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ModeId(u64);

impl ModeId {
    /// Create a new `ModeId` from a stable identifier.
    pub(crate) fn new(id: u64) -> Self {
        Self(id)
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
    /// Stable identifier for the mode closure (used for orphan detection).
    pub(crate) id: ModeId,
    /// Rhai function pointer for invoking the mode closure.
    pub(crate) func: FnPtr,
    /// Default title declared at mode creation time, if any.
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

impl ModeRef {
    pub fn id(&self) -> ModeId {
        self.id
    }

    pub fn default_title(&self) -> Option<&str> {
        self.default_title.as_deref()
    }
}

/// Opaque wrapper around a Rhai handler closure.
#[derive(Clone)]
pub struct HandlerRef {
    /// Rhai function pointer for invoking the handler closure.
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
    pub app: String,
    pub title: String,
    pub pid: i64,
    pub hud: bool,
    pub depth: i64,
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
    Push {
        mode: ModeRef,
        title: Option<String>,
    },
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
    /// Focused application name.
    pub(crate) app: String,
    /// Focused window title.
    pub(crate) title: String,
    /// Focused process id.
    pub(crate) pid: i64,
    /// Current HUD visibility.
    pub(crate) hud: bool,
    /// Current stack depth (root = 0).
    pub(crate) depth: i64,
    /// Shared mutable handler output state.
    shared: Arc<Mutex<ActionCtxShared>>,
}

#[derive(Debug, Default)]
/// Shared mutable state for an executing handler closure.
struct ActionCtxShared {
    /// Queued effects to apply after handler completion.
    effects: Vec<Effect>,
    /// Optional navigation request to apply after handler completion.
    nav: Option<NavRequest>,
    /// Whether the handler requested staying in the current mode.
    stay: bool,
}

impl ActionCtx {
    /// Create a new handler context for a given focused app/window state.
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

    /// Push a new effect into the handler result queue.
    pub(crate) fn push_effect(&self, effect: Effect) {
        lock_unpoisoned(&self.shared).effects.push(effect);
    }

    /// Set the navigation request, replacing any previously requested navigation.
    pub(crate) fn set_nav(&self, nav: NavRequest) {
        lock_unpoisoned(&self.shared).nav = Some(nav);
    }

    /// Request that the engine stays in the current mode after executing the handler.
    pub(crate) fn set_stay(&self) {
        lock_unpoisoned(&self.shared).stay = true;
    }

    /// Drain and return all queued effects.
    pub(crate) fn take_effects(&self) -> Vec<Effect> {
        mem::take(&mut lock_unpoisoned(&self.shared).effects)
    }

    /// Take and clear the navigation request, if any.
    pub(crate) fn take_nav(&self) -> Option<NavRequest> {
        lock_unpoisoned(&self.shared).nav.take()
    }

    /// Return whether the handler requested staying in the current mode.
    pub(crate) fn stay(&self) -> bool {
        lock_unpoisoned(&self.shared).stay
    }
}

/// Software repeat configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RepeatSpec {
    pub delay_ms: Option<u64>,
    pub interval_ms: Option<u64>,
}

/// Binding-level flags.
#[derive(Debug, Clone, Default)]
pub struct BindingFlags {
    pub hidden: bool,
    pub global: bool,
    pub stay: bool,
    pub repeat: Option<RepeatSpec>,
}

/// Opaque style overlay attached to modes and bindings.
#[derive(Clone)]
pub struct StyleOverlay {
    /// Dynamic style closure, invoked with `(ctx)` to produce a map.
    pub(crate) func: Option<FnPtr>,
    /// Static raw style overlay.
    pub(crate) raw: Option<RawStyle>,
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
    pub chord: Chord,
    pub desc: String,
    pub kind: BindingKind,
    pub mode_id: Option<ModeId>,
    pub flags: BindingFlags,
    pub style: Option<StyleOverlay>,
    pub mode_style: Option<StyleOverlay>,
    pub mode_capture: bool,
    /// Source position of the binding declaration for diagnostics.
    pub(crate) pos: Position,
}

/// A stack frame representing an active mode.
#[derive(Debug, Clone)]
pub struct ModeFrame {
    pub title: String,
    pub closure: ModeRef,
    pub entered_via: Option<(Chord, ModeId)>,
    pub rendered: Vec<Binding>,
    pub style: Option<StyleOverlay>,
    pub capture: bool,
}

/// One HUD row entry produced by rendering.
#[derive(Debug, Clone)]
pub struct HudRow {
    pub chord: Chord,
    pub desc: String,
    pub is_mode: bool,
    pub style: Option<HudRowStyle>,
}

/// Optional per-binding HUD style overrides after resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HudRowStyle {
    pub key_fg: (u8, u8, u8),
    pub key_bg: (u8, u8, u8),
    pub mod_fg: (u8, u8, u8),
    pub mod_bg: (u8, u8, u8),
    pub tag_fg: (u8, u8, u8),
}

/// Render output for the current runtime state.
#[derive(Debug, Clone)]
pub struct RenderedState {
    pub bindings: Vec<(Chord, Binding)>,
    pub hud_rows: Vec<HudRow>,
    pub style: Style,
    pub capture: bool,
}
