use std::sync::Arc;

use crate::Result;

/// Capture-all guard returned by the hotkey API.
pub(crate) enum CaptureGuard {
    /// Real OS-backed capture guard.
    Real {
        /// Guard retained solely for its drop behavior.
        _guard: mac_hotkey::CaptureGuard,
    },
    /// No-op guard used by tests.
    Fake,
}

/// Minimal hotkey API used by the engine's binding manager.
pub(crate) trait HotkeyApi: Send + Sync {
    fn intercept(&self, chord: mac_keycode::Chord) -> u32;
    fn unregister(&self, id: u32) -> Result<()>;
    fn capture_all(&self) -> CaptureGuard;
    fn is_fake(&self) -> bool {
        false
    }
}

pub(crate) struct RealHotkeyApi {
    inner: Arc<mac_hotkey::Manager>,
}

impl RealHotkeyApi {
    pub(crate) fn new(inner: Arc<mac_hotkey::Manager>) -> Self {
        Self { inner }
    }
}

impl HotkeyApi for RealHotkeyApi {
    fn intercept(&self, chord: mac_keycode::Chord) -> u32 {
        self.inner.intercept(chord)
    }
    fn unregister(&self, id: u32) -> Result<()> {
        Ok(self.inner.unregister(id)?)
    }
    fn capture_all(&self) -> CaptureGuard {
        CaptureGuard::Real {
            _guard: self.inner.capture_all(),
        }
    }
    fn is_fake(&self) -> bool {
        false
    }
}

/// Mock API for tests that avoids OS interaction.
pub(crate) struct MockHotkeyApi {
    next_id: std::sync::atomic::AtomicU32,
}

impl Default for MockHotkeyApi {
    fn default() -> Self {
        Self::new()
    }
}

impl MockHotkeyApi {
    /// Create a new mock hotkey API.
    pub fn new() -> Self {
        Self {
            next_id: std::sync::atomic::AtomicU32::new(1000),
        }
    }
}

impl HotkeyApi for MockHotkeyApi {
    fn intercept(&self, _chord: mac_keycode::Chord) -> u32 {
        self.next_id
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            + 1
    }
    fn unregister(&self, _id: u32) -> Result<()> {
        Ok(())
    }
    fn capture_all(&self) -> CaptureGuard {
        CaptureGuard::Fake
    }
    fn is_fake(&self) -> bool {
        true
    }
}

// (Window operations handled externally; no in-crate bindings remain.)
