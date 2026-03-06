use config::dynamic::engine::{ModeCtx, ModeFrame, ModeRef, RenderedState};

use crate::selector::SelectorState;

/// Focus snapshot carried in runtime state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct FocusInfo {
    pub(crate) app: String,
    pub(crate) title: String,
    pub(crate) pid: i32,
}

/// Stack-based runtime state for dynamic configuration.
#[derive(Debug)]
pub(crate) struct RuntimeState {
    pub(crate) hud_visible: bool,
    pub(crate) stack: Vec<ModeFrame>,
    pub(crate) focus: FocusInfo,
    pub(crate) rendered: RenderedState,
    pub(crate) theme_name: String,
    pub(crate) selector: Option<SelectorState>,
}

impl RuntimeState {
    pub(crate) fn empty() -> Self {
        Self {
            hud_visible: false,
            stack: Vec::new(),
            focus: FocusInfo::default(),
            rendered: Self::empty_rendered(config::Style::default()),
            theme_name: "default".to_string(),
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

    pub(crate) fn root_frame(closure: ModeRef) -> ModeFrame {
        ModeFrame {
            title: "root".to_string(),
            closure,
            entered_via: None,
            rendered: Vec::new(),
            style: None,
            capture: false,
        }
    }

    pub(crate) fn ensure_root(&mut self, root: ModeRef) {
        if self.stack.is_empty() {
            self.stack.push(Self::root_frame(root));
        }
    }

    pub(crate) fn reset_to_root(&mut self, root: ModeRef) {
        self.stack = vec![Self::root_frame(root)];
    }

    pub(crate) fn clear_config_state(&mut self, style: config::Style) {
        self.hud_visible = false;
        self.selector = None;
        self.stack.clear();
        self.rendered = Self::empty_rendered(style);
    }

    pub(crate) fn depth(&self) -> usize {
        self.stack.len().saturating_sub(1)
    }
}

impl FocusInfo {
    pub(crate) fn mode_ctx(&self, hud: bool, depth: usize) -> ModeCtx {
        ModeCtx {
            app: self.app.clone(),
            title: self.title.clone(),
            pid: self.pid as i64,
            hud,
            depth: depth as i64,
        }
    }
}
