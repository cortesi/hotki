#![allow(clippy::disallowed_methods)]
//! Smoketest binary for Hotki. Provides repeat and UI validation helpers.
use clap::Parser;
use logging as logshared;
use tracing::info;
use tracing_subscriber::{fmt, prelude::*};

/// Artifact capture utilities for failure diagnostics.
mod artifacts;
/// Scenario-specific smoketest cases and mimic harness helpers.
mod cases;
mod cli;
mod config;
/// Error definitions and hint helpers used by smoketest.
mod error;
mod helper_window;
/// Shared helper utilities for new smoketest cases.
mod helpers;
/// Registry of helper process IDs for cleanup.
mod proc_registry;
mod process;
mod results;
mod runtime;
/// RPC driving helpers against the running server.
mod server_drive;
/// Session management for launching and controlling hotki.
mod session;
/// Mission Control capture helpers.
mod space_probe;
/// Smoketest case registry and runner.
mod suite;
mod test_runner;
mod tests;
mod ui_interaction;
/// Utility helpers for path resolution and minor tasks.
mod util;
/// UI overlay to warn users to avoid typing during smoketests.
mod warn_overlay;
/// Helper window for UI-driven tests and animations.
mod winhelper;
/// World snapshot helpers backed by hotki-world.
mod world;

use std::{cmp::max, env, path::Path, process::exit, sync::mpsc, thread, time::Duration};

use cli::{Cli, Commands, FsState, SeqTest};
use error::print_hints;
use hotki_protocol::Toggle;
use tests::*;

/// Print a standardized heading for a smoketest section.
pub(crate) fn heading(title: &str) {
    println!("\n==> {}", title);
}

/// Run `f` on a background thread and enforce a timeout via watchdog.
pub(crate) fn run_with_watchdog<F, T>(name: &str, timeout_ms: u64, f: F) -> T
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    use std::time::Instant;
    let start = Instant::now();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let out = f();
        let _send_res = tx.send(out);
    });
    match rx.recv_timeout(Duration::from_millis(timeout_ms)) {
        Ok(v) => {
            let elapsed = start.elapsed();
            info!("{}: completed in {:.3}s", name, elapsed.as_secs_f64());
            v
        }
        Err(_) => {
            eprintln!(
                "ERROR: smoketest watchdog timeout ({} ms) in {} — force exiting",
                timeout_ms, name
            );
            proc_registry::kill_all();
            exit(2);
        }
    }
}

// Some tests (e.g., those that create a winit/Tao EventLoop) must run on the
// main thread on macOS. This variant keeps the test on the main thread and
// enforces a timeout via a background watchdog.
/// Run `f` on the main thread with a watchdog that force-exits on timeout.
pub(crate) fn run_on_main_with_watchdog<F, T>(name: &str, timeout_ms: u64, f: F) -> T
where
    F: FnOnce() -> T,
{
    use std::{
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        thread,
        time::{Duration, Instant},
    };

    let canceled = Arc::new(AtomicBool::new(false));
    let canceled_flag = canceled.clone();
    let name_owned = name.to_string();
    let start = Instant::now();
    let watchdog = thread::spawn(move || {
        let start = Instant::now();
        loop {
            if canceled_flag.load(Ordering::SeqCst) {
                return;
            }
            if start.elapsed() >= Duration::from_millis(timeout_ms) {
                eprintln!(
                    "ERROR: smoketest watchdog timeout ({} ms) in {} — force exiting",
                    timeout_ms, name_owned
                );
                proc_registry::kill_all();
                exit(2);
            }
            thread::sleep(Duration::from_millis(25));
        }
    });

    // Run the test body on the main thread
    let out = f();
    canceled.store(true, Ordering::SeqCst);
    let _join_res = watchdog.join();
    let elapsed = start.elapsed();
    info!("{}: completed in {:.3}s", name, elapsed.as_secs_f64());
    out
}

// Re-export common result types
pub use results::{TestDetails, TestOutcome};

/// Unified case runner: heading + optional overlay + watchdog.
#[allow(clippy::too_many_arguments)]
fn run_case<F, T>(
    heading_title: &str,
    name: &str,
    timeout_ms: u64,
    quiet: bool,
    warn_overlay: bool,
    info: Option<&str>,
    main_thread: bool,
    f: F,
) -> T
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    if !quiet {
        heading(&format!("Test: {}", heading_title));
    }
    let mut overlay = None;
    if warn_overlay && !quiet {
        overlay = process::start_warn_overlay_with_delay();
        process::write_overlay_status(name);
        if let Some(i) = info {
            process::write_overlay_info(i);
        }
    }

    let out = if main_thread {
        run_on_main_with_watchdog(name, timeout_ms, f)
    } else {
        run_with_watchdog(name, timeout_ms, f)
    };

    if let Some(mut o) = overlay
        && let Err(e) = o.kill_and_wait()
    {
        eprintln!("smoketest: failed to stop overlay: {}", e);
    }
    out
}

