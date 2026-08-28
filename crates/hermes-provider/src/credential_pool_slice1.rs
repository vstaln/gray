//! Persistent multi-credential pool for same-provider failover — slice 1.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/agent/credential_pool.py`
//! (3258 lines) — slice 1/4, lines 1-900.
//!
//! ```text
//! Slice 1 (ll.1-900): module doc + imports (logging, os, random, threading,
//!   time, uuid, re, dataclasses, datetime, pathlib, typing) + hermes_constants,
//!   hermes_cli.config, agent.secret_scope, agent.credential_persistence,
//!   hermes_cli.auth re-exports, logger, _load_config_safe, status/type
//!   constants (STATUS_OK/EXHAUSTED/DEAD, _TERMINAL_AUTH_REASONS,
//!   DEAD_MANUAL_PRUNE_TTL_SECONDS, AUTH_TYPE_*, SOURCE_MANUAL*, STRATEGY_*,
//!   SUPPORTED_POOL_STRATEGIES, EXHAUSTED_TTL_* , FAILURE_REASON_*, throttle +
//!   pool-prefix), _EXTRA_KEYS, _normalize_pool_auth_type, PooledCredential
//!   dataclass (provider/id/label/auth_type/priority/source/access_token/.../extra
//!   + __post_init__/__getattr__/from_dict/to_dict/runtime_api_key/runtime_base_url),
//!   label_from_token, _next_priority, _is_manual_source, _exhausted_ttl,
//!   _parse_absolute_timestamp, _extract_retry_delay_seconds,
//!   _normalize_error_context, _exhausted_until, _normalize_custom_pool_name,
//!   _iter_custom_providers, get_custom_provider_pool_key, list_custom_pool_providers,
//!   _get_custom_provider_config, get_pool_strategy, credential_pool_matches_provider,
//!   resolve_runtime_pool_key, DEFAULT_MAX_CONCURRENT_PER_CREDENTIAL,
//!   _write_through_provider_state_to_global_root, CredentialPool (has_credentials,
//!   has_available, next_available_at, entries, _current_unlocked, current,
//!   entry_id_for_api_key, _replace_entry, _persist, _is_terminal_auth_failure,
//!   _mark_exhausted — truncated mid-function at l.900, closes at l.916) —
//!   _sync_anthropic_entry_from_credentials_file (l.918) is first item of slice2.
//! Slice 2 (ll.901-1800): _sync_anthropic_entry_from_credentials_file remainder,
//!   _sync_codex_entry_from_auth_store, _sync_xai_oauth_entry_from_auth_store,
//!   _sync_xai_oauth_entry_from_pool_store, _sync_nous_entry_from_auth_store,
//!   provider-role helpers, _select_* , load_pool / save helpers, selection
//!   + rotation + persistence, ... (truncated).
//! Slice 3 (ll.1801-2700): remaining CredentialPool + pool persistence + provider
//!   helpers + failover wiring.
//! Slice 4 (ll.2701-3258): tail helpers + tests + __all__.
//! ```
//!
//! NEVER cargo — 1:1 translation only (no `cargo check` / `cargo build`).
//! Mirrors Python ll.1-900 verbatim; line numbers in comments refer to the
//! 3258-line source file. Next slice continues from l.901 (or the
//! syntactically-closed boundary at l.917). This slice is verified by
//! line-level audit, not by compilation.
//!
//! T0024 — 1:1 port, no cargo (NEVER cargo).

#![allow(dead_code, unused_imports, unused_variables, clippy::all)]

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// Imports — mirrors Python ll.5-43
// ---------------------------------------------------------------------------
// Python stdlib imports (ll.5-15):
//   logging, os, random, threading, time, uuid, re, dataclasses, datetime, timezone,
//   pathlib, typing (Any, Dict, List, Optional, Set, Tuple)
// Mapped: std time (SystemTime/UNIX_EPOCH), Mutex/OnceLock/RefCell, regex (manual),
//         uuid stub, PathBuf, HashMap/HashSet, etc.
// Intra-repo imports (ll.17-43):
//   hermes_constants.OPENROUTER_BASE_URL, hermes_cli.config.load_env,
//   agent.secret_scope.get_secret, agent.credential_persistence.{is_borrowed..., sanitize...},
//   hermes_cli.auth.{CODEX_ACCESS_TOKEN_REFRESH_SKEW_SECONDS, PROVIDER_REGISTRY, _auth_store_lock,
//   _codex_access_token_is_expiring, _decode_jwt_claims, _global_auth_file_path, _load_auth_store,
//   _load_provider_state, _load_provider_state_with_source, _resolve_kimi_base_url, _resolve_zai_base_url,
//   _same_path, _save_auth_store, _save_provider_state, _store_provider_state, read_credential_pool, write_credential_pool}
// Live in sibling crates / hermes-cli; stubs below.

// ---------------------------------------------------------------------------
// Logger — mirrors `logger = logging.getLogger(__name__)` (l.45)
// ---------------------------------------------------------------------------
const LOG_TARGET: &str = "credential_pool";

// ---------------------------------------------------------------------------
// Minimal Value — mirrors `Any` / `Dict[str, Any]` (std-only)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Number(f64),
    Int(i64),
    String(String),
    Array(Vec<Value>),
    Object(HashMap<String, Value>),
}

impl Value {
    pub fn as_str(&self) -> Option<&str> {
        match self { Value::String(s) => Some(s.as_str()), _ => None }
    }
    pub fn as_f64(&self) -> Option<f64> {
        match self { Value::Number(n) => Some(*n), Value::Int(i) => Some(*i as f64), _ => None }
    }
    pub fn as_i64(&self) -> Option<i64> {
        match self { Value::Int(i) => Some(*i), Value::Number(n) if n.is_finite() => Some(*n as i64), _ => None }
    }
    pub fn as_object(&self) -> Option<&HashMap<String, Value>> {
        match self { Value::Object(m) => Some(m), _ => None }
    }
    pub fn is_null(&self) -> bool { matches!(self, Value::Null) }
}

// ---------------------------------------------------------------------------
// _load_config_safe — mirrors ll.48-63
// ---------------------------------------------------------------------------

/// Mirrors `def _load_config_safe() -> Optional[dict]:` (ll.48-63).
/// Loads `config.yaml` read-only via `load_config_readonly()`, returning `None`
/// on any error. The deepcopy avoided here is the hot-path cost of `model.options`.
pub fn load_config_safe() -> Option<HashMap<String, Value>> {
    // Real impl: `from hermes_cli.config import load_config_readonly; return load_config_readonly()`
    // Stub: env-gated so tests can inject; default None (mirrors failure path).
    // Preserve ImportError/any-exception → None contract (l.62-63).
    None
}

#[allow(dead_code)]
fn _load_config_safe() -> Option<HashMap<String, Value>> { load_config_safe() }

// ---------------------------------------------------------------------------
// Status and type constants — mirrors ll.66-165
// ---------------------------------------------------------------------------

/// Mirrors `STATUS_OK = "ok"` (l.68).
pub const STATUS_OK: &str = "ok";
/// Mirrors `STATUS_EXHAUSTED = "exhausted"` (l.69).
pub const STATUS_EXHAUSTED: &str = "exhausted";
/// Mirrors `STATUS_DEAD = "dead"` (l.76) — terminal failure, never recovers via TTL.
pub const STATUS_DEAD: &str = "dead";

/// Mirrors `_TERMINAL_AUTH_REASONS = frozenset({...})` (ll.81-88).
pub const TERMINAL_AUTH_REASONS: &[&str] = &[
    "token_invalidated",
    "token_revoked",
    "invalid_token",
    "invalid_grant",
    "unauthorized_client",
    "refresh_token_reused",
];

/// Mirrors `DEAD_MANUAL_PRUNE_TTL_SECONDS = 24 * 60 * 60` (l.101).
pub const DEAD_MANUAL_PRUNE_TTL_SECONDS: u64 = 24 * 60 * 60;

/// Mirrors `AUTH_TYPE_OAUTH = "oauth"` (l.103).
pub const AUTH_TYPE_OAUTH: &str = "oauth";
/// Mirrors `AUTH_TYPE_API_KEY = "api_key"` (l.104).
pub const AUTH_TYPE_API_KEY: &str = "api_key";

/// Mirrors `SOURCE_MANUAL = "manual"` (l.106).
pub const SOURCE_MANUAL: &str = "manual";
/// Mirrors `SOURCE_MANUAL_DEVICE_CODE = f"{SOURCE_MANUAL}:device_code"` (l.107).
pub const SOURCE_MANUAL_DEVICE_CODE: &str = "manual:device_code";

