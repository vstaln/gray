use serde::{Deserialize, Serialize};
use std::time::Duration;
use thiserror::Error;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderDetail {
    pub status: Option<u16>,

    pub vendor_code: Option<String>,

    pub vendor_message: Option<String>,

    pub cf_ray: Option<String>,

    pub request_id: Option<String>,

    #[serde(default)]
    pub retry_after_ms: Option<u64>,
}

impl ProviderDetail {
    pub fn retry_after(&self) -> Option<std::time::Duration> {
        self.retry_after_ms.map(std::time::Duration::from_millis)
    }
}

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Provider error: {0}")]
    Provider(String),

    #[error("auth failed: {0}")]
    Auth(String),

    #[error("rate limited: {0}")]
    RateLimited(String),

    #[error("quota exhausted: {message}")]
    Quota {
        message: String,
        resets_in: Option<Duration>,
    },

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

    #[error("{class}")]
    Detailed {
        class: Box<CoreError>,
        detail: Box<ProviderDetail>,
    },
}

impl CoreError {
    pub fn class(&self) -> &CoreError {
        let mut cur = self;
        while let Self::Detailed { class, .. } = cur {
            cur = class;
        }
        cur
    }

    pub fn detail(&self) -> Option<&ProviderDetail> {
        if let Self::Detailed { detail, .. } = self {
            Some(detail.as_ref())
        } else {
            None
        }
    }

    pub fn retry_after(&self) -> Option<std::time::Duration> {
        self.detail().and_then(ProviderDetail::retry_after)
    }

    pub fn code(&self) -> &'static str {
        match self.class() {
            Self::Auth(_) => "auth_failed",
            Self::RateLimited(_) => "rate_limited",
            Self::Quota { .. } => "quota_exhausted",
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

            Self::Detailed { class, .. } => class.code(),
        }
    }

    pub fn retryable(&self) -> bool {
        matches!(
            self.class(),
            Self::RateLimited(_)
                | Self::ServerError(_)
                | Self::Stream(_)
                | Self::Connection(_)
                | Self::Timeout(_)
        )
    }
}