fn main() {
    let cli = Cli::parse();

    init_tracing_from_cli(&cli);

    if handle_helper_commands_early(&cli) {
        return;
    }

    let perms = permissions::check_permissions();
    let fake_mode = (!perms.accessibility_ok || !perms.input_ok) && env::var_os("CI").is_some();
    if fake_mode && !cli.quiet {
        println!(
            "smoketest: Accessibility/Input permissions missing; running fake placement smoke"
        );
    }

    enforce_permissions_or_exit(perms, fake_mode);
    build_hotki_or_exit(&cli);

    dispatch_command(&cli, fake_mode);
}

/// Initialize tracing/logging according to CLI flags and defaults.
fn init_tracing_from_cli(cli: &Cli) {
    let spec = if cli.quiet {
        logshared::level_spec_for("warn")
    } else {
        logshared::compute_spec(
            cli.log.trace,
            cli.log.debug,
            cli.log.log_level.as_deref(),
            cli.log.log_filter.as_deref(),
        )
    };
    let env_filter = logshared::env_filter_from_spec(&spec);
    let _init_res = tracing_subscriber::registry()
        .with(env_filter)
        .with(fmt::layer().without_time())
        .try_init();
}

/// Handle helper subcommands that bypass standard checks. Returns true if handled.
fn handle_helper_commands_early(cli: &Cli) -> bool {
    if let Commands::FocusWinHelper {
        title,
        time,
        delay_setframe_ms,
        delay_apply_ms,
        tween_ms,
        apply_target,
        apply_grid,
        slot,
        grid,
        size,
        pos,
        label_text,
        min_size,
        step_size,
        start_minimized,
        start_zoomed,
        panel_nonmovable,
        non_resizable,
        attach_sheet,
    } = &cli.command
    {
        let grid_tuple = grid
            .as_ref()
            .and_then(|v| (v.len() == 4).then(|| (v[0], v[1], v[2], v[3])));
        let size_tuple = size
            .as_ref()
            .and_then(|v| (v.len() == 2).then(|| (v[0], v[1])));
        let pos_tuple = pos
            .as_ref()
            .and_then(|v| (v.len() == 2).then(|| (v[0], v[1])));
        let step_size_tuple = step_size
            .as_ref()
            .and_then(|v| (v.len() == 2).then(|| (v[0], v[1])));
        let min_size_tuple = min_size
            .as_ref()
            .and_then(|v| (v.len() == 2).then(|| (v[0], v[1])));
        let apply_target_tuple = apply_target
            .as_ref()
            .and_then(|v| (v.len() == 4).then(|| (v[0], v[1], v[2], v[3])));
        let apply_grid_tuple = apply_grid
            .as_ref()
            .and_then(|v| (v.len() == 4).then(|| (v[0], v[1], v[2], v[3])));

        if let Err(e) = winhelper::run_focus_winhelper(
            title,
            *time,
            delay_setframe_ms.unwrap_or(0),
            delay_apply_ms.unwrap_or(0),
            tween_ms.unwrap_or(0),
            apply_target_tuple,
            apply_grid_tuple,
            *slot,
            grid_tuple,
            size_tuple,
            pos_tuple,
            label_text.clone(),
            min_size_tuple,
            step_size_tuple,
            *start_minimized,
            *start_zoomed,
            *panel_nonmovable,
            *non_resizable,
            *attach_sheet,
        ) {
            eprintln!("focus-winhelper: ERROR: {}", e);
            exit(2);
        }
        return true;
    }

    if let Commands::WarnOverlay {
        status_path,
        info_path,
    } = &cli.command
    {
        match warn_overlay::run_warn_overlay(status_path.clone(), info_path.clone()) {
            Ok(()) => {}
            Err(e) => {
                eprintln!("warn-overlay: ERROR: {}", e);
                exit(2);
            }
        }
        return true;
    }
    false
}

