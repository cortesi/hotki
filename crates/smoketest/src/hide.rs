use std::{
    cmp, env, fs,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use core_foundation::base::{CFRelease, CFTypeRef, TCFType};
use core_foundation::string::{CFString, CFStringRef};
use objc2_app_kit::NSScreen;
use objc2_foundation::MainThreadMarker;

use crate::{
    config,
    error::{Error, Result},
    process::HelperWindowBuilder,
    session::HotkiSession,
    util::resolve_hotki_bin,
};

// ---------- Minimal AX FFI (public macOS frameworks) ----------

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXUIElementCreateApplication(pid: i32) -> *mut core::ffi::c_void;
    fn AXUIElementCopyAttributeValue(
        element: *mut core::ffi::c_void,
        attr: CFStringRef,
        value: *mut CFTypeRef,
    ) -> i32;
    fn AXValueGetValue(value: CFTypeRef, theType: i32, valuePtr: *mut core::ffi::c_void) -> bool;
}


#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Cgp {
    x: f64,
    y: f64,
}
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Cgs {
    width: f64,
    height: f64,
}

fn cfstr(s: &'static str) -> CFStringRef {
    CFString::from_static_string(s).as_CFTypeRef() as CFStringRef
}

fn ax_get_point(element: *mut core::ffi::c_void, attr: CFStringRef) -> Option<Cgp> {
    let mut v: CFTypeRef = std::ptr::null_mut();
    let err = unsafe { AXUIElementCopyAttributeValue(element, attr, &mut v) };
    if err != 0 || v.is_null() {
        return None;
    }
    let mut p = Cgp { x: 0.0, y: 0.0 };
    let ok = unsafe { AXValueGetValue(v, config::AX_VALUE_CGPOINT_TYPE, &mut p as *mut _ as *mut _) };
    unsafe { CFRelease(v) };
    if !ok { None } else { Some(p) }
}

fn ax_get_size(element: *mut core::ffi::c_void, attr: CFStringRef) -> Option<Cgs> {
    let mut v: CFTypeRef = std::ptr::null_mut();
    let err = unsafe { AXUIElementCopyAttributeValue(element, attr, &mut v) };
    if err != 0 || v.is_null() {
        return None;
    }
    let mut s = Cgs {
        width: 0.0,
        height: 0.0,
    };
    let ok = unsafe { AXValueGetValue(v, config::AX_VALUE_CGSIZE_TYPE, &mut s as *mut _ as *mut _) };
    unsafe { CFRelease(v) };
    if !ok { None } else { Some(s) }
}

/// Locate an AX window element for `pid` by exact title match.
fn ax_find_window_by_title(pid: i32, title: &str) -> Option<*mut core::ffi::c_void> {
    let app = unsafe { AXUIElementCreateApplication(pid) };
    if app.is_null() {
        return None;
    }
    let attr_windows = cfstr("AXWindows");
    let mut wins_ref: CFTypeRef = std::ptr::null_mut();
    let err = unsafe { AXUIElementCopyAttributeValue(app, attr_windows, &mut wins_ref) };
    unsafe { CFRelease(app as CFTypeRef) };
    if err != 0 || wins_ref.is_null() {
        return None;
    }
    let arr = unsafe {
        core_foundation::array::CFArray::<*const core::ffi::c_void>::wrap_under_get_rule(
            wins_ref as _,
        )
    };
    for i in 0..unsafe { core_foundation::array::CFArrayGetCount(arr.as_concrete_TypeRef()) } {
        let wref =
            unsafe { core_foundation::array::CFArrayGetValueAtIndex(arr.as_concrete_TypeRef(), i) };
        let w = wref as *mut core::ffi::c_void;
        if w.is_null() {
            continue;
        }
        let mut t_ref: CFTypeRef = std::ptr::null_mut();
        let terr = unsafe { AXUIElementCopyAttributeValue(w, cfstr("AXTitle"), &mut t_ref) };
        if terr != 0 || t_ref.is_null() {
            continue;
        }
        let cfs = unsafe { CFString::wrap_under_get_rule(t_ref as CFStringRef) };
        let t = cfs.to_string();
        if t == title {
            return Some(w);
        }
    }
    None
}

fn approx(a: f64, b: f64, eps: f64) -> bool {
    (a - b).abs() <= eps
}

// ---------- Local helpers ----------

fn send_key(seq: &str) {
    if let Some(ch) = mac_keycode::Chord::parse(seq) {
        let rk = relaykey::RelayKey::new_unlabeled();
        rk.key_down(0, ch.clone(), false);
        thread::sleep(config::ms(config::KEY_EVENT_DELAY_MS));
        rk.key_up(0, ch);
    }
}

