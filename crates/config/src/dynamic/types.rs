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
    /// Return the stable identity of this mode closure (used for orphan detection).
    pub fn id(&self) -> ModeId {
        self.id
    }

    /// Return the default title declared for this mode, if any.
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
    /// Focused application name.
    pub app: String,
    /// Focused window title.
    pub title: String,
    /// Focused process identifier.
    pub pid: i64,
    /// Whether the HUD is currently visible.
    pub hud: bool,
    /// Current stack depth (root = 0).
    pub depth: i64,
}

/// Effect emitted by handlers (and render warnings) for the engine to apply.
#[derive(Debug, Clone)]
pub enum Effect {
    /// Execute a primitive action.
    Exec(Action),
    /// Show a notification.
    Notify {
        /// Notification severity kind.
        kind: NotifyKind,
        /// Notification title text.
        title: String,
        /// Notification body text.
        body: String,
    },
}

/// Navigation request emitted by handlers or primitive actions.
#[derive(Debug, Clone)]
pub enum NavRequest {
    /// Push a mode onto the stack.
    Push {
        /// Mode closure to push.
        mode: ModeRef,
        /// Optional title override for the pushed frame.
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
    /// Optional initial delay before the first repeat, in milliseconds.
    pub delay_ms: Option<u64>,
    /// Optional interval between repeats, in milliseconds.
    pub interval_ms: Option<u64>,
}

/// Binding-level flags.
#[derive(Debug, Clone, Default)]
pub struct BindingFlags {
    /// True when the binding is hidden from the HUD.
    pub hidden: bool,
    /// True when the binding is inherited by child modes.
    pub global: bool,
    /// True when the binding suppresses auto-exit after execution.
    pub stay: bool,
    /// Optional software repeat configuration for this binding.
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
    /// Key chord that triggers the binding.
    pub chord: Chord,
    /// Human-readable description shown in the HUD.
    pub desc: String,
    /// Binding behavior (action, handler, or mode entry).
    pub kind: BindingKind,
    /// Mode identity when `kind` is [`BindingKind::Mode`].
    pub mode_id: Option<ModeId>,
    /// Execution and visibility flags.
    pub flags: BindingFlags,
    /// Optional per-binding style overlay.
    pub style: Option<StyleOverlay>,
    /// True when entering the bound mode should enable capture-all.
    pub mode_capture: bool,
    /// Source position of the binding declaration for diagnostics.
    pub(crate) pos: Position,
}

/// A stack frame representing an active mode.
#[derive(Debug, Clone)]
pub struct ModeFrame {
    /// Current title for this frame (used in breadcrumbs).
    pub title: String,
    /// Mode closure backing this frame.
    pub closure: ModeRef,
    /// Entry chord and mode identity when entered via a mode binding.
    pub entered_via: Option<(Chord, ModeId)>,
    /// Cached rendered bindings for this frame.
    pub rendered: Vec<Binding>,
    /// Optional mode-level style overlay for this frame.
    pub style: Option<StyleOverlay>,
    /// True when this frame requests capture-all while HUD is visible.
    pub capture: bool,
}

/// One HUD row entry produced by rendering.
#[derive(Debug, Clone)]
pub struct HudRow {
    /// Key chord that triggers the binding.
    pub chord: Chord,
    /// Human-readable description.
    pub desc: String,
    /// True when the binding enters a child mode.
    pub is_mode: bool,
    /// Optional per-row HUD style overrides.
    pub style: Option<HudRowStyle>,
}

/// Optional per-binding HUD style overrides after resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HudRowStyle {
    /// Foreground color for non-modifier key tokens.
    pub key_fg: (u8, u8, u8),
    /// Background color for non-modifier key tokens.
    pub key_bg: (u8, u8, u8),
    /// Foreground color for modifier key tokens.
    pub mod_fg: (u8, u8, u8),
    /// Background color for modifier key tokens.
    pub mod_bg: (u8, u8, u8),
    /// Foreground color for the submenu tag indicator.
    pub tag_fg: (u8, u8, u8),
}

/// Render output for the current runtime state.
#[derive(Debug, Clone)]
pub struct RenderedState {
    /// Flattened binding list used for dispatch and HUD generation.
    pub bindings: Vec<(Chord, Binding)>,
    /// HUD rows visible in the current mode.
    pub hud_rows: Vec<HudRow>,
    /// Effective resolved style (base theme + overlays).
    pub style: Style,
    /// True when capture-all mode is active in the current frame.
    pub capture: bool,
}