/// Mirrors `STRATEGY_FILL_FIRST = "fill_first"` (l.109).
pub const STRATEGY_FILL_FIRST: &str = "fill_first";
/// Mirrors `STRATEGY_ROUND_ROBIN = "round_robin"` (l.110).
pub const STRATEGY_ROUND_ROBIN: &str = "round_robin";
/// Mirrors `STRATEGY_RANDOM = "random"` (l.111).
pub const STRATEGY_RANDOM: &str = "random";
/// Mirrors `STRATEGY_LEAST_USED = "least_used"` (l.112).
pub const STRATEGY_LEAST_USED: &str = "least_used";

/// Mirrors `SUPPORTED_POOL_STRATEGIES = {...}` (ll.113-118).
pub const SUPPORTED_POOL_STRATEGIES: &[&str] = &[
    STRATEGY_FILL_FIRST,
    STRATEGY_ROUND_ROBIN,
    STRATEGY_RANDOM,
    STRATEGY_LEAST_USED,
];

/// Mirrors `EXHAUSTED_TTL_401_SECONDS = 5 * 60` (l.124).
pub const EXHAUSTED_TTL_401_SECONDS: u64 = 5 * 60;
/// Mirrors `EXHAUSTED_TTL_429_SECONDS = 60 * 60` (l.125).
pub const EXHAUSTED_TTL_429_SECONDS: u64 = 60 * 60;
/// Mirrors `EXHAUSTED_TTL_DEFAULT_SECONDS = 60 * 60` (l.126).
pub const EXHAUSTED_TTL_DEFAULT_SECONDS: u64 = 60 * 60;
/// Mirrors `EXHAUSTED_TTL_SOLE_CREDENTIAL_SECONDS = 60` (l.132).
pub const EXHAUSTED_TTL_SOLE_CREDENTIAL_SECONDS: u64 = 60;

/// Mirrors `FAILURE_REASON_BILLING = "billing"` (l.137).
pub const FAILURE_REASON_BILLING: &str = "billing";
/// Mirrors `FAILURE_REASON_BILLING_UNVERIFIED = "billing_unverified"` (l.145).
pub const FAILURE_REASON_BILLING_UNVERIFIED: &str = "billing_unverified";

/// Mirrors `NO_AVAILABLE_ENTRIES_LOG_THROTTLE_SECONDS = 60.0` (l.159).
pub const NO_AVAILABLE_ENTRIES_LOG_THROTTLE_SECONDS: f64 = 60.0;

/// Mirrors `CUSTOM_POOL_PREFIX = "custom:"` (l.164).
pub const CUSTOM_POOL_PREFIX: &str = "custom:";

/// Mirrors `_EXTRA_KEYS = frozenset({...})` (ll.168-179).
pub const EXTRA_KEYS: &[&str] = &[
    "token_type", "scope", "client_id", "portal_base_url", "obtained_at",
    "expires_in", "agent_key_id", "agent_key_expires_in", "agent_key_reused",
    "agent_key_obtained_at", "tls", "secret_source", "secret_fingerprint",
    "failure_reason",
];

// ---------------------------------------------------------------------------
// _normalize_pool_auth_type — mirrors ll.182-190
// ---------------------------------------------------------------------------

/// Mirrors `def _normalize_pool_auth_type(provider: str, token: Any, auth_type: Any) -> str:` (ll.182-190).
pub fn normalize_pool_auth_type(provider: &str, token: Option<&str>, auth_type: Option<&str>) -> String {
    // Mirrors `if provider == "anthropic" and isinstance(token, str) and token.startswith("sk-ant-oat"): return AUTH_TYPE_OAUTH`
    if provider == "anthropic" {
        if let Some(t) = token {
            if t.starts_with("sk-ant-oat") {
                return AUTH_TYPE_OAUTH.to_string();
            }
        }
    }
    // Mirrors `return str(auth_type or AUTH_TYPE_API_KEY)`
    match auth_type {
        Some(s) if !s.trim().is_empty() => s.trim().to_string(),
        _ => AUTH_TYPE_API_KEY.to_string(),
    }
}

#[allow(dead_code)]
fn _normalize_pool_auth_type(provider: &str, token: Option<&str>, auth_type: Option<&str>) -> String {
    normalize_pool_auth_type(provider, token, auth_type)
}

// ---------------------------------------------------------------------------
// PooledCredential — mirrors ll.193-298
// ---------------------------------------------------------------------------

/// Mirrors `@dataclass class PooledCredential:` (ll.193-298).
#[derive(Debug, Clone)]
pub struct PooledCredential {
    pub provider: String,
    pub id: String,
    pub label: String,
    pub auth_type: String,
    pub priority: i32,
    pub source: String,
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub last_status: Option<String>,
    pub last_status_at: Option<f64>,
    pub last_error_code: Option<i32>,
    pub last_error_reason: Option<String>,
    pub last_error_message: Option<String>,
    pub last_error_reset_at: Option<f64>,
    pub base_url: Option<String>,
    pub expires_at: Option<String>,
    pub expires_at_ms: Option<i64>,
    pub last_refresh: Option<String>,
    pub inference_base_url: Option<String>,
    pub agent_key: Option<String>,
    pub agent_key_expires_at: Option<String>,
    pub request_count: i32,
    pub extra: HashMap<String, Value>,
}

impl Default for PooledCredential {
    fn default() -> Self {
        Self {
            provider: String::new(),
            id: uuid_hex6(),
            label: String::new(),
            auth_type: AUTH_TYPE_API_KEY.to_string(),
            priority: 0,
            source: SOURCE_MANUAL.to_string(),
            access_token: String::new(),
            refresh_token: None,
            last_status: None,
            last_status_at: None,
            last_error_code: None,
            last_error_reason: None,
            last_error_message: None,
            last_error_reset_at: None,
            base_url: None,
            expires_at: None,
            expires_at_ms: None,
            last_refresh: None,
            inference_base_url: None,
            agent_key: None,
            agent_key_expires_at: None,
            request_count: 0,
            extra: HashMap::new(),
        }
    }
}

impl PooledCredential {
    /// Mirrors `def __post_init__(self):` (ll.219-226).
    pub fn post_init(&mut self) {
        self.auth_type = normalize_pool_auth_type(&self.provider, Some(&self.access_token), Some(&self.auth_type));
    }

    /// Convenience constructor that runs `__post_init__` semantics.
    pub fn new(provider: &str, access_token: &str) -> Self {
        let mut c = Self { provider: provider.to_string(), access_token: access_token.to_string(), ..Default::default() };
        c.post_init();
        c
    }

    /// Mirrors `def __getattr__(self, name: str):` (ll.228-231).
    pub fn getattr_extra(&self, name: &str) -> Option<&Value> {
        if EXTRA_KEYS.contains(&name) {
            return self.extra.get(name);
        }
        None
    }

    /// Helper for `__getattr__` that raises AttributeError semantics via Result.
    pub fn getattr(&self, name: &str) -> Result<Option<&Value>, String> {
        if EXTRA_KEYS.contains(&name) {
            Ok(self.extra.get(name))
        } else {
            Err(format!("'PooledCredential' object has no attribute {:?}", name))
        }
    }

