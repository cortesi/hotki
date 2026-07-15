use config::runtime::{ConfigRuntime, ModeCtx, ModeId, ModeRef, ModeStack, RenderedState};
use hotki_protocol::FocusSnapshot;
use mac_keycode::Chord;

use crate::selector::SelectorState;

/// Stack-based runtime state for dynamic configuration.
#[derive(Debug)]
pub(crate) struct RuntimeState {
    pub(crate) hud_visible: bool,
    pub(crate) stack: ModeStack,
    pub(crate) focus: Option<FocusSnapshot>,
    pub(crate) rendered: RenderedState,
    pub(crate) selector: Option<SelectorState>,
}

/// Cloneable portion of runtime state used to roll back a failed refresh.
#[derive(Clone)]
pub(crate) struct RuntimeCheckpoint {
    hud_visible: bool,
    stack: ModeStack,
    focus: Option<FocusSnapshot>,
    rendered: RenderedState,
}

impl RuntimeState {
    pub(crate) fn empty() -> Self {
        Self {
            hud_visible: false,
            stack: ModeStack::default(),
            focus: None,
            rendered: Self::empty_rendered(config::Style::default()),
            selector: None,
        }
    }

    pub(crate) fn empty_rendered(style: config::Style) -> RenderedState {
        RenderedState {
            bindings: Vec::new(),
            hud_rows: Vec::new(),
            style,
            capture: false,
        }
    }

    pub(crate) fn checkpoint(&self) -> RuntimeCheckpoint {
        RuntimeCheckpoint {
            hud_visible: self.hud_visible,
            stack: self.stack.clone(),
            focus: self.focus.clone(),
            rendered: self.rendered.clone(),
        }
    }

    pub(crate) fn restore(&mut self, checkpoint: RuntimeCheckpoint) {
        self.hud_visible = checkpoint.hud_visible;
        self.stack = checkpoint.stack;
        self.focus = checkpoint.focus;
        self.rendered = checkpoint.rendered;
    }

    pub(crate) fn install_config(&mut self, config: &ConfigRuntime) {
        self.selector = None;
        config.reset_stack(&mut self.stack);
        self.rendered = Self::empty_rendered(config.style());
    }

    pub(crate) fn clear_config_state(&mut self, style: config::Style) {
        self.hud_visible = false;
        self.selector = None;
        self.stack.clear();
        self.rendered = Self::empty_rendered(style);
    }

    pub(crate) fn depth(&self) -> usize {
        self.stack.depth()
    }

    /// Push a child mode frame and make the HUD visible.
    pub(crate) fn push_mode(
        &mut self,
        title: String,
        closure: ModeRef,
        entered_via: Option<(Chord, ModeId)>,
        capture: bool,
    ) {
        self.hud_visible = true;
        self.stack.push(title, closure, entered_via, capture);
    }
}

pub(crate) fn mode_ctx(window: &Option<FocusSnapshot>, hud: bool, depth: usize) -> ModeCtx {
    ModeCtx {
        window: window.clone(),
        hud,
        depth: depth as i64,
    }
}
