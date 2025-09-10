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
}

// (WindowOps moved to mac-winops::ops)
