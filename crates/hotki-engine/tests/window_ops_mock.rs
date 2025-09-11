use std::sync::Arc;

use hotki_engine::Engine;
use hotki_engine::MockHotkeyApi;
use hotki_protocol::MsgToUI;
use hotki_world::World;
use mac_winops::ops::MockWinOps;
use tokio::sync::mpsc;

// Using mac_winops::ops::MockWinOps provided under the `test-utils` feature.

fn ensure_no_os_interaction() {}

#[tokio::test(flavor = "current_thread")]
async fn engine_uses_window_ops_for_focus() {
    ensure_no_os_interaction();
    let (tx, mut _rx): (
        mpsc::UnboundedSender<MsgToUI>,
        mpsc::UnboundedReceiver<MsgToUI>,
    ) = mpsc::unbounded_channel();
    let mock = Arc::new(MockWinOps::new());
    let api = Arc::new(MockHotkeyApi::new());
    let world = World::spawn(mock.clone(), hotki_world::WorldCfg::default());
    let engine = Engine::new_with_api_and_ops(api, tx, mock.clone(), false, world);

    // Simple config: single binding that triggers focus(left)
    let keys = keymode::Keys::from_ron("[(\"a\", \"focus left\", focus(left))]").unwrap();
    let cfg = config::Config::from_parts(keys, config::Style::default());
    let mut engine = engine;
    engine.set_config(cfg).await.unwrap();
    engine
        .on_focus_snapshot(mac_winops::focus::FocusSnapshot {
            app: "X".into(),
            title: "T".into(),
            pid: 42,
        })
        .await
        .unwrap();

    // Dispatch the bound key
    let id = engine
        .resolve_id_for_ident("a")
        .await
        .expect("registered id for a");
    engine
        .dispatch(id, mac_hotkey::EventKind::KeyDown, false)
        .await;

    assert!(mock.calls_contains("focus_dir"));
}

#[tokio::test(flavor = "current_thread")]
async fn engine_hide_uses_winops() {
    ensure_no_os_interaction();
    let (tx, _rx) = mpsc::unbounded_channel();
    let mock = Arc::new(MockWinOps::new());
    let api = Arc::new(MockHotkeyApi::new());
    let world = World::spawn(mock.clone(), hotki_world::WorldCfg::default());
    let engine = Engine::new_with_api_and_ops(api, tx, mock.clone(), false, world);
    let keys = keymode::Keys::from_ron("[(\"a\", \"hide\", hide(on))]").unwrap();
    let cfg = config::Config::from_parts(keys, config::Style::default());
    let mut engine = engine;
    engine.set_config(cfg).await.unwrap();
    engine
        .on_focus_snapshot(mac_winops::focus::FocusSnapshot {
            app: "X".into(),
            title: "T".into(),
            pid: 77,
        })
        .await
        .unwrap();
    let id = engine.resolve_id_for_ident("a").await.unwrap();
    engine
        .dispatch(id, mac_hotkey::EventKind::KeyDown, false)
        .await;
    assert!(mock.calls_contains("hide"));
}

#[tokio::test(flavor = "current_thread")]
async fn engine_fullscreen_routes_native_and_nonnative() {
    ensure_no_os_interaction();
    let (tx, _rx) = mpsc::unbounded_channel();
    let mock = Arc::new(MockWinOps::new());
    let api = Arc::new(MockHotkeyApi::new());
    let world = World::spawn(mock.clone(), hotki_world::WorldCfg::default());
    let engine = Engine::new_with_api_and_ops(api, tx, mock.clone(), false, world);
    let keys = keymode::Keys::from_ron(
        "[(\"n\", \"fs native\", fullscreen(on, native)), (\"f\", \"fs nonnative\", fullscreen(on, nonnative))]",
    )
    .unwrap();
    let cfg = config::Config::from_parts(keys, config::Style::default());
    let mut engine = engine;
    engine.set_config(cfg).await.unwrap();
    engine
        .on_focus_snapshot(mac_winops::focus::FocusSnapshot {
            app: "X".into(),
            title: "T".into(),
            pid: 123,
        })
        .await
        .unwrap();
    let id_n = engine.resolve_id_for_ident("n").await.unwrap();
    engine
        .dispatch(id_n, mac_hotkey::EventKind::KeyDown, false)
        .await;
    let id_f = engine.resolve_id_for_ident("f").await.unwrap();
    engine
        .dispatch(id_f, mac_hotkey::EventKind::KeyDown, false)
        .await;
    assert!(mock.calls_contains("fullscreen_native"));
    assert!(mock.calls_contains("fullscreen_nonnative"));
}

