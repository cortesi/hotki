//! Keymode: interpret chords against a nested key mode configuration.
#![warn(missing_docs)]
#![warn(unsafe_op_in_unsafe_fn)]

/// Error types used by the keymode state machine.
mod error;
/// State machine and transition logic for keymode.
mod state;

pub use config::{Action, Keys, KeysAttrs, NotificationType, ShellModifiers, ShellSpec};
pub use error::KeymodeError;
pub use state::{KeyResponse, ShellRepeatConfig, State};
