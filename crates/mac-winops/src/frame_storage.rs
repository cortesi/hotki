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
