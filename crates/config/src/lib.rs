//! Shared configuration types (modes, themes, parsing) used by Hotki.
#![allow(missing_docs)]
#![warn(unsafe_op_in_unsafe_fn)]

use std::{
    env,
    path::{Path, PathBuf},
};

mod defaults;
mod error;
pub mod dynamic;
mod keys;
mod loader;
mod mode;
mod notify;
mod raw;
mod rhai;
mod style;
pub mod themes;
mod types;

#[cfg(test)]
mod test_merge;
#[cfg(test)]
mod test_parse;
#[cfg(test)]
mod test_rhai;

pub use error::Error;
pub use hotki_protocol::{Cursor, Toggle};
pub use keys::{Config, CursorEnsureExt};
pub use loader::{LoadedConfig, load_for_server_from_path, load_from_path};
pub use mode::{Action, Keys, KeysAttrs, NotifyKind, ShellModifiers, ShellSpec};
pub use notify::Notify;
pub use rhai::RhaiRuntime;
pub use style::{Hud, Style};
pub use types::{FontWeight, Mode, NotifyPos, NotifyTheme, NotifyWindowStyle, Offset, Pos};

/// Parse color into raw rgb tuple.
pub(crate) fn parse_rgb(s: &str) -> Option<(u8, u8, u8)> {
    colornames::Color::try_from(s).ok().map(|c| c.rgb())
}

/// Determine the preferred user config path (`~/.hotki/config.rhai`).
pub fn default_config_path() -> PathBuf {
    let mut p = PathBuf::from(env::var_os("HOME").unwrap_or_default());
    p.push(".hotki");
    p.push("config.rhai");
    p
}

/// Resolve the effective config path using the default policy.
///
/// Policy:
/// 1) Use `explicit` when provided.
/// 2) Else use `~/.hotki/config.rhai` when it exists.
/// 3) Else return a clear "no config found" error pointing to `examples/complete.rhai`.
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
            "No config found. Create ~/.hotki/config.rhai (preferred) or copy examples/complete.rhai"
                .to_string(),
    })
}
