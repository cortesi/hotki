//! Command-line interface definitions for smoketest.

use clap::{Parser, Subcommand, ValueEnum};
use logging::LogArgs;

use crate::config;

#[derive(Parser, Debug)]
#[command(name = "smoketest", about = "Hotki smoketest tool", version)]
pub struct Cli {
    /// Logging controls
    #[command(flatten)]
    pub log: LogArgs,

    /// Suppress headings and non-error output (used by orchestrated runs)
    #[arg(long)]
    pub quiet: bool,

    /// Disable the hands-off keyboard warning overlay
    #[arg(long)]
    pub no_warn: bool,

    /// Optional short info text to show in the warning overlay under the test title
    #[arg(long)]
    pub info: Option<String>,

    /// Default duration for repeat tests in milliseconds
    #[arg(long, default_value_t = config::DEFAULT_DURATION_MS)]
    pub duration: u64,

    /// Default timeout for UI readiness and waits in milliseconds
    #[arg(long, default_value_t = config::DEFAULT_TIMEOUT_MS)]
    pub timeout: u64,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
#[value(rename_all = "kebab-case")]
pub enum SeqTest {
    RepeatRelay,
    RepeatShell,
    RepeatVolume,
    Focus,
    Raise,
    Hide,
    Place,
    PlaceAsync,
    PlaceAnimated,
    Fullscreen,
    Ui,
    Minui,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
pub enum FsState {
    Toggle,
    On,
    Off,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Measure relay repeats posted to the focused window
    #[command(name = "repeat-relay")]
    Relay,

    /// Measure number of shell invocations when repeating a shell command
    #[command(name = "repeat-shell")]
    Shell,

    /// Measure repeats by incrementing system volume from zero
    #[command(name = "repeat-volume")]
    Volume,

    /// Run all smoketests (repeats + UI demos)
    #[command(name = "all")]
    All,

    /// Run a sequence of smoketests in order
    ///
    /// Example: smoketest seq repeat-relay focus-tracking ui
    #[command(name = "seq")]
    Seq {
        /// One or more test names to run in order
        #[arg(value_enum, value_name = "TEST", num_args = 1..)]
        tests: Vec<SeqTest>,
    },

    /// Verify raise(action) by switching focus between two titled windows
    Raise,

    /// Verify focus(dir) by navigating between arranged helper windows
    #[command(name = "focus-nav")]
    FocusNav,

    /// Verify focus tracking by activating a test window
    #[command(name = "focus-tracking")]
    Focus,

    /// Verify hide(toggle)/on/off by moving a helper window off/on screen right
    Hide,

    /// Verify window placement into a grid by cycling a helper window through all cells
    Place,

    /// Verify placement convergence when the target app applies geometry with a small delay
    #[command(name = "place-async")]
    PlaceAsync,

    /// Verify placement while the target window animates to the requested frame
    #[command(name = "place-animated")]
    PlaceAnimated,

    /// Verify placement when the app enforces discrete resize increments. This
    /// uses a helper that rounds all requested sizes to multiples of `(W,H)` and
    /// checks that anchored edges are flush to the grid.
    #[command(name = "place-increments")]
    PlaceIncrements,

    /// Verify placement after normalizing a minimized window
    #[command(name = "place-minimized")]
    PlaceMinimized,

    /// Verify placement after normalizing a zoomed window
    #[command(name = "place-zoomed")]
    PlaceZoomed,

    /// Flexible placement harness for Stage-8 variants (direct mac-winops calls)
    #[command(name = "place-flex")]
    PlaceFlex {
        /// Grid columns
        #[arg(long, default_value_t = config::PLACE_COLS)]
        cols: u32,
        /// Grid rows
        #[arg(long, default_value_t = config::PLACE_ROWS)]
        rows: u32,
        /// Target column (0-based)
        #[arg(long, default_value_t = 0)]
        col: u32,
        /// Target row (0-based)
        #[arg(long, default_value_t = 0)]
        row: u32,
        /// Force size->pos fallback even if pos->size succeeds
        #[arg(long, default_value_t = false)]
        force_size_pos: bool,
        /// Disable size->pos fallback; only attempt pos->size
        #[arg(long, default_value_t = false)]
        pos_first_only: bool,
        /// Force shrink->move->grow fallback even if dual-order attempts succeed (smoketest only)
        #[arg(long, default_value_t = false)]
        force_shrink_move_grow: bool,
    },

    /// Convenience: exercise size->pos fallback path explicitly
    #[command(name = "place-fallback")]
    PlaceFallback,

    /// Focused test: exercise shrink->move->grow fallback deterministically
    #[command(name = "place-smg")]
    PlaceSmg,

