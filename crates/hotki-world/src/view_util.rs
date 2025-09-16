//! Convenience helpers layered on top of [`WorldView`].

use crate::{WindowKey, WorldView, WorldWindow};

/// Fetch a complete snapshot via the [`WorldView`].
pub async fn list_windows<W>(world: &W) -> Vec<WorldWindow>
where
    W: WorldView + ?Sized,
{
    world.snapshot().await
}

/// Resolve the frontmost window using focus preference, falling back to lowest `z`.
pub async fn frontmost_window<W>(world: &W) -> Option<WorldWindow>
where
    W: WorldView + ?Sized,
{
    if let Some(focused) = world.focused_window().await {
        return Some(focused);
    }
    let snap = world.snapshot().await;
    snap.into_iter().min_by_key(|w| w.z)
}

/// Resolve a window by `WindowKey` if still present in the snapshot.
pub async fn resolve_key<W>(world: &W, key: WindowKey) -> Option<WorldWindow>
where
    W: WorldView + ?Sized,
{
    world
        .snapshot()
        .await
        .into_iter()
        .find(|w| w.pid == key.pid && w.id == key.id)
}

/// Resolve a window by `(pid, title)` pair.
pub async fn window_by_pid_title<W>(world: &W, pid: i32, title: &str) -> Option<WorldWindow>
where
    W: WorldView + ?Sized,
{
    let title_norm = title;
    world
        .snapshot()
        .await
        .into_iter()
        .find(|w| w.pid == pid && w.title == title_norm)
}

/// Check whether any window satisfies predicate.
pub async fn any_window_matching<W, F>(world: &W, mut pred: F) -> bool
where
    W: WorldView + ?Sized,
    F: FnMut(&WorldWindow) -> bool,
{
    world.snapshot().await.into_iter().any(|w| pred(&w))
}