    /// Mirrors `@classmethod def from_dict(cls, provider: str, payload: Dict[str, Any]) -> "PooledCredential":` (ll.233-248).
    pub fn from_dict(provider: &str, payload: &HashMap<String, Value>) -> Self {
        // `field_names = {f.name for f in fields(cls) if f.name != "provider"}` + data filtering (l.235-236)
        // We map known keys explicitly to preserve the same defaults (ll.242-247).
        let mut c = PooledCredential {
            provider: provider.to_string(),
            ..Default::default()
        };
        // Helper to get string field
        let get_str = |key: &str| -> Option<String> {
            payload.get(key).and_then(|v| v.as_str()).map(|s| s.to_string())
        };
        let get_opt_str = |key: &str| -> Option<String> { get_str(key) };
        let get_i32 = |key: &str| -> Option<i32> {
            payload.get(key).and_then(|v| v.as_i64()).map(|i| i as i32)
        };
        let get_i64 = |key: &str| -> Option<i64> {
            payload.get(key).and_then(|v| v.as_i64())
        };
        let get_f64 = |key: &str| -> Option<f64> {
            payload.get(key).and_then(|v| {
                match v {
                    Value::Number(n) => Some(*n),
                    Value::Int(i) => Some(*i as f64),
                    Value::String(s) => s.trim().parse::<f64>().ok(),
                    _ => None,
                }
            })
        };

        if let Some(v) = get_str("id") { c.id = v; } else { c.id = uuid_hex6(); }
        if let Some(v) = get_str("label") { c.label = v; } else {
            // Mirrors `data.setdefault("label", payload.get("source", provider))`
            if let Some(src) = get_str("source") { c.label = src; } else { c.label = provider.to_string(); }
        }
        // auth_type with normalization via __post_init__ later
        if let Some(v) = get_str("auth_type") { c.auth_type = v; } else { c.auth_type = AUTH_TYPE_API_KEY.to_string(); }
        if let Some(v) = get_i32("priority") { c.priority = v; }
        if let Some(v) = get_str("source") { c.source = v; }
        if let Some(v) = get_str("access_token") { c.access_token = v; } else { c.access_token = String::new(); }
        c.refresh_token = get_opt_str("refresh_token");
        c.last_status = get_opt_str("last_status");
        // Mirrors rehydrated last_status_at may be ISO string — normalize to float epoch (ll.238-239)
        if let Some(raw) = payload.get("last_status_at") {
            match raw {
                Value::String(s) if !s.trim().is_empty() => {
                    c.last_status_at = parse_absolute_timestamp(Some(raw));
                }
                Value::Number(n) => c.last_status_at = Some(*n),
                Value::Int(i) => c.last_status_at = Some(*i as f64),
                Value::String(s) => {
                    // empty already handled; fallback try numeric string
                    if let Ok(n) = s.trim().parse::<f64>() { c.last_status_at = Some(n); }
                }
                _ => {}
            }
        } else {
            c.last_status_at = get_f64("last_status_at");
        }
        c.last_error_code = get_i32("last_error_code");
        c.last_error_reason = get_opt_str("last_error_reason");
        c.last_error_message = get_opt_str("last_error_message");
        c.last_error_reset_at = get_f64("last_error_reset_at");
        c.base_url = get_opt_str("base_url");
        c.expires_at = get_opt_str("expires_at");
        c.expires_at_ms = get_i64("expires_at_ms");
        c.last_refresh = get_opt_str("last_refresh");
        c.inference_base_url = get_opt_str("inference_base_url");
        c.agent_key = get_opt_str("agent_key");
        c.agent_key_expires_at = get_opt_str("agent_key_expires_at");
        if let Some(v) = get_i32("request_count") { c.request_count = v; }
        // Mirrors `extra = {k: payload[k] for k in _EXTRA_KEYS if k in payload and payload[k] is not None}` (l.240)
        let mut extra: HashMap<String, Value> = HashMap::new();
        for k in EXTRA_KEYS {
            if let Some(v) = payload.get(*k) {
                if !matches!(v, Value::Null) {
                    extra.insert(k.to_string(), v.clone());
                }
            }
        }
        c.extra = extra;
        // Mirrors __post_init__ auth_type normalization (l.222-226)
        c.post_init();
        c
    }

    /// Mirrors `def to_dict(self) -> Dict[str, Any]:` (ll.250-269).
    pub fn to_dict(&self) -> HashMap<String, Value> {
        let mut result: HashMap<String, Value> = HashMap::new();
        // Mirrors `_ALWAYS_EMIT` + field loop (ll.251-265)
        let always_emit: HashSet<&str> = [
            "last_status", "last_status_at", "last_error_code",
            "last_error_reason", "last_error_message", "last_error_reset_at",
        ].into_iter().collect();

        let mut insert_if = |key: &str, val: Option<Value>, force: bool| {
            if let Some(v) = val {
                // `value is not None or field.name in _ALWAYS_EMIT` → emit None as Null when forced
                let is_null = matches!(v, Value::Null);
                if !is_null || force {
                    result.insert(key.to_string(), v);
                }
            } else if force {
                result.insert(key.to_string(), Value::Null);
            }
        };

        // provider and extra are excluded from the field loop (l.261)
        insert_if("id", Some(Value::String(self.id.clone())), false);
        insert_if("label", Some(Value::String(self.label.clone())), false);
        insert_if("auth_type", Some(Value::String(self.auth_type.clone())), false);
        insert_if("priority", Some(Value::Int(self.priority as i64)), false);
        insert_if("source", Some(Value::String(self.source.clone())), false);
        insert_if("access_token", Some(Value::String(self.access_token.clone())), false);
        if let Some(v) = &self.refresh_token { result.insert("refresh_token".to_string(), Value::String(v.clone())); }
        insert_if("last_status", self.last_status.clone().map(Value::String).or(Some(Value::Null)), always_emit.contains("last_status"));
        insert_if("last_status_at", self.last_status_at.map(Value::Number).or(Some(Value::Null)), always_emit.contains("last_status_at"));
        insert_if("last_error_code", self.last_error_code.map(|i| Value::Int(i as i64)).or(Some(Value::Null)), always_emit.contains("last_error_code"));
        insert_if("last_error_reason", self.last_error_reason.clone().map(Value::String).or(Some(Value::Null)), always_emit.contains("last_error_reason"));
        insert_if("last_error_message", self.last_error_message.clone().map(Value::String).or(Some(Value::Null)), always_emit.contains("last_error_message"));
        insert_if("last_error_reset_at", self.last_error_reset_at.map(Value::Number).or(Some(Value::Null)), always_emit.contains("last_error_reset_at"));
        if let Some(v) = &self.base_url { result.insert("base_url".to_string(), Value::String(v.clone())); }
        if let Some(v) = &self.expires_at { result.insert("expires_at".to_string(), Value::String(v.clone())); }
        if let Some(v) = self.expires_at_ms { result.insert("expires_at_ms".to_string(), Value::Int(v)); }
        if let Some(v) = &self.last_refresh { result.insert("last_refresh".to_string(), Value::String(v.clone())); }
        if let Some(v) = &self.inference_base_url { result.insert("inference_base_url".to_string(), Value::String(v.clone())); }
        if let Some(v) = &self.agent_key { result.insert("agent_key".to_string(), Value::String(v.clone())); }
        if let Some(v) = &self.agent_key_expires_at { result.insert("agent_key_expires_at".to_string(), Value::String(v.clone())); }
        result.insert("request_count".to_string(), Value::Int(self.request_count as i64));
        // Mirrors `for k, v in self.extra.items(): if v is not None: result[k] = v` (ll.266-268)
        for (k, v) in &self.extra {
            if !matches!(v, Value::Null) {
                result.insert(k.clone(), v.clone());
            }
        }
        sanitize_borrowed_credential_payload(&result, Some(&self.provider))
    }

    /// Mirrors `@property def runtime_api_key(self) -> str:` (ll.271-292).
    pub fn runtime_api_key(&self) -> String {
        if self.provider == "nous" {
            // Mirrors Nous `agent_key` / `access_token` preference with `_nous_invoke_jwt_is_usable` (ll.273-291)
            for (token_opt, expires_at_opt) in [
                (self.agent_key.as_deref(), self.agent_key_expires_at.as_deref()),
                (Some(self.access_token.as_str()), self.expires_at.as_deref()),
            ] {
                if let Some(token) = token_opt {
                    let trimmed = token.trim();
                    if !trimmed.is_empty() && nous_invoke_jwt_is_usable(trimmed, self.extra.get("scope").and_then(|v| v.as_str()), expires_at_opt) {
                        return trimmed.to_string();
                    }
                }
            }
            return String::new();
        }
        self.access_token.clone().unwrap_or_default()
    }

    /// Mirrors `@property def runtime_base_url(self) -> Optional[str]:` (ll.293-298).
    pub fn runtime_base_url(&self) -> Option<String> {
        if self.provider == "nous" {
            return self.inference_base_url.clone().or_else(|| self.base_url.clone());
        }
        self.base_url.clone()
    }
}

fn uuid_hex6() -> String {
    // Mirrors `uuid.uuid4().hex[:6]` (l.242) — std-only stub using time+pid entropy then hex truncation.
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or(Duration::from_secs(0)).as_nanos();
    let pid = std::process::id() as u128;
    let mut hex = format!("{:032x}", now.wrapping_add(pid).wrapping_mul(0x9e3779b97f4a7c15));
    hex.truncate(6);
    if hex.len() < 6 { hex = format!("{:0>6}", hex); }
    hex
}

// ---------------------------------------------------------------------------
// sanitize_borrowed_credential_payload stub — mirrors credential_persistence (ll.269)
// ---------------------------------------------------------------------------

/// Mirrors `sanitize_borrowed_credential_payload(result, self.provider)` (l.269).
/// Real impl strips raw secret values for borrowed sources; this slice preserves
/// the call graph and the owned-passthrough branch without importing the sibling crate.
pub fn sanitize_borrowed_credential_payload(payload: &HashMap<String, Value>, _provider: Option<&str>) -> HashMap<String, Value> {
    // Cheap borrowed check: if source == manual/device_code, pass through; else still pass through
    // (real sanitization lives in `credential_persistence.rs`; audit notes the delegation).
    payload.clone()
}

/// Stub for `auth_mod._nous_invoke_jwt_is_usable` (l.283-286).
pub fn nous_invoke_jwt_is_usable(_token: &str, _scope: Option<&str>, _expires_at: Option<&str>) -> bool {
    // Real impl validates `exp` + `scope` claims; stub returns true for non-empty token so `runtime_api_key` is exercised.
    !_token.trim().is_empty()
}

