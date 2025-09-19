//! Test orchestration and execution logic.

#[cfg(test)]
use std::sync::{Mutex, OnceLock};
use std::{
    cmp, env,
    io::{self, Read},
    path::PathBuf,
    process as std_process,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use serde::Serialize;

use crate::{cli::SeqTest, config, helper_window::ManagedChild, process};

/// Print a test heading to stdout.
pub fn heading(title: &str) {
    println!("\n==> {}", title);
}

/// How the orchestrator should manage stdio for a subtest process.
#[derive(Copy, Clone)]
enum SubtestIo {
    /// Leave stdout/stderr attached to the parent and optionally enable log streaming.
    Inherit {
        /// Whether to append `--logs` to the child invocation.
        logs: bool,
    },
    /// Capture stdout/stderr while forcing the child into quiet/no-warn mode.
    CaptureQuiet,
}

impl SubtestIo {
    /// Returns true when the child should inherit stdio handles from the parent.
    fn inherits_stdio(&self) -> bool {
        matches!(self, Self::Inherit { .. })
    }

    /// Returns true when output should be captured instead of inherited.
    fn quiet_capture(&self) -> bool {
        matches!(self, Self::CaptureQuiet)
    }

    /// Returns true when the `--logs` flag should be supplied.
    fn logs_enabled(&self) -> bool {
        matches!(self, Self::Inherit { logs: true })
    }
}

#[cfg(test)]
fn forced_exe_slot() -> &'static Mutex<Option<PathBuf>> {
    static SLOT: OnceLock<Mutex<Option<PathBuf>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

#[cfg(test)]
fn forced_overhead_slot() -> &'static Mutex<Option<u64>> {
    static SLOT: OnceLock<Mutex<Option<u64>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

/// Resolve the smoketest executable path, honoring test overrides when set.
fn resolve_smoketest_executable() -> io::Result<PathBuf> {
    #[cfg(test)]
    if let Some(path) = forced_exe_slot().lock().unwrap().clone() {
        return Ok(path);
    }
    env::current_exe()
}

/// Base watchdog overhead duration, optionally overridden for tests.
fn watchdog_base_overhead() -> Duration {
    #[cfg(test)]
    if let Some(ms) = *forced_overhead_slot().lock().unwrap() {
        return Duration::from_millis(ms);
    }
    Duration::from_millis(15_000)
}

#[cfg(test)]
fn set_forced_executable(path: PathBuf) {
    *forced_exe_slot().lock().unwrap() = Some(path);
}

#[cfg(test)]
fn clear_forced_executable() {
    *forced_exe_slot().lock().unwrap() = None;
}

#[cfg(test)]
fn set_watchdog_overhead_for_tests(ms: u64) {
    *forced_overhead_slot().lock().unwrap() = Some(ms);
}

#[cfg(test)]
fn clear_watchdog_overhead_override() {
    *forced_overhead_slot().lock().unwrap() = None;
}

/// Result from running a subtest under the watchdog.
struct SubtestOutcome {
    /// Whether the child exited successfully.
    success: bool,
    /// Captured stdout payload; empty when inheriting stdio or when no data was emitted.
    stdout: String,
    /// Captured stderr payload; empty when inheriting stdio or when no data was emitted.
    stderr: String,
}

/// Pretty-print captured stdout/stderr for a failed run, preserving stream boundaries.
fn print_captured_streams(outcome: &SubtestOutcome) {
    let stderr = outcome.stderr.trim_end();
    let stdout = outcome.stdout.trim_end();
    if stderr.is_empty() && stdout.is_empty() {
        return;
    }
    if !stderr.is_empty() {
        println!("--- stderr ---\n{}", stderr);
    }
    if !stdout.is_empty() {
        println!("--- stdout ---\n{}", stdout);
    }
}

/// Track a failed smoketest and update fail-fast state.
fn record_failure(
    failures: &mut Vec<String>,
    stop_early: &mut bool,
    fail_fast: bool,
    name: impl Into<String>,
) {
    failures.push(name.into());
    if fail_fast {
        *stop_early = true;
    }
}

/// Execute a smoketest subcommand with standard overlay updates and reporting.
fn run_basic_test(
    name: &str,
    duration_ms: u64,
    timeout_ms: u64,
    failures: &mut Vec<String>,
    stop_early: &mut bool,
    fail_fast: bool,
) -> bool {
    if *stop_early {
        return true;
    }
    process::write_overlay_status(name);
    process::write_overlay_info("");
    let start = Instant::now();
    let outcome = run_subtest_capture(name, duration_ms, timeout_ms, &[]);
    let elapsed = start.elapsed();
    if outcome.success {
        println!("{}... OK ({:.3}s)", name, elapsed.as_secs_f64());
    } else {
        println!("{}... FAIL ({:.3}s)", name, elapsed.as_secs_f64());
        print_captured_streams(&outcome);
        record_failure(failures, stop_early, fail_fast, name);
    }
    outcome.success
}

/// Launch and supervise a smoketest child process with the requested stdio policy.
fn run_subtest(
    subcmd: &str,
    duration_ms: u64,
    timeout_ms: u64,
    extra_watchdog_ms: u64,
    extra_args: &[String],
    io: SubtestIo,
) -> SubtestOutcome {
    let exe = match resolve_smoketest_executable() {
        Ok(p) => p,
        Err(e) => {
            let msg = format!("orchestrator: failed to resolve current_exe: {}", e);
            if io.inherits_stdio() {
                eprintln!("{}", msg);
            }
            return SubtestOutcome {
                success: false,
                stdout: String::new(),
                stderr: msg,
            };
        }
    };

    let mut args: Vec<String> = Vec::new();
    if io.quiet_capture() {
        args.push("--quiet".into());
        args.push("--no-warn".into());
    }
    args.push("--duration".into());
    args.push(duration_ms.to_string());
    args.push("--timeout".into());
    args.push(timeout_ms.to_string());
    if io.logs_enabled() {
        args.push("--logs".into());
    }
    args.push(subcmd.into());
    args.extend(extra_args.iter().cloned());

    let mut command = Command::new(exe);
    command.args(&args).stdin(Stdio::null());
    if io.inherits_stdio() {
        command.stdout(Stdio::inherit()).stderr(Stdio::inherit());
    } else {
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
    }

    let mut child = match command.spawn() {
        Ok(c) => c,
        Err(e) => {
            let msg = format!("orchestrator: failed to spawn subtest '{}': {}", subcmd, e);
            if io.inherits_stdio() {
                eprintln!("{}", msg);
            }
            return SubtestOutcome {
                success: false,
                stdout: String::new(),
                stderr: msg,
            };
        }
    };

    let overhead = watchdog_base_overhead();
    let extra_overhead = Duration::from_millis(extra_watchdog_ms);
    let max_wait = Duration::from_millis(timeout_ms)
        .saturating_add(overhead)
        .saturating_add(extra_overhead);
    let deadline = Instant::now() + max_wait;

    let success = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.success(),
            Ok(None) => {
                if Instant::now() >= deadline {
                    if let Err(e) = child.kill() {
                        eprintln!("orchestrator: failed to kill timed-out child: {}", e);
                    }
                    if let Err(e) = child.wait() {
                        eprintln!("orchestrator: failed to reap timed-out child: {}", e);
                    }
                    if io.inherits_stdio() {
                        eprintln!(
                            "orchestrator: watchdog timeout ({} ms + overhead) for subtest '{}'",
                            timeout_ms, subcmd
                        );
                    }
                    break false;
                }
                thread::sleep(Duration::from_millis(config::INPUT_DELAYS.retry_delay_ms));
            }
            Err(e) => {
                if io.inherits_stdio() {
                    eprintln!("orchestrator: error waiting for '{}': {}", subcmd, e);
                }
                break false;
            }
        }
    };

    if io.quiet_capture() {
        let mut stderr = String::new();
        let mut stdout = String::new();
        if let Some(mut s) = child.stderr.take()
            && let Err(e) = s.read_to_string(&mut stderr)
        {
            eprintln!("orchestrator: failed reading stderr: {}", e);
        }
        if let Some(mut s) = child.stdout.take()
            && let Err(e) = s.read_to_string(&mut stdout)
        {
            eprintln!("orchestrator: failed reading stdout: {}", e);
        }
        SubtestOutcome {
            success,
            stdout,
            stderr,
        }
    } else {
        SubtestOutcome {
            success,
            stdout: String::new(),
            stderr: String::new(),
        }
    }
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
    run_subtest(
        subcmd,
        duration_ms,
        timeout_ms,
        0,
        extra_args,
        SubtestIo::Inherit { logs },
    )
    .success
}

