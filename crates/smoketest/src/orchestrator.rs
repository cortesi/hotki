//! Test orchestration and execution logic.

use std::{
    io::Read,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use serde::Serialize;

use crate::{cli::SeqTest, config};

/// Print a test heading to stdout.
pub fn heading(title: &str) {
    println!("\n==> {}", title);
}

/// Run a child `smoketest` subcommand with a watchdog.
/// Returns true on success, false on failure/timeout.
fn run_subtest_with_watchdog(
    subcmd: &str,
    duration_ms: u64,
    timeout_ms: u64,
    logs: bool,
    extra_args: &[String],
) -> bool {
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("orchestrator: failed to resolve current_exe: {}", e);
            return false;
        }
    };
    let mut args: Vec<String> = vec![
        "--duration".into(),
        duration_ms.to_string(),
        "--timeout".into(),
        timeout_ms.to_string(),
    ];
    if logs {
        args.push("--logs".into());
    }
    args.push(subcmd.into());
    args.extend(extra_args.iter().cloned());

    let mut child = match Command::new(exe)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("orchestrator: failed to spawn subtest '{}': {}", subcmd, e);
            return false;
        }
    };

    // Watchdog: allow some overhead on top of the configured timeout.
    let overhead = Duration::from_millis(15_000);
    let max_wait = Duration::from_millis(timeout_ms).saturating_add(overhead);
    let deadline = Instant::now() + max_wait;

    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    eprintln!(
                        "orchestrator: watchdog timeout ({} ms + overhead) for subtest '{}'",
                        timeout_ms, subcmd
                    );
                    return false;
                }
                std::thread::sleep(Duration::from_millis(config::RETRY_DELAY_MS));
            }
            Err(e) => {
                eprintln!("orchestrator: error waiting for '{}': {}", subcmd, e);
                return false;
            }
        }
    }
}

/// Run a child `smoketest` subcommand with a watchdog, capturing output and forcing quiet mode.
/// Returns (success, stderr + newline + stdout) for error details.
fn run_subtest_capture(
    subcmd: &str,
    duration_ms: u64,
    timeout_ms: u64,
    extra_args: &[String],
) -> (bool, String) {
    run_subtest_capture_with_extra(subcmd, duration_ms, timeout_ms, 0, extra_args)
}

/// Like `run_subtest_capture` but allows adding extra watchdog headroom in milliseconds
/// for slower cases (e.g., focus-nav), without inflating the base timeout at call sites.
fn run_subtest_capture_with_extra(
    subcmd: &str,
    duration_ms: u64,
    timeout_ms: u64,
    extra_watchdog_ms: u64,
    extra_args: &[String],
) -> (bool, String) {
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            return (
                false,
                format!("orchestrator: failed to resolve current_exe: {}", e),
            );
        }
    };
    // In orchestrated runs, a single overlay is spawned by the parent.
    // Disable overlays in subtests to avoid multiple top-most windows.
    let mut args: Vec<String> = vec![
        "--quiet".into(),
        "--no-warn".into(),
        "--duration".into(),
        duration_ms.to_string(),
        "--timeout".into(),
        timeout_ms.to_string(),
    ];
    args.push(subcmd.into());
    args.extend(extra_args.iter().cloned());

    let mut child = match Command::new(exe)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            return (
                false,
                format!("orchestrator: failed to spawn subtest '{}': {}", subcmd, e),
            );
        }
    };

    // Watchdog: allow some overhead on top of the configured timeout.
    let overhead = Duration::from_millis(15_000);
    let extra_overhead = Duration::from_millis(extra_watchdog_ms);
    let max_wait = Duration::from_millis(timeout_ms)
        .saturating_add(overhead)
        .saturating_add(extra_overhead);
    let deadline = Instant::now() + max_wait;

    // Poll for completion or timeout
    let success = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.success(),
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    break false;
                }
                std::thread::sleep(Duration::from_millis(config::RETRY_DELAY_MS));
            }
            Err(_) => break false,
        }
    };

    // Gather outputs
    let mut stderr = String::new();
    let mut stdout = String::new();
    if let Some(mut s) = child.stderr.take() {
        let _ = s.read_to_string(&mut stderr);
    }
    if let Some(mut s) = child.stdout.take() {
        let _ = s.read_to_string(&mut stdout);
    }
    let details = if stderr.trim().is_empty() {
        stdout
    } else if stdout.trim().is_empty() {
        stderr
    } else {
        format!("{}\n{}", stderr, stdout)
    };
    (success, details)
}

