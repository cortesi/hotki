//! World AX props smoketest.
//!
//! Spawns a helper window, ensures it is frontmost, spawns a `hotki-world`
//! instance, and queries `WorldHandle::ax_props` for the focused window.
//! Verifies that role/subrole are non-empty and settable flags are resolved.

use std::{
    process as std_process,
    sync::Arc,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use mac_winops::ops::{RealWinOps, WinOps};

use crate::{
    config,
    error::{Error, Result},
    helper_window::{ensure_frontmost, spawn_helper_visible},
    runtime,
};

/// Verify AX properties of the focused window via `WorldHandle::ax_props`.
pub fn run_world_ax_test(timeout_ms: u64, _logs: bool) -> Result<()> {
    // Spawn a visible helper and keep it alive longer than our timeout.
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let title = format!("hotki smoketest: world-ax {}-{}", std_process::id(), now);
    let lifetime = timeout_ms.saturating_add(config::HELPER_WINDOW.extra_time_ms);
    let mut helper = spawn_helper_visible(
        &title,
        lifetime,
        config::WAITS.first_window_ms,
        config::INPUT_DELAYS.poll_interval_ms,
        "AX",
    )?;

    // Bring helper frontmost best-effort.
    ensure_frontmost(helper.pid, &title, 3, 100);

    // Start a local world with real winops.
    let winops: Arc<dyn WinOps> = Arc::new(RealWinOps);
    // Ensure a Tokio runtime context exists when spawning the world actor.
    let world = runtime::block_on(async move {
        hotki_world::World::spawn(winops, hotki_world::WorldCfg::default())
    })?;

    // Wait for world to report a focused window with AX props populated.
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    let p = loop {
        let fw = runtime::block_on(async { world.focused_window().await })?;
        if let Some(w) = fw
            && let Some(props) = w.ax
        {
            break props;
        }
        if Instant::now() >= deadline {
            return Err(Error::InvalidState(
                "world-ax: missing ax props on focused window".into(),
            ));
        }
        thread::sleep(Duration::from_millis(25));
    };

    // Basic verification: role/subrole present and settable flags resolved (Some(_)).
    let role_ok = p
        .role
        .as_ref()
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false);
    let subrole_ok = p
        .subrole
        .as_ref()
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false);
    let pos_flag_ok = p.can_set_pos.is_some();
    let size_flag_ok = p.can_set_size.is_some();

    if !(role_ok && subrole_ok && pos_flag_ok && size_flag_ok) {
        return Err(Error::InvalidState(format!(
            "world-ax: invalid props (role={:?} subrole={:?} can_set_pos={:?} can_set_size={:?})",
            p.role, p.subrole, p.can_set_pos, p.can_set_size
        )));
    }

    println!(
        "world-ax: role='{}' subrole='{}' can_set_pos={} can_set_size={}",
        p.role.unwrap_or_default(),
        p.subrole.unwrap_or_default(),
        p.can_set_pos.unwrap_or(false),
        p.can_set_size.unwrap_or(false)
    );

    if let Err(_e) = helper.kill_and_wait() {}
    Ok(())
}
