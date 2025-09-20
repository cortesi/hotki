use std::sync::Arc;

use hotki_world::{
    WindowKey, World, WorldCfg, WorldEvent, test_api as world_test,
    test_support::{
        drain_events, override_scope, recv_event_until, run_async_test, wait_snapshot_until,
    },
};
use mac_winops::{
    Pos, WindowInfo,
    ops::{MockWinOps, WinOps},
};

fn cfg_fast() -> WorldCfg {
    WorldCfg {
        poll_ms_min: 5,
        poll_ms_max: 20,
        include_offscreen: false,
        ax_watch_frontmost: false,
        events_buffer: 64,
    }
}

fn base_window() -> WindowInfo {
    WindowInfo {
        app: "App".into(),
        title: "Win".into(),
        pid: 4242,
        id: 7,
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

#[test]
fn window_space_transition_yields_update() {
    run_async_test(async move {
        let _guard = override_scope();
        let mock = Arc::new(MockWinOps::new());
        mock.set_windows(vec![base_window()]);
        world_test::set_ax_bridge_enabled(false);
        world_test::set_accessibility_ok(false);
        world_test::set_screen_recording_ok(false);
        world_test::set_displays(vec![(1, 0, 0, 1920, 1080)]);
        let world = World::spawn(mock.clone() as Arc<dyn WinOps>, cfg_fast());
        tokio::task::yield_now().await;
        let mut cursor = world.subscribe();

        assert!(
            wait_snapshot_until(&world, 200, |s| s.len() == 1).await,
            "initial window should be present"
        );
        drain_events(&world, &mut cursor);

        // Move the window to a different Mission Control space (off active space)
        let mut moved = base_window();
        moved.space = Some(2);
        moved.focused = false;
        moved.is_on_screen = false;
        moved.on_active_space = false;
        mock.set_windows(vec![moved]);

        world.hint_refresh();
        let key = WindowKey { pid: 4242, id: 7 };
        let evt = recv_event_until(
            &world,
            &mut cursor,
            300,
            |ev| matches!(ev, WorldEvent::Updated(k, _) if *k == key),
        )
        .await;
        assert!(matches!(evt, Some(WorldEvent::Updated(k, _)) if k == key));

        let snap = world.snapshot().await;
        let entry = snap.iter().find(|w| w.pid == 4242 && w.id == 7).unwrap();
        assert_eq!(entry.space, Some(2));
        assert!(!entry.on_active_space);
        assert!(!entry.is_on_screen);
    });
}
