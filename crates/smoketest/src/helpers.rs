use std::time::Duration;

use hotki_world::{Frames, RectDelta, RectPx, WaitConfig, WaitError};

use crate::{
    config,
    error::{Error, Result},
    suite::LOG_TARGET,
};

/// Standard wait configuration used for world-driven assertions.
#[must_use]
pub fn default_wait_config() -> WaitConfig {
    let overall = Duration::from_millis(config::DEFAULTS.timeout_ms);
    let idle = Duration::from_millis(config::INPUT_DELAYS.poll_interval_ms.max(5));
    WaitConfig::new(overall, idle, 512)
}

/// Map a world wait error into the smoketest error domain.
#[must_use]
pub fn wait_failure(case: &str, err: &WaitError) -> Error {
    Error::InvalidState(format!("case={case} wait_error={err}"))
}

/// Assert that the authoritative frame in `frames` matches `expected` within `eps` pixels.
///
/// Emits a single-line diagnostic with standardized formatting that includes raw AX/CG deltas when
/// diagnostic data is available.
pub fn assert_frame_matches(case: &str, expected: RectPx, frames: &Frames, eps: i32) -> Result<()> {
    let actual = frames.authoritative;
    let delta = expected.delta(&actual);
    if frame_within_eps(&delta, eps) {
        return Ok(());
    }

    let scale = frames.scale;
    let message = format!(
        "case=<{}> scale=<{:.2}> eps=<{}> expected={} got={} delta={}{}",
        case,
        scale,
        eps,
        format_rect(expected),
        format_rect(actual),
        format_delta(delta),
        frame_extras(frames, actual)
    );

    tracing::error!(
        target: LOG_TARGET,
        event = "frame_mismatch",
        case,
        scale = scale,
        eps,
        expected = %format_rect(expected),
        actual = %format_rect(actual),
        delta = %format_delta(delta),
        extras = %frame_extras(frames, actual),
        "frame mismatch"
    );

    Err(Error::InvalidState(message))
}

/// Return `true` when each component of a rectangle delta is within the supplied epsilon.
fn frame_within_eps(delta: &RectDelta, eps: i32) -> bool {
    delta.dx.abs() <= eps && delta.dy.abs() <= eps && delta.dw.abs() <= eps && delta.dh.abs() <= eps
}

/// Format a rectangle as `<x,y,w,h>` in integer pixels.
fn format_rect(rect: RectPx) -> String {
    format!("<{},{},{},{}>", rect.x, rect.y, rect.w, rect.h)
}

/// Format a rectangle delta as `<dx,dy,dw,dh>` for diagnostics.
fn format_delta(delta: RectDelta) -> String {
    format!("<{},{},{},{}>", delta.dx, delta.dy, delta.dw, delta.dh)
}

/// Format optional raw AX/CG deltas captured alongside the authoritative frame.
fn frame_extras(frames: &Frames, actual: RectPx) -> String {
    let mut extras = String::new();
    if let Some(ax) = frames.ax {
        let ax_delta = actual.delta(&ax);
        if !delta_is_zero(&ax_delta) {
            extras.push_str(" ax_delta=");
            extras.push_str(&format_delta(ax_delta));
        }
    }
    if let Some(cg) = frames.cg {
        let cg_delta = actual.delta(&cg);
        if !delta_is_zero(&cg_delta) {
            extras.push_str(" cg_delta=");
            extras.push_str(&format_delta(cg_delta));
        }
    }
    extras
}

/// Return true when a rectangle delta equals zero in all dimensions.
fn delta_is_zero(delta: &RectDelta) -> bool {
    delta.dx == 0 && delta.dy == 0 && delta.dw == 0 && delta.dh == 0
}
