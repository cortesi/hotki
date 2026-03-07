//! Simple, macOS-only permission checks for Hotki.
//!
//! This crate exposes a minimal, stable API to query whether the process has
//! the required Accessibility and Input Monitoring permissions. It calls into
//! the respective macOS frameworks and returns booleans that the UI can act on.
//! There is no prompting logic here: the host is responsible for guiding the
//! user to System Settings if permissions are missing.
//!
//! Notes
//! - `accessibility_ok()` checks the global Accessibility permission.
//! - `input_monitoring_ok()` checks Input Monitoring (listening to keyboard).
//! - `check_permissions()` returns both as a simple status struct.
//!
//! All calls are fast and side‑effect free.
use serde::{Deserialize, Serialize};

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> bool;
    fn CGPreflightListenEventAccess() -> bool;
    fn CGPreflightScreenCaptureAccess() -> bool;
}

pub fn accessibility_ok() -> bool {
    unsafe { AXIsProcessTrusted() }
}

/// Check if the application has the "Input Monitoring" permission.
///
/// Returns `true` when the process is allowed to listen for keyboard events
/// (CGEvent tap), and `false` otherwise.
pub fn input_monitoring_ok() -> bool {
    unsafe { CGPreflightListenEventAccess() }
}

/// Check if the application has the "Screen Recording" permission.
///
/// Returns `true` when the process is allowed to access screen content via
/// CoreGraphics APIs that require Screen Recording permission (e.g., window
/// titles in `CGWindowListCopyWindowInfo`), and `false` otherwise.
pub fn screen_recording_ok() -> bool {
    unsafe { CGPreflightScreenCaptureAccess() }
}

/// Permission state for a specific macOS capability.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PermissionState {
    /// Permission is granted.
    Granted,
    /// Permission is denied.
    Denied,
    /// Permission state has not been queried yet.
    #[default]
    Unknown,
}

impl PermissionState {
    /// Return whether this permission state is granted.
    #[must_use]
    pub fn is_granted(self) -> bool {
        matches!(self, Self::Granted)
    }
}

impl From<bool> for PermissionState {
    fn from(value: bool) -> Self {
        if value { Self::Granted } else { Self::Denied }
    }
}

/// Current permission status for the process.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionsStatus {
    /// Accessibility (AX) permission.
    pub accessibility: PermissionState,
    /// Input Monitoring permission.
    pub input_monitoring: PermissionState,
    /// Screen Recording permission.
    pub screen_recording: PermissionState,
}

impl PermissionsStatus {
    /// Return whether Accessibility permission is granted.
    #[must_use]
    pub fn accessibility_ok(self) -> bool {
        self.accessibility.is_granted()
    }

    /// Return whether Input Monitoring permission is granted.
    #[must_use]
    pub fn input_ok(self) -> bool {
        self.input_monitoring.is_granted()
    }

    /// Return whether Screen Recording permission is granted.
    #[must_use]
    pub fn screen_recording_ok(self) -> bool {
        self.screen_recording.is_granted()
    }
}

/// Query both Accessibility and Input Monitoring permissions.
///
/// This is a convenience wrapper over [`accessibility_ok`] and
/// [`input_monitoring_ok`]. The function performs no prompting and has no
/// side effects.
pub fn check_permissions() -> PermissionsStatus {
    PermissionsStatus {
        accessibility: accessibility_ok().into(),
        input_monitoring: input_monitoring_ok().into(),
        screen_recording: screen_recording_ok().into(),
    }
}