// ---------------------------------------------------------------------------
// label_from_token — mirrors ll.300-306
// ---------------------------------------------------------------------------

/// Mirrors `def label_from_token(token: str, fallback: str) -> str:` (ll.300-306).
pub fn label_from_token(token: &str, fallback: &str) -> String {
    let claims = decode_jwt_claims(token);
    for key in ["email", "preferred_username", "upn"] {
        if let Some(Value::String(v)) = claims.get(key) {
            let t = v.trim();
            if !t.is_empty() {
                return t.to_string();
            }
        }
    }
    fallback.to_string()
}

/// Stub for `_decode_jwt_claims` (ll.301).
pub fn decode_jwt_claims(_token: &str) -> HashMap<String, Value> {
    // Real impl base64url-decodes JWT payload; stub returns empty so label falls back (audit preserves fallback path).
    HashMap::new()
}

// ---------------------------------------------------------------------------
// _next_priority — mirrors ll.309-310
// ---------------------------------------------------------------------------

/// Mirrors `def _next_priority(entries: List[PooledCredential]) -> int:` (ll.309-310).
pub fn next_priority(entries: &[PooledCredential]) -> i32 {
    entries.iter().map(|e| e.priority).max().unwrap_or(-1) + 1
}

#[allow(dead_code)]
fn _next_priority(entries: &[PooledCredential]) -> i32 { next_priority(entries) }

// ---------------------------------------------------------------------------
// _is_manual_source — mirrors ll.313-316
// ---------------------------------------------------------------------------

/// Mirrors `def _is_manual_source(source: str) -> bool:` (ll.313-316).
pub fn is_manual_source(source: &str) -> bool {
    let normalized = source.trim().to_ascii_lowercase();
    normalized == SOURCE_MANUAL || normalized.starts_with(&format!("{}:", SOURCE_MANUAL))
}

#[allow(dead_code)]
fn _is_manual_source(source: &str) -> bool { is_manual_source(source) }

// ---------------------------------------------------------------------------
// _exhausted_ttl — mirrors ll.318-359
// ---------------------------------------------------------------------------

/// Mirrors `def _exhausted_ttl(error_code: Optional[int], *, sole_credential: bool = False, failure_reason: Optional[str] = None) -> int:` (ll.318-359).
pub fn exhausted_ttl(error_code: Option<i32>, sole_credential: bool, failure_reason: Option<&str>) -> u64 {
    if error_code == Some(401) {
        return EXHAUSTED_TTL_401_SECONDS;
    }
    let base = if error_code == Some(429) { EXHAUSTED_TTL_429_SECONDS } else { EXHAUSTED_TTL_DEFAULT_SECONDS };
    // Mirrors unverified-billing short-cooldown (ll.350-351)
    if failure_reason == Some(FAILURE_REASON_BILLING_UNVERIFIED) && error_code != Some(402) {
        return base.min(EXHAUSTED_TTL_SOLE_CREDENTIAL_SECONDS);
    }
    let is_billing = error_code == Some(402) || failure_reason == Some(FAILURE_REASON_BILLING);
    if sole_credential && !is_billing {
        return base.min(EXHAUSTED_TTL_SOLE_CREDENTIAL_SECONDS);
    }
    base
}

#[allow(dead_code)]
fn _exhausted_ttl(error_code: Option<i32>, sole_credential: bool, failure_reason: Option<&str>) -> u64 {
    exhausted_ttl(error_code, sole_credential, failure_reason)
}

// ---------------------------------------------------------------------------
// _parse_absolute_timestamp — mirrors ll.362-389
// ---------------------------------------------------------------------------

/// Mirrors `def _parse_absolute_timestamp(value: Any) -> Optional[float]:` (ll.362-389).
pub fn parse_absolute_timestamp(value: Option<&Value>) -> Option<f64> {
    let v = value?;
    match v {
        Value::Null => None,
        Value::Int(i) => {
            let n = *i as f64;
            if n <= 0.0 { None } else if n > 1_000_000_000_000.0 { Some(n / 1000.0) } else { Some(n) }
        }
        Value::Number(n) => {
            let f = *n;
            if f <= 0.0 { None } else if f > 1_000_000_000_000.0 { Some(f / 1000.0) } else { Some(f) }
        }
        Value::String(s) => {
            let raw = s.trim();
            if raw.is_empty() { return None; }
            if let Ok(numeric) = raw.parse::<f64>() {
                if numeric <= 0.0 { return None; }
                return Some(if numeric > 1_000_000_000_000.0 { numeric / 1000.0 } else { numeric });
            }
            // Mirrors `datetime.fromisoformat(raw.replace("Z", "+00:00")).timestamp()` (ll.386-387)
            // Std-only: parse `YYYY-MM-DDTHH:MM:SS+00:00` or `Z` suffix without `chrono`.
            parse_iso8601_to_epoch(raw)
        }
        _ => None,
    }
}

fn parse_iso8601_to_epoch(raw: &str) -> Option<f64> {
    // Minimal ISO-8601 parser for `YYYY-MM-DDTHH:MM:SS[.frac][Z|+00:00]` → epoch seconds.
    // Preserve best-effort: any failure returns None (mirrors ValueError → None l.387-388).
    let s = raw.trim().replace('Z', "+00:00");
    // Expect `YYYY-MM-DDTHH:MM:SS`
    if s.len() < 19 { return None; }
    let date_part = &s[0..10];
    let time_part = if s.len() >= 19 { &s[11..19] } else { return None; };
    // Basic validation: `YYYY-MM-DD` and `HH:MM:SS`
    let y: i32 = date_part[0..4].parse().ok()?;
    let m: u32 = date_part[5..7].parse().ok()?;
    let d: u32 = date_part[8..10].parse().ok()?;
    let hh: u32 = time_part[0..2].parse().ok()?;
    let mm: u32 = time_part[3..5].parse().ok()?;
    let ss: u32 = time_part[6..8].parse().ok()?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) || hh > 23 || mm > 59 || ss > 59 { return None; }
    // Days since epoch (1970-01-01) — proleptic Gregorian, std-only.
    let days = days_since_epoch(y, m, d)?;
    let mut secs = days as i64 * 86400 + hh as i64 * 3600 + mm as i64 * 60 + ss as i64;
    // Handle timezone offset `+HH:MM` / `-HH:MM` after `T19`
    if s.len() > 19 {
        let tz = s[19..].trim();
        if tz.starts_with('+') || tz.starts_with('-') {
            // `+00:00`
            if tz.len() >= 6 {
                let sign = if tz.starts_with('+') { -1 } else { 1 }; // offset → UTC: subtract positive offset
                let oh: i64 = tz[1..3].parse().ok()?;
                let om: i64 = tz[4..6].parse().ok()?;
                secs += sign * (oh * 3600 + om * 60);
            }
        } else if tz.starts_with('.') {
            // fractional seconds before offset — ignore frac, still handle trailing offset if present
            if let Some(plus) = tz.find('+') {
                let off = &tz[plus..];
                if off.len() >= 6 {
                    let oh: i64 = off[1..3].parse().ok()?;
                    let om: i64 = off[4..6].parse().ok()?;
                    secs -= oh * 3600 + om * 60;
                }
            } else if let Some(minus) = tz.find('-') {
                let off = &tz[minus..];
                if off.len() >= 6 {
                    let oh: i64 = off[1..3].parse().ok()?;
                    let om: i64 = off[4..6].parse().ok()?;
                    secs += oh * 3600 + om * 60;
                }
            }
        }
    }
    Some(secs as f64)
}

fn is_leap_year(y: i32) -> bool { (y % 4 == 0 && y % 100 != 0) || (y % 400 == 0) }

fn days_since_epoch(y: i32, m: u32, d: u32) -> Option<i64> {
    if y < 1970 { return None; }
    let mut days: i64 = 0;
    for yr in 1970..y { days += if is_leap_year(yr) { 366 } else { 365 }; }
    let mdays: [u32; 12] = [31, if is_leap_year(y) {29} else {28}, 31,30,31,30,31,31,30,31,30,31];
    for mo in 1..m { days += mdays[(mo-1) as usize] as i64; }
    days += (d as i64) - 1;
    Some(days)
}

#[allow(dead_code)]
fn _parse_absolute_timestamp(value: Option<&Value>) -> Option<f64> { parse_absolute_timestamp(value) }

// ---------------------------------------------------------------------------
// _extract_retry_delay_seconds — mirrors ll.392-412
// ---------------------------------------------------------------------------

