// Unified geometry primitives and helpers.
// CGPoint/CGSize mirror CoreGraphics types (f64 fields) for AXValue interop.

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CGPoint {
    pub x: f64,
    pub y: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CGSize {
    pub width: f64,
    pub height: f64,
}

#[inline]
pub fn approx_eq_eps(a: f64, b: f64, eps: f64) -> bool {
    (a - b).abs() <= eps
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    #[inline]
    pub fn left(&self) -> f64 {
        self.x
    }
    #[inline]
    pub fn right(&self) -> f64 {
        self.x + self.w
    }
    #[inline]
    pub fn bottom(&self) -> f64 {
        self.y
    }
    #[inline]
    pub fn top(&self) -> f64 {
        self.y + self.h
    }
    #[inline]
    pub fn cx(&self) -> f64 {
        self.x + self.w / 2.0
    }
    #[inline]
    pub fn cy(&self) -> f64 {
        self.y + self.h / 2.0
    }
}

impl From<(CGPoint, CGSize)> for Rect {
    fn from(v: (CGPoint, CGSize)) -> Self {
        let (p, s) = v;
        Rect {
            x: p.x,
            y: p.y,
            w: s.width,
            h: s.height,
        }
    }
}

impl From<Rect> for (CGPoint, CGSize) {
    fn from(r: Rect) -> Self {
        (
            CGPoint { x: r.x, y: r.y },
            CGSize {
                width: r.w,
                height: r.h,
            },
        )
    }
}

#[inline]
pub fn approx_eq(a: f64, b: f64, eps: f64) -> bool {
    approx_eq_eps(a, b, eps)
}

#[inline]
pub fn rect_approx_eq(p1: CGPoint, s1: CGSize, p2: CGPoint, s2: CGSize, eps: f64) -> bool {
    approx_eq(p1.x, p2.x, eps)
        && approx_eq(p1.y, p2.y, eps)
        && approx_eq(s1.width, s2.width, eps)
        && approx_eq(s1.height, s2.height, eps)
}

#[inline]
pub fn rect_eq(p1: CGPoint, s1: CGSize, p2: CGPoint, s2: CGSize) -> bool {
    rect_approx_eq(p1, s1, p2, s2, 1.0)
}

#[inline]
pub fn point_in_rect(px: f64, py: f64, r: &Rect) -> bool {
    px >= r.left() && px <= r.right() && py >= r.bottom() && py <= r.top()
}

#[inline]
pub fn overlap_1d(a1: f64, a2: f64, b1: f64, b2: f64) -> f64 {
    let l = a1.max(b1);
    let r = a2.min(b2);
    (r - l).max(0.0)
}

#[inline]
pub fn same_row_by_overlap(a: &Rect, b: &Rect, ratio: f64) -> bool {
    let min_h = a.h.abs().min(b.h.abs());
    overlap_1d(a.bottom(), a.top(), b.bottom(), b.top()) >= ratio * min_h
}

#[inline]
pub fn same_col_by_overlap(a: &Rect, b: &Rect, ratio: f64) -> bool {
    let min_w = a.w.abs().min(b.w.abs());
    overlap_1d(a.left(), a.right(), b.left(), b.right()) >= ratio * min_w
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    Horizontal,
    Vertical,
}

#[inline]
pub fn center_distance_bias(a: &Rect, b: &Rect, axis: Axis) -> (f64, f64) {
    match axis {
        Axis::Horizontal => ((b.cx() - a.cx()).abs(), (b.cy() - a.cy()).abs()),
        Axis::Vertical => ((b.cy() - a.cy()).abs(), (b.cx() - a.cx()).abs()),
    }
}

// Grid helpers ----------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
pub fn grid_cell_rect(
    vf_x: f64,
    vf_y: f64,
    vf_w: f64,
    vf_h: f64,
    cols: u32,
    rows: u32,
    col: u32,
    row: u32,
) -> (f64, f64, f64, f64) {
    let c = cols.max(1) as f64;
    let r = rows.max(1) as f64;
    let tile_w = (vf_w / c).floor().max(1.0);
    let tile_h = (vf_h / r).floor().max(1.0);
    let rem_w = vf_w - tile_w * (cols as f64);
    let rem_h = vf_h - tile_h * (rows as f64);

    let x = vf_x + tile_w * (col as f64);
    let w = if col == cols.saturating_sub(1) {
        tile_w + rem_w
    } else {
        tile_w
    };
    let y = vf_y + tile_h * (row as f64);
    let h = if row == rows.saturating_sub(1) {
        tile_h + rem_h
    } else {
        tile_h
    };
    (x, y, w, h)
}

#[allow(clippy::too_many_arguments)]
pub fn grid_find_cell(
    vf_x: f64,
    vf_y: f64,
    vf_w: f64,
    vf_h: f64,
    cols: u32,
    rows: u32,
    pos: CGPoint,
    size: CGSize,
    eps: f64,
) -> Option<(u32, u32)> {
    for row in 0..rows {
        for col in 0..cols {
            let (x, y, w, h) = grid_cell_rect(vf_x, vf_y, vf_w, vf_h, cols, rows, col, row);
            if approx_eq(pos.x, x, eps)
                && approx_eq(pos.y, y, eps)
                && approx_eq(size.width, w, eps)
                && approx_eq(size.height, h, eps)
            {
                return Some((col, row));
            }
        }
    }
    None
}

// Corner helpers ---------------------------------------------------------------

#[inline]
pub fn corner_dir(corner: crate::ScreenCorner) -> (i32, i32) {
    match corner {
        crate::ScreenCorner::BottomRight => (1, 1),
        crate::ScreenCorner::BottomLeft => (-1, 1),
        crate::ScreenCorner::TopLeft => (-1, -1),
    }
}

#[inline]
pub fn overshoot_target(p: CGPoint, corner: crate::ScreenCorner, magnitude: f64) -> CGPoint {
    let (dx, dy) = corner_dir(corner);
    CGPoint {
        x: p.x + magnitude * (dx as f64),
        y: p.y + magnitude * (dy as f64),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approx_eq_works() {
        assert!(approx_eq(1.0, 1.0, 0.0));
        assert!(approx_eq(1.0, 1.000_5, 0.001));
        assert!(!approx_eq(1.0, 1.01, 0.001));
    }

    #[test]
    fn rect_edges_and_center() {
        let r = Rect {
            x: 10.0,
            y: 20.0,
            w: 30.0,
            h: 40.0,
        };
        assert_eq!(r.left(), 10.0);
        assert_eq!(r.right(), 40.0);
        assert_eq!(r.bottom(), 20.0);
        assert_eq!(r.top(), 60.0);
        assert_eq!(r.cx(), 25.0);
        assert_eq!(r.cy(), 40.0);
    }

    #[test]
    fn point_in_rect_inclusive() {
        let r = Rect {
            x: 0.0,
            y: 0.0,
            w: 10.0,
            h: 10.0,
        };
        assert!(point_in_rect(0.0, 0.0, &r));
        assert!(point_in_rect(10.0, 10.0, &r));
        assert!(point_in_rect(5.0, 1.0, &r));
        assert!(!point_in_rect(-0.1, 0.0, &r));
        assert!(!point_in_rect(0.0, 10.1, &r));
    }

    #[test]
    fn overlap_and_same_row_col() {
        let a = Rect {
            x: 0.0,
            y: 0.0,
            w: 50.0,
            h: 50.0,
        };
        let b = Rect {
            x: 60.0,
            y: 10.0,
            w: 50.0,
            h: 50.0,
        };
        // Y overlap is 40 over min height 50 => 0.8 ratio
        let ov_y = overlap_1d(a.bottom(), a.top(), b.bottom(), b.top());
        assert_eq!(ov_y, 40.0);
        assert!(same_row_by_overlap(&a, &b, 0.8));
        // X overlap is 0
        let ov_x = overlap_1d(a.left(), a.right(), b.left(), b.right());
        assert_eq!(ov_x, 0.0);
        assert!(!same_col_by_overlap(&a, &b, 0.8));
    }

    #[test]
    fn grid_cell_rect_corners_and_remainders() {
        let (vf_x, vf_y, vf_w, vf_h) = (0.0, 0.0, 100.0, 100.0);
        let (x0, y0, w0, h0) = grid_cell_rect(vf_x, vf_y, vf_w, vf_h, 3, 2, 0, 0);
        assert_eq!((x0, y0, w0, h0), (0.0, 0.0, 33.0, 50.0));
        let (x1, y1, w1, h1) = grid_cell_rect(vf_x, vf_y, vf_w, vf_h, 3, 2, 2, 0);
        assert_eq!((x1, y1, w1, h1), (66.0, 0.0, 34.0, 50.0));
        let (_x2, y2, _w2, _h2) = grid_cell_rect(vf_x, vf_y, vf_w, vf_h, 3, 2, 0, 1);
        assert_eq!(y2, 50.0);
    }

    #[test]
    fn grid_find_cell_match() {
        let (vf_x, vf_y, vf_w, vf_h) = (0.0, 0.0, 100.0, 80.0);
        let (x, y, w, h) = grid_cell_rect(vf_x, vf_y, vf_w, vf_h, 4, 2, 3, 1);
        let pos = CGPoint { x, y };
        let size = CGSize {
            width: w,
            height: h,
        };
        let cell = grid_find_cell(vf_x, vf_y, vf_w, vf_h, 4, 2, pos, size, 0.5);
        assert_eq!(cell, Some((3, 1)));
    }

    #[test]
    fn corner_helpers() {
        let p = CGPoint { x: 10.0, y: 10.0 };
        let q = overshoot_target(p, crate::ScreenCorner::BottomLeft, 100.0);
        assert_eq!(q.x, -90.0);
        assert_eq!(q.y, 110.0);
        assert_eq!(corner_dir(crate::ScreenCorner::TopLeft), (-1, -1));
        assert_eq!(corner_dir(crate::ScreenCorner::BottomRight), (1, 1));
    }
}
