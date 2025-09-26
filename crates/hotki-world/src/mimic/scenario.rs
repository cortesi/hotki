use std::{fmt, sync::Arc};

use crate::PlaceOptions;

/// Quirks that can be applied to a mimic window to simulate application-specific behaviour.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Quirk {
    /// Round raw Accessibility geometry while leaving CoreGraphics untouched.
    AxRounding,
    /// Delay authoritative geometry updates until the helper runloop has pumped once after
    /// observing a `set_frame` request.
    DelayApplyMove,
    /// Ignore direct move requests while the window is minimised.
    IgnoreMoveIfMinimized,
    /// Cycle focus between siblings before yielding the target when keeping the current frontmost
    /// window ahead of the mimic under test.
    RaiseCyclesToSibling,
}

impl fmt::Display for Quirk {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::AxRounding => "AxRounding",
            Self::DelayApplyMove => "DelayApplyMove",
            Self::IgnoreMoveIfMinimized => "IgnoreMoveIfMinimized",
            Self::RaiseCyclesToSibling => "RaiseCyclesToSibling",
        };
        write!(f, "{name}")
    }
}

/// Specification for a single mimic window within a scenario.
#[derive(Clone, Debug)]
pub struct MimicSpec {
    /// Stable slug identifying the scenario that owns this window.
    pub scenario_slug: Arc<str>,
    /// Short label (e.g. `primary`, `sibling`) used in artifacts and diagnostics.
    pub window_label: Arc<str>,
    /// NSWindow title assigned to the helper window.
    pub title: String,
    /// Placement tuning options supplied by smoketest scenarios.
    pub place: PlaceOptions,
    /// Quirk list applied to this helper.
    pub quirks: Vec<Quirk>,
    /// Runtime configuration that mirrors the legacy winhelper knobs.
    pub config: HelperConfig,
}

impl MimicSpec {
    /// Convenience for building a spec with default helper configuration.
    #[must_use]
    pub fn new(slug: Arc<str>, label: impl Into<Arc<str>>, title: impl Into<String>) -> Self {
        Self {
            scenario_slug: slug,
            window_label: label.into(),
            title: title.into(),
            place: PlaceOptions::default(),
            quirks: Vec::new(),
            config: HelperConfig::default(),
        }
    }

    /// Attach quirks to the spec.
    #[must_use]
    pub fn with_quirks(mut self, quirks: Vec<Quirk>) -> Self {
        self.quirks = quirks;
        self
    }

    /// Override placement options.
    #[must_use]
    pub fn with_place(mut self, place: PlaceOptions) -> Self {
        self.place = place;
        self
    }

    /// Override helper configuration.
    #[must_use]
    pub fn with_config(mut self, config: HelperConfig) -> Self {
        self.config = config;
        self
    }
}

/// Scenario container: a slug plus one or more mimic specs.
#[derive(Clone, Debug)]
pub struct MimicScenario {
    /// Stable slug for artifact tagging.
    pub slug: Arc<str>,
    /// Ordered list of mimic windows.
    pub windows: Vec<MimicSpec>,
}

impl MimicScenario {
    /// Construct a scenario with the provided slug and window specifications.
    #[must_use]
    pub fn new(slug: impl Into<Arc<str>>, windows: Vec<MimicSpec>) -> Self {
        Self {
            slug: slug.into(),
            windows,
        }
    }
}

