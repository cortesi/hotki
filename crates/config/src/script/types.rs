use std::{
    collections::hash_map::DefaultHasher,
    fmt,
    hash::{Hash, Hasher},
    mem,
    path::PathBuf,
    sync::{Arc, Mutex},
};

pub use hotki_protocol::{HudRow, HudRowStyle};
use mac_keycode::Chord;
use mlua::Function;

use super::{SelectorConfig, util::lock_unpoisoned};
use crate::{Action, NotifyKind, Style, raw::RawStyle};

/// Source location attached to a binding for diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourcePos {
    /// Source file for the binding, when known.
    pub path: Option<PathBuf>,
    /// 1-based line number.
    pub line: Option<usize>,
    /// 1-based column number.
    pub col: Option<usize>,
}

/// Unique identifier for a mode closure.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ModeId(u64);

impl ModeId {
    /// Create a new `ModeId` from a stable identifier.
    pub(crate) const fn new(id: u64) -> Self {
        Self(id)
    }
}

impl fmt::Debug for ModeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("ModeId").field(&self.0).finish()
    }
}

/// Opaque wrapper around a Luau mode renderer.
#[derive(Clone)]
pub struct ModeRef {
    /// Stable identity used for orphan detection.
    pub(crate) id: ModeId,
    /// Luau function implementing the renderer.
    pub(crate) func: Function,
    /// Optional default title.
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
    /// Create a mode reference from a Luau function and optional title salt.
    pub(crate) fn from_function(func: Function, title: Option<String>) -> Self {
        let info = func.info();
        let mut hasher = DefaultHasher::new();
        info.source.hash(&mut hasher);
        info.line_defined.hash(&mut hasher);
        title.hash(&mut hasher);
        if info.source.is_none() && info.line_defined.is_none() {
            (func.to_pointer() as usize).hash(&mut hasher);
        }
        Self {
            id: ModeId::new(hasher.finish()),
            func,
            default_title: title,
        }
    }

    /// Return the stable identity of this mode closure.
    pub fn id(&self) -> ModeId {
        self.id
    }

    /// Return the default title declared for this mode, if any.
    pub fn default_title(&self) -> Option<&str> {
        self.default_title.as_deref()
    }
}

/// Opaque wrapper around a Luau handler closure.
#[derive(Clone)]
pub struct HandlerRef {
    /// Luau function implementing the handler.
    pub(crate) func: Function,
}

impl fmt::Debug for HandlerRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HandlerRef").finish_non_exhaustive()
    }
}

/// Render-time context passed into mode closures.
#[derive(Debug, Clone)]
pub struct ContextSnapshot {
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

/// Render-time context passed into mode closures.
pub type ModeCtx = ContextSnapshot;

/// Effect emitted by handlers.
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
    /// Shared render snapshot also exposed to action handlers.
    snapshot: ContextSnapshot,
    /// Shared mutable handler output state.
    shared: Arc<Mutex<ActionCtxShared>>,
}

#[derive(Debug, Default)]
/// Shared mutable state collected while a handler executes.
struct ActionCtxShared {
    /// Queued side effects.
    effects: Vec<Effect>,
    /// Pending navigation request.
    nav: Option<NavRequest>,
    /// Whether the handler requested stay-in-mode behavior.
    stay: bool,
}

impl ActionCtx {
    /// Create a new handler context for a given focused app/window state.
    pub(crate) fn new(snapshot: ContextSnapshot) -> Self {
        Self {
            snapshot,
            shared: Arc::new(Mutex::new(ActionCtxShared::default())),
        }
    }

    /// Return the focused application name.
    pub fn app(&self) -> &str {
        &self.snapshot.app
    }

    /// Return the focused window title.
    pub fn title(&self) -> &str {
        &self.snapshot.title
    }

    /// Return the focused process identifier.
    pub fn pid(&self) -> i64 {
        self.snapshot.pid
    }

    /// Return whether the HUD is visible.
    pub fn hud(&self) -> bool {
        self.snapshot.hud
    }

    /// Return the current stack depth.
    pub fn depth(&self) -> i64 {
        self.snapshot.depth
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

/// Mode-level style overlay.
#[derive(Clone, Debug)]
pub struct StyleOverlay {
    /// Static raw style overlay.
    pub(crate) raw: RawStyle,
}

/// Binding-level style overlay and visibility modifiers.
#[derive(Clone, Debug, Default)]
pub struct BindingStyle {
    /// Whether the binding should be hidden from the HUD.
    pub hidden: bool,
    /// Optional overlay to apply to HUD row colors.
    pub overlay: Option<RawStyle>,
}

/// The kind of binding produced by a mode closure.
#[derive(Debug, Clone)]
pub enum BindingKind {
    /// Primitive action binding.
    Action(Action),
    /// Handler binding.
    Handler(HandlerRef),
    /// Open an interactive selector popup.
    Selector(SelectorConfig),
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
    /// Binding behavior.
    pub kind: BindingKind,
    /// Mode identity when `kind` is [`BindingKind::Mode`].
    pub mode_id: Option<ModeId>,
    /// Execution and visibility flags.
    pub flags: BindingFlags,
    /// Optional per-binding style overlay.
    pub style: Option<BindingStyle>,
    /// True when entering the bound mode should enable capture-all.
    pub mode_capture: bool,
    /// Source position of the binding declaration for diagnostics.
    pub pos: Option<SourcePos>,
}

/// A stack frame representing an active mode.
#[derive(Debug, Clone)]
pub struct ModeFrame {
    /// Current title for this frame.
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

/// Render output for the current runtime state.
#[derive(Debug, Clone)]
pub struct RenderedState {
    /// Flattened binding list used for dispatch and HUD generation.
    pub bindings: Vec<(Chord, Binding)>,
    /// HUD rows visible in the current mode.
    pub hud_rows: Vec<HudRow>,
    /// Effective resolved style.
    pub style: Style,
    /// True when capture-all mode is active in the current frame.
    pub capture: bool,
}
