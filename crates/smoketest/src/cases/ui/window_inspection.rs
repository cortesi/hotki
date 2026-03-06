use std::cmp::Ordering;

use core_foundation::{
    array::CFArray,
    base::{CFType, ItemRef, TCFType},
    dictionary::CFDictionary,
    number::CFNumber,
    string::CFString,
};
use core_graphics::{
    display::CGDisplay,
    geometry::{CGPoint, CGRect, CGSize},
    window::{
        copy_window_info, kCGNullWindowID, kCGWindowBounds, kCGWindowIsOnscreen, kCGWindowLayer,
        kCGWindowListExcludeDesktopElements, kCGWindowListOptionOnScreenOnly, kCGWindowName,
        kCGWindowNumber, kCGWindowOwnerName, kCGWindowOwnerPID,
    },
};
use serde::Serialize;

use crate::error::{Error, Result};

/// Serializable summary of a HUD window observation.
#[derive(Clone, Serialize)]
pub(super) struct HudWindowSnapshot {
    /// Owning process identifier.
    pub(super) pid: i32,
    /// Window identifier within the process.
    pub(super) id: u32,
    /// Observed window title.
    pub(super) title: String,
    /// Window layer as reported by CoreGraphics.
    pub(super) layer: i32,
    /// Whether the window was reported on-screen.
    pub(super) is_on_screen: bool,
    /// Display identifier derived from window bounds, when available.
    pub(super) display_id: Option<u32>,
}

/// Lightweight description of a display's bounds in bottom-left coordinates.
#[derive(Clone, Copy, Debug)]
pub(super) struct DisplayFrame {
    /// Display identifier.
    pub(super) id: u32,
    /// Origin X coordinate.
    x: f32,
    /// Origin Y coordinate.
    y: f32,
    /// Width in pixels.
    width: f32,
    /// Height in pixels.
    height: f32,
}

/// Collect HUD windows belonging to the active Hotki session.
pub(super) fn collect_hud_windows(pid: i32) -> Result<Vec<HudWindowSnapshot>> {
    let displays = enumerate_displays()?;
    let options = kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements;
    let windows: CFArray = copy_window_info(options, kCGNullWindowID)
        .ok_or_else(|| Error::InvalidState("failed to read window list".into()))?;

    let keys = WindowDictKeys::new();
    let mut snapshots = Vec::new();
    for raw in windows.iter() {
        let dict_ptr = *raw;
        let dict: CFDictionary<CFString, CFType> =
            unsafe { CFDictionary::wrap_under_get_rule(dict_ptr as _) };

        if dict_value_i32(&dict, &keys.owner_pid) != Some(pid) {
            continue;
        }

        let title = dict_value_string(&dict, &keys.name).unwrap_or_default();
        let id = dict_value_u32(&dict, &keys.number).unwrap_or(0);
        let layer = dict_value_i32(&dict, &keys.layer).unwrap_or(-1);
        let is_on_screen = dict_value_bool(&dict, &keys.onscreen).unwrap_or(false);
        let display_id = dict_value_rect(&dict, &keys.bounds)
            .as_ref()
            .and_then(|bounds| display_for_rect(bounds, &displays));

        snapshots.push(HudWindowSnapshot {
            pid,
            id,
            title: if title.is_empty() {
                dict_value_string(&dict, &keys.owner_name).unwrap_or_default()
            } else {
                title
            },
            layer,
            is_on_screen,
            display_id,
        });
    }

    Ok(snapshots)
}

/// Enumerate active displays and produce simple bounding frames.
pub(super) fn enumerate_displays() -> Result<Vec<DisplayFrame>> {
    let mut frames = Vec::new();
    if let Ok(active) = CGDisplay::active_displays() {
        for id in active {
            let display = CGDisplay::new(id);
            let bounds: CGRect = display.bounds();
            frames.push(DisplayFrame {
                id: display.id,
                x: bounds.origin.x as f32,
                y: bounds.origin.y as f32,
                width: bounds.size.width as f32,
                height: bounds.size.height as f32,
            });
        }
    }

    if frames.is_empty() {
        return Err(Error::InvalidState("no active displays detected".into()));
    }
    Ok(frames)
}

/// Resolve the display identifier containing the currently focused window.
pub(super) fn focused_display_id(displays: &[DisplayFrame]) -> Option<u32> {
    let options = kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements;
    let windows: CFArray = copy_window_info(options, kCGNullWindowID)?;
    let layer_key = unsafe { CFString::wrap_under_get_rule(kCGWindowLayer) };
    let bounds_key = unsafe { CFString::wrap_under_get_rule(kCGWindowBounds) };

    for raw in windows.iter() {
        let dict_ptr = *raw;
        let dict: CFDictionary<CFString, CFType> =
            unsafe { CFDictionary::wrap_under_get_rule(dict_ptr as _) };
        let layer = dict_value_i32(&dict, &layer_key).unwrap_or(1);
        if layer != 0 {
            continue;
        }
        let display = dict_value_rect(&dict, &bounds_key)
            .as_ref()
            .and_then(|bounds| display_for_rect(bounds, displays));
        if display.is_some() {
            return display;
        }
        break;
    }
    None
}