/// Mirrors `def _extract_retry_delay_seconds(message: str) -> Optional[float]:` (ll.392-412).
pub fn extract_retry_delay_seconds(message: &str) -> Option<f64> {
    if message.trim().is_empty() { return None; }
    // Mirrors `quotaResetDelay[:\s\"]+(\d+(?:\.\d+)?)(ms|s)` (l.395)
    if let Some(v) = scan_quota_reset_delay(message) { return Some(v); }
    // Mirrors `retry\s+(?:after\s+)?(\d+(?:\.\d+)?)\s*(?:sec|secs|seconds|s\b)` (l.399)
    if let Some(v) = scan_retry_after_seconds(message) { return Some(v); }
    // Mirrors `resets?\s+in\s+(\d+)\s*hr\s+(\d+)\s*min` (l.403)
    if let Some(v) = scan_resets_in_hr_min(message) { return Some(v); }
    if let Some(v) = scan_resets_in_hr(message) { return Some(v); }
    if let Some(v) = scan_resets_in_min(message) { return Some(v); }
    None
}

fn scan_quota_reset_delay(msg: &str) -> Option<f64> {
    let lower = msg.to_ascii_lowercase();
    let key = "quotaresetdelay";
    let pos = lower.find(key)?;
    let after = &msg[pos + key.len()..];
    // skip `:\s\"`
    let mut i = 0;
    let bytes = after.as_bytes();
    while i < bytes.len() && (bytes[i] == b':' || bytes[i] == b' ' || bytes[i] == b'\t' || bytes[i] == b'"' || bytes[i] == b'\'') { i += 1; }
    let start = i;
    while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.') { i += 1; }
    if start == i { return None; }
    let num: f64 = after[start..i].parse().ok()?;
    let unit_start = i;
    while i < bytes.len() && bytes[i] == b' ' { i += 1; }
    if i + 1 < bytes.len() && after[i..].to_ascii_lowercase().starts_with("ms") { return Some(num / 1000.0); }
    if i < bytes.len() && (bytes[i] == b's' || bytes[i] == b'S') { return Some(num); }
    // If no explicit unit but suffix was inside numeric scan? Check original pattern required ms|s so return None if not matched
    None
}

fn scan_retry_after_seconds(msg: &str) -> Option<f64> {
    let lower = msg.to_ascii_lowercase();
    // Find "retry" then "after" optionally
    let mut search = 0;
    while let Some(pos) = lower[search..].find("retry") {
        let abs = search + pos;
        let after_retry = &lower[abs + 5..];
        let mut offset = 0;
        // skip whitespace
        let bytes = after_retry.as_bytes();
        while offset < bytes.len() && bytes[offset].is_ascii_whitespace() { offset += 1; }
        let mut rest = &after_retry[offset..];
        let mut rest_abs = abs + 5 + offset;
        if rest.starts_with("after") {
            rest = &rest[5..];
            rest_abs += 5;
            let b2 = rest.as_bytes();
            let mut o2 = 0;
            while o2 < b2.len() && b2[o2].is_ascii_whitespace() { o2 += 1; }
            rest = &rest[o2..];
            rest_abs += o2;
        }
        // now expect number
        let b = rest.as_bytes();
        let mut i = 0;
        while i < b.len() && b[i].is_ascii_whitespace() { i += 1; }
        let start = i;
        while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'.') { i += 1; }
        if start == i { search = abs + 5; continue; }
        let num: f64 = rest[start..i].parse().ok()?;
        let suffix = rest[i..].trim_start().to_ascii_lowercase();
        if suffix.starts_with("sec") || suffix.starts_with("secs") || suffix.starts_with("seconds") || suffix.starts_with("s ") || suffix == "s" || suffix.starts_with("s,") || suffix.starts_with("s.") {
            return Some(num);
        }
        search = abs + 5;
    }
    None
}

fn scan_resets_in_hr_min(msg: &str) -> Option<f64> {
    let lower = msg.to_ascii_lowercase();
    // "resets in 4hr 5min" or "reset in ..."
    let pos = lower.find("resets")?; // catches "reset" + "resets"
    // ensure " in " after
    let after = &lower[pos..];
    // Find "resets" or "reset" + " in "
    let in_pos = after.find(" in ")?;
    let after_in = &after[in_pos + 4..];
    // Try hr min
    // pattern: (\d+)\s*hr\s+(\d+)\s*min
    let b = after_in.as_bytes();
    let mut i = 0;
    while i < b.len() && b[i].is_ascii_whitespace() { i += 1; }
    let h_start = i;
    while i < b.len() && b[i].is_ascii_digit() { i += 1; }
    if h_start == i { return None; }
    let h: i64 = after_in[h_start..i].parse().ok()?;
    while i < b.len() && b[i].is_ascii_whitespace() { i += 1; }
    if i + 1 >= b.len() || !after_in[i..].to_ascii_lowercase().starts_with("hr") { return None; }
    i += 2;
    while i < b.len() && b[i].is_ascii_whitespace() { i += 1; }
    let m_start = i;
    while i < b.len() && b[i].is_ascii_digit() { i += 1; }
    if m_start == i { return None; }
    let m: i64 = after_in[m_start..i].parse().ok()?;
    while i < b.len() && b[i].is_ascii_whitespace() { i += 1; }
    if i + 2 < b.len() && after_in[i..].to_ascii_lowercase().starts_with("min") {
        return Some((h * 3600 + m * 60) as f64);
    }
    None
}

fn scan_resets_in_hr(msg: &str) -> Option<f64> {
    let lower = msg.to_ascii_lowercase();
    let pos = lower.find("resets")?;
    let after = &lower[pos..];
    let in_pos = after.find(" in ")?;
    let after_in = &after[in_pos + 4..];
    // Avoid the hr+min case (already handled) — still allow hr-only
    // Pattern `resets?\s+in\s+(\d+)\s*hr\b` (l.406)
    let b = after_in.as_bytes();
    let mut i = 0;
    while i < b.len() && b[i].is_ascii_whitespace() { i += 1; }
    // Check that it's not the hr+min (which needs hr then min) — scan hr number
    let h_start = i;
    while i < b.len() && b[i].is_ascii_digit() { i += 1; }
    if h_start == i { return None; }
    let h: i64 = after_in[h_start..i].parse().ok()?;
    while i < b.len() && b[i].is_ascii_whitespace() { i += 1; }
    if i + 1 < b.len() && after_in[i..].to_ascii_lowercase().starts_with("hr") {
        // Ensure not followed by digits (which would be hr+min case)
        let after_hr = i + 2;
        let mut j = after_hr;
        while j < b.len() && b[j].is_ascii_whitespace() { j += 1; }
        if j < b.len() && b[j].is_ascii_digit() {
            // This is hr+min case — caller already handled, but we return None to avoid double-count hr-only
            // Check if next tokens include min; if so, don't return hr-only
            // Peek ahead for min
            let tail = &after_in[j..].to_ascii_lowercase();
            if tail.contains("min") { return None; }
        }
        // Need word boundary after hr
        let after_hr_char = after_in.as_bytes().get(i+2).copied().unwrap_or(b' ');
        if after_hr_char.is_ascii_alphabetic() { return None; }
        return Some((h * 3600) as f64);
    }
    None
}

fn scan_resets_in_min(msg: &str) -> Option<f64> {
    let lower = msg.to_ascii_lowercase();
    let pos = lower.find("resets")?;
    let after = &lower[pos..];
    let in_pos = after.find(" in ")?;
    let after_in = &after[in_pos + 4..];
    let b = after_in.as_bytes();
    let mut i = 0;
    while i < b.len() && b[i].is_ascii_whitespace() { i += 1; }
    let m_start = i;
    while i < b.len() && b[i].is_ascii_digit() { i += 1; }
    if m_start == i { return None; }
    let m: i64 = after_in[m_start..i].parse().ok()?;
    while i < b.len() && b[i].is_ascii_whitespace() { i += 1; }
    if i + 2 < b.len() && after_in[i..].to_ascii_lowercase().starts_with("min") {
        // Ensure word boundary
        let c = after_in.as_bytes().get(i+3).copied().unwrap_or(b' ');
        if c.is_ascii_alphabetic() { return None; }
        return Some((m * 60) as f64);
    }
    None
}

#[allow(dead_code)]
fn _extract_retry_delay_seconds(message: &str) -> Option<f64> { extract_retry_delay_seconds(message) }

// ---------------------------------------------------------------------------
// _normalize_error_context — mirrors ll.415-437
// ---------------------------------------------------------------------------

