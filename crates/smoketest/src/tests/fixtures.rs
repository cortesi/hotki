//! Shared smoketest fixtures and helpers.

use std::{
    path::Path,
    result::Result as StdResult,
    thread,
    time::{Duration, Instant},
};

pub use mac_winops::{Rect, WindowId};
use mac_winops::{approx_eq_eps, cell_rect as win_cell_rect, screen, wait};

use crate::{
    config,
    error::{Error, Result},
    helper_window::{self, FRONTMOST_IGNORE_TITLES},
    server_drive,
};

/// Approximate float equality within `eps` tolerance.
#[inline]
pub fn approx(a: f64, b: f64, eps: f64) -> bool {
    approx_eq_eps(a, b, eps)
}

/// Wait until all `(pid, title)` pairs are visible via CG or AX.
#[inline]
pub fn wait_for_windows_visible(entries: &[(i32, &str)], timeout_ms: u64) -> bool {
    wait::wait_for_windows_visible(
        entries,
        config::ms(timeout_ms),
        config::ms(config::INPUT_DELAYS.poll_interval_ms),
    )
}

/// Wait until the backend focus reporter sees `expected_title`.
#[inline]
pub fn wait_for_backend_focused_title(expected_title: &str, timeout_ms: u64) -> Result<()> {
    server_drive::wait_for_focused_title(expected_title, timeout_ms)?;
    Ok(())
}

/// Resolve the visible frame containing the current AX position of `(pid, title)`.
#[inline]
pub fn resolve_vf_for_window(pid: i32, title: &str, timeout_ms: u64, poll_ms: u64) -> Option<Rect> {
    wait::resolve_vf_for_window(
        pid,
        title,
        Duration::from_millis(timeout_ms),
        Duration::from_millis(poll_ms),
    )
}

/// Find the CoreGraphics window id for `(pid, title)` within `timeout_ms`.
#[inline]
pub fn find_window_id(pid: i32, title: &str, timeout_ms: u64, poll_ms: u64) -> Option<WindowId> {
    wait::find_window_id(
        pid,
        title,
        Duration::from_millis(timeout_ms),
        Duration::from_millis(poll_ms),
    )
}

/// Compute the exact grid cell rectangle within a given visible frame.
#[inline]
pub fn cell_rect(vf: Rect, cols: u32, rows: u32, col: u32, row: u32) -> Rect {
    win_cell_rect(vf, cols, rows, col, row)
}

/// Wait until `(pid,title)` reports an AX frame approximately equal to `expected`.
pub fn wait_for_expected_frame(
    pid: i32,
    title: &str,
    expected: Rect,
    eps: f64,
    timeout_ms: u64,
    poll_ms: u64,
) -> StdResult<(), FrameMismatch> {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    let mut last_actual = None;
    while Instant::now() < deadline {
        if let Some(((px, py), (w, h))) = mac_winops::ax_window_frame(pid, title) {
            let actual = Rect::new(px, py, w, h);
            if actual.approx_eq(&expected, eps) {
                return Ok(());
            }
            last_actual = Some(actual);
        }
        thread::sleep(Duration::from_millis(poll_ms));
    }
    Err(FrameMismatch::new(expected, last_actual, eps))
}

/// Resolve the visible frame for the screen containing `(x, y)`.
#[inline]
pub fn visible_frame_containing_point(x: f64, y: f64) -> Option<Rect> {
    screen::visible_frame_containing_point(x, y)
}

