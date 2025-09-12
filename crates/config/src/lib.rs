//! Shared configuration types (modes, themes, parsing) used by Hotki.
#![allow(missing_docs)]
#![warn(unsafe_op_in_unsafe_fn)]

use std::{collections::HashSet, convert::TryFrom};

use serde::{Deserialize, Serialize};

mod defaults;
mod error;
mod loader;
mod mode;
mod raw;
pub mod themes;
mod types;

pub use error::Error;
pub use hotki_protocol::{Cursor, Toggle};
pub use loader::{load_from_path, load_from_str};
pub use mode::{
    Action, At, AtSpec, Dir, FullscreenKind, FullscreenSpec, Grid, GridSpec, Keys, KeysAttrs,
    NotificationType, ShellModifiers, ShellSpec,
};
use raw::RawConfig;
pub use types::{FontWeight, Mode, NotifyPos, NotifyTheme, NotifyWindowStyle, Offset, Pos};

/// Extension trait providing `Cursor::ensure_in` semantics without creating a
/// dependency cycle with `hotki-protocol`.
pub trait CursorEnsureExt {
    /// Return a cursor clamped to a valid path for the given focus `(app, title)`
    /// under this configuration. Also returns `true` when the path changed.
    fn ensure_in(&self, cfg: &Config, app: &str, title: &str) -> (Cursor, bool);
}

impl CursorEnsureExt for Cursor {
    fn ensure_in(&self, cfg: &Config, app: &str, title: &str) -> (Cursor, bool) {
        let mut loc = self.clone();
        let mut changed = false;
        loop {
            if loc.path().is_empty() {
                break;
            }
            let plen = loc.path().len();
            // Walk to the parent keys without holding a long-lived borrow on loc.path
            let mut cur = &cfg.keys;
            let mut invalid = false;
            for j in 0..(plen - 1) {
                let i = loc.path()[j] as usize;
                match cur.keys.get(i) {
                    Some((_, _, Action::Keys(next), _)) => cur = next,
                    _ => {
                        invalid = true;
                        break;
                    }
                }
            }
            if invalid {
                let _ = loc.pop();
                changed = true;
                continue;
            }
            let last = loc.path()[plen - 1] as usize;
            let ok = match cur.keys.get(last) {
                Some((_k, _d, Action::Keys(_), _attrs)) => {
                    let eff = cfg.merged_mode_attrs(&loc.path()[..plen]);
                    entry_matches(&eff, app, title)
                }
                _ => false,
            };
            if ok {
                break;
            }
            let _ = loc.pop();
            changed = true;
        }
        (loc, changed)
    }
}

/// Public input form of user configuration that carries keys, optional base theme name,
/// and an optional raw style overlay. This type is suitable for reading
/// user configuration from RON without exposing raw internals.
#[derive(Debug, Clone, Serialize, Default)]
pub(crate) struct ConfigInput {
    pub keys: Keys,
    pub base_theme: Option<String>,
    pub style: Option<raw::RawStyle>,
}

impl<'de> Deserialize<'de> for ConfigInput {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = RawConfig::deserialize(deserializer)?;
        Ok(ConfigInput {
            keys: raw.keys,
            base_theme: raw.base_theme.as_option().cloned(),
            style: raw.style.into_option(),
        })
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// Visual theme configuration grouping HUD and notification settings.
pub struct Style {
    /// HUD configuration section.
    #[serde(default)]
    pub hud: Hud,

    /// Notification configuration section.
    #[serde(default)]
    pub notify: Notify,
}

impl Style {
    /// Overlay raw style overrides onto this base style using current values as defaults.
    pub(crate) fn overlay_raw(mut self, overrides: &raw::RawStyle) -> Style {
        if let Some(h) = &overrides.hud {
            self.hud = h.clone().into_hud_over(&self.hud);
        }
        if let Some(n) = &overrides.notify {
            self.notify = n.clone().into_notify_over(&self.notify);
        }
        self
    }

