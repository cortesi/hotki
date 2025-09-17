//! Record raw CoreGraphics window snapshots to inspect Mission Control spaces.

use std::{
    fs::File,
    io::{self, BufWriter, Write},
    path::Path,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use mac_winops::{
    Pos, SpaceId, WindowInfo, active_space_ids,
    ops::{RealWinOps, WinOps},
};
use serde::Serialize;
use tracing::info;

use crate::error::{Error, Result};

#[derive(Serialize)]
/// Captured window bounds for diagnostics.
struct ProbeRect {
    /// Left coordinate in screen points.
    x: i32,
    /// Bottom coordinate in screen points.
    y: i32,
    /// Width in screen points.
    width: i32,
    /// Height in screen points.
    height: i32,
}

impl From<Pos> for ProbeRect {
    fn from(pos: Pos) -> Self {
        Self {
            x: pos.x,
            y: pos.y,
            width: pos.width,
            height: pos.height,
        }
    }
}

#[derive(Serialize)]
/// Lightweight window snapshot emitted by the probe.
struct ProbeWindow {
    /// Application name (`kCGWindowOwnerName`).
    app: String,
    /// Window title.
    title: String,
    /// Owning process identifier.
    pid: i32,
    /// CoreGraphics window id.
    id: u32,
    /// CoreGraphics layer index.
    layer: i32,
    /// Reported Mission Control space id, when known.
    space: Option<SpaceId>,
    /// Whether the window is currently focused.
    focused: bool,
    /// Whether CoreGraphics considers the window onscreen.
    is_on_screen: bool,
    /// Whether the window belongs to an active Mission Control space.
    on_active_space: bool,
    /// Window bounds snapshot.
    pos: Option<ProbeRect>,
}

impl From<WindowInfo> for ProbeWindow {
    fn from(info: WindowInfo) -> Self {
        Self {
            app: info.app,
            title: info.title,
            pid: info.pid,
            id: info.id,
            layer: info.layer,
            space: info.space,
            focused: info.focused,
            is_on_screen: info.is_on_screen,
            on_active_space: info.on_active_space,
            pos: info.pos.map(ProbeRect::from),
        }
    }
}

#[derive(Serialize)]
/// Single capture record including all windows and active spaces.
struct ProbeRecord {
    /// Sequence number of the capture (0-based).
    seq: u32,
    /// Milliseconds since Unix epoch for the capture time.
    timestamp_ms: u64,
    /// Active Mission Control space identifiers.
    active_spaces: Vec<SpaceId>,
    /// Windows observed for the capture.
    windows: Vec<ProbeWindow>,
}

/// Run the space probe with the given sampling plan.
pub fn run(samples: u32, interval_ms: u64, output: Option<&Path>, quiet: bool) -> Result<()> {
    let writer: Box<dyn Write> = match output {
        Some(path) => Box::new(BufWriter::new(File::create(path)?)),
        None => Box::new(BufWriter::new(io::stdout())),
    };
    run_with_writer(samples, interval_ms, writer, quiet)
}

/// Internal helper that writes JSONL output for the probe.
fn run_with_writer(
    samples: u32,
    interval_ms: u64,
    mut writer: Box<dyn Write>,
    quiet: bool,
) -> Result<()> {
    let ops = RealWinOps;
    for seq in 0..samples {
        let ts_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or_default();
        let active = active_space_ids();
        let capture_start = Instant::now();
        let windows = ops
            .list_windows_for_spaces(&[])
            .into_iter()
            .map(ProbeWindow::from)
            .collect::<Vec<_>>();
        let capture_ms = capture_start.elapsed().as_secs_f64() * 1000.0;
        let record = ProbeRecord {
            seq,
            timestamp_ms: ts_ms,
            active_spaces: active.clone(),
            windows,
        };
        serde_json::to_writer(&mut writer, &record)
            .map_err(|e| Error::InvalidState(format!("failed to encode probe record: {e}")))?;
        writer.write_all(b"\n")?;
        writer.flush()?;
        if !quiet {
            info!(
                seq,
                timestamp_ms = ts_ms,
                active_spaces = ?active,
                window_count = record.windows.len(),
                capture_ms,
                "space_probe_sample"
            );
        }
        if seq + 1 < samples {
            thread::sleep(Duration::from_millis(interval_ms));
        }
    }
    Ok(())
}
