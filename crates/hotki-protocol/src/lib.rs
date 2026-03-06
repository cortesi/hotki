//! Hotki protocol types for client/server IPC and UI integration.
//!
//! This crate defines the serializable message types and supporting
//! structures that the backend server and the UI exchange.
#![warn(missing_docs)]
#![warn(unsafe_op_in_unsafe_fn)]

mod display;
mod focus;
mod style;
mod ui;

pub use display::{DisplayFrame, DisplaysSnapshot};
pub use focus::FocusSnapshot;
pub use style::{
    FontWeight, HudStyle, Mode, NotifyConfig, NotifyPos, NotifyTheme, NotifyWindowStyle, Offset,
    Pos, SelectorStyle, Style,
};
pub use ui::{
    HudRow, HudRowStyle, HudState, MsgToUI, NotifyKind, SelectorItemSnapshot, SelectorSnapshot,
    Toggle, WorldStreamMsg,
};

/// IPC-related helpers: channel aliases and message codec.
pub mod ipc {
    use super::MsgToUI;

    /// Default capacity for the bounded UI event pipeline.
    /// Large enough to absorb short spikes without unbounded growth.
    pub const DEFAULT_UI_CHANNEL_CAPACITY: usize = 10_000;

    /// Tokio bounded sender for UI messages.
    pub type UiTx = tokio::sync::mpsc::Sender<MsgToUI>;
    /// Tokio bounded receiver for UI messages.
    pub type UiRx = tokio::sync::mpsc::Receiver<MsgToUI>;

    /// Create the standard bounded UI channel (sender, receiver).
    pub fn ui_channel() -> (UiTx, UiRx) {
        tokio::sync::mpsc::channel::<MsgToUI>(DEFAULT_UI_CHANNEL_CAPACITY)
    }

    /// Codec for encoding/decoding UI messages used by the IPC layer.
    pub mod codec;

    /// Heartbeat tuning parameters shared by client and server.
    ///
    /// - `interval()` is how often the server emits a heartbeat.
    /// - `timeout()` is how long the client waits without receiving any
    ///   message (including heartbeat) before assuming the server is gone.
    pub mod heartbeat {
        use std::time::Duration;

        /// Default server→client heartbeat interval.
        pub const INTERVAL_MS: u64 = 500;
        /// Default client tolerance before declaring the server dead.
        pub const TIMEOUT_MS: u64 = 2_000;

        /// Convenience accessor for the interval as a `Duration`.
        pub fn interval() -> Duration {
            Duration::from_millis(INTERVAL_MS)
        }

        /// Convenience accessor for the timeout as a `Duration`.
        pub fn timeout() -> Duration {
            Duration::from_millis(TIMEOUT_MS)
        }
    }
}

/// Typed RPC definitions.
pub mod rpc;
