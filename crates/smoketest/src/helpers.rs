use std::{path::Path, time::Duration};

use hotki_world::{Frames, RectDelta, RectPx, WaitConfig, WaitError};

use crate::{
    config,
    error::{Error, Result},
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
/// `test-introspection` data is available.
pub fn assert_frame_matches<P>(
    case: &str,
    expected: RectPx,
    frames: &Frames,
    eps: i32,
    artifacts: &[P],
) -> Result<()>
where
    P: AsRef<Path>,
{
    let actual = frames.authoritative;
    let delta = expected.delta(&actual);
    if frame_within_eps(&delta, eps) {
        return Ok(());
    }

    let scale = frames.scale;
    let message = format!(
        "case=<{}> scale=<{:.2}> eps=<{}> expected={} got={} delta={}{} artifacts={}",
        case,
        scale,
        eps,
        format_rect(expected),
        format_rect(actual),
        format_delta(delta),
        frame_extras(frames, actual),
        format_artifacts(artifacts)
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

/// Format artifact paths for inclusion in failure messages.
fn format_artifacts<P: AsRef<Path>>(artifacts: &[P]) -> String {
    if artifacts.is_empty() {
        "-".to_string()
    } else {
        artifacts
            .iter()
            .map(|p| p.as_ref().display().to_string())
            .collect::<Vec<_>>()
            .join(",")
    }
}

/// Format optional raw AX/CG deltas when the test-introspection feature is enabled.
fn frame_extras(frames: &Frames, actual: RectPx) -> String {
    #[cfg(feature = "test-introspection")]
    {
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
    #[cfg(not(feature = "test-introspection"))]
    {
        let _ = (frames, actual);
        String::new()
    }
}

#[cfg(feature = "test-introspection")]
/// Return true when a rectangle delta equals zero in all dimensions.
fn delta_is_zero(delta: &RectDelta) -> bool {
    delta.dx == 0 && delta.dy == 0 && delta.dw == 0 && delta.dh == 0
}
