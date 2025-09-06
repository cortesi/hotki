//! Test orchestration and execution logic.

use crate::{config, error::print_hints, logging, tests::*};
use std::process::Command;

/// Print a test heading to stdout.
pub fn heading(title: &str) {
    println!("\n==> {}", title);
}

/// Run preflight checks for permissions and capabilities.
pub fn run_preflight() -> bool {
    // Accessibility and Input Monitoring via permissions crate
    let p = permissions::check_permissions();
    println!(
        "permissions: accessibility={}, input_monitoring={}",
        p.accessibility_ok, p.input_ok
    );

    // Screen Recording via screencapture
    use std::ffi::OsStr;
    let tmp = std::env::temp_dir().join(format!(
        "hotki-smoketest-preflight-{}-{}.png",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let status = Command::new("screencapture")
        .args([
            OsStr::new("-x"),
            OsStr::new("-R"),
            OsStr::new("0,0,1,1"),
            tmp.as_os_str(),
        ])
        .status();
    let mut screen_ok = false;
    if let Ok(st) = status {
        screen_ok = st.success();
    }
    let _ = std::fs::remove_file(&tmp);
    println!("screen_recording: {}", screen_ok);

    if !p.accessibility_ok {
        eprintln!(
            "hint: grant Accessibility permission to your terminal under System Settings → Privacy & Security → Accessibility"
        );
    }
    if !p.input_ok {
        eprintln!(
            "hint: grant Input Monitoring permission to your terminal under System Settings → Privacy & Security → Input Monitoring"
        );
    }
    if !screen_ok {
        eprintln!(
            "hint: grant Screen Recording permission under System Settings → Privacy & Security → Screen Recording"
        );
    }
    p.accessibility_ok && p.input_ok && screen_ok
}

/// Run all smoketests sequentially.
pub fn run_all_tests(duration_ms: u64, timeout_ms: u64) {
    // Repeat tests: run sequentially; print result immediately after each
    heading("Test: repeat-relay");
    let relay = count_relay(duration_ms);
    if relay < 3 {
        eprintln!("FAIL repeat-relay: {} repeats (< 3)", relay);
        logging::events::test_failure("repeat-relay", format!("Only {} repeats (< 3)", relay));
        std::process::exit(1);
    } else {
        println!("repeat-relay: {} repeats", relay);
    }

    heading("Test: repeat-shell");
    let shell = count_shell(duration_ms);
    if shell < 3 {
        eprintln!("FAIL repeat-shell: {} repeats (< 3)", shell);
        logging::events::test_failure("repeat-shell", format!("Only {} repeats (< 3)", shell));
        std::process::exit(1);
    } else {
        println!("repeat-shell: {} repeats", shell);
    }

    heading("Test: repeat-volume");
    let volume = count_volume(std::cmp::max(
        duration_ms,
        config::MIN_VOLUME_TEST_DURATION_MS,
    ));
    if volume < 3 {
        eprintln!("FAIL repeat-volume: {} repeats (< 3)", volume);
        logging::events::test_failure("repeat-volume", format!("Only {} repeats (< 3)", volume));
        std::process::exit(1);
    } else {
        println!("repeat-volume: {} repeats", volume);
    }

    // Focus test: verify engine observes a frontmost window title change
    heading("Test: focus");
    match focus::run_focus_test(timeout_ms, false) {
        Ok(out) => println!(
            "focus: OK (title='{}', pid={}, time_to_match_ms={})",
            out.title, out.pid, out.elapsed_ms
        ),
        Err(e) => {
            eprintln!("focus: ERROR: {}", e);
            logging::events::test_failure("focus", &e);
            print_hints(&e);
            std::process::exit(1);
        }
    }

    // Raise test: verify raise by title twice
    heading("Test: raise");
    match raise::run_raise_test(timeout_ms, false) {
        Ok(()) => println!("raise: OK (raised by title twice)"),
        Err(e) => {
            eprintln!("raise: ERROR: {}", e);
            logging::events::test_failure("raise", &e);
            print_hints(&e);
            std::process::exit(1);
        }
    }

    // UI demos: ensure HUD appears and basic theme cycling works (ui + miniui)
    heading("Test: ui");
    match ui::run_ui_demo(timeout_ms) {
        Ok(s) => println!(
            "ui: OK (hud_seen={}, time_to_hud_ms={:?})",
            s.hud_seen, s.time_to_hud_ms
        ),
        Err(e) => {
            eprintln!("ui: ERROR: {}", e);
            logging::events::test_failure("ui_demo", &e);
            print_hints(&e);
            std::process::exit(1);
        }
    }

    heading("Test: minui");
    match ui::run_minui_demo(timeout_ms) {
        Ok(s) => println!(
            "minui: OK (hud_seen={}, time_to_hud_ms={:?})",
            s.hud_seen, s.time_to_hud_ms
        ),
        Err(e) => {
            eprintln!("minui: ERROR: {}", e);
            logging::events::test_failure("minui_demo", &e);
            print_hints(&e);
            std::process::exit(1);
        }
    }

    println!("All smoketests passed");
}
