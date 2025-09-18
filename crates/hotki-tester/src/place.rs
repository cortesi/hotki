//! Implementation for the `place` diagnostic subcommand.

use std::{
    convert::TryFrom,
    env,
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

use config::{Action, At, AtSpec, Dir, Grid, GridSpec};
use hotki_world::{MoveDirection, World, WorldCfg};
use hotki_world_ids::WorldWindowId;
use humantime::format_duration;
use mac_winops::ops::{RealWinOps, WinOps};
use mac_winops::{
    self, WindowId, ax_focused_window_id_for_pid, ax_props_for_window_id, ax_window_frame,
    placement_counters_reset, placement_counters_snapshot,
};
use ron::{Options, extensions::Extensions};
use tokio::runtime::Runtime;
use tracing::{debug, info, warn};

use crate::{
    backend::BackendProcess,
    cli::PlaceArgs,
    diagnostics::{self, WindowSnapshot},
    error::{Error, Result},
};

/// Normalized placement directives accepted by the CLI.
#[derive(Debug, Clone, Copy)]
enum Directive {
    /// Absolute placement within a grid cell.
    Place {
        /// Number of columns in the placement grid.
        cols: u32,
        /// Number of rows in the placement grid.
        rows: u32,
        /// Target column index (0-based).
        col: u32,
        /// Target row index (0-based).
        row: u32,
    },
    /// Relative movement within a grid.
    PlaceMove {
        /// Number of columns in the placement grid.
        cols: u32,
        /// Number of rows in the placement grid.
        rows: u32,
        /// Direction to move within the grid.
        dir: Dir,
    },
}

/// Run the placement diagnostic workflow.
pub fn run(args: &PlaceArgs, log_spec: &str) -> Result<()> {
    let parsed_directives: Vec<(String, Directive)> = args
        .specs
        .iter()
        .enumerate()
        .map(|(idx, raw)| {
            parse_directive(raw)
                .map(|directive| (raw.clone(), directive))
                .map_err(|err| Error::parse(format!("directive {} ('{}'): {}", idx + 1, raw, err)))
        })
        .collect::<Result<_>>()?;

    if parsed_directives.is_empty() {
        return Err(Error::NoPlacementDirectives);
    }

    let total_steps = parsed_directives.len();
    info!(steps = total_steps, "Placement directives queued");
    let hotki_bin = resolve_hotki_bin(args)?;
    let runtime = Runtime::new()?;

    let winops: Arc<dyn WinOps> = Arc::new(RealWinOps);
    let world = World::spawn_view(winops.clone(), WorldCfg::default());

    let mut backend = BackendProcess::spawn(&hotki_bin, args.server_logs, Some(log_spec))?;
    let mut conn = connect_backend(
        backend.socket_path(),
        &runtime,
        args.ready_timeout,
        args.ready_poll,
    )?;

    if let Some(cfg_path) = &args.config {
        let cfg = config::load_from_path(cfg_path)?;
        info!(path = %cfg_path.display(), "Sending config to backend");
        runtime.block_on(conn.set_config(cfg))?;
    }

    let accessibility_ok = permissions::accessibility_ok();
    let screen_ok = permissions::screen_recording_ok();
    diagnostics::log_permissions(accessibility_ok, screen_ok);
    if !accessibility_ok {
        warn!("Accessibility permission missing; placement may fail");
    }
    if !screen_ok {
        warn!("Screen Recording permission missing; window titles may be redacted");
    }

    info!(
        wait = %format_duration(args.snapshot_after),
        "Waiting before snapshot; focus the target window now"
    );
    thread::sleep(args.snapshot_after);

    let mut current_snapshot = runtime.block_on(conn.get_world_snapshot())?;
    diagnostics::log_world_snapshot("initial", &current_snapshot);

    for (idx, (raw_spec, directive)) in parsed_directives.iter().enumerate() {
        let step = idx + 1;

        let status = runtime.block_on(conn.get_world_status())?;
        info!(
            step,
            total = total_steps,
            ?status,
            "World status before placement"
        );

        let focused_pid = current_snapshot
            .focused
            .as_ref()
            .map(|app| app.pid)
            .or_else(|| status.focused_pid.and_then(|p| i32::try_from(p).ok()))
            .ok_or(Error::NoFocusedWindow)?;
        info!(
            step,
            total = total_steps,
            pid = focused_pid,
            "Focused PID selected for placement"
        );

        let pre_window = collect_window_snapshot(&current_snapshot, focused_pid)?;
        if let Some(ref snap) = pre_window {
            diagnostics::log_window_snapshot(&format!("step-{}-before", step), snap);
        } else {
            warn!(
                step,
                "Focused window geometry not available before placement"
            );
        }

        placement_counters_reset();
        match directive {
            Directive::Place {
                cols,
                rows,
                col,
                row,
            } => {
                info!(
                    step,
                    total = total_steps,
                    directive = raw_spec.as_str(),
                    grid.cols = *cols,
                    grid.rows = *rows,
                    target.col = *col,
                    target.row = *row,
                    "Applying place directive"
                );
                let window_id = resolve_window_id(pre_window.as_ref(), focused_pid)?;
                let target = WorldWindowId::new(focused_pid, window_id);
                runtime
                    .block_on(
                        world
                            .clone()
                            .request_place_for_window(target, *cols, *rows, *col, *row, None),
                    )
                    .map_err(Error::from)?;
            }
            Directive::PlaceMove { cols, rows, dir } => {
                let window_id = resolve_window_id(pre_window.as_ref(), focused_pid)?;
                let move_dir = to_move_dir(*dir);
                info!(
                    step,
                    total = total_steps,
                    directive = raw_spec.as_str(),
                    grid.cols = *cols,
                    grid.rows = *rows,
                    move.dir = ?dir,
                    window.id = window_id,
                    "Applying place_move directive"
                );
                let target = WorldWindowId::new(focused_pid, window_id);
                runtime
                    .block_on(
                        world
                            .clone()
                            .request_place_move_for_window(target, *cols, *rows, move_dir, None),
                    )
                    .map_err(Error::from)?;
            }
        }

        info!(
            step,
            total = total_steps,
            wait = %format_duration(args.settle_after),
            "Waiting after placement for geometry to settle"
        );
        thread::sleep(args.settle_after);

        let snapshot_after = runtime.block_on(conn.get_world_snapshot())?;
        diagnostics::log_world_snapshot(&format!("step-{}-after", step), &snapshot_after);

        let post_window = collect_window_snapshot(&snapshot_after, focused_pid)?;
        if let Some(ref snap) = post_window {
            diagnostics::log_window_snapshot(&format!("step-{}-after-window", step), snap);
        } else {
            warn!(
                step,
                "Focused window geometry not available after placement"
            );
        }
        diagnostics::log_window_delta(
            &format!("step-{}", step),
            pre_window.as_ref(),
            post_window.as_ref(),
        );

        let counters = placement_counters_snapshot();
        info!(step, total = total_steps, counters = ?counters, "Placement counters snapshot");

        current_snapshot = snapshot_after;
    }

    match runtime.block_on(conn.shutdown()) {
        Ok(()) => info!("Requested backend shutdown"),
        Err(e) => {
            warn!("Failed to send shutdown RPC: {}", e);
            backend.force_stop();
        }
    }

    drop(conn);

    backend.wait(Duration::from_secs(5))?;
    info!("Placement diagnostic complete");
    Ok(())
}

/// Parse a RON placement specification into a normalized directive.
fn parse_directive(spec: &str) -> Result<Directive> {
    let options = Options::default()
        .with_default_extension(Extensions::UNWRAP_NEWTYPES)
        .with_default_extension(Extensions::UNWRAP_VARIANT_NEWTYPES);
    let action: Action = options
        .from_str(spec)
        .map_err(|e| Error::parse(e.to_string()))?;
    match action {
        Action::Place(GridSpec::Grid(Grid(cols, rows)), AtSpec::At(At(col, row))) => {
            Ok(Directive::Place {
                cols,
                rows,
                col,
                row,
            })
        }
        Action::PlaceMove(GridSpec::Grid(Grid(cols, rows)), dir) => {
            Ok(Directive::PlaceMove { cols, rows, dir })
        }
        other => Err(Error::parse(format!(
            "expected place(...) or place_move(...), got {:?}",
            other
        ))),
    }
}

/// Determine which Hotki binary to spawn in `--server` mode.
fn resolve_hotki_bin(args: &PlaceArgs) -> Result<PathBuf> {
    if let Some(explicit) = &args.hotki_bin {
        if explicit.exists() {
            return Ok(explicit.clone());
        }
        return Err(Error::other(format!(
            "hotki binary override does not exist: {}",
            explicit.display()
        )));
    }

    if let Ok(env_path) = env::var("HOTKI_BIN") {
        let pb = PathBuf::from(&env_path);
        if pb.exists() {
            return Ok(pb);
        }
        warn!(path = env_path, "HOTKI_BIN set but file does not exist");
    }

    if let Ok(cargo_bin) = env::var("CARGO_BIN_EXE_hotki") {
        let pb = PathBuf::from(&cargo_bin);
        if pb.exists() {
            info!(path = %pb.display(), "Using cargo-provided hotki binary path");
            return Ok(pb);
        }
        debug!(
            path = cargo_bin,
            "CARGO_BIN_EXE_hotki provided but missing on disk"
        );
    }

    let exe = env::current_exe()?;
    let exe_dir = exe.parent();
    let workspace_root = workspace_root()?;
    let target_dir = target_dir(&workspace_root);
    let profile = exe_dir
        .and_then(|dir| dir.file_name())
        .and_then(|name| name.to_str())
        .map(|s| s.to_owned())
        .or_else(|| env::var("PROFILE").ok())
        .unwrap_or_else(|| "debug".to_string());

    let mut primary = primary_hotki_paths(exe_dir, &target_dir, profile.as_str());
    if let Some(existing) = primary.iter().find(|path| path.exists()) {
        info!(path = %existing.display(), "Using existing hotki backend binary");
        return Ok(existing.clone());
    }

    info!(profile = %profile, "Hotki backend binary missing; building via cargo");
    build_hotki_backend(&workspace_root, profile.as_str())?;

    primary = primary_hotki_paths(exe_dir, &target_dir, profile.as_str());
    if let Some(existing) = primary.iter().find(|path| path.exists()) {
        info!(path = %existing.display(), "Hotki backend built");
        return Ok(existing.clone());
    }

    let fallback = fallback_hotki_paths(&target_dir, profile.as_str());
    if let Some(existing) = fallback.iter().find(|path| path.exists()) {
        info!(
            path = %existing.display(),
            "Falling back to alternate-profile hotki backend binary"
        );
        return Ok(existing.clone());
    }

    let searched = primary
        .into_iter()
        .chain(fallback)
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    Err(Error::other(format!(
        "could not locate hotki binary; pass --hotki-bin or set HOTKI_BIN (searched: {searched})"
    )))
}

/// Return the workspace root directory derived from the crate manifest.
fn workspace_root() -> Result<PathBuf> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .ok_or_else(|| Error::other("failed to resolve workspace root"))
}

