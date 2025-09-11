use std::sync::Arc;
use std::time::Duration;

use hotki_world::{World, WorldCfg, WorldEvent, WindowKey};
use mac_winops::ops::{MockWinOps, WinOps};
use mac_winops::{Pos, WindowId, WindowInfo};

fn win(app: &str, title: &str, pid: i32, id: WindowId, x: i32, y: i32, w: i32, h: i32, layer: i32, focused: bool) -> WindowInfo {
    WindowInfo {
        app: app.into(),
        title: title.into(),
        pid,
        id,
        pos: Some(Pos { x, y, width: w, height: h }),
        space: Some(1),
        layer,
        focused,
    }
}

fn cfg_fast() -> WorldCfg {
    WorldCfg { poll_ms_min: 10, poll_ms_max: 30, include_offscreen: false, ax_watch_frontmost: false }
}

#[test]
fn startup_adds_and_z_order() {
    let rt = tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap();
    rt.block_on(async move {
        let mock = Arc::new(MockWinOps::new());
        mock.set_windows(vec![
            win("AppA", "A1", 100, 1, 0, 0, 100, 100, 0, true),  // frontmost
            win("AppB", "B1", 200, 2, 0, 0, 100, 100, 0, false),
        ]);
        let world = World::spawn(mock.clone() as Arc<dyn WinOps>, cfg_fast());

        // Wait for initial reconcile
        let mut tries = 0;
        loop {
            let snap = world.snapshot().await;
            if snap.len() == 2 { break; }
            tries += 1;
            if tries > 50 { panic!("timeout waiting for initial snapshot"); }
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
    let rt = tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap();
    rt.block_on(async move {
        let mock = Arc::new(MockWinOps::new());
        mock.set_windows(vec![
            win("AppA", "A1", 100, 1, 0, 0, 100, 100, 0, true),
            win("AppB", "B1", 200, 2, 0, 0, 100, 100, 0, false),
        ]);
        let world = World::spawn(mock.clone() as Arc<dyn WinOps>, cfg_fast());
        let mut rx = world.subscribe();

        // Wait for initial reconcile and drain any startup events
        tokio::time::sleep(Duration::from_millis(60)).await;
        while let Ok(_e) = rx.try_recv() {}

        // Flip focus to AppB
        mock.set_windows(vec![
            win("AppA", "A1", 100, 1, 0, 0, 100, 100, 0, false),
            win("AppB", "B1", 200, 2, 0, 0, 100, 100, 0, true),
        ]);

        // Observe FocusChanged event to AppB
        let mut focused_key: Option<WindowKey> = None;
        let start = tokio::time::Instant::now();
        while start.elapsed() < Duration::from_millis(500) {
            if let Ok(ev) = rx.try_recv() {
                if let WorldEvent::FocusChanged(k) = ev { focused_key = k; break; }
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
fn title_update_emits_updated() {
    let rt = tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap();
    rt.block_on(async move {
        let mock = Arc::new(MockWinOps::new());
        mock.set_windows(vec![ win("AppA", "Old", 100, 1, 0, 0, 100, 100, 0, true) ]);
        let world = World::spawn(mock.clone() as Arc<dyn WinOps>, cfg_fast());
        let mut rx = world.subscribe();
        // Wait for initial Added
        tokio::time::sleep(Duration::from_millis(30)).await;

        // Change title
        mock.set_windows(vec![ win("AppA", "New", 100, 1, 0, 0, 100, 100, 0, true) ]);

        // Expect an Updated event and snapshot to contain new title
        let mut saw_update = false;
        let start = tokio::time::Instant::now();
        while start.elapsed() < Duration::from_millis(500) {
            if let Ok(ev) = rx.try_recv() {
                if let WorldEvent::Updated(k, _d) = ev {
                    if k == (WindowKey { pid: 100, id: 1 }) { saw_update = true; break; }
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(saw_update, "did not see Updated for title change");
        let snap = world.snapshot().await;
        assert_eq!(snap[0].title, "New");
    });
}