/// Pick the display that contains the majority of a rectangle.
fn display_for_rect(bounds: &CGRect, displays: &[DisplayFrame]) -> Option<u32> {
    if displays.is_empty() {
        return None;
    }

    let center_x = (bounds.origin.x + bounds.size.width * 0.5) as f32;
    let center_y = (bounds.origin.y + bounds.size.height * 0.5) as f32;

    if let Some(display) = displays
        .iter()
        .find(|display| point_in_display(display, center_x, center_y))
    {
        return Some(display.id);
    }

    displays
        .iter()
        .map(|display| (display.id, overlap_area(bounds, display)))
        .filter(|(_, area)| *area > 0.0)
        .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(Ordering::Equal))
        .map(|(id, _)| id)
}

/// Check whether a point lies within a display frame.
fn point_in_display(display: &DisplayFrame, x: f32, y: f32) -> bool {
    x >= display.x
        && x <= display.x + display.width
        && y >= display.y
        && y <= display.y + display.height
}

/// Compute the area of overlap between a rect and a display.
fn overlap_area(bounds: &CGRect, display: &DisplayFrame) -> f32 {
    let left = bounds.origin.x.max(display.x as f64) as f32;
    let right =
        (bounds.origin.x + bounds.size.width).min((display.x + display.width) as f64) as f32;
    let bottom = bounds.origin.y.max(display.y as f64) as f32;
    let top =
        (bounds.origin.y + bounds.size.height).min((display.y + display.height) as f64) as f32;

    let width = (right - left).max(0.0);
    let height = (top - bottom).max(0.0);
    width * height
}

/// Cached CoreGraphics dictionary keys used when scanning windows.
struct WindowDictKeys {
    /// Window layer field.
    layer: CFString,
    /// Owning process identifier field.
    owner_pid: CFString,
    /// Owning process name field.
    owner_name: CFString,
    /// Window title field.
    name: CFString,
    /// Window number field.
    number: CFString,
    /// Window bounds field.
    bounds: CFString,
    /// On-screen flag field.
    onscreen: CFString,
}

impl WindowDictKeys {
    /// Build the CoreGraphics key set once per scan.
    fn new() -> Self {
        Self {
            layer: unsafe { CFString::wrap_under_get_rule(kCGWindowLayer) },
            owner_pid: unsafe { CFString::wrap_under_get_rule(kCGWindowOwnerPID) },
            owner_name: unsafe { CFString::wrap_under_get_rule(kCGWindowOwnerName) },
            name: unsafe { CFString::wrap_under_get_rule(kCGWindowName) },
            number: unsafe { CFString::wrap_under_get_rule(kCGWindowNumber) },
            bounds: unsafe { CFString::wrap_under_get_rule(kCGWindowBounds) },
            onscreen: unsafe { CFString::wrap_under_get_rule(kCGWindowIsOnscreen) },
        }
    }
}

/// Read a string from a CoreGraphics window dictionary value.
fn dict_value_string(dict: &CFDictionary<CFString, CFType>, key: &CFString) -> Option<String> {
    dict.find(key)
        .and_then(|value: ItemRef<CFType>| value.downcast::<CFString>())
        .map(|value: CFString| value.to_string())
}

/// Read a boolean from a CoreGraphics window dictionary value.
fn dict_value_bool(dict: &CFDictionary<CFString, CFType>, key: &CFString) -> Option<bool> {
    dict.find(key)
        .and_then(|value: ItemRef<CFType>| value.downcast::<CFNumber>())
        .and_then(|value: CFNumber| value.to_i64())
        .map(|value| value != 0)
}

/// Read an i32 from a CoreGraphics window dictionary value.
fn dict_value_i32(dict: &CFDictionary<CFString, CFType>, key: &CFString) -> Option<i32> {
    dict.find(key)
        .and_then(|value: ItemRef<CFType>| value.downcast::<CFNumber>())
        .and_then(|value: CFNumber| value.to_i64())
        .map(|value| value as i32)
}

/// Read a u32 from a CoreGraphics window dictionary value.
fn dict_value_u32(dict: &CFDictionary<CFString, CFType>, key: &CFString) -> Option<u32> {
    dict_value_i32(dict, key).map(|value| value as u32)
}

/// Extract a CGRect from a CoreGraphics window dictionary.
fn dict_value_rect(dict: &CFDictionary<CFString, CFType>, key: &CFString) -> Option<CGRect> {
    let bounds_dict: CFDictionary<CFString, CFType> =
        unsafe { CFDictionary::wrap_under_get_rule(dict.find(key)?.as_CFTypeRef() as _) };
    let x = dict_value_f32(&bounds_dict, "X")?;
    let y = dict_value_f32(&bounds_dict, "Y")?;
    let width = dict_value_f32(&bounds_dict, "Width")?;
    let height = dict_value_f32(&bounds_dict, "Height")?;
    let origin = CGPoint::new(x as f64, y as f64);
    let size = CGSize::new(width as f64, height as f64);
    Some(CGRect::new(&origin, &size))
}

/// Read an f32 from a CoreGraphics window dictionary entry.
fn dict_value_f32(dict: &CFDictionary<CFString, CFType>, name: &'static str) -> Option<f32> {
    let key = CFString::from_static_string(name);
    dict.find(&key)
        .and_then(|value: ItemRef<CFType>| value.downcast::<CFNumber>())
        .and_then(|value: CFNumber| value.to_f64())
        .map(|value| value as f32)
}
