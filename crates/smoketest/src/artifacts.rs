use std::{
    collections::HashMap,
    env,
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use hotki_world::{EventRecord, RectPx, WorldEvent, WorldHandle, WorldWindow};
use image::{ImageBuffer, Rgba};
use serde_json::json;

use crate::{
    error::{Error, Result},
    runtime,
    suite::{Budget, StageDurationsOptional},
};

#[cfg_attr(not(test), allow(dead_code))]
/// Padding applied around rendered overlay rectangles.
const OVERLAY_PADDING: i32 = 16;

/// Capture diagnostic artifacts for a failing smoketest case.
#[cfg_attr(not(test), allow(dead_code))]
pub fn capture_failure_artifacts(
    world: &WorldHandle,
    case: &str,
    expected: Option<RectPx>,
    actual: Option<RectPx>,
    output_dir: &Path,
) -> Result<Vec<PathBuf>> {
    fs::create_dir_all(output_dir)?;

    let snapshot = runtime::block_on({
        let world = world.clone();
        async move { world.snapshot().await }
    })?;
    let frames = runtime::block_on({
        let world = world.clone();
        async move { world.frames_snapshot().await }
    })?;
    let events = world.recent_events(50);
    let commit = repo_commit().unwrap_or_else(|| "unknown".to_string());
    let generated_at = SystemTime::now();

    let mut artifacts = Vec::new();

    let world_path = output_dir.join(format!("{case}.world.txt"));
    write_world_file(
        &world_path,
        &snapshot,
        &frames,
        &commit,
        generated_at,
        expected,
        actual,
    )?;
    artifacts.push(world_path);

    let frames_path = output_dir.join(format!("{case}.frames.txt"));
    write_frames_file(&frames_path, &frames)?;
    artifacts.push(frames_path);

    let events_path = output_dir.join(format!("{case}.events.txt"));
    write_events_file(&events_path, &events, &frames)?;
    artifacts.push(events_path);

    if let Some(path) = write_overlay_png(output_dir, case, expected, actual)? {
        artifacts.push(path);
    }

    Ok(artifacts)
}

/// Write configured/actual budget metadata for a case and return the emitted path.
pub fn write_budget_report(
    case: &str,
    budget: &Budget,
    actual: &StageDurationsOptional,
    output_dir: &Path,
) -> Result<PathBuf> {
    fs::create_dir_all(output_dir)?;
    let path = output_dir.join(format!("{case}.budget.json"));
    let payload = json!({
        "case": case,
        "configured": {
            "setup_ms": budget.setup_ms,
            "action_ms": budget.action_ms,
            "settle_ms": budget.settle_ms,
        },
        "actual": actual,
    });
    let mut file = File::create(&path)?;
    serde_json::to_writer_pretty(&mut file, &payload)
        .map_err(|e| Error::InvalidState(format!("failed to serialize budget: {}", e)))?;
    file.write_all(b"\n")?;
    Ok(path)
}

/// Write the world snapshot metadata to disk.
fn write_world_file(
    path: &Path,
    snapshot: &[WorldWindow],
    frames: &HashMap<hotki_world::WindowKey, hotki_world::Frames>,
    commit: &str,
    generated_at: SystemTime,
    expected: Option<RectPx>,
    actual: Option<RectPx>,
) -> Result<()> {
    let mut file = File::create(path)?;
    writeln!(file, "world_commit: {}", commit)?;
    let ts_ms = generated_at
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::from_secs(0))
        .as_millis();
    writeln!(file, "generated_at_ms: {}", ts_ms)?;
    if let Some(rect) = expected {
        writeln!(file, "expected_rect: {}", fmt_rect(rect))?;
    }
    if let Some(rect) = actual {
        writeln!(file, "actual_rect: {}", fmt_rect(rect))?;
    }
    writeln!(file, "windows:")?;
    let mut windows = snapshot.to_vec();
    windows.sort_by_key(|w| (w.pid, w.id));
    for window in windows {
        writeln!(
            file,
            "  - pid={} id={} app={} title={} layer={} focused={} display_id={:?} space={:?} on_active_space={} is_on_screen={}",
            window.pid,
            window.id,
            window.app,
            window.title,
            window.layer,
            window.focused,
            window.display_id,
            window.space,
            window.on_active_space,
            window.is_on_screen
        )?;
        if let Some(frame) = frames.get(&hotki_world::WindowKey {
            pid: window.pid,
            id: window.id,
        }) {
            writeln!(
                file,
                "    authoritative={} mode={:?} display_id={:?} space_id={:?} scale={:.2}",
                fmt_rect(frame.authoritative),
                frame.mode,
                frame.display_id,
                frame.space_id,
                frame.scale
            )?;
        }
    }
    Ok(())
}