/// Configuration knobs for the helper window runtime.
#[derive(Clone, Debug)]
pub struct HelperConfig {
    /// Lifetime for the helper window before automatic shutdown (ms).
    pub time_ms: u64,
    /// Delay applied when the system attempts to set the frame directly (ms).
    pub delay_setframe_ms: u64,
    /// Delay before applying the target frame (ms).
    pub delay_apply_ms: u64,
    /// Duration for tweened placement animations (ms).
    pub tween_ms: u64,
    /// Explicit `(x, y, w, h)` target applied after the delay, when present.
    pub apply_target: Option<(f64, f64, f64, f64)>,
    /// Grid target `(cols, rows, col, row)` used when explicit geometry is not provided.
    pub apply_grid: Option<(u32, u32, u32, u32)>,
    /// Optional slot identifier used by legacy 2x2 placements.
    pub slot: Option<u8>,
    /// Optional explicit grid specification `(cols, rows, col, row)`.
    pub grid: Option<(u32, u32, u32, u32)>,
    /// Optional explicit inner size `(w, h)` for the helper window.
    pub size: Option<(f64, f64)>,
    /// Optional explicit position `(x, y)` for the helper window.
    pub pos: Option<(f64, f64)>,
    /// Optional overlay label text rendered inside the window.
    pub label_text: Option<String>,
    /// Optional minimum content size `(w, h)`.
    pub min_size: Option<(f64, f64)>,
    /// Optional rounding step `(w, h)` applied to requested sizes.
    pub step_size: Option<(f64, f64)>,
    /// Scenario slug used for diagnostics and sibling lookups.
    pub scenario_slug: Arc<str>,
    /// Helper-specific label used for diagnostics and artifacts.
    pub window_label: Arc<str>,
    /// Launch the helper window minimized when true.
    pub start_minimized: bool,
    /// Launch the helper window zoomed when true.
    pub start_zoomed: bool,
    /// Prevent manual movement of the window when true.
    pub panel_nonmovable: bool,
    /// Prevent manual resizing of the window when true.
    pub panel_nonresizable: bool,
    /// Attach a modal sheet to the helper window when true.
    pub attach_sheet: bool,
    /// Active quirk list applied to the helper runtime.
    pub quirks: Vec<Quirk>,
    /// Placement strategy carried alongside the window for diagnostics.
    pub place: PlaceOptions,
    /// Shutdown flag shared with the controlling harness.
    pub shutdown: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl HelperConfig {
    /// Replace the shutdown flag while preserving other configuration options.
    #[must_use]
    pub fn with_shutdown(
        mut self,
        shutdown: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> Self {
        self.shutdown = shutdown;
        self
    }

    /// Attach a new quirk list to the configuration.
    #[must_use]
    pub fn with_quirks(mut self, quirks: Vec<Quirk>) -> Self {
        self.quirks = quirks;
        self
    }
}

impl Default for HelperConfig {
    fn default() -> Self {
        use std::sync::{Arc, atomic::AtomicBool};

        Self {
            time_ms: 15_000,
            delay_setframe_ms: 0,
            delay_apply_ms: 0,
            tween_ms: 0,
            apply_target: None,
            apply_grid: None,
            slot: None,
            grid: None,
            size: None,
            pos: None,
            label_text: None,
            min_size: None,
            step_size: None,
            scenario_slug: Arc::from(""),
            window_label: Arc::from(""),
            start_minimized: false,
            start_zoomed: false,
            panel_nonmovable: false,
            panel_nonresizable: false,
            attach_sheet: false,
            quirks: Vec::new(),
            place: PlaceOptions::default(),
            shutdown: Arc::new(AtomicBool::new(false)),
        }
    }
}

#[must_use]
pub(super) fn format_quirks(quirks: &[Quirk]) -> String {
    quirks
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

pub(super) fn apply_quirk_defaults(config: &mut HelperConfig, quirks: &[Quirk]) {
    if quirks.iter().any(|q| matches!(q, Quirk::DelayApplyMove)) && config.delay_apply_ms == 0 {
        config.delay_apply_ms = 160;
    }
    if quirks.iter().any(|q| matches!(q, Quirk::AxRounding)) && config.step_size.is_none() {
        config.step_size = Some((1.0, 1.0));
    }
}

/// Diagnostic snapshot describing an active mimic window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MimicDiagnostic {
    /// Scenario slug owning the window.
    pub scenario_slug: Arc<str>,
    /// Window label recorded for artifacts.
    pub window_label: Arc<str>,
    /// Quirks currently active for the helper.
    pub quirks: Vec<Quirk>,
    /// Placement strategy in effect for the helper window.
    pub place: PlaceOptions,
}

impl MimicDiagnostic {
    /// Return the `{scenario_slug}/{window_label}` identifier for diagnostics.
    #[must_use]
    pub fn tag(&self) -> String {
        format!(
            "{}/{}",
            self.scenario_slug.as_ref(),
            self.window_label.as_ref()
        )
    }

    /// Produce a human-readable quirk list for logging.
    #[must_use]
    pub fn quirks_display(&self) -> String {
        format_quirks(&self.quirks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delay_apply_move_sets_default_delay() {
        let mut cfg = HelperConfig {
            delay_apply_ms: 0,
            ..HelperConfig::default()
        };
        apply_quirk_defaults(&mut cfg, &[Quirk::DelayApplyMove]);
        assert!(cfg.delay_apply_ms > 0);
    }

    #[test]
    fn ax_rounding_sets_step_size() {
        let mut cfg = HelperConfig {
            step_size: None,
            ..HelperConfig::default()
        };
        apply_quirk_defaults(&mut cfg, &[Quirk::AxRounding]);
        assert_eq!(cfg.step_size, Some((1.0, 1.0)));
    }
}