#[tokio::test(flavor = "current_thread")]
async fn engine_raise_activates_on_match() {
    ensure_no_os_interaction();
    let (tx, _rx) = mpsc::unbounded_channel();
    let mock = Arc::new(MockWinOps::new());
    // Provide a matching window so the immediate path (no debounce) is taken
    mock.set_windows(vec![mac_winops::WindowInfo {
        id: 3,
        pid: 888,
        app: "Zed".into(),
        title: "Downloads".into(),
        pos: None,
        space: None,
        layer: 0,
        focused: false,
    }]);
    let api = Arc::new(MockHotkeyApi::new());
    let world = World::spawn(mock.clone(), hotki_world::WorldCfg::default());
    let engine = Engine::new_with_api_and_ops(api, tx, mock.clone(), false, world);
    let keys = keymode::Keys::from_ron(
        "[(\"a\", \"raise\", raise(app: \"^Zed$\", title: \"Downloads\"))]",
    )
    .unwrap();
    let cfg = config::Config::from_parts(keys, config::Style::default());
    let mut engine = engine;
    engine.set_config(cfg).await.unwrap();
    engine
        .on_focus_snapshot(mac_winops::focus::FocusSnapshot {
            app: "Foo".into(),
            title: "Bar".into(),
            pid: 1,
        })
        .await
        .unwrap();
    let id = engine.resolve_id_for_ident("a").await.unwrap();
    engine
        .dispatch(id, mac_hotkey::EventKind::KeyDown, false)
        .await;
    assert!(mock.calls_contains("activate_pid"));
}

#[tokio::test(flavor = "current_thread")]
async fn engine_place_prefers_last_raise_pid_then_clears() {
    ensure_no_os_interaction();
    let (tx, _rx) = mpsc::unbounded_channel();
    let mock = Arc::new(MockWinOps::new());
    // Frontmost is A
    let frontmost = mac_winops::WindowInfo {
        id: 1,
        pid: 100,
        app: "A".into(),
        title: "front".into(),
        pos: None,
        space: None,
        layer: 0,
        focused: true,
    };
    mock.set_frontmost(Some(frontmost.clone()));
    // Also have B for raise
    mock.set_windows(vec![
        frontmost.clone(),
        mac_winops::WindowInfo {
            id: 2,
            pid: 200,
            app: "B".into(),
            title: "raise-me".into(),
            pos: None,
            space: None,
            layer: 0,
            focused: false,
        },
    ]);
    let api = Arc::new(MockHotkeyApi::new());
    let world = World::spawn(mock.clone(), hotki_world::WorldCfg::default());
    let engine = Engine::new_with_api_and_ops(api, tx, mock.clone(), false, world);
    let keys = keymode::Keys::from_ron(
        "[(\"r\", \"raise\", raise(title: \"raise-me\")), (\"p\", \"place\", place(grid(2,2), at(0,0)))]",
    )
    .unwrap();
    let cfg = config::Config::from_parts(keys, config::Style::default());
    let mut engine = engine;
    engine.set_config(cfg).await.unwrap();
    engine
        .on_focus_snapshot(mac_winops::focus::FocusSnapshot {
            app: "A".into(),
            title: "front".into(),
            pid: 100,
        })
        .await
        .unwrap();
    // Raise to B
    let id_r = engine.resolve_id_for_ident("r").await.unwrap();
    engine
        .dispatch(id_r, mac_hotkey::EventKind::KeyDown, false)
        .await;
    // Place should prefer last raise pid (200)
    let id_p = engine.resolve_id_for_ident("p").await.unwrap();
    engine
        .dispatch(id_p, mac_hotkey::EventKind::KeyDown, false)
        .await;
    assert_eq!(mock.last_place_grid_pid(), Some(200));
    // Simulate focus moving to pid 200 in the world snapshot
    mock.set_windows(vec![
        mac_winops::WindowInfo {
            id: 1,
            pid: 100,
            app: "A".into(),
            title: "front".into(),
            pos: None,
            space: None,
            layer: 0,
            focused: false,
        },
        mac_winops::WindowInfo {
            id: 2,
            pid: 200,
            app: "B".into(),
            title: "raise-me".into(),
            pos: None,
            space: None,
            layer: 0,
            focused: true,
        },
    ]);
    engine.world_handle().hint_refresh();
    tokio::time::sleep(std::time::Duration::from_millis(60)).await;
    // Next place should use world-focused (cleared hint)
    engine
        .dispatch(id_p, mac_hotkey::EventKind::KeyDown, false)
        .await;
    assert_eq!(mock.last_place_grid_pid(), Some(200));
}

