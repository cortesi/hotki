// Unified geometry primitives and helpers.

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Size {
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
    /// Construct a new rectangle from position `(x, y)` and size `(w, h)`.
    #[inline]
    pub fn new(x: f64, y: f64, w: f64, h: f64) -> Self {
        Rect { x, y, w, h }
    }

    /// Find the `(col, row)` grid index inside `self` that matches a rectangle
    /// defined by `pos` and `size` within `eps`, if any.
    #[inline]
    pub fn grid_find_cell(
        &self,
        cols: u32,
        rows: u32,
        pos: Point,
        size: Size,
        eps: f64,
    ) -> Option<(u32, u32)> {
        for row in 0..rows {
            for col in 0..cols {
                let cell = self.grid_cell(cols, rows, col, row);
                if approx_eq(pos.x, cell.x, eps)
                    && approx_eq(pos.y, cell.y, eps)
                    && approx_eq(size.width, cell.w, eps)
                    && approx_eq(size.height, cell.h, eps)
                {
                    return Some((col, row));
                }
            }
        }
        None
    }

    /// Return true if vertical overlap between `self` and `b` is at least
    /// `ratio * min(h_self, h_b)`.
    #[inline]
    pub fn same_row_by_overlap(&self, b: &Rect, ratio: f64) -> bool {
        let min_h = self.h.abs().min(b.h.abs());
        overlap_1d(self.bottom(), self.top(), b.bottom(), b.top()) >= ratio * min_h
    }

    /// Return true if horizontal overlap between `self` and `b` is at least
    /// `ratio * min(w_self, w_b)`.
    #[inline]
    pub fn same_col_by_overlap(&self, b: &Rect, ratio: f64) -> bool {
        let min_w = self.w.abs().min(b.w.abs());
        overlap_1d(self.left(), self.right(), b.left(), b.right()) >= ratio * min_w
    }

    /// Return the `col,row` grid cell inside this rectangle, split into `cols x rows` tiles.
    /// The last column/row absorbs remainders. Dimensions are floored to whole points.
    #[inline]
    pub fn grid_cell(&self, cols: u32, rows: u32, col: u32, row: u32) -> Rect {
        let c = cols.max(1) as f64;
        let r = rows.max(1) as f64;
        let vf_w = self.w.max(1.0);
        let vf_h = self.h.max(1.0);
        let tile_w = (vf_w / c).floor().max(1.0);
        let tile_h = (vf_h / r).floor().max(1.0);
        let rem_w = vf_w - tile_w * (cols as f64);
        let rem_h = vf_h - tile_h * (rows as f64);

        let x = self.x + tile_w * (col as f64);
        let w = if col == cols.saturating_sub(1) {
            tile_w + rem_w
        } else {
            tile_w
        };
        let y = self.y + tile_h * (row as f64);
        let h = if row == rows.saturating_sub(1) {
            tile_h + rem_h
        } else {
            tile_h
        };
        Rect { x, y, w, h }
    }

    /// Left edge (minimum x).
    #[inline]
    pub fn left(&self) -> f64 {
        self.x
    }

    /// Right edge (`x + w`).
    #[inline]
    pub fn right(&self) -> f64 {
        self.x + self.w
    }

    /// Bottom edge (minimum y).
    #[inline]
    pub fn bottom(&self) -> f64 {
        self.y
    }

    /// Top edge (`y + h`).
    #[inline]
    pub fn top(&self) -> f64 {
        self.y + self.h
    }

    /// Center x coordinate.
    #[inline]
    pub fn cx(&self) -> f64 {
        self.x + self.w / 2.0
    }

    /// Center y coordinate.
    #[inline]
    pub fn cy(&self) -> f64 {
        self.y + self.h / 2.0
    }

    /// Center point of the rectangle.
    #[inline]
    pub fn center(&self) -> Point {
        Point {
            x: self.cx(),
            y: self.cy(),
        }
    }

    /// Inclusive point containment check.
    #[inline]
    pub fn contains(&self, px: f64, py: f64) -> bool {
        px >= self.left() && px <= self.right() && py >= self.bottom() && py <= self.top()
    }

    /// Absolute per‑field differences between two rects, returned as a `Rect`
    /// with `(dx, dy, dw, dh)` mapped to `(x, y, w, h)`.
    #[inline]
    pub fn diffs(&self, other: &Rect) -> Rect {
        Rect {
            x: (self.x - other.x).abs(),
            y: (self.y - other.y).abs(),
            w: (self.w - other.w).abs(),
            h: (self.h - other.h).abs(),
        }
    }

    /// Approximate equality within `eps` per component.
    #[inline]
    pub fn approx_eq(&self, other: &Rect, eps: f64) -> bool {
        approx_eq(self.x, other.x, eps)
            && approx_eq(self.y, other.y, eps)
            && approx_eq(self.w, other.w, eps)
            && approx_eq(self.h, other.h, eps)
    }

    /// Check whether all components of this diff-rect are within `eps`.
    /// Intended to be called on values produced by `Rect::diffs`.
    #[inline]
    pub fn within_diff_eps(&self, eps: f64) -> bool {
        self.x <= eps && self.y <= eps && self.w <= eps && self.h <= eps
    }
}

