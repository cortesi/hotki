use std::cmp::Ordering;

use core_graphics::{display::CGDisplay, geometry::CGRect};

use crate::{DisplayFrame, DisplaysSnapshot, state::display_frame};

pub(crate) fn gather_displays() -> DisplaysSnapshot {
    let mut frames = Vec::new();
    let mut global_top = 0.0_f32;
    let main = CGDisplay::main();
    let main_bounds: CGRect = main.bounds();
    let mut active = None;

    if let Ok(active_ids) = CGDisplay::active_displays() {
        for id in active_ids {
            let display = CGDisplay::new(id);
            let bounds: CGRect = display.bounds();
            let frame = display_frame(
                display.id,
                bounds.origin.x as f32,
                bounds.origin.y as f32,
                bounds.size.width as f32,
                bounds.size.height as f32,
            );
            if display.id == main.id {
                active = Some(frame);
            }
            global_top = global_top.max(frame.top());
            frames.push(frame);
        }
    }

    if active.is_none() {
        let fallback = display_frame(
            main.id,
            main_bounds.origin.x as f32,
            main_bounds.origin.y as f32,
            main_bounds.size.width as f32,
            main_bounds.size.height as f32,
        );
        global_top = global_top.max(fallback.top());
        active = Some(fallback);
    }

    DisplaysSnapshot {
        global_top,
        active,
        displays: frames,
    }
}

pub(crate) fn display_for_rect(bounds: &CGRect, displays: &[DisplayFrame]) -> Option<u32> {
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

fn point_in_display(display: &DisplayFrame, x: f32, y: f32) -> bool {
    x >= display.x
        && x <= display.x + display.width
        && y >= display.y
        && y <= display.y + display.height
}

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

#[cfg(test)]
mod tests {
    use core_graphics::geometry::{CGPoint, CGSize};

    use super::*;

    fn rect(x: f64, y: f64, width: f64, height: f64) -> CGRect {
        CGRect::new(&CGPoint::new(x, y), &CGSize::new(width, height))
    }

    #[test]
    fn display_for_rect_prefers_center_containing_display() {
        let displays = [
            display_frame(1, 0.0, 0.0, 100.0, 100.0),
            display_frame(2, 100.0, 0.0, 100.0, 100.0),
        ];

        assert_eq!(
            display_for_rect(&rect(120.0, 10.0, 30.0, 30.0), &displays),
            Some(2)
        );
    }

    #[test]
    fn display_for_rect_falls_back_to_largest_overlap() {
        let displays = [
            display_frame(1, 0.0, 0.0, 100.0, 100.0),
            display_frame(2, 100.0, 0.0, 100.0, 100.0),
        ];

        assert_eq!(
            display_for_rect(&rect(70.0, 10.0, 40.0, 30.0), &displays),
            Some(1)
        );
    }
}
