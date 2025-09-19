use std::{future::Future, sync::Arc, time::Duration};

use hotki_world::{
    WindowKey, World, WorldCfg, WorldEvent, test_api as world_test,
    test_support::{
        drain_events, override_scope, recv_event_until, run_async_test, wait_debounce_pending,
        wait_snapshot_until,
    },
};
use mac_winops::{
    Pos, WindowId, WindowInfo,
    ops::{MockWinOps, WinOps},
};

fn win(
    app: &str,
    title: &str,
    pid: i32,
    id: WindowId,
    pos: Pos,
    layer: i32,
    focused: bool,
) -> WindowInfo {
    WindowInfo {
        app: app.into(),
        title: title.into(),
        pid,
        id,
        pos: Some(pos),
        space: Some(1),
        layer,
        focused,
        is_on_screen: true,
        on_active_space: true,
    }
}

fn cfg_fast() -> WorldCfg {
    WorldCfg {
        poll_ms_min: 1,
        poll_ms_max: 10,
        include_offscreen: false,
        ax_watch_frontmost: false,
        events_buffer: 64,
    }
}

const FAST_COALESCE_MS: u64 = 30;

fn run_world_test<F>(coalesce_ms: Option<u64>, fut: F)
where
    F: Future<Output = ()> + Send + 'static,
{
    run_async_test(async move {
        let _guard = override_scope();
        world_test::set_accessibility_ok(true);
        world_test::set_screen_recording_ok(true);
        world_test::set_displays(vec![(1, 0, 0, 1920, 1080), (2, 1920, 0, 1920, 1080)]);
        if let Some(ms) = coalesce_ms {
            world_test::set_coalesce_ms(ms);
        }
        fut.await;
    });
}

#[test]
fn startup_adds_and_z_order() {
    run_world_test(Some(FAST_COALESCE_MS), async move {
        let mock = Arc::new(MockWinOps::new());
        world_test::set_displays(vec![(1, 0, 0, 1920, 1080), (2, 1920, 0, 1920, 1080)]);
        mock.set_windows(vec![
            win(
                "AppA",
                "A1",
                100,
                1,
                Pos {
                    x: 0,
                    y: 0,
                    width: 100,
                    height: 100,
                },
                0,
                true,
            ), // frontmost
            win(
                "AppB",
                "B1",
                200,
                2,
                Pos {
                    x: 0,
                    y: 0,
                    width: 100,
                    height: 100,
                },
                0,
                false,
            ),
        ]);
        let world = World::spawn(mock.clone() as Arc<dyn WinOps>, cfg_fast());

        assert!(wait_snapshot_until(&world, 200, |s| s.len() == 2).await);
        let mut snap = world.snapshot().await;
        snap.sort_by_key(|w| (w.z, w.pid, w.id));
        assert_eq!(snap[0].app, "AppA");
        assert_eq!(snap[0].z, 0);
        assert!(snap[0].on_active_space);
        assert_eq!(snap[1].app, "AppB");
        assert_eq!(snap[1].z, 1);
        assert!(snap[1].on_active_space);
    });
}