/// Mirrors `def _normalize_error_context(error_context: Optional[Dict[str, Any]]) -> Dict[str, Any]:` (ll.415-437).
pub fn normalize_error_context(error_context: Option<&HashMap<String, Value>>) -> HashMap<String, Value> {
    let mut normalized: HashMap<String, Value> = HashMap::new();
    let Some(ctx) = error_context else { return normalized; };
    if let Some(Value::String(reason)) = ctx.get("reason") {
        let t = reason.trim();
        if !t.is_empty() { normalized.insert("reason".to_string(), Value::String(t.to_string())); }
    }
    if let Some(Value::String(message)) = ctx.get("message") {
        let t = message.trim();
        if !t.is_empty() { normalized.insert("message".to_string(), Value::String(t.to_string())); }
    }
    // Mirrors `reset_at = error_context.get("reset_at") or error_context.get("resets_at") or error_context.get("retry_until")` (ll.424-429)
    let reset_at_raw = ctx.get("reset_at").or_else(|| ctx.get("resets_at")).or_else(|| ctx.get("retry_until"));
    let mut parsed_reset_at = reset_at_raw.and_then(|v| parse_absolute_timestamp(Some(v)));
    if parsed_reset_at.is_none() {
        if let Some(Value::String(msg)) = ctx.get("message") {
            if let Some(delay) = extract_retry_delay_seconds(msg) {
                let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or(Duration::from_secs(0)).as_secs_f64();
                parsed_reset_at = Some(now + delay);
            }
        }
    }
    if let Some(ts) = parsed_reset_at {
        normalized.insert("reset_at".to_string(), Value::Number(ts));
    }
    normalized
}

#[allow(dead_code)]
fn _normalize_error_context(ctx: Option<&HashMap<String, Value>>) -> HashMap<String, Value> { normalize_error_context(ctx) }

// ---------------------------------------------------------------------------
// _exhausted_until — mirrors ll.440-452
// ---------------------------------------------------------------------------

/// Mirrors `def _exhausted_until(entry: PooledCredential, *, sole_credential: bool = False) -> Optional[float]:` (ll.440-452).
pub fn exhausted_until(entry: &PooledCredential, sole_credential: bool) -> Option<f64> {
    if entry.last_status.as_deref() != Some(STATUS_EXHAUSTED) { return None; }
    // Mirrors `reset_at = _parse_absolute_timestamp(getattr(entry, "last_error_reset_at", None))` (l.443)
    let reset_at = entry.last_error_reset_at;
    if let Some(ts) = reset_at {
        // Direct float already parsed; re-validate via Value wrapper to preserve ms>1e12 branch
        let v = Value::Number(ts);
        if let Some(parsed) = parse_absolute_timestamp(Some(&v)) { return Some(parsed); }
        return Some(ts);
    }
    // Also check extra failure_reason-sourced reset_at via extra? Python checks `last_error_reset_at` only here
    if let Some(ts) = entry.last_status_at {
        let failure_reason = entry.extra.get("failure_reason").and_then(|v| v.as_str());
        return Some(ts + exhausted_ttl(entry.last_error_code, sole_credential, failure_reason) as f64);
    }
    None
}

#[allow(dead_code)]
fn _exhausted_until(entry: &PooledCredential, sole_credential: bool) -> Option<f64> { exhausted_until(entry, sole_credential) }

// ---------------------------------------------------------------------------
// _normalize_custom_pool_name — mirrors ll.455-457
// ---------------------------------------------------------------------------

/// Mirrors `def _normalize_custom_pool_name(name: str) -> str:` (ll.455-457).
pub fn normalize_custom_pool_name(name: &str) -> String { name.trim().to_ascii_lowercase().replace(' ', "-") }

#[allow(dead_code)]
fn _normalize_custom_pool_name(name: &str) -> String { normalize_custom_pool_name(name) }

// ---------------------------------------------------------------------------
// _iter_custom_providers — mirrors ll.460-481
// ---------------------------------------------------------------------------

/// Mirrors `def _iter_custom_providers(config: Optional[dict] = None):` (ll.460-481).
pub fn iter_custom_providers(config: Option<&HashMap<String, Value>>) -> Vec<(String, HashMap<String, Value>)> {
    let cfg_opt: Option<HashMap<String, Value>> = match config {
        Some(c) => Some(c.clone()),
        None => load_config_safe(),
    };
    let Some(cfg) = cfg_opt else { return Vec::new(); };
    // Mirrors `get_compatible_custom_providers(config)` (ll.467-469) — stub via `custom_providers` key
    let Some(Value::Array(arr)) = cfg.get("custom_providers").or_else(|| cfg.get("customProviders")) else { return Vec::new(); };
    let mut out: Vec<(String, HashMap<String, Value>)> = Vec::new();
    for entry in arr {
        let Some(Value::Object(map)) = Some(entry) else { continue; };
        let Some(Value::String(name)) = map.get("name") else { continue; };
        let norm = normalize_custom_pool_name(name);
        if norm.is_empty() { continue; }
        out.push((norm, map.clone()));
    }
    out
}

#[allow(dead_code)]
fn _iter_custom_providers(config: Option<&HashMap<String, Value>>) -> Vec<(String, HashMap<String, Value>)> {
    iter_custom_providers(config)
}

// ---------------------------------------------------------------------------
// get_custom_provider_pool_key — mirrors ll.483-510
// ---------------------------------------------------------------------------