/// Write the frame metadata snapshot to disk.
fn write_frames_file(
    path: &Path,
    frames: &HashMap<hotki_world::WindowKey, hotki_world::Frames>,
) -> Result<()> {
    let mut entries: Vec<_> = frames.iter().collect();
    entries.sort_by_key(|(k, _)| (k.pid, k.id));
    let mut file = File::create(path)?;
    for (key, frame) in entries {
        writeln!(
            file,
            "pid={} id={} authoritative={} mode={:?} display_id={:?} space_id={:?} scale={:.2}",
            key.pid,
            key.id,
            fmt_rect(frame.authoritative),
            frame.mode,
            frame.display_id,
            frame.space_id,
            frame.scale
        )?;
    }
    Ok(())
}

/// Persist recent world events for diagnostics.
fn write_events_file(
    path: &Path,
    events: &[EventRecord],
    frames: &HashMap<hotki_world::WindowKey, hotki_world::Frames>,
) -> Result<()> {
    let mut file = File::create(path)?;
    for record in events {
        let ts_ms = record
            .timestamp
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::from_secs(0))
            .as_millis();
        writeln!(
            file,
            "seq={} ts_ms={} {}",
            record.seq,
            ts_ms,
            describe_event(&record.event, frames)
        )?;
    }
    Ok(())
}

/// Render a human-readable description of an event, including frame context when available.
fn describe_event(
    event: &WorldEvent,
    frames: &HashMap<hotki_world::WindowKey, hotki_world::Frames>,
) -> String {
    match event {
        WorldEvent::Added(window) => format!(
            "Added pid={} id={} app={} title={} display_id={:?} space={:?}",
            window.pid, window.id, window.app, window.title, window.display_id, window.space
        ),
        WorldEvent::Removed(key) => {
            let extra = frames
                .get(key)
                .map(|f| {
                    format!(
                        " display_id={:?} space_id={:?} scale={:.2}",
                        f.display_id, f.space_id, f.scale
                    )
                })
                .unwrap_or_default();
            format!("Removed pid={} id={}{}", key.pid, key.id, extra)
        }
        WorldEvent::Updated(key, delta) => {
            let extra = frames
                .get(key)
                .map(|f| {
                    format!(
                        " display_id={:?} space_id={:?} scale={:.2}",
                        f.display_id, f.space_id, f.scale
                    )
                })
                .unwrap_or_default();
            format!(
                "Updated pid={} id={} delta={:?}{}",
                key.pid, key.id, delta, extra
            )
        }
        WorldEvent::MetaAdded(key, _) | WorldEvent::MetaRemoved(key, _) => {
            format!("MetaChange pid={} id={}", key.pid, key.id)
        }
        WorldEvent::FocusChanged(change) => format!(
            "FocusChanged key={:?} app={:?} title={:?} pid={:?}",
            change.key, change.app, change.title, change.pid
        ),
    }
}

