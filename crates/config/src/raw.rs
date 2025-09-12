use serde::{Deserialize, Serialize};

use super::{
    Config, Hud, Keys, Notify, Style,
    defaults::TAG_SUBMENU,
    parse_rgb,
    types::{FontWeight, NotifyPos, Offset, Pos},
};

// ===== FIELD WRAPPERS FOR OPTIONAL VALUES =====

/// Generic optional wrapper used when deserializing user config values.
///
/// Purpose
/// - Treats an omitted field or explicit unit `()` as not provided (None).
/// - Accepts a plain value `T` as provided (Some(T)).
/// - Accepts an `Option<T>` for completeness and pass‑through.
///
/// This wrapper lets top‑level and nested style sections be succinct while still
/// allowing explicit nulling where supported.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Maybe<T> {
    Unit(()),
    Value(T),
    Opt(Option<T>),
}

impl<T> Default for Maybe<T> {
    fn default() -> Self {
        Maybe::Unit(())
    }
}

impl<T> Maybe<T> {
    pub fn into_option(self) -> Option<T> {
        match self {
            Maybe::Unit(()) => None,
            Maybe::Value(v) => Some(v),
            Maybe::Opt(o) => o,
        }
    }
    pub fn as_option(&self) -> Option<&T> {
        match self {
            Maybe::Unit(()) => None,
            Maybe::Value(v) => Some(v),
            Maybe::Opt(Some(v)) => Some(v),
            Maybe::Opt(None) => None,
        }
    }
}

// Helper: accept either a plain string or an Option<String>
fn de_opt_string<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Helper {
        S(String),
        Opt(Option<String>),
    }
    match Helper::deserialize(deserializer)? {
        Helper::S(s) => Ok(Some(s)),
        Helper::Opt(o) => Ok(o),
    }
}

// ===== RAW NOTIFICATION STYLE =====

/// Raw notification style with all optional fields for merging
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RawNotifyStyle {
    #[serde(default)]
    pub bg: Option<String>,
    #[serde(default)]
    pub title_fg: Option<String>,
    #[serde(default)]
    pub body_fg: Option<String>,
    #[serde(default)]
    pub title_font_size: Option<f32>,
    #[serde(default)]
    pub title_font_weight: Option<FontWeight>,
    #[serde(default)]
    pub body_font_size: Option<f32>,
    #[serde(default)]
    pub body_font_weight: Option<FontWeight>,
    #[serde(default)]
    pub icon: Option<String>,
}

impl RawNotifyStyle {
    /// Convert to final NotifyStyle with defaults applied
    pub fn into_notify_style(self, defaults: RawNotifyWindowStyle) -> RawNotifyWindowStyle {
        RawNotifyWindowStyle {
            bg: self.bg.or(defaults.bg),
            title_fg: self.title_fg.or(defaults.title_fg),
            body_fg: self.body_fg.or(defaults.body_fg),
            title_font_size: self.title_font_size.or(defaults.title_font_size),
            title_font_weight: self.title_font_weight.or(defaults.title_font_weight),
            body_font_size: self.body_font_size.or(defaults.body_font_size),
            body_font_weight: self.body_font_weight.or(defaults.body_font_weight),
            icon: self.icon.or(defaults.icon),
        }
    }
}

/// Raw notification window styling read from configuration (string colors, optional sizes/weights).
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub(crate) struct RawNotifyWindowStyle {
    #[serde(default)]
    pub bg: Option<String>,
    #[serde(default)]
    pub title_fg: Option<String>,
    #[serde(default)]
    pub body_fg: Option<String>,
    #[serde(default)]
    pub title_font_size: Option<f32>,
    #[serde(default)]
    pub title_font_weight: Option<FontWeight>,
    #[serde(default)]
    pub body_font_size: Option<f32>,
    #[serde(default)]
    pub body_font_weight: Option<FontWeight>,
    #[serde(default)]
    pub icon: Option<String>,
}

// ===== RAW NOTIFICATION CONFIG =====