/// Ensure required macOS permissions are granted; exit with a helpful message if not.
fn enforce_permissions_or_exit(perms: permissions::PermissionsStatus, fake_mode: bool) {
    if fake_mode {
        return;
    }
    if !perms.accessibility_ok || !perms.input_ok {
        eprintln!(
            "ERROR: required permissions missing (accessibility={}, input_monitoring={})",
            perms.accessibility_ok, perms.input_ok
        );
        eprintln!(
            "Grant Accessibility and Input Monitoring to your terminal under System Settings → Privacy & Security."
        );
        exit(1);
    }
}

/// Build the hotki binary once up-front to avoid stale binaries.
fn build_hotki_or_exit(cli: &Cli) {
    if !cli.quiet {
        heading("Building hotki");
    }
    if let Err(e) = process::build_hotki_quiet() {
        eprintln!("Failed to build 'hotki' binary: {}", e);
        eprintln!("Try: cargo build -p hotki");
        exit(1);
    }
}

/// Dispatch to the concrete smoketest command handlers.
fn dispatch_command(cli: &Cli, fake_mode: bool) {
    match &cli.command {
        Commands::Relay => handle_relay(cli),
        Commands::Shell => handle_shell(cli),
        Commands::Volume => handle_volume(cli),
        Commands::All => {
            if fake_mode {
                if !cli.quiet {
                    heading("Test: place-fake");
                }
                handle_place_fake(cli);
            } else {
                let runner_cfg = suite::RunnerConfig {
                    quiet: cli.quiet,
                    warn_overlay: !cli.no_warn,
                    base_timeout_ms: cli.timeout,
                    fail_fast: !cli.no_fail_fast,
                    overlay_info: cli.info.as_deref(),
                };
                if let Err(err) = suite::run_all(&runner_cfg) {
                    eprintln!("smoketest all failed: {}", err);
                    exit(1);
                }
            }
        }
        Commands::PlaceIncrements => handle_place_increments(cli),
        Commands::Seq { tests } => {
            let names: Vec<&'static str> = tests.iter().map(seq_case_name).collect();
            let runner_cfg = suite::RunnerConfig {
                quiet: cli.quiet,
                warn_overlay: !cli.no_warn,
                base_timeout_ms: cli.timeout,
                fail_fast: !cli.no_fail_fast,
                overlay_info: cli.info.as_deref(),
            };
            if let Err(err) = suite::run_sequence(&names, &runner_cfg) {
                eprintln!("smoketest seq failed: {}", err);
                exit(1);
            }
        }
        Commands::Raise => handle_raise(cli),
        Commands::PlaceFlex {
            cols,
            rows,
            col,
            row,
            force_size_pos,
            pos_first_only,
            force_shrink_move_grow,
        } => {
            let args = PlaceFlexArgs {
                cols: *cols,
                rows: *rows,
                col: *col,
                row: *row,
                force_size_pos: *force_size_pos,
                pos_first_only: *pos_first_only,
                force_shrink_move_grow: *force_shrink_move_grow,
            };
            handle_place_flex(cli, &args)
        }
        Commands::PlaceFallback => handle_place_fallback(cli),
        Commands::PlaceSmg => handle_place_smg(cli),
        Commands::PlaceSkip => handle_place_skip(cli),
        Commands::FocusNav => handle_focus_nav(cli),
        Commands::Focus => handle_focus(cli),
        Commands::Hide => handle_hide(cli),
        Commands::Place => {
            if fake_mode {
                handle_place_fake(cli);
            } else {
                handle_place(cli);
            }
        }
        Commands::PlaceFake => handle_place_fake(cli),
        Commands::PlaceAsync => handle_place_async(cli),
        Commands::PlaceAnimated => handle_place_animated(cli),
        Commands::PlaceTerm => handle_place_term(cli),
        Commands::PlaceMoveMin => handle_place_move_min(cli),
        Commands::PlaceMoveNonresizable => handle_place_move_nonresizable(cli),
        Commands::PlaceMinimized => handle_place_minimized(cli),
        Commands::PlaceZoomed => handle_place_zoomed(cli),
        Commands::FocusWinHelper { .. } => unreachable!(),
        Commands::WarnOverlay { .. } => unreachable!(),
        Commands::Ui => handle_ui(cli),
        // Screenshots extracted to separate tool: hotki-shots
        Commands::Minui => handle_minui(cli),
        Commands::Fullscreen { state, native } => handle_fullscreen(cli, *state, *native),
        Commands::WorldStatus => handle_world_status(cli),
        Commands::WorldAx => handle_world_ax(cli),
        Commands::WorldSpaces => handle_world_spaces(cli),
        Commands::SpaceProbe {
            samples,
            interval_ms,
            output,
        } => handle_space_probe(cli, *samples, *interval_ms, output.as_deref()),
    }
}

