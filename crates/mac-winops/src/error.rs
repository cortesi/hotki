use thiserror::Error;

/// Errors that can occur during window operations.
#[derive(Error, Debug)]
pub enum Error {
    /// Accessibility permission is required but not granted.
    #[error("Accessibility permission missing")]
    Permission,

    /// Failed to create an Accessibility API application element.
    #[error("Failed to create AX application element")]
    AppElement,

    /// No focused window could be found for the given process.
    #[error("Focused window not available")]
    FocusedWindow,

    /// An Accessibility API operation failed with the given error code.
    #[error("AX operation failed: code {0}")]
    AxCode(i32),

    /// Operation must be executed on the main thread.
    #[error("Operation requires main thread")]
    MainThread,

    /// The requested attribute or operation is not supported.
    #[error("Unsupported attribute")]
    Unsupported,

    /// The main-thread operation queue is poisoned or inaccessible.
    #[error("Main-thread queue poisoned or push failed")]
    QueuePoisoned,

    /// An invalid index was provided.
    #[error("Invalid index")]
    InvalidIndex,

    /// Failed to activate the application.
    #[error("Activation failed")]
    ActivationFailed,
}

pub type Result<T> = std::result::Result<T, Error>;
