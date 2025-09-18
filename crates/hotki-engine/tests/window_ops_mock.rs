use std::sync::Arc;

use hotki_engine::{Engine, Error, MockHotkeyApi, test_support as testutil};
use hotki_protocol::MsgToUI;
use hotki_world::{World, WorldWindowId};
use mac_winops::ops::MockWinOps;
use testutil::{fast_world_cfg, recv_error_with_title, wait_snapshot_until};
use tokio::sync::mpsc;

// Using mac_winops::ops::MockWinOps provided under the `test-utils` feature.

fn ensure_no_os_interaction() {}

async fn set_world_focus(engine: &Engine, mock: &MockWinOps, app: &str, title: &str, pid: i32) {
    mock.set_windows(vec![mac_winops::WindowInfo {
        id: 1,
        pid,
        app: app.into(),
        title: title.into(),
        pos: None,
        space: None,
        layer: 0,
        focused: true,
        is_on_screen: true,
        on_active_space: true,
    }]);
    let world = engine.world();
    world.hint_refresh();
    let _ = wait_snapshot_until(world.as_ref(), 50, |snap| {
        snap.iter().any(|w| w.pid == pid && w.focused)
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn engine_uses_window_ops_for_focus() {
    ensure_no_os_interaction();
    let (tx, mut _rx): (mpsc::Sender<MsgToUI>, mpsc::Receiver<MsgToUI>) = mpsc::channel(32);
    let mock = Arc::new(MockWinOps::new());
    let api = Arc::new(MockHotkeyApi::new());
    let world = World::spawn_view(mock.clone(), fast_world_cfg());
    let engine = Engine::new_with_api_and_ops(api, tx, mock.clone(), false, world);

    // Simple config: single binding that triggers focus(left)
    let keys = keymode::Keys::from_ron("[(\"a\", \"focus left\", focus(left))]").unwrap();
    let cfg = config::Config::from_parts(keys, config::Style::default());
    let mut engine = engine;
    engine.set_config(cfg).await.unwrap();
    set_world_focus(&engine, &mock, "X", "T", 42).await;

    // Dispatch the bound key
    let id = engine
        .resolve_id_for_ident("a")
        .await
        .expect("registered id for a");
    engine
        .dispatch(id, mac_hotkey::EventKind::KeyDown, false)
        .await
        .expect("dispatch focus");

    assert!(mock.calls_contains("focus_dir"));
}

#[tokio::test(flavor = "current_thread")]
async fn engine_hide_uses_winops() {
    ensure_no_os_interaction();
    let (tx, _rx) = mpsc::channel(16);
    let mock = Arc::new(MockWinOps::new());
    let api = Arc::new(MockHotkeyApi::new());
    let world = World::spawn_view(mock.clone(), fast_world_cfg());
    let engine = Engine::new_with_api_and_ops(api, tx, mock.clone(), false, world);
    let keys = keymode::Keys::from_ron("[(\"a\", \"hide\", hide(on))]").unwrap();
    let cfg = config::Config::from_parts(keys, config::Style::default());
    let mut engine = engine;
    engine.set_config(cfg).await.unwrap();
    set_world_focus(&engine, &mock, "X", "T", 77).await;
    let id = engine.resolve_id_for_ident("a").await.unwrap();
    engine
        .dispatch(id, mac_hotkey::EventKind::KeyDown, false)
        .await
        .expect("dispatch hide");
    assert!(mock.calls_contains("hide"));
}

#[tokio::test(flavor = "current_thread")]
async fn engine_fullscreen_routes_native_and_nonnative() {
    ensure_no_os_interaction();
    let (tx, _rx) = mpsc::channel(16);
    let mock = Arc::new(MockWinOps::new());
    let api = Arc::new(MockHotkeyApi::new());
    let world = World::spawn_view(mock.clone(), fast_world_cfg());
    let engine = Engine::new_with_api_and_ops(api, tx, mock.clone(), false, world);
    let keys = keymode::Keys::from_ron(
        "[(\"n\", \"fs native\", fullscreen(on, native)), (\"f\", \"fs nonnative\", fullscreen(on, nonnative))]",
    )
    .unwrap();
    let cfg = config::Config::from_parts(keys, config::Style::default());
    let mut engine = engine;
    engine.set_config(cfg).await.unwrap();
    set_world_focus(&engine, &mock, "X", "T", 123).await;
    let id_n = engine.resolve_id_for_ident("n").await.unwrap();
    engine
        .dispatch(id_n, mac_hotkey::EventKind::KeyDown, false)
        .await
        .expect("dispatch fullscreen native");
    let id_f = engine.resolve_id_for_ident("f").await.unwrap();
    engine
        .dispatch(id_f, mac_hotkey::EventKind::KeyDown, false)
        .await
        .expect("dispatch fullscreen nonnative");
    assert!(mock.calls_contains("fullscreen_native"));
    assert!(mock.calls_contains("fullscreen_nonnative"));
}

#[tokio::test(flavor = "current_thread")]
async fn engine_raise_activates_on_match() {
    ensure_no_os_interaction();
    let (tx, _rx) = mpsc::channel(16);
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
        is_on_screen: true,
        on_active_space: true,
    }]);
    let api = Arc::new(MockHotkeyApi::new());
    let world = World::spawn_view(mock.clone(), hotki_world::WorldCfg::default());
    let engine = Engine::new_with_api_and_ops(api, tx, mock.clone(), false, world);
    let keys = keymode::Keys::from_ron(
        "[(\"a\", \"raise\", raise(app: \"^Zed$\", title: \"Downloads\"))]",
    )
    .unwrap();
    let cfg = config::Config::from_parts(keys, config::Style::default());
    let mut engine = engine;
    engine.set_config(cfg).await.unwrap();
    // Seed world with current focus and a matching target window
    mock.set_windows(vec![
        mac_winops::WindowInfo {
            id: 1,
            pid: 1,
            app: "Foo".into(),
            title: "Bar".into(),
            pos: None,
            space: None,
            layer: 0,
            focused: true,
            is_on_screen: true,
            on_active_space: true,
        },
        mac_winops::WindowInfo {
            id: 3,
            pid: 888,
            app: "Zed".into(),
            title: "Downloads".into(),
            pos: None,
            space: None,
            layer: 0,
            focused: false,
            is_on_screen: true,
            on_active_space: true,
        },
    ]);
    let world = engine.world();
    world.hint_refresh();
    let _ = wait_snapshot_until(world.as_ref(), 60, |snap| {
        snap.iter().any(|w| w.pid == 1 && w.focused)
            && snap.iter().any(|w| w.pid == 888 && !w.focused)
    })
    .await;
    let id = engine.resolve_id_for_ident("a").await.unwrap();
    engine
        .dispatch(id, mac_hotkey::EventKind::KeyDown, false)
        .await
        .expect("dispatch raise");
    assert!(mock.calls_contains("raise_window"));
    assert!(mock.calls_contains("ensure_frontmost"));
    assert!(
        !mock.calls_contains("activate_pid"),
        "raise should not fall back to activate when winops succeeds"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn engine_place_prefers_last_raise_pid_then_clears() {
    ensure_no_os_interaction();
    let (tx, _rx) = mpsc::channel(16);
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
        is_on_screen: true,
        on_active_space: true,
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
            is_on_screen: true,
            on_active_space: true,
        },
    ]);
    let api = Arc::new(MockHotkeyApi::new());
    let world = World::spawn_view(mock.clone(), hotki_world::WorldCfg::default());
    let engine = Engine::new_with_api_and_ops(api, tx, mock.clone(), false, world);
    let keys = keymode::Keys::from_ron(
        "[(\"r\", \"raise\", raise(title: \"raise-me\")), (\"p\", \"place\", place(grid(2,2), at(0,0)))]",
    )
    .unwrap();
    let cfg = config::Config::from_parts(keys, config::Style::default());
    let mut engine = engine;
    engine.set_config(cfg).await.unwrap();
    // World already has A focused and B present via set_windows above.
    let world = engine.world();
    world.hint_refresh();
    let _ = wait_snapshot_until(world.as_ref(), 60, |snap| {
        snap.iter().any(|w| w.pid == 200 && w.focused)
    })
    .await;
    // Raise to B
    let id_r = engine.resolve_id_for_ident("r").await.unwrap();
    engine
        .dispatch(id_r, mac_hotkey::EventKind::KeyDown, false)
        .await
        .expect("dispatch raise to B");
    // Place should prefer last raise pid (200)
    let id_p = engine.resolve_id_for_ident("p").await.unwrap();
    engine
        .dispatch(id_p, mac_hotkey::EventKind::KeyDown, false)
        .await
        .expect("dispatch place with raise pid");
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
            is_on_screen: true,
            on_active_space: true,
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
            is_on_screen: true,
            on_active_space: true,
        },
    ]);
    let world = engine.world();
    world.hint_refresh();
    let _ = wait_snapshot_until(world.as_ref(), 60, |snap| {
        snap.iter().any(|w| w.pid == 200 && w.focused)
    })
    .await;
    // Next place should use world-focused (cleared hint)
    engine
        .dispatch(id_p, mac_hotkey::EventKind::KeyDown, false)
        .await
        .expect("dispatch place with focus pid");
    assert_eq!(mock.last_place_grid_pid(), Some(200));
}

