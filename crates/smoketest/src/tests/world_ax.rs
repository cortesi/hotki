//! World AX props smoketest.
//!
//! Spawns a helper window, ensures it is frontmost, spawns a `hotki-world`
//! instance, and queries `WorldHandle::ax_props` for the focused window.
//! Verifies that role/subrole are non-empty and settable flags are resolved.

use std::{sync::Arc, time::Duration};

use crate::error::{Error, Result};

pub fn run_world_ax_test(timeout_ms: u64, _logs: bool) -> Result<()> {
    // Spawn a visible helper and keep it alive longer than our timeout.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let title = format!("hotki smoketest: world-ax {}-{}", std::process::id(), now);
    let lifetime = timeout_ms.saturating_add(crate::config::HELPER_WINDOW_EXTRA_TIME_MS);
    let mut helper = crate::tests::helpers::spawn_helper_visible(
        title.clone(),
        lifetime,
        crate::config::WAIT_FIRST_WINDOW_MS,
        crate::config::POLL_INTERVAL_MS,
        "AX",
    )?;

    // Bring helper frontmost best-effort.
    crate::tests::helpers::ensure_frontmost(helper.pid, &title, 3, 100);

    // Start a local world with real winops.
    let winops: Arc<dyn mac_winops::ops::WinOps> = Arc::new(mac_winops::ops::RealWinOps);
    // Ensure a Tokio runtime context exists when spawning the world actor.
    let world = crate::runtime::block_on(async move {
        hotki_world::World::spawn(winops, hotki_world::WorldCfg::default())
    })?;

    // Wait briefly for world to tick and observe focus.
    let deadline = std::time::Instant::now() + Duration::from_millis(timeout_ms);
    let mut focused_key = None;
    while std::time::Instant::now() < deadline {
        focused_key = crate::runtime::block_on(async { world.focused().await })?;
        if focused_key.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    let Some(key) = focused_key else {
        return Err(Error::InvalidState(
            "world-ax: no focused window observed".into(),
        ));
    };

    // Fetch the focused window snapshot and read statically captured props.
    let fw = crate::runtime::block_on(async { world.focused_window().await })?;
    let Some(w) = fw else {
        return Err(Error::InvalidState("world-ax: no focused window".into()));
    };
    let Some(p) = w.ax else {
        return Err(Error::InvalidState(
            "world-ax: missing ax props on focused window".into(),
        ));
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

    let _ = helper.kill_and_wait();
    Ok(())
}