    /// Apply multiple raw overlays left-to-right.
    pub(crate) fn overlay_all_raw(mut self, overlays: &[raw::RawStyle]) -> Style {
        for ov in overlays {
            self = self.overlay_raw(ov);
        }
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// HUD configuration section.
pub struct Hud {
    /// Display mode selection for the HUD.
    #[serde(default)]
    pub mode: Mode,
    /// Screen anchor position for the HUD window.
    #[serde(default)]
    pub pos: Pos,
    /// Pixel offset added to the anchored position. Positive `x` moves right; positive `y` moves up.
    #[serde(default)]
    pub offset: Offset,
    /// Base font size for descriptions and general HUD text.
    #[serde(default = "defaults::default_font_size")]
    pub font_size: f32,
    /// Font weight for title/description text.
    #[serde(default)]
    pub title_font_weight: FontWeight,
    /// Font size for key tokens inside their rounded boxes. Defaults to `font_size`.
    pub key_font_size: f32,
    /// Font weight for non-modifier key tokens.
    #[serde(default)]
    pub key_font_weight: FontWeight,
    /// Font size for the tag indicator shown for sub-modes. Defaults to `font_size`.
    pub tag_font_size: f32,
    /// Font weight for the sub-mode tag indicator.
    #[serde(default)]
    pub tag_font_weight: FontWeight,
    /// Foreground color for title/description text (parsed RGB).
    pub title_fg: (u8, u8, u8),
    /// HUD background fill color (parsed RGB).
    pub bg: (u8, u8, u8),
    /// Foreground color for non-modifier key tokens (parsed RGB).
    pub key_fg: (u8, u8, u8),
    /// Background color for non-modifier key tokens (parsed RGB).
    pub key_bg: (u8, u8, u8),
    /// Foreground color for modifier key tokens (parsed RGB).
    pub mod_fg: (u8, u8, u8),
    /// Font weight for modifier key tokens.
    #[serde(default)]
    pub mod_font_weight: FontWeight,
    /// Background color for modifier key tokens (parsed RGB).
    pub mod_bg: (u8, u8, u8),
    /// Foreground color for the sub-mode tag indicator (parsed RGB).
    pub tag_fg: (u8, u8, u8),
    /// Window opacity in the range [0.0, 1.0]. `1.0` is fully opaque.
    #[serde(default = "defaults::default_opacity")]
    pub opacity: f32,
    /// Corner radius for key boxes.
    #[serde(default = "defaults::default_key_radius")]
    pub key_radius: f32,
    /// Horizontal padding inside key boxes.
    #[serde(default = "defaults::default_key_pad_x")]
    pub key_pad_x: f32,
    /// Vertical padding inside key boxes.
    #[serde(default = "defaults::default_key_pad_y")]
    pub key_pad_y: f32,
    /// Corner radius for the HUD window itself.
    #[serde(default = "defaults::default_radius")]
    pub radius: f32,
    /// Text tag shown for sub-modes at the end of rows.
    #[serde(default = "defaults::default_tag_submenu")]
    pub tag_submenu: String,
}

impl Default for Hud {
    fn default() -> Self {
        let parse_or = |s: &str| parse_rgb(s).unwrap_or((255, 255, 255));
        let fs = defaults::HUD_FONT_SIZE;
        Self {
            mode: Mode::Hud,
            pos: defaults::HUD_POS,
            offset: defaults::HUD_OFFSET,
            font_size: fs,
            title_font_weight: FontWeight::Regular,
            key_font_size: fs,
            key_font_weight: FontWeight::Regular,
            tag_font_size: fs,
            tag_font_weight: FontWeight::Regular,
            title_fg: parse_or(defaults::HUD_TITLE_FG),
            bg: parse_or(defaults::HUD_BG),
            key_fg: parse_or(defaults::HUD_KEY_FG),
            key_bg: parse_or(defaults::HUD_KEY_BG),
            mod_fg: parse_or(defaults::HUD_MOD_FG),
            mod_font_weight: FontWeight::Regular,
            mod_bg: parse_or(defaults::HUD_MOD_BG),
            tag_fg: parse_or(defaults::HUD_TAG_FG),
            opacity: defaults::HUD_OPACITY,
            key_radius: defaults::KEY_RADIUS,
            key_pad_x: defaults::KEY_PAD_X,
            key_pad_y: defaults::KEY_PAD_Y,
            radius: defaults::HUD_RADIUS,
            tag_submenu: defaults::TAG_SUBMENU.to_string(),
        }
    }
}

// Parsed HUD colors are stored directly on Hud; palette helper removed.

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Notify {
    /// Fixed width in pixels for each notification window.
    #[serde(default = "defaults::default_notify_width")]
    pub width: f32,

    /// Screen side where the notification stack is anchored (left or right).
    #[serde(default)]
    pub pos: NotifyPos,

    /// Overall window opacity in the range [0.0, 1.0].
    #[serde(default = "defaults::default_notify_opacity")]
    pub opacity: f32,

    /// Auto-dismiss timeout for a notification, in seconds.
    #[serde(default = "defaults::default_notify_timeout")]
    pub timeout: f32,

    /// Maximum number of notifications kept in the on-screen stack.
    #[serde(default = "defaults::default_notify_buffer")]
    pub buffer: usize,

    /// Corner radius for notification windows.
    #[serde(default = "defaults::default_notify_radius")]
    pub radius: f32,

    /// Styling for Info notifications.
    #[serde(default = "defaults::default_notify_info_style")]
    info: raw::RawNotifyWindowStyle,
    /// Styling for Warn notifications.
    #[serde(default = "defaults::default_notify_warn_style")]
    warn: raw::RawNotifyWindowStyle,
    /// Styling for Error notifications.
    #[serde(default = "defaults::default_notify_error_style")]
    error: raw::RawNotifyWindowStyle,
    /// Styling for Success notifications.
    #[serde(default = "defaults::default_notify_success_style")]
    success: raw::RawNotifyWindowStyle,
}

impl Default for Notify {
    fn default() -> Self {
        Self {
            width: defaults::NOTIFY_WIDTH,
            pos: defaults::NOTIFY_POS,
            opacity: defaults::NOTIFY_OPACITY,
            timeout: defaults::NOTIFY_TIMEOUT,
            buffer: defaults::NOTIFY_BUFFER,
            radius: defaults::NOTIFY_RADIUS,
            info: defaults::default_notify_info_style(),
            warn: defaults::default_notify_warn_style(),
            error: defaults::default_notify_error_style(),
            success: defaults::default_notify_success_style(),
        }
    }
}

impl Notify {
    pub fn theme(&self) -> NotifyTheme {
        // Fallback defaults from a default Notify instance
        let d = Notify::default();

        fn choose<'a>(s: &'a Option<String>, d: &'a Option<String>) -> &'a str {
            if let Some(v) = s {
                v
            } else {
                d.as_deref().unwrap()
            }
        }
        fn parse_or_default(val: &str, def: &str) -> (u8, u8, u8) {
            parse_rgb(val).unwrap_or_else(|| parse_rgb(def).unwrap())
        }

        let weight_or = |w: &Option<FontWeight>| w.unwrap_or(FontWeight::Regular);
        let size_or = |s: &Option<f32>, def: f32| s.unwrap_or(def);

        NotifyTheme {
            info: NotifyWindowStyle {
                bg: parse_or_default(
                    choose(&self.info.bg, &d.info.bg),
                    d.info.bg.as_deref().unwrap(),
                ),
                title_fg: parse_or_default(
                    choose(&self.info.title_fg, &d.info.title_fg),
                    d.info.title_fg.as_deref().unwrap(),
                ),
                body_fg: parse_or_default(
                    choose(&self.info.body_fg, &d.info.body_fg),
                    d.info.body_fg.as_deref().unwrap(),
                ),
                title_font_size: size_or(
                    &self.info.title_font_size,
                    d.info.title_font_size.unwrap_or(14.0),
                ),
                title_font_weight: weight_or(&self.info.title_font_weight),
                body_font_size: size_or(
                    &self.info.body_font_size,
                    d.info.body_font_size.unwrap_or(12.0),
                ),
                body_font_weight: weight_or(&self.info.body_font_weight),
                icon: Some(choose(&self.info.icon, &d.info.icon).to_string()),
            },
            warn: NotifyWindowStyle {
                bg: parse_or_default(
                    choose(&self.warn.bg, &d.warn.bg),
                    d.warn.bg.as_deref().unwrap(),
                ),
                title_fg: parse_or_default(
                    choose(&self.warn.title_fg, &d.warn.title_fg),
                    d.warn.title_fg.as_deref().unwrap(),
                ),
                body_fg: parse_or_default(
                    choose(&self.warn.body_fg, &d.warn.body_fg),
                    d.warn.body_fg.as_deref().unwrap(),
                ),
                title_font_size: size_or(
                    &self.warn.title_font_size,
                    d.warn.title_font_size.unwrap_or(14.0),
                ),
                title_font_weight: weight_or(&self.warn.title_font_weight),
                body_font_size: size_or(
                    &self.warn.body_font_size,
                    d.warn.body_font_size.unwrap_or(12.0),
                ),
                body_font_weight: weight_or(&self.warn.body_font_weight),
                icon: Some(choose(&self.warn.icon, &d.warn.icon).to_string()),
            },
            error: NotifyWindowStyle {
                bg: parse_or_default(
                    choose(&self.error.bg, &d.error.bg),
                    d.error.bg.as_deref().unwrap(),
                ),
                title_fg: parse_or_default(
                    choose(&self.error.title_fg, &d.error.title_fg),
                    d.error.title_fg.as_deref().unwrap(),
                ),
                body_fg: parse_or_default(
                    choose(&self.error.body_fg, &d.error.body_fg),
                    d.error.body_fg.as_deref().unwrap(),
                ),
                title_font_size: size_or(
                    &self.error.title_font_size,
                    d.error.title_font_size.unwrap_or(14.0),
                ),
                title_font_weight: weight_or(&self.error.title_font_weight),
                body_font_size: size_or(
                    &self.error.body_font_size,
                    d.error.body_font_size.unwrap_or(12.0),
                ),
                body_font_weight: weight_or(&self.error.body_font_weight),
                icon: Some(choose(&self.error.icon, &d.error.icon).to_string()),
            },
            success: NotifyWindowStyle {
                bg: parse_or_default(
                    choose(&self.success.bg, &d.success.bg),
                    d.success.bg.as_deref().unwrap(),
                ),
                title_fg: parse_or_default(
                    choose(&self.success.title_fg, &d.success.title_fg),
                    d.success.title_fg.as_deref().unwrap(),
                ),
                body_fg: parse_or_default(
                    choose(&self.success.body_fg, &d.success.body_fg),
                    d.success.body_fg.as_deref().unwrap(),
                ),
                title_font_size: size_or(
                    &self.success.title_font_size,
                    d.success.title_font_size.unwrap_or(14.0),
                ),
                title_font_weight: weight_or(&self.success.title_font_weight),
                body_font_size: size_or(
                    &self.success.body_font_size,
                    d.success.body_font_size.unwrap_or(12.0),
                ),
                body_font_weight: weight_or(&self.success.body_font_weight),
                icon: Some(choose(&self.success.icon, &d.success.icon).to_string()),
            },
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
/// User configuration for keys and UI theme (HUD + notifications).
pub struct Config {
    /// Key bindings and nested modes that drive the HUD.
    pub(crate) keys: Keys,

    /// Base style from the selected theme (before applying user overlays).
    pub(crate) style: Style,

    /// Optional user-provided raw style overlay parsed from the config file.
    #[serde(default)]
    pub(crate) user_overlay: Option<raw::RawStyle>,
}

// Note: Config derives Deserialize for the direct wire shape only.
// RON parsing uses loader::{load_from_path, load_from_str}, which go via RawConfig.

impl Config {
    /// Get a clone of the base Style without applying overlays.
    pub fn base_style(&self) -> Style {
        self.style.clone()
    }
    // Construct a Config from discrete parts.
    pub fn from_parts(keys: Keys, style: Style) -> Self {
        Self {
            keys,
            style,
            user_overlay: None,
        }
    }

    /// Resolve the `Keys` node at a location. Returns root when `path` is empty.
    pub fn resolve<'a>(&'a self, loc: &Cursor) -> Option<&'a Keys> {
        let mut cur = &self.keys;
        for (depth, idx) in loc.path().iter().enumerate() {
            let i = *idx as usize;
            let (_, _, action, _) = cur.keys.get(i)?;
            match action {
                Action::Keys(next) => cur = next,
                _ => {
                    // Invalid path: points at non-mode entry
                    let _ = depth; // silence unused in non-tracing builds
                    return None;
                }
            }
        }
        Some(cur)
    }

    /// Description of the binding leading into the current mode (None at root or viewing_root).
    pub fn parent_title<'a>(&'a self, loc: &Cursor) -> Option<&'a str> {
        if loc.path().is_empty() || loc.viewing_root {
            return None;
        }
        // Walk to parent keys, then take desc at last index
        let parent_path_len = loc.path().len() - 1;
        let mut cur = &self.keys;
        for idx in &loc.path()[..parent_path_len] {
            let i = *idx as usize;
            let (_, _, action, _) = cur.keys.get(i)?;
            match action {
                Action::Keys(next) => cur = next,
                _ => return None,
            }
        }
        let last = *loc.path().last().unwrap() as usize;
        cur.keys.get(last).map(|(_, desc, _, _)| desc.as_str())
    }

    /// Logical depth equals the path length (viewing_root does not add depth).
    pub fn depth(&self, loc: &Cursor) -> usize {
        loc.depth()
    }

    /// HUD is visible when viewing_root is set or depth > 0.
    pub fn hud_visible(&self, loc: &Cursor) -> bool {
        loc.viewing_root || !loc.path().is_empty()
    }

    /// Effective style for a location: base style overlaid by the chain.
    pub(crate) fn style(&self, loc: &Cursor) -> Style {
        let mut chain = Vec::new();
        let mut cur = &self.keys;
        for idx in loc.path().iter() {
            let i = *idx as usize;
            if let Some((_, _, action, attrs)) = cur.keys.get(i) {
                if let Some(ov) = &attrs.style {
                    chain.push(ov.clone());
                }
                if let Action::Keys(next) = action {
                    cur = next;
                } else {
                    break;
                }
            } else {
                break;
            }
        }

        // Select base theme: location override takes precedence, else loaded style.
        let mut base = if let Some(name) = &loc.override_theme {
            themes::load_theme(Some(name.as_str()))
        } else {
            self.style.clone()
        };
        // Apply user overlay unless disabled
        if !loc.user_ui_disabled
            && let Some(ov) = &self.user_overlay
        {
            base = base.overlay_raw(ov);
        }
        base.overlay_all_raw(&chain)
    }

    /// HUD style for a location after applying all overlays along `loc`.
    pub fn hud(&self, loc: &Cursor) -> Hud {
        self.style(loc).hud
    }

    /// Notification configuration for a location (layout + per-kind styles).
    pub fn notify_config(&self, loc: &Cursor) -> Notify {
        self.style(loc).notify
    }

    /// Notification window style for a given notification `ty` at `loc`.
    ///
    /// This applies all overlays along `loc`, resolves the full notification theme,
    /// and returns the concrete per-kind style.
    pub fn notify(&self, loc: &Cursor, ty: NotificationType) -> NotifyWindowStyle {
        let theme = self.style(loc).notify.theme();
        match ty {
            NotificationType::Info | NotificationType::Ignore => theme.info,
            NotificationType::Warn => theme.warn,
            NotificationType::Error => theme.error,
            NotificationType::Success => theme.success,
        }
    }

    /// Resolve the action for a chord using app/title from the location's App context.
    pub fn action_ctx(
        &self,
        loc: &Cursor,
        chord: &mac_keycode::Chord,
    ) -> Option<(Action, KeysAttrs, Option<usize>)> {
        let (app, title) = loc
            .app_ref()
            .map(|a| (a.app.as_str(), a.title.as_str()))
            .unwrap_or(("", ""));
        self.action(loc, chord, app, title)
    }

    // Compute merged mode attributes along the current path.
    fn merged_mode_attrs(&self, path: &[u32]) -> KeysAttrs {
        let mut cur = &self.keys;
        let mut acc = KeysAttrs::default();
        for idx in path {
            let i = *idx as usize;
            match cur.keys.get(i) {
                Some((_k, _d, act, attrs)) => {
                    acc = acc.merged_with(attrs);
                    if let Action::Keys(next) = act {
                        cur = next;
                    } else {
                        break;
                    }
                }
                None => break,
            }
        }
        acc
    }

    /// Resolve the action for a chord at the given location and focus context.
    ///
    /// Resolution order:
    /// - Current keys: first definition that matches the chord and guard.
    /// - Parent chain (nearest outwards first): first `global` entry matching chord and guard.
    ///
    /// Returns the chosen action, attributes, and the index within the `Keys` node where
    /// it was defined when the action is a `Keys` (submode). For non-mode actions the
    /// index is `None`.
    pub fn action(
        &self,
        loc: &Cursor,
        chord: &mac_keycode::Chord,
        app: &str,
        title: &str,
    ) -> Option<(Action, KeysAttrs, Option<usize>)> {
        // 1) Current node (merged attrs)
        let cur = self.resolve(loc).unwrap_or(&self.keys);
        let mode_chain = self.merged_mode_attrs(loc.path());
        for (i, (k, _d, a, attrs)) in cur.keys.iter().enumerate() {
            if k != chord {
                continue;
            }
            let eff = mode_chain.merged_with(attrs);
            if !entry_matches(&eff, app, title) {
                continue;
            }
            let idx = matches!(a, Action::Keys(_)).then_some(i);
            return Some((a.clone(), eff, idx));
        }

        // 2) Parents outward (including root when path non-empty)
        if !loc.path().is_empty() {
            let mut parents: Vec<&Keys> = Vec::new();
            let mut k = &self.keys;
            for idx in loc.path().iter() {
                parents.push(k);
                let ii = *idx as usize;
                match k.keys.get(ii) {
                    Some((_, _, Action::Keys(next), _)) => k = next,
                    _ => break,
                }
            }
            // Track path prefix for each parent depth to compute merged attrs
            let plen = parents.len();
            for (i_parent, parent) in parents.into_iter().rev().enumerate() {
                let prefix_len = plen - 1 - i_parent;
                let mode_chain = self.merged_mode_attrs(&loc.path()[..prefix_len]);
                for (i, (k, _d, a, attrs)) in parent.keys.iter().enumerate() {
                    let eff = mode_chain.merged_with(attrs);
                    if k != chord || !eff.global() {
                        continue;
                    }
                    if !entry_matches(&eff, app, title) {
                        continue;
                    }
                    let idx = matches!(a, Action::Keys(_)).then_some(i);
                    return Some((a.clone(), eff, idx));
                }
            }
        }

        None
    }

    /// Return visible keys that should be displayed in the HUD for a location and focus.
    pub fn hud_keys(
        &self,
        loc: &Cursor,
        app: &str,
        app_title: &str,
    ) -> Vec<(mac_keycode::Chord, String, KeysAttrs, bool)> {
        let mut out = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();

        let cur = self.resolve(loc).unwrap_or(&self.keys);
        let hud_visible = self.hud_visible(loc);
        let mode_chain = self.merged_mode_attrs(loc.path());

        // Current mode entries first (with merged attrs)
        for (k, desc, attrs) in cur.keys_with_attrs() {
            let eff = mode_chain.merged_with(&attrs);
            if !entry_matches(&eff, app, app_title) {
                continue;
            }
            if eff.hud_only() && !hud_visible {
                continue;
            }
            let key_s = k.to_string();
            if !seen.insert(key_s) {
                continue;
            }
            let is_mode = matches!(cur.get_with_attrs(&k), Some((Action::Keys(_), _)));
            out.push((k, desc, eff, is_mode));
        }

        // Parents for inherited globals
        if !loc.path().is_empty() {
            // Build vector of parents from nearest to root
            let mut parents: Vec<&Keys> = Vec::new();
            let mut k = &self.keys;
            for idx in loc.path().iter() {
                parents.push(k);
                let ii = *idx as usize;
                match k.keys.get(ii) {
                    Some((_, _, Action::Keys(next), _)) => k = next,
                    _ => break,
                }
            }
            let plen = parents.len();
            for (i_parent, parent) in parents.into_iter().rev().enumerate() {
                let prefix_len = plen - 1 - i_parent;
                let parent_chain = self.merged_mode_attrs(&loc.path()[..prefix_len]);
                for (k, desc, attrs) in parent.keys_with_attrs() {
                    let eff = parent_chain.merged_with(&attrs);
                    if !eff.global() {
                        continue;
                    }
                    if eff.hud_only() && !hud_visible {
                        continue;
                    }
                    let key_s = k.to_string();
                    if seen.contains(&key_s) {
                        continue;
                    }
                    let ok = match parent.get_with_attrs(&k) {
                        Some((Action::Keys(_), _)) => entry_matches(&eff, app, app_title),
                        _ => entry_matches(&eff, app, app_title),
                    };
                    if !ok {
                        continue;
                    }
                    let is_mode = matches!(parent.get_with_attrs(&k), Some((Action::Keys(_), _)));
                    seen.insert(key_s);
                    out.push((k, desc, eff, is_mode));
                }
            }
        }

        out
    }

    /// Return visible keys for HUD using app/title from the location's App context.
    pub fn hud_keys_ctx(&self, loc: &Cursor) -> Vec<(mac_keycode::Chord, String, KeysAttrs, bool)> {
        let (app, title) = loc
            .app_ref()
            .map(|a| (a.app.as_str(), a.title.as_str()))
            .unwrap_or(("", ""));
        self.hud_keys(loc, app, title)
    }

    /// Returns only the current frame's capture request (callers gate with HUD visibility).
    pub fn mode_requests_capture(&self, loc: &Cursor) -> bool {
        if loc.path().is_empty() {
            return false;
        }
        let eff = self.merged_mode_attrs(loc.path());
        eff.capture()
    }
}

fn entry_matches(attrs: &KeysAttrs, app: &str, title: &str) -> bool {
    let ma = attrs
        .match_app
        .as_ref()
        .and_then(|s| regex::Regex::new(s).ok());
    if let Some(r) = &ma
        && !r.is_match(app)
    {
        return false;
    }
    let mt = attrs
        .match_title
        .as_ref()
        .and_then(|s| regex::Regex::new(s).ok());
    if let Some(r) = &mt
        && !r.is_match(title)
    {
        return false;
    }
    true
}

/// Parse color into raw rgb tuple
pub(crate) fn parse_rgb(s: &str) -> Option<(u8, u8, u8)> {
    colornames::Color::try_from(s).ok().map(|c| c.rgb())
}

/// Determine the default user config path (~/.hotki.ron).
pub fn default_config_path() -> std::path::PathBuf {
    let mut p = std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    p.push(".hotki.ron");
    p
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::loader;

    #[test]
    fn test_hud_mode_default_is_hud() {
        let cfg: Config = loader::load_from_str("(keys: [])", None).unwrap();
        assert!(matches!(cfg.style.hud.mode, Mode::Hud));
    }

    #[test]
    fn test_hud_mode_deserialize_variants() {
        let cfg_hide: Config =
            loader::load_from_str("(keys: [], style: (hud: (mode: hide)))", None).unwrap();
        assert!(matches!(cfg_hide.hud(&Cursor::default()).mode, Mode::Hide));

        let cfg_mini: Config =
            loader::load_from_str("(keys: [], style: (hud: (mode: mini)))", None).unwrap();
        assert!(matches!(cfg_mini.hud(&Cursor::default()).mode, Mode::Mini));

        let cfg_hud: Config =
            loader::load_from_str("(keys: [], style: (hud: (mode: hud)))", None).unwrap();
        assert!(matches!(cfg_hud.hud(&Cursor::default()).mode, Mode::Hud));
    }

    #[test]
    fn test_hud_mode_overlay_application() {
        // Nice overlay form
        let base: Config = loader::load_from_str("(keys: [])", None).unwrap();
        let overlay: crate::raw::RawStyle = crate::raw::RawStyle {
            hud: Some(crate::raw::RawHud {
                mode: Some(Mode::Mini),
                ..Default::default()
            }),
            ..Default::default()
        };
        let themed = base.style.clone().overlay_raw(&overlay);
        assert!(matches!(themed.hud.mode, Mode::Mini));

        // Raw overlay form (through RawStyle path)
        let overlay_raw: crate::raw::RawStyle = crate::raw::RawStyle {
            hud: Some(crate::raw::RawHud {
                mode: Some(Mode::Hide),
                ..Default::default()
            }),
            ..Default::default()
        };
        let themed2 = base.style.overlay_raw(&overlay_raw);
        assert!(matches!(themed2.hud.mode, Mode::Hide));
    }

    #[test]
    fn test_config_deserialization() {
        // Test with proper Config struct format
        let config_text = r#"(
            keys: [
                ("a", "Say hello", shell("echo 'Hello'")),
                ("b", "Say world", shell("echo 'World'")),
                ("m", "Submenu", keys([
                    ("x", "Exit submenu", pop),
                ])),
            ],
            style: (hud: (pos: n)),
        )"#;

        let config: Config = loader::load_from_str(config_text, None).unwrap();

        // Verify we have the expected keys
        let keys = config.keys.keys();
        let key_vec: Vec<_> = keys.collect();
        assert_eq!(key_vec.len(), 3);

        // Check that the keys contain our expected values
        let key_strings: Vec<String> = key_vec.iter().map(|(k, _)| k.clone()).collect();
        assert!(key_strings.contains(&"a".to_string()));
        assert!(key_strings.contains(&"b".to_string()));
        assert!(key_strings.contains(&"m".to_string()));

        // Defaults present (theme defaults)
        assert_eq!(config.style.hud.font_size, 14.0);
        assert_eq!(config.style.hud.title_fg, parse_rgb("#d0d0d0").unwrap());
        assert_eq!(config.style.hud.bg, parse_rgb("#101010").unwrap());
        // Parsed color defaults
        assert_eq!(config.style.hud.key_fg, parse_rgb("#d0d0d0").unwrap());
        assert_eq!(config.style.hud.key_bg, parse_rgb("#2c3471").unwrap());
        assert_eq!(config.style.hud.mod_fg, parse_rgb("white").unwrap());
        assert_eq!(config.style.hud.mod_bg, parse_rgb("#43414d").unwrap());
        // key_font_size comes from theme
        assert_eq!(config.style.hud.key_font_size, 19.0);
        // opacity default
        assert!((config.style.hud.opacity - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_parse_color_names_and_hex() {
        // Named color, case/whitespace-insensitive
        assert!(parse_rgb("DodgerBlue").is_some());
        assert!(parse_rgb("dodgerblue").is_some());
        assert!(parse_rgb("dodger blue").is_some());

        // Hex long and short
        assert_eq!(parse_rgb("#000011").unwrap(), (0, 0, 17));
        assert_eq!(parse_rgb("#fff").unwrap(), (255, 255, 255));

        // Invalid returns None
        assert!(parse_rgb("not-a-color").is_none());
    }

    #[test]
    fn test_visible_keys_ordering_and_filters() {
        // Root has two global keys (x, y), a HUD-only root key 'r', and a mode 'm' with several entries
        let keys: Keys = ron::from_str(
            r#"[
                ("x", "RootA", shell("echo"), (global: true)),
                ("y", "RootB", shell("echo"), (global: true)),
                ("r", "Root HUD only", shell("echo"), (hud_only: true)),
                ("m", "Mode", keys([
                    ("a", "A", shell("echo")),
                    ("b", "B", shell("echo")),
                    ("h", "HUD only", shell("echo"), (hud_only: true))
                ]), (capture: true)),
            ]"#,
        )
        .expect("parse keys");
        let cfg = Config::from_parts(keys, Style::default());

        // Location into the fourth entry (index 3): the mode 'm'
        let loc_mode = Cursor::new(vec![3], false);

        // In a mode, HUD is visible -> expect A,B,H then root globals X,Y (R is root-only and not global)
        let vks_mode = cfg.hud_keys(&loc_mode, "AppX", "Bad title");
        let idents_mode: Vec<String> = vks_mode
            .into_iter()
            .map(|(c, _, _, _)| c.to_string())
            .collect();
        assert_eq!(idents_mode, vec!["a", "b", "h", "x", "y"]);

        // At root (no viewing_root), HUD is hidden -> root HUD-only key 'r' is suppressed
        let loc_root = Cursor::new(vec![], false);
        let vks_root = cfg.hud_keys(&loc_root, "AppX", "Bad title");
        let idents_root: Vec<String> = vks_root
            .into_iter()
            .map(|(c, _, _, _)| c.to_string())
            .collect();
        // Expect the visible root keys: globals X,Y and the mode 'm' (not global but present at root).
        // 'r' should be filtered because HUD is not visible at root.
        assert_eq!(idents_root, vec!["x", "y", "m"]);
    }

    #[test]
    fn test_capture_and_hud_visible() {
        let cfg: Config = loader::load_from_str(
            r#"(
                keys: [
                    ("m", "Mode", keys([
                        ("a", "A", shell("echo")),
                    ]), (capture: true)),
                ],
            )"#,
            None,
        )
        .expect("parse config");

        let loc_mode = Cursor::new(vec![0], false);
        assert!(cfg.hud_visible(&loc_mode));
        assert!(cfg.mode_requests_capture(&loc_mode));

        let loc_root_view = Cursor::new(vec![], true);
        assert!(cfg.hud_visible(&loc_root_view));

        let loc_root = Cursor::new(vec![], false);
        assert!(!cfg.hud_visible(&loc_root));
    }

    #[test]
    fn test_config_color_defaults_and_parsing() {
        // Missing title_fg/bg should default to theme defaults
        let cfg: Config = loader::load_from_str("(keys: [])", None).unwrap();
        assert_eq!(cfg.style.hud.title_fg, parse_rgb("#d0d0d0").unwrap());
        assert_eq!(cfg.style.hud.bg, parse_rgb("#101010").unwrap());
        assert_eq!(cfg.style.hud.key_fg, parse_rgb("#d0d0d0").unwrap());
        assert_eq!(cfg.style.hud.key_bg, parse_rgb("#2c3471").unwrap());
        assert_eq!(cfg.style.hud.mod_fg, parse_rgb("white").unwrap());
        assert_eq!(cfg.style.hud.mod_bg, parse_rgb("#43414d").unwrap());

        // Custom colors
        let cfg2: Config = loader::load_from_str(
            "(keys: [], style: (hud: (title_fg: \"Pink Lemonade\", bg: \"#123\")))",
            None,
        )
        .unwrap();
        let hud2 = cfg2.hud(&Cursor::default());
        assert_eq!(hud2.title_fg, parse_rgb("Pink Lemonade").unwrap());
        assert_eq!(hud2.bg, parse_rgb("#123").unwrap());
    }

    #[test]
    fn test_key_font_size_default_and_override() {
        // Default: None -> consumer should fall back to font_size
        let cfg: Config =
            loader::load_from_str("(keys: [], style: (hud: (font_size: 18.0)))", None).unwrap();
        let hud = cfg.hud(&Cursor::default());
        assert_eq!(hud.font_size, 18.0);
        assert_eq!(hud.key_font_size, 18.0);

        // Explicit override as a number
        let cfg2: Config = loader::load_from_str(
            "(keys: [], style: (hud: (font_size: 16.0, key_font_size: 22.0)))",
            None,
        )
        .unwrap();
        let hud2 = cfg2.hud(&Cursor::default());
        assert_eq!(hud2.key_font_size, 22.0);
    }

    #[test]
    fn test_pos_and_offset_defaults_and_custom() {
        let cfg: Config = loader::load_from_str("(keys: [])", None).unwrap();
        // Pos default is Center
        assert!(matches!(cfg.style.hud.pos, Pos::Center));
        assert_eq!(cfg.style.hud.offset.x, 0.0);
        assert_eq!(cfg.style.hud.offset.y, 0.0);

        // Custom pos/offset
        let cfg2: Config = loader::load_from_str(
            "(keys: [], style: (hud: (pos: sw, offset: (x: 10.0, y: -5.0))))",
            None,
        )
        .unwrap();
        let hud2 = cfg2.hud(&Cursor::default());
        assert!(matches!(hud2.pos, Pos::SW));
        assert_eq!(hud2.offset.x, 10.0);
        assert_eq!(hud2.offset.y, -5.0);
    }

    #[test]
    fn test_opacity_default_and_custom() {
        let cfg: Config = loader::load_from_str("(keys: [])", None).unwrap();
        assert!((cfg.hud(&Cursor::default()).opacity - 1.0).abs() < f32::EPSILON);

        let cfg2: Config =
            loader::load_from_str("(keys: [], style: (hud: (opacity: 0.4)))", None).unwrap();
        assert!((cfg2.hud(&Cursor::default()).opacity - 0.4).abs() < f32::EPSILON);
    }

    #[test]
    fn test_unknown_top_level_key_fails() {
        let invalid = loader::load_from_str("(keys: [], unknown: 1)", None);
        assert!(invalid.is_err());
    }

    #[test]
    fn test_unknown_notify_field_fails() {
        let invalid =
            loader::load_from_str("(keys: [], style: (notify: (width: 400.0, foo: 1)))", None);
        assert!(invalid.is_err());
    }

    #[test]
    fn test_notify_defaults_when_omitted() {
        let cfg: Config = loader::load_from_str("(keys: [])", None).unwrap();
        let notify = cfg.notify_config(&Cursor::default());
        assert_eq!(notify.width, 420.0);
        assert_eq!(notify.buffer, 200);
    }

    #[test]
    fn test_notify_empty_tuple_uses_defaults() {
        let cfg: Config = loader::load_from_str("(keys: [], style: (notify: ()))", None).unwrap();
        let notify = cfg.notify_config(&Cursor::default());
        assert_eq!(notify.width, 420.0);
        assert_eq!(notify.buffer, 200);
    }

    #[test]
    fn test_notify_partial_fields_override() {
        let cfg: Config = loader::load_from_str(
            "(keys: [], style: (notify: (width: 500.0, timeout: 2.5)))",
            None,
        )
        .unwrap();
        let notify = cfg.notify_config(&Cursor::default());
        assert!((notify.width - 500.0).abs() < f32::EPSILON);
        assert!((notify.timeout - 2.5).abs() < f32::EPSILON);
        // Unspecified fields default
        assert_eq!(notify.buffer, 200);
    }

    #[test]
    fn test_notify_style_defaults_when_omitted() {
        let cfg: Config = loader::load_from_str("(keys: [])", None).unwrap();
        // Defaults applied when subsection is omitted
        let n = cfg.notify_config(&Cursor::default());
        assert_eq!(n.info.title_fg.as_deref(), Some("white"));
        assert_eq!(n.info.bg.as_deref(), Some("#222222"));
        assert_eq!(n.warn.title_fg.as_deref(), Some("#ffc100"));
        assert_eq!(n.warn.bg.as_deref(), Some("#442a00"));
    }

    #[test]
    fn test_switch_theme_changes_base_colors_via_cursor() {
        // Build via loader so base theme comes from theme files
        let cfg = loader::load_from_str("(keys: [])", None).unwrap();
        let root = Cursor::default();
        // Starts with default theme values
        assert_eq!(cfg.hud(&root).title_fg, (0xd0, 0xd0, 0xd0));

        let mut loc2 = Cursor::default();
        loc2.set_theme(Some("dark-blue"));
        assert_eq!(cfg.hud(&loc2).title_fg, (0xa0, 0xc4, 0xff));

        // Reset override
        loc2.clear_theme();
        assert_eq!(cfg.hud(&loc2).title_fg, (0xd0, 0xd0, 0xd0));
    }

    #[test]
    fn test_user_ui_disabled_toggles_overlay_via_cursor() {
        let cfg =
            loader::load_from_str("(keys: [], style: (hud: (font_size: 20.0)))", None).unwrap();
        let root = Cursor::default();
        assert_eq!(cfg.hud(&root).font_size, 20.0);

        let mut loc2 = root.clone();
        loc2.set_user_style_enabled(false);
        // Theme default font size in theme files is 14.0
        assert_eq!(cfg.hud(&loc2).font_size, 14.0);

        loc2.set_user_style_enabled(true);
        assert_eq!(cfg.hud(&loc2).font_size, 20.0);
    }

    #[test]
    fn test_attrs_inherit_noexit_and_repeat() {
        // Parent mode sets noexit and repeat timing
        let cfg: Config = loader::load_from_str(
            r#"(
                keys: [
                    ("m", "Mode", keys([
                        ("x", "Exec", shell("echo")),
                        ("y", "Exec2", shell("echo"), (noexit: false, repeat: false, repeat_delay: 10, repeat_interval: 20)),
                    ]), (noexit: true, repeat: true, repeat_delay: 300, repeat_interval: 50)),
                ],
            )"#,
            None,
        )
        .unwrap();
        let loc = Cursor::new(vec![0], false);
        let x = mac_keycode::Chord::parse("x").unwrap();
        let y = mac_keycode::Chord::parse("y").unwrap();

        // Inherit from parent
        let (_a, attrs, _idx) = cfg.action(&loc, &x, "App", "Title").expect("x present");
        assert!(attrs.noexit());
        assert_eq!(attrs.repeat, Some(true));
        assert_eq!(attrs.repeat_delay, Some(300));
        assert_eq!(attrs.repeat_interval, Some(50));

        // Child override to false and new delays
        let (_a, attrs2, _idx) = cfg.action(&loc, &y, "App", "Title").expect("y present");
        assert!(!attrs2.noexit());
        assert_eq!(attrs2.repeat, Some(false));
        assert_eq!(attrs2.repeat_delay, Some(10));
        assert_eq!(attrs2.repeat_interval, Some(20));
    }

