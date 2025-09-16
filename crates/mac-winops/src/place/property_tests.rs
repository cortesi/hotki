use proptest::prelude::*;

use super::{
    common::{VERIFY_EPS, clamp_flags},
    grid_guess_cell_by_pos,
};
use crate::geom::{self, Point, Rect};

fn rect_strategy() -> impl Strategy<Value = Rect> {
    (
        -1000.0f64..1000.0,
        -1000.0f64..1000.0,
        10.0f64..1500.0,
        10.0f64..1500.0,
    )
        .prop_map(|(x, y, w, h)| Rect::new(x, y, w, h))
}

proptest! {
    #[test]
    fn grid_guess_stays_within_bounds(
        vf_x in -2000.0f64..2000.0,
        vf_y in -2000.0f64..2000.0,
        vf_w in 50.0f64..3000.0,
        vf_h in 50.0f64..3000.0,
        cols in 1u32..16,
        rows in 1u32..16,
        pos_x in -4000.0f64..4000.0,
        pos_y in -4000.0f64..4000.0,
    ) {
        let (col, row) = grid_guess_cell_by_pos(
            vf_x,
            vf_y,
            vf_w,
            vf_h,
            cols,
            rows,
            Point { x: pos_x, y: pos_y },
        );
        prop_assert!(col < cols);
        prop_assert!(row < rows);
    }
}

proptest! {
    #[test]
    fn clamp_flags_matches_component_checks(
        vf in rect_strategy(),
        got in rect_strategy(),
        eps in 0.5f64..(VERIFY_EPS * 4.0),
    ) {
        let flags = clamp_flags(&got, &vf, eps);
        prop_assert_eq!(flags.left, geom::approx_eq(got.left(), vf.left(), eps));
        prop_assert_eq!(flags.right, geom::approx_eq(got.right(), vf.right(), eps));
        prop_assert_eq!(flags.bottom, geom::approx_eq(got.bottom(), vf.bottom(), eps));
        prop_assert_eq!(flags.top, geom::approx_eq(got.top(), vf.top(), eps));
        prop_assert_eq!(flags.any(), flags.left || flags.right || flags.bottom || flags.top);
    }
}