#[test]
fn focus_changes_emit_event_and_snapshot_updates() {
    run_world_test(Some(FAST_COALESCE_MS), async move {
        let mock = Arc::new(MockWinOps::new());
        mock.set_windows(vec![
            win(
                "AppA",
                "A1",
                100,
                1,
                Pos {
                    x: 0,
                    y: 0,
                    width: 100,
                    height: 100,
                },
                0,
                true,
            ),
            win(
                "AppB",
                "B1",
                200,
                2,
                Pos {
                    x: 0,
                    y: 0,
                    width: 100,
                    height: 100,
                },
                0,
                false,
            ),
        ]);
        let world = World::spawn(mock.clone() as Arc<dyn WinOps>, cfg_fast());
        let mut rx = world.subscribe();

        // Wait for initial reconcile and drain any startup events
        let _ = wait_snapshot_until(&world, 200, |s| s.len() == 2).await;
        drain_events(&mut rx);

        // Flip focus to AppB
        mock.set_windows(vec![
            win(
                "AppA",
                "A1",
                100,
                1,
                Pos {
                    x: 0,
                    y: 0,
                    width: 100,
                    height: 100,
                },
                0,
                false,
            ),
            win(
                "AppB",
                "B1",
                200,
                2,
                Pos {
                    x: 0,
                    y: 0,
                    width: 100,
                    height: 100,
                },
                0,
                true,
            ),
        ]);

        // Observe FocusChanged event to AppB
        let ev = recv_event_until(&mut rx, 220, |ev| {
            if let WorldEvent::FocusChanged(change) = ev {
                change.key == Some(WindowKey { pid: 200, id: 2 })
            } else {
                false
            }
        })
        .await;
        match ev {
            Some(WorldEvent::FocusChanged(change)) => {
                assert_eq!(change.key, Some(WindowKey { pid: 200, id: 2 }));
                assert_eq!(change.app.as_deref(), Some("AppB"));
                assert_eq!(change.title.as_deref(), Some("B1"));
                assert_eq!(change.pid, Some(200));
            }
            _ => panic!("expected FocusChanged to AppB"),
        }

        // Snapshot should reflect focused flags
        let snap = world.snapshot().await;
        let a = snap.iter().find(|w| w.pid == 100).unwrap();
        let b = snap.iter().find(|w| w.pid == 200).unwrap();
        assert!(!a.focused);
        assert!(b.focused);
    });
}

#[test]
fn hint_refresh_via_trait_updates_snapshot() {
    run_world_test(Some(FAST_COALESCE_MS), async move {
        let mock = Arc::new(MockWinOps::new());
        mock.set_windows(vec![win(
            "AppA",
            "A1",
            100,
            1,
            Pos {
                x: 0,
                y: 0,
                width: 400,
                height: 300,
            },
            0,
            true,
        )]);
        let world = World::spawn_view(mock.clone() as Arc<dyn WinOps>, cfg_fast());
        assert!(wait_snapshot_until(world.as_ref(), 200, |snap| snap.len() == 1).await);

        mock.set_windows(vec![win(
            "AppB",
            "B2",
            200,
            2,
            Pos {
                x: 10,
                y: 10,
                width: 800,
                height: 600,
            },
            0,
            true,
        )]);
        world.hint_refresh();
        let updated = wait_snapshot_until(world.as_ref(), 200, |snap| {
            snap.iter()
                .any(|w| w.pid == 200 && w.title == "B2" && w.focused)
        })
        .await;
        assert!(updated, "world snapshot did not refresh after hint");
    });
}

#[test]
fn title_update_reflected_in_snapshot() {
    run_world_test(Some(FAST_COALESCE_MS), async move {
        let mock = Arc::new(MockWinOps::new());
        mock.set_windows(vec![win(
            "AppA",
            "Old",
            100,
            1,
            Pos {
                x: 0,
                y: 0,
                width: 100,
                height: 100,
            },
            0,
            true,
        )]);
        let world = World::spawn(mock.clone() as Arc<dyn WinOps>, cfg_fast());
        let _rx = world.subscribe();

        world.hint_refresh();
        tokio::time::sleep(Duration::from_millis(5)).await;
        assert!(
            wait_snapshot_until(&world, 200, |s| s.len() == 1).await,
            "expected initial window in snapshot"
        );
        assert!(
            wait_debounce_pending(&world, 0, 200).await,
            "initial debounce queue did not drain"
        );

        mock.set_windows(vec![win(
            "AppA",
            "New",
            100,
            1,
            Pos {
                x: 0,
                y: 0,
                width: 100,
                height: 100,
            },
            0,
            true,
        )]);

        assert!(
            wait_snapshot_until(&world, 220, |s| s
                .iter()
                .any(|w| w.id == 1 && w.title == "New"))
            .await,
            "snapshot did not reflect title change"
        );
    });
}

