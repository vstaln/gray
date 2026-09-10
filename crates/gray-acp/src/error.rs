#[derive(Debug, thiserror::Error)]
pub enum AcpError {
    #[error("agent '{0}' not installed ({1})")]
    NotInstalled(String, String),
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
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}
