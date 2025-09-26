//! Command-line interface definitions for smoketest.

use clap::{Parser, Subcommand, ValueEnum};
use logging::LogArgs;

use crate::{
    config,
    suite::{CaseRunOpts, case_by_alias},
};

/// Command-line interface arguments for the smoketest binary.
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

    /// Continue running the full `all` suite even if individual tests fail
    #[arg(long)]
    pub no_fail_fast: bool,

    /// Optional short info text to show in the warning overlay under the test title
    #[arg(long)]
    pub info: Option<String>,

    /// Default duration for repeat tests in milliseconds
    #[arg(long, default_value_t = config::DEFAULTS.duration_ms)]
    pub duration: u64,

    /// Default timeout for UI readiness and waits in milliseconds
    #[arg(long, default_value_t = config::DEFAULTS.timeout_ms)]
    pub timeout: u64,

    /// Repeat the selected tests this many times
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u32).range(1..))]
    pub repeat: u32,

    /// Which subcommand to run
    #[command(subcommand)]
    pub command: Commands,
}

/// Named tests that can be run in sequence via `seq`.
#[derive(Copy, Clone, Debug, ValueEnum)]
pub enum SeqTest {
    /// Relay repeat performance
    #[value(name = "repeat-relay")]
    RepeatRelay,
    /// Shell repeat performance
    #[value(name = "repeat-shell")]
    RepeatShell,
    /// Volume repeat performance
    #[value(name = "repeat-volume")]
    RepeatVolume,
    /// Focus tracking flow
    #[value(name = "focus.tracking")]
    FocusTracking,
    /// Focus navigation flow
    #[value(name = "focus.nav")]
    FocusNav,
    /// Raise operation
    #[value(name = "raise")]
    Raise,
    /// Hide toggle behavior
    #[value(name = "hide.toggle.roundtrip")]
    HideToggle,
    /// Grid placement cycle
    #[value(name = "place.grid.cycle")]
    PlaceGrid,
    /// Async placement behavior
    #[value(name = "place.async.delay")]
    PlaceAsync,
    /// Animated placement behavior
    #[value(name = "place.animated.tween")]
    PlaceAnimated,
    /// Terminal placement guard
    #[value(name = "place.term.anchor")]
    PlaceTerm,
    /// Placement with increments
    #[value(name = "place.increments.anchor")]
    PlaceIncrements,
    /// Move with minimum size constraint
    #[value(name = "place.move.min")]
    PlaceMoveMin,
    /// Move with non-resizable constraint
    #[value(name = "place.move.nonresizable")]
    PlaceMoveNonresizable,
    /// Placement skip behavior
    #[value(name = "place.skip.nonmovable")]
    PlaceSkip,
    /// Fake placement harness (no GUI required)
    #[value(name = "place.fake.adapter")]
    PlaceFake,
    /// Minimized placement restore
    #[value(name = "place.minimized.defer")]
    PlaceMinimized,
    /// Zoomed placement normalize
    #[value(name = "place.zoomed.normalize")]
    PlaceZoomed,
    /// Flexible placement default path
    #[value(name = "place.flex.default")]
    PlaceFlex,
    /// Shrink-move-grow placement
    #[value(name = "place.flex.smg")]
    PlaceFlexSmg,
    /// Size->pos placement fallback
    #[value(name = "place.flex.force_size_pos")]
    PlaceFlexFallback,
    /// Fullscreen toggle behavior
    #[value(name = "fullscreen.toggle.nonnative")]
    Fullscreen,
    /// Full UI smoke
    #[value(name = "ui.demo.standard")]
    Ui,
    /// Mini UI smoke
    #[value(name = "ui.demo.mini")]
    Minui,
    /// Simulated multi-space adoption/performance check
    #[value(name = "world.spaces.adoption")]
    WorldSpaces,
    /// World status surface check
    #[value(name = "world.status.permissions")]
    WorldStatus,
    /// World AX focus props
    #[value(name = "world.ax.focus_props")]
    WorldAx,
}

