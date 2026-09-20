use thiserror::Error;

/// Errors that can occur within the core agent harness.
#[derive(Debug, Error)]
pub enum CoreError {
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Provider error: {0}")]
    Provider(String),

    // Provider failures keep their taxonomy (mirrors `ProviderError`) so
    // callers can branch on the class instead of parsing the message.
    #[error("auth failed: {0}")]
    Auth(String),

    #[error("rate limited: {0}")]
    RateLimited(String),

    #[error("bad request: {0}")]
    BadRequest(String),

    #[error("context exhausted — start /new or compact: {0}")]
    ContextOverflow(String),

    #[error("server error: {0}")]
    ServerError(String),

    #[error("stream broken: {0}")]
    Stream(String),

    #[error("Connection failed: {0}")]
    Connection(String),

    #[error("Request timed out: {0}")]
    Timeout(String),

    #[error("Tool loop detected: {0}")]
    LoopDetected(String),

    #[error("Operation cancelled")]
    Cancelled,
}

impl CoreError {
    /// Stable machine-readable class. Same string the `--json` error record
    /// carries, so harnesses and humans read one vocabulary.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Auth(_) => "auth_failed",
            Self::RateLimited(_) => "rate_limited",
            Self::BadRequest(_) => "bad_request",
            Self::ContextOverflow(_) => "context_overflow",
            Self::ServerError(_) => "server_error",
            Self::Stream(_) => "stream_broken",
            Self::Connection(_) => "connection_failed",
            Self::Timeout(_) => "timeout",
            Self::LoopDetected(_) => "loop_detected",
            Self::Cancelled => "cancelled",
            Self::Serialization(_) => "serialization",
            Self::Provider(_) => "provider_error",
        }
    }

    /// True when re-running the same turn can plausibly succeed: the provider
    /// or network failed, not the request. Auth/bad-request/context failures
    /// repeat identically until the configuration or session changes.
    pub fn retryable(&self) -> bool {
        matches!(
            self,
            Self::RateLimited(_)
                | Self::ServerError(_)
                | Self::Stream(_)
                | Self::Connection(_)
                | Self::Timeout(_)
        )
    }
}

/// Convenience type alias for Result with CoreError.
pub type Result<T> = std::result::Result<T, CoreError>;
