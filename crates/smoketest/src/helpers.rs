use std::{
    path::Path,
    time::{Duration, Instant},
};

use hotki_world::{EventCursor, Frames, RectDelta, RectPx, WorldHandle, mimic::pump_active_mimics};

use crate::error::{Error, Result};

/// Duration in milliseconds for each runloop pump step while waiting on events.
const PUMP_STEP_MS: u64 = 5;

/// Wait until `confirm` returns true or `timeout` elapses, pumping the main thread and draining
/// events while ensuring world event ordering remains intact.
pub fn wait_for_events_or<F>(
    case: &str,
    world: &WorldHandle,
    cursor: &mut EventCursor,
    timeout: Duration,
    mut confirm: F,
) -> Result<()>
where
    F: FnMut() -> Result<bool>,
{
    let deadline = Instant::now() + timeout;
    let baseline_lost = cursor.lost_count;
    loop {
        pump_active_mimics();
        if confirm()? {
            ensure_no_event_loss(cursor, baseline_lost)?;
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(Error::InvalidState(format!(
                "timeout waiting for {case} (lost_count={} next_index={})",
                cursor.lost_count, cursor.next_index
            )));
        }
        let pump_until = Instant::now() + Duration::from_millis(PUMP_STEP_MS);
        world.pump_main_until(pump_until);
        pump_active_mimics();
        while world.next_event_now(cursor).is_some() {
            ensure_no_event_loss(cursor, baseline_lost)?;
            pump_active_mimics();
            if confirm()? {
                ensure_no_event_loss(cursor, baseline_lost)?;
                return Ok(());
            }
        }
        pump_active_mimics();
        ensure_no_event_loss(cursor, baseline_lost)?;
    }
}

/// Return an error if the subscription lost events while the wait condition was evaluated.
fn ensure_no_event_loss(cursor: &EventCursor, baseline: u64) -> Result<()> {
    if cursor.lost_count > baseline {
        return Err(Error::InvalidState(format!(
            "events lost during wait (lost_count={}): see artifacts",
            cursor.lost_count
        )));
    }
    Ok(())
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
