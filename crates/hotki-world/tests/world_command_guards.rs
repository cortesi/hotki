use std::{future::Future, sync::Arc};

use hotki_world::{
    MoveDirection, MoveIntent, PlaceIntent, RaiseIntent, World, WorldCfg, test_api as world_test,
    test_support::{override_scope, run_async_test, wait_snapshot_until},
};
use mac_winops::{
    AxProps, Pos, WindowId, WindowInfo,
    ops::{MockWinOps, WinOps},
};
use regex::Regex;

const FAST_COALESCE_MS: u64 = 30;

fn cfg_fast() -> WorldCfg {
    WorldCfg {
        poll_ms_min: 1,
        poll_ms_max: 10,
        include_offscreen: false,
        ax_watch_frontmost: false,
        events_buffer: 64,
    }
}

fn run_world_test<F>(coalesce_ms: Option<u64>, fut: F)
where
    F: Future<Output = ()> + Send + 'static,
{
    run_async_test(async move {
        let _guard = override_scope();
        world_test::set_accessibility_ok(true);
        world_test::set_screen_recording_ok(true);
        world_test::set_displays(vec![(1, 0, 0, 1920, 1080)]);
        if let Some(ms) = coalesce_ms {
            world_test::set_coalesce_ms(ms);
        }
        fut.await;
    });
}

fn win(
    app: &str,
    title: &str,
    pid: i32,
    id: WindowId,
    focused: bool,
    on_active_space: bool,
) -> WindowInfo {
    WindowInfo {
        app: app.into(),
        title: title.into(),
        pid,
        id,
        pos: Some(Pos {
            x: 0,
            y: 0,
            width: 800,
            height: 600,
        }),
        space: Some(if on_active_space { 1 } else { 2 }),
        layer: 0,
        focused,
        is_on_screen: true,
        on_active_space,
    }
}

fn ax(role: &str, subrole: &str, can_set_pos: bool) -> AxProps {
    AxProps {
        role: Some(role.into()),
        subrole: Some(subrole.into()),
        can_set_pos: Some(can_set_pos),
        can_set_size: Some(true),
        frame: None,
        minimized: Some(false),
        fullscreen: Some(false),
        visible: Some(true),
        zoomed: Some(false),
    }
}

#[test]
fn placement_guard_skips_guarded_roles() {
    run_world_test(Some(FAST_COALESCE_MS), async move {
        world_test::ensure_ax_pool_inited();
        world_test::ax_pool_reset_metrics_and_cache();
        world_test::set_ax_bridge_enabled(false);

        let mock = Arc::new(MockWinOps::new());
        let pid = 4242;
        let id = 7;
        mock.set_windows(vec![win("Guarded", "Sheet", pid, id, true, true)]);
        world_test::set_ax_props(pid, id, ax("AXSheet", "AXStandardWindow", true));

        let world = World::spawn(mock.clone() as Arc<dyn WinOps>, cfg_fast());

        assert!(
            wait_snapshot_until(&world, 200, |snap| {
                snap.iter().any(|w| {
                    w.id == id && w.ax.as_ref().and_then(|p| p.role.as_deref()) == Some("AXSheet")
                })
            })
            .await
        );

        let receipt = world
            .request_place_grid(PlaceIntent {
                cols: 6,
                rows: 4,
                col: 1,
                row: 1,
                pid_hint: Some(pid),
                target: None,
                options: None,
            })
            .await
            .expect("place guard succeed");
        assert!(
            receipt.target.is_none(),
            "guarded placement should skip target"
        );
        assert_eq!(
            mock.call_count("place_grid"),
            0,
            "no placement should be attempted"
        );

        let move_receipt = world
            .request_place_move_grid(MoveIntent {
                cols: 6,
                rows: 4,
                dir: MoveDirection::Right,
                pid_hint: Some(pid),
                target: None,
                options: None,
            })
            .await
            .expect("move guard succeed");
        assert!(
            move_receipt.target.is_none(),
            "guarded move should skip target"
        );
        assert_eq!(
            mock.call_count("place_move"),
            0,
            "no move should be attempted"
        );
    });
}