impl SeqTest {
    /// Return the registry slug corresponding to this sequence entry.
    pub fn slug(self) -> &'static str {
        let alias_value = self
            .to_possible_value()
            .expect("seq test must expose a clap alias");
        let alias = alias_value.get_name();
        case_by_alias(alias)
            .map(|entry| entry.name)
            .expect("seq test alias must map to a registered case")
    }
}

/// CLI commands for the smoketest runner.
#[derive(Subcommand, Debug)]
#[allow(clippy::large_enum_variant)]
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
    /// Example: smoketest seq repeat-relay focus.tracking ui.demo.standard
    #[command(name = "seq")]
    Seq {
        /// One or more test names to run in order
        #[arg(value_enum, value_name = "TEST", num_args = 1..)]
        tests: Vec<SeqTest>,
    },

    /// Verify raise(action) by switching focus between two titled windows
    Raise,

    /// Verify focus(dir) by navigating between arranged helper windows
    #[command(name = "focus.nav")]
    FocusNav,

    /// Verify focus tracking by activating a test window
    #[command(name = "focus.tracking")]
    Focus,

    /// Verify hide(toggle)/on/off by moving a helper window off/on screen right
    #[command(name = "hide.toggle.roundtrip")]
    Hide,

    /// Verify window placement into a grid by cycling a helper window through all cells
    #[command(name = "place.grid.cycle")]
    Place,

    /// Exercise placement flows with a fake AX adapter (no GUI required)
    #[command(name = "place.fake.adapter")]
    PlaceFake,

    /// Verify placement convergence when the target app applies geometry with a small delay
    #[command(name = "place.async.delay")]
    PlaceAsync,

    /// Verify placement while the target window animates to the requested frame
    #[command(name = "place.animated.tween")]
    PlaceAnimated,

    /// Exercise placement under terminal-style resize increments with a
    /// timeline check that ensures we never thrash position after origin is
    /// correct (terminal guard).
    #[command(name = "place.term.anchor")]
    PlaceTerm,

    /// Verify placement when the app enforces discrete resize increments. This
    /// uses a helper that rounds all requested sizes to multiples of `(W,H)` and
    /// checks that anchored edges are flush to the grid.
    #[command(name = "place.increments.anchor")]
    PlaceIncrements,

    /// Verify placement after normalizing a minimized window
    #[command(name = "place.minimized.defer")]
    PlaceMinimized,

    /// Verify placement after normalizing a zoomed window
    #[command(name = "place.zoomed.normalize")]
    PlaceZoomed,

    /// Repro for move-with-grid when minimum height exceeds cell size
    #[command(name = "place.move.min")]
    PlaceMoveMin,

    /// Repro for move-with-grid when window is non-resizable
    #[command(name = "place.move.nonresizable")]
    PlaceMoveNonresizable,

    /// Flexible placement harness for Stage-8 variants (direct mac-winops calls)
    #[command(name = "place-flex")]
    PlaceFlex {
        /// Force size->pos fallback even if pos->size succeeds
        #[arg(long, default_value_t = false)]
        force_size_pos: bool,
        /// Force shrink->move->grow fallback even if dual-order attempts succeed (smoketest only)
        #[arg(long, default_value_t = false)]
        force_shrink_move_grow: bool,
    },

    /// Convenience: exercise size->pos fallback path explicitly
    #[command(name = "place.flex.force_size_pos")]
    PlaceFallback,

    /// Focused test: exercise shrink->move->grow fallback deterministically
    #[command(name = "place.flex.smg")]
    PlaceSmg,

    /// Internal helper: create a foreground window with a title for focus testing
    #[command(hide = true, name = "focus-winhelper")]
    FocusWinHelper {
        /// Title to set on the helper window
        #[arg(long)]
        title: String,
        /// How long to keep the window alive (ms)
        #[arg(long, default_value_t = config::HELPER_WINDOW.default_lifetime_ms)]
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
        /// Optional minimum content size `(W, H)` enforced by the helper window.
        /// Simulates apps (e.g., browsers) that refuse to shrink below a floor.
        #[arg(long, value_names = ["W", "H"])]
        min_size: Option<Vec<f64>>,
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
        /// Make the helper non-resizable (removes NSWindowStyleMask::Resizable)
        #[arg(long, default_value_t = false)]
        non_resizable: bool,
        /// Attach a simple sheet to the helper window (AXRole=AXSheet)
        #[arg(long, default_value_t = false)]
        attach_sheet: bool,
    },

    /// Launch UI with test config and drive a short HUD + theme cycle
    #[command(name = "ui.demo.standard")]
    Ui,

    /// Take HUD-only screenshots for a theme
    // Screenshots extracted to separate tool: hotki-shots

    /// Launch UI in mini HUD mode and cycle themes
    #[command(name = "ui.demo.mini")]
    Minui,

    /// Control fullscreen on a helper window (non-native registry case)
    #[command(name = "fullscreen.toggle.nonnative")]
    Fullscreen,
    /// Query world status via RPC and verify basic invariants
    #[command(name = "world.status.permissions")]
    WorldStatus,
    /// Query AX props for the frontmost helper via WorldHandle
    #[command(name = "world.ax.focus_props")]
    WorldAx,
    /// Simulate multi-space navigation and verify adoption performance.
    #[command(name = "world.spaces.adoption")]
    WorldSpaces,
    // Preflight smoketest removed.
    /// Focused test: attempt placement on a non-movable window and assert skip
    #[command(name = "place.skip.nonmovable")]
    PlaceSkip,
}

