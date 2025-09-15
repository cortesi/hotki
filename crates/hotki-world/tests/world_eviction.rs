use std::{sync::Arc, time::Duration};

use hotki_world::{
    WindowKey, World, WorldCfg, WorldEvent,
    test_support::{drain_events, recv_event_until, wait_snapshot_until},
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
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    rt.block_on(async move {
        let mock = Arc::new(MockWinOps::new());
        mock.set_windows(vec![win("AppA", "A1", 100, 1)]);
        let world = World::spawn(mock.clone() as Arc<dyn WinOps>, cfg_fast());

        assert!(
            wait_snapshot_until(&world, 500, |s| s.iter().any(|w| w.pid == 100 && w.id == 1)).await,
            "window should be present initially"
        );

        // Remove from CG; first pass marks suspect, second confirms and removes
        mock.set_windows(vec![]);

        // Request fast reconcile; consider removal successful on event or when snapshot no longer contains it
        let mut rx = world.subscribe();
        drain_events(&mut rx);
        world.hint_refresh();
        let mut gone = false;
        let deadline2 = tokio::time::Instant::now() + Duration::from_millis(1200);
        while tokio::time::Instant::now() < deadline2 {
            if let Some(WorldEvent::Removed(k)) = recv_event_until(&mut rx, 150, |_| true).await
                && k == (WindowKey { pid: 100, id: 1 })
            {
                gone = true;
                break;
            }
            let snap_now = world.snapshot().await;
            if !snap_now.iter().any(|w| w.pid == 100 && w.id == 1) {
                gone = true;
                break;
            }
        }
        assert!(gone, "window should be evicted within two passes");
    });
}

#[test]
fn pid_reuse_no_false_positive() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    rt.block_on(async move {
        let mock = Arc::new(MockWinOps::new());
        // Start with pid=100, id=1
        mock.set_windows(vec![win("OldApp", "Old", 100, 1)]);
        let world = World::spawn(mock.clone() as Arc<dyn WinOps>, cfg_fast());
        let _ =
            wait_snapshot_until(&world, 600, |s| s.iter().any(|w| w.pid == 100 && w.id == 1)).await;

        // First pass: old window disappears, new pid reuses same CG id (1)
        mock.set_windows(vec![win("NewApp", "New", 101, 1)]);
        world.hint_refresh();
        tokio::time::sleep(Duration::from_millis(100)).await; // old becomes suspect

        // Second pass: confirm removal of old (100,1); new (101,1) stays
        let mut rx = world.subscribe();
        drain_events(&mut rx);
        world.hint_refresh();
        let mut removed_or_gone = false;
        let deadline = tokio::time::Instant::now() + Duration::from_millis(1500);
        while tokio::time::Instant::now() < deadline {
            if let Some(WorldEvent::Removed(k)) = recv_event_until(&mut rx, 150, |_| true).await
                && k == (WindowKey { pid: 100, id: 1 })
            {
                removed_or_gone = true;
                break;
            }
            let snap_now = world.snapshot().await;
            if !snap_now.iter().any(|w| w.pid == 100 && w.id == 1) {
                removed_or_gone = true;
                break;
            }
        }
        assert!(removed_or_gone, "expected old (pid=100,id=1) to be removed");

        let snap = world.snapshot().await;
        assert!(
            snap.iter().any(|w| w.pid == 101 && w.id == 1),
            "new window must remain"
        );
    });
}
