use objc2::rc::autoreleasepool;
use objc2_app_kit::{NSApplication, NSColor, NSEvent, NSScreen, NSWindowCollectionBehavior};
use objc2_foundation::MainThreadMarker;

pub fn apply_transparent_rounded(title_match: &str, radius: f64) -> Result<(), &'static str> {
    let Some(mtm) = MainThreadMarker::new() else {
        return Err("macOS window ops must be called on main thread");
    };
    let app = NSApplication::sharedApplication(mtm);
    let windows = app.windows();
    for w in windows.iter() {
        let title = w.title();
        let is_match = autoreleasepool(|pool| unsafe { title.to_str(pool) == title_match });
        if is_match {
            w.setOpaque(false);
            w.setHasShadow(false);
            let clear = unsafe { NSColor::clearColor() };
            w.setBackgroundColor(Some(&clear));
            if let Some(view) = w.contentView() {
                view.setWantsLayer(true);
                let layer_opt = unsafe { view.layer() };
                if let Some(layer) = layer_opt {
                    layer.setMasksToBounds(true);
                    layer.setCornerRadius(radius);
                }
            }
            let current_alpha = unsafe { w.alphaValue() };
            if (current_alpha - 1.0).abs() > 0.0001 {
                unsafe { w.setAlphaValue(1.0) };
            }
        }
    }
    Ok(())
}

pub fn get_window_frame(title_match: &str) -> Result<Option<(f32, f32, f32, f32)>, &'static str> {
    let Some(mtm) = MainThreadMarker::new() else {
        return Err("macOS window ops must be called on main thread");
    };
    let app = NSApplication::sharedApplication(mtm);
    let windows = app.windows();
    for w in windows.iter() {
        let title = w.title();
        let is_match = autoreleasepool(|pool| unsafe { title.to_str(pool) == title_match });
        if is_match {
            let fr = w.frame();
            return Ok(Some((
                fr.origin.x as f32,
                fr.origin.y as f32,
                fr.size.width as f32,
                fr.size.height as f32,
            )));
        }
    }
    Ok(None)
}

/// Set window to appear on all desktops/spaces
pub fn set_window_on_all_spaces(title_match: &str) -> Result<(), &'static str> {
    let Some(mtm) = MainThreadMarker::new() else {
        return Err("macOS window ops must be called on main thread");
    };
    let app = NSApplication::sharedApplication(mtm);
    let windows = app.windows();
    for w in windows.iter() {
        let title = w.title();
        let is_match = autoreleasepool(|pool| unsafe { title.to_str(pool) == title_match });
        if is_match {
            // Set collection behavior to appear on all spaces
            unsafe {
                w.setCollectionBehavior(NSWindowCollectionBehavior::CanJoinAllSpaces);
            }
        }
    }
    Ok(())
}

pub fn active_screen_frame() -> (f32, f32, f32, f32, f32) {
    unsafe {
        let mtm = MainThreadMarker::new().expect("Main thread required");
        let screens = NSScreen::screens(mtm);
        let mut global_top: f32 = f32::MIN;
        for scr in screens.iter() {
            let fr = scr.frame();
            let top = fr.origin.y as f32 + fr.size.height as f32;
            if top > global_top {
                global_top = top;
            }
        }

        let mouse = NSEvent::mouseLocation();
        let mut chosen_frame = None;
        for scr in screens.iter() {
            let fr = scr.frame();
            let x = fr.origin.x as f32;
            let y = fr.origin.y as f32;
            let w = fr.size.width as f32;
            let h = fr.size.height as f32;
            if mouse.x as f32 >= x
                && mouse.x as f32 <= x + w
                && mouse.y as f32 >= y
                && mouse.y as f32 <= y + h
            {
                chosen_frame = Some(fr);
                break;
            }
        }
        let fr = if let Some(fr) = chosen_frame {
            fr
        } else if let Some(main) = NSScreen::mainScreen(mtm) {
            main.frame()
        } else if let Some(first) = screens.iter().next() {
            first.frame()
        } else {
            NSScreen::screens(mtm).iter().next().unwrap().frame()
        };
        (
            fr.origin.x as f32,
            fr.origin.y as f32,
            fr.size.width as f32,
            fr.size.height as f32,
            global_top,
        )
    }
}
