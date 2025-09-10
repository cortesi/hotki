use thiserror::Error;

/// Error type for keymode state handling
#[derive(Debug, Error)]
#[allow(missing_docs)]
pub enum KeymodeError {
    /// `place(grid(x, y), at(ix, iy))` coordinates out of range
    #[error(
        "place(): at() out of range: got ({ix}, {iy}) for grid ({gx} x {gy})\n  Valid x: 0..{max_x}  |  Valid y: 0..{max_y}"
    )]
    PlaceAtOutOfRange {
        ix: u32,
        iy: u32,
        gx: u32,
        gy: u32,
        max_x: u32,
        max_y: u32,
    },

    /// `raise()` requires at least one of `app` or `title`
    #[error("raise(): at least one of app or title must be provided")]
    RaiseMissingAppOrTitle,

    /// Invalid relay keyspec string
    #[error("Invalid relay keyspec '{spec}'")]
    InvalidRelayKeyspec { spec: String },
}
