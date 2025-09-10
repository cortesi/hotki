use std::sync::Arc;

use hotki_engine::Engine;
use mac_winops::ops::MockWinOps;
use hotki_protocol::MsgToUI;
use tokio::sync::mpsc;

// Using mac_winops::ops::MockWinOps provided under the `test-utils` feature.

fn ensure_no_os_interaction() {
    use std::sync::Once;
    static INIT: Once = Once::new();
    INIT.call_once(|| unsafe {
        std::env::set_var("HOTKI_TEST_FAKE_BINDINGS", "1");
        std::env::set_var("HOTKI_TEST_FAKE_RELAY", "1");
    });
}

#[tokio::test(flavor = "multi_thread")]
async fn engine_uses_window_ops_for_focus() {
    ensure_no_os_interaction();
    let (tx, mut _rx): (
        mpsc::UnboundedSender<MsgToUI>,
        mpsc::UnboundedReceiver<MsgToUI>,
    ) = mpsc::unbounded_channel();
    let mock = Arc::new(MockWinOps::new());
    let mgr = Arc::new(mac_hotkey::Manager::new().expect("manager"));
    let engine = Engine::new_with_ops(mgr, tx, mock.clone());

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

#[tokio::test(flavor = "multi_thread")]
async fn engine_hide_uses_winops() {
    ensure_no_os_interaction();
    let (tx, _rx) = mpsc::unbounded_channel();
    let mock = Arc::new(MockWinOps::new());
    let mgr = Arc::new(mac_hotkey::Manager::new().expect("manager"));
    let engine = Engine::new_with_ops(mgr, tx, mock.clone());
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

#[tokio::test(flavor = "multi_thread")]
async fn engine_fullscreen_routes_native_and_nonnative() {
    ensure_no_os_interaction();
    let (tx, _rx) = mpsc::unbounded_channel();
    let mock = Arc::new(MockWinOps::new());
    let mgr = Arc::new(mac_hotkey::Manager::new().expect("manager"));
    let engine = Engine::new_with_ops(mgr, tx, mock.clone());
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

#[tokio::test(flavor = "multi_thread")]
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
    let mgr = Arc::new(mac_hotkey::Manager::new().expect("manager"));
    let engine = Engine::new_with_ops(mgr, tx, mock.clone());
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

#[tokio::test(flavor = "multi_thread")]
async fn engine_place_move_uses_winops() {
    ensure_no_os_interaction();
    let (tx, _rx): (mpsc::UnboundedSender<MsgToUI>, mpsc::UnboundedReceiver<MsgToUI>) =
        mpsc::unbounded_channel();
    let mock = Arc::new(MockWinOps::new());
    // Ensure the engine finds a frontmost window for current pid
    mock.set_frontmost_for_pid(Some(mac_winops::WindowInfo {
        id: 7,
        pid: 99,
        app: "X".into(),
        title: "T".into(),
        pos: None,
        space: None,
        layer: 0,
        focused: true,
    }));
    let mgr = Arc::new(mac_hotkey::Manager::new().expect("manager"));
    let engine = Engine::new_with_ops(mgr, tx, mock.clone());
    let keys = keymode::Keys::from_ron(
        "[(\"a\", \"move left\", place_move(grid(2,2), left))]",
    )
    .unwrap();
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
