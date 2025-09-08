//! Test orchestration and execution logic.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::cli::SeqTest;
use crate::config;

/// Print a test heading to stdout.
pub fn heading(title: &str) {
    println!("\n==> {}", title);
}

/// Run a child `smoketest` subcommand with a watchdog.
/// Returns true on success, false on failure/timeout.
fn run_subtest_with_watchdog(subcmd: &str, duration_ms: u64, timeout_ms: u64, logs: bool) -> bool {
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

/// Run all smoketests sequentially in isolated subprocesses.
pub fn run_all_tests(duration_ms: u64, timeout_ms: u64, logs: bool) {
    // Repeat tests
    heading("Test: repeat-relay");
    if !run_subtest_with_watchdog("repeat-relay", duration_ms, timeout_ms, logs) {
        std::process::exit(1);
    }

    heading("Test: repeat-shell");
    if !run_subtest_with_watchdog("repeat-shell", duration_ms, timeout_ms, logs) {
        std::process::exit(1);
    }

    heading("Test: repeat-volume");
    let vol_duration = std::cmp::max(duration_ms, config::MIN_VOLUME_TEST_DURATION_MS);
    if !run_subtest_with_watchdog("repeat-volume", vol_duration, timeout_ms, logs) {
        std::process::exit(1);
    }

    // Focus and window ops
    heading("Test: focus");
    if !run_subtest_with_watchdog("focus", duration_ms, timeout_ms, logs) {
        std::process::exit(1);
    }

    heading("Test: raise");
    if !run_subtest_with_watchdog("raise", duration_ms, timeout_ms, logs) {
        std::process::exit(1);
    }

    // UI demos
    heading("Test: ui");
    if !run_subtest_with_watchdog("ui", duration_ms, timeout_ms, logs) {
        std::process::exit(1);
    }

    heading("Test: minui");
    if !run_subtest_with_watchdog("minui", duration_ms, timeout_ms, logs) {
        std::process::exit(1);
    }

    println!("All smoketests passed");
}

fn to_subcmd(t: SeqTest) -> &'static str {
    match t {
        SeqTest::RepeatRelay => "repeat-relay",
        SeqTest::RepeatShell => "repeat-shell",
        SeqTest::RepeatVolume => "repeat-volume",
        SeqTest::Focus => "focus",
        SeqTest::Raise => "raise",
        SeqTest::Hide => "hide",
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
        if !run_subtest_with_watchdog(name, dur, timeout_ms, logs) {
            std::process::exit(1);
        }
    }
    println!("Selected smoketests passed");
}