/// Assert that the frontmost window matches `expected_title` and occupies the grid cell.
pub fn assert_frontmost_cell(
    expected_title: &str,
    vf: Rect,
    cols: u32,
    rows: u32,
    col: u32,
    row: u32,
    eps: f64,
) -> Result<()> {
    let front = helper_window::frontmost_app_window(FRONTMOST_IGNORE_TITLES)
        .ok_or_else(|| Error::InvalidState("No frontmost app window".into()))?;
    if front.title != expected_title {
        return Err(Error::FocusNotObserved {
            timeout_ms: 1000,
            expected: format!("{} (frontmost: {})", expected_title, front.title),
        });
    }
    let ((x, y), (w, h)) = mac_winops::ax_window_frame(front.pid, &front.title)
        .ok_or_else(|| Error::InvalidState("AX frame for frontmost not available".into()))?;
    let expected = win_cell_rect(vf, cols, rows, col, row);
    let actual = Rect::new(x, y, w, h);
    if !actual.approx_eq(&expected, eps) {
        let case = format!(
            "frontmost_cell[title={},col={},row={}]",
            expected_title, col, row
        );
        let mismatch = FrameMismatch::new(expected, Some(actual), eps);
        return Err(Error::InvalidState(
            mismatch.failure_line::<&str>(&case, &[]),
        ));
    }
    Ok(())
}

/// Structured comparison data for mismatched frames.
#[derive(Clone, Copy, Debug)]
pub struct FrameMismatch {
    /// Expected rectangle used for comparison.
    expected: Rect,
    /// Last observed rectangle (if any) that failed the comparison.
    actual: Option<Rect>,
    /// Pixel tolerance applied during the comparison.
    eps: f64,
}

impl FrameMismatch {
    /// Construct a new mismatch record.
    pub fn new(expected: Rect, actual: Option<Rect>, eps: f64) -> Self {
        Self {
            expected,
            actual,
            eps,
        }
    }

    /// Render the canonical failure line for this mismatch.
    pub fn failure_line<P: AsRef<Path>>(&self, case: &str, artifacts: &[P]) -> String {
        let scale = self.scale_factor();
        let expected = format_rect(self.expected);
        let (got, delta) = match self.actual {
            Some(actual) => (format_rect(actual), format_delta(actual, self.expected)),
            None => (
                "<n/a,n/a,n/a,n/a>".to_string(),
                "<n/a,n/a,n/a,n/a>".to_string(),
            ),
        };
        let artifacts = format_artifacts(artifacts);

        format!(
            "case=<{}> scale=<{:.2}> eps=<{:.2}> expected={} got={} delta={} artifacts={}",
            case, scale, self.eps, expected, got, delta, artifacts
        )
    }

    /// Resolve the backing scale factor for the mismatched frame.
    fn scale_factor(&self) -> f64 {
        let anchor = self.actual.unwrap_or(self.expected);
        let center = anchor.center();
        screen::display_scale_containing_point(center.x, center.y).unwrap_or(1.0)
    }
}

/// Convenience wrapper to render the canonical failure line.
pub fn frame_failure_line<P: AsRef<Path>>(
    case: &str,
    expected: Rect,
    actual: Option<Rect>,
    eps: f64,
    artifacts: &[P],
) -> String {
    FrameMismatch::new(expected, actual, eps).failure_line(case, artifacts)
}

/// Format a rectangle as `<x,y,w,h>` with single decimal precision.
fn format_rect(rect: Rect) -> String {
    format!("<{:.1},{:.1},{:.1},{:.1}>", rect.x, rect.y, rect.w, rect.h)
}

/// Format the delta between actual and expected rectangles.
fn format_delta(actual: Rect, expected: Rect) -> String {
    let dx = actual.x - expected.x;
    let dy = actual.y - expected.y;
    let dw = actual.w - expected.w;
    let dh = actual.h - expected.h;
    format!("<{:.1},{:.1},{:.1},{:.1}>", dx, dy, dw, dh)
}

/// Render artifact paths in the canonical `<path1,path2>` list form.
fn format_artifacts<P: AsRef<Path>>(artifacts: &[P]) -> String {
    if artifacts.is_empty() {
        return "<[]>".to_string();
    }
    let entries: Vec<String> = artifacts
        .iter()
        .map(|p| p.as_ref().display().to_string())
        .collect();
    format!("<{}>", entries.join(","))
}
