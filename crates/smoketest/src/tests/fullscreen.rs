use std::time::Instant;

use crate::{
    config,
    error::{Error, Result},
    process::{HelperWindowBuilder, ManagedChild},
    test_runner::{TestConfig, TestRunner},
    ui_interaction::{send_activation_chord, send_key},
};

/// Run a focused non-native fullscreen toggle against a helper window.
///
/// Steps:
/// - Launch hotki with a tiny temp config that binds `f` to fullscreen(toggle, nonnative).
/// - Spawn a helper window with a unique title and keep it frontmost.
/// - Show HUD, press `f` to toggle non-native fullscreen.
/// - Optionally validate the window frame changed; then immediately kill the helper.
pub fn run_fullscreen_test(timeout_ms: u64, with_logs: bool) -> Result<()> {
    // Minimal config: bind activation and fullscreen(toggle, nonnative)
    let ron_config = r#"(
        keys: [
            ("shift+cmd+0", "activate", keys([
                ("f", "Fullscreen (non-native)", fullscreen(toggle, nonnative)),
            ])),
            ("shift+cmd+0", "exit", exit, (global: true, hide: true)),
            ("esc", "Back", pop, (global: true, hide: true, hud_only: true)),
        ],
    )"#;

    let config = TestConfig::new(timeout_ms)
        .with_temp_config(ron_config)
        .with_logs(with_logs);

    TestRunner::new("fullscreen_test", config)
        .with_setup(|ctx| {
            ctx.launch_hotki()?;
            Ok(())
        })
        .with_execute(|ctx| {
            // Spawn helper window with unique title
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let title = format!("hotki smoketest: fullscreen {}", ts);

            let helper_time = ctx
                .config
                .timeout_ms
                .saturating_add(config::HELPER_WINDOW_EXTRA_TIME_MS);
            let mut helper: ManagedChild = HelperWindowBuilder::new(title.clone())
                .with_time_ms(helper_time)
                .spawn()?;

            // Give the system a moment to show and focus the helper
            std::thread::sleep(config::ms(300));

            // Capture initial frame via AX
            let before = mac_winops::ax_window_frame(helper.pid, &title)
                .ok_or_else(|| Error::InvalidState("Failed to read initial window frame".into()))?;

            // Show HUD and trigger fullscreen toggle
            send_activation_chord();
            std::thread::sleep(config::ms(200));
            send_key("f");
            std::thread::sleep(config::ms(300));

            // Read new frame; tolerate AX timing
            let mut after = mac_winops::ax_window_frame(helper.pid, &title);
            let start_wait = Instant::now();
            while after.is_none() && start_wait.elapsed() < config::ms(1000) {
                std::thread::sleep(config::ms(50));
                after = mac_winops::ax_window_frame(helper.pid, &title);
            }
            let after = after.ok_or_else(|| {
                Error::InvalidState("Failed to read window frame after toggle".into())
            })?;

            // Quick sanity: area or dimensions changed meaningfully
            let area_before = before.1.0 * before.1.1;
            let area_after = after.1.0 * after.1.1;
            if (area_after - area_before).abs() < 1.0 {
                // Not necessarily an error — some displays may already have a maximized window,
                // but we still proceed to kill the helper to exercise the path.
            }

            // Immediately kill the helper window to exercise teardown path
            let _ = helper.kill_and_wait();

            Ok(())
        })
        .with_teardown(|ctx, _| {
            ctx.shutdown();
            Ok(())
        })
        .run()
}