/// Determine the Cargo target directory, honouring `CARGO_TARGET_DIR` when set.
fn target_dir(workspace_root: &Path) -> PathBuf {
    env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| workspace_root.join("target"))
}

/// Candidate hotki binaries that match the current build profile.
fn primary_hotki_paths(exe_dir: Option<&Path>, target_dir: &Path, profile: &str) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let bin_name = hotki_binary_name();

    if let Some(dir) = exe_dir {
        paths.push(dir.join(bin_name));
    }

    paths.push(target_dir.join(profile).join(bin_name));

    paths.sort();
    paths.dedup();
    paths
}

/// Alternate-profile hotki binaries used as a fallback when primary lookup fails.
fn fallback_hotki_paths(target_dir: &Path, profile: &str) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let bin_name = hotki_binary_name();
    let alt_profile = if profile == "release" {
        "debug"
    } else {
        "release"
    };

    paths.push(target_dir.join(alt_profile).join(bin_name));
    paths.sort();
    paths.dedup();
    paths
}

/// Platform-specific executable name for the hotki binary.
fn hotki_binary_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "hotki.exe"
    } else {
        "hotki"
    }
}

/// Invoke `cargo build` to produce the hotki binary for the given profile.
fn build_hotki_backend(workspace_root: &Path, profile: &str) -> Result<()> {
    let mut command = Command::new("cargo");
    command.arg("build").arg("-p").arg("hotki");
    if profile == "release" {
        command.arg("--release");
    }
    info!(profile = %profile, "Running cargo build for hotki backend");
    let status = command.current_dir(workspace_root).status()?;
    if status.success() {
        Ok(())
    } else {
        Err(Error::other(format!(
            "cargo build -p hotki failed with status {}",
            status
        )))
    }
}