/// Map legacy `seq` invocations onto registry-backed case names.
fn seq_case_name(test: &SeqTest) -> &'static str {
    match test {
        SeqTest::RepeatRelay => "repeat-relay",
        SeqTest::RepeatShell => "repeat-shell",
        SeqTest::RepeatVolume => "repeat-volume",
        SeqTest::Focus => "focus-tracking",
        SeqTest::Raise => "raise",
        SeqTest::Hide => "hide",
        SeqTest::Place => "place.minimized.defer",
        SeqTest::PlaceAsync => "place.async.delay",
        SeqTest::PlaceAnimated => "place.animated.tween",
        SeqTest::Fullscreen => "fullscreen",
        SeqTest::Ui => "ui",
        SeqTest::Minui => "minui",
        SeqTest::PlaceFake => "place-fake",
        SeqTest::WorldSpaces => "world-spaces",
    }
}

/// Handle the `repeat-relay` test case.
fn handle_relay(cli: &Cli) {
    if !cli.quiet {
        heading("Test: repeat-relay");
    }
    let duration = cli.duration;
    let mut overlay = None;
    if !cli.no_warn {
        overlay = process::start_warn_overlay_with_delay();
        process::write_overlay_status("repeat-relay");
        if let Some(info) = &cli.info {
            process::write_overlay_info(info);
        }
    }
    run_on_main_with_watchdog("repeat-relay", cli.timeout, move || repeat_relay(duration));
    if let Some(mut o) = overlay
        && let Err(e) = o.kill_and_wait()
    {
        eprintln!("smoketest: failed to stop overlay: {}", e);
    }
}

/// Handle the `repeat-shell` test case.
fn handle_shell(cli: &Cli) {
    if !cli.quiet {
        heading("Test: repeat-shell");
    }
    let duration = cli.duration;
    let mut overlay = None;
    if !cli.no_warn {
        overlay = process::start_warn_overlay_with_delay();
        process::write_overlay_status("repeat-shell");
        if let Some(info) = &cli.info {
            process::write_overlay_info(info);
        }
    }
    run_with_watchdog("repeat-shell", cli.timeout, move || repeat_shell(duration));
    if let Some(mut o) = overlay
        && let Err(e) = o.kill_and_wait()
    {
        eprintln!("smoketest: failed to stop overlay: {}", e);
    }
}

/// Handle the `repeat-volume` test case.
fn handle_volume(cli: &Cli) {
    if !cli.quiet {
        heading("Test: repeat-volume");
    }
    let duration = max(cli.duration, config::DEFAULTS.min_volume_duration_ms);
    let mut overlay = None;
    if !cli.no_warn {
        overlay = process::start_warn_overlay_with_delay();
        process::write_overlay_status("repeat-volume");
        if let Some(info) = &cli.info {
            process::write_overlay_info(info);
        }
    }
    run_with_watchdog("repeat-volume", cli.timeout, move || {
        repeat_volume(duration)
    });
    if let Some(mut o) = overlay
        && let Err(e) = o.kill_and_wait()
    {
        eprintln!("smoketest: failed to stop overlay: {}", e);
    }
}

/// Handle `place-increments` test case.
fn handle_place_increments(cli: &Cli) {
    let timeout = cli.timeout;
    let logs = true;
    match run_case(
        "place-increments",
        "place-increments",
        timeout,
        cli.quiet,
        !cli.no_warn,
        cli.info.as_deref(),
        true,
        move || tests::place_increments::run_place_increments_test(timeout, logs),
    ) {
        Ok(()) => {
            if !cli.quiet {
                println!("place-increments: OK (anchored edges verified)");
            }
        }
        Err(e) => {
            eprintln!("place-increments: ERROR: {}", e);
            print_hints(&e);
            exit(1);
        }
    }
}

/// Handle `raise` test case.
fn handle_raise(cli: &Cli) {
    let timeout = cli.timeout;
    let logs = true;
    match run_case(
        "raise",
        "raise",
        timeout,
        cli.quiet,
        !cli.no_warn,
        cli.info.as_deref(),
        false,
        move || raise::run_raise_test(timeout, logs),
    ) {
        Ok(()) => {
            if !cli.quiet {
                println!("raise: OK (raised by title twice)");
            }
        }
        Err(e) => {
            eprintln!("raise: ERROR: {}", e);
            print_hints(&e);
            exit(1);
        }
    }
}

