//! Shared configuration types (modes, themes, parsing) used by Hotki.
#![allow(missing_docs)]
#![warn(unsafe_op_in_unsafe_fn)]

use std::{env, path::PathBuf};

mod defaults;
mod error;
mod keys;
mod loader;
mod mode;
mod notify;
mod raw;
mod style;
pub mod themes;
mod types;

#[cfg(test)]
mod test_merge;
#[cfg(test)]
mod test_parse;

pub use error::Error;
pub use hotki_protocol::{Cursor, Toggle};
pub(crate) use keys::ConfigInput;
pub use keys::{Config, CursorEnsureExt, ServerTunables};
pub use loader::{load_from_path, load_from_str};
pub use mode::{Action, Keys, KeysAttrs, NotifyKind, ShellModifiers, ShellSpec};
pub use notify::Notify;
pub use style::{Hud, Style};
pub use types::{FontWeight, Mode, NotifyPos, NotifyTheme, NotifyWindowStyle, Offset, Pos};

/// Parse color into raw rgb tuple.
pub(crate) fn parse_rgb(s: &str) -> Option<(u8, u8, u8)> {
    colornames::Color::try_from(s).ok().map(|c| c.rgb())
}

/// Determine the default user config path (~/.hotki.ron).
pub fn default_config_path() -> PathBuf {
    let mut p = PathBuf::from(env::var_os("HOME").unwrap_or_default());
    p.push(".hotki.ron");
    p
}