#[test]
fn ax_focus_and_title_precedence() {
    run_world_test(Some(FAST_COALESCE_MS), async move {
        // Two windows for same app; CG marks first as focused, but AX will point to second.
        let mock = Arc::new(MockWinOps::new());
        mock.set_windows(vec![
            win(
                "AppA",
                "CG-Title-1",
                10,
                101,
                Pos {
                    x: 0,
                    y: 0,
                    width: 100,
                    height: 100,
                },
                0,
                true,
            ),
            win(
                "AppA",
                "CG-Title-2",
                10,
                102,
                Pos {
                    x: 0,
                    y: 0,
                    width: 100,
                    height: 100,
                },
                0,
                false,
            ),
        ]);

        // Force AX path and declare AX focused window + AX title for the second window.
        world_test::set_accessibility_ok(true);
        world_test::set_ax_focus(10, 102);
        world_test::set_ax_title(102, "AX-Title-2");

        let world = World::spawn(mock.clone() as Arc<dyn WinOps>, cfg_fast());
        // Wait until AX-focused window is reflected in snapshot
        assert!(
            wait_snapshot_until(&world, 250, |s| {
                s.len() == 2
                    && s.iter()
                        .any(|w| w.id == 102 && w.focused && w.title == "AX-Title-2")
            })
            .await
        );
        let snap = world.snapshot().await;
        let a1 = snap.iter().find(|w| w.id == 101).unwrap();
        let a2 = snap.iter().find(|w| w.id == 102).unwrap();
        assert!(!a1.focused);
        assert!(a2.focused);
        assert_eq!(a2.title, "AX-Title-2");
    });
}

#[test]
fn display_mapping_selects_best_overlap() {
    run_world_test(Some(FAST_COALESCE_MS), async move {
        // Two displays side-by-side: left id=1, right id=2
        world_test::set_displays(vec![(1, 0, 0, 800, 600), (2, 800, 0, 800, 600)]);
        let mock = Arc::new(MockWinOps::new());
        mock.set_windows(vec![
            win(
                "AppA",
                "Left",
                1,
                1,
                Pos {
                    x: 10,
                    y: 10,
                    width: 200,
                    height: 200,
                },
                0,
                true,
            ),
            win(
                "AppB",
                "Right",
                2,
                2,
                Pos {
                    x: 900,
                    y: 10,
                    width: 200,
                    height: 200,
                },
                0,
                false,
            ),
            // Overlapping across both, but larger overlap on right
            win(
                "AppC",
                "Overlap",
                3,
                3,
                Pos {
                    x: 700,
                    y: 10,
                    width: 300,
                    height: 300,
                },
                0,
                false,
            ),
        ]);
        let world = World::spawn(mock.clone() as Arc<dyn WinOps>, cfg_fast());
        let _ = wait_snapshot_until(&world, 200, |s| s.len() == 3).await;
        let mut snap = world.snapshot().await;
        snap.sort_by_key(|w| w.id);
        assert_eq!(snap[0].display_id, Some(1));
        assert_eq!(snap[1].display_id, Some(2));
        assert_eq!(snap[2].display_id, Some(2));
    });
}