/// Handle `place-flex` test case.
/// Arguments for the `place-flex` test case.
struct PlaceFlexArgs {
    /// Number of columns in the grid.
    cols: u32,
    /// Number of rows in the grid.
    rows: u32,
    /// Column index.
    col: u32,
    /// Row index.
    row: u32,
    /// Whether to force size before position.
    force_size_pos: bool,
    /// Whether to set the position first only.
    pos_first_only: bool,
    /// Whether to force the shrink→move→grow path.
    force_shrink_move_grow: bool,
}

/// Handle the `place-flex` test case.
fn handle_place_flex(cli: &Cli, args: &PlaceFlexArgs) {
    if !cli.quiet {
        heading("Test: place-flex");
    }
    let timeout = cli.timeout;
    let logs = true;
    let mut overlay = None;
    if !cli.no_warn {
        overlay = process::start_warn_overlay_with_delay();
        process::write_overlay_status("place-flex");
        if let Some(info) = &cli.info {
            process::write_overlay_info(info);
        }
    }
    match run_on_main_with_watchdog("place-flex", timeout, move || {
        if logs {
            // logging already configured
        }
        tests::place_flex::run_place_flex(
            args.cols,
            args.rows,
            args.col,
            args.row,
            args.force_size_pos,
            args.pos_first_only,
            args.force_shrink_move_grow,
        )
    }) {
        Ok(()) => {
            if !cli.quiet {
                println!(
                    "place-flex: OK (cols={} rows={} cell=({},{}), force_size_pos={}, pos_first_only={})",
                    args.cols,
                    args.rows,
                    args.col,
                    args.row,
                    args.force_size_pos,
                    args.pos_first_only
                );
            }
        }
        Err(e) => {
            eprintln!("place-flex: ERROR: {}", e);
            print_hints(&e);
            if let Some(mut o) = overlay
                && let Err(e) = o.kill_and_wait()
            {
                eprintln!("smoketest: failed to stop overlay: {}", e);
            }
            exit(1);
        }
    }
    if let Some(mut o) = overlay
        && let Err(e) = o.kill_and_wait()
    {
        eprintln!("smoketest: failed to stop overlay: {}", e);
    }
}

/// Handle `place-fallback` test case.
fn handle_place_fallback(cli: &Cli) {
    let timeout = cli.timeout;
    match run_case(
        "place-fallback",
        "place-fallback",
        timeout,
        cli.quiet,
        !cli.no_warn,
        cli.info.as_deref(),
        true,
        move || {
            tests::place_flex::run_place_flex(
                config::PLACE.grid_cols,
                config::PLACE.grid_rows,
                0,
                0,
                true,
                false,
                false,
            )
        },
    ) {
        Ok(()) => {
            if !cli.quiet {
                println!("place-fallback: OK (forced size->pos path)");
            }
        }
        Err(e) => {
            eprintln!("place-fallback: ERROR: {}", e);
            print_hints(&e);
            exit(1);
        }
    }
}

/// Handle `place-smg` test case.
fn handle_place_smg(cli: &Cli) {
    let timeout = cli.timeout;
    match run_case(
        "place-smg (shrink→move→grow)",
        "place-smg",
        timeout,
        cli.quiet,
        !cli.no_warn,
        None,
        true,
        move || tests::place_flex::run_place_flex(2, 2, 1, 1, false, false, true),
    ) {
        Ok(()) => {
            if !cli.quiet {
                println!("place-smg: OK (forced shrink→move→grow path)");
            }
        }
        Err(e) => {
            eprintln!("place-smg: ERROR: {}", e);
            print_hints(&e);
            exit(1);
        }
    }
}

/// Handle `place-skip` test case.
fn handle_place_skip(cli: &Cli) {
    if !cli.quiet {
        heading("Test: place-skip (non-movable)");
    }
    let timeout = cli.timeout;
    let logs = true;
    let mut overlay = None;
    if !cli.no_warn {
        overlay = process::start_warn_overlay_with_delay();
        process::write_overlay_status("place-skip");
    }
    match run_on_main_with_watchdog("place-skip", timeout, move || {
        tests::place_skip::run_place_skip_test(timeout, logs)
    }) {
        Ok(()) => {
            if !cli.quiet {
                println!("place-skip: OK (engine skipped non-movable)");
            }
        }
        Err(e) => {
            eprintln!("place-skip: ERROR: {}", e);
            print_hints(&e);
            if let Some(mut o) = overlay
                && let Err(e) = o.kill_and_wait()
            {
                eprintln!("smoketest: failed to stop overlay: {}", e);
            }
            exit(1);
        }
    }
    if let Some(mut o) = overlay
        && let Err(e) = o.kill_and_wait()
    {
        eprintln!("smoketest: failed to stop overlay: {}", e);
    }
}

