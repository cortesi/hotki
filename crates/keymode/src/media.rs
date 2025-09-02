/// Set system volume to an absolute value (0-100)
pub fn set_volume(level: u8) -> String {
    let level = level.min(100);
    format!("set volume output volume {}", level)
}

/// Change system volume by a relative amount
pub fn change_volume(delta: i8) -> String {
    format!(
        "set currentVolume to output volume of (get volume settings)\nset volume output volume (currentVolume + {})",
        delta
    )
}

/// Mute system audio
pub fn mute() -> String {
    "set volume output muted true".to_string()
}

/// Unmute system audio
pub fn unmute() -> String {
    "set volume output muted false".to_string()
}

/// Toggle mute state
pub fn toggle_mute() -> String {
    "set curMuted to output muted of (get volume settings)\nset volume output muted not curMuted"
        .to_string()
}

// Only string builders below; execution is handled by the engine.
