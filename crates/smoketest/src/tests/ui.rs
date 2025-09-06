use crate::{
    error::Result,
    results::Summary,
    test_runner::{TestConfig, TestRunner},
    ui_interaction::send_key_sequence,
};

/// Run the standard UI demo test.
pub fn run_ui_demo(timeout_ms: u64) -> Result<Summary> {
    let config = TestConfig::new(timeout_ms).with_logs(true);

    TestRunner::new("ui_demo", config)
        .with_setup(|ctx| {
            ctx.launch_hotki()?;
            Ok(())
        })
        .with_execute(|ctx| {
            let time_to_hud = ctx.wait_for_hud()?;

            // Send key sequence to test UI
            let mut seq: Vec<&str> = Vec::new();
            seq.push("t");
            seq.extend(std::iter::repeat_n("l", 5));
            seq.push("esc");
            seq.push("shift+cmd+0");
            send_key_sequence(&seq);

            let mut sum = Summary::new();
            sum.hud_seen = true;
            sum.time_to_hud_ms = Some(time_to_hud);
            Ok(sum)
        })
        .with_teardown(|ctx, _| {
            ctx.shutdown();
            Ok(())
        })
        .run()
}

/// Run the mini UI demo test.
pub fn run_minui_demo(timeout_ms: u64) -> Result<Summary> {
    let ron_config = r#"(
        keys: [
            ("shift+cmd+0", "activate", keys([
                ("t", "Theme tester", keys([
                    ("h", "Theme Prev", theme_prev, (noexit: true)),
                    ("l", "Theme Next", theme_next, (noexit: true)),
                ])),
            ])),
            ("shift+cmd+0", "exit", exit, (global: true, hide: true)),
            ("esc", "Back", pop, (global: true, hide: true, hud_only: true)),
        ],
        style: (hud: (mode: mini)),
    )"#;

    let config = TestConfig::new(timeout_ms)
        .with_temp_config(ron_config)
        .with_logs(false);

    TestRunner::new("minui_demo", config)
        .with_setup(|ctx| {
            ctx.launch_hotki()?;
            Ok(())
        })
        .with_execute(|ctx| {
            let time_to_hud = ctx.wait_for_hud()?;

            // Send key sequence to test mini UI
            let mut seq: Vec<&str> = Vec::new();
            seq.push("t");
            seq.extend(std::iter::repeat_n("l", 5));
            seq.push("esc");
            seq.push("shift+cmd+0");
            send_key_sequence(&seq);

            let mut sum = Summary::new();
            sum.hud_seen = true;
            sum.time_to_hud_ms = Some(time_to_hud);
            Ok(sum)
        })
        .with_teardown(|ctx, _| {
            ctx.shutdown();
            Ok(())
        })
        .run()
}