/// Handle `focus-nav` test case.
fn handle_focus_nav(cli: &Cli) {
    let timeout = cli.timeout;
    let logs = true;
    match run_case(
        "focus-nav",
        "focus-nav",
        timeout,
        cli.quiet,
        !cli.no_warn,
        cli.info.as_deref(),
        true,
        move || tests::focus_nav::run_focus_nav_test(timeout, logs),
    ) {
        Ok(()) => {
            if !cli.quiet {
                println!("focus-nav: OK (navigated right, down, left, up)");
            }
        }
        Err(e) => {
            eprintln!("focus-nav: ERROR: {}", e);
            print_hints(&e);
            exit(1);
        }
    }
}

/// Handle `focus-tracking` test case.
fn handle_focus(cli: &Cli) {
    let timeout = cli.timeout;
    let logs = true;
    match run_case(
        "focus-tracking",
        "focus-tracking",
        timeout,
        cli.quiet,
        !cli.no_warn,
        cli.info.as_deref(),
        false,
        move || focus::run_focus_test(timeout, logs),
    ) {
        Ok(out) => {
            if !cli.quiet {
                println!("{}", out.format_status("focus-tracking"));
            }
        }
        Err(e) => {
            eprintln!("focus-tracking: ERROR: {}", e);
            print_hints(&e);
            exit(1);
        }
    }
}

/// Handle `hide` test case.
fn handle_hide(cli: &Cli) {
    let timeout = cli.timeout;
    let logs = true;
    match run_case(
        "hide",
        "hide",
        timeout,
        cli.quiet,
        !cli.no_warn,
        cli.info.as_deref(),
        false,
        move || hide::run_hide_test(timeout, logs),
    ) {
        Ok(()) => {
            if !cli.quiet {
                println!("hide: OK (toggle on/off roundtrip)");
            }
        }
        Err(e) => {
            eprintln!("hide: ERROR: {}", e);
            print_hints(&e);
            exit(1);
        }
    }
}

/// Handle `place` test case.
fn handle_place(cli: &Cli) {
    let timeout = cli.timeout;
    let logs = true;
    match run_case(
        "place",
        "place",
        timeout,
        cli.quiet,
        !cli.no_warn,
        None,
        true,
        move || tests::place::run_place_test(timeout, logs),
    ) {
        Ok(()) => {
            if !cli.quiet {
                println!("place: OK (cycled all grid cells)");
            }
        }
        Err(e) => {
            eprintln!("place: ERROR: {}", e);
            print_hints(&e);
            exit(1);
        }
    }
}

/// Handle the fake placement test harness.
fn handle_place_fake(cli: &Cli) {
    let timeout = cli.timeout;
    let logs = true;
    match run_case(
        "place-fake",
        "place-fake",
        timeout,
        cli.quiet,
        false,
        cli.info.as_deref(),
        false,
        move || tests::place_fake::run_fake_place_test(timeout, logs),
    ) {
        Ok(()) => {
            if !cli.quiet {
                println!("place-fake: OK (fake adapter flows)");
            }
        }
        Err(e) => {
            eprintln!("place-fake: ERROR: {}", e);
            print_hints(&e);
            exit(1);
        }
    }
}

/// Handle `place-async` test case.
fn handle_place_async(cli: &Cli) {
    let timeout = cli.timeout;
    let logs = true;
    match run_case(
        "place-async",
        "place-async",
        timeout,
        cli.quiet,
        !cli.no_warn,
        None,
        true,
        move || tests::place_async::run_place_async_test(timeout, logs),
    ) {
        Ok(()) => {
            if !cli.quiet {
                println!("place-async: OK (converged within default budget)");
            }
        }
        Err(e) => {
            eprintln!("place-async: ERROR: {}", e);
            print_hints(&e);
            exit(1);
        }
    }
}

/// Handle `place-animated` test case.
fn handle_place_animated(cli: &Cli) {
    let timeout = cli.timeout;
    let logs = true;
    match run_case(
        "place-animated",
        "place-animated",
        timeout,
        cli.quiet,
        !cli.no_warn,
        None,
        true,
        move || tests::place_animated::run_place_animated_test(timeout, logs),
    ) {
        Ok(()) => {
            if !cli.quiet {
                println!("place-animated: OK (converged with tween)");
            }
        }
        Err(e) => {
            eprintln!("place-animated: ERROR: {}", e);
            print_hints(&e);
            exit(1);
        }
    }
}