/// Connect to the backend socket, retrying until the timeout expires.
fn connect_backend(
    socket_path: &str,
    runtime: &Runtime,
    timeout: Duration,
    poll: Duration,
) -> Result<hotki_server::Connection> {
    let deadline = Instant::now() + timeout;
    loop {
        match runtime.block_on(hotki_server::Connection::connect_unix(socket_path)) {
            Ok(conn) => return Ok(conn),
            Err(err) => {
                if Instant::now() >= deadline {
                    return Err(Error::BackendStartupTimeout(timeout));
                }
                debug!(error = %err, "Waiting for backend socket");
                thread::sleep(poll);
            }
        }
    }
}

/// Build a diagnostic snapshot for the target window by combining world and AX data.
fn collect_window_snapshot(
    snapshot: &hotki_server::WorldSnapshotLite,
    pid: i32,
) -> Result<Option<WindowSnapshot>> {
    let window = snapshot
        .windows
        .iter()
        .find(|w| w.pid == pid && w.focused)
        .or_else(|| snapshot.windows.iter().find(|w| w.pid == pid));
    let Some(win) = window else {
        return Ok(None);
    };

    let ax = match ax_props_for_window_id(win.id) {
        Ok(props) => Some(props),
        Err(err) => {
            debug!("Failed to query AX props: {}", err);
            None
        }
    };
    let frame =
        ax_window_frame(pid, &win.title).map(|((x, y), (w, h))| mac_winops::Rect::new(x, y, w, h));

    Ok(Some(WindowSnapshot {
        app: win.app.clone(),
        title: win.title.clone(),
        pid: win.pid,
        window_id: win.id,
        display_id: win.display_id,
        z: win.z,
        ax,
        frame,
    }))
}

