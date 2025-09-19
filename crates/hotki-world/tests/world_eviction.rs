use std::sync::Arc;

use hotki_world::{
    WindowKey, World, WorldCfg, WorldEvent, test_api as world_test,
    test_support::{
        drain_events, override_scope, recv_event_until, run_async_test, wait_metrics_until,
        wait_snapshot_until,
    },
};
use mac_winops::{
    Pos, WindowId, WindowInfo,
    ops::{MockWinOps, WinOps},
};

fn win(app: &str, title: &str, pid: i32, id: WindowId) -> WindowInfo {
    WindowInfo {
        app: app.into(),
        title: title.into(),
        pid,
        id,
        pos: Some(Pos {
            x: 0,
            y: 0,
            width: 100,
            height: 100,
        }),
        space: Some(1),
        layer: 0,
        focused: true,
        is_on_screen: true,
        on_active_space: true,
    }
}

fn cfg_fast() -> WorldCfg {
    // Use long polling so only hint_refresh drives reconcile in tests below
    WorldCfg {
        poll_ms_min: 1000,
        poll_ms_max: 1000,
        include_offscreen: false,
        ax_watch_frontmost: false,
        events_buffer: 64,
    }
}

#[test]
fn evicts_after_two_passes_when_missing() {
    run_async_test(async move {
        let _guard = override_scope();
        let mock = Arc::new(MockWinOps::new());
        mock.set_windows(vec![win("AppA", "A1", 100, 1)]);
        world_test::set_ax_bridge_enabled(false);
        world_test::set_accessibility_ok(false);
        world_test::set_screen_recording_ok(false);
        world_test::set_displays(vec![(1, 0, 0, 1920, 1080)]);
        let world = World::spawn(mock.clone() as Arc<dyn WinOps>, cfg_fast());
        tokio::task::yield_now().await;

        assert!(
            wait_snapshot_until(&world, 500, |s| s.iter().any(|w| w.pid == 100 && w.id == 1)).await,
            "window should be present initially"
        );

        // Remove from CG; first pass marks suspect, second confirms and removes
        mock.set_windows(vec![]);

        let mut rx = world.subscribe();
        drain_events(&mut rx);

        // Track reconcile progress via status instead of wall-clock delays.
        let baseline_seq = world.metrics_snapshot().reconcile_seq;
        world.hint_refresh();
        let first_snapshot =
            wait_metrics_until(&world, 1000, |metrics| metrics.reconcile_seq > baseline_seq)
                .await
                .expect("first reconcile pass should execute");
        let suspect_marked = first_snapshot.suspects_pending == 1;
        let removed_immediately = first_snapshot.windows_count == 0;
        assert!(
            suspect_marked || removed_immediately,
            "expected suspect mark or immediate removal, got {:?}",
            first_snapshot
        );

        world.hint_refresh();
        let second_pass = wait_metrics_until(&world, 1000, |metrics| {
            metrics.reconcile_seq > first_snapshot.reconcile_seq
        })
        .await
        .expect("window should be evicted on second pass");
        assert_eq!(second_pass.windows_count, 0, "expected window removal");
        assert_eq!(second_pass.suspects_pending, 0, "expected suspects cleared");

        let removed = recv_event_until(&mut rx, 200, |ev| {
            matches!(
                ev,
                WorldEvent::Removed(k) if *k == (WindowKey { pid: 100, id: 1 })
            )
        })
        .await;
        assert!(
            removed.is_some(),
            "expected removed event for missing window"
        );

        let final_snap = world.snapshot().await;
        assert!(
            !final_snap.iter().any(|w| w.pid == 100 && w.id == 1),
            "window should be absent from snapshot"
        );
    });
}