/// Handle `place-term` test case.
fn handle_place_term(cli: &Cli) {
    let timeout = cli.timeout;
    match run_case(
        "place-term",
        "place-term",
        timeout,
        cli.quiet,
        !cli.no_warn,
        cli.info.as_deref(),
        true,
        move || tests::place_term::run_place_term_test(timeout, true),
    ) {
        Ok(()) => {
            if !cli.quiet {
                println!("place-term: OK (latched origin; no thrash)");
            }
        }
        Err(e) => {
            eprintln!("place-term: ERROR: {}", e);
            print_hints(&e);
            exit(1);
        }
    }
}

/// Handle `place-move-min` test case.
fn handle_place_move_min(cli: &Cli) {
    let timeout = cli.timeout;
    let logs = true;
    match run_case(
        "place-move-min",
        "place-move-min",
        timeout,
        cli.quiet,
        !cli.no_warn,
        cli.info.as_deref(),
        true,
        move || tests::place_move_min::run_place_move_min_test(timeout, logs),
    ) {
        Ok(()) => {
            if !cli.quiet {
                println!("place-move-min: OK (moved with min-height anchored)");
            }
        }
        Err(e) => {
            eprintln!("place-move-min: ERROR: {}", e);
            print_hints(&e);
            exit(1);
        }
    }
}

/// Handle `place-move-nonresizable` test case.
fn handle_place_move_nonresizable(cli: &Cli) {
    let timeout = cli.timeout;
    let logs = true;
    match run_case(
        "place-move-nonresizable",
        "place-move-nonresizable",
        timeout,
        cli.quiet,
        !cli.no_warn,
        cli.info.as_deref(),
        true,
        move || tests::place_move_nonresizable::run_place_move_nonresizable_test(timeout, logs),
    ) {
        Ok(()) => {
            if !cli.quiet {
                println!("place-move-nonresizable: OK (moved with anchored fallback)");
            }
        }
        Err(e) => {
            eprintln!("place-move-nonresizable: ERROR: {}", e);
            print_hints(&e);
            exit(1);
        }
    }
}

/// Handle `place-minimized` test case.
fn handle_place_minimized(cli: &Cli) {
    if !cli.quiet {
        heading("Test: place-minimized");
    }
    let timeout = cli.timeout;
    let logs = true;
    match run_on_main_with_watchdog("place-minimized", timeout, move || {
        tests::place_state::run_place_minimized_test(timeout, logs)
    }) {
        Ok(()) => {
            if !cli.quiet {
                println!("place-minimized: OK (normalized minimized -> placed)");
            }
        }
        Err(e) => {
            eprintln!("place-minimized: ERROR: {}", e);
            print_hints(&e);
            exit(1);
        }
    }
}

/// Handle `place-zoomed` test case.
fn handle_place_zoomed(cli: &Cli) {
    if !cli.quiet {
        heading("Test: place-zoomed");
    }
    let timeout = cli.timeout;
    let logs = true;
    match run_on_main_with_watchdog("place-zoomed", timeout, move || {
        tests::place_state::run_place_zoomed_test(timeout, logs)
    }) {
        Ok(()) => {
            if !cli.quiet {
                println!("place-zoomed: OK (normalized zoomed -> placed)");
            }
        }
        Err(e) => {
            eprintln!("place-zoomed: ERROR: {}", e);
            print_hints(&e);
            exit(1);
        }
    }
}

/// Handle `ui` test case.
fn handle_ui(cli: &Cli) {
    if !cli.quiet {
        heading("Test: ui");
    }
    let timeout = cli.timeout;
    let mut overlay = None;
    if !cli.no_warn {
        overlay = process::start_warn_overlay_with_delay();
        process::write_overlay_status("ui");
        if let Some(info) = &cli.info {
            process::write_overlay_info(info);
        }
    }
    match run_with_watchdog("ui", timeout, move || ui::run_ui_demo(timeout)) {
        Ok(out) => {
            if !cli.quiet {
                println!("{}", out.format_status("ui"));
            }
        }
        Err(e) => {
            eprintln!("ui: ERROR: {}", e);
            print_hints(&e);
            if let Some(mut o) = overlay
                && let Err(e) = o.kill_and_wait()
            {
                eprintln!("smoketest: failed to stop overlay: {}", e);
            }
            exit(1);
        }
    }
    if let Some(mut o) = overlay
        && let Err(e) = o.kill_and_wait()
    {
        eprintln!("smoketest: failed to stop overlay: {}", e);
    }
}