/// Determine the Core Graphics window identifier to target for the move directive.
fn resolve_window_id(pre_window: Option<&WindowSnapshot>, pid: i32) -> Result<WindowId> {
    if let Some(snapshot) = pre_window {
        return Ok(snapshot.window_id);
    }
    if let Some(id) = ax_focused_window_id_for_pid(pid) {
        debug!(pid, id, "Resolved window id via AX focus fallback");
        return Ok(id);
    }
    Err(Error::WindowIdUnavailable { pid })
}

/// Convert configuration directions into mac_winops move directions.
fn to_move_dir(dir: Dir) -> MoveDirection {
    match dir {
        Dir::Left => MoveDirection::Left,
        Dir::Right => MoveDirection::Right,
        Dir::Up => MoveDirection::Up,
        Dir::Down => MoveDirection::Down,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_directive_accepts_place() {
        let directive = parse_directive("place(grid(4, 4), at(1, 0))").expect("parse");
        match directive {
            Directive::Place {
                cols,
                rows,
                col,
                row,
            } => {
                assert_eq!(cols, 4);
                assert_eq!(rows, 4);
                assert_eq!(col, 1);
                assert_eq!(row, 0);
            }
            other => panic!("unexpected directive: {other:?}"),
        }
    }

    #[test]
    fn parse_directive_accepts_place_move() {
        let directive = parse_directive("place_move(grid(3, 2), left)").expect("parse");
        match directive {
            Directive::PlaceMove { cols, rows, dir } => {
                assert_eq!(cols, 3);
                assert_eq!(rows, 2);
                assert_eq!(dir, Dir::Left);
            }
            other => panic!("unexpected directive: {other:?}"),
        }
    }
}