#[tokio::test(flavor = "current_thread")]
async fn engine_place_and_move_use_world_window_ids_for_pid_collisions() {
    ensure_no_os_interaction();
    let (tx, _rx) = mpsc::channel(16);
    let mock = Arc::new(MockWinOps::new());
    let api = Arc::new(MockHotkeyApi::new());
    let world = World::spawn_view(mock.clone(), fast_world_cfg());
    let engine = Engine::new_with_api_and_ops(api, tx, mock.clone(), false, world);
    mac_winops::reset_focused_fallback_count();
    let keys = keymode::Keys::from_ron(
        "[(\"p\", \"place\", place(grid(2,2), at(0,0))), (\"m\", \"move\", place_move(grid(2,2), right))]",
    )
    .unwrap();
    let cfg = config::Config::from_parts(keys, config::Style::default());
    let mut engine = engine;
    engine.set_config(cfg).await.unwrap();

    let pid = 4949;
    let decoy = mac_winops::WindowInfo {
        id: 10,
        pid,
        app: "PidTwin".into(),
        title: "Decoy".into(),
        pos: None,
        space: None,
        layer: 0,
        focused: false,
        is_on_screen: true,
        on_active_space: true,
    };
    let target = mac_winops::WindowInfo {
        id: 44,
        pid,
        app: "PidTwin".into(),
        title: "Target".into(),
        pos: None,
        space: None,
        layer: 0,
        focused: true,
        is_on_screen: true,
        on_active_space: true,
    };
    mock.set_frontmost_for_pid(Some(decoy.clone()));
    mock.set_windows(vec![decoy.clone(), target.clone()]);
    let world = engine.world();
    world.hint_refresh();
    let snapshot_ready = wait_snapshot_until(world.as_ref(), 120, |snap| {
        let pid_windows: Vec<_> = snap.iter().filter(|w| w.pid == pid).collect();
        pid_windows.len() == 2 && pid_windows.iter().any(|w| w.id == target.id && w.focused)
    })
    .await;
    assert!(
        snapshot_ready,
        "world should observe both windows with focus on target"
    );

    let place_id = engine.resolve_id_for_ident("p").await.unwrap();
    engine
        .dispatch(place_id, mac_hotkey::EventKind::KeyDown, false)
        .await
        .expect("place dispatch");

    let expected_target = WorldWindowId::new(pid, target.id);
    assert_eq!(
        mock.last_place_grid_target(),
        Some(expected_target),
        "place should target the world-selected window id"
    );
    assert!(
        mock.calls_contains("place_grid"),
        "place should call explicit id-based placement"
    );
    assert!(
        !mock.calls_contains("place_grid_focused"),
        "place should not fall back to focused placement"
    );

    let move_id = engine.resolve_id_for_ident("m").await.unwrap();
    engine
        .dispatch(move_id, mac_hotkey::EventKind::KeyDown, false)
        .await
        .expect("place_move dispatch");
    assert_eq!(
        mock.last_place_move_target(),
        Some(expected_target),
        "place_move should retain the same world window id"
    );
    assert_eq!(
        mac_winops::focused_fallback_count(),
        0,
        "tests should trigger observability if fallback placement sneaks in"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn engine_place_rejects_offspace_window() {
    ensure_no_os_interaction();
    let (tx, mut rx) = mpsc::channel(16);
    let mock = Arc::new(MockWinOps::new());
    let api = Arc::new(MockHotkeyApi::new());
    let world = World::spawn_view(mock.clone(), hotki_world::WorldCfg::default());
    let engine = Engine::new_with_api_and_ops(api, tx, mock.clone(), false, world);
    let keys = keymode::Keys::from_ron("[(\"p\", \"place\", place(grid(2,2), at(0,0)))]").unwrap();
    let cfg = config::Config::from_parts(keys, config::Style::default());
    let mut engine = engine;
    engine.set_config(cfg).await.unwrap();

    set_world_focus(&engine, &mock, "App", "Win", 77).await;

    mock.set_windows(vec![mac_winops::WindowInfo {
        id: 1,
        pid: 77,
        app: "App".into(),
        title: "Win".into(),
        pos: None,
        space: Some(5),
        layer: 0,
        focused: true,
        is_on_screen: false,
        on_active_space: false,
    }]);
    let world = engine.world();
    world.hint_refresh();
    let _ = wait_snapshot_until(world.as_ref(), 80, |snap| {
        snap.iter().any(|w| w.pid == 77 && !w.on_active_space)
    })
    .await;

    let id = engine.resolve_id_for_ident("p").await.unwrap();
    let err = engine
        .dispatch(id, mac_hotkey::EventKind::KeyDown, false)
        .await
        .expect_err("expected place guard error");
    assert!(matches!(err, Error::OffActiveSpace { op: "place", .. }));

    let saw = recv_error_with_title(&mut rx, "Place", 80).await;
    assert!(saw, "expected Place guard notification");
    assert!(
        !mock.calls_contains("place_grid_focused"),
        "place operation should not reach winops"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn engine_place_move_rejects_offspace_window() {
    ensure_no_os_interaction();
    let (tx, mut rx) = mpsc::channel(16);
    let mock = Arc::new(MockWinOps::new());
    let api = Arc::new(MockHotkeyApi::new());
    let world = World::spawn_view(mock.clone(), hotki_world::WorldCfg::default());
    let engine = Engine::new_with_api_and_ops(api, tx, mock.clone(), false, world);
    let keys =
        keymode::Keys::from_ron("[(\"m\", \"move\", place_move(grid(2,2), right))]").unwrap();
    let cfg = config::Config::from_parts(keys, config::Style::default());
    let mut engine = engine;
    engine.set_config(cfg).await.unwrap();

    set_world_focus(&engine, &mock, "App", "Win", 42).await;
    let world = engine.world();

    mock.set_windows(Vec::new());
    world.hint_refresh();
    let _ = wait_snapshot_until(world.as_ref(), 80, |snap| snap.is_empty()).await;

    mock.set_windows(vec![mac_winops::WindowInfo {
        id: 9,
        pid: 42,
        app: "App".into(),
        title: "Win".into(),
        pos: None,
        space: Some(3),
        layer: 0,
        focused: true,
        is_on_screen: true,
        on_active_space: false,
    }]);
    world.hint_refresh();
    let _ = wait_snapshot_until(world.as_ref(), 80, |snap| {
        snap.iter().any(|w| w.pid == 42 && !w.on_active_space)
    })
    .await;

    let id = engine.resolve_id_for_ident("m").await.unwrap();
    let err = engine
        .dispatch(id, mac_hotkey::EventKind::KeyDown, false)
        .await
        .expect_err("expected move guard error");
    assert!(matches!(
        err,
        Error::OffActiveSpace {
            op: "place_move",
            ..
        }
    ));

    let saw = recv_error_with_title(&mut rx, "Move", 150).await;
    assert!(saw, "expected Move guard notification");
    assert!(
        !mock.calls_contains("place_move"),
        "place_move should not run for off-space window"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn engine_raise_rejects_offspace_window() {
    ensure_no_os_interaction();
    let (tx, mut rx) = mpsc::channel(16);
    let mock = Arc::new(MockWinOps::new());
    let api = Arc::new(MockHotkeyApi::new());
    let world = World::spawn_view(mock.clone(), hotki_world::WorldCfg::default());
    let engine = Engine::new_with_api_and_ops(api, tx, mock.clone(), false, world);
    let keys =
        keymode::Keys::from_ron("[(\"r\", \"raise\", raise(app: \"^Target$\", title: \"Win\"))]")
            .unwrap();
    let cfg = config::Config::from_parts(keys, config::Style::default());
    let mut engine = engine;
    engine.set_config(cfg).await.unwrap();

    mock.set_windows(vec![
        mac_winops::WindowInfo {
            id: 1,
            pid: 10,
            app: "Other".into(),
            title: "Front".into(),
            pos: None,
            space: Some(1),
            layer: 0,
            focused: true,
            is_on_screen: true,
            on_active_space: true,
        },
        mac_winops::WindowInfo {
            id: 2,
            pid: 20,
            app: "Target".into(),
            title: "Win".into(),
            pos: None,
            space: Some(4),
            layer: 0,
            focused: false,
            is_on_screen: false,
            on_active_space: false,
        },
    ]);
    let world = engine.world();
    world.hint_refresh();
    let _ = wait_snapshot_until(world.as_ref(), 80, |snap| {
        snap.iter().any(|w| w.pid == 20 && !w.on_active_space)
    })
    .await;

    let id = engine.resolve_id_for_ident("r").await.unwrap();
    let err = engine
        .dispatch(id, mac_hotkey::EventKind::KeyDown, false)
        .await
        .expect_err("expected raise guard error");
    assert!(matches!(err, Error::OffActiveSpace { op: "raise", .. }));

    let saw = recv_error_with_title(&mut rx, "Raise", 80).await;
    assert!(saw, "expected Raise guard notification");
    assert!(
        !mock.calls_contains("raise_window"),
        "raise should not schedule off-space window"
    );
    assert!(
        !mock.calls_contains("ensure_frontmost"),
        "raise should not run ensure_frontmost off-space"
    );
    assert!(
        !mock.calls_contains("activate_pid"),
        "raise should not activate off-space window"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn engine_fullscreen_error_notifies() {
    ensure_no_os_interaction();
    let (tx, mut rx) = mpsc::channel(16);
    let mock = Arc::new(MockWinOps::new());
    mock.set_fail_fullscreen_nonnative(true);
    let api = Arc::new(MockHotkeyApi::new());
    let world = World::spawn_view(mock.clone(), hotki_world::WorldCfg::default());
    let engine = Engine::new_with_api_and_ops(api, tx, mock.clone(), false, world);
    let keys = keymode::Keys::from_ron("[(\"f\", \"fs\", fullscreen(on, nonnative))]").unwrap();
    let cfg = config::Config::from_parts(keys, config::Style::default());
    let mut engine = engine;
    engine.set_config(cfg).await.unwrap();
    set_world_focus(&engine, &mock, "X", "T", 11).await;
    let id = engine.resolve_id_for_ident("f").await.unwrap();
    engine
        .dispatch(id, mac_hotkey::EventKind::KeyDown, false)
        .await
        .expect("dispatch raise invalid regex");
    // expect an error Notify with title "Fullscreen"
    let saw = recv_error_with_title(&mut rx, "Fullscreen", 80).await;
    assert!(saw, "expected Fullscreen error notification");
}

#[tokio::test(flavor = "current_thread")]
async fn engine_hide_error_notifies() {
    ensure_no_os_interaction();
    let (tx, mut rx) = mpsc::channel(16);
    let mock = Arc::new(MockWinOps::new());
    mock.set_fail_hide(true);
    let api = Arc::new(MockHotkeyApi::new());
    let world = World::spawn_view(mock.clone(), hotki_world::WorldCfg::default());
    let engine = Engine::new_with_api_and_ops(api, tx, mock.clone(), false, world);
    let keys = keymode::Keys::from_ron("[(\"h\", \"hide\", hide(on))]").unwrap();
    let cfg = config::Config::from_parts(keys, config::Style::default());
    let mut engine = engine;
    engine.set_config(cfg).await.unwrap();
    set_world_focus(&engine, &mock, "X", "T", 22).await;
    let id = engine.resolve_id_for_ident("h").await.unwrap();
    engine
        .dispatch(id, mac_hotkey::EventKind::KeyDown, false)
        .await
        .expect("dispatch focus left error");
    let saw = recv_error_with_title(&mut rx, "Hide", 80).await;
    assert!(saw, "expected Hide error notification");
}

#[tokio::test(flavor = "current_thread")]
async fn engine_raise_invalid_regex_notifies() {
    ensure_no_os_interaction();
    let (tx, mut rx) = mpsc::channel(16);
    let mock = Arc::new(MockWinOps::new());
    let api = Arc::new(MockHotkeyApi::new());
    let world = World::spawn_view(mock.clone(), hotki_world::WorldCfg::default());
    let engine = Engine::new_with_api_and_ops(api, tx, mock.clone(), false, world);
    // invalid regex for app
    let keys = keymode::Keys::from_ron("[(\"r\", \"raise\", raise(app: \"(unclosed\"))]").unwrap();
    let cfg = config::Config::from_parts(keys, config::Style::default());
    let mut engine = engine;
    engine.set_config(cfg).await.unwrap();
    set_world_focus(&engine, &mock, "X", "T", 33).await;
    let id = engine.resolve_id_for_ident("r").await.unwrap();
    engine
        .dispatch(id, mac_hotkey::EventKind::KeyDown, false)
        .await
        .expect("dispatch place_move");
    let saw = recv_error_with_title(&mut rx, "Raise", 80).await;
    assert!(saw, "expected Raise error notification for invalid regex");
}

#[tokio::test(flavor = "current_thread")]
async fn engine_focus_error_propagates_notification() {
    ensure_no_os_interaction();
    let (tx, mut rx) = mpsc::channel(16);
    let mock = Arc::new(MockWinOps::new());
    mock.set_fail_focus_dir(true);
    let api = Arc::new(MockHotkeyApi::new());
    let world = World::spawn_noop_view();
    let engine = Engine::new_with_api_and_ops(api, tx, mock.clone(), false, world);
    let keys = keymode::Keys::from_ron("[(\"a\", \"focus left\", focus(left))]").unwrap();
    let cfg = config::Config::from_parts(keys, config::Style::default());
    let mut engine = engine;
    engine.set_config(cfg).await.unwrap();
    set_world_focus(&engine, &mock, "X", "T", 123).await;
    let id = engine.resolve_id_for_ident("a").await.unwrap();
    engine
        .dispatch(id, mac_hotkey::EventKind::KeyDown, false)
        .await
        .expect("dispatch focus dir error");

    // Drain messages until we see an error notification about Focus
    let saw_error = recv_error_with_title(&mut rx, "Focus", 80).await;
    assert!(saw_error, "expected Focus error notification");
}

// Debounce-based raise behavior removed: world-only model does no engine-side retries.

#[tokio::test(flavor = "current_thread")]
async fn engine_place_move_uses_winops() {
    ensure_no_os_interaction();
    let (tx, _rx): (mpsc::Sender<MsgToUI>, mpsc::Receiver<MsgToUI>) = mpsc::channel(32);
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
        is_on_screen: true,
        on_active_space: true,
    }]);
    let mgr = Arc::new(mac_hotkey::Manager::new().expect("manager"));
    let engine = Engine::new_with_ops(mgr, tx, mock.clone());
    let keys =
        keymode::Keys::from_ron("[(\"a\", \"move left\", place_move(grid(2,2), left))]").unwrap();
    let cfg = config::Config::from_parts(keys, config::Style::default());
    let mut engine = engine;
    engine.set_config(cfg).await.unwrap();
    set_world_focus(&engine, &mock, "X", "T", 99).await;
    let id = engine
        .resolve_id_for_ident("a")
        .await
        .expect("registered id");
    engine
        .dispatch(id, mac_hotkey::EventKind::KeyDown, false)
        .await
        .expect("dispatch place_move success");
    assert!(mock.calls_contains("place_move"));
}