#[test]
fn placement_guard_allows_standard_windows() {
    run_world_test(Some(FAST_COALESCE_MS), async move {
        world_test::ensure_ax_pool_inited();
        world_test::ax_pool_reset_metrics_and_cache();
        world_test::set_ax_bridge_enabled(false);

        let mock = Arc::new(MockWinOps::new());
        let pid = 5555;
        let id = 8;
        mock.set_windows(vec![win("Normal", "Primary", pid, id, true, true)]);
        world_test::set_ax_props(pid, id, ax("AXWindow", "AXStandardWindow", true));

        let world = World::spawn(mock.clone() as Arc<dyn WinOps>, cfg_fast());

        assert!(
            wait_snapshot_until(&world, 200, |snap| {
                snap.iter().any(|w| w.id == id && w.ax.is_some())
            })
            .await
        );

        let receipt = world
            .request_place_grid(PlaceIntent {
                cols: 4,
                rows: 4,
                col: 0,
                row: 0,
                pid_hint: Some(pid),
                target: None,
                options: None,
            })
            .await
            .expect("place command");
        let target = receipt
            .target
            .expect("placement should pick focused window");
        assert_eq!(target.id, id);
        assert_eq!(
            mock.call_count("place_grid"),
            1,
            "placement should invoke backend"
        );

        let move_receipt = world
            .request_place_move_grid(MoveIntent {
                cols: 4,
                rows: 4,
                dir: MoveDirection::Left,
                pid_hint: Some(pid),
                target: None,
                options: None,
            })
            .await
            .expect("move command");
        let move_target = move_receipt
            .target
            .expect("move should pick focused window");
        assert_eq!(move_target.id, id);
        assert_eq!(
            mock.call_count("place_move"),
            1,
            "move should invoke backend"
        );
    });
}

#[test]
fn raise_intent_cycles_and_handles_off_space() {
    run_world_test(Some(FAST_COALESCE_MS), async move {
        world_test::ensure_ax_pool_inited();
        world_test::ax_pool_reset_metrics_and_cache();
        let mock = Arc::new(MockWinOps::new());
        let pid = 7777;
        let id1 = 10;
        let id2 = 11;
        let id3 = 12;
        mock.set_windows(vec![
            win("Alpha", "Alpha 1", pid, id1, true, true),
            win("Alpha", "Alpha 2", pid, id2, false, true),
            win("Alpha", "Alpha Off", pid, id3, false, false),
        ]);

        let world = World::spawn(mock.clone() as Arc<dyn WinOps>, cfg_fast());

        assert!(wait_snapshot_until(&world, 200, |snap| snap.len() == 3).await);

        let intent = RaiseIntent {
            app_regex: Some(Arc::new(Regex::new(r"^Alpha$").unwrap())),
            title_regex: Some(Arc::new(Regex::new(r"^Alpha ").unwrap())),
        };

        let receipt = world
            .request_raise(intent.clone())
            .await
            .expect("raise command");
        let first_target = receipt.target.expect("raise should find a target");
        assert_eq!(
            first_target.id, id2,
            "should cycle to the next matching window"
        );
        assert_eq!(mock.call_count("ensure_frontmost"), 1);

        mock.set_windows(vec![
            win("Alpha", "Alpha 1", pid, id1, false, true),
            win("Alpha", "Alpha 2", pid, id2, true, true),
            win("Alpha", "Alpha Off", pid, id3, false, false),
        ]);
        world.hint_refresh();
        assert!(
            wait_snapshot_until(&world, 200, |snap| {
                snap.iter().any(|w| w.id == id2 && w.focused)
            })
            .await
        );

        let second_receipt = world
            .request_raise(intent.clone())
            .await
            .expect("second raise");
        let second_target = second_receipt
            .target
            .expect("second raise should find target");
        assert_eq!(
            second_target.id, id1,
            "should wrap around to first candidate"
        );
        assert_eq!(mock.call_count("ensure_frontmost"), 2);

        let offspace_intent = RaiseIntent {
            app_regex: Some(Arc::new(Regex::new(r"^Alpha$").unwrap())),
            title_regex: Some(Arc::new(Regex::new(r"Off$").unwrap())),
        };
        let offspace_receipt = world
            .request_raise(offspace_intent)
            .await
            .expect("offspace raise");
        let offspace_target = offspace_receipt
            .target
            .expect("offspace candidate should be returned");
        assert_eq!(offspace_target.id, id3);
        assert!(!offspace_target.on_active_space);
        assert_eq!(mock.call_count("ensure_frontmost"), 3);
    });
}
