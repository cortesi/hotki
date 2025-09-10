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

/// Current permission status for the process.
#[derive(Debug, Clone, Copy)]
pub struct PermissionsStatus {
    /// Accessibility (AX) permission; `true` if granted.
    pub accessibility_ok: bool,
    /// Input Monitoring permission; `true` if granted.
    pub input_ok: bool,
    /// Screen Recording permission; `true` if granted.
    pub screen_recording_ok: bool,
}

/// Query both Accessibility and Input Monitoring permissions.
///
/// This is a convenience wrapper over [`accessibility_ok`] and
/// [`input_monitoring_ok`]. The function performs no prompting and has no
/// side effects.
pub fn check_permissions() -> PermissionsStatus {
    PermissionsStatus {
        accessibility_ok: accessibility_ok(),
        input_ok: input_monitoring_ok(),
        screen_recording_ok: screen_recording_ok(),
    }
}
