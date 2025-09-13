use std::{sync::Arc, time::Duration};

use hotki_world::{WindowKey, World, WorldCfg, WorldEvent};
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
        pos: Some(Pos { x: 0, y: 0, width: 100, height: 100 }),
        space: Some(1),
        layer: 0,
        focused: true,
    }
}

fn cfg_fast() -> WorldCfg {
    // Use long polling so only hint_refresh drives reconcile in tests below
    WorldCfg { poll_ms_min: 1000, poll_ms_max: 1000, include_offscreen: false, ax_watch_frontmost: false, events_buffer: 64 }
}

#[test]
fn evicts_after_two_passes_when_missing() {
    let rt = tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap();
    rt.block_on(async move {
        let mock = Arc::new(MockWinOps::new());
        mock.set_windows(vec![win("AppA", "A1", 100, 1)]);
        let world = World::spawn(mock.clone() as Arc<dyn WinOps>, cfg_fast());

        // Wait for initial presence
        let mut present = false;
        let deadline = tokio::time::Instant::now() + Duration::from_millis(500);
        while tokio::time::Instant::now() < deadline {
            if world.snapshot().await.iter().any(|w| w.pid == 100 && w.id == 1) {
                present = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(present, "window should be present initially");

        // Remove from CG; first pass marks suspect, second confirms and removes
        mock.set_windows(vec![]);

        // Request fast reconcile; removal must occur within two cycles (and may
        // occur sooner depending on scheduler timing).
        let mut rx = world.subscribe();
        while let Ok(_e) = rx.try_recv() {}
        world.hint_refresh();
        let mut removed_seen = false;
        let deadline2 = tokio::time::Instant::now() + Duration::from_millis(600);
        while tokio::time::Instant::now() < deadline2 {
            if let Ok(ev) = rx.try_recv() {
                if let WorldEvent::Removed(k) = ev
                    && k == (WindowKey { pid: 100, id: 1 }) { removed_seen = true; break; }
            } else {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }
        assert!(removed_seen, "expected Removed event for missing window");
        assert!(
            !world.snapshot().await.iter().any(|w| w.pid == 100 && w.id == 1),
            "window should be evicted after confirmation pass"
        );
    });
}

#[test]
fn pid_reuse_no_false_positive() {
    let rt = tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap();
    rt.block_on(async move {
        let mock = Arc::new(MockWinOps::new());
        // Start with pid=100, id=1
        mock.set_windows(vec![win("OldApp", "Old", 100, 1)]);
        let world = World::spawn(mock.clone() as Arc<dyn WinOps>, cfg_fast());
        tokio::time::sleep(Duration::from_millis(60)).await;

        // First pass: old window disappears, new pid reuses same CG id (1)
        mock.set_windows(vec![win("NewApp", "New", 101, 1)]);
        world.hint_refresh();
        tokio::time::sleep(Duration::from_millis(60)).await; // old becomes suspect

        // Second pass: confirm removal of old (100,1); new (101,1) stays
        let mut rx = world.subscribe();
        while let Ok(_e) = rx.try_recv() {}
        world.hint_refresh();
        let mut removed_old = false;
        let deadline = tokio::time::Instant::now() + Duration::from_millis(600);
        while tokio::time::Instant::now() < deadline {
            if let Ok(ev) = rx.try_recv() {
                if let WorldEvent::Removed(k) = ev && k == (WindowKey { pid: 100, id: 1 }) { removed_old = true; break; }
            } else {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }
        assert!(removed_old, "expected old (pid=100,id=1) to be removed");

        let snap = world.snapshot().await;
        assert!(snap.iter().any(|w| w.pid == 101 && w.id == 1), "new window must remain");
    });
}
