//! Cursor state used by the legacy static configuration/UI path.

use hotki_protocol::App;
use serde::{Deserialize, Serialize};

/// Pointer into a static key hierarchy plus UI overrides.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Cursor {
    /// Indices into the parent `Keys.keys` vector for each descent step.
    path: Vec<u32>,

    /// True when showing the root HUD via a root view (no logical descent).
    #[serde(default)]
    pub viewing_root: bool,

    /// Optional override of the base theme name for this view.
    /// When `None`, uses the theme bundled in the loaded config.
    #[serde(default)]
    pub override_theme: Option<String>,

    /// When true, ignore user overlay and render the theme without user UI tweaks.
    #[serde(default)]
    pub user_ui_disabled: bool,

    /// Optional focused application context carried with the cursor for UI/HUD
    /// rendering. When absent, callers may fall back to empty strings.
    #[serde(default)]
    pub app: Option<App>,
}

impl Cursor {
    /// Construct a new cursor from parts.
    pub fn new(path: Vec<u32>, viewing_root: bool) -> Self {
        Self {
            path,
            viewing_root,
            override_theme: None,
            user_ui_disabled: false,
            app: None,
        }
    }

    /// Logical depth equals the number of elements in the path (root = 0).
    pub fn depth(&self) -> usize {
        self.path.len()
    }

    /// Push an index step into the location path.
    pub fn push(&mut self, idx: u32) {
        self.path.push(idx);
    }

    /// Pop a step from the location path. Returns the popped index if any.
    pub fn pop(&mut self) -> Option<u32> {
        self.path.pop()
    }

    /// Clear the path, returning to root (does not change viewing_root flag).
    pub fn clear(&mut self) {
        self.path.clear();
    }

    /// Borrow the immutable path for inspection/logging.
    pub fn path(&self) -> &[u32] {
        &self.path
    }

    /// Attach an app context to this cursor and return it.
    pub fn with_app(mut self, app: App) -> Self {
        self.app = Some(app);
        self
    }

    /// Borrow the app context if present.
    pub fn app_ref(&self) -> Option<&App> {
        self.app.as_ref()
    }

    /// Set a theme override for this cursor. Use `None` to fall back to the
    /// theme loaded from disk.
    pub fn set_theme(&mut self, name: Option<&str>) {
        self.override_theme = name.map(|s| s.to_string());
    }

    /// Enable or disable user style overlays at this location.
    ///
    /// - `true` enables user-provided overlays
    /// - `false` disables them (rendering the base theme only)
    pub fn set_user_style_enabled(&mut self, enabled: bool) {
        self.user_ui_disabled = !enabled;
    }
}