/// Run a child `smoketest` subcommand with a watchdog, capturing output and forcing quiet mode.
/// Returns the outcome alongside separated stdout/stderr payloads.
fn run_subtest_capture(
    subcmd: &str,
    duration_ms: u64,
    timeout_ms: u64,
    extra_args: &[String],
) -> SubtestOutcome {
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
) -> SubtestOutcome {
    run_subtest(
        subcmd,
        duration_ms,
        timeout_ms,
        extra_watchdog_ms,
        extra_args,
        SubtestIo::CaptureQuiet,
    )
}

/// Run all smoketests sequentially in isolated subprocesses.
#[derive(Serialize, Copy, Clone)]
struct PlaceFlexSettings {
    /// Grid columns
    cols: u32,
    /// Grid rows
    rows: u32,
    /// Target column (0-based)
    col: u32,
    /// Target row (0-based)
    row: u32,
    /// Force size->pos fallback even if pos->size succeeds
    force_size_pos: bool,
    /// Disable size->pos fallback; only attempt pos->size
    pos_first_only: bool,
    /// Force shrink->move->grow fallback even if other attempts succeed
    #[serde(default)]
    force_shrink_move_grow: bool,
}

// Variants for the place-flex sweep, with a human-readable info string.
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
            cols: config::PLACE.grid_cols,
            rows: config::PLACE.grid_rows,
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
            cols: config::PLACE.grid_cols,
            rows: config::PLACE.grid_rows,
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

