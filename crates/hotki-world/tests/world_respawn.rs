use std::{sync::Arc, time::Duration};

use hotki_world::{
    World, WorldCfg, test_api as world_test,
    test_support::{override_scope, run_async_test, wait_snapshot_until},
};
use mac_winops::{
    Pos, WindowId, WindowInfo,
    ops::{MockWinOps, WinOps},
};
use tokio::time::{Instant, sleep};

fn cfg_slow_poll() -> WorldCfg {
    WorldCfg {
        poll_ms_min: 1000,
        poll_ms_max: 1000,
        include_offscreen: false,
        ax_watch_frontmost: false,
        events_buffer: 64,
    }
}

fn win(app: &str, title: &str, pid: i32, id: WindowId, focused: bool) -> WindowInfo {
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
        focused,
        is_on_screen: true,
        on_active_space: true,
    }
}

#[test]
fn ax_pool_hint_reaches_respawned_world() {
    run_async_test(async move {
        let _guard = override_scope();
        world_test::clear();
        world_test::set_accessibility_ok(true);
        world_test::set_screen_recording_ok(true);
        world_test::set_displays(vec![(1, 0, 0, 1920, 1080)]);
        world_test::set_ax_bridge_enabled(false);
        world_test::set_ax_async_only(true);
        world_test::ensure_ax_pool_inited();
        world_test::ax_pool_reset_metrics_and_cache();

        let mock = Arc::new(MockWinOps::new());
        let pid = 4242;
        let id: WindowId = 7;
        mock.set_windows(vec![win("AppA", "Initial", pid, id, true)]);

        let cfg = cfg_slow_poll();
        let world1 = World::spawn(mock.clone() as Arc<dyn WinOps>, cfg.clone());
        assert!(wait_snapshot_until(&world1, 500, |snap| snap.len() == 1).await);

        // Create a worker tied to this pid so it survives the first world instance.
        world_test::set_ax_title(id, "T-1");
        let deadline = Instant::now() + Duration::from_millis(2500);
        loop {
            if matches!(
                world_test::ax_pool_peek_title(pid, id).as_deref(),
                Some("T-1")
            ) {
                break;
            }
            if let Some(title) = world_test::ax_pool_schedule_title(pid, id) {
                assert_eq!(title, "T-1");
                break;
            }
            if Instant::now() >= deadline {
                panic!("timed out waiting for initial AX title cache");
            }
            sleep(Duration::from_millis(10)).await;
        }

        drop(world1);
        tokio::task::yield_now().await;

        // Spawn a fresh world instance that should reuse the AX worker pool.
        mock.set_windows(vec![win("AppA", "Initial", pid, id, true)]);
        let world2 = World::spawn(mock.clone() as Arc<dyn WinOps>, cfg.clone());
        assert!(wait_snapshot_until(&world2, 500, |snap| snap.len() == 1).await);

        // Change underlying windows; only a timely HintRefresh should reveal the update before
        // the poll interval elapses.
        let id_new: WindowId = 8;
        mock.set_windows(vec![
            win("AppA", "Initial", pid, id, true),
            win("AppB", "Second", pid, id_new, false),
        ]);

        world_test::set_ax_title(id_new, "T-2");
        assert!(world_test::ax_pool_schedule_title(pid, id_new).is_none());

        let refreshed = wait_snapshot_until(&world2, 600, |snap| snap.len() == 2).await;
        assert!(
            refreshed,
            "expected HintRefresh from reused AX worker to reach respawned world before poll interval",
        );

        drop(world2);
    });
}