#[test]
fn debounce_updates_within_window() {
    run_world_test(Some(FAST_COALESCE_MS), async move {
        let mock = Arc::new(MockWinOps::new());
        world_test::set_accessibility_ok(false);
        world_test::set_screen_recording_ok(false);
        world_test::set_ax_bridge_enabled(false);
        mock.set_windows(vec![win(
            "AppA",
            "T0",
            1,
            1,
            Pos {
                x: 0,
                y: 0,
                width: 100,
                height: 100,
            },
            0,
            true,
        )]);
        let world = World::spawn(
            mock.clone() as Arc<dyn WinOps>,
            WorldCfg {
                poll_ms_min: 10,
                poll_ms_max: 10,
                include_offscreen: false,
                ax_watch_frontmost: false,
                events_buffer: 64,
            },
        );
        let mut rx = world.subscribe();
        tokio::time::sleep(Duration::from_millis(60)).await; // drain Added and exceed debounce window
        drain_events(&mut rx);

        // First change -> expect snapshot to update
        mock.set_windows(vec![win(
            "AppA",
            "T1",
            1,
            1,
            Pos {
                x: 0,
                y: 0,
                width: 100,
                height: 100,
            },
            0,
            true,
        )]);
        assert!(
            wait_snapshot_until(&world, 220, |s| s
                .iter()
                .any(|w| w.id == 1 && w.title == "T1"))
            .await,
            "expected snapshot to update after first change"
        );

        // Second change within debounce window -> should be coalesced (no extra Updated)
        mock.set_windows(vec![win(
            "AppA",
            "T2",
            1,
            1,
            Pos {
                x: 0,
                y: 0,
                width: 100,
                height: 100,
            },
            0,
            true,
        )]);
        // Second change stays within the debounce window; snapshot should still update
        assert!(
            wait_snapshot_until(&world, 160, |s| s
                .iter()
                .any(|w| w.id == 1 && w.title == "T2"))
            .await,
            "expected snapshot to update after second change"
        );

        // After 60ms, another change should emit again
        tokio::time::sleep(Duration::from_millis(60)).await;
        mock.set_windows(vec![win(
            "AppA",
            "T3",
            1,
            1,
            Pos {
                x: 0,
                y: 0,
                width: 100,
                height: 100,
            },
            0,
            true,
        )]);
        // Third change after debounce window; snapshot must reflect T3
        assert!(
            wait_snapshot_until(&world, 220, |s| s
                .iter()
                .any(|w| w.id == 1 && w.title == "T3"))
            .await,
            "expected snapshot to update after third change"
        );
    });
}