/// Run all smoketests sequentially with basic reporting; exits with non-zero
/// status on failure.
pub fn run_all_tests(
    duration_ms: u64,
    timeout_ms: u64,
    _logs: bool,
    warn_overlay: bool,
    fake_mode: bool,
    fail_fast: bool,
) {
    // Optionally show the hands-off overlay for the entire run
    let mut overlay: Option<ManagedChild> = None;
    if warn_overlay
        && !fake_mode
        && let Ok(child) = process::spawn_warn_overlay()
    {
        overlay = Some(child);
        thread::sleep(Duration::from_millis(config::WARN_OVERLAY.initial_delay_ms));
        // Initialize overlay title
        process::write_overlay_status("Preparing tests...");
    }

    let mut failures: Vec<String> = Vec::new();
    let mut stop_early = false;

    let finalize = |overlay: &mut Option<ManagedChild>, failures: &Vec<String>| {
        if let Some(mut c) = overlay.take()
            && let Err(e) = c.kill_and_wait()
        {
            eprintln!("orchestrator: failed to stop overlay: {}", e);
        }
        if failures.is_empty() {
            return;
        }
        println!("\nFailed smoketests:");
        for name in failures {
            println!("- {}", name);
        }
        std_process::exit(1);
    };

    if fake_mode {
        println!("Running fake placement smoke (GUI unavailable)");
        run_basic_test(
            "place-fake",
            duration_ms,
            timeout_ms,
            &mut failures,
            &mut stop_early,
            fail_fast,
        );
        finalize(&mut overlay, &failures);
        return;
    }

    // Quick diagnostics: verify world status/permissions first
    run_basic_test(
        "world-status",
        duration_ms,
        timeout_ms,
        &mut failures,
        &mut stop_early,
        fail_fast,
    );
    // Verify World AX props path as a quick sanity check.
    run_basic_test(
        "world-ax",
        duration_ms,
        timeout_ms,
        &mut failures,
        &mut stop_early,
        fail_fast,
    );
    run_basic_test(
        "world-spaces",
        duration_ms,
        timeout_ms,
        &mut failures,
        &mut stop_early,
        fail_fast,
    );

    // Repeat tests
    run_basic_test(
        "repeat-relay",
        duration_ms,
        timeout_ms,
        &mut failures,
        &mut stop_early,
        fail_fast,
    );
    run_basic_test(
        "repeat-shell",
        duration_ms,
        timeout_ms,
        &mut failures,
        &mut stop_early,
        fail_fast,
    );
    let vol_duration = cmp::max(duration_ms, config::DEFAULTS.min_volume_duration_ms);
    run_basic_test(
        "repeat-volume",
        vol_duration,
        timeout_ms,
        &mut failures,
        &mut stop_early,
        fail_fast,
    );

    // Focus and window ops
    run_basic_test(
        "focus-tracking",
        duration_ms,
        timeout_ms,
        &mut failures,
        &mut stop_early,
        fail_fast,
    );
    run_basic_test(
        "raise",
        duration_ms,
        timeout_ms,
        &mut failures,
        &mut stop_early,
        fail_fast,
    );
    // Focus-nav can be a bit slower due to AX+IPC; add small extra watchdog headroom.
    if !stop_early {
        let name = "focus-nav";
        process::write_overlay_status(name);
        process::write_overlay_info("");
        let start = Instant::now();
        let outcome = run_subtest_capture_with_extra(
            name,
            duration_ms,
            timeout_ms.saturating_add(10_000),
            10_000,
            &[],
        );
        let elapsed = start.elapsed();
        if outcome.success {
            println!("{}... OK ({:.3}s)", name, elapsed.as_secs_f64());
        } else {
            println!("{}... FAIL ({:.3}s)", name, elapsed.as_secs_f64());
            print_captured_streams(&outcome);
            record_failure(&mut failures, &mut stop_early, fail_fast, name);
        }
    }
    run_basic_test(
        "place",
        duration_ms,
        timeout_ms,
        &mut failures,
        &mut stop_early,
        fail_fast,
    );
    // Async placement helper: exercises delayed-apply behavior (~50ms) and
    // verifies the engine converges via settle polling. Keep near the other
    // placement cases so failures are easier to triage.
    run_basic_test(
        "place-async",
        duration_ms,
        timeout_ms,
        &mut failures,
        &mut stop_early,
        fail_fast,
    );
    // Animated placement helper: exercises tweened setFrame behavior (~120ms).
    run_basic_test(
        "place-animated",
        duration_ms,
        timeout_ms,
        &mut failures,
        &mut stop_early,
        fail_fast,
    );
    // Increments placement: simulate terminal-style resize increments and verify
    // anchor-legal-size behavior keeps cell edges flush.
    run_basic_test(
        "place-increments",
        duration_ms,
        timeout_ms,
        &mut failures,
        &mut stop_early,
        fail_fast,
    );
    // Terminal guard: ensure no position thrash after origin latch under increments.
    run_basic_test(
        "place-term",
        duration_ms,
        timeout_ms,
        &mut failures,
        &mut stop_early,
        fail_fast,
    );
    run_basic_test(
        "place-move-min",
        duration_ms,
        timeout_ms,
        &mut failures,
        &mut stop_early,
        fail_fast,
    );
    run_basic_test(
        "place-move-nonresizable",
        duration_ms,
        timeout_ms,
        &mut failures,
        &mut stop_early,
        fail_fast,
    );
    // place-minimized can be slower on some hosts after de-miniaturize; add small extra headroom.
    if !stop_early {
        let name = "place-minimized";
        process::write_overlay_status(name);
        process::write_overlay_info("");
        let start = Instant::now();
        let outcome = run_subtest_capture_with_extra(
            name,
            duration_ms,
            timeout_ms.saturating_add(60_000), // give the child extra timeout too
            60_000,                            // and add watchdog headroom on top
            &[],
        );
        let elapsed = start.elapsed();
        if outcome.success {
            println!("{}... OK ({:.3}s)", name, elapsed.as_secs_f64());
        } else {
            println!("{}... FAIL ({:.3}s)", name, elapsed.as_secs_f64());
            print_captured_streams(&outcome);
            record_failure(&mut failures, &mut stop_early, fail_fast, name);
        }
    }
    run_basic_test(
        "place-zoomed",
        duration_ms,
        timeout_ms,
        &mut failures,
        &mut stop_early,
        fail_fast,
    );
    // Stage 6 advisory gating: validate skip behavior when possible
    run_basic_test(
        "place-skip",
        duration_ms,
        timeout_ms,
        &mut failures,
        &mut stop_early,
        fail_fast,
    );
    // Stage 6 advisory gating (focused): available as a separate subcommand `place-skip`.
    // Stage‑3/8 variants via place-flex
    // Encode variants once and iterate to reduce repetition.
    for (cfg, info) in PLACE_FLEX_VARIANTS.iter().copied() {
        if stop_early {
            break;
        }
        process::write_overlay_status("place-flex");
        process::write_overlay_info(info);
        let args = place_flex_args(&cfg);
        let json = serde_json::to_string(&cfg).unwrap_or_default();
        let start = Instant::now();
        let outcome = run_subtest_capture("place-flex", duration_ms, timeout_ms, &args[1..]);
        let elapsed = start.elapsed();
        let summary_name = format!("place-flex ({})", info);
        if outcome.success {
            println!(
                "place-flex... OK ({:.3}s)\n  settings: {}\n  info: {}",
                elapsed.as_secs_f64(),
                json,
                info
            );
        } else {
            println!(
                "place-flex... FAIL ({:.3}s)\n  settings: {}\n  info: {}",
                elapsed.as_secs_f64(),
                json,
                info
            );
            print_captured_streams(&outcome);
            record_failure(&mut failures, &mut stop_early, fail_fast, summary_name);
        }
    }
    run_basic_test(
        "fullscreen",
        duration_ms,
        timeout_ms,
        &mut failures,
        &mut stop_early,
        fail_fast,
    );

    // UI demos
    run_basic_test(
        "ui",
        duration_ms,
        timeout_ms,
        &mut failures,
        &mut stop_early,
        fail_fast,
    );
    run_basic_test(
        "minui",
        duration_ms,
        timeout_ms,
        &mut failures,
        &mut stop_early,
        fail_fast,
    );

    finalize(&mut overlay, &failures);
}