/// Run all smoketests sequentially in isolated subprocesses.
#[derive(Serialize, Copy, Clone)]
struct PlaceFlexSettings {
    cols: u32,
    rows: u32,
    col: u32,
    row: u32,
    force_size_pos: bool,
    pos_first_only: bool,
    #[serde(default)]
    force_shrink_move_grow: bool,
}

/// Convert a `PlaceFlexSettings` value to CLI args for the `place-flex` subcommand.
fn place_flex_args(cfg: &PlaceFlexSettings) -> Vec<String> {
    let mut args = vec![
        "place-flex".to_string(),
        "--cols".into(),
        cfg.cols.to_string(),
        "--rows".into(),
        cfg.rows.to_string(),
        "--col".into(),
        cfg.col.to_string(),
        "--row".into(),
        cfg.row.to_string(),
    ];
    if cfg.force_size_pos {
        args.push("--force-size-pos".into());
    }
    if cfg.pos_first_only {
        args.push("--pos-first-only".into());
    }
    if cfg.force_shrink_move_grow {
        args.push("--force-shrink-move-grow".into());
    }
    args
}

pub fn run_all_tests(duration_ms: u64, timeout_ms: u64, _logs: bool, warn_overlay: bool) {
    let mut all_ok = true;

    // Optionally show the hands-off overlay for the entire run
    let mut overlay: Option<crate::process::ManagedChild> = None;
    if warn_overlay && let Ok(child) = crate::process::spawn_warn_overlay() {
        overlay = Some(child);
        std::thread::sleep(std::time::Duration::from_millis(
            crate::config::WARN_OVERLAY_INITIAL_DELAY_MS,
        ));
        // Initialize overlay title
        crate::process::write_overlay_status("Preparing tests...");
    }

    // Helper to run + print one-line summary
    let run = |name: &str, dur: u64| -> bool {
        // Update overlay title to current test
        crate::process::write_overlay_status(name);
        // Clear info by default unless a variant sets it explicitly
        crate::process::write_overlay_info("");
        let (ok, details) = run_subtest_capture(name, dur, timeout_ms, &[]);
        if ok {
            println!("{}... OK", name);
            true
        } else {
            println!("{}... FAIL", name);
            if !details.trim().is_empty() {
                println!("{}", details.trim_end());
            }
            false
        }
    };

    // Quick diagnostics: verify world status/permissions first
    all_ok &= run("world-status", duration_ms);
    // Verify World AX props path as a quick sanity check.
    all_ok &= run("world-ax", duration_ms);

    // Repeat tests
    all_ok &= run("repeat-relay", duration_ms);
    all_ok &= run("repeat-shell", duration_ms);
    let vol_duration = std::cmp::max(duration_ms, config::MIN_VOLUME_TEST_DURATION_MS);
    all_ok &= run("repeat-volume", vol_duration);

    // Focus and window ops
    all_ok &= run("focus-tracking", duration_ms);
    all_ok &= run("raise", duration_ms);
    // Focus-nav can be a bit slower due to AX+IPC; add small extra watchdog headroom.
    {
        let name = "focus-nav";
        crate::process::write_overlay_status(name);
        crate::process::write_overlay_info("");
        let (ok, details) = run_subtest_capture_with_extra(
            name,
            duration_ms,
            timeout_ms.saturating_add(10_000),
            10_000,
            &[],
        );
        if ok {
            println!("{}... OK", name);
        } else {
            println!("{}... FAIL", name);
            if !details.trim().is_empty() {
                println!("{}", details.trim_end());
            }
        }
    }
    all_ok &= run("place", duration_ms);
    // place-minimized can be slower on some hosts after de-miniaturize; add small extra headroom.
    {
        let name = "place-minimized";
        crate::process::write_overlay_status(name);
        crate::process::write_overlay_info("");
        let (ok, details) =
            run_subtest_capture_with_extra(name, duration_ms, timeout_ms, 10_000, &[]);
        if ok {
            println!("{}... OK", name);
        } else {
            println!("{}... FAIL", name);
            if !details.trim().is_empty() {
                println!("{}", details.trim_end());
            }
        }
        all_ok &= ok;
    }
    all_ok &= run("place-zoomed", duration_ms);
    // Stage 6 advisory gating: validate skip behavior when possible
    all_ok &= run("place-skip", duration_ms);
    // Stage 6 advisory gating (focused): available as a separate subcommand `place-skip`.
    // Stage‑3/8 variants via place-flex
    // Encode variants once and iterate to reduce repetition.
    const PLACE_FLEX_VARIANTS: &[(PlaceFlexSettings, &str)] = &[
        // 2x2 BR cell, force size->pos
        (
            PlaceFlexSettings {
                cols: 2,
                rows: 2,
                col: 1,
                row: 1,
                force_size_pos: true,
                pos_first_only: false,
                force_shrink_move_grow: false,
            },
            "2x2 BR, force size->pos",
        ),
        // Default grid TL cell, pos-first-only
        (
            PlaceFlexSettings {
                cols: config::PLACE_COLS,
                rows: config::PLACE_ROWS,
                col: 0,
                row: 0,
                force_size_pos: false,
                pos_first_only: true,
                force_shrink_move_grow: false,
            },
            "TL, pos-first-only",
        ),
        // Default grid BL cell, normal path
        (
            PlaceFlexSettings {
                cols: config::PLACE_COLS,
                rows: config::PLACE_ROWS,
                col: 0,
                row: 1,
                force_size_pos: false,
                pos_first_only: false,
                force_shrink_move_grow: false,
            },
            "BL, normal",
        ),
        // 2x2 BR cell, force shrink->move->grow fallback
        (
            PlaceFlexSettings {
                cols: 2,
                rows: 2,
                col: 1,
                row: 1,
                force_size_pos: false,
                pos_first_only: false,
                force_shrink_move_grow: true,
            },
            "2x2 BR, force shrink->move->grow",
        ),
    ];

    for (cfg, info) in PLACE_FLEX_VARIANTS.iter().copied() {
        crate::process::write_overlay_status("place-flex");
        crate::process::write_overlay_info(info);
        let args = place_flex_args(&cfg);
        let json = serde_json::to_string(&cfg).unwrap_or_default();
        let (ok, details) = run_subtest_capture("place-flex", duration_ms, timeout_ms, &args[1..]);
        if ok {
            println!("place-flex... OK\n  settings: {}\n  info: {}", json, info);
        } else {
            println!("place-flex... FAIL\n  settings: {}\n  info: {}", json, info);
            if !details.trim().is_empty() {
                println!("{}", details.trim_end());
            }
        }
        all_ok &= ok;
    }
    all_ok &= run("fullscreen", duration_ms);

    // UI demos
    run("ui", duration_ms);
    run("minui", duration_ms);

    if let Some(mut c) = overlay {
        let _ = c.kill_and_wait();
    }
    if !all_ok {
        std::process::exit(1);
    }
}

