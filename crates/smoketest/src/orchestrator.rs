//! Test orchestration and execution logic.

use std::io::Read;
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

/// Run a child `smoketest` subcommand with a watchdog, capturing output and forcing quiet mode.
/// Returns (success, stderr + newline + stdout) for error details.
fn run_subtest_capture(subcmd: &str, duration_ms: u64, timeout_ms: u64) -> (bool, String) {
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            return (
                false,
                format!("orchestrator: failed to resolve current_exe: {}", e),
            );
        }
    };
    let args: Vec<String> = vec![
        "--quiet".into(),
        "--duration".into(),
        duration_ms.to_string(),
        "--timeout".into(),
        timeout_ms.to_string(),
        subcmd.into(),
    ];

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
    let max_wait = Duration::from_millis(timeout_ms).saturating_add(overhead);
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
pub fn run_all_tests(duration_ms: u64, timeout_ms: u64, _logs: bool, warn_overlay: bool) {
    let mut all_ok = true;

    // Optionally show the hands-off overlay for the entire run
    let mut overlay: Option<crate::process::ManagedChild> = None;
    if warn_overlay && let Ok(child) = crate::process::spawn_warn_overlay() {
        overlay = Some(child);
        std::thread::sleep(std::time::Duration::from_millis(
            crate::config::WARN_OVERLAY_INITIAL_DELAY_MS,
        ));
    }

    // Helper to run + print one-line summary
    let mut run = |name: &str, dur: u64| {
        let (ok, details) = run_subtest_capture(name, dur, timeout_ms);
        if ok {
            println!("{}... OK", name);
        } else {
            all_ok = false;
            println!("{}... FAIL", name);
            if !details.trim().is_empty() {
                println!("{}", details.trim_end());
            }
        }
    };

    // Repeat tests
    run("repeat-relay", duration_ms);
    run("repeat-shell", duration_ms);
    let vol_duration = std::cmp::max(duration_ms, config::MIN_VOLUME_TEST_DURATION_MS);
    run("repeat-volume", vol_duration);

    // Focus and window ops
    run("focus", duration_ms);
    run("raise", duration_ms);
    run("place", duration_ms);
    run("fullscreen", duration_ms);

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
        SeqTest::Focus => "focus",
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
        if !run_subtest_with_watchdog(name, dur, timeout_ms, logs) {
            std::process::exit(1);
        }
    }
    println!("Selected smoketests passed");
}
