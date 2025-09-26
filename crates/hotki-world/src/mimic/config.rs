use std::time::Duration;

#[derive(Clone, Copy)]
pub(super) struct HelperWindowConfig {
    pub width_px: f64,
    pub height_px: f64,
    pub margin_px: f64,
}

#[derive(Clone, Copy)]
pub(super) struct InputDelays {
    pub retry_delay_ms: u64,
    pub window_registration_delay_ms: u64,
}

#[derive(Clone, Copy)]
pub(super) struct PlaceConfig {
    pub eps: f64,
}

pub(super) const HELPER_WINDOW: HelperWindowConfig = HelperWindowConfig {
    width_px: 280.0,
    height_px: 180.0,
    margin_px: 8.0,
};

pub(super) const INPUT_DELAYS: InputDelays = InputDelays {
    retry_delay_ms: 80,
    window_registration_delay_ms: 80,
};

pub(super) const PLACE: PlaceConfig = PlaceConfig { eps: 2.0 };

pub(super) const fn ms(millis: u64) -> Duration {
    Duration::from_millis(millis)
}