impl core::fmt::Display for Rect {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "({:.1},{:.1},{:.1},{:.1})",
            self.x, self.y, self.w, self.h
        )
    }
}

impl From<(Point, Size)> for Rect {
    fn from(v: (Point, Size)) -> Self {
        let (p, s) = v;
        Rect {
            x: p.x,
            y: p.y,
            w: s.width,
            h: s.height,
        }
    }
}

impl From<Rect> for (Point, Size) {
    fn from(r: Rect) -> Self {
        (
            Point { x: r.x, y: r.y },
            Size {
                width: r.w,
                height: r.h,
            },
        )
    }
}

impl core::fmt::Display for Point {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "({:.1},{:.1})", self.x, self.y)
    }
}

#[inline]
pub fn approx_eq(a: f64, b: f64, eps: f64) -> bool {
    approx_eq_eps(a, b, eps)
}

// within_eps moved to `Rect::within_eps`

#[inline]
pub fn overlap_1d(a1: f64, a2: f64, b1: f64, b2: f64) -> f64 {
    let l = a1.max(b1);
    let r = a2.min(b2);
    (r - l).max(0.0)
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

#[inline]
pub fn corner_dir(corner: crate::ScreenCorner) -> (i32, i32) {
    match corner {
        crate::ScreenCorner::BottomRight => (1, 1),
        crate::ScreenCorner::BottomLeft => (-1, 1),
        crate::ScreenCorner::TopLeft => (-1, -1),
    }
}

#[inline]
pub fn overshoot_target(p: Point, corner: crate::ScreenCorner, magnitude: f64) -> Point {
    let (dx, dy) = corner_dir(corner);
    Point {
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
        assert!(r.contains(0.0, 0.0));
        assert!(r.contains(10.0, 10.0));
        assert!(r.contains(5.0, 1.0));
        assert!(!r.contains(-0.1, 0.0));
        assert!(!r.contains(0.0, 10.1));
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
        assert!(a.same_row_by_overlap(&b, 0.8));
        // X overlap is 0
        let ov_x = overlap_1d(a.left(), a.right(), b.left(), b.right());
        assert_eq!(ov_x, 0.0);
        assert!(!a.same_col_by_overlap(&b, 0.8));
    }

    #[test]
    fn grid_cell_rect_corners_and_remainders() {
        let vf = Rect {
            x: 0.0,
            y: 0.0,
            w: 100.0,
            h: 100.0,
        };
        let r0 = vf.grid_cell(3, 2, 0, 0);
        assert_eq!((r0.x, r0.y, r0.w, r0.h), (0.0, 0.0, 33.0, 50.0));
        let r1 = vf.grid_cell(3, 2, 2, 0);
        assert_eq!((r1.x, r1.y, r1.w, r1.h), (66.0, 0.0, 34.0, 50.0));
        let r2 = vf.grid_cell(3, 2, 0, 1);
        assert_eq!(r2.y, 50.0);
    }

    #[test]
    fn grid_find_cell_match() {
        let vf = Rect {
            x: 0.0,
            y: 0.0,
            w: 100.0,
            h: 80.0,
        };
        let r = vf.grid_cell(4, 2, 3, 1);
        let pos = Point { x: r.x, y: r.y };
        let size = Size {
            width: r.w,
            height: r.h,
        };
        let cell = vf.grid_find_cell(4, 2, pos, size, 0.5);
        assert_eq!(cell, Some((3, 1)));
    }

    #[test]
    fn corner_helpers() {
        let p = Point { x: 10.0, y: 10.0 };
        let q = overshoot_target(p, crate::ScreenCorner::BottomLeft, 100.0);
        assert_eq!(q.x, -90.0);
        assert_eq!(q.y, 110.0);
        assert_eq!(corner_dir(crate::ScreenCorner::TopLeft), (-1, -1));
        assert_eq!(corner_dir(crate::ScreenCorner::BottomRight), (1, 1));
    }
}