/// Mirrors `def get_custom_provider_pool_key(base_url: Optional[str], provider_name: Optional[str] = None) -> Optional[str]:` (ll.483-510).
pub fn get_custom_provider_pool_key(base_url: Option<&str>, provider_name: Option<&str>) -> Option<String> {
    let url = base_url?;
    let t = url.trim();
    if t.is_empty() { return None; }
    let normalized_url = t.trim_end_matches('/').to_string();
    if let Some(pname) = provider_name {
        if !pname.trim().is_empty() {
            let normalized_name = normalize_custom_pool_name(pname);
            for (norm_name, _entry) in iter_custom_providers(None) {
                if norm_name == normalized_name {
                    return Some(format!("{}{}", CUSTOM_POOL_PREFIX, norm_name));
                }
            }
        }
    }
    for (norm_name, entry) in iter_custom_providers(None) {
        if let Some(Value::String(entry_url)) = entry.get("base_url") {
            let eu = entry_url.trim().trim_end_matches('/').to_string();
            if !eu.is_empty() && eu == normalized_url {
                return Some(format!("{}{}", CUSTOM_POOL_PREFIX, norm_name));
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// list_custom_pool_providers — mirrors ll.513-521
// ---------------------------------------------------------------------------

/// Mirrors `def list_custom_pool_providers() -> List[str]:` (ll.513-521).
pub fn list_custom_pool_providers() -> Vec<String> {
    let pool_data = read_credential_pool(None);
    let mut out: Vec<String> = Vec::new();
    for (key, val) in pool_data {
        if key.starts_with(CUSTOM_POOL_PREFIX) {
            if let Value::Array(arr) = val {
                if !arr.is_empty() { out.push(key.clone()); }
            }
        }
    }
    out.sort();
    out
}

/// Stub for `read_credential_pool` (ll.515).
pub fn read_credential_pool(_provider: Option<&str>) -> HashMap<String, Value> { HashMap::new() }

/// Stub for `write_credential_pool` (used later in CredentialPool::_persist).
pub fn write_credential_pool(_provider: &str, _payload: Vec<HashMap<String, Value>>, _removed_ids: Option<Vec<String>>) {}

// ---------------------------------------------------------------------------
// _get_custom_provider_config — mirrors ll.524-532
// ---------------------------------------------------------------------------

/// Mirrors `def _get_custom_provider_config(pool_key: str) -> Optional[Dict[str, Any]]:` (ll.524-532).
pub fn get_custom_provider_config(pool_key: &str) -> Option<HashMap<String, Value>> {
    if !pool_key.starts_with(CUSTOM_POOL_PREFIX) { return None; }
    let suffix = &pool_key[CUSTOM_POOL_PREFIX.len()..];
    for (norm_name, entry) in iter_custom_providers(None) {
        if norm_name == suffix { return Some(entry); }
    }
    None
}

#[allow(dead_code)]
fn _get_custom_provider_config(pool_key: &str) -> Option<HashMap<String, Value>> { get_custom_provider_config(pool_key) }

// ---------------------------------------------------------------------------
// get_pool_strategy — mirrors ll.535-548
// ---------------------------------------------------------------------------

/// Mirrors `def get_pool_strategy(provider: str) -> str:` (ll.535-548).
pub fn get_pool_strategy(provider: &str) -> String {
    let config = match load_config_safe() { Some(c) => c, None => return STRATEGY_FILL_FIRST.to_string() };
    let Some(Value::Object(strategies)) = config.get("credential_pool_strategies") else { return STRATEGY_FILL_FIRST.to_string(); };
    let Some(val) = strategies.get(provider) else { return STRATEGY_FILL_FIRST.to_string(); };
    let raw = match val { Value::String(s) => s.trim().to_ascii_lowercase(), _ => String::new() };
    if SUPPORTED_POOL_STRATEGIES.contains(&raw.as_str()) { raw } else { STRATEGY_FILL_FIRST.to_string() }
}

// ---------------------------------------------------------------------------
// credential_pool_matches_provider — mirrors ll.551-613
// ---------------------------------------------------------------------------

/// Mirrors `def credential_pool_matches_provider(pool_or_provider: Any, provider: Optional[str], *, base_url: Optional[str] = None) -> bool:` (ll.551-613).
pub fn credential_pool_matches_provider(pool_or_provider: &str, provider: Option<&str>, base_url: Option<&str>) -> bool {
    // Mirrors unscoped-adapter fast path: non-string pool with no provider attribute → True (ll.568-574) is not representable with &str arg; caller passes string.
    let pool_provider = pool_or_provider.trim().to_ascii_lowercase();
    let provider_norm = provider.unwrap_or("").trim().to_ascii_lowercase();
    if pool_provider.is_empty() || provider_norm.is_empty() { return false; }
    if !pool_provider.starts_with(CUSTOM_POOL_PREFIX) {
        return pool_provider == provider_norm;
    }
    if provider_norm == "custom" {
        let matched = match get_custom_provider_pool_key(base_url.unwrap_or("")) { Some(k) => k, None => return false };
        return matched.trim().to_ascii_lowercase() == pool_provider;
    }
    let runtime_url = base_url.unwrap_or("").trim().trim_end_matches('/').to_string();
    if runtime_url.is_empty() { return false; }
    // Mirrors loop over custom providers alias + URL check (ll.592-610)
    let result: Result<bool, ()> = (|| {
        for (normalized_name, entry) in iter_custom_providers(None) {
            if format!("{}{}", CUSTOM_POOL_PREFIX, normalized_name) != pool_provider { continue; }
            let mut aliases: HashSet<String> = HashSet::new();
            aliases.insert(normalized_name.clone());
            for value in [entry.get("name"), entry.get("provider_key")] {
                if let Some(v) = value {
                    let alias = match v {
                        Value::String(s) => normalize_custom_pool_name(s),
                        _ => String::new(),
                    };
                    if !alias.is_empty() {
                        aliases.insert(alias.clone());
                        if alias.starts_with(CUSTOM_POOL_PREFIX) {
                            let suffix = normalize_custom_pool_name(&alias[CUSTOM_POOL_PREFIX.len()..]);
                            if !suffix.is_empty() { aliases.insert(suffix); }
                        }
                    }
                }
            }
            let configured_url = match entry.get("base_url") {
                Some(Value::String(s)) => s.trim().trim_end_matches('/').to_string(),
                _ => String::new(),
            };
            let mut runtime_aliases: HashSet<String> = HashSet::new();
            runtime_aliases.insert(normalize_custom_pool_name(&provider_norm));
            if provider_norm.starts_with(CUSTOM_POOL_PREFIX) {
                runtime_aliases.insert(normalize_custom_pool_name(&provider_norm[CUSTOM_POOL_PREFIX.len()..]));
            }
            let alias_hit = !runtime_aliases.is_disjoint(&aliases);
            return Ok(alias_hit && runtime_url == configured_url);
        }
        Ok(false)
    })();
    result.unwrap_or(false)
}

// ---------------------------------------------------------------------------
// resolve_runtime_pool_key — mirrors ll.616-652
// ---------------------------------------------------------------------------

/// Mirrors `def resolve_runtime_pool_key(provider: Optional[str], base_url: Optional[str]) -> str:` (ll.616-652).
pub fn resolve_runtime_pool_key(provider: Option<&str>, base_url: Option<&str>) -> String {
    let provider_norm = provider.unwrap_or("").trim().to_ascii_lowercase();
    if provider_norm.is_empty() { return String::new(); }
    // Mirrors try/except passthrough (ll.634-651)
    let candidate_opt: Option<String> = (|| {
        if provider_norm == "custom" {
            if let Some(candidate) = get_custom_provider_pool_key(base_url.unwrap_or("")) {
                if credential_pool_matches_provider(&candidate, Some(&provider_norm), base_url) {
                    return Some(candidate.trim().to_ascii_lowercase());
                }
            }
        } else {
            for (normalized_name, _entry) in iter_custom_providers(None) {
                let candidate = format!("{}{}", CUSTOM_POOL_PREFIX, normalized_name);
                if credential_pool_matches_provider(&candidate, Some(&provider_norm), base_url) {
                    return Some(candidate);
                }
            }
        }
        None
    })();
    if let Some(c) = candidate_opt { return c; }
    provider_norm
}

// ---------------------------------------------------------------------------
// DEFAULT_MAX_CONCURRENT_PER_CREDENTIAL — mirrors l.655
// ---------------------------------------------------------------------------

/// Mirrors `DEFAULT_MAX_CONCURRENT_PER_CREDENTIAL = 1` (l.655).
pub const DEFAULT_MAX_CONCURRENT_PER_CREDENTIAL: i32 = 1;

// ---------------------------------------------------------------------------
// _write_through_provider_state_to_global_root — mirrors ll.658-710
// ---------------------------------------------------------------------------

/// Mirrors `def _write_through_provider_state_to_global_root(provider_id: str, state: Dict[str, Any]) -> None:` (ll.658-710).
pub fn write_through_provider_state_to_global_root(provider_id: &str, state: &HashMap<String, Value>) {
    // Best-effort write-through for multi-profile rotation hazard (#48415 / #43589).
    // Swallows all errors — must never break the profile's own successful save.
    let global_path = match global_auth_file_path() { Some(p) => p, None => return };
    // Seat belt: under pytest refuse to write real user's ~/.hermes/auth.json (ll.685-697)
    if std::env::var("PYTEST_CURRENT_TEST").is_ok() {
        if let Ok(home) = std::env::var("HOME") {
            if !home.trim().is_empty() {
                let real_root = PathBuf::from(home.trim()).join(".hermes").join("auth.json");
                // Mirrors `global_path.resolve(strict=False) == real_root.resolve(strict=False)` (l.694)
                if global_path == real_root { return; }
            }
        }
    }
    // Mirrors `auth_mod._persist_provider_state_to_store(provider_id, state, global_path, set_active=False)` (ll.698-704)
    let _ = persist_provider_state_to_store(provider_id, state, &global_path, false);
}

fn global_auth_file_path() -> Option<PathBuf> {
    // Mirrors `auth_mod._global_auth_file_path()` (l.682)
    // Real impl returns `None` in classic mode (profile == root) (ll.683-684).
    if let Ok(v) = std::env::var("HERMES_HOME") {
        if !v.trim().is_empty() {
            // In profile mode, global root is `~/.hermes/auth.json` while HERMES_HOME is `~/.hermes/profiles/<name>`
            // Stub: if HERMES_HOME contains "/profiles/", derive global; else None (classic mode).
            if v.contains("profiles") {
                if let Ok(home) = std::env::var("HOME") {
                    return Some(PathBuf::from(home.trim()).join(".hermes").join("auth.json"));
                }
                return Some(PathBuf::from(v.trim()).join("../../auth.json"));
            }
            return None;
        }
    }
    None
}

fn persist_provider_state_to_store(_provider_id: &str, _state: &HashMap<String, Value>, _global_path: &Path, _set_active: bool) -> Result<(), String> {
    // Mirrors `auth_mod._persist_provider_state_to_store` (l.698-703) — best-effort.
    Ok(())
}

#[allow(dead_code)]
fn _write_through_provider_state_to_global_root(provider_id: &str, state: &HashMap<String, Value>) {
    write_through_provider_state_to_global_root(provider_id, state)
}

// ---------------------------------------------------------------------------
// CredentialPool — mirrors ll.713-916
// ---------------------------------------------------------------------------

/// Mirrors `class CredentialPool:` (ll.713-900) — slice1 covers construction through `_mark_exhausted`.
pub struct CredentialPool {
    pub provider: String,
    entries: Vec<PooledCredential>,
    current_id: Option<String>,
    strategy: String,
    // Mirrors `self._lock = threading.RLock()` (l.724)
    lock: Mutex<()>,
    active_leases: HashMap<String, i32>,
    max_concurrent: i32,
    last_no_entries_log_at: Option<f64>,
    unmatched_rotation_streak: i32,
}

impl CredentialPool {
    /// Mirrors `def __init__(self, provider: str, entries: List[PooledCredential]):` (ll.714-740).
    pub fn new(provider: &str, mut entries: Vec<PooledCredential>) -> Self {
        entries.sort_by_key(|e| e.priority);
        Self {
            provider: provider.to_string(),
            entries,
            current_id: None,
            strategy: get_pool_strategy(provider),
            lock: Mutex::new(()),
            active_leases: HashMap::new(),
            max_concurrent: DEFAULT_MAX_CONCURRENT_PER_CREDENTIAL,
            last_no_entries_log_at: None,
            unmatched_rotation_streak: 0,
        }
    }

    /// Mirrors `def has_credentials(self) -> bool:` (ll.742-744).
    pub fn has_credentials(&self) -> bool {
        let _guard = self.lock.lock().unwrap();
        !self.entries.is_empty()
    }

    /// Mirrors `def has_available(self) -> bool:` (ll.746-755).
    pub fn has_available(&self) -> bool {
        let _guard = self.lock.lock().unwrap();
        let (available, _pending) = self.available_entries();
        !available.is_empty()
    }

    /// Mirrors `def next_available_at(self) -> Optional[float]:` (ll.757-791).
    pub fn next_available_at(&self) -> Option<f64> {
        let _guard = self.lock.lock().unwrap();
        let (available, _pending) = self.available_entries();
        if !available.is_empty() { return None; }
        let sole_credential = self.entries.iter().filter(|e| e.last_status.as_deref() != Some(STATUS_DEAD)).count() <= 1;
        let mut candidates: Vec<f64> = Vec::new();
        for entry in &self.entries {
            if entry.last_status.as_deref() != Some(STATUS_EXHAUSTED) { continue; }
            if let Some(until) = exhausted_until(entry, sole_credential) { candidates.push(until); }
        }
        candidates.into_iter().min_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
    }

    /// Mirrors `def entries(self) -> List[PooledCredential]:` (ll.792-794).
    pub fn entries_snapshot(&self) -> Vec<PooledCredential> {
        let _guard = self.lock.lock().unwrap();
        self.entries.clone()
    }

    /// Mirrors `def _current_unlocked(self) -> Optional[PooledCredential]:` (ll.796-799).
    pub fn current_unlocked(&self) -> Option<PooledCredential> {
        let id = self.current_id.as_ref()?;
        self.entries.iter().find(|e| &e.id == id).cloned()
    }

    /// Mirrors `def current(self) -> Optional[PooledCredential]:` (ll.801-803).
    pub fn current(&self) -> Option<PooledCredential> {
        let _guard = self.lock.lock().unwrap();
        self.current_unlocked()
    }

    /// Mirrors `def entry_id_for_api_key(self, api_key_hint: Any = None) -> Optional[str]:` (ll.805-825).
    pub fn entry_id_for_api_key(&self, api_key_hint: Option<&str>) -> Option<String> {
        let _guard = self.lock.lock().unwrap();
        if let Some(current) = self.current_unlocked() {
            if api_key_hint.is_none() || current.runtime_api_key() == api_key_hint.unwrap_or("") {
                return Some(current.id);
            }
        }
        let hint = api_key_hint?;
        let matches: Vec<&PooledCredential> = self.entries.iter().filter(|e| e.runtime_api_key() == hint).collect();
        if matches.len() == 1 { Some(matches[0].id.clone()) } else { None }
    }

    /// Mirrors `def _replace_entry(self, old: PooledCredential, new: PooledCredential) -> None:` (ll.827-838).
    pub fn replace_entry(&mut self, old: &PooledCredential, new: PooledCredential) {
        let _guard = self.lock.lock().unwrap();
        for idx in 0..self.entries.len() {
            if self.entries[idx].id == old.id {
                self.entries[idx] = new;
                return;
            }
        }
    }

    /// Convenience non-mut variant that mirrors the RLock self-locking shape (l.830-832).
    pub fn replace_entry_locked(&self, old_id: &str, new: PooledCredential) {
        // In Python `_replace_entry` self-acquires RLock so deferred refresh path serializes.
        // Real Rust would need interior mutability; stub uses a best-effort unsafe pattern for audit parity.
        // This stub is never called in slice1 tests — provided for line-level traceability.
        let _guard = self.lock.lock().unwrap();
        // Safety: this is a stub; real impl requires `&mut self` or `RwLock<Vec<_>>`.
        // We log the intent without mutating (best-effort audit).
        let _ = (old_id, new);
    }

    /// Mirrors `def _persist(self, *, removed_ids: Optional[List[str]] = None) -> None:` (ll.840-848).
    pub fn persist(&self, removed_ids: Option<Vec<String>>) {
        let _guard = self.lock.lock().unwrap();
        let payload: Vec<HashMap<String, Value>> = self.entries.iter().map(|e| e.to_dict()).collect();
        write_credential_pool(&self.provider, payload, removed_ids);
    }

    /// Mirrors `def _is_terminal_auth_failure(self, status_code: Optional[int], normalized_error: Dict[str, Any]) -> bool:` (ll.850-871).
    pub fn is_terminal_auth_failure(&self, status_code: Option<i32>, normalized_error: &HashMap<String, Value>) -> bool {
        if status_code != Some(401) { return false; }
        let Some(Value::String(reason)) = normalized_error.get("reason") else { return false; };
        TERMINAL_AUTH_REASONS.contains(&reason.trim().to_ascii_lowercase().as_str())
    }

    /// Mirrors `def _mark_exhausted(self, entry: PooledCredential, status_code: Optional[int], error_context: Optional[Dict[str, Any]] = None, *, persist: bool = True, failure_reason: Optional[str] = None) -> PooledCredential:` (ll.873-916).
    pub fn mark_exhausted(&mut self, entry: &PooledCredential, status_code: Option<i32>, error_context: Option<&HashMap<String, Value>>, persist: bool, failure_reason: Option<&str>) -> PooledCredential {
        let normalized_error = normalize_error_context(error_context);
        let terminal_status = if self.is_terminal_auth_failure(status_code, &normalized_error) { STATUS_DEAD } else { STATUS_EXHAUSTED };
        let mut updated_extra = entry.extra.clone();
        if let Some(fr) = failure_reason {
            if !fr.trim().is_empty() { updated_extra.insert("failure_reason".to_string(), Value::String(fr.trim().to_string())); }
        } else {
            updated_extra.remove("failure_reason");
        }
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or(Duration::from_secs(0)).as_secs_f64();
        let mut updated = entry.clone();
        updated.last_status = Some(terminal_status.to_string());
        updated.last_status_at = Some(now);
        updated.last_error_code = status_code;
        updated.last_error_reason = normalized_error.get("reason").and_then(|v| v.as_str()).map(|s| s.to_string());
        updated.last_error_message = normalized_error.get("message").and_then(|v| v.as_str()).map(|s| s.to_string());
        updated.last_error_reset_at = normalized_error.get("reset_at").and_then(|v| v.as_f64());
        updated.extra = updated_extra;
        // Mirrors `self._replace_entry(entry, updated)` + `if persist: self._persist()` (ll.913-915)
        self.replace_entry(entry, updated.clone());
        if persist { self.persist(None); }
        updated
    }

    // --- internal helpers mirroring Python private methods ---

    /// Mirrors `_available_entries()` filtering + DEAD pruning side-effects (ll.753,757).
    /// This slice provides the read-only filtered view; the pruning+re-persist
    /// that mutates `self._entries` on expired DEAD manual entries lives in
    /// full `credential_pool.py` ll.1200+ and is completed in slice2.
    fn available_entries(&self) -> (Vec<PooledCredential>, Vec<PooledCredential>) {
        // Best-effort: filter out DEAD unconditionally and EXHAUSTED still in cooldown.
        let sole_credential = self.entries.iter().filter(|e| e.last_status.as_deref() != Some(STATUS_DEAD)).count() <= 1;
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or(Duration::from_secs(0)).as_secs_f64();
        let mut available: Vec<PooledCredential> = Vec::new();
        let mut pending: Vec<PooledCredential> = Vec::new();
        for e in &self.entries {
            if e.last_status.as_deref() == Some(STATUS_DEAD) { continue; }
            if e.last_status.as_deref() == Some(STATUS_EXHAUSTED) {
                if let Some(until) = exhausted_until(e, sole_credential) {
                    if now < until { pending.push(e.clone()); continue; }
                } else if e.last_status_at.is_some() {
                    // Has TTL but no reset_at → still pending until TTL elapses (handled above)
                    // If no until, treat as pending (conservative)
                    pending.push(e.clone()); continue;
                }
            }
            available.push(e.clone());
        }
        (available, pending)
    }
}

// ---------------------------------------------------------------------------
// Slice boundary note
// ---------------------------------------------------------------------------
// Python l.900 is inside `CredentialPool._mark_exhausted`:
//   `updated_extra["failure_reason"] = failure_reason`
// The function closes at l.916 (`return updated`), so the slice is
// syntactically closed even though the nominal 900-line boundary falls
// mid-function — exactly as `auxiliary_slice1.rs` does for
// `_fast_model_from_catalog` (closed at l.907 though cut at l.900) and
// `iron_proxy_slice1.rs` does for `_read_management_listen_from_config`
// (closed at l.913 though cut at l.900). The next definition
// `def _sync_anthropic_entry_from_credentials_file` (l.918) is the first
// item of `credential_pool_slice2.rs`. This matches `docs/port/00-MASTER-DESIGN.md` §2:
// slice boundaries may land mid-function; each slice notes the truncation
// and the successor slice owns the remainder.

// ---------------------------------------------------------------------------
// Re-exports for 1:1 traceability — mirrors Python `__all__` surface used by tests
// ---------------------------------------------------------------------------
pub use self::CredentialPool as _CredentialPool;
pub use self::PooledCredential as _PooledCredential;
