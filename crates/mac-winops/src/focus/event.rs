//! Event types emitted by the focus watcher.

/// A focus-related event emitted by the watcher.
#[derive(Debug, Clone)]
pub enum FocusEvent {
    /// The foreground application's name or bundle identifier changed.
    AppChanged { title: String, pid: i32 },
    /// The focused window's title changed.
    TitleChanged { title: String, pid: i32 },
}
