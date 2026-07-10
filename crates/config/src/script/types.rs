use std::{
    collections::hash_map::DefaultHasher,
    fmt,
    hash::{Hash, Hasher},
    mem,
    path::PathBuf,
    sync::{Arc, Mutex},
};

pub use hotki_protocol::HudRow;
use mac_keycode::Chord;
use ruau::vm::{Function, RuntimeError, Scope, SourceLocation};

use super::{SelectorConfig, callback::CallbackRef, util::lock_unpoisoned};
use crate::{Action, NotifyKind, Style};

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
    /// Retained Luau function implementing the renderer.
    pub(crate) func: CallbackRef,
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
    pub(crate) fn from_function<'s>(
        scope: &Scope<'s>,
        func: Function<'s>,
        title: Option<String>,
    ) -> Result<Self, RuntimeError> {
        let info = func.info(scope)?;
        let mut hasher = DefaultHasher::new();
        info.chunk_name.hash(&mut hasher);
        info.line_defined.hash(&mut hasher);
        title.hash(&mut hasher);
        if info.chunk_name.is_none() && info.line_defined.is_none() {
            func.id().hash(&mut hasher);
        }
        let func = CallbackRef::from_function(scope, func)?;
        Ok(Self {
            id: ModeId::new(hasher.finish()),
            func,
            default_title: title,
        })
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
    /// Retained Luau function implementing the handler.
    pub(crate) func: CallbackRef,
}

impl fmt::Debug for HandlerRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HandlerRef").finish_non_exhaustive()
    }
}

impl HandlerRef {
    /// Create a handler reference from a Luau function.
    pub(crate) fn from_function<'s>(
        scope: &Scope<'s>,
        func: Function<'s>,
    ) -> Result<Self, RuntimeError> {
        Ok(Self {
            func: CallbackRef::from_function(scope, func)?,
        })
    }
}

impl SourcePos {
    /// Build a source position from ruau's caller-location metadata.
    pub(crate) fn from_location(location: SourceLocation) -> Self {
        let path = (location.chunk_name != "<memory>").then(|| PathBuf::from(location.chunk_name));
        Self {
            path,
            line: Some(location.line as usize),
            col: Some(1),
        }
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
    /// Apply a navigation request.
    Nav(NavRequest),
    /// Open a selector popup.
    Select(SelectorConfig),
    /// Run a stashed action repeatedly until the triggering key is released.
    UntilKeyUp {
        /// Action closure to run on each repeat tick.
        action: HandlerRef,
        /// Repeat timing overrides.
        repeat: Option<RepeatSpec>,
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
    /// Whether this context may create an until-keyup loop.
    repeat: ActionRepeatPermission,
}

#[derive(Debug, Default)]
/// Shared mutable state collected while a handler executes.
struct ActionCtxShared {
    /// Queued side effects.
    effects: Vec<Effect>,
    /// Whether the handler requested stay-in-mode behavior.
    stay: bool,
    /// Whether the context is still valid for host effects.
    active: bool,
    /// Whether `ctx:until_keyup` was already requested during this activation.
    until_keyup: bool,
}

/// Whether a handler context may create a held-key repeat loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionRepeatPermission {
    /// A top-level binding activation with a held key.
    HeldKey,
    /// A selector callback or other action with no held triggering key.
    Keyless,
    /// A repeated action invocation, where nested repeat loops are rejected.
    RepeatedAction,
}

impl ActionCtx {
    /// Create a new handler context for a given focused app/window state.
    pub(crate) fn new(snapshot: ContextSnapshot, repeat: ActionRepeatPermission) -> Self {
        Self {
            snapshot,
            shared: Arc::new(Mutex::new(ActionCtxShared {
                active: true,
                ..ActionCtxShared::default()
            })),
            repeat,
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
    pub(crate) fn push_effect(&self, effect: Effect) -> Result<(), RuntimeError> {
        let mut shared = lock_unpoisoned(&self.shared);
        ensure_active(&shared)?;
        shared.effects.push(effect);
        Ok(())
    }

    /// Push an ordered navigation effect into the handler result queue.
    pub(crate) fn push_nav(&self, nav: NavRequest) -> Result<(), RuntimeError> {
        self.push_effect(Effect::Nav(nav))
    }

    /// Push an until-keyup effect into the handler result queue.
    pub(crate) fn push_until_keyup(
        &self,
        action: HandlerRef,
        repeat: Option<RepeatSpec>,
    ) -> Result<(), RuntimeError> {
        let mut shared = lock_unpoisoned(&self.shared);
        ensure_active(&shared)?;
        match self.repeat {
            ActionRepeatPermission::HeldKey => {}
            ActionRepeatPermission::Keyless => {
                return Err(RuntimeError::runtime(
                    "ctx:until_keyup requires a held triggering key",
                ));
            }
            ActionRepeatPermission::RepeatedAction => {
                return Err(RuntimeError::runtime(
                    "ctx:until_keyup cannot be nested inside a repeated action",
                ));
            }
        }
        if shared.until_keyup {
            return Err(RuntimeError::runtime(
                "ctx:until_keyup can only be called once per action",
            ));
        }
        shared.until_keyup = true;
        shared.effects.push(Effect::UntilKeyUp { action, repeat });
        Ok(())
    }

    /// Request that the engine stays in the current mode after executing the handler.
    pub(crate) fn set_stay(&self) -> Result<(), RuntimeError> {
        let mut shared = lock_unpoisoned(&self.shared);
        ensure_active(&shared)?;
        shared.stay = true;
        Ok(())
    }

    /// Finish this context, invalidate it, and drain the queued effects.
    pub(crate) fn finish(&self) -> (Vec<Effect>, bool) {
        let mut shared = lock_unpoisoned(&self.shared);
        shared.active = false;
        (mem::take(&mut shared.effects), shared.stay)
    }

    /// Invalidate this context without draining its effects.
    pub(crate) fn invalidate(&self) {
        let mut shared = lock_unpoisoned(&self.shared);
        shared.active = false;
        shared.effects.clear();
    }
}

/// Validate that a context is still usable.
fn ensure_active(shared: &ActionCtxShared) -> Result<(), RuntimeError> {
    if shared.active {
        Ok(())
    } else {
        Err(RuntimeError::runtime("ActionContext is no longer valid"))
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
}

/// The kind of binding produced by a mode closure.
#[derive(Debug, Clone)]
pub enum BindingKind {
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
    /// Binding behavior.
    pub kind: BindingKind,
    /// Mode identity when `kind` is [`BindingKind::Mode`].
    pub mode_id: Option<ModeId>,
    /// Execution and visibility flags.
    pub flags: BindingFlags,
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