#[test]
fn debounce_event_coalescing_for_repetitive_changes() {
    run_world_test(Some(FAST_COALESCE_MS), async move {
        let mock = Arc::new(MockWinOps::new());
        world_test::set_accessibility_ok(false);
        world_test::set_screen_recording_ok(false);
        world_test::set_ax_bridge_enabled(false);
        // Start with a single focused window
        mock.set_windows(vec![win(
            "AppA",
            "T0",
            1,
            1,
            Pos {
                x: 0,
                y: 0,
                width: 100,
                height: 100,
            },
            0,
            true,
        )]);
        let world = World::spawn(
            mock.clone() as Arc<dyn WinOps>,
            WorldCfg {
                poll_ms_min: 10,
                poll_ms_max: 10,
                include_offscreen: false,
                ax_watch_frontmost: false,
                events_buffer: 64,
            },
        );

        // Subscribe and drain startup events (Added, FocusChanged, etc.)
        let mut rx = world.subscribe();
        tokio::time::sleep(Duration::from_millis(60)).await;
        drain_events(&mut rx);

        // Burst of rapid updates still inside the debounce window:
        //  - title change
        //  - move
        //  - resize
        mock.set_windows(vec![win(
            "AppA",
            "T1",
            1,
            1,
            Pos {
                x: 0,
                y: 0,
                width: 100,
                height: 100,
            },
            0,
            true,
        )]);
        world.hint_refresh();
        assert!(
            wait_debounce_pending(&world, 1, FAST_COALESCE_MS * 2).await,
            "expected debounce queue to register first change"
        );
        mock.set_windows(vec![win(
            "AppA",
            "T1",
            1,
            1,
            Pos {
                x: 10,
                y: 5,
                width: 100,
                height: 100,
            },
            0,
            true,
        )]); // move
        world.hint_refresh();
        assert!(
            wait_debounce_pending(&world, 1, FAST_COALESCE_MS * 2).await,
            "debounce queue should continue tracking the burst"
        );
        mock.set_windows(vec![win(
            "AppA",
            "T2",
            1,
            1,
            Pos {
                x: 10,
                y: 5,
                width: 120,
                height: 110,
            },
            0,
            true,
        )]); // resize + title
        world.hint_refresh();
        assert!(
            wait_debounce_pending(&world, 1, FAST_COALESCE_MS * 2).await,
            "debounce queue should still be pending after final burst change"
        );

        // Expect exactly one coalesced Updated event for the burst.
        let key = WindowKey { pid: 1, id: 1 };
        let updated = recv_event_until(
            &mut rx,
            FAST_COALESCE_MS * 4,
            |ev| matches!(ev, WorldEvent::Updated(k, _) if *k == key),
        )
        .await;
        assert!(updated.is_some(), "expected one coalesced Updated event");

        assert!(
            wait_debounce_pending(&world, 0, FAST_COALESCE_MS * 4).await,
            "debounce queue should drain after emitting the coalesced event"
        );
        assert!(
            recv_event_until(&mut rx, FAST_COALESCE_MS * 2, |ev| {
                matches!(ev, WorldEvent::Updated(k, _) if *k == key)
            })
            .await
            .is_none(),
            "no additional Updated events expected once quiet"
        );

        // Snapshot should reflect the latest state from the burst (T2 and resized size)
        let snap = world.snapshot().await;
        let w = snap.iter().find(|w| w.id == 1).unwrap();
        assert_eq!(w.title, "T2");
        assert_eq!(w.pos.unwrap().width, 120);
        assert_eq!(w.pos.unwrap().height, 110);

        // After the debounce window, another change should emit a NEW Updated event.
        tokio::time::sleep(Duration::from_millis(FAST_COALESCE_MS + 10)).await;
        assert!(
            wait_debounce_pending(&world, 0, FAST_COALESCE_MS * 2).await,
            "debounce queue should be empty after quiet period"
        );
        mock.set_windows(vec![win(
            "AppA",
            "T3",
            1,
            1,
            Pos {
                x: 20,
                y: 10,
                width: 130,
                height: 120,
            },
            0,
            true,
        )]);
        world.hint_refresh();

        let next = recv_event_until(
            &mut rx,
            FAST_COALESCE_MS * 4,
            |ev| matches!(ev, WorldEvent::Updated(k, _) if *k == key),
        )
        .await;
        assert!(
            next.is_some(),
            "expected Updated event after debounce window"
        );
        assert!(
            wait_debounce_pending(&world, 0, FAST_COALESCE_MS * 4).await,
            "debounce queue should drain after trailing update"
        );
        assert!(
            recv_event_until(&mut rx, FAST_COALESCE_MS * 2, |ev| {
                matches!(ev, WorldEvent::Updated(k, _) if *k == key)
            })
            .await
            .is_none(),
            "no further Updated events expected"
        );

        // Final snapshot reflects T3 and latest geometry
        let final_snap = world.snapshot().await;
        let w2 = final_snap.iter().find(|w| w.id == 1).unwrap();
        assert_eq!(w2.title, "T3");
        assert_eq!(w2.pos.unwrap().x, 20);
        assert_eq!(w2.pos.unwrap().y, 10);
        assert_eq!(w2.pos.unwrap().width, 130);
        assert_eq!(w2.pos.unwrap().height, 120);
    });
}