    /// Internal helper: create a foreground window with a title for focus testing
    #[command(hide = true, name = "focus-winhelper")]
    FocusWinHelper {
        /// Title to set on the helper window
        #[arg(long)]
        title: String,
        /// How long to keep the window alive (ms)
        #[arg(long, default_value_t = config::DEFAULT_HELPER_WINDOW_TIME_MS)]
        time: u64,
        /// Optional delay to apply when the system attempts to change the
        /// window frame (position/size). When set, the helper will briefly
        /// revert to the previous frame and only apply the new frame after
        /// `delay-setframe-ms` has elapsed. This simulates apps that apply
        /// geometry asynchronously.
        #[arg(long, value_name = "MS")]
        delay_setframe_ms: Option<u64>,
        /// Explicit delayed-apply: after `delay-apply-ms`, set the window
        /// frame to `apply-target` regardless of prior changes. This avoids
        /// relying on event delivery for simulation.
        #[arg(long, value_name = "MS")]
        delay_apply_ms: Option<u64>,
        /// Animate frame changes to the latest requested target over this duration
        /// (milliseconds). When set, the helper intercepts setFrame attempts and
        /// tweens from the last-known frame to the most recent desired frame.
        /// Useful to simulate apps that animate their own geometry updates.
        #[arg(long, value_name = "MS")]
        tween_ms: Option<u64>,
        /// Target `(x y w h)` for delayed apply (AppKit logical coords)
        #[arg(long, value_names = ["X", "Y", "W", "H"])]
        apply_target: Option<Vec<f64>>,
        /// Grid `(cols rows col row)` for delayed apply; helper computes
        /// target rect on its current screen's visible frame
        #[arg(long, value_names = ["COLS", "ROWS", "COL", "ROW"])]
        apply_grid: Option<Vec<u32>>,
        /// Optional 2x2 grid slot: 1=tl, 2=tr, 3=bl, 4=br
        #[arg(long)]
        slot: Option<u8>,
        /// Optional explicit grid placement (cols, rows, col, row)
        #[arg(long, value_names = ["COLS", "ROWS", "COL", "ROW"])]
        grid: Option<Vec<u32>>,
        /// Optional size (width, height)
        #[arg(long, value_names = ["W", "H"])]
        size: Option<Vec<f64>>,
        /// Optional position (x, y) in AppKit logical coords
        #[arg(long, value_names = ["X", "Y"])]
        pos: Option<Vec<f64>>,
        /// Optional label text to render centered inside the window
        #[arg(long)]
        label_text: Option<String>,
        /// Optional step size for rounding requested window sizes to the nearest
        /// multiples `(W, H)`. Simulates terminal-style resize increments.
        #[arg(long, value_names = ["W", "H"])]
        step_size: Option<Vec<f64>>,
        /// Start the helper window minimized (miniaturized)
        #[arg(long, default_value_t = false)]
        start_minimized: bool,
        /// Start the helper window zoomed (macOS 'zoom' state)
        #[arg(long, default_value_t = false)]
        start_zoomed: bool,
        /// Make the helper non-movable (sets NSWindow.movable=false)
        #[arg(long, default_value_t = false)]
        panel_nonmovable: bool,
        /// Attach a simple sheet to the helper window (AXRole=AXSheet)
        #[arg(long, default_value_t = false)]
        attach_sheet: bool,
    },

    /// Internal helper: show a borderless, always-on-top hands-off overlay (until killed)
    #[command(hide = true, name = "warn-overlay")]
    WarnOverlay {
        /// Optional path from which the overlay reads status text to display
        #[arg(long)]
        status_path: Option<std::path::PathBuf>,
        /// Optional path from which the overlay reads info text to display
        #[arg(long)]
        info_path: Option<std::path::PathBuf>,
    },

    /// Launch UI with test config and drive a short HUD + theme cycle
    Ui,

    /// Take HUD-only screenshots for a theme
    // Screenshots extracted to separate tool: hotki-shots

    /// Launch UI in mini HUD mode and cycle themes
    Minui,

    /// Control fullscreen on a helper window (toggle/on/off; native or non-native)
    Fullscreen {
        /// Desired state (toggle/on/off)
        #[arg(long, value_enum, default_value_t = FsState::Toggle)]
        state: FsState,
        /// Use native system fullscreen instead of non-native
        #[arg(long, default_value_t = false)]
        native: bool,
    },
    /// Query world status via RPC and verify basic invariants
    #[command(name = "world-status")]
    WorldStatus,
    /// Query AX props for the frontmost helper via WorldHandle
    #[command(name = "world-ax")]
    WorldAx,
    // Preflight smoketest removed.
    /// Focused test: attempt placement on a non-movable window and assert skip
    #[command(name = "place-skip")]
    PlaceSkip,
}