/// Generate an overlay image showing expected vs. actual rectangles.
fn write_overlay_png(
    output_dir: &Path,
    case: &str,
    expected: Option<RectPx>,
    actual: Option<RectPx>,
) -> Result<Option<PathBuf>> {
    let mut rects: Vec<(RectPx, Rgba<u8>)> = Vec::new();
    if let Some(rect) = expected {
        rects.push((rect, Rgba([0, 200, 0, 255])));
    }
    if let Some(rect) = actual {
        rects.push((rect, Rgba([220, 32, 32, 255])));
    }
    if rects.is_empty() {
        return Ok(None);
    }

    let min_x = rects.iter().map(|(r, _)| r.x).min().unwrap_or(0);
    let min_y = rects.iter().map(|(r, _)| r.y).min().unwrap_or(0);
    let max_x = rects.iter().map(|(r, _)| r.x + r.w).max().unwrap_or(1);
    let max_y = rects.iter().map(|(r, _)| r.y + r.h).max().unwrap_or(1);

    let width = ((max_x - min_x) + OVERLAY_PADDING * 2).max(1) as u32;
    let height = ((max_y - min_y) + OVERLAY_PADDING * 2).max(1) as u32;

    let mut img =
        ImageBuffer::<Rgba<u8>, Vec<u8>>::from_pixel(width, height, Rgba([32, 32, 32, 255]));

    for (rect, color) in rects {
        draw_rect(&mut img, rect, min_x, min_y, color);
    }

    let path = output_dir.join(format!("{case}.overlay.png"));
    img.save(&path)
        .map_err(|e| Error::InvalidState(format!("failed to save overlay png: {e}")))?;
    Ok(Some(path))
}

/// Draw a rectangular outline in the output overlay image.
fn draw_rect(
    img: &mut ImageBuffer<Rgba<u8>, Vec<u8>>,
    rect: RectPx,
    min_x: i32,
    min_y: i32,
    color: Rgba<u8>,
) {
    let width = img.width() as i32;
    let height = img.height() as i32;
    let x0 = rect.x - min_x + OVERLAY_PADDING;
    let y0 = rect.y - min_y + OVERLAY_PADDING;
    let x1 = x0 + rect.w;
    let y1 = y0 + rect.h;

    for x in x0..=x1 {
        if x >= 0 && x < width {
            if y0 >= 0 && y0 < height {
                img.put_pixel(x as u32, y0 as u32, color);
            }
            if y1 >= 0 && y1 < height {
                img.put_pixel(x as u32, y1 as u32, color);
            }
        }
    }
    for y in y0..=y1 {
        if y >= 0 && y < height {
            if x0 >= 0 && x0 < width {
                img.put_pixel(x0 as u32, y as u32, color);
            }
            if x1 >= 0 && x1 < width {
                img.put_pixel(x1 as u32, y as u32, color);
            }
        }
    }
}

/// Format a rectangle as `<x,y,w,h>` string for logs.
fn fmt_rect(rect: RectPx) -> String {
    format!("<{},{},{},{}>", rect.x, rect.y, rect.w, rect.h)
}

/// Resolve the git commit for the workspace if available.
fn repo_commit() -> Option<String> {
    use std::process::Command;

    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let repo_root = manifest_dir.ancestors().nth(2)?;
    let output = Command::new("git")
        .arg("rev-parse")
        .arg("HEAD")
        .current_dir(repo_root)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let sha = String::from_utf8(output.stdout).ok()?;
    Some(sha.trim().to_string())
}

#[cfg(test)]
mod tests {
    use hotki_world::World;

    use super::*;

    #[test]
    fn capture_artifacts_emits_files() {
        let world = {
            let rt = runtime::shared_runtime().expect("shared runtime");
            let runtime = rt.lock();
            let _guard = runtime.enter();
            World::spawn_noop()
        };
        let expected = RectPx {
            x: 10,
            y: 20,
            w: 100,
            h: 80,
        };
        let actual = RectPx {
            x: 25,
            y: 35,
            w: 110,
            h: 90,
        };

        let temp_dir = env::temp_dir().join(format!(
            "hotki_artifacts_test_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis()
        ));
        fs::create_dir_all(&temp_dir).unwrap();

        let artifacts = capture_failure_artifacts(
            &world,
            "artifact_case",
            Some(expected),
            Some(actual),
            &temp_dir,
        )
        .unwrap();

        assert!(
            artifacts
                .iter()
                .any(|p| p.ends_with("artifact_case.world.txt"))
        );
        assert!(
            artifacts
                .iter()
                .any(|p| p.ends_with("artifact_case.frames.txt"))
        );
        assert!(
            artifacts
                .iter()
                .any(|p| p.ends_with("artifact_case.events.txt"))
        );
        let overlay = temp_dir.join("artifact_case.overlay.png");
        assert!(overlay.exists());
        let meta = fs::metadata(&overlay).unwrap();
        assert!(meta.len() > 0);

        let _ = fs::remove_dir_all(&temp_dir).ok();
    }
}