    #[test]
    fn test_attrs_inherit_global_to_descendants() {
        // Parent mode marks entries as global via inherited attribute
        let cfg: Config = loader::load_from_str(
            r#"(
                keys: [
                    ("m", "Mode", keys([
                        ("a", "Alpha", shell("echo")),
                        ("n", "Next", keys([])),
                    ]), (global: true)),
                ],
            )"#,
            None,
        )
        .unwrap();
        // Enter m -> then n (grandchild location)
        let loc = Cursor::new(vec![0, 1], false);
        let a = mac_keycode::Chord::parse("a").unwrap();
        // Expect to resolve parent's 'a' via inherited global
        let (_act, attrs, _idx) = cfg.action(&loc, &a, "", "").expect("global a available");
        assert!(attrs.global());
    }

    #[test]
    fn test_attrs_inherit_match_app_and_ensure_context() {
        // Parent mode guards by app name; child has no guard
        let cfg: Config = loader::load_from_str(
            r#"(
                keys: [
                    ("m", "Mode", keys([
                        ("x", "Exec", shell("echo")),
                    ]), (match_app: "^Foo$")),
                ],
            )"#,
            None,
        )
        .unwrap();
        let loc = Cursor::new(vec![0], false);
        // Mismatch -> ensure_in should pop back to root
        let (loc_after, changed) = CursorEnsureExt::ensure_in(&loc, &cfg, "Bar", "");
        assert!(changed);
        assert_eq!(loc_after.path(), &[]);

