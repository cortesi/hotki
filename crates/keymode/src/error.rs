use thiserror::Error;

/// Error type for keymode state handling
#[derive(Debug, Error)]
#[allow(missing_docs)]
pub enum KeymodeError {
    /// Invalid relay keyspec string
    #[error("Invalid relay keyspec '{spec}'")]
    InvalidRelayKeyspec { spec: String },
}
