//! Smoketest binary for Hotki. Provides repeat and UI validation helpers.
use clap::Parser;
use logging as logshared;
use tracing_subscriber::prelude::*;

mod cli;
mod config;
/// Error definitions and hint helpers used by smoketest.
mod error;
// no local logging module; use shared crate
mod orchestrator;
/// Registry of helper process IDs for cleanup.
mod proc_registry;
mod process;
mod results;
mod runtime;
/// RPC driving helpers against the running server.
mod server_drive;
/// Session management for launching and controlling hotki.
mod session;
mod test_runner;
mod tests;
mod ui_interaction;
mod util;
mod warn_overlay;
mod winhelper;

use std::{sync::mpsc, time::Duration};

use cli::{Cli, Commands, FsState};
use error::print_hints;
use hotki_protocol::Toggle;
use orchestrator::{heading, run_all_tests};
use tests::*;

fn run_with_watchdog<F, T>(name: &str, timeout_ms: u64, f: F) -> T
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let out = f();
        let _ = tx.send(out);
    });
    match rx.recv_timeout(Duration::from_millis(timeout_ms)) {
        Ok(v) => v,
        Err(_) => {
            eprintln!(
                "ERROR: smoketest watchdog timeout ({} ms) in {} — force exiting",
                timeout_ms, name
            );
            crate::proc_registry::kill_all();
            std::process::exit(2);
        }
    }
}

// Some tests (e.g., those that create a winit/Tao EventLoop) must run on the
// main thread on macOS. This variant keeps the test on the main thread and
// enforces a timeout via a background watchdog.
fn run_on_main_with_watchdog<F, T>(name: &str, timeout_ms: u64, f: F) -> T
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
                crate::proc_registry::kill_all();
                std::process::exit(2);
            }
            thread::sleep(Duration::from_millis(25));
        }
    });

    // Run the test body on the main thread
    let out = f();
    canceled.store(true, Ordering::SeqCst);
    let _ = watchdog.join();
    out
}

// Re-export common result types
pub use results::{FocusOutcome, Summary, TestDetails, TestOutcome};

// Unified case runner: heading + optional overlay + watchdog.
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
    if warn_overlay {
        overlay = crate::process::start_warn_overlay_with_delay();
        crate::process::write_overlay_status(name);
        if let Some(i) = info {
            crate::process::write_overlay_info(i);
        }
    }

    let out = if main_thread {
        run_on_main_with_watchdog(name, timeout_ms, f)
    } else {
        run_with_watchdog(name, timeout_ms, f)
    };

    if let Some(mut o) = overlay {
        if let Err(e) = o.kill_and_wait() {
            eprintln!("smoketest: failed to stop overlay: {}", e);
        }
    }
    out
}