/// Handle `minui` demo.
fn handle_minui(cli: &Cli) {
    if !cli.quiet {
        heading("Test: minui");
    }
    let timeout = cli.timeout;
    let mut overlay = None;
    if !cli.no_warn {
        overlay = process::start_warn_overlay_with_delay();
        process::write_overlay_status("minui");
        if let Some(info) = &cli.info {
            process::write_overlay_info(info);
        }
    }
    match run_with_watchdog("minui", timeout, move || ui::run_minui_demo(timeout)) {
        Ok(out) => {
            if !cli.quiet {
                println!("{}", out.format_status("minui"));
            }
        }
        Err(e) => {
            eprintln!("minui: ERROR: {}", e);
            print_hints(&e);
            if let Some(mut o) = overlay
                && let Err(e) = o.kill_and_wait()
            {
                eprintln!("smoketest: failed to stop overlay: {}", e);
            }
            exit(1);
        }
    }
    if let Some(mut o) = overlay
        && let Err(e) = o.kill_and_wait()
    {
        eprintln!("smoketest: failed to stop overlay: {}", e);
    }
}

/// Handle `fullscreen` test case.
fn handle_fullscreen(cli: &Cli, state: FsState, native: bool) {
    let toggle = match state {
        FsState::Toggle => Toggle::Toggle,
        FsState::On => Toggle::On,
        FsState::Off => Toggle::Off,
    };
    let timeout = cli.timeout;
    let logs = true;
    match run_case(
        "fullscreen",
        "fullscreen",
        timeout,
        cli.quiet,
        !cli.no_warn,
        cli.info.as_deref(),
        false,
        move || tests::fullscreen::run_fullscreen_test(timeout, logs, toggle, native),
    ) {
        Ok(()) => {
            if !cli.quiet {
                println!("fullscreen: OK (toggled non-native fullscreen)");
            }
        }
        Err(e) => {
            eprintln!("fullscreen: ERROR: {}", e);
            print_hints(&e);
            exit(1);
        }
    }
}

/// Handle `world-status` test case.
fn handle_world_status(cli: &Cli) {
    if !cli.quiet {
        heading("Test: world-status");
    }
    let timeout = cli.timeout;
    match run_with_watchdog("world-status", timeout, move || {
        tests::world_status::run_world_status_test(timeout, true)
    }) {
        Ok(()) => {
            if !cli.quiet {
                println!("world-status: OK (permissions granted; status sane)");
            }
        }
        Err(e) => {
            eprintln!("world-status: ERROR: {}", e);
            print_hints(&e);
            exit(1);
        }
    }
}

/// Handle `world-spaces` test case.
fn handle_world_spaces(cli: &Cli) {
    if !cli.quiet {
        heading("Test: world-spaces");
    }
    let timeout = cli.timeout;
    match run_with_watchdog("world-spaces", timeout, move || {
        tests::world_spaces::run_world_spaces_test(timeout, true)
    }) {
        Ok(()) => {
            if !cli.quiet {
                println!("world-spaces: OK (multi-space adoption within budget)");
            }
        }
        Err(e) => {
            eprintln!("world-spaces: ERROR: {}", e);
            print_hints(&e);
            exit(1);
        }
    }
}

/// Handle `world-ax` test case.
fn handle_world_ax(cli: &Cli) {
    if !cli.quiet {
        heading("Test: world-ax");
    }
    let timeout = cli.timeout;
    match run_with_watchdog("world-ax", timeout, move || {
        tests::world_ax::run_world_ax_test(timeout, true)
    }) {
        Ok(()) => {
            if !cli.quiet {
                println!("world-ax: OK (role/subrole present; flags resolved)");
            }
        }
        Err(e) => {
            eprintln!("world-ax: ERROR: {}", e);
            print_hints(&e);
            exit(1);
        }
    }
}

/// Invoke the Mission Control space probe helper.
fn handle_space_probe(cli: &Cli, samples: u32, interval_ms: u64, output: Option<&Path>) {
    if !cli.quiet {
        heading("space-probe");
    }
    if let Err(e) = space_probe::run(samples, interval_ms, output, cli.quiet) {
        eprintln!("space-probe: ERROR: {}", e);
        print_hints(&e);
        exit(1);
    }
}
