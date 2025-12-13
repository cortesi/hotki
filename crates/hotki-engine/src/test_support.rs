//! Test support utilities for hotki-engine integration/unit tests.
//! These helpers are public to avoid dead_code warnings and are lightweight.
//! They are intended for use by the test suite only.

use std::{
    env, fs,
    future::Future,
    path::PathBuf,
    process,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use hotki_protocol::MsgToUI;
use hotki_world::{FocusChange, TestWorld, WindowKey, WorldEvent, WorldView, WorldWindow};
use tokio::{
    sync::mpsc,
    time::{Instant, sleep},
};

/// Create a low-latency `hotki_world` configuration suitable for tests.
pub fn fast_world_cfg() -> hotki_world::WorldCfg {
    hotki_world::WorldCfg {
        poll_ms_min: 1,
        poll_ms_max: 10,
    }
}

/// Run an asynchronous engine test body on a dedicated runtime.
///
/// The helper ensures the runtime shuts down promptly once the test future completes.
pub fn run_engine_test<F>(fut: F)
where
    F: Future<Output = ()> + Send + 'static,
{
    hotki_world::test_support::run_async_test(fut);
}

/// Await until the world snapshot satisfies `pred`, up to `timeout_ms`.
pub async fn wait_snapshot_until<F, W>(world: &W, timeout_ms: u64, mut pred: F) -> bool
where
    W: WorldView + ?Sized,
    F: FnMut(&[hotki_world::WorldWindow]) -> bool,
{
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        let snapshot = world.snapshot().await;
        if pred(&snapshot) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        sleep(Duration::from_millis(2)).await;
    }
}

/// Receive an error notification with a specific `title` within `timeout_ms`.
pub async fn recv_error_with_title(
    rx: &mut tokio::sync::mpsc::Receiver<MsgToUI>,
    title: &str,
    timeout_ms: u64,
) -> bool {
    let want = title.to_string();
    tokio::time::timeout(Duration::from_millis(timeout_ms), async {
        while let Some(msg) = rx.recv().await {
            if let MsgToUI::Notify { kind, title, .. } = msg
                && matches!(kind, hotki_protocol::NotifyKind::Error)
                && title == want
            {
                return true;
            }
        }
        false
    })
    .await
    .unwrap_or(false)
}

/// Construct a test engine with default mock components and optional relay support.
pub async fn create_test_engine_with_relay(
    relay_enabled: bool,
) -> (crate::Engine, mpsc::Receiver<MsgToUI>, Arc<TestWorld>) {
    let (tx, rx) = mpsc::channel(128);
    let api = Arc::new(crate::MockHotkeyApi::new());
    let world = Arc::new(TestWorld::new());
    let engine = crate::Engine::new_with_api_and_world(api, tx, relay_enabled, world.clone());
    (engine, rx, world)
}

/// Construct a test engine with relay disabled (common default).
pub async fn create_test_engine() -> (crate::Engine, mpsc::Receiver<MsgToUI>, Arc<TestWorld>) {
    create_test_engine_with_relay(false).await
}

/// Hook to assert no platform interaction occurs during tests (currently a no-op).
pub fn ensure_no_os_interaction() {}

/// Seed the world focus to a specific window and wait for it to be observed.
pub async fn set_world_focus(world: &TestWorld, app: &str, title: &str, pid: i32) {
    let window = WorldWindow {
        app: app.into(),
        title: title.into(),
        pid,
        id: 1,
        display_id: None,
        focused: true,
    };
    let key = WindowKey { pid, id: window.id };
    world.set_snapshot(vec![window], Some(key));
    world.push_event(WorldEvent::FocusChanged(FocusChange {
        key: Some(key),
        app: Some(app.into()),
        title: Some(title.into()),
        pid: Some(pid),
        display_id: None,
    }));

    let ready = wait_snapshot_until(world, 200, |snap| {
        snap.iter().any(|w| w.pid == pid && w.focused)
    })
    .await;
    assert!(
        ready,
        "world failed to observe focused window pid={pid} app={app} title={title}"
    );
}

fn temp_config_path(prefix: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    env::temp_dir().join(format!("{prefix}-{}-{counter}.rhai", process::id()))
}

/// Load a Rhai config script from an in-memory string for tests.
pub fn load_test_config(script: &str) -> config::Config {
    let path = temp_config_path("hotki-test-config");
    fs::write(&path, script).expect("write test config");
    let loaded = config::load_for_server_from_path(&path).expect("load test config");
    let _ignored = fs::remove_file(&path);
    loaded.config
}

/// Minimal configuration used across engine tests.
pub fn create_test_config() -> config::Config {
    load_test_config(
        r#"
        global.mode("cmd+k", "test", |m| {
          m.bind("a", "action", pop);
          m.mode("b", "nested", |sub| {
            sub.bind("c", "deep", pop);
          });
        });
        "#,
    )
}

/// Receive UI messages until `pred` matches or `timeout_ms` elapses.
pub async fn recv_until<F>(
    rx: &mut tokio::sync::mpsc::Receiver<MsgToUI>,
    timeout_ms: u64,
    mut pred: F,
) -> bool
where
    F: FnMut(&MsgToUI) -> bool,
{
    tokio::time::timeout(Duration::from_millis(timeout_ms), async {
        while let Some(msg) = rx.recv().await {
            if pred(&msg) {
                return true;
            }
        }
        false
    })
    .await
    .unwrap_or(false)
}
