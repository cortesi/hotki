use config::dynamic::{ModeFrame, RenderedState};

/// Focus snapshot carried in runtime state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct FocusInfo {
    pub(crate) app: String,
    pub(crate) title: String,
    pub(crate) pid: i32,
}

/// Stack-based runtime state for dynamic configuration.
#[derive(Debug, Clone)]
pub(crate) struct RuntimeState {
    pub(crate) hud_visible: bool,
    pub(crate) stack: Vec<ModeFrame>,
    pub(crate) focus: FocusInfo,
    pub(crate) rendered: RenderedState,
    pub(crate) theme_name: String,
}

impl RuntimeState {
    pub(crate) fn empty() -> Self {
        Self {
            hud_visible: false,
            stack: Vec::new(),
            focus: FocusInfo::default(),
            rendered: RenderedState {
                bindings: Vec::new(),
                hud_rows: Vec::new(),
                style: config::Style::default(),
                capture: false,
            },
            theme_name: "default".to_string(),
        }
    }

    pub(crate) fn depth(&self) -> usize {
        self.stack.len().saturating_sub(1)
    }
}