#[tokio::test(flavor = "current_thread")]
async fn engine_fullscreen_error_notifies() {
    ensure_no_os_interaction();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mock = Arc::new(MockWinOps::new());
    mock.set_fail_fullscreen_nonnative(true);
    let api = Arc::new(MockHotkeyApi::new());
    let world = World::spawn(mock.clone(), hotki_world::WorldCfg::default());
    let engine = Engine::new_with_api_and_ops(api, tx, mock.clone(), false, world);
    let keys = keymode::Keys::from_ron("[(\"f\", \"fs\", fullscreen(on, nonnative))]").unwrap();
    let cfg = config::Config::from_parts(keys, config::Style::default());
    let mut engine = engine;
    engine.set_config(cfg).await.unwrap();
    engine
        .on_focus_snapshot(mac_winops::focus::FocusSnapshot {
            app: "X".into(),
            title: "T".into(),
            pid: 11,
        })
        .await
        .unwrap();
    let id = engine.resolve_id_for_ident("f").await.unwrap();
    engine
        .dispatch(id, mac_hotkey::EventKind::KeyDown, false)
        .await;
    // expect an error Notify with title "Fullscreen"
    let mut saw = false;
    for _ in 0..10 {
        if let Ok(msg) = rx.try_recv() {
            if let hotki_protocol::MsgToUI::Notify { kind, title, .. } = msg
                && matches!(kind, hotki_protocol::NotifyKind::Error)
                && title == "Fullscreen"
            {
                saw = true;
                break;
            }
        } else {
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    }
    assert!(saw, "expected Fullscreen error notification");
}

#[tokio::test(flavor = "current_thread")]
async fn engine_hide_error_notifies() {
    ensure_no_os_interaction();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mock = Arc::new(MockWinOps::new());
    mock.set_fail_hide(true);
    let api = Arc::new(MockHotkeyApi::new());
    let world = World::spawn_noop();
    let engine = Engine::new_with_api_and_ops(api, tx, mock.clone(), false, world);
    let keys = keymode::Keys::from_ron("[(\"h\", \"hide\", hide(on))]").unwrap();
    let cfg = config::Config::from_parts(keys, config::Style::default());
    let mut engine = engine;
    engine.set_config(cfg).await.unwrap();
    engine
        .on_focus_snapshot(mac_winops::focus::FocusSnapshot {
            app: "X".into(),
            title: "T".into(),
            pid: 22,
        })
        .await
        .unwrap();
    let id = engine.resolve_id_for_ident("h").await.unwrap();
    engine
        .dispatch(id, mac_hotkey::EventKind::KeyDown, false)
        .await;
    let mut saw = false;
    for _ in 0..10 {
        if let Ok(msg) = rx.try_recv() {
            if let hotki_protocol::MsgToUI::Notify { kind, title, .. } = msg
                && matches!(kind, hotki_protocol::NotifyKind::Error)
                && title == "Hide"
            {
                saw = true;
                break;
            }
        } else {
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    }
    assert!(saw, "expected Hide error notification");
}

#[tokio::test(flavor = "current_thread")]
async fn engine_raise_invalid_regex_notifies() {
    ensure_no_os_interaction();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mock = Arc::new(MockWinOps::new());
    let api = Arc::new(MockHotkeyApi::new());
    let world = World::spawn_noop();
    let engine = Engine::new_with_api_and_ops(api, tx, mock.clone(), false, world);
    // invalid regex for app
    let keys = keymode::Keys::from_ron("[(\"r\", \"raise\", raise(app: \"(unclosed\"))]").unwrap();
    let cfg = config::Config::from_parts(keys, config::Style::default());
    let mut engine = engine;
    engine.set_config(cfg).await.unwrap();
    engine
        .on_focus_snapshot(mac_winops::focus::FocusSnapshot {
            app: "X".into(),
            title: "T".into(),
            pid: 33,
        })
        .await
        .unwrap();
    let id = engine.resolve_id_for_ident("r").await.unwrap();
    engine
        .dispatch(id, mac_hotkey::EventKind::KeyDown, false)
        .await;
    let mut saw = false;
    for _ in 0..10 {
        if let Ok(msg) = rx.try_recv() {
            if let hotki_protocol::MsgToUI::Notify { kind, title, .. } = msg
                && matches!(kind, hotki_protocol::NotifyKind::Error)
                && title == "Raise"
            {
                saw = true;
                break;
            }
        } else {
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    }
    assert!(saw, "expected Raise error notification for invalid regex");
}

#[tokio::test(flavor = "current_thread")]
async fn engine_focus_error_propagates_notification() {
    ensure_no_os_interaction();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mock = Arc::new(MockWinOps::new());
    mock.set_fail_focus_dir(true);
    let api = Arc::new(MockHotkeyApi::new());
    let world = World::spawn_noop();
    let engine = Engine::new_with_api_and_ops(api, tx, mock.clone(), false, world);
    let keys = keymode::Keys::from_ron("[(\"a\", \"focus left\", focus(left))]").unwrap();
    let cfg = config::Config::from_parts(keys, config::Style::default());
    let mut engine = engine;
    engine.set_config(cfg).await.unwrap();
    engine
        .on_focus_snapshot(mac_winops::focus::FocusSnapshot {
            app: "X".into(),
            title: "T".into(),
            pid: 123,
        })
        .await
        .unwrap();
    let id = engine.resolve_id_for_ident("a").await.unwrap();
    engine
        .dispatch(id, mac_hotkey::EventKind::KeyDown, false)
        .await;

    // Drain messages until we see an error notification about Focus
    let mut saw_error = false;
    for _ in 0..10 {
        if let Ok(msg) = rx.try_recv() {
            if let hotki_protocol::MsgToUI::Notify { kind, title, .. } = msg
                && matches!(kind, hotki_protocol::NotifyKind::Error)
                && title == "Focus"
            {
                saw_error = true;
                break;
            }
        } else {
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    }
    assert!(saw_error, "expected Focus error notification");
}

// Debounce-based raise behavior removed: world-only model does no engine-side retries.

#[tokio::test(flavor = "current_thread")]
async fn engine_place_move_uses_winops() {
    ensure_no_os_interaction();
    let (tx, _rx): (
        mpsc::UnboundedSender<MsgToUI>,
        mpsc::UnboundedReceiver<MsgToUI>,
    ) = mpsc::unbounded_channel();
    let mock = Arc::new(MockWinOps::new());
    // Ensure the world snapshot has a window for the current pid
    mock.set_windows(vec![mac_winops::WindowInfo {
        id: 7,
        pid: 99,
        app: "X".into(),
        title: "T".into(),
        pos: None,
        space: None,
        layer: 0,
        focused: true,
    }]);
    let mgr = Arc::new(mac_hotkey::Manager::new().expect("manager"));
    let engine = Engine::new_with_ops(mgr, tx, mock.clone());
    let keys =
        keymode::Keys::from_ron("[(\"a\", \"move left\", place_move(grid(2,2), left))]").unwrap();
    let cfg = config::Config::from_parts(keys, config::Style::default());
    let mut engine = engine;
    engine.set_config(cfg).await.unwrap();
    engine
        .on_focus_snapshot(mac_winops::focus::FocusSnapshot {
            app: "X".into(),
            title: "T".into(),
            pid: 99,
        })
        .await
        .unwrap();
    let id = engine
        .resolve_id_for_ident("a")
        .await
        .expect("registered id");
    engine
        .dispatch(id, mac_hotkey::EventKind::KeyDown, false)
        .await;
    assert!(mock.calls_contains("place_move"));
}
