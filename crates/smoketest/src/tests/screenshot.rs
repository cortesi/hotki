use std::{
    env,
    ffi::{OsStr, OsString},
    fs,
    path::{Path, PathBuf},
    process::{self, Command},
    thread,
    time::{SystemTime, UNIX_EPOCH},
};

use crate::util::resolve_hotki_bin;
use crate::{
    config,
    error::{Error, Result},
    results::Summary,
    session::HotkiSession,
    ui_interaction::{send_activation_chord, send_key},
};

// ===== Window discovery and capture =====

fn find_window_by_title(pid: u32, title: &str) -> Option<(u32, Option<(i32, i32, i32, i32)>)> {
    // Use mac-winops to get window information
    mac_winops::list_windows()
        .into_iter()
        .find(|w| w.pid == pid as i32 && w.title == title)
        .map(|w| {
            let rect = w.pos.map(|pos| (pos.x, pos.y, pos.width, pos.height));
            (w.id, rect)
        })
}

fn capture_window_by_id_or_rect(pid: u32, title: &str, dir: &Path, name: &str) -> bool {
    if let Some((win_id, rect_opt)) = find_window_by_title(pid, title) {
        eprintln!("screens: found window '{}' id={} rect={:?}", title, win_id, rect_opt);
        let sanitized = name
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect::<String>();
        let path = dir.join(format!("{}.png", sanitized));
        // First, try to capture by window id (works even if we lack precise bounds)
        let status = Command::new("screencapture")
            .args([
                OsStr::new("-x"),
                OsStr::new("-o"),
                OsStr::new("-l"),
                OsString::from(win_id.to_string()).as_os_str(),
                path.as_os_str(),
            ])
            .status();
        eprintln!("screens: screencapture -l {} -> {:?}", win_id, status);
        if matches!(status, Ok(s) if s.success()) {
            return true;
        }
        if let Some((x, y, w, h)) = rect_opt {
            let rect_arg = format!("{},{},{},{}", x, y, w, h);
            let status = Command::new("screencapture")
                .args([
                    OsStr::new("-x"),
                    OsStr::new("-R"),
                    OsStr::new(&rect_arg),
                    path.as_os_str(),
                ])
                .status();
            eprintln!("screens: screencapture -R {} -> {:?}", rect_arg, status);
            return matches!(status, Ok(s) if s.success());
        }
        return false;
    }
    eprintln!(
        "screens: did not find window '{}' under pid {} (available: {:?})",
        title,
        pid,
        mac_winops::list_windows()
            .into_iter()
            .filter(|w| w.pid == pid as i32)
            .map(|w| (w.title, w.id, w.pos))
            .collect::<Vec<_>>()
    );
    false
}

pub fn run_screenshots(theme: Option<String>, dir: PathBuf, timeout_ms: u64) -> Result<Summary> {
    let cwd = env::current_dir()?;
    let cfg_path = cwd.join(config::DEFAULT_TEST_CONFIG_PATH);
    if !cfg_path.exists() {
        return Err(Error::MissingConfig(cfg_path));
    }

    let Some(hotki_bin) = resolve_hotki_bin() else {
        return Err(Error::HotkiBinNotFound);
    };

    // Optional theme override by writing a temp config
    let used_cfg_path = if let Some(name) = theme.clone() {
        match fs::read_to_string(&cfg_path) {
            Ok(s) => {
                let mut out = String::new();
                if s.contains("base_theme:") {
                    let re = regex::Regex::new("base_theme\\s*:\\s*\"[^\"]*\"").unwrap();
                    out = re
                        .replace(&s, format!("base_theme: \"{}\"", name))
                        .to_string();
                } else if let Some(pos) = s.find('(') {
                    let (head, tail) = s.split_at(pos + 1);
                    out.push_str(head);
                    out.push('\n');
                    out.push_str(&format!("    base_theme: \"{}\",\n", name));
                    out.push_str(tail);
                } else {
                    out = s;
                }
                let tmp = env::temp_dir().join(format!(
                    "hotki-smoketest-shots-{}-{}.ron",
                    process::id(),
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap()
                        .as_nanos()
                ));
                if fs::write(&tmp, out).is_ok() {
                    tmp
                } else {
                    cfg_path.clone()
                }
            }
            Err(_) => cfg_path.clone(),
        }
    } else {
        cfg_path.clone()
    };

    let mut sess = HotkiSession::launch_with_config(&hotki_bin, &used_cfg_path, true)?;
    let (seen_hud, t_hud) = sess.wait_for_hud(timeout_ms);

    let _ = fs::create_dir_all(&dir);
    let pid = sess.pid();
    let hud_ok = capture_window_by_id_or_rect(pid, "Hotki HUD", &dir, "hud");

    // Trigger notifications via chords
    let gap = config::ms(config::SCREENSHOT_FRAME_GAP_MS);
    for (k, name) in [
        ("t", None),
        ("s", Some("notify_success")),
        ("i", Some("notify_info")),
        ("w", Some("notify_warning")),
        ("e", Some("notify_error")),
    ] {
        send_key(k);
        thread::sleep(gap);
        if let Some(n) = name {
            thread::sleep(config::ms(config::UI_ACTION_DELAY_MS));
            let _ = capture_window_by_id_or_rect(pid, "Hotki Notification", &dir, n);
        }
    }

    // Exit HUD and shutdown
    send_activation_chord();
    sess.shutdown();
    sess.kill_and_wait();

    let mut sum = Summary::new();
    sum.hud_seen = seen_hud;
    sum.time_to_hud_ms = Some(t_hud);
    if !seen_hud {
        return Err(Error::HudNotVisible { timeout_ms });
    }
    if !hud_ok {
        eprintln!(
            "warning: HUD capture failed (missing Screen Recording permission?). Skipping image writes."
        );
    }
    Ok(sum)
}
