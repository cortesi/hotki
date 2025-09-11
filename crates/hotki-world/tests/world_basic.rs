use std::sync::Arc;
use std::time::Duration;

use hotki_world::test_api as world_test;
use hotki_world::{WindowKey, World, WorldCfg, WorldEvent};
use mac_winops::ops::{MockWinOps, WinOps};
use mac_winops::{Pos, WindowId, WindowInfo};

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
    }
}

fn cfg_fast() -> WorldCfg {
    WorldCfg {
        poll_ms_min: 10,
        poll_ms_max: 30,
        include_offscreen: false,
        ax_watch_frontmost: false,
        events_buffer: 64,
    }
}

#[test]
fn startup_adds_and_z_order() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    rt.block_on(async move {
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

        // Wait for initial reconcile
        let mut tries = 0;
        loop {
            let snap = world.snapshot().await;
            if snap.len() == 2 {
                break;
            }
            tries += 1;
            if tries > 50 {
                panic!("timeout waiting for initial snapshot");
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
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
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    rt.block_on(async move {
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
        tokio::time::sleep(Duration::from_millis(60)).await;
        while let Ok(_e) = rx.try_recv() {}

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
        let mut focused_key: Option<WindowKey> = None;
        let start = tokio::time::Instant::now();
        while start.elapsed() < Duration::from_millis(500) {
            if let Ok(ev) = rx.try_recv()
                && let WorldEvent::FocusChanged(k) = ev
            {
                focused_key = k;
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(focused_key, Some(WindowKey { pid: 200, id: 2 }));

        // Snapshot should reflect focused flags
        let snap = world.snapshot().await;
        let a = snap.iter().find(|w| w.pid == 100).unwrap();
        let b = snap.iter().find(|w| w.pid == 200).unwrap();
        assert!(!a.focused);
        assert!(b.focused);
    });
}

#[test]
fn title_update_reflected_in_snapshot() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    rt.block_on(async move {
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
        // Wait for initial Added and debounce window to expire
        tokio::time::sleep(Duration::from_millis(80)).await;

        // Change title
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

        // Confirm snapshot reflects new title
        let mut ok = false;
        let deadline = tokio::time::Instant::now() + Duration::from_millis(800);
        while tokio::time::Instant::now() < deadline {
            let snap = world.snapshot().await;
            if let Some(w) = snap.iter().find(|w| w.id == 1)
                && w.title == "New"
            {
                ok = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(ok, "snapshot did not reflect title change");
    });
}

#[test]
fn ax_focus_and_title_precedence() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    rt.block_on(async move {
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
        // Wait for reconcile
        tokio::time::sleep(Duration::from_millis(60)).await;
        let snap = world.snapshot().await;
        let a1 = snap.iter().find(|w| w.id == 101).unwrap();
        let a2 = snap.iter().find(|w| w.id == 102).unwrap();
        assert!(!a1.focused);
        assert!(a2.focused);
        assert_eq!(a2.title, "AX-Title-2");

        world_test::clear();
    });
}

#[test]
fn display_mapping_selects_best_overlap() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    rt.block_on(async move {
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
        tokio::time::sleep(Duration::from_millis(60)).await;
        let mut snap = world.snapshot().await;
        snap.sort_by_key(|w| w.id);
        assert_eq!(snap[0].display_id, Some(1));
        assert_eq!(snap[1].display_id, Some(2));
        assert_eq!(snap[2].display_id, Some(2));

        world_test::clear();
    });
}

#[test]
fn debounce_updates_within_window() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    rt.block_on(async move {
        let mock = Arc::new(MockWinOps::new());
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
        tokio::time::sleep(Duration::from_millis(80)).await; // drain Added and exceed debounce window
        while let Ok(_e) = rx.try_recv() {}

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
        let mut ok = false;
        let deadline = tokio::time::Instant::now() + Duration::from_millis(800);
        while tokio::time::Instant::now() < deadline {
            let s = world.snapshot().await;
            if let Some(w) = s.iter().find(|w| w.id == 1)
                && w.title == "T1"
            {
                ok = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(ok, "expected snapshot to update after first change");

        // Second change within 50ms window -> should be coalesced (no extra Updated)
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
        // Second change within 50ms; snapshot should update too (debounce only affects events)
        let mut ok2 = false;
        let deadline = tokio::time::Instant::now() + Duration::from_millis(300);
        while tokio::time::Instant::now() < deadline {
            let s = world.snapshot().await;
            if let Some(w) = s.iter().find(|w| w.id == 1)
                && w.title == "T2"
            {
                ok2 = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(ok2, "expected snapshot to update after second change");

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
        let mut ok3 = false;
        let deadline = tokio::time::Instant::now() + Duration::from_millis(800);
        while tokio::time::Instant::now() < deadline {
            let s = world.snapshot().await;
            if let Some(w) = s.iter().find(|w| w.id == 1)
                && w.title == "T3"
            {
                ok3 = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(ok3, "expected snapshot to update after third change");
    });
}

#[test]
fn debounce_event_coalescing_for_repetitive_changes() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    rt.block_on(async move {
        let mock = Arc::new(MockWinOps::new());
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
        tokio::time::sleep(Duration::from_millis(80)).await;
        while let Ok(_e) = rx.try_recv() {}

        // Burst of rapid updates within debounce window (<50ms):
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
        tokio::time::sleep(Duration::from_millis(5)).await;
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
        tokio::time::sleep(Duration::from_millis(5)).await;
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

        // Trigger immediate reconcile to observe burst deterministically.
        world.hint_refresh();

        // Collect events for a little while; expect exactly ONE Updated in this burst.
        let start = tokio::time::Instant::now();
        let mut updated_count = 0u32;
        while start.elapsed() < Duration::from_millis(600) {
            if let Ok(ev) = rx.try_recv() {
                if let WorldEvent::Updated(k, _) = ev
                    && k == (WindowKey { pid: 1, id: 1 })
                {
                    updated_count += 1;
                }
            } else {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }
        assert_eq!(
            updated_count, 1,
            "debounce should coalesce rapid updates to a single event"
        );

        // Snapshot should reflect the latest state from the burst (T2 and resized size)
        let snap = world.snapshot().await;
        let w = snap.iter().find(|w| w.id == 1).unwrap();
        assert_eq!(w.title, "T2");
        assert_eq!(w.pos.unwrap().width, 120);
        assert_eq!(w.pos.unwrap().height, 110);

        // After debounce window (>50ms), another change should emit a NEW Updated event.
        tokio::time::sleep(Duration::from_millis(70)).await;
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

        // Expect one more Updated for the same window.
        let start2 = tokio::time::Instant::now();
        let mut added = 0u32;
        while start2.elapsed() < Duration::from_millis(600) {
            if let Ok(ev) = rx.try_recv() {
                if let WorldEvent::Updated(k, _) = ev
                    && k == (WindowKey { pid: 1, id: 1 })
                {
                    added += 1;
                    break;
                }
            } else {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }
        assert_eq!(
            added, 1,
            "expected one additional Updated after debounce window"
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
fn startup_focus_event_and_context_and_snapshot() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    rt.block_on(async move {
        // Single focused window present at startup
        let mock = Arc::new(MockWinOps::new());
        mock.set_windows(vec![win(
            "AppX",
            "TitleX",
            300,
            30,
            Pos { x: 0, y: 0, width: 200, height: 150 },
            0,
            true,
        )]);

        let world = World::spawn(mock.clone() as Arc<dyn WinOps>, cfg_fast());

        // Snapshot should be available shortly after spawn (best effort ~poll_ms_min)
        let deadline = tokio::time::Instant::now() + Duration::from_millis(800);
        let mut snap_ok = false;
        while tokio::time::Instant::now() < deadline {
            let s = world.snapshot().await;
            if s.iter().any(|w| w.id == 30) {
                snap_ok = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(snap_ok, "expected startup snapshot to include initial window");

        // Focused context should match the focused window
        let ctx = world.focused_context().await;
        assert_eq!(ctx, Some(("AppX".to_string(), "TitleX".to_string(), 300)));

        // Subscribe with snapshot returns a consistent snapshot + focused key
        let (_rx2, snap, f2) = world.subscribe_with_snapshot().await;
        assert_eq!(f2, Some(WindowKey { pid: 300, id: 30 }));
        assert!(snap.iter().any(|w| w.id == 30 && w.focused));
    });
}
