//! Shared configuration types (modes, style, parsing) used by Hotki.
#![warn(missing_docs)]
#![warn(unsafe_op_in_unsafe_fn)]

use std::{
    env,
    path::{Path, PathBuf},
};

mod check;
mod docs;
mod error;
mod mode;
mod raw;
mod script;
mod style;
mod types;

/// Engine-facing retained configuration runtime.
pub mod runtime;

#[cfg(test)]
mod test_merge;

pub use check::{
    LuauCheckReport, check_luau_config, check_luau_style_file, check_luau_style_source,
};
pub use docs::{LuauApiSurface, luau_api, luau_api_markdown, luau_api_surface, luau_api_text};
pub use error::Error;
pub use hotki_protocol::{NotifyKind, Toggle};
pub use mode::{Action, ShellModifiers, ShellSpec};
#[cfg(test)]
pub(crate) use script::loader::load_dynamic_config;
pub use style::{
    Hud, Notify, ResolvedStyle, STYLE_FILE_NAME, Selector, Style, StyleProvenance, StyleResolver,
    default_style, default_style_source,
};
pub use types::{FontWeight, Mode, NotifyPos, NotifyTheme, NotifyWindowStyle, Offset, Pos};

/// Parse color into raw rgb tuple.
pub(crate) fn parse_rgb(s: &str) -> Option<(u8, u8, u8)> {
    colornames::Color::try_from(s).ok().map(|c| c.rgb())
}

/// Determine the preferred user config path (`~/.hotki/config.luau`).
pub fn default_config_path() -> PathBuf {
    let mut p = PathBuf::from(env::var_os("HOME").unwrap_or_default());
    p.push(".hotki");
    p.push("config.luau");
    p
}

/// Resolve the effective config path using the default policy.
///
/// Policy:
/// 1) Use `explicit` when provided.
/// 2) Else use `~/.hotki/config.luau` when it exists.
/// 3) Else return a clear "no config found" error pointing to `examples/complete.luau`.
pub fn resolve_config_path(explicit: Option<&Path>) -> Result<PathBuf, Error> {
    if let Some(path) = explicit {
        return Ok(path.to_path_buf());
    }

    let preferred = default_config_path();
    if preferred.exists() {
        return Ok(preferred);
    }

    Err(Error::Read {
        path: Some(preferred),
        message:
            "No config found. Create ~/.hotki/config.luau (preferred) or copy examples/complete.luau"
                .to_string(),
    })
}
