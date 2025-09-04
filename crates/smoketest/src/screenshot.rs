use std::{
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

use crate::{SmkError, Summary, session::HotkiSession};

fn resolve_hotki_bin() -> Option<PathBuf> {
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

// ===== Window discovery and capture =====

fn find_window_by_title(pid: u32, title: &str) -> Option<(u32, (i32, i32, i32, i32))> {
    use core_foundation::dictionary::CFDictionaryRef;
    use core_foundation::{
        array::CFArray,
        base::{CFType, TCFType, TCFTypeRef},
        dictionary::CFDictionary,
        number::CFNumber,
        string::CFString,
    };
    use core_graphics2::window::{
        CGWindowListOption, copy_window_info, kCGNullWindowID, kCGWindowBounds, kCGWindowName,
        kCGWindowNumber, kCGWindowOwnerPID,
    };
    let arr: CFArray = copy_window_info(CGWindowListOption::OnScreenOnly, kCGNullWindowID)?;
    for item in arr.iter() {
        let dict_ref = unsafe { CFDictionaryRef::from_void_ptr(*item) };
        let dict: CFDictionary<CFString, CFType> =
            unsafe { CFDictionary::wrap_under_get_rule(dict_ref) };
        let owner_pid = unsafe { dict.find(kCGWindowOwnerPID) }
            .and_then(|v| v.downcast::<CFNumber>())
            .and_then(|n| n.to_i64().map(|v| v as u32))
            .unwrap_or_default();
        if owner_pid != pid {
            continue;
        }
        let name = unsafe { dict.find(kCGWindowName) }
            .and_then(|v| v.downcast::<CFString>())
            .map(|s| s.to_string())
            .unwrap_or_default();
        if name != title {
            continue;
        }
        let win_id: u32 = unsafe { dict.find(kCGWindowNumber) }
            .and_then(|v| v.downcast::<CFNumber>())
            .and_then(|n| n.to_i64().map(|v| v as u32))?;
        let bdict_any = unsafe { dict.find(kCGWindowBounds) }?;
        let bdict_ref: CFDictionaryRef = bdict_any.as_CFTypeRef() as CFDictionaryRef;
        let bdict: CFDictionary<CFString, CFType> =
            unsafe { CFDictionary::wrap_under_get_rule(bdict_ref) };
        let kx = CFString::from_static_string("X");
        let ky = CFString::from_static_string("Y");
        let kw = CFString::from_static_string("Width");
        let kh = CFString::from_static_string("Height");
        let get = |k: &CFString| {
            bdict
                .find(k.clone())
                .and_then(|v| v.downcast::<CFNumber>())
                .and_then(|n| n.to_i64().map(|v| v as i32))
        };
        let (x, y, w, h) = (get(&kx)?, get(&ky)?, get(&kw)?, get(&kh)?);
        return Some((win_id, (x, y, w, h)));
    }
    None
}

fn capture_window_by_id_or_rect(pid: u32, title: &str, dir: &Path, name: &str) -> bool {
    use std::ffi::OsStr;
    if let Some((win_id, (x, y, w, h))) = find_window_by_title(pid, title) {
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
        let status = Command::new("screencapture")
            .args([
                OsStr::new("-x"),
                OsStr::new("-o"),
                OsStr::new("-l"),
                std::ffi::OsString::from(win_id.to_string()).as_os_str(),
                path.as_os_str(),
            ])
            .status();
        if matches!(status, Ok(s) if s.success()) {
            return true;
        }
        let rect_arg = format!("{},{},{},{}", x, y, w, h);
        let status = Command::new("screencapture")
            .args([
                OsStr::new("-x"),
                OsStr::new("-R"),
                OsStr::new(&rect_arg),
                path.as_os_str(),
            ])
            .status();
        return matches!(status, Ok(s) if s.success());
    }
    false
}

pub(crate) fn run_screenshots(
    theme: Option<String>,
    dir: PathBuf,
    timeout_ms: u64,
) -> Result<Summary, SmkError> {
    let cwd = std::env::current_dir().map_err(SmkError::Io)?;
    let cfg_path = cwd.join("examples/test.ron");
    if !cfg_path.exists() {
        return Err(SmkError::MissingConfig(cfg_path));
    }

    let Some(hotki_bin) = resolve_hotki_bin() else {
        return Err(SmkError::HotkiBinNotFound);
    };

    // Optional theme override by writing a temp config
    let used_cfg_path = if let Some(name) = theme.clone() {
        match std::fs::read_to_string(&cfg_path) {
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
                let tmp = std::env::temp_dir().join(format!(
                    "hotki-smoketest-shots-{}-{}.ron",
                    std::process::id(),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_nanos()
                ));
                if std::fs::write(&tmp, out).is_ok() {
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

    let _ = std::fs::create_dir_all(&dir);
    let pid = sess.pid();
    let hud_ok = capture_window_by_id_or_rect(pid, "Hotki HUD", &dir, "001_hud");

    // Trigger notifications via chords
    let gap = Duration::from_millis(160);
    let down_ms = Duration::from_millis(80);
    for (k, name) in [
        ("t", None),
        ("s", Some("002_notify_success")),
        ("i", Some("003_notify_info")),
        ("w", Some("004_notify_warning")),
        ("e", Some("005_notify_error")),
    ] {
        if let Some(ch) = mac_keycode::Chord::parse(k) {
            let relayer = relaykey::RelayKey::new_unlabeled();
            relayer.key_down(0, ch.clone(), false);
            std::thread::sleep(down_ms);
            relayer.key_up(0, ch);
            std::thread::sleep(gap);
            if let Some(n) = name {
                std::thread::sleep(Duration::from_millis(120));
                let _ = capture_window_by_id_or_rect(pid, "Hotki Notification", &dir, n);
            }
        }
    }

    // Exit HUD and shutdown
    if let Some(ch) = mac_keycode::Chord::parse("shift+cmd+0") {
        let relayer = relaykey::RelayKey::new_unlabeled();
        relayer.key_down(0, ch.clone(), false);
        std::thread::sleep(down_ms);
        relayer.key_up(0, ch);
    }
    sess.shutdown();
    sess.kill_and_wait();

    let mut sum = Summary::new();
    sum.hud_seen = seen_hud;
    sum.time_to_hud_ms = Some(t_hud);
    if !seen_hud {
        return Err(SmkError::HudNotVisible { timeout_ms });
    }
    if !hud_ok {
        return Err(SmkError::CaptureFailed("HUD"));
    }
    Ok(sum)
}
