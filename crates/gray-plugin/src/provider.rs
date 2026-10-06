//! Protocol 1.2 provider declarations and authenticated sidecar payloads.

use std::collections::BTreeMap;
use std::fmt;

use gray_core::credential::{CredentialEnvelope, CredentialMaterial};
use reqwest::header::HeaderName;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use url::Url;

pub const PROVIDER_CREDENTIALS: &str = "provider.credentials";
pub const PROVIDER_PROTOCOL: &str = "1.2";
pub const PROVIDER_DECLARATION_LIMIT: usize = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderDecl {
    pub id: String,
    pub name: String,
    pub transport: ProviderTransportDecl,
    pub auth_methods: Vec<AuthMethodDecl>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderTransportDecl {
    pub kind: String,
    pub base_url: Url,
    pub authorization: ProviderAuthorizationDecl,
    #[serde(default)]
    pub request: ProviderRequestPolicyDecl,
    #[serde(default)]
    pub headers: Vec<ProviderHeaderDecl>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderAuthorizationDecl {
    pub kind: String,
    pub secret_name: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderRequestPolicyDecl {
    #[serde(default)]
    pub prompt_cache_key: bool,
    /// Opt-in: the host may verbatim-replay this provider's requests for
    /// prompt-cache warming. Only for transports the host calls directly
    /// (a real HTTPS endpoint); relay sidecars that spawn per-turn
    /// children must leave it off. Skipped when false so declarations
    /// that don't opt in keep their existing profile binding.
    #[serde(default, skip_serializing_if = "is_false")]
    pub warm_replay: bool,
    #[serde(default)]
    pub store: bool,
    #[serde(default)]
    pub include_reasoning_encrypted: bool,
    #[serde(default)]
    pub previous_response_id: bool,
    #[serde(default)]
    pub tool_choice: Option<String>,
    #[serde(default)]
    pub parallel_tool_calls: Option<bool>,
    #[serde(default)]
    pub text_verbosity: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderHeaderDecl {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<ProviderHeaderSourceDecl>,
    #[serde(default)]
    pub required: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProviderHeaderSourceDecl {
    Static { value: String },
    Metadata { name: String },
    SessionId,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthMethodDecl {
    pub id: String,
    pub name: String,
    pub kind: String,
    #[serde(default)]
    pub operations: Vec<String>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ProviderAuthStart {
    pub operation_id: String,
    pub status: String,
    pub verification_uri: String,
    pub expires_at: u64,
    pub retry_after_ms: u64,
}

impl fmt::Debug for ProviderAuthStart {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProviderAuthStart")
            .field("operation_id", &self.operation_id)
            .field("status", &self.status)
            .field("verification_uri", &"<redacted>")
            .field("expires_at", &self.expires_at)
            .field("retry_after_ms", &self.retry_after_ms)
            .finish()
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub enum ProviderAuthPoll {
    Pending { retry_after_ms: Option<u64> },
    Completed(CredentialMaterial),
    Failed(ProviderRpcFailure),
    Cancelled,
    OperationLost,
}

impl fmt::Debug for ProviderAuthPoll {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Pending { retry_after_ms } => f
                .debug_struct("Pending")
                .field("retry_after_ms", retry_after_ms)
                .finish(),
            Self::Completed(material) => f
                .debug_struct("Completed")
                .field("credential", material)
                .finish(),
            Self::Failed(failure) => f.debug_tuple("Failed").field(failure).finish(),
            Self::Cancelled => f.write_str("Cancelled"),
            Self::OperationLost => f.write_str("OperationLost"),
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ProviderRpcFailure {
    pub code: String,
    pub message: String,
    pub retryable: bool,
    pub terminal: bool,
}

impl fmt::Debug for ProviderRpcFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProviderRpcFailure")
            .field("code", &self.code)
            .field("retryable", &self.retryable)
            .field("terminal", &self.terminal)
            .field("message", &"<redacted>")
            .finish()
    }
}

#[derive(Debug, Clone)]
pub enum ProviderRpcError {
    CapabilityMissing(String),
    Protocol(String),
    Rpc(ProviderRpcFailure),
    Unavailable(String),
}

impl ProviderRpcError {
    pub fn protocol(message: impl Into<String>) -> Self {
        Self::Protocol(message.into())
    }
}

impl fmt::Display for ProviderRpcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CapabilityMissing(s) => write!(f, "provider capability missing: {s}"),
            Self::Protocol(s) => write!(f, "provider protocol error: {s}"),
            Self::Rpc(e) => write!(f, "provider RPC {}: {}", e.code, e.message),
            Self::Unavailable(s) => write!(f, "provider unavailable: {s}"),
        }
    }
}

impl std::error::Error for ProviderRpcError {}

#[derive(Clone, Serialize, Deserialize)]
pub struct ProviderRefreshRequest {
    pub provider: String,
    pub auth_method: String,
    pub profile_binding: String,
    pub credential: CredentialEnvelope,
}

impl fmt::Debug for ProviderRefreshRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProviderRefreshRequest")
            .field("provider", &self.provider)
            .field("auth_method", &self.auth_method)
            .field("profile_binding", &self.profile_binding)
            .field("credential", &self.credential)
            .finish()
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ProviderRevokeRequest {
    pub provider: String,
    pub auth_method: String,
    pub profile_binding: String,
    pub credential: CredentialEnvelope,
}

impl fmt::Debug for ProviderRevokeRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProviderRevokeRequest")
            .field("provider", &self.provider)
            .field("auth_method", &self.auth_method)
            .field("profile_binding", &self.profile_binding)
            .field("credential", &self.credential)
            .finish()
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ProviderModelsRequest {
    pub provider: String,
    pub auth_method: String,
    pub profile_binding: String,
    pub credential: CredentialEnvelope,
}

impl fmt::Debug for ProviderModelsRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProviderModelsRequest")
            .field("provider", &self.provider)
            .field("auth_method", &self.auth_method)
            .field("profile_binding", &self.profile_binding)
            .field("credential", &self.credential)
            .finish()
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ProviderChatRequest {
    pub provider: String,
    pub auth_method: String,
    #[serde(default)]
    pub model: String,
}

impl fmt::Debug for ProviderChatRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProviderChatRequest")
            .field("provider", &self.provider)
            .field("auth_method", &self.auth_method)
            .field("model", &self.model)
            .finish()
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ProviderChatResult {
    pub relay_url: String,
    pub relay_token: String,
}

impl fmt::Debug for ProviderChatResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Both fields carry the per-turn bearer: relay_url embeds it in its
        // path, relay_token is it. Neither may reach a log.
        f.debug_struct("ProviderChatResult")
            .field("relay_url", &"<redacted>")
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ProviderModelCatalog {
    pub models: Vec<ProviderModel>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ProviderModel {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub context_window: Option<u32>,
    #[serde(default)]
    pub reasoning_efforts: Vec<String>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderRevokeResult {
    Revoked,
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderValidationError {
    pub message: String,
}

impl ProviderValidationError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for ProviderValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ProviderValidationError {}

impl ProviderDecl {
    pub fn from_value(value: &Value) -> Result<Self, ProviderValidationError> {
        let encoded = serde_json::to_vec(value)
            .map_err(|_| ProviderValidationError::new("provider serialization failed"))?;
        if encoded.len() > PROVIDER_DECLARATION_LIMIT {
            return Err(ProviderValidationError::new(
                "provider declaration too large",
            ));
        }
        let decl: Self = serde_json::from_value(value.clone())
            .map_err(|e| ProviderValidationError::new(format!("provider JSON: {e}")))?;
        decl.validate()?;
        Ok(decl)
    }

    pub fn validate(&self) -> Result<(), ProviderValidationError> {
        validate_slug(&self.id, "provider id")?;
        validate_text(&self.name, "provider name", 256)?;
        if self.auth_methods.is_empty() {
            return Err(ProviderValidationError::new(
                "provider must declare at least one auth method",
            ));
        }
        let mut auth_ids = BTreeMap::new();
        for method in &self.auth_methods {
            validate_slug(&method.id, "auth-method id")?;
            validate_text(&method.name, "auth-method name", 256)?;
            if !matches!(method.kind.as_str(), "oauth" | "api_key") {
                return Err(ProviderValidationError::new("unsupported auth method kind"));
            }
            if auth_ids.insert(method.id.clone(), ()).is_some() {
                return Err(ProviderValidationError::new("duplicate auth-method id"));
            }
            if method.operations.is_empty() {
                return Err(ProviderValidationError::new(
                    "auth method must declare an operation",
                ));
            }
            let mut operations = BTreeMap::new();
            for operation in &method.operations {
                if !matches!(
                    operation.as_str(),
                    "login" | "refresh" | "revoke" | "models" | "chat"
                ) {
                    return Err(ProviderValidationError::new("unsupported auth operation"));
                }
                if operations.insert(operation, ()).is_some() {
                    return Err(ProviderValidationError::new("duplicate auth operation"));
                }
            }
        }
        self.transport.validate()?;
        let encoded = serde_json::to_vec(self)
            .map_err(|e| ProviderValidationError::new(format!("provider serialization: {e}")))?;
        if encoded.len() > PROVIDER_DECLARATION_LIMIT {
            return Err(ProviderValidationError::new(
                "provider declaration too large",
            ));
        }
        Ok(())
    }

    pub fn profile_binding(&self, auth_method_id: &str) -> Result<String, ProviderValidationError> {
        if !self.auth_methods.iter().any(|m| m.id == auth_method_id) {
            return Err(ProviderValidationError::new("unknown auth method"));
        }
        let mut transport = self.transport.clone();
        transport.base_url = normalize_base_url(transport.base_url.clone());
        transport
            .headers
            .sort_by_key(|h| h.name.to_ascii_lowercase());
        // The ops list is part of the contract the credential was issued
        // against: adding `chat` must rebind rather than silently widening
        // what an existing profile is allowed to do.
        let mut operations = self
            .auth_methods
            .iter()
            .find(|m| m.id == auth_method_id)
            .map(|m| m.operations.clone())
            .unwrap_or_default();
        operations.sort();
        let value = serde_json::json!({
            "provider_id": self.id,
            "auth_method_id": auth_method_id,
            "operations": operations,
            "transport": transport,
        });
        let canonical = canonical_json(value);
        let bytes = serde_json::to_vec(&canonical)
            .map_err(|e| ProviderValidationError::new(format!("profile serialization: {e}")))?;
        let digest = Sha256::digest(bytes);
        Ok(format!("sha256:{digest:x}"))
    }
}

impl ProviderTransportDecl {
    fn validate(&self) -> Result<(), ProviderValidationError> {
        if self.kind != "openai-responses" {
            return Err(ProviderValidationError::new(
                "unsupported provider transport kind",
            ));
        }
        if self.base_url.as_str().len() > 2048
            || self.base_url.scheme() != "https"
            || self.base_url.host_str().is_none()
            || !self.base_url.username().is_empty()
            || self.base_url.password().is_some()
            || self.base_url.query().is_some()
            || self.base_url.fragment().is_some()
        {
            return Err(ProviderValidationError::new("invalid provider base URL"));
        }
        if self.authorization.kind != "bearer" {
            return Err(ProviderValidationError::new(
                "only bearer provider authorization is supported",
            ));
        }
        validate_slug(&self.authorization.secret_name, "authorization secret name")?;
        if self.request.store {
            return Err(ProviderValidationError::new("provider store must be false"));
        }
        if self
            .request
            .tool_choice
            .as_deref()
            .is_some_and(|v| !matches!(v, "auto" | "none"))
        {
            return Err(ProviderValidationError::new("invalid tool choice"));
        }
        if self
            .request
            .text_verbosity
            .as_deref()
            .is_some_and(|v| !matches!(v, "low" | "medium" | "high"))
        {
            return Err(ProviderValidationError::new("invalid text verbosity"));
        }
        let mut names = BTreeMap::new();
        for header in &self.headers {
            let name = HeaderName::from_bytes(header.name.as_bytes())
                .map_err(|_| ProviderValidationError::new("invalid provider header name"))?;
            let lower = name.as_str().to_ascii_lowercase();
            if matches!(
                lower.as_str(),
                "host"
                    | "content-length"
                    | "transfer-encoding"
                    | "connection"
                    | "content-type"
                    | "accept"
                    | "authorization"
            ) {
                return Err(ProviderValidationError::new(
                    "provider header is host-owned",
                ));
            }
            if names.insert(lower, ()).is_some() {
                return Err(ProviderValidationError::new("duplicate provider header"));
            }
            if header.value.is_some() == header.source.is_some() {
                return Err(ProviderValidationError::new(
                    "provider header needs exactly one value or source",
                ));
            }
            if let Some(value) = &header.value {
                validate_header_value(value)?;
            }
            if let Some(source) = &header.source {
                match source {
                    ProviderHeaderSourceDecl::Static { value } => validate_header_value(value)?,
                    ProviderHeaderSourceDecl::Metadata { name } => {
                        validate_slug(name, "metadata header name")?
                    }
                    ProviderHeaderSourceDecl::SessionId => {}
                }
            }
        }
        Ok(())
    }
}

fn is_false(b: &bool) -> bool {
    !*b
}

fn normalize_base_url(mut url: Url) -> Url {
    let scheme = url.scheme().to_ascii_lowercase();
    let _ = url.set_scheme(&scheme);
    if let Some(host) = url.host_str() {
        let host = host.to_ascii_lowercase();
        let _ = url.set_host(Some(&host));
    }
    if url.scheme() == "https" && url.port() == Some(443) {
        let _ = url.set_port(None);
    }
    let path = url.path().trim_end_matches('/').to_owned();
    let path = if path.is_empty() {
        "/".to_owned()
    } else {
        path
    };
    url.set_path(&path);
    url
}

fn validate_slug(value: &str, label: &str) -> Result<(), ProviderValidationError> {
    if value.is_empty()
        || value.len() > 64
        || !value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
    {
        return Err(ProviderValidationError::new(format!("invalid {label}")));
    }
    Ok(())
}

fn validate_text(value: &str, label: &str, max: usize) -> Result<(), ProviderValidationError> {
    if value.trim().is_empty() || value.len() > max || value.contains(['\r', '\n']) {
        return Err(ProviderValidationError::new(format!("invalid {label}")));
    }
    Ok(())
}

fn validate_header_value(value: &str) -> Result<(), ProviderValidationError> {
    if value.len() > 4096 || value.contains(['\r', '\n']) {
        return Err(ProviderValidationError::new(
            "invalid provider header value",
        ));
    }
    Ok(())
}

fn canonical_json(value: Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.into_iter().map(canonical_json).collect()),
        Value::Object(values) => {
            let mut entries: Vec<_> = values.into_iter().collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            let mut out = serde_json::Map::new();
            for (key, value) in entries {
                out.insert(key, canonical_json(value));
            }
            Value::Object(out)
        }
        other => other,
    }
}
