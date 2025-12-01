//! Keymode: interpret chords against a nested key mode configuration.

/// State machine and transition logic for keymode.
mod state;

pub use state::{KeyResponse, KeymodeError, ShellRepeatConfig, State};
