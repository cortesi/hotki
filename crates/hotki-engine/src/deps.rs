use std::sync::Arc;

use crate::Result;

// ---- Hotkey API abstraction ----

pub trait CaptureToken: Send {}

impl CaptureToken for mac_hotkey::CaptureGuard {}

/// Minimal hotkey API used by the engine's binding manager.
pub trait HotkeyApi: Send + Sync {
    fn intercept(&self, chord: mac_keycode::Chord) -> u32;
    fn unregister(&self, id: u32) -> Result<()>;
    fn capture_all(&self) -> Box<dyn CaptureToken>;
    fn is_fake(&self) -> bool {
        false
    }
}

pub struct RealHotkeyApi {
    inner: Arc<mac_hotkey::Manager>,
}

impl RealHotkeyApi {
    pub fn new(inner: Arc<mac_hotkey::Manager>) -> Self {
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
    fn capture_all(&self) -> Box<dyn CaptureToken> {
        Box::new(self.inner.capture_all())
    }
    fn is_fake(&self) -> bool {
        false
    }
}

/// No-op capture token used by tests to satisfy the `CaptureToken` trait.
pub struct NoopCaptureToken;
impl CaptureToken for NoopCaptureToken {}

/// Mock API for tests that avoids OS interaction.
pub struct MockHotkeyApi {
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
    fn capture_all(&self) -> Box<dyn CaptureToken> {
        Box::new(NoopCaptureToken)
    }
    fn is_fake(&self) -> bool {
        true
    }
}

// (Window operations handled externally; no in-crate bindings remain.)
