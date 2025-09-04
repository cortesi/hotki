use std::{path::PathBuf, time::Duration};

use crate::{SmkError, Summary, session::HotkiSession};

pub(crate) fn resolve_hotki_bin() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("HOTKI_BIN") {
        let pb = PathBuf::from(p);
        if pb.exists() {
            return Some(pb);
        }
    }
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("hotki")))
        .filter(|p| p.exists())
}

// ===== UI demos (no screenshot capture here) =====

pub(crate) fn run_ui_demo(timeout_ms: u64) -> Result<Summary, SmkError> {
    let cwd = std::env::current_dir().map_err(SmkError::Io)?;
    let cfg_path = cwd.join("examples/test.ron");
    if !cfg_path.exists() {
        return Err(SmkError::MissingConfig(cfg_path));
    }
    let Some(hotki_bin) = resolve_hotki_bin() else {
        return Err(SmkError::HotkiBinNotFound);
    };

    let mut sess = HotkiSession::launch_with_config(&hotki_bin, &cfg_path, true)?;
    std::thread::sleep(std::time::Duration::from_millis(2000));
    let (seen_hud, t_hud) = sess.wait_for_hud(timeout_ms);

    let mut seq: Vec<&str> = Vec::new();
    if seen_hud {
        seq.push("t");
        seq.extend(std::iter::repeat_n("l", 5));
        seq.push("esc");
    }
    seq.push("shift+cmd+0");
    let gap = Duration::from_millis(150);
    let down_ms = Duration::from_millis(80);
    for s in seq {
        if let Some(ch) = mac_keycode::Chord::parse(s) {
            let relayer = relaykey::RelayKey::new_unlabeled();
            relayer.key_down(0, ch.clone(), false);
            std::thread::sleep(down_ms);
            relayer.key_up(0, ch);
            std::thread::sleep(gap);
        } else {
            eprintln!("failed to parse chord: {}", s);
            std::thread::sleep(gap);
        }
    }

    sess.shutdown();
    sess.kill_and_wait();

    let mut sum = Summary::new();
    sum.hud_seen = seen_hud;
    sum.time_to_hud_ms = Some(t_hud);
    if !seen_hud {
        return Err(SmkError::HudNotVisible { timeout_ms });
    }
    Ok(sum)
}

pub(crate) fn run_minui_demo(timeout_ms: u64) -> Result<Summary, SmkError> {
    let ron = r#"(
        keys: [
            ("shift+cmd+0", "activate", keys([
                ("t", "Theme tester", keys([
                    ("h", "Theme Prev", theme_prev, (noexit: true)),
                    ("l", "Theme Next", theme_next, (noexit: true)),
                ])),
            ])),
            ("shift+cmd+0", "exit", exit, (global: true, hide: true)),
            ("esc", "Back", pop, (global: true, hide: true, hud_only: true)),
        ],
        style: (hud: (mode: mini)),
    )"#;
    let cfg_path = std::env::temp_dir().join(format!(
        "hotki-smoketest-minui-{}-{}.ron",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(&cfg_path, ron).map_err(SmkError::Io)?;

    let Some(hotki_bin) = resolve_hotki_bin() else {
        let _ = std::fs::remove_file(&cfg_path);
        return Err(SmkError::HotkiBinNotFound);
    };

    let mut sess = HotkiSession::launch_with_config(&hotki_bin, &cfg_path, false)?;
    std::thread::sleep(std::time::Duration::from_millis(2000));
    let (seen_hud, t_hud) = sess.wait_for_hud(timeout_ms);
    if !seen_hud {
        let _ = std::fs::remove_file(&cfg_path);
        sess.kill_and_wait();
        return Err(SmkError::HudNotVisible { timeout_ms });
    }

    let mut seq: Vec<String> = Vec::new();
    seq.push("t".to_string());
    seq.extend(std::iter::repeat_n("l".to_string(), 5));
    seq.push("esc".to_string());
    seq.push("shift+cmd+0".to_string());
    let gap = Duration::from_millis(150);
    let down_ms = Duration::from_millis(80);
    for s in seq {
        if let Some(ch) = mac_keycode::Chord::parse(&s) {
            let relayer = relaykey::RelayKey::new_unlabeled();
            relayer.key_down(0, ch.clone(), false);
            std::thread::sleep(down_ms);
            relayer.key_up(0, ch);
            std::thread::sleep(gap);
        } else {
            eprintln!("failed to parse chord: {}", s);
            std::thread::sleep(gap);
        }
    }
    sess.shutdown();
    sess.kill_and_wait();
    let _ = std::fs::remove_file(&cfg_path);

    let mut sum = Summary::new();
    sum.hud_seen = seen_hud;
    sum.time_to_hud_ms = Some(t_hud);
    Ok(sum)
}
