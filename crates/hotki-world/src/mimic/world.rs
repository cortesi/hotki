use std::{future::Future, sync::Arc, thread, time::Duration};

use hotki_world_ids::WorldWindowId;
use mac_winops::{self, WindowInfo, ops::RealWinOps};
use once_cell::sync::OnceCell;
use regex::Regex;
use tokio::runtime::{Builder, Runtime};

use crate::{
    CommandReceipt, PlaceAttemptOptions, RaiseIntent, World, WorldCfg, WorldView, WorldWindow,
};

type Result<T> = std::result::Result<T, String>;

static RUNTIME: OnceCell<Runtime> = OnceCell::new();
static WORLD: OnceCell<Arc<dyn WorldView>> = OnceCell::new();

fn runtime() -> &'static Runtime {
    RUNTIME.get_or_init(|| {
        Builder::new_multi_thread()
            .enable_all()
            .worker_threads(2)
            .thread_name("mimic-rt")
            .build()
            .expect("failed to build mimic runtime")
    })
}

fn block_on<F>(fut: F) -> F::Output
where
    F: Future,
{
    runtime().block_on(fut)
}

fn ensure_world() -> Result<Arc<dyn WorldView>> {
    let rt = runtime();
    let guard = rt.enter();
    let world = WORLD
        .get_or_init(|| {
            let winops = Arc::new(RealWinOps);
            World::spawn_view(winops, WorldCfg::default())
        })
        .clone();
    drop(guard);
    Ok(world)
}

fn convert_window(w: WorldWindow) -> WindowInfo {
    WindowInfo {
        app: w.app,
        title: w.title,
        pid: w.pid,
        id: w.id,
        pos: w.pos,
        space: w.space,
        layer: w.layer,
        focused: w.focused,
        is_on_screen: w.is_on_screen,
        on_active_space: w.on_active_space,
    }
}

pub(super) fn list_windows() -> Result<Vec<WindowInfo>> {
    let world = ensure_world()?;
    let windows = block_on(async move { world.list_windows().await })
        .into_iter()
        .map(convert_window)
        .collect();
    Ok(windows)
}

pub(super) fn place_window(
    target: WorldWindowId,
    cols: u32,
    rows: u32,
    col: u32,
    row: u32,
    options: Option<PlaceAttemptOptions>,
) -> Result<CommandReceipt> {
    let world = ensure_world()?;
    let receipt = block_on(async move {
        world
            .request_place_for_window(target, cols, rows, col, row, options)
            .await
    })
    .map_err(|err| format!("world place_window failed: {err:?}"))?;
    mac_winops::drain_main_ops();
    Ok(receipt)
}

pub(super) fn ensure_frontmost(
    pid: i32,
    title: &str,
    attempts: usize,
    delay_ms: u64,
) -> Result<()> {
    let regex = Regex::new(&format!("^{}$", regex::escape(title)))
        .map_err(|e| format!("invalid title regex: {e}"))?;
    let intent = RaiseIntent {
        app_regex: None,
        title_regex: Some(Arc::new(regex)),
    };

    for attempt in 0..attempts {
        let world = ensure_world()?;
        let receipt = block_on(async { world.request_raise(intent.clone()).await })
            .map_err(|err| format!("world raise failed: {err:?}"))?;
        if let Some(target) = receipt.target
            && target.pid == pid
            && target.title == title
        {
            return Ok(());
        }
        if attempt + 1 < attempts {
            thread::sleep(Duration::from_millis(delay_ms));
        }
    }

    Err(format!(
        "failed to raise window pid={} title='{}' after {} attempts",
        pid, title, attempts
    ))
}