fn main() {
    let cli = Cli::parse();

    // Compose spec: quiet forces warn for our crates; otherwise use shared precedence
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
    let _ = tracing_subscriber::registry()
        .with(env_filter)
        .with(tracing_subscriber::fmt::layer().without_time())
        .try_init();

    // For helper commands, skip permission/build checks and heading
    if matches!(cli.command, Commands::FocusWinHelper { .. }) {
        match cli.command {
            Commands::FocusWinHelper {
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
                attach_sheet,
            } => {
                let grid_tuple = grid.and_then(|v| {
                    if v.len() == 4 {
                        Some((v[0], v[1], v[2], v[3]))
                    } else {
                        None
                    }
                });
                let size_tuple = size.and_then(|v| {
                    if v.len() == 2 {
                        Some((v[0], v[1]))
                    } else {
                        None
                    }
                });
                let pos_tuple = pos.and_then(|v| {
                    if v.len() == 2 {
                        Some((v[0], v[1]))
                    } else {
                        None
                    }
                });
                let step_size_tuple = step_size.and_then(|v| {
                    if v.len() == 2 {
                        Some((v[0], v[1]))
                    } else {
                        None
                    }
                });
                let min_size_tuple = min_size.and_then(|v| {
                    if v.len() == 2 {
                        Some((v[0], v[1]))
                    } else {
                        None
                    }
                });
                let apply_target_tuple = apply_target.and_then(|v| {
                    if v.len() == 4 {
                        Some((v[0], v[1], v[2], v[3]))
                    } else {
                        None
                    }
                });
                let apply_grid_tuple = apply_grid.and_then(|v| {
                    if v.len() == 4 {
                        Some((v[0], v[1], v[2], v[3]))
                    } else {
                        None
                    }
                });
                if let Err(e) = winhelper::run_focus_winhelper(
                    &title,
                    time,
                    delay_setframe_ms.unwrap_or(0),
                    delay_apply_ms.unwrap_or(0),
                    tween_ms.unwrap_or(0),
                    apply_target_tuple,
                    apply_grid_tuple,
                    slot,
                    grid_tuple,
                    size_tuple,
                    pos_tuple,
                    label_text,
                    min_size_tuple,
                    step_size_tuple,
                    start_minimized,
                    start_zoomed,
                    panel_nonmovable,
                    attach_sheet,
                ) {
                    eprintln!("focus-winhelper: ERROR: {}", e);
                    std::process::exit(2);
                }
            }
            _ => unreachable!(),
        }
        return;
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
                std::process::exit(2);
            }
        }
        return;
    }

    // Enforce required permissions for all smoketests.
    let p = permissions::check_permissions();
    if !p.accessibility_ok || !p.input_ok {
        eprintln!(
            "ERROR: required permissions missing (accessibility={}, input_monitoring={})",
            p.accessibility_ok, p.input_ok
        );
        eprintln!(
            "Grant Accessibility and Input Monitoring to your terminal under System Settings → Privacy & Security."
        );
        std::process::exit(1);
    }

    // Screenshots extracted to separate tool: hotki-shots

    // Build the hotki binary once at startup to avoid running against a stale build.
    if !cli.quiet {
        heading("Building hotki");
    }
    if let Err(e) = process::build_hotki_quiet() {
        eprintln!("Failed to build 'hotki' binary: {}", e);
        eprintln!("Try: cargo build -p hotki");
        std::process::exit(1);
    }

    match cli.command {
        Commands::Relay => {
            if !cli.quiet {
                heading("Test: repeat-relay");
            }
            let duration = cli.duration;
            let mut overlay = None;
            if !cli.no_warn {
                overlay = crate::process::start_warn_overlay_with_delay();
                crate::process::write_overlay_status("repeat-relay");
                if let Some(info) = &cli.info {
                    crate::process::write_overlay_info(info);
                }
            }
            // repeat‑relay opens a winit EventLoop; it must run on the main thread.
            run_on_main_with_watchdog("repeat-relay", cli.timeout, move || repeat_relay(duration));
            if let Some(mut o) = overlay {
                if let Err(e) = o.kill_and_wait() {
                    eprintln!("smoketest: failed to stop overlay: {}", e);
                }
            }
        }
        Commands::Shell => {
            if !cli.quiet {
                heading("Test: repeat-shell");
            }
            let duration = cli.duration;
            let mut overlay = None;
            if !cli.no_warn {
                overlay = crate::process::start_warn_overlay_with_delay();
                crate::process::write_overlay_status("repeat-shell");
                if let Some(info) = &cli.info {
                    crate::process::write_overlay_info(info);
                }
            }
            run_with_watchdog("repeat-shell", cli.timeout, move || repeat_shell(duration));
            if let Some(mut o) = overlay {
                if let Err(e) = o.kill_and_wait() {
                    eprintln!("smoketest: failed to stop overlay: {}", e);
                }
            }
        }
        Commands::Volume => {
            if !cli.quiet {
                heading("Test: repeat-volume");
            }
            // Volume can be slightly slower; keep a floor to reduce flakiness
            let duration = std::cmp::max(cli.duration, config::MIN_VOLUME_TEST_DURATION_MS);
            let mut overlay = None;
            if !cli.no_warn {
                overlay = crate::process::start_warn_overlay_with_delay();
                crate::process::write_overlay_status("repeat-volume");
                if let Some(info) = &cli.info {
                    crate::process::write_overlay_info(info);
                }
            }
            run_with_watchdog("repeat-volume", cli.timeout, move || {
                repeat_volume(duration)
            });
            if let Some(mut o) = overlay {
                if let Err(e) = o.kill_and_wait() {
                    eprintln!("smoketest: failed to stop overlay: {}", e);
                }
            }
        }
        Commands::All => run_all_tests(cli.duration, cli.timeout, true, !cli.no_warn),
        Commands::PlaceIncrements => {
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
                        println!("place-increments: OK (anchored edges verified)")
                    }
                }
                Err(e) => {
                    eprintln!("place-increments: ERROR: {}", e);
                    print_hints(&e);
                    std::process::exit(1);
                }
            }
        }
        Commands::Seq { tests } => {
            orchestrator::run_sequence_tests(&tests, cli.duration, cli.timeout, true)
        }
        Commands::Raise => {
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
                        println!("raise: OK (raised by title twice)")
                    }
                }
                Err(e) => {
                    eprintln!("raise: ERROR: {}", e);
                    print_hints(&e);
                    std::process::exit(1);
                }
            }
        }
        Commands::PlaceFlex {
            cols,
            rows,
            col,
            row,
            force_size_pos,
            pos_first_only,
            force_shrink_move_grow,
        } => {
            if !cli.quiet {
                heading("Test: place-flex");
            }
            let timeout = cli.timeout;
            let logs = true; // logs only affect tracing env
            let mut overlay = None;
            if !cli.no_warn {
                overlay = crate::process::start_warn_overlay_with_delay();
                crate::process::write_overlay_status("place-flex");
                if let Some(info) = &cli.info {
                    crate::process::write_overlay_info(info);
                }
            }
            match run_on_main_with_watchdog("place-flex", timeout, move || {
                if logs {
                    // no-op: logging already initialized via RUST_LOG
                }
                tests::place_flex::run_place_flex(
                    cols,
                    rows,
                    col,
                    row,
                    force_size_pos,
                    pos_first_only,
                    force_shrink_move_grow,
                )
            }) {
                Ok(()) => {
                    if !cli.quiet {
                        println!(
                            "place-flex: OK (cols={} rows={} cell=({},{}), force_size_pos={}, pos_first_only={})",
                            cols, rows, col, row, force_size_pos, pos_first_only
                        );
                    }
                }
                Err(e) => {
                    eprintln!("place-flex: ERROR: {}", e);
                    print_hints(&e);
                    if let Some(mut o) = overlay {
                        if let Err(e) = o.kill_and_wait() {
                            eprintln!("smoketest: failed to stop overlay: {}", e);
                        }
                    }
                    std::process::exit(1);
                }
            }
            if let Some(mut o) = overlay {
                if let Err(e) = o.kill_and_wait() {
                    eprintln!("smoketest: failed to stop overlay: {}", e);
                }
            }
        }
        Commands::PlaceFallback => {
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
                        crate::config::PLACE_COLS,
                        crate::config::PLACE_ROWS,
                        0,
                        0,
                        true,  // force_size_pos
                        false, // pos_first_only
                        false, // force_shrink_move_grow
                    )
                },
            ) {
                Ok(()) => {
                    if !cli.quiet {
                        println!("place-fallback: OK (forced size->pos path)")
                    }
                }
                Err(e) => {
                    eprintln!("place-fallback: ERROR: {}", e);
                    print_hints(&e);
                    std::process::exit(1);
                }
            }
        }
        Commands::PlaceSmg => {
            let timeout = cli.timeout;
            match run_case(
                "place-smg (shrink→move→grow)",
                "place-smg",
                timeout,
                cli.quiet,
                !cli.no_warn,
                None,
                true,
                move || {
                    tests::place_flex::run_place_flex(
                        2,     // cols
                        2,     // rows
                        1,     // col (BR)
                        1,     // row (BR)
                        false, // force_size_pos
                        false, // pos_first_only
                        true,  // force_shrink_move_grow
                    )
                },
            ) {
                Ok(()) => {
                    if !cli.quiet {
                        println!("place-smg: OK (forced shrink→move→grow path)")
                    }
                }
                Err(e) => {
                    eprintln!("place-smg: ERROR: {}", e);
                    print_hints(&e);
                    std::process::exit(1);
                }
            }
        }
        Commands::PlaceSkip => {
            if !cli.quiet {
                heading("Test: place-skip (non-movable)");
            }
            let timeout = cli.timeout;
            let logs = true;
            let mut overlay = None;
            if !cli.no_warn {
                overlay = crate::process::start_warn_overlay_with_delay();
                crate::process::write_overlay_status("place-skip");
            }
            match run_on_main_with_watchdog("place-skip", timeout, move || {
                tests::place_skip::run_place_skip_test(timeout, logs)
            }) {
                Ok(()) => {
                    if !cli.quiet {
                        println!("place-skip: OK (engine skipped non-movable)")
                    }
                }
                Err(e) => {
                    eprintln!("place-skip: ERROR: {}", e);
                    print_hints(&e);
                    if let Some(mut o) = overlay {
                        if let Err(e) = o.kill_and_wait() {
                            eprintln!("smoketest: failed to stop overlay: {}", e);
                        }
                    }
                    std::process::exit(1);
                }
            }
            if let Some(mut o) = overlay {
                if let Err(e) = o.kill_and_wait() {
                    eprintln!("smoketest: failed to stop overlay: {}", e);
                }
            }
        }
        Commands::FocusNav => {
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
                        println!("focus-nav: OK (navigated right, down, left, up)")
                    }
                }
                Err(e) => {
                    eprintln!("focus-nav: ERROR: {}", e);
                    print_hints(&e);
                    std::process::exit(1);
                }
            }
        }
        Commands::Focus => {
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
                        println!(
                            "focus-tracking: OK (title='{}', pid={}, time_to_match_ms={})",
                            out.title, out.pid, out.elapsed_ms
                        );
                    }
                }
                Err(e) => {
                    eprintln!("focus-tracking: ERROR: {}", e);
                    print_hints(&e);
                    std::process::exit(1);
                }
            }
        }
        Commands::Hide => {
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
                        println!("hide: OK (toggle on/off roundtrip)")
                    }
                }
                Err(e) => {
                    eprintln!("hide: ERROR: {}", e);
                    print_hints(&e);
                    std::process::exit(1);
                }
            }
        }
        Commands::Place => {
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
                        println!("place: OK (cycled all grid cells)")
                    }
                }
                Err(e) => {
                    eprintln!("place: ERROR: {}", e);
                    print_hints(&e);
                    std::process::exit(1);
                }
            }
        }
        Commands::PlaceAsync => {
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
                        println!("place-async: OK (converged within default budget)")
                    }
                }
                Err(e) => {
                    eprintln!("place-async: ERROR: {}", e);
                    print_hints(&e);
                    std::process::exit(1);
                }
            }
        }
        Commands::PlaceAnimated => {
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
                        println!("place-animated: OK (converged with tween)")
                    }
                }
                Err(e) => {
                    eprintln!("place-animated: ERROR: {}", e);
                    print_hints(&e);
                    std::process::exit(1);
                }
            }
        }
        Commands::PlaceTerm => {
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
                        println!("place-term: OK (latched origin; no thrash)")
                    }
                }
                Err(e) => {
                    eprintln!("place-term: ERROR: {}", e);
                    print_hints(&e);
                    std::process::exit(1);
                }
            }
        }
        Commands::PlaceMoveMin => {
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
                        println!("place-move-min: OK (moved with min-height anchored)")
                    }
                }
                Err(e) => {
                    eprintln!("place-move-min: ERROR: {}", e);
                    print_hints(&e);
                    std::process::exit(1);
                }
            }
        }
        Commands::PlaceMinimized => {
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
                        println!("place-minimized: OK (normalized minimized -> placed)")
                    }
                }
                Err(e) => {
                    eprintln!("place-minimized: ERROR: {}", e);
                    print_hints(&e);
                    std::process::exit(1);
                }
            }
        }
        Commands::PlaceZoomed => {
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
                        println!("place-zoomed: OK (normalized zoomed -> placed)")
                    }
                }
                Err(e) => {
                    eprintln!("place-zoomed: ERROR: {}", e);
                    print_hints(&e);
                    std::process::exit(1);
                }
            }
        }
        Commands::FocusWinHelper { .. } => {
            // Already handled above
            unreachable!()
        }
        Commands::WarnOverlay { .. } => {
            // Already handled above
            unreachable!()
        }
        Commands::Ui => {
            if !cli.quiet {
                heading("Test: ui");
            }
            let timeout = cli.timeout;
            let mut overlay = None;
            if !cli.no_warn {
                overlay = crate::process::start_warn_overlay_with_delay();
                crate::process::write_overlay_status("ui");
                if let Some(info) = &cli.info {
                    crate::process::write_overlay_info(info);
                }
            }
            match run_with_watchdog("ui", timeout, move || ui::run_ui_demo(timeout)) {
                Ok(sum) => {
                    if !cli.quiet {
                        println!(
                            "ui: OK (hud_seen={}, time_to_hud_ms={:?})",
                            sum.hud_seen, sum.time_to_hud_ms
                        );
                    }
                }
                Err(e) => {
                    eprintln!("ui: ERROR: {}", e);
                    print_hints(&e);
                    if let Some(mut o) = overlay {
                        if let Err(e) = o.kill_and_wait() {
                            eprintln!("smoketest: failed to stop overlay: {}", e);
                        }
                    }
                    std::process::exit(1);
                }
            }
            if let Some(mut o) = overlay {
                if let Err(e) = o.kill_and_wait() {
                    eprintln!("smoketest: failed to stop overlay: {}", e);
                }
            }
        }
        // Screenshots extracted to separate tool: hotki-shots
        Commands::Minui => {
            if !cli.quiet {
                heading("Test: minui");
            }
            let timeout = cli.timeout;
            let mut overlay = None;
            if !cli.no_warn {
                overlay = crate::process::start_warn_overlay_with_delay();
                crate::process::write_overlay_status("minui");
                if let Some(info) = &cli.info {
                    crate::process::write_overlay_info(info);
                }
            }
            match run_with_watchdog("minui", timeout, move || ui::run_minui_demo(timeout)) {
                Ok(sum) => {
                    if !cli.quiet {
                        println!(
                            "minui: OK (hud_seen={}, time_to_hud_ms={:?})",
                            sum.hud_seen, sum.time_to_hud_ms
                        );
                    }
                }
                Err(e) => {
                    eprintln!("minui: ERROR: {}", e);
                    print_hints(&e);
                    if let Some(mut o) = overlay {
                        if let Err(e) = o.kill_and_wait() {
                            eprintln!("smoketest: failed to stop overlay: {}", e);
                        }
                    }
                    std::process::exit(1);
                }
            }
            if let Some(mut o) = overlay {
                if let Err(e) = o.kill_and_wait() {
                    eprintln!("smoketest: failed to stop overlay: {}", e);
                }
            }
        }
        Commands::Fullscreen { state, native } => {
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
                        println!("fullscreen: OK (toggled non-native fullscreen)")
                    }
                }
                Err(e) => {
                    eprintln!("fullscreen: ERROR: {}", e);
                    print_hints(&e);
                    std::process::exit(1);
                }
            }
        } // Preflight smoketest removed.
        Commands::WorldStatus => {
            if !cli.quiet {
                heading("Test: world-status");
            }
            let timeout = cli.timeout;
            match run_with_watchdog("world-status", timeout, move || {
                tests::world_status::run_world_status_test(timeout, true)
            }) {
                Ok(()) => {
                    if !cli.quiet {
                        println!("world-status: OK (permissions granted; status sane)")
                    }
                }
                Err(e) => {
                    eprintln!("world-status: ERROR: {}", e);
                    print_hints(&e);
                    std::process::exit(1);
                }
            }
        }
        Commands::WorldAx => {
            if !cli.quiet {
                heading("Test: world-ax");
            }
            let timeout = cli.timeout;
            match run_with_watchdog("world-ax", timeout, move || {
                tests::world_ax::run_world_ax_test(timeout, true)
            }) {
                Ok(()) => {
                    if !cli.quiet {
                        println!("world-ax: OK (role/subrole present; flags resolved)")
                    }
                }
                Err(e) => {
                    eprintln!("world-ax: ERROR: {}", e);
                    print_hints(&e);
                    std::process::exit(1);
                }
            }
        }
    }
}
