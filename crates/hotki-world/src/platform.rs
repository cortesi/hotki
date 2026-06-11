use core_foundation::{
    array::CFArray,
    base::{CFType, TCFType},
    dictionary::CFDictionary,
    number::CFNumber,
    string::CFString,
};
use core_graphics::{
    geometry::{CGPoint, CGRect, CGSize},
    window::{
        copy_window_info, kCGNullWindowID, kCGWindowBounds, kCGWindowLayer,
        kCGWindowListExcludeDesktopElements, kCGWindowListOptionOnScreenOnly, kCGWindowName,
        kCGWindowNumber, kCGWindowOwnerName, kCGWindowOwnerPID,
    },
};
use permissions::{accessibility_ok, input_monitoring_ok, screen_recording_ok};

use crate::{
    Capabilities, DisplayFrame, DisplaysSnapshot,
    geometry::{display_for_rect, gather_displays},
};

#[derive(Clone, Debug, Default)]
pub(crate) struct PlatformWindow {
    pub(crate) app: String,
    pub(crate) title: String,
    pub(crate) pid: i32,
    pub(crate) id: u32,
    pub(crate) display_id: Option<u32>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct PlatformSnapshot {
    pub(crate) windows: Vec<PlatformWindow>,
    pub(crate) focused: Option<PlatformWindow>,
    pub(crate) displays: DisplaysSnapshot,
    pub(crate) capabilities: Capabilities,
}

pub(crate) fn capture_platform_snapshot() -> PlatformSnapshot {
    let capabilities = Capabilities {
        accessibility: accessibility_ok().into(),
        input_monitoring: input_monitoring_ok().into(),
        screen_recording: screen_recording_ok().into(),
    };

    let mut displays = gather_displays();
    let focused = active_window(&displays.displays);
    if let Some(ref window) = focused
        && let Some(active_id) = window.display_id
    {
        displays.active = displays
            .displays
            .iter()
            .find(|display| display.id == active_id)
            .copied()
            .or(displays.active);
    }
    if displays.active.is_none() {
        displays.active = displays.displays.first().copied();
    }

    let windows = focused.iter().cloned().collect();

    PlatformSnapshot {
        windows,
        focused,
        displays,
        capabilities,
    }
}

fn active_window(displays: &[DisplayFrame]) -> Option<PlatformWindow> {
    let options = kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements;
    let arr: CFArray = copy_window_info(options, kCGNullWindowID)?;
    // SAFETY: CoreGraphics exposes these constants as process-lifetime CFStringRefs.
    let key_layer = unsafe { CFString::wrap_under_get_rule(kCGWindowLayer) };
    // SAFETY: CoreGraphics exposes these constants as process-lifetime CFStringRefs.
    let key_owner_pid = unsafe { CFString::wrap_under_get_rule(kCGWindowOwnerPID) };
    // SAFETY: CoreGraphics exposes these constants as process-lifetime CFStringRefs.
    let key_owner_name = unsafe { CFString::wrap_under_get_rule(kCGWindowOwnerName) };
    // SAFETY: CoreGraphics exposes these constants as process-lifetime CFStringRefs.
    let key_name = unsafe { CFString::wrap_under_get_rule(kCGWindowName) };
    // SAFETY: CoreGraphics exposes these constants as process-lifetime CFStringRefs.
    let key_number = unsafe { CFString::wrap_under_get_rule(kCGWindowNumber) };
    // SAFETY: CoreGraphics exposes these constants as process-lifetime CFStringRefs.
    let key_bounds = unsafe { CFString::wrap_under_get_rule(kCGWindowBounds) };

    for raw in arr.iter() {
        let dict_ptr = *raw;
        // SAFETY: `copy_window_info` returns an array of window dictionaries. Each element is
        // retained by the array for the duration of this loop, and we only wrap it for reads.
        let dict: CFDictionary<CFString, CFType> =
            unsafe { CFDictionary::wrap_under_get_rule(dict_ptr as _) };
        let layer = dict_value_i32(&dict, &key_layer).unwrap_or(1);
        if layer != 0 {
            continue;
        }
        let Some(pid) = dict_value_i32(&dict, &key_owner_pid) else {
            continue;
        };
        let id = dict_value_u32(&dict, &key_number).unwrap_or(0);
        let app = dict_value_string(&dict, &key_owner_name).unwrap_or_default();
        let title = dict_value_string(&dict, &key_name).unwrap_or_default();
        let display_id = dict_value_rect(&dict, &key_bounds)
            .as_ref()
            .and_then(|rect| display_for_rect(rect, displays));
        return Some(PlatformWindow {
            app,
            title,
            pid,
            id,
            display_id,
        });
    }

    None
}

fn dict_value_string(dict: &CFDictionary<CFString, CFType>, key: &CFString) -> Option<String> {
    dict.find(key)
        .and_then(|value| value.downcast::<CFString>())
        .map(|value| value.to_string())
}

fn dict_value_i32(dict: &CFDictionary<CFString, CFType>, key: &CFString) -> Option<i32> {
    dict.find(key)
        .and_then(|value| value.downcast::<CFNumber>())
        .and_then(|number| number.to_i64())
        .map(|number| number as i32)
}

fn dict_value_u32(dict: &CFDictionary<CFString, CFType>, key: &CFString) -> Option<u32> {
    dict_value_i32(dict, key).map(|value| value as u32)
}

fn dict_value_rect(dict: &CFDictionary<CFString, CFType>, key: &CFString) -> Option<CGRect> {
    // SAFETY: CoreGraphics stores the bounds value as a dictionary for `kCGWindowBounds`.
    // The value is owned by `dict`, which remains alive while we read its numeric fields.
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

fn dict_value_f32(dict: &CFDictionary<CFString, CFType>, name: &'static str) -> Option<f32> {
    let key = CFString::from_static_string(name);
    dict.find(&key)
        .and_then(|value| value.downcast::<CFNumber>())
        .and_then(|number| number.to_f64())
        .map(|value| value as f32)
}
