use thiserror::Error;

/// Errors that can occur within the core agent harness.
#[derive(Debug, Error)]
pub enum CoreError {
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Provider error: {0}")]
    Provider(String),

    #[error("Connection failed: {0}")]
    Connection(String),

    #[error("Request timed out: {0}")]
    Timeout(String),

    #[error("Tool loop detected: {0}")]
    LoopDetected(String),

    #[error("Operation cancelled")]
    Cancelled,
}

/// Convenience type alias for Result with CoreError.
pub type Result<T> = std::result::Result<T, CoreError>;
