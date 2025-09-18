//! Shared identifiers for world-managed windows.
#![warn(missing_docs)]
#![warn(unsafe_op_in_unsafe_fn)]

/// Identifiers for windows as observed and managed by `hotki-world`.
///
/// The identifier couples a process id and a Core Graphics window id. The
/// pairing ensures downstream crates always reference windows via world state
/// rather than inventing identifiers on the fly.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct WorldWindowId {
    /// Process identifier that owns the window.
    pid: i32,
    /// Core Graphics window identifier (`kCGWindowNumber`).
    window_id: u32,
}

impl WorldWindowId {
    /// Construct a new identifier using the owning process id and window id.
    #[must_use]
    pub const fn new(pid: i32, window_id: u32) -> Self {
        Self { pid, window_id }
    }

    /// Owning process id for this window.
    #[must_use]
    pub const fn pid(self) -> i32 {
        self.pid
    }

    /// Core Graphics window id (`kCGWindowNumber`).
    #[must_use]
    pub const fn window_id(self) -> u32 {
        self.window_id
    }
}

impl From<(i32, u32)> for WorldWindowId {
    fn from(value: (i32, u32)) -> Self {
        Self::new(value.0, value.1)
    }
}

impl From<WorldWindowId> for (i32, u32) {
    fn from(value: WorldWindowId) -> Self {
        (value.pid(), value.window_id())
    }
}
