use std::{
    cmp, env, fs, thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use objc2_app_kit::NSScreen;
use objc2_foundation::MainThreadMarker;

use crate::{
    config,
    error::{Error, Result},
    process::HelperWindowBuilder,
    server_drive,
    session::HotkiSession,
    ui_interaction::send_activation_chord,
    util::resolve_hotki_bin,
};

// ---------- Helper functions ----------

fn approx(a: f64, b: f64, eps: f64) -> bool {
    (a - b).abs() <= eps
}

pub fn run_hide_test(timeout_ms: u64, with_logs: bool) -> Result<()> {
    let Some(hotki_bin) = resolve_hotki_bin() else {
        return Err(Error::HotkiBinNotFound);
    };

    // Spawn our own helper window (winit) and use it as the hide target.
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let title = config::hide_test_title(now);
    let helper_time = timeout_ms.saturating_add(config::HIDE_HELPER_EXTRA_TIME_MS);
    let helper = HelperWindowBuilder::new(&title)
        .with_time_ms(helper_time)
        .spawn()?;
    let pid = helper.pid;
    // Wait until the helper window is visible via CG or AX
    let deadline = Instant::now()
        + Duration::from_millis(std::cmp::min(timeout_ms, config::HIDE_FIRST_WINDOW_MAX_MS));
    let mut ready = false;
    while Instant::now() < deadline {
        let wins = mac_winops::list_windows();
        let cg_ok = wins.iter().any(|w| w.pid == pid && w.title == title);
        let ax_ok = mac_winops::ax_has_window_title(pid, &title);
        if cg_ok || ax_ok {
            ready = true;
            break;
        }
        thread::sleep(config::ms(config::HIDE_POLL_MS));
    }
    if !ready {
        // helper cleans up automatically via Drop
        return Err(Error::FocusNotObserved {
            timeout_ms,
            expected: format!("helper window '{}' not visible", title),
        });
    }

    // Temporary config: shift+cmd+0 -> h -> (t/on/off); hide HUD to reduce intrusiveness
    let cfg = r#"(
    keys: [
        ("shift+cmd+0", "activate", keys([
            ("h", "hide", keys([
                ("t", "toggle", hide(toggle)),
                ("o", "on", hide(on)),
                ("f", "off", hide(off)),
            ])),
            ("shift+cmd+0", "exit", exit, (global: true, hide: true)),
        ])),
        ("esc", "Back", pop, (global: true, hide: true, hud_only: true)),
    ],
    style: (hud: (mode: hide))
)
"#
    .to_string();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let tmp_path = env::temp_dir().join(format!("hotki-smoketest-hide-{}.ron", now));
    fs::write(&tmp_path, cfg)?;

    // Launch hotki
    let mut sess = HotkiSession::launch_with_config(&hotki_bin, &tmp_path, with_logs)?;
    let (hud_ok, _ms) = sess.wait_for_hud(timeout_ms);
    if !hud_ok {
        return Err(Error::HudNotVisible { timeout_ms });
    }

    // Snapshot initial AX frame of the helper window
    let (p0, s0) =
        if let Some(((px, py), (width, height))) = mac_winops::ax_window_frame(pid, &title) {
            ((px, py), (width, height))
        } else {
            return Err(Error::FocusNotObserved {
                timeout_ms,
                expected: "AX window for helper".into(),
            });
        };

    // Compute expected target X on the main screen (1px sliver)
    let target_x = if let Some(mtm) = MainThreadMarker::new() {
        let scr = NSScreen::mainScreen(mtm).expect("main screen");
        let vf = scr.visibleFrame();
        (vf.origin.x + vf.size.width) - 1.0
    } else {
        // Fallback guess: large X likely on right
        p0.0 + config::WINDOW_POSITION_OFFSET
    };

    // Drive: send 'h' then gate and send 'o' (hide on)
    if server_drive::is_ready() {
        let _ = server_drive::wait_for_ident("h", crate::config::BINDING_GATE_DEFAULT_MS);
    }
    crate::ui_interaction::send_key("h");
    if server_drive::is_ready() {
        let _ = server_drive::wait_for_ident("o", crate::config::BINDING_GATE_DEFAULT_MS);
    }
    crate::ui_interaction::send_key("o");

    // Wait for position change
    let mut moved = false;
    let deadline = Instant::now()
        + Duration::from_millis(cmp::max(config::HIDE_MIN_TIMEOUT_MS, timeout_ms / 4));
    let mut _p_on = p0;
    while Instant::now() < deadline {
        if let Some((px, py)) = mac_winops::ax_window_position(pid, &title) {
            _p_on = (px, py);
            if !approx(px, p0.0, 2.0) || approx(px, target_x, 6.0) {
                moved = true;
                break;
            }
        }
        thread::sleep(config::ms(config::KEY_EVENT_DELAY_MS));
    }
    if !moved {
        eprintln!(
            "debug: no movement detected after hide(on). last vs start x: {:.1} -> {:.1}",
            _p_on.0, p0.0
        );
        // Cleanup session (helper cleans up automatically via Drop)
        sess.shutdown();
        sess.kill_and_wait();
        return Err(Error::SpawnFailed(
            "window position did not change after hide(on)".into(),
        ));
    }

    // Drive: reopen/activate and turn hide off (reveal)
    thread::sleep(config::ms(config::HIDE_REOPEN_DELAY_MS));
    send_activation_chord();
    thread::sleep(config::ms(config::HIDE_ACTIVATE_POST_DELAY_MS));
    if server_drive::is_ready() {
        let _ = server_drive::wait_for_ident("h", crate::config::BINDING_GATE_DEFAULT_MS);
    }
    crate::ui_interaction::send_key("h");
    if server_drive::is_ready() {
        let _ = server_drive::wait_for_ident("f", crate::config::BINDING_GATE_DEFAULT_MS);
    }
    crate::ui_interaction::send_key("f");

    // Wait until position roughly returns to original
    let mut restored = false;
    let deadline2 = Instant::now()
        + Duration::from_millis(std::cmp::min(
            config::HIDE_RESTORE_MAX_MS,
            cmp::max(config::HIDE_SECONDARY_MIN_TIMEOUT_MS, timeout_ms / 3),
        ));
    while Instant::now() < deadline2 {
        if let Some(((px2, py2), (width2, height2))) = mac_winops::ax_window_frame(pid, &title) {
            let pos_ok = approx(px2, p0.0, 8.0) && approx(py2, p0.1, 8.0);
            let size_ok = approx(width2, s0.0, 8.0) && approx(height2, s0.1, 8.0);
            // quiet on success path
            if pos_ok && size_ok {
                restored = true;
                break;
            }
        }
        thread::sleep(config::ms(config::HIDE_POLL_MS));
    }

    // Cleanup (helper cleans up automatically via Drop)
    sess.shutdown();
    sess.kill_and_wait();

    if !restored {
        return Err(Error::SpawnFailed(
            "window did not restore to original frame after hide(off)".into(),
        ));
    }
    Ok(())
}