/// Map a `SeqTest` variant to the corresponding CLI subcommand name.
fn to_subcmd(t: SeqTest) -> &'static str {
    match t {
        SeqTest::RepeatRelay => "repeat-relay",
        SeqTest::RepeatShell => "repeat-shell",
        SeqTest::RepeatVolume => "repeat-volume",
        SeqTest::Focus => "focus-tracking",
        SeqTest::Raise => "raise",
        SeqTest::Hide => "hide",
        SeqTest::Place => "place",
        SeqTest::PlaceAsync => "place-async",
        SeqTest::PlaceAnimated => "place-animated",
        SeqTest::Fullscreen => "fullscreen",
        SeqTest::Ui => "ui",
        SeqTest::Minui => "minui",
        SeqTest::PlaceFake => "place-fake",
        SeqTest::WorldSpaces => "world-spaces",
    }
}

/// Run a listed sequence of smoketests in isolated subprocesses.
pub fn run_sequence_tests(tests: &[SeqTest], duration_ms: u64, timeout_ms: u64, logs: bool) {
    if tests.is_empty() {
        eprintln!("no tests provided (use: smoketest seq <tests...>)");
        std_process::exit(2);
    }
    for t in tests.iter().copied() {
        let name = to_subcmd(t);
        heading(&format!("Test: {}", name));
        let dur = if matches!(t, SeqTest::RepeatVolume) {
            cmp::max(duration_ms, config::DEFAULTS.min_volume_duration_ms)
        } else {
            duration_ms
        };
        if !run_subtest_with_watchdog(name, dur, timeout_ms, logs, &[]) {
            std_process::exit(1);
        }
    }
    println!("Selected smoketests passed");
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        process::Command as StdCommand,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::*;

    const STUB_SRC: &str = r#"
use std::{env, thread, time::Duration};

fn main() {
    let mut args = env::args().skip(1);
    let mut timeout_ms: u64 = 0;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--timeout" => {
                if let Some(val) = args.next() {
                    timeout_ms = val.parse().unwrap_or(0);
                }
            }
            "--duration" => {
                let _ = args.next();
            }
            _ => {}
        }
    }
    let sleep_ms = timeout_ms.saturating_add(100);
    thread::sleep(Duration::from_millis(sleep_ms));
}
"#;

    fn build_watchdog_stub() -> PathBuf {
        let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tmp");
        fs::create_dir_all(&base).unwrap();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let src = base.join(format!("watchdog_stub_{}.rs", nanos));
        fs::write(&src, STUB_SRC).unwrap();
        let bin = base.join(format!("watchdog_stub_{}", nanos));
        let status = StdCommand::new("rustc")
            .arg(src.to_str().unwrap())
            .arg("-O")
            .arg("-o")
            .arg(bin.to_str().unwrap())
            .status()
            .expect("failed to compile watchdog stub");
        assert!(status.success());
        if let Err(_e) = fs::remove_file(src) {}
        bin
    }

    struct OverrideGuard {
        path: PathBuf,
    }

    impl Drop for OverrideGuard {
        fn drop(&mut self) {
            clear_watchdog_overhead_override();
            clear_forced_executable();
            if let Err(_e) = fs::remove_file(&self.path) {}
        }
    }

    #[test]
    fn watchdog_kills_hung_child() {
        let stub = build_watchdog_stub();
        set_forced_executable(stub.clone());
        set_watchdog_overhead_for_tests(50);
        let _guard = OverrideGuard { path: stub };
        let outcome = run_subtest_capture("watchdog-stub", 5, 25, &[]);
        assert!(!outcome.success);
    }
}
