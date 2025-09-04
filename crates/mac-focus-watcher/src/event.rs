//! Event types emitted by the focus watcher.

/// A focus-related event emitted by the watcher.
///
/// Semantics:
/// - `AppChanged { title, pid }`: emitted when the foreground application
///   changes. `title` may be a localized app name or the bundle identifier
///   (when available). `pid` is the process identifier of the foreground app
///   (or -1 if unavailable).
/// - `TitleChanged { title, pid }`: emitted when the focused window's title
///   changes. `title` is the new title string (may be empty if unavailable).
///   `pid` is the PID of the owning app (or -1 if unavailable).
#[derive(Debug, Clone)]
pub enum FocusEvent {
    /// The foreground application's name or bundle identifier changed.
    AppChanged { title: String, pid: i32 },
    /// The focused window's title changed.
    TitleChanged { title: String, pid: i32 },
}