#[test]
fn coalesced_trailing_update_after_quiet_period() {
    run_world_test(Some(FAST_COALESCE_MS), async move {
        let mock = Arc::new(MockWinOps::new());
        world_test::set_accessibility_ok(false);
        world_test::set_screen_recording_ok(false);
        world_test::set_ax_bridge_enabled(false);
        mock.set_windows(vec![win(
            "AppA",
            "T0",
            42,
            420,
            Pos {
                x: 0,
                y: 0,
                width: 100,
                height: 100,
            },
            0,
            true,
        )]);
        let world = World::spawn(
            mock.clone() as Arc<dyn WinOps>,
            WorldCfg {
                poll_ms_min: 10,
                poll_ms_max: 10,
                include_offscreen: false,
                ax_watch_frontmost: false,
                events_buffer: 64,
            },
        );
        let mut rx = world.subscribe();
        // Drain startup events (Added/FocusChanged)
        tokio::time::sleep(Duration::from_millis(25)).await;
        drain_events(&mut rx);

        // Rapid changes within debounce window: should yield a single Updated after quiet
        mock.set_windows(vec![win(
            "AppA",
            "T1",
            42,
            420,
            Pos {
                x: 0,
                y: 0,
                width: 100,
                height: 100,
            },
            0,
            true,
        )]);
        world.hint_refresh();
        assert!(
            wait_debounce_pending(&world, 1, FAST_COALESCE_MS * 2).await,
            "expected first change to enqueue debounce entry"
        );
        mock.set_windows(vec![win(
            "AppA",
            "T2",
            42,
            420,
            Pos {
                x: 5,
                y: 5,
                width: 110,
                height: 110,
            },
            0,
            true,
        )]);
        world.hint_refresh();
        assert!(
            wait_debounce_pending(&world, 1, FAST_COALESCE_MS * 2).await,
            "debounce entry should remain pending during burst"
        );
        mock.set_windows(vec![win(
            "AppA",
            "T3",
            42,
            420,
            Pos {
                x: 5,
                y: 5,
                width: 120,
                height: 110,
            },
            0,
            true,
        )]);
        world.hint_refresh();
        assert!(
            wait_debounce_pending(&world, 1, FAST_COALESCE_MS * 2).await,
            "pending debounce entry should stay coalesced"
        );

        // Expect exactly one Updated event emitted after the quiet period
        let key = WindowKey { pid: 42, id: 420 };
        let only = recv_event_until(
            &mut rx,
            FAST_COALESCE_MS * 4,
            |ev| matches!(ev, WorldEvent::Updated(k, _) if *k == key),
        )
        .await;
        assert!(only.is_some(), "expected trailing coalesced Updated");
        assert!(
            wait_debounce_pending(&world, 0, FAST_COALESCE_MS * 4).await,
            "debounce queue should drain after trailing event"
        );
        assert!(
            recv_event_until(&mut rx, FAST_COALESCE_MS * 2, |ev| {
                matches!(ev, WorldEvent::Updated(k, _) if *k == key)
            })
            .await
            .is_none(),
            "no further Updated events expected after quiet period"
        );
    });
}

#[test]
fn startup_focus_event_and_context_and_snapshot() {
    run_world_test(Some(FAST_COALESCE_MS), async move {
        // Single focused window present at startup
        let mock = Arc::new(MockWinOps::new());
        mock.set_windows(vec![win(
            "AppX",
            "TitleX",
            300,
            30,
            Pos {
                x: 0,
                y: 0,
                width: 200,
                height: 150,
            },
            0,
            true,
        )]);

        let world = World::spawn(mock.clone() as Arc<dyn WinOps>, cfg_fast());

        // Snapshot should be available shortly after spawn (best effort ~poll_ms_min)
        assert!(
            wait_snapshot_until(&world, 200, |s| s.iter().any(|w| w.id == 30)).await,
            "expected startup snapshot to include initial window"
        );

        // Focused context should match the focused window
        let ctx = world.focused_context().await;
        assert_eq!(ctx, Some(("AppX".to_string(), "TitleX".to_string(), 300)));

        // Subscribe with snapshot returns a consistent snapshot + focused key
        let (_rx2, snap, f2) = world.subscribe_with_snapshot().await;
        assert_eq!(f2, Some(WindowKey { pid: 300, id: 30 }));
        assert!(snap.iter().any(|w| w.id == 30 && w.focused));
    });
}
