use std::sync::Arc;

use hotki_world::{
    FrameKind, PlaceIntent, WindowKey, WindowMode, World, WorldCfg, test_api as world_test,
    test_support::{TestHarness, override_scope, wait_frames_until, wait_snapshot_until},
};
use mac_winops::{
    AxProps, Pos, Rect, WindowInfo,
    ops::{MockWinOps, WinOps},
};

fn cfg_fast() -> WorldCfg {
    WorldCfg {
        poll_ms_min: 1,
        poll_ms_max: 10,
        include_offscreen: false,
        ax_watch_frontmost: false,
        events_buffer: 64,
    }
}

#[allow(clippy::too_many_arguments)]
fn win(
    app: &str,
    title: &str,
    pid: i32,
    id: u32,
    pos: Pos,
    focused: bool,
    is_on_screen: bool,
    on_active_space: bool,
) -> WindowInfo {
    WindowInfo {
        app: app.into(),
        title: title.into(),
        pid,
        id,
        pos: Some(pos),
        space: Some(1),
        layer: 0,
        focused,
        is_on_screen,
        on_active_space,
    }
}

fn ax_props(
    frame: Rect,
    minimized: bool,
    visible: Option<bool>,
    fullscreen: bool,
    subrole: &str,
) -> AxProps {
    AxProps {
        role: Some("AXWindow".into()),
        subrole: Some(subrole.into()),
        can_set_pos: Some(true),
        can_set_size: Some(true),
        frame: Some(frame),
        minimized: Some(minimized),
        fullscreen: Some(fullscreen),
        visible,
        zoomed: Some(false),
    }
}

#[test]
fn frames_cache_last_authoritative_when_minimized() {
    let _guard = override_scope();
    world_test::ensure_ax_pool_inited();
    world_test::ax_pool_reset_metrics_and_cache();
    world_test::set_ax_bridge_enabled(false);
    world_test::set_accessibility_ok(true);
    world_test::set_screen_recording_ok(true);
    world_test::set_displays(vec![(1, 0, 0, 1920, 1080)]);

    let harness = TestHarness::new();
    let mock = Arc::new(MockWinOps::new());
    let pid = 4242;
    let id = 7;
    let initial_pos = Pos {
        x: 200,
        y: 150,
        width: 640,
        height: 480,
    };
    mock.set_windows(vec![win(
        "App",
        "A",
        pid,
        id,
        initial_pos,
        true,
        true,
        true,
    )]);
    let world = harness.block_on({
        let mock = mock.clone();
        async move { World::spawn(mock as Arc<dyn WinOps>, cfg_fast()) }
    });

    let snapshot_ready = harness.block_on({
        let world = world.clone();
        async move { wait_snapshot_until(&world, 200, |snap| snap.len() == 1).await }
    });
    assert!(snapshot_ready);
    let key = WindowKey { pid, id };

    let frame = Rect::new(200.0, 150.0, 640.0, 480.0);
    world_test::set_ax_props(
        pid,
        id,
        ax_props(frame, true, Some(false), false, "AXStandardWindow"),
    );
    mock.set_windows(vec![win(
        "App",
        "A",
        pid,
        id,
        initial_pos,
        false,
        false,
        false,
    )]);
    world.hint_refresh();

    let minimized_ready = harness.block_on({
        let world = world.clone();
        async move {
            wait_frames_until(&world, 800, move |frames| {
                matches!(
                    frames.get(&key).map(|f| f.mode),
                    Some(WindowMode::Minimized)
                )
            })
            .await
        }
    });
    assert!(minimized_ready);

    let frames = harness
        .block_on({
            let world = world.clone();
            async move { world.frames_snapshot().await }
        })
        .remove(&key)
        .expect("frames present");
    assert_eq!(frames.mode, WindowMode::Minimized);
    assert_eq!(frames.authoritative_kind, FrameKind::Cached);
    assert_eq!(
        frames.authoritative,
        hotki_world::RectPx::from_pos(&initial_pos)
    );

    let eps = harness.block_on({
        let world = world.clone();
        async move { world.authoritative_eps(1).await }
    });
    assert_eq!(eps, 0);

    let place = harness.block_on({
        let world = world.clone();
        async move {
            world
                .request_place_grid(PlaceIntent {
                    cols: 2,
                    rows: 2,
                    col: 0,
                    row: 0,
                    pid_hint: Some(pid),
                    target: None,
                    options: None,
                })
                .await
        }
    });
    assert!(matches!(
        place,
        Err(hotki_world::CommandError::InvalidRequest { .. }
            | hotki_world::CommandError::OffActiveSpace { .. })
    ));
}

