#[derive(Debug, thiserror::Error)]
pub enum AcpError {
    #[error("unknown agent '{0}'; available: {1}")]
    UnknownAgent(String, String),
    #[error("agent '{0}' not installed ({1})")]
    NotInstalled(String, String),
    #[error("failed to spawn '{0}': {1}")]
    Spawn(String, String),
    #[error("agent process exited (code {code}): {stderr_tail}")]
    ProcessExited { code: i32, stderr_tail: String },
    #[error("ACP request '{method}' failed: {message}")]
    Request {
        method: &'static str,
        message: String,
    },
    #[error("agent requires auth: {0}")]
    AuthRequired(String),
    #[error("protocol version mismatch: agent speaks {0}")]
    VersionMismatch(String),
    #[error("operation timed out after {0:?}")]
    Timeout(std::time::Duration),
    #[error("cancelled")]
    Cancelled,
    #[error("session is busy (one prompt at a time)")]
    Busy,
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}