/// Raw notification config with all optional fields for conversion
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RawNotify {
    #[serde(default)]
    pub width: Option<f32>,
    #[serde(default)]
    pub pos: Option<NotifyPos>,
    #[serde(default)]
    pub opacity: Option<f32>,
    #[serde(default)]
    pub timeout: Option<f32>,
    #[serde(default)]
    pub buffer: Option<usize>,
    #[serde(default)]
    pub radius: Option<f32>,
    #[serde(default)]
    pub info: Option<RawNotifyStyle>,
    #[serde(default)]
    pub warn: Option<RawNotifyStyle>,
    #[serde(default)]
    pub error: Option<RawNotifyStyle>,
    #[serde(default)]
    pub success: Option<RawNotifyStyle>,
}

impl RawNotify {
    /// Internal helper: apply overrides over a base Notify
    fn apply_over(self, base: &Notify) -> Notify {
        let defaults = base.clone();
        macro_rules! or_field {
            ($field:ident) => {
                self.$field.unwrap_or(defaults.$field)
            };
        }
        Notify {
            width: or_field!(width),
            pos: or_field!(pos),
            opacity: or_field!(opacity),
            timeout: or_field!(timeout),
            buffer: or_field!(buffer),
            radius: or_field!(radius),
            info: self
                .info
                .map(|s| s.into_notify_style(defaults.info.clone()))
                .unwrap_or(defaults.info),
            warn: self
                .warn
                .map(|s| s.into_notify_style(defaults.warn.clone()))
                .unwrap_or(defaults.warn),
            error: self
                .error
                .map(|s| s.into_notify_style(defaults.error.clone()))
                .unwrap_or(defaults.error),
            success: self
                .success
                .map(|s| s.into_notify_style(defaults.success.clone()))
                .unwrap_or(defaults.success),
        }
    }

    /// Convert to final Notify with defaults applied
    pub fn into_notify(self) -> Notify {
        self.apply_over(&Notify::default())
    }

    /// Convert to final Notify using the provided base as defaults
    pub fn into_notify_over(self, base: &Notify) -> Notify {
        self.apply_over(base)
    }
}

// ===== RAW HUD CONFIG =====

/// Raw HUD config with all optional fields for conversion
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RawHud {
    #[serde(default)]
    pub mode: Option<crate::Mode>,
    #[serde(default)]
    pub pos: Option<Pos>,
    #[serde(default)]
    pub offset: Option<Offset>,
    #[serde(default)]
    pub font_size: Option<f32>,
    #[serde(default)]
    pub title_font_weight: Option<FontWeight>,
    #[serde(default)]
    pub key_font_size: Option<f32>,
    #[serde(default)]
    pub key_font_weight: Option<FontWeight>,
    #[serde(default)]
    pub tag_font_size: Option<f32>,
    #[serde(default)]
    pub tag_font_weight: Option<FontWeight>,
    #[serde(default)]
    pub title_fg: Option<String>,
    #[serde(default)]
    pub bg: Option<String>,
    #[serde(default)]
    pub key_fg: Option<String>,
    #[serde(default)]
    pub key_bg: Option<String>,
    #[serde(default)]
    pub mod_fg: Option<String>,
    #[serde(default)]
    pub mod_font_weight: Option<FontWeight>,
    #[serde(default)]
    pub mod_bg: Option<String>,
    #[serde(default)]
    pub tag_fg: Option<String>,
    #[serde(default)]
    pub opacity: Option<f32>,
    #[serde(default)]
    pub key_radius: Option<f32>,
    #[serde(default)]
    pub key_pad_x: Option<f32>,
    #[serde(default)]
    pub key_pad_y: Option<f32>,
    #[serde(default)]
    pub radius: Option<f32>,
    #[serde(default)]
    pub tag_submenu: Option<String>,
}

// Shared color fallback for HUD
fn color_or(src: &Option<String>, default: (u8, u8, u8)) -> (u8, u8, u8) {
    match src.as_deref() {
        Some(s) => match parse_rgb(s) {
            Some(rgb) => rgb,
            None => {
                eprintln!("Warning: invalid color '{}', using default", s);
                default
            }
        },
        None => default,
    }
}