fn to_subcmd(t: SeqTest) -> &'static str {
    match t {
        SeqTest::RepeatRelay => "repeat-relay",
        SeqTest::RepeatShell => "repeat-shell",
        SeqTest::RepeatVolume => "repeat-volume",
        SeqTest::Focus => "focus-tracking",
        SeqTest::Raise => "raise",
        SeqTest::Hide => "hide",
        SeqTest::Place => "place",
        SeqTest::Fullscreen => "fullscreen",
        SeqTest::Ui => "ui",
        SeqTest::Minui => "minui",
    }
}

/// Run a listed sequence of smoketests in isolated subprocesses.
pub fn run_sequence_tests(tests: &[SeqTest], duration_ms: u64, timeout_ms: u64, logs: bool) {
    if tests.is_empty() {
        eprintln!("no tests provided (use: smoketest seq <tests...>)");
        std::process::exit(2);
    }
    for t in tests.iter().copied() {
        let name = to_subcmd(t);
        heading(&format!("Test: {}", name));
        let dur = if matches!(t, SeqTest::RepeatVolume) {
            std::cmp::max(duration_ms, config::MIN_VOLUME_TEST_DURATION_MS)
        } else {
            duration_ms
        };
        if !run_subtest_with_watchdog(name, dur, timeout_ms, logs, &[]) {
            std::process::exit(1);
        }
    }
    println!("Selected smoketests passed");
}
