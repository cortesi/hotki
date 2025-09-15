use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use hotki_world::{World, WorldCfg, test_api as world_test, test_support::wait_snapshot_until};
use mac_winops::{
    AxEvent, AxEventKind, Pos, WindowHint, WindowId, WindowInfo,
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
    }
}

fn cfg_slow_min() -> WorldCfg {
    WorldCfg {
        // Make the normal poll relatively slow so the HintRefresh effect is observable
        poll_ms_min: 500,
        poll_ms_max: 1000,
        include_offscreen: false,
        ax_watch_frontmost: false,
        events_buffer: 64,
    }
}

#[test]
fn ax_event_created_triggers_fast_refresh() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    rt.block_on(async move {
        let mock = Arc::new(MockWinOps::new());
        mock.set_windows(vec![win(
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
        )]);
        let world = World::spawn(mock.clone() as Arc<dyn WinOps>, cfg_slow_min());

        // Wait for initial reconcile (1 window)
        assert!(wait_snapshot_until(&world, 500, |s| s.len() == 1).await);

        // Change underlying windows to add one more; without a hint this would
        // only be observed after ~poll_ms_min (500 ms).
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
                    x: 20,
                    y: 20,
                    width: 80,
                    height: 80,
                },
                0,
                false,
            ),
        ]);

        // Send a synthetic AXWindowCreated event through the bridge sender.
        let tx = world_test::ax_hint_bridge_sender()
            .expect("ax hint bridge sender should be initialized by World::spawn");
        let _ = tx.send(AxEvent {
            pid: 200,
            kind: AxEventKind::Created,
            hint: WindowHint::default(),
        });

        // Expect the world to observe the new window well before poll_ms_min.
        let t0 = Instant::now();
        let timeout = Duration::from_millis(200); // generous bound < 500ms
        assert!(
            wait_snapshot_until(&world, timeout.as_millis() as u64, |s| s.len() == 2).await,
            "world did not refresh promptly after AX event (elapsed = {:?})",
            t0.elapsed()
        );
    });
}