impl RawHud {
    /// Internal helper: apply overrides over a base Hud
    fn apply_over(self, base: &Hud) -> Hud {
        macro_rules! or_field {
            ($self_:expr, $defaults:expr, $field:ident) => {
                $self_.$field.unwrap_or($defaults.$field)
            };
        }
        macro_rules! color_field {
            ($self_:expr, $defaults:expr, $field:ident) => {
                color_or(&$self_.$field, $defaults.$field)
            };
        }
        let defaults = base.clone();
        let mode = or_field!(self, defaults, mode);
        let font_size = or_field!(self, defaults, font_size);
        let title_fg = color_field!(self, defaults, title_fg);
        let bg = color_field!(self, defaults, bg);
        let key_fg = color_field!(self, defaults, key_fg);
        let key_bg = color_field!(self, defaults, key_bg);
        let mod_fg = color_field!(self, defaults, mod_fg);
        let mod_bg = color_field!(self, defaults, mod_bg);
        let tag_fg = color_field!(self, defaults, tag_fg);

        Hud {
            mode,
            pos: or_field!(self, defaults, pos),
            offset: or_field!(self, defaults, offset),
            font_size,
            title_font_weight: or_field!(self, defaults, title_font_weight),
            // key_font_size and tag_font_size default to font_size if not specified
            key_font_size: self.key_font_size.unwrap_or(font_size),
            key_font_weight: or_field!(self, defaults, key_font_weight),
            tag_font_size: self.tag_font_size.unwrap_or(font_size),
            tag_font_weight: or_field!(self, defaults, tag_font_weight),
            title_fg,
            bg,
            key_fg,
            key_bg,
            mod_fg,
            mod_font_weight: or_field!(self, defaults, mod_font_weight),
            mod_bg,
            tag_fg,
            opacity: or_field!(self, defaults, opacity),
            key_radius: or_field!(self, defaults, key_radius),
            key_pad_x: or_field!(self, defaults, key_pad_x),
            key_pad_y: or_field!(self, defaults, key_pad_y),
            radius: or_field!(self, defaults, radius),
            tag_submenu: self
                .tag_submenu
                .unwrap_or_else(|| defaults.tag_submenu.clone()),
        }
    }

    /// Convert to final Hud with defaults applied
    pub fn into_hud(self) -> Hud {
        self.apply_over(&Hud::default())
    }

    /// Convert to final Hud using the provided base as defaults
    pub fn into_hud_over(self, base: &Hud) -> Hud {
        self.apply_over(base)
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RawStyle {
    #[serde(default)]
    pub hud: Option<RawHud>,
    #[serde(default)]
    pub notify: Option<RawNotify>,
}

/// Raw configuration with all optional fields for conversion
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawConfig {
    #[serde(default)]
    pub keys: Keys,

    // Base theme selection name (used by host for theming)
    #[serde(default)]
    pub base_theme: Maybe<String>,

    // Submenu tag text displayed in HUD for nested modes
    #[serde(default, deserialize_with = "de_opt_string")]
    pub tag_submenu: Option<String>,

    // Theme configuration (grouping hud + notify)
    #[serde(default)]
    pub style: Maybe<RawStyle>,

    /// Server-side tunables. Optional; primarily for tests/smoketests.
    #[serde(default)]
    pub server: Maybe<RawServerTunables>,
}

impl RawConfig {
    /// Convert to final Config with defaults applied
    pub fn into_config(self) -> Config {
        let (hud, notify) = match self.style.into_option() {
            Some(t) => {
                let h = t.hud.map(|h| h.into_hud()).unwrap_or_else(Hud::default);
                let n = t
                    .notify
                    .map(|n| n.into_notify())
                    .unwrap_or_else(Notify::default);
                (h, n)
            }
            None => (Hud::default(), Notify::default()),
        };
        // Top-level tag_submenu (legacy) overrides HUD tag text
        let tag_text = self.tag_submenu.unwrap_or_else(|| TAG_SUBMENU.to_string());
        let mut style = Style { hud, notify };
        style.hud.tag_submenu = tag_text;
        let mut cfg = Config::from_parts(self.keys, style);
        if let Some(s) = self.server.into_option() {
            cfg.server = s.into_server_tunables();
        }
        cfg
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(deny_unknown_fields)]
pub struct RawServerTunables {
    #[serde(default)]
    pub exit_if_no_clients: bool,
}

impl RawServerTunables {
    pub fn into_server_tunables(self) -> crate::ServerTunables {
        crate::ServerTunables {
            exit_if_no_clients: self.exit_if_no_clients,
        }
    }
}
