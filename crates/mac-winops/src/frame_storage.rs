use std::collections::HashMap;

use once_cell::sync::Lazy;
use parking_lot::Mutex;

use crate::{
    WindowId,
    geom::{Point, Size},
};

/// In-memory storage of pre-maximize frames to allow toggling back.
pub type FrameKey = (i32, WindowId);
pub type FrameVal = (Point, Size);

pub static PREV_FRAMES: Lazy<Mutex<HashMap<FrameKey, FrameVal>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));
pub const PREV_FRAMES_CAP: usize = 256;

/// Frames stored before hiding so we can restore on reveal
pub static HIDDEN_FRAMES: Lazy<Mutex<HashMap<FrameKey, FrameVal>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));
pub const HIDDEN_FRAMES_CAP: usize = 512;

/// Target positions we drove hidden windows towards when hiding.
pub static HIDDEN_TARGETS: Lazy<Mutex<HashMap<FrameKey, Point>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

/// Return true if the hide cache currently tracks `(pid, id)`.
#[must_use]
pub fn is_hidden(pid: i32, id: WindowId) -> bool {
    HIDDEN_FRAMES.lock().contains_key(&(pid, id))
}

/// Retrieve the stored frame for a hidden window, if any.
#[must_use]
pub fn hidden_frame(pid: i32, id: WindowId) -> Option<FrameVal> {
    HIDDEN_FRAMES.lock().get(&(pid, id)).cloned()
}

/// Remove a hidden frame entry if it exists.
pub fn clear_hidden(pid: i32, id: WindowId) {
    let key = (pid, id);
    HIDDEN_FRAMES.lock().remove(&key);
    HIDDEN_TARGETS.lock().remove(&key);
}

/// Record both the frame prior to hiding and the target frame we moved towards.
pub fn store_hidden(pid: i32, id: WindowId, frame: FrameVal, target: Option<Point>) {
    let key = (pid, id);
    {
        let mut frames = HIDDEN_FRAMES.lock();
        if frames.len() >= HIDDEN_FRAMES_CAP
            && let Some(old_key) = frames.keys().next().copied()
        {
            frames.remove(&old_key);
            HIDDEN_TARGETS.lock().remove(&old_key);
        }
        frames.insert(key, frame);
    }
    match target {
        Some(target_point) => {
            HIDDEN_TARGETS.lock().insert(key, target_point);
        }
        None => {
            HIDDEN_TARGETS.lock().remove(&key);
        }
    }
}

/// Retrieve the overshoot target for a hidden window, if tracked.
#[must_use]
pub fn hidden_target(pid: i32, id: WindowId) -> Option<Point> {
    HIDDEN_TARGETS.lock().get(&(pid, id)).copied()
}