#[test]
fn pid_reuse_no_false_positive() {
    run_async_test(async move {
        let _guard = override_scope();
        let mock = Arc::new(MockWinOps::new());
        // Start with pid=100, id=1
        mock.set_windows(vec![win("OldApp", "Old", 100, 1)]);
        world_test::set_ax_bridge_enabled(false);
        world_test::set_accessibility_ok(false);
        world_test::set_screen_recording_ok(false);
        world_test::set_displays(vec![(1, 0, 0, 1920, 1080)]);
        let world = World::spawn(mock.clone() as Arc<dyn WinOps>, cfg_fast());
        tokio::task::yield_now().await;
        let _ =
            wait_snapshot_until(&world, 600, |s| s.iter().any(|w| w.pid == 100 && w.id == 1)).await;

        // First pass: old window disappears, new pid reuses same CG id (1)
        mock.set_windows(vec![win("NewApp", "New", 101, 1)]);
        let mut rx = world.subscribe();
        drain_events(&mut rx);

        let baseline_seq = world.metrics_snapshot().reconcile_seq;
        world.hint_refresh();
        let first_snapshot =
            wait_metrics_until(&world, 1000, |metrics| metrics.reconcile_seq > baseline_seq)
                .await
                .expect("first pass should run");

        world.hint_refresh();
        let second_pass = wait_metrics_until(&world, 1000, |metrics| {
            metrics.reconcile_seq > first_snapshot.reconcile_seq
        })
        .await
        .expect("second pass should resolve suspect set");
        assert_eq!(second_pass.suspects_pending, 0, "suspects should clear");

        let removed = recv_event_until(&mut rx, 200, |ev| {
            matches!(
                ev,
                WorldEvent::Removed(k) if *k == (WindowKey { pid: 100, id: 1 })
            )
        })
        .await;
        assert!(removed.is_some(), "expected removed event for old window");

        let snap = world.snapshot().await;
        assert!(
            !snap.iter().any(|w| w.pid == 100 && w.id == 1),
            "old window must be gone"
        );
        assert!(
            snap.iter().any(|w| w.pid == 101 && w.id == 1),
            "new window must remain"
        );
    });
}

#[test]
fn confirmation_snapshot_reused_across_suspects() {
    run_async_test(async move {
        let _guard = override_scope();
        let mock = Arc::new(MockWinOps::new());
        mock.set_windows(vec![win("AppA", "A1", 200, 1), win("AppB", "B1", 201, 2)]);
        world_test::set_ax_bridge_enabled(false);
        world_test::set_accessibility_ok(false);
        world_test::set_screen_recording_ok(false);
        world_test::set_displays(vec![(1, 0, 0, 1920, 1080)]);

        let world = World::spawn(mock.clone() as Arc<dyn WinOps>, cfg_fast());
        tokio::task::yield_now().await;
        assert!(
            wait_snapshot_until(&world, 500, |s| s.len() == 2).await,
            "expected two windows to be tracked initially"
        );

        mock.set_windows(vec![]);
        drain_events(&mut world.subscribe()); // drop immediate Added events we don't care about

        let initial_calls = mock.call_count("list_windows_for_spaces");
        let baseline_seq = world.metrics_snapshot().reconcile_seq;

        world.hint_refresh();
        let first_pass =
            wait_metrics_until(&world, 1000, |metrics| metrics.reconcile_seq > baseline_seq)
                .await
                .expect("first reconcile pass should execute");
        assert_eq!(
            mock.call_count("list_windows_for_spaces") - initial_calls,
            1,
            "first pass should only enumerate once"
        );
        assert_eq!(
            first_pass.suspects_pending, 2,
            "both windows should be suspect"
        );

        let calls_before_second = mock.call_count("list_windows_for_spaces");
        world.hint_refresh();
        let second_pass = wait_metrics_until(&world, 1000, |metrics| {
            metrics.reconcile_seq > first_pass.reconcile_seq
        })
        .await
        .expect("second reconcile pass should execute");

        let calls_after_second = mock.call_count("list_windows_for_spaces");
        assert_eq!(
            calls_after_second - calls_before_second,
            2,
            "second pass should reuse a single confirmation snapshot for both suspects"
        );
        assert_eq!(second_pass.windows_count, 0, "windows should be evicted");
        assert_eq!(
            second_pass.suspects_pending, 0,
            "suspects should be cleared"
        );
    });
}