        // Match -> remains in mode and action available
        let loc2 = Cursor::new(vec![0], false);
        let (loc2_after, changed2) = CursorEnsureExt::ensure_in(&loc2, &cfg, "Foo", "");
        assert!(!changed2);
        let x = mac_keycode::Chord::parse("x").unwrap();
        assert!(cfg.action(&loc2_after, &x, "Foo", "").is_some());
    }

    #[test]
    fn test_per_mode_overlay_applies_with_theme_and_toggle_via_cursor() {
        // A key that overlays HUD mode to mini
        let cfg = loader::load_from_str(
            "(keys: [ (\"a\", \"Go\", keys([]), (style: (hud: (mode: mini)))) ])",
            None,
        )
        .unwrap();
        let mut loc_root = Cursor::default();
        let loc_sub = Cursor::new(vec![0], false);

        assert!(matches!(cfg.hud(&loc_root).mode, Mode::Hud));
        assert!(matches!(cfg.hud(&loc_sub).mode, Mode::Mini));

        loc_root.set_user_style_enabled(false);
        // Per-mode overlay still applies regardless of the user UI toggle
        assert!(matches!(cfg.hud(&loc_sub).mode, Mode::Mini));

        loc_root.set_theme(Some("dark-blue"));
        assert!(matches!(cfg.hud(&loc_sub).mode, Mode::Mini));
    }

    // Removed roundtrip test for override flags: theme override and UI toggle now live on Location.
    #[test]
    fn test_error_pretty_contains_excerpt_and_pointer() {
        let src = "keys: [ (\"a\", \"desc\", exit), ]";
        let err = ron::from_str::<raw::RawConfig>(src).unwrap_err();
        let e = Error::from_ron(src, &err, Some(Path::new("/tmp/test.ron")));
        let msg = e.pretty();
        assert!(msg.contains("/tmp/test.ron"));
        assert!(msg.contains("^"));
    }
}
