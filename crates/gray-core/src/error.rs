use thiserror::Error;

/// Errors that can occur within the core agent harness.
#[derive(Debug, Error)]
pub enum CoreError {
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Provider error: {0}")]
    Provider(String),

    #[error("Tool execution error: {0}")]
    #[allow(dead_code)]
    ToolExecution(String),

    #[error("Invalid message state: {0}")]
    #[allow(dead_code)]
    InvalidState(String),

    #[error("Max turns exceeded ({0})")]
    MaxTurnsExceeded(usize),

    #[error("Operation cancelled")]
    Cancelled,
}

/// Convenience type alias for Result with CoreError.
pub type Result<T> = std::result::Result<T, CoreError>;