impl Commands {
    /// Return the case slug and run options for a command.
    pub fn case_info(&self, fake_mode: bool) -> Option<(&'static str, CaseRunOpts)> {
        let default_opts = CaseRunOpts::default();
        let fake_opts = CaseRunOpts {
            warn_overlay: Some(false),
            fail_fast: Some(true),
        };

        let candidate = match self {
            Self::Relay => "repeat-relay",
            Self::Shell => "repeat-shell",
            Self::Volume => "repeat-volume",
            Self::Raise => "raise",
            Self::FocusNav => "focus.nav",
            Self::Focus => "focus.tracking",
            Self::Hide => "hide.toggle.roundtrip",
            Self::Place if fake_mode => "place.fake.adapter",
            Self::Place => "place.grid.cycle",
            Self::PlaceFake => "place.fake.adapter",
            Self::PlaceAsync => "place.async.delay",
            Self::PlaceAnimated => "place.animated.tween",
            Self::PlaceTerm => "place.term.anchor",
            Self::PlaceIncrements => "place.increments.anchor",
            Self::PlaceMoveMin => "place.move.min",
            Self::PlaceMoveNonresizable => "place.move.nonresizable",
            Self::PlaceMinimized => "place.minimized.defer",
            Self::PlaceZoomed => "place.zoomed.normalize",
            Self::PlaceFlex {
                force_size_pos,
                force_shrink_move_grow,
            } => {
                if *force_shrink_move_grow {
                    "place.flex.smg"
                } else if *force_size_pos {
                    "place.flex.force_size_pos"
                } else {
                    "place.flex.default"
                }
            }
            Self::PlaceFallback => "place.flex.force_size_pos",
            Self::PlaceSmg => "place.flex.smg",
            Self::PlaceSkip => "place.skip.nonmovable",
            Self::Ui => "ui.demo.standard",
            Self::Minui => "ui.demo.mini",
            Self::Fullscreen => "fullscreen.toggle.nonnative",
            Self::WorldStatus => "world.status.permissions",
            Self::WorldAx => "world.ax.focus_props",
            Self::WorldSpaces => "world.spaces.adoption",
            Self::All | Self::Seq { .. } | Self::FocusWinHelper { .. } => return None,
        };

        let opts = if fake_mode && candidate.starts_with("place.") {
            fake_opts
        } else {
            default_opts
        };

        let entry = case_by_alias(candidate)?;
        Some((entry.name, opts))
    }
}
