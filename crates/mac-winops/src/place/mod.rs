use crate::geom;

mod apply;
mod common;
mod fallback;
mod normalize;
mod ops_focused;
mod ops_id;
mod ops_move;

pub use common::PlaceAttemptOptions;
pub use ops_focused::{place_grid_focused, place_grid_focused_opts};
pub(crate) use ops_id::place_grid;
pub(crate) use ops_move::place_move_grid;

#[inline]
fn grid_guess_cell_by_pos(
    vf_x: f64,
    vf_y: f64,
    vf_w: f64,
    vf_h: f64,
    cols: u32,
    rows: u32,
    pos: geom::Point,
) -> (u32, u32) {
    let cols_f = cols.max(1) as f64;
    let rows_f = rows.max(1) as f64;
    let tile_w = (vf_w / cols_f).floor().max(1.0);
    let tile_h = (vf_h / rows_f).floor().max(1.0);
    let mut c = ((pos.x - vf_x) / tile_w).floor() as i64;
    let mut r = ((pos.y - vf_y) / tile_h).floor() as i64;
    if c < 0 {
        c = 0;
    }
    if r < 0 {
        r = 0;
    }
    if c as u32 >= cols {
        c = cols.saturating_sub(1) as i64;
    }
    if r as u32 >= rows {
        r = rows.saturating_sub(1) as i64;
    }
    (c as u32, r as u32)
}
