//! Cursor clamping and key/action resolution helpers.

use std::collections::HashSet;

use mac_keycode::Chord;
use serde::{Deserialize, Serialize};

use crate::{
    Action, Cursor, Keys, KeysAttrs, NotifyKind, Style,
    raw::{self, RawConfig},
    themes,
};

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
pub struct ConfigInput {
    /// User key bindings.
    pub keys: Keys,
    /// Optional base theme name.
    pub base_theme: Option<String>,
    /// Optional raw style overlay (applied over base theme).
    pub style: Option<raw::RawStyle>,
}

impl<'de> Deserialize<'de> for ConfigInput {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = RawConfig::deserialize(deserializer)?;
        Ok(Self {
            keys: raw.keys,
            base_theme: raw.base_theme.as_option().cloned(),
            style: raw.style.into_option(),
        })
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// User configuration for keys and UI theme (HUD + notifications).
pub struct Config {
    /// Key bindings and nested modes that drive the HUD.
    pub(crate) keys: Keys,

    /// Base style from the selected theme (before applying user overlays).
    pub(crate) style: Style,

    /// Optional user-provided raw style overlay parsed from the config file.
    #[serde(default)]
    pub(crate) user_overlay: Option<raw::RawStyle>,

    /// Server tunables: primarily used during tests/smoketests.
    #[serde(default)]
    pub(crate) server: ServerTunables,
}

// Note: Config derives Deserialize for the direct wire shape only.
// RON parsing uses loader::{load_from_path, load_from_str}, which go via RawConfig.

struct KeysScope<'a> {
    /// Keys node being searched in this scope.
    keys: &'a Keys,
    /// Merged inherited attributes for the path prefix to this scope.
    chain: KeysAttrs,
    /// When true, only `global` entries are considered.
    require_global: bool,
}

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
            server: ServerTunables::default(),
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
    pub fn hud(&self, loc: &Cursor) -> crate::Hud {
        self.style(loc).hud
    }

    /// Notification configuration for a location (layout + per-kind styles).
    pub fn notify_config(&self, loc: &Cursor) -> crate::Notify {
        self.style(loc).notify
    }

    /// Notification window style for a given notification `ty` at `loc`.
    ///
    /// This applies all overlays along `loc`, resolves the full notification theme,
    /// and returns the concrete per-kind style.
    pub fn notify(&self, loc: &Cursor, ty: NotifyKind) -> crate::NotifyWindowStyle {
        let theme = self.style(loc).notify.theme();
        match ty {
            NotifyKind::Info | NotifyKind::Ignore => theme.info,
            NotifyKind::Warn => theme.warn,
            NotifyKind::Error => theme.error,
            NotifyKind::Success => theme.success,
        }
    }

    /// Compute merged mode attributes along the current path.
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

    /// Precompute merged mode attributes for each prefix of `path`.
    fn merged_mode_prefixes(&self, path: &[u32]) -> Vec<KeysAttrs> {
        let mut cur = &self.keys;
        let mut acc = KeysAttrs::default();
        let mut out = Vec::with_capacity(path.len() + 1);
        out.push(acc.clone());
        for idx in path {
            let i = *idx as usize;
            match cur.keys.get(i) {
                Some((_k, _d, act, attrs)) => {
                    acc = acc.merged_with(attrs);
                    out.push(acc.clone());
                    if let Action::Keys(next) = act {
                        cur = next;
                    } else {
                        break;
                    }
                }
                None => break,
            }
        }
        out
    }

    /// Build the ordered list of key scopes for a location: current mode then parent globals.
    fn key_scopes<'a>(&'a self, loc: &Cursor) -> Vec<KeysScope<'a>> {
        let prefixes = self.merged_mode_prefixes(loc.path());
        let cur_keys = self.resolve(loc).unwrap_or(&self.keys);
        let mut scopes = Vec::new();
        let cur_chain = prefixes.last().cloned().unwrap_or_default();
        scopes.push(KeysScope {
            keys: cur_keys,
            chain: cur_chain,
            require_global: false,
        });

        if loc.path().is_empty() {
            return scopes;
        }

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
            let chain = prefixes.get(prefix_len).cloned().unwrap_or_default();
            scopes.push(KeysScope {
                keys: parent,
                chain,
                require_global: true,
            });
        }

        scopes
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
        chord: &Chord,
        app: &str,
        title: &str,
    ) -> Option<(Action, KeysAttrs, Option<usize>)> {
        for scope in self.key_scopes(loc) {
            for (i, (k, _d, a, attrs)) in scope.keys.keys.iter().enumerate() {
                if k != chord {
                    continue;
                }
                let eff = scope.chain.merged_with(attrs);
                if scope.require_global && !eff.global() {
                    continue;
                }
                if !entry_matches(&eff, app, title) {
                    continue;
                }
                let idx = matches!(a, Action::Keys(_)).then_some(i);
                return Some((a.clone(), eff, idx));
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
    ) -> Vec<(Chord, String, KeysAttrs, bool)> {
        let mut out = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        let hud_visible = self.hud_visible(loc);

        for scope in self.key_scopes(loc) {
            for (k, desc, attrs) in scope.keys.keys_with_attrs() {
                let eff = scope.chain.merged_with(&attrs);
                if scope.require_global && !eff.global() {
                    continue;
                }
                if eff.hud_only() && !hud_visible {
                    continue;
                }
                if !entry_matches(&eff, app, app_title) {
                    continue;
                }
                let key_s = k.to_string();
                if !seen.insert(key_s) {
                    continue;
                }
                let is_mode = matches!(scope.keys.get_with_attrs(&k), Some((Action::Keys(_), _)));
                out.push((k, desc, eff, is_mode));
            }
        }

        out
    }

    /// Return visible keys for HUD using app/title from the location's App context.
    pub fn hud_keys_ctx(&self, loc: &Cursor) -> Vec<(Chord, String, KeysAttrs, bool)> {
        let (app, title) = loc
            .app_ref()
            .map(|a| (a.app.as_str(), a.title.as_str()))
            .unwrap_or(("", ""));
        self.hud_keys(loc, app, title)
    }

    /// Returns only the current mode entries that are visible in HUD.
    pub fn visible_mode_keys(&self, loc: &Cursor) -> Vec<(Chord, String, KeysAttrs, bool)> {
        let (app, title) = loc
            .app_ref()
            .map(|a| (a.app.as_str(), a.title.as_str()))
            .unwrap_or(("", ""));
        let cur = self.resolve(loc).unwrap_or(&self.keys);
        let mode_chain = self.merged_mode_attrs(loc.path());
        let hud_visible = self.hud_visible(loc);
        cur.keys_with_attrs()
            .filter_map(|(k, desc, attrs)| {
                let eff = mode_chain.merged_with(&attrs);
                if !entry_matches(&eff, app, title) {
                    return None;
                }
                if eff.hud_only() && !hud_visible {
                    return None;
                }
                let is_mode = matches!(cur.get_with_attrs(&k), Some((Action::Keys(_), _)));
                Some((k, desc, eff, is_mode))
            })
            .collect()
    }

    /// Compute effective attributes for a chord at a location (if any).
    pub fn attrs_for_key(&self, loc: &Cursor, chord: &Chord) -> Option<KeysAttrs> {
        let cur = self.resolve(loc).unwrap_or(&self.keys);
        let mode_chain = self.merged_mode_attrs(loc.path());
        cur.keys
            .iter()
            .find(|(k, _, _, _)| k == chord)
            .map(|(_, _, _, attrs)| mode_chain.merged_with(attrs))
    }

    /// Determine if the current mode stack requests capture-all.
    pub fn mode_requests_capture(&self, loc: &Cursor) -> bool {
        if loc.path().is_empty() {
            return false;
        }
        let eff = self.merged_mode_attrs(loc.path());
        eff.capture()
    }

    /// Server tunables accessor.
    pub fn server(&self) -> &ServerTunables {
        &self.server
    }
}

/// Server-side tunables carried in the user Config so the UI can influence
/// how the embedded server behaves while exercising tests.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerTunables {
    /// When true, the server will auto-shutdown if it has no connected UI
    /// clients for a short grace period. Intended for smoketests only.
    #[serde(default)]
    pub exit_if_no_clients: bool,
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