#[test]
fn placement_rejects_hidden_and_fullscreen_modes() {
    let _guard = override_scope();
    world_test::ensure_ax_pool_inited();
    world_test::ax_pool_reset_metrics_and_cache();
    world_test::set_ax_bridge_enabled(false);
    world_test::set_accessibility_ok(true);
    world_test::set_screen_recording_ok(true);
    world_test::set_displays(vec![(1, 0, 0, 1920, 1080)]);

    let harness = TestHarness::new();
    let mock = Arc::new(MockWinOps::new());
    let pid = 9898;
    let id = 11;
    let pos = Pos {
        x: 20,
        y: 30,
        width: 800,
        height: 600,
    };
    mock.set_windows(vec![win("App", "B", pid, id, pos, true, true, true)]);
    let world = harness.block_on({
        let mock = mock.clone();
        async move { World::spawn(mock as Arc<dyn WinOps>, cfg_fast()) }
    });
    let snapshot_ready = harness.block_on({
        let world = world.clone();
        async move { wait_snapshot_until(&world, 200, |snap| snap.len() == 1).await }
    });
    assert!(snapshot_ready);
    let key = WindowKey { pid, id };

    let rect = Rect::new(20.0, 30.0, 800.0, 600.0);
    // Hidden mode
    world_test::ax_pool_reset_metrics_and_cache();
    world_test::set_ax_props(
        pid,
        id,
        ax_props(rect, false, Some(false), false, "AXStandardWindow"),
    );
    mock.set_windows(vec![win("App", "B", pid, id, pos, false, false, false)]);
    world.hint_refresh();
    let hidden_ready = harness.block_on({
        let world = world.clone();
        async move {
            wait_frames_until(&world, 800, move |frames| {
                matches!(frames.get(&key).map(|f| f.mode), Some(WindowMode::Hidden))
            })
            .await
        }
    });
    assert!(hidden_ready);
    let hidden = harness
        .block_on({
            let world = world.clone();
            async move { world.frames_snapshot().await }
        })
        .remove(&key)
        .expect("frames present");
    assert_eq!(hidden.mode, WindowMode::Hidden);

    let place_hidden = harness.block_on({
        let world = world.clone();
        async move {
            world
                .request_place_grid(PlaceIntent {
                    cols: 2,
                    rows: 2,
                    col: 0,
                    row: 0,
                    pid_hint: Some(pid),
                    target: None,
                    options: None,
                })
                .await
        }
    });
    assert!(matches!(
        place_hidden,
        Err(hotki_world::CommandError::InvalidRequest { .. }
            | hotki_world::CommandError::OffActiveSpace { .. })
    ));

    // Fullscreen mode
    world_test::ax_pool_reset_metrics_and_cache();
    world_test::set_ax_props(
        pid,
        id,
        ax_props(rect, false, Some(true), true, "AXFullScreenWindow"),
    );
    mock.set_windows(vec![win("App", "B", pid, id, pos, true, true, true)]);
    world.hint_refresh();
    let fullscreen_ready = harness.block_on({
        let world = world.clone();
        async move {
            wait_frames_until(&world, 800, move |frames| {
                matches!(
                    frames.get(&key).map(|f| f.mode),
                    Some(WindowMode::Fullscreen)
                )
            })
            .await
        }
    });
    assert!(fullscreen_ready);
    let fullscreen = harness
        .block_on({
            let world = world.clone();
            async move { world.frames_snapshot().await }
        })
        .remove(&key)
        .expect("frames present");
    assert_eq!(fullscreen.mode, WindowMode::Fullscreen);

    let place_fullscreen = harness.block_on({
        let world = world.clone();
        async move {
            world
                .request_place_grid(PlaceIntent {
                    cols: 2,
                    rows: 2,
                    col: 1,
                    row: 1,
                    pid_hint: Some(pid),
                    target: None,
                    options: None,
                })
                .await
        }
    });
    assert!(matches!(
        place_fullscreen,
        Err(hotki_world::CommandError::InvalidRequest { .. }
            | hotki_world::CommandError::OffActiveSpace { .. })
    ));

    // Tiled mode (fullscreen + standard subrole)
    world_test::ax_pool_reset_metrics_and_cache();
    world_test::set_ax_props(
        pid,
        id,
        ax_props(rect, false, Some(true), true, "AXStandardWindow"),
    );
    world.hint_refresh();
    let tiled_ready = harness.block_on({
        let world = world.clone();
        async move {
            wait_frames_until(&world, 800, move |frames| {
                matches!(frames.get(&key).map(|f| f.mode), Some(WindowMode::Tiled))
            })
            .await
        }
    });
    assert!(tiled_ready);
    let place_tiled = harness.block_on({
        let world = world.clone();
        async move {
            world
                .request_place_grid(PlaceIntent {
                    cols: 3,
                    rows: 3,
                    col: 2,
                    row: 2,
                    pid_hint: Some(pid),
                    target: None,
                    options: None,
                })
                .await
        }
    });
    assert!(matches!(
        place_tiled,
        Err(hotki_world::CommandError::InvalidRequest { .. }
            | hotki_world::CommandError::OffActiveSpace { .. })
    ));
}