/// First AXWindow element for a pid.
fn ax_first_window_for_pid(pid: i32) -> Option<*mut core::ffi::c_void> {
    let app = unsafe { AXUIElementCreateApplication(pid) };
    if app.is_null() {
        return None;
    }
    let mut wins_ref: CFTypeRef = std::ptr::null_mut();
    let err = unsafe { AXUIElementCopyAttributeValue(app, cfstr("AXWindows"), &mut wins_ref) };
    unsafe { CFRelease(app as CFTypeRef) };
    if err != 0 || wins_ref.is_null() {
        return None;
    }
    let arr = unsafe {
        core_foundation::array::CFArray::<*const core::ffi::c_void>::wrap_under_get_rule(
            wins_ref as _,
        )
    };
    let count = unsafe { core_foundation::array::CFArrayGetCount(arr.as_concrete_TypeRef()) };
    for i in 0..count {
        let wref =
            unsafe { core_foundation::array::CFArrayGetValueAtIndex(arr.as_concrete_TypeRef(), i) };
        let w = wref as *mut core::ffi::c_void;
        if !w.is_null() {
            return Some(w);
        }
    }
    None
}


// Wait for the AX window to be discoverable and return its pos/size.
// (unused helper removed)

pub(crate) fn run_hide_test(timeout_ms: u64, with_logs: bool) -> Result<()> {
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
    let mut helper = HelperWindowBuilder::new(&title)
        .with_time_ms(helper_time)
        .spawn()?;
    let pid = helper.pid;
    // Wait until the helper window is visible via CG or AX
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    let mut ready = false;
    while Instant::now() < deadline {
        let wins = mac_winops::list_windows();
        let cg_ok = wins.iter().any(|w| w.pid == pid && w.title == title);
        let ax_ok = mac_winops::ax_has_window_title(pid, &title);
        if cg_ok || ax_ok {
            ready = true;
            break;
        }
        thread::sleep(config::ms(config::KEY_EVENT_DELAY_MS));
    }
    if !ready {
        // helper cleans up automatically via Drop
        return Err(Error::FocusNotObserved {
            timeout_ms,
            expected: format!("helper window '{}' not visible", title),
        });
    }

    // Temporary config: shift+cmd+0 -> h -> (t/on/off)
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
    ]
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
    let (p0, s0) = if let Some(w) =
        ax_find_window_by_title(pid, &title).or_else(|| ax_first_window_for_pid(pid))
    {
        if let (Some(p), Some(s)) = (
            ax_get_point(w, cfstr("AXPosition")),
            ax_get_size(w, cfstr("AXSize")),
        ) {
            (p, s)
        } else {
            return Err(Error::FocusNotObserved {
                timeout_ms,
                expected: "AX frame for helper window".into(),
            });
        }
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
        p0.x + config::WINDOW_POSITION_OFFSET
    };

    // Drive: h -> o (hide on)
    send_key("h");
    thread::sleep(config::ms(config::UI_ACTION_DELAY_MS));
    send_key("o");

    // Wait for position change
    let mut moved = false;
    let deadline = Instant::now() + Duration::from_millis(cmp::max(config::HIDE_MIN_TIMEOUT_MS, timeout_ms / 4));
    let mut _p_on = p0;
    while Instant::now() < deadline {
        if let Some(w) = ax_first_window_for_pid(pid)
            && let Some(p) = ax_get_point(w, cfstr("AXPosition"))
        {
            _p_on = p;
            if !approx(p.x, p0.x, 2.0) || approx(p.x, target_x, 6.0) {
                moved = true;
                break;
            }
        }
        thread::sleep(config::ms(config::KEY_EVENT_DELAY_MS));
    }
    if !moved {
        eprintln!(
            "debug: no movement detected after hide(on). last vs start x: {:.1} -> {:.1}",
            _p_on.x, p0.x
        );
        // Cleanup session (helper cleans up automatically via Drop)
        sess.shutdown();
        sess.kill_and_wait();
        return Err(Error::SpawnFailed(
            "window position did not change after hide(on)".into(),
        ));
    }

    // Drive: reopen HUD if needed and turn hide off (reveal)
    thread::sleep(config::ms(config::UI_STABILIZE_DELAY_MS));
    send_key("shift+cmd+0");
    thread::sleep(config::ms(config::UI_ACTION_DELAY_MS));
    send_key("h");
    thread::sleep(config::ms(config::UI_ACTION_DELAY_MS));
    send_key("f");

    // Wait until position roughly returns to original
    let mut restored = false;
    let deadline2 = Instant::now() + Duration::from_millis(cmp::max(config::HIDE_SECONDARY_MIN_TIMEOUT_MS, timeout_ms / 3));
    while Instant::now() < deadline2 {
        if let Some(w) = ax_first_window_for_pid(pid)
            && let Some(p2) = ax_get_point(w, cfstr("AXPosition"))
            && let Some(s2) = ax_get_size(w, cfstr("AXSize"))
        {
            let pos_ok = approx(p2.x, p0.x, 8.0) && approx(p2.y, p0.y, 8.0);
            let size_ok = approx(s2.width, s0.width, 8.0) && approx(s2.height, s0.height, 8.0);
            // quiet on success path
            if pos_ok && size_ok {
                restored = true;
                break;
            }
        }
        thread::sleep(config::ms(config::POLL_INTERVAL_MS + 30));
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
