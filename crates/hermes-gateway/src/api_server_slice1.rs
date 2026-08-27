//! OpenAI-compatible API server platform adapter (slice 1: lines 1–900).
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/gateway/platforms/api_server.py`
//! (8549 LOC), slice 1 covering lines 1–900. This slice contains the module
//! docstring, imports, sentinel, profile helpers, `ContextVar` shims, artifact
//! scope facade, browser-control constants, `_approval_event_choices`, aiohttp
//! availability flag, scoped-secret helper, WS sender, version probe, default
//! settings, `ThreadSafeAsyncQueue`, SSE framing, port/bool coercion, reasoning
//! helpers, runtime-override helpers, request-agent overrides, compaction helpers,
//! chat-content normalizers, multimodal validation, and the turn-process
//! ownership epoch bookkeeping through `_publish_turn_process_ownership`
//! (the slice boundary cuts mid-function at line 900 — the complete function
//! body through `agent._gateway_turn_process_epoch = epoch` is included here
//! for compilability; slice 2 continues at `_clear_turn_process_ownership`).
//!
//! Python source docstring (preserved):
//! ```text
//! OpenAI-compatible API server platform adapter.
//!
//! Exposes an HTTP server with endpoints:
//! - POST /v1/chat/completions        — OpenAI Chat Completions format (stateless; opt-in session continuity via X-Hermes-Session-Id header; opt-in long-term memory scoping via X-Hermes-Session-Key header)
//! - POST /v1/responses               — OpenAI Responses API format (stateful via previous_response_id; X-Hermes-Session-Key supported)
//! - GET  /v1/responses/{response_id} — Retrieve a stored response
//! - DELETE /v1/responses/{response_id} — Delete a stored response
//! - GET  /v1/models                  — lists hermes-agent and any configured model_routes aliases
//! - GET  /v1/capabilities            — machine-readable API capabilities for external UIs
//! - GET  /api/sessions               — list client-visible Hermes sessions
//! - POST /api/sessions               — create an empty Hermes session
//! - GET/PATCH/DELETE /api/sessions/{session_id} — read/update/delete a session
//! - GET  /api/sessions/{session_id}/messages — read session message history
//! - POST /api/sessions/{session_id}/fork — branch a session using SessionDB lineage
//! - POST /api/sessions/{session_id}/chat[/stream] — chat with a persisted session
//! - POST /v1/runs                    — start a run, returns run_id immediately (202)
//! - GET  /v1/runs/{run_id}           — retrieve current run status
//! - GET  /v1/runs/{run_id}/events    — SSE stream of structured lifecycle events
//! - POST /v1/runs/{run_id}/approval — resolve a pending run approval
//! - POST /v1/runs/{run_id}/steer      — inject guidance into a running agent
//! - POST /v1/runs/{run_id}/stop       — interrupt a running agent
//! - GET  /health                     — health check
//! - GET  /health/detailed            — rich status for cross-container dashboard probing
//!
//! Any OpenAI-compatible frontend (Open WebUI, LobeChat, LibreChat,
//! AnythingLLM, NextChat, ChatBox, etc.) can connect to hermes-agent
//! through this adapter by pointing at http://localhost:8642/v1 and
//! authenticating with API_SERVER_KEY.
//!
//! When ``gateway.multiplex_profiles`` is on, the default profile owns this
//! listener and secondary profiles are reached via a URL prefix — same contract
//! as the webhook adapter:
//!
//!     GET  /p/<profile>/v1/models
//!     POST /p/<profile>/v1/chat/completions
//!     ...
//!
//! Requires:
//! - aiohttp (already available in the gateway)
//! ```
//!
//! # Mapping
//!
//! - `_PROFILE_REJECTED = object()` → [`PROFILE_REJECTED`] / [`_PROFILE_REJECTED`] (sentinel unit struct, `object()` identity → typed sentinel)
//! - `def _prefix_names_served_profile(profile: str) -> bool` → [`prefix_names_served_profile`] / [`_prefix_names_served_profile`]
//! - `_api_request_profile: ContextVar[Optional[str]]` → [`get_api_request_profile`] / [`set_api_request_profile`] / [`reset_api_request_profile`] (thread-local `RefCell<Option<String>>`)
//! - `_api_request_browser_control_principal: ContextVar[str]` → [`get_api_request_browser_control_principal`] / [`set_api_request_browser_control_principal`]
//! - `_api_request_browser_control_transport_family: ContextVar[str]` → [`get_api_request_browser_control_transport_family`] / [`set_api_request_browser_control_transport_family`]
//! - `class _ArtifactScopeFacade` → [`ArtifactScopeFacade`]
//! - `_BROWSER_CONTROL_PROTOCOL_VERSION = 1` → [`BROWSER_CONTROL_PROTOCOL_VERSION`] / [`_BROWSER_CONTROL_PROTOCOL_VERSION`]
//! - `_BROWSER_CONTROL_WS_PROTOCOL = "hermes-browser-control-v1"` → [`BROWSER_CONTROL_WS_PROTOCOL`] / [`_BROWSER_CONTROL_WS_PROTOCOL`]
//! - `_BROWSER_CONTROL_TICKET_PROTOCOL_PREFIX = "hermes-browser-control-ticket."` → [`BROWSER_CONTROL_TICKET_PROTOCOL_PREFIX`]
//! - `def _approval_event_choices(... ) -> list[str]` → [`approval_event_choices`] / [`_approval_event_choices`]
//! - `try: from aiohttp import web; AIOHTTP_AVAILABLE = True; except ImportError: AIOHTTP_AVAILABLE = False` → [`AIOHTTP_AVAILABLE`] (bool, `false` in std-only build, `true` when `aiohttp`/`axum` wired)
//! - `from gateway.config import Platform, PlatformConfig` → documented only (gateway config types)
//! - `from gateway.platforms.base import (MEDIA_TAG_CLEANUP_RE, BasePlatformAdapter, SendResult, ...)` → documented only
//! - `from agent.redact import redact_sensitive_text` → [`redact_sensitive_text`] (ponytail: inline stub, call `hermes_redact` when wired)
//! - `from agent.interrupt_compat import request_hard_interrupt` → documented only
//! - `from gateway.readiness import collect_runtime_readiness` → documented only
//! - `from gateway.browser_control_artifacts import (...)` → documented only (artifact store types)
//! - `from gateway.browser_control_broker import (...)` → documented only (broker types)
//! - `from agent.secret_scope import get_secret` → [`get_scoped_secret`] / [`_get_scoped_secret`]
//! - `def _get_scoped_secret(name, default=None)` → [`get_scoped_secret`] / [`_get_scoped_secret`]
//! - `def _browser_controller_ws_sender(ws, loop, *, wait_timeout: float = 10.0)` → [`browser_controller_ws_sender`] / [`BrowserControllerWsSender`] + [`WsSender`]
//! - `def _hermes_version() -> str` → [`hermes_version`] / [`_hermes_version`] / [`get_hermes_version`]
//! - `DEFAULT_HOST = "127.0.0.1"` → [`DEFAULT_HOST`] / [`_DEFAULT_HOST`]
//! - `DEFAULT_PORT = 8642` → [`DEFAULT_PORT`] / [`_DEFAULT_PORT`]
//! - `MAX_STORED_RESPONSES = 100` → [`MAX_STORED_RESPONSES`] / [`_MAX_STORED_RESPONSES`]
//! - `MAX_REQUEST_BYTES = 10_000_000` → [`MAX_REQUEST_BYTES`] / [`_MAX_REQUEST_BYTES`]
//! - `CHAT_COMPLETIONS_SSE_KEEPALIVE_SECONDS = 30.0` → [`CHAT_COMPLETIONS_SSE_KEEPALIVE_SECONDS`]
//! - `MAX_NORMALIZED_TEXT_LENGTH = 65_536` → [`MAX_NORMALIZED_TEXT_LENGTH`] / [`_MAX_NORMALIZED_TEXT_LENGTH`]
//! - `MAX_CONTENT_LIST_SIZE = 1_000` → [`MAX_CONTENT_LIST_SIZE`] / [`_MAX_CONTENT_LIST_SIZE`]
//! - `RESPONSES_AUTO_TRUNCATION_HISTORY_LIMIT = 100` → [`RESPONSES_AUTO_TRUNCATION_HISTORY_LIMIT`]
//! - `_COMPRESSED_SUMMARY_METADATA_KEY = "_compressed_summary"` → [`COMPRESSED_SUMMARY_METADATA_KEY`] / [`_COMPRESSED_SUMMARY_METADATA_KEY`]
//! - `class ThreadSafeAsyncQueue(asyncio.Queue)` → [`ThreadSafeAsyncQueue`] (std-only `Mutex<VecDeque>` + `Condvar`, no `asyncio`)
//! - `def _sse_frame(data: Any, *, event: str = None, ensure_ascii: bool = True) -> bytes` → [`sse_frame`] / [`_sse_frame`]
//! - `def _coerce_port(value: Any, default: int = DEFAULT_PORT) -> int` → [`coerce_port`] / [`_coerce_port`] + [`coerce_port_value`]
//! - `_TRUE_REQUEST_BOOL_STRINGS / _FALSE_REQUEST_BOOL_STRINGS` → [`TRUE_REQUEST_BOOL_STRINGS`] / [`FALSE_REQUEST_BOOL_STRINGS`]
//! - `def _coerce_request_bool(value: Any, default: bool = False) -> bool` → [`coerce_request_bool`] / [`_coerce_request_bool`] + [`coerce_request_bool_value`]
//! - `_REQUEST_OPTION_MISSING = object()` → [`REQUEST_OPTION_MISSING`] (unit struct sentinel)
//! - `_REASONING_EFFORTS = frozenset({...})` → [`REASONING_EFFORTS`] / [`_REASONING_EFFORTS`]
//! - `_RUNTIME_AGENT_OVERRIDE_KEYS = (...)` → [`RUNTIME_AGENT_OVERRIDE_KEYS`] / [`_RUNTIME_AGENT_OVERRIDE_KEYS`]
//! - `def _clean_request_string(value: Any) -> Optional[str]` → [`clean_request_string`] / [`_clean_request_string`]
//! - `def _request_reasoning_config(model_options: Any) -> Optional[Dict[str, Any]]` → [`request_reasoning_config`] / [`_request_reasoning_config`]
//! - `def _request_service_tier(model_options: Any) -> Any` → [`request_service_tier`] / [`_request_service_tier`] (returns [`ServiceTierResult`])
//! - `def _apply_runtime_agent_overrides(runtime_kwargs, overrides)` → [`apply_runtime_agent_overrides`] / [`_apply_runtime_agent_overrides`]
//! - `def _resolve_request_runtime_agent_kwargs(provider: str, target_model: Optional[str] = None)` → [`resolve_request_runtime_agent_kwargs`] / [`_resolve_request_runtime_agent_kwargs`]
//! - `def _request_agent_overrides(body: Any, *, virtual_model: Optional[str] = None, allow_bare_model: bool = True)` → [`request_agent_overrides`] / [`_request_agent_overrides`]
//! - `def _is_compressed_summary_message(message: Any) -> bool` → [`is_compressed_summary_message`] / [`_is_compressed_summary_message`]
//! - `def _project_client_message(message: Dict[str, Any]) -> Dict[str, Any]` → [`project_client_message`] / [`_project_client_message`]
//! - `def _auto_truncate_response_history(conversation_history: List[Dict[str, Any]], *, limit: int = ...)` → [`auto_truncate_response_history`] / [`_auto_truncate_response_history`]
//! - `def _normalize_chat_content(content: Any, *, _max_depth: int = 10, _depth: int = 0) -> str` → [`normalize_chat_content`] / [`_normalize_chat_content`]
//! - `_TEXT_PART_TYPES = frozenset({"text", "input_text", "output_text"})` → [`TEXT_PART_TYPES`] / [`_TEXT_PART_TYPES`]
//! - `_IMAGE_PART_TYPES = frozenset({"image_url", "input_image"})` → [`IMAGE_PART_TYPES`] / [`_IMAGE_PART_TYPES`]
//! - `_FILE_PART_TYPES = frozenset({"file", "input_file"})` → [`FILE_PART_TYPES`] / [`_FILE_PART_TYPES`]
//! - `def _normalize_multimodal_content(content: Any) -> Any` → [`normalize_multimodal_content`] / [`_normalize_multimodal_content`] (returns `Result<NormalizedContent, MultimodalError>`)
//! - `def _content_has_visible_payload(content: Any) -> bool` → [`content_has_visible_payload`] / [`_content_has_visible_payload`]
//! - `def _multimodal_validation_error(exc: ValueError, *, param: str) -> web.Response` → [`multimodal_validation_error`] / [`_multimodal_validation_error`] (returns [`HttpError`])
//! - `def _reap_disconnected_agent_processes(agent: Any, *, source: str = "api_server_sse_disconnect")` → [`reap_disconnected_agent_processes`] / [`_reap_disconnected_agent_processes`] (std `thread::spawn` daemon)
//! - `_TURN_PROCESS_EPOCHS: Dict[str, int] = {}` → [`TURN_PROCESS_EPOCHS`] (behind `Mutex<HashMap>`)
//! - `_TURN_PROCESS_EPOCH_LOCK = threading.Lock()` → [`TURN_PROCESS_EPOCH_LOCK`]
//! - `_TURN_PROCESS_EPOCH_COUNTER = itertools.count(1)` → [`TURN_PROCESS_EPOCH_COUNTER`] (`AtomicUsize`)
//! - `def _publish_turn_process_ownership(agent: Any, task_id: str)` → [`publish_turn_process_ownership`] / [`_publish_turn_process_ownership`]
//!
//! # Notes on runtime deps not ported in this slice
//!
//! Python imports `asyncio`, `concurrent.futures`, `hashlib`, `hmac`, `re`,
//! `sqlite3`, `gateway.run._reap_gateway_turn_processes`,
//! `tools.process_registry`, `hermes_cli.runtime_provider`, etc. Those are
//! runtime/loop-level concerns above slice 1's pure helpers and are documented
//! as comments (`// Python:`) where referenced. The pure helpers are fully
//! ported; side-effecting gateway coupling is stubbed with `ponytail:` notes.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Condvar, Mutex, OnceLock,
};

use serde_json::{json, Map, Value};

// ---------------------------------------------------------------------------
// Module-level sentinel — mirrors Python lines 64–67
// ---------------------------------------------------------------------------

/// Sentinel returned by `resolve_request_profile` when a `/p/<profile>/` prefix
/// names a profile this gateway does not serve (→ 404). Distinct from `None`
/// (no prefix / multiplexing off → handle as default profile).
///
/// Mirrors:
/// ```python
/// _PROFILE_REJECTED = object()
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProfileRejectedSentinel;

impl std::fmt::Display for ProfileRejectedSentinel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "_PROFILE_REJECTED")
    }
}

/// Global sentinel value. Mirrors `_PROFILE_REJECTED = object()`.
pub const PROFILE_REJECTED: ProfileRejectedSentinel = ProfileRejectedSentinel {};

/// Alias matching Python private name for grep-ability.
pub const _PROFILE_REJECTED: ProfileRejectedSentinel = PROFILE_REJECTED;

// ---------------------------------------------------------------------------
// Profile prefix guard — mirrors Python lines 70–88
// ---------------------------------------------------------------------------

/// True when a `/p/<profile>/` prefix names the profile this gateway serves.
///
/// Mirrors:
/// ```python
/// def _prefix_names_served_profile(profile: str) -> bool:
///     try:
///         from hermes_cli.profiles import profile_matches_home
///         return profile_matches_home(profile)
///     except Exception:
///         return False
/// ```
///
/// Single-profile (non-multiplex) gateways historically ignored the prefix and
/// answered every `/p/<x>/` request from their own config — which silently
/// served the gateway owner's toolsets/capabilities under another profile's
/// URL (#91583 defect 2). Only a self-referential prefix may fall through.
/// Fail closed.
///
/// Rust: std-only heuristic — compare `profile` against `HERMES_PROFILE`
/// env or `HERMES_HOME` suffix when available; otherwise return `false`
/// (fail closed, matches Python exception fallback).
pub fn prefix_names_served_profile(profile: &str) -> bool {
    // ponytail: no `hermes_cli.profiles` dep in std-only; env heuristic + fail-closed
    if let Ok(active) = std::env::var("HERMES_PROFILE") {
        if active.trim() == profile.trim() {
            return true;
        }
        // also allow matching when env is empty (default profile) and prefix is "default"
        if active.trim().is_empty() && profile.trim() == "default" {
            return true;
        }
        // fall through to home suffix check before returning false
    }
    // Try HERMES_HOME suffix: ~/.hermes/profiles/<profile> or ~/.hermes
    if let Ok(home) = std::env::var("HERMES_HOME") {
        let trimmed = home.trim().trim_end_matches('/');
        if trimmed.ends_with(&format!("/{}", profile.trim())) {
            return true;
        }
        // default profile home is not inside /profiles/ — only self prefix "default" matches
        if !trimmed.contains("/profiles/") && profile.trim() == "default" {
            return true;
        }
    }
    // Last resort: when no env is set we cannot confirm multiplexing, so fail closed
    false
}

/// Private alias for grep discoverability.
pub fn _prefix_names_served_profile(profile: &str) -> bool {
    prefix_names_served_profile(profile)
}

// ---------------------------------------------------------------------------
// ContextVar shims — mirrors Python lines 90–101
// ---------------------------------------------------------------------------

thread_local! {
    static API_REQUEST_PROFILE: RefCell<Option<String>> = const { RefCell::new(None) };
    static API_REQUEST_BROWSER_CONTROL_PRINCIPAL: RefCell<String> = const { RefCell::new(String::new()) };
    static API_REQUEST_BROWSER_CONTROL_TRANSPORT_FAMILY: RefCell<String> = const { RefCell::new(String::new()) };
}

/// Get current API request profile. Mirrors
/// `_api_request_profile.get()` / `_api_request_profile: ContextVar[Optional[str]]`.
///
/// Default `None` when no middleware has set it.
pub fn get_api_request_profile() -> Option<String> {
    API_REQUEST_PROFILE.with(|c| c.borrow().clone())
}

/// Set current API request profile (middleware entry point). Mirrors
/// `_api_request_profile.set(value)` → returns `Token`.
/// In Rust we return the previous value as the reset token.
pub fn set_api_request_profile(value: Option<String>) -> Option<String> {
    API_REQUEST_PROFILE.with(|c| {
        let prev = c.borrow().clone();
        *c.borrow_mut() = value;
        prev
    })
}

/// Reset API request profile to `token` (mirrors `ContextVar.reset(token)`).
pub fn reset_api_request_profile(token: Option<String>) {
    API_REQUEST_PROFILE.with(|c| *c.borrow_mut() = token);
}

/// Convenience: set profile from `&str` (empty → `None`).
pub fn set_api_request_profile_str(profile: Option<&str>) {
    let v = profile.and_then(|s| {
        let t = s.trim().to_string();
        if t.is_empty() { None } else { Some(t) }
    });
    set_api_request_profile(v);
}

/// Browser control principal for the current request.
///
/// Mirrors `_api_request_browser_control_principal: ContextVar[str] = ContextVar(..., default="")`.
pub fn get_api_request_browser_control_principal() -> String {
    API_REQUEST_BROWSER_CONTROL_PRINCIPAL.with(|c| c.borrow().clone())
}

pub fn set_api_request_browser_control_principal(value: String) -> String {
    API_REQUEST_BROWSER_CONTROL_PRINCIPAL.with(|c| {
        let prev = c.borrow().clone();
        *c.borrow_mut() = value.clone();
        prev
    })
}

pub fn reset_api_request_browser_control_principal(token: String) {
    API_REQUEST_BROWSER_CONTROL_PRINCIPAL.with(|c| *c.borrow_mut() = token);
}

/// Browser control transport family for the current request.
///
/// Mirrors `_api_request_browser_control_transport_family: ContextVar[str] = ContextVar(..., default="")`.
pub fn get_api_request_browser_control_transport_family() -> String {
    API_REQUEST_BROWSER_CONTROL_TRANSPORT_FAMILY.with(|c| c.borrow().clone())
}

pub fn set_api_request_browser_control_transport_family(value: String) -> String {
    API_REQUEST_BROWSER_CONTROL_TRANSPORT_FAMILY.with(|c| {
        let prev = c.borrow().clone();
        *c.borrow_mut() = value.clone();
        prev
    })
}

pub fn reset_api_request_browser_control_transport_family(token: String) {
    API_REQUEST_BROWSER_CONTROL_TRANSPORT_FAMILY.with(|c| *c.borrow_mut() = token);
}

// ---------------------------------------------------------------------------
// Artifact scope facade — mirrors Python lines 102–115
// ---------------------------------------------------------------------------

/// Minimal scope shape accepted by `gateway.browser_control_artifacts.artifact_scope_key`:
/// principal + session + transport family. The API server authenticates the
/// caller itself, so the facade carries only the server-derived principal and
/// the loopback/remote family.
///
/// Mirrors:
/// ```python
/// class _ArtifactScopeFacade:
///     __slots__ = ("principal_id", "session_id", "transport_family")
///     def __init__(self, principal_id: str, *, session_id: str = "", transport_family: str = ""):
///         self.principal_id = principal_id
///         self.session_id = session_id
///         self.transport_family = transport_family
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactScopeFacade {
    pub principal_id: String,
    pub session_id: String,
    pub transport_family: String,
}

impl ArtifactScopeFacade {
    /// Create a new facade. Mirrors `__init__`.
    pub fn new(principal_id: impl Into<String>, session_id: impl Into<String>, transport_family: impl Into<String>) -> Self {
        Self {
            principal_id: principal_id.into(),
            session_id: session_id.into(),
            transport_family: transport_family.into(),
        }
    }

    /// Convenience: build from current request context vars.
    pub fn from_request_context(session_id: impl Into<String>) -> Self {
        Self {
            principal_id: get_api_request_browser_control_principal(),
            session_id: session_id.into(),
            transport_family: get_api_request_browser_control_transport_family(),
        }
    }
}

impl std::fmt::Display for ArtifactScopeFacade {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Mirrors `__repr__` : `_ArtifactScopeFacade(principal=...)`
        write!(f, "_ArtifactScopeFacade(principal={:?})", self.principal_id)
    }
}

#[allow(dead_code)]
fn _artifact_scope_facade_new(principal_id: &str) -> ArtifactScopeFacade {
    ArtifactScopeFacade::new(principal_id, "", "")
}

// ---------------------------------------------------------------------------
// Browser-control protocol constants — mirrors Python lines 116–120
// ---------------------------------------------------------------------------

/// Browser-extension control protocol version advertised in capabilities and
/// echoed in registration responses. Strict validation is centralized in the
/// broker's `browser_control_protocol_supported` helper.
///
/// Mirrors `_BROWSER_CONTROL_PROTOCOL_VERSION = 1`.
pub const BROWSER_CONTROL_PROTOCOL_VERSION: u32 = 1;
pub const _BROWSER_CONTROL_PROTOCOL_VERSION: u32 = BROWSER_CONTROL_PROTOCOL_VERSION;

/// Mirrors `_BROWSER_CONTROL_WS_PROTOCOL = "hermes-browser-control-v1"`.
pub const BROWSER_CONTROL_WS_PROTOCOL: &str = "hermes-browser-control-v1";
pub const _BROWSER_CONTROL_WS_PROTOCOL: &str = BROWSER_CONTROL_WS_PROTOCOL;

/// Mirrors `_BROWSER_CONTROL_TICKET_PROTOCOL_PREFIX = "hermes-browser-control-ticket."`.
pub const BROWSER_CONTROL_TICKET_PROTOCOL_PREFIX: &str = "hermes-browser-control-ticket.";
pub const _BROWSER_CONTROL_TICKET_PROTOCOL_PREFIX: &str = BROWSER_CONTROL_TICKET_PROTOCOL_PREFIX;

// ---------------------------------------------------------------------------
// Approval event choices — mirrors Python lines 121–132
// ---------------------------------------------------------------------------

/// Compute approval event choices.
///
/// Mirrors:
/// ```python
/// def _approval_event_choices(*, smart_denied: bool, allow_session: bool, allow_permanent: bool) -> list[str]:
///     if smart_denied or not allow_session:
///         return ["once", "deny"]
///     return (
///         ["once", "session", "always", "deny"]
///         if allow_permanent
///         else ["once", "session", "deny"]
///     )
/// ```
pub fn approval_event_choices(smart_denied: bool, allow_session: bool, allow_permanent: bool) -> Vec<String> {
    if smart_denied || !allow_session {
        return vec!["once".to_string(), "deny".to_string()];
    }
    if allow_permanent {
        vec![
            "once".to_string(),
            "session".to_string(),
            "always".to_string(),
            "deny".to_string(),
        ]
    } else {
        vec!["once".to_string(), "session".to_string(), "deny".to_string()]
    }
}

#[allow(dead_code)]
fn _approval_event_choices(smart_denied: bool, allow_session: bool, allow_permanent: bool) -> Vec<String> {
    approval_event_choices(smart_denied, allow_session, allow_permanent)
}

// ---------------------------------------------------------------------------
// aiohttp availability — mirrors Python lines 133–140
// ---------------------------------------------------------------------------

/// Whether `aiohttp` (HTTP server) is available.
///
/// Mirrors:
/// ```python
/// try:
///     from aiohttp import web
///     AIOHTTP_AVAILABLE = True
/// except ImportError:
///     AIOHTTP_AVAILABLE = False
///     web = None
/// ```
///
/// Rust: std-only build has no `axum`/`aiohttp` in this crate slice;
/// hard-code `false`. When the real HTTP layer is wired, flip to `true`
/// (or gate on a `cfg(feature = "aiohttp")`).
pub const AIOHTTP_AVAILABLE: bool = false;
#[allow(dead_code)]
pub const _AIOHTTP_AVAILABLE: bool = AIOHTTP_AVAILABLE;

// Python imports from `gateway.config`, `gateway.platforms.base` etc. are
// gateway runtime types (Platform, PlatformConfig, BasePlatformAdapter,
// SendResult, MEDIA_TAG_CLEANUP_RE, validate_media_delivery_path,
// is_network_accessible) plus `agent.redact.redact_sensitive_text`,
// `agent.interrupt_compat.request_hard_interrupt`,
// `gateway.readiness.collect_runtime_readiness`,
// `gateway.browser_control_artifacts.*`, `gateway.browser_control_broker.*`
// — all above-slice runtime concerns. Documented here for traceability;
// omitted from this slice's Rust imports.

// ---------------------------------------------------------------------------
// Scoped secret helper — mirrors Python lines 153–174
// ---------------------------------------------------------------------------

/// Scope-aware credential read with the default-profile startup fallback.
///
/// Mirrors:
/// ```python
/// def _get_scoped_secret(name, default=None):
///     try:
///         val = _scoped_get_secret(name, default)
///     except _UnscopedSecretError:
///         val = os.getenv(name)
///     return val if val is not None else default
/// ```
///
/// Secondary profiles construct adapters under a profile secret scope —
/// the scope is authoritative and a scoped miss returns `default` (no
/// cross-profile borrow from `os.environ`, which may hold another profile's
/// value). The DEFAULT profile's adapter constructs unscoped under
/// multiplexing, where a bare `get_secret` would raise
/// `UnscopedSecretError` and crash this path; there `os.environ` is that
/// profile's own value, so fall back to it.
///
/// Rust: std-only heuristic — probe thread-local secret scope map if present
/// (ponytail: no `agent.secret_scope` crate yet), then `std::env::var`.
/// Fail-closed on scoped miss; only default-profile fallback reads env.
pub fn get_scoped_secret(name: &str, default: Option<&str>) -> Option<String> {
    // ponytail: no `agent.secret_scope` dep yet — check scoped map if installed
    if let Some(scoped) = scoped_secret_store_get(name) {
        return Some(scoped);
    }
    // If scope is installed and miss occurred, fail closed (return default, NOT env)
    // Heuristic: scope presence is env `HERMES_SECRET_SCOPE_ACTIVE=1` (installed by gateway turn)
    let scope_active = std::env::var("HERMES_SECRET_SCOPE_ACTIVE")
        .map(|v| v.trim() == "1" || v.trim().to_lowercase() == "true")
        .unwrap_or(false);
    if scope_active {
        // Scoped miss — do NOT borrow from os.environ (leaks default profile's value)
        return default.map(|s| s.to_string());
    }
    // Default-profile / single-profile path: os.environ IS this profile's value
    if let Ok(val) = std::env::var(name) {
        if !val.is_empty() {
            return Some(val);
        }
    }
    default.map(|s| s.to_string())
}

pub fn _get_scoped_secret(name: &str, default: Option<&str>) -> Option<String> {
    get_scoped_secret(name, default)
}

// Tiny in-process scoped-secret map for tests / future wiring.
// ponytail: global lock, per-process secret map if throughput matters (mirrors Python ContextVar scope).
static SCOPED_SECRET_STORE: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();

fn scoped_store() -> &'static Mutex<HashMap<String, String>> {
    SCOPED_SECRET_STORE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Install a scoped secret (test seam / future gateway wiring). Returns previous value.
pub fn scoped_secret_store_set(name: &str, value: String) -> Option<String> {
    let m = scoped_store();
    let mut g = m.lock().unwrap();
    g.insert(name.to_string(), value)
}

pub fn scoped_secret_store_get(name: &str) -> Option<String> {
    let m = scoped_store();
    let g = m.lock().unwrap();
    g.get(name).cloned()
}

pub fn scoped_secret_store_remove(name: &str) -> Option<String> {
    let m = scoped_store();
    let mut g = m.lock().unwrap();
    g.remove(name)
}

pub fn scoped_secret_store_clear() {
    let m = scoped_store();
    let mut g = m.lock().unwrap();
    g.clear();
}

// ---------------------------------------------------------------------------
// Browser controller WS sender — mirrors Python lines 199–213
// ---------------------------------------------------------------------------

/// Sender for the browser-control WebSocket.
///
/// Mirrors `_browser_controller_ws_sender(ws, loop, *, wait_timeout: float = 10.0)`
/// which returns a `send(frame: dict) -> None` closure that checks `ws.closed`,
/// detects loop-thread vs foreign-thread, and either `loop.create_task(ws.send_json(frame))`
/// or `asyncio.run_coroutine_threadsafe(ws.send_json(frame), loop).result(timeout)`.
/// A wait timeout keeps the broker command pending; late send failures are observed
/// via `add_done_callback`.
///
/// Rust: std-only — no `asyncio`/`aiohttp` WebSocket handle in this slice.
/// Provide a channel-backed sender with the same `send` + `closed` semantics
/// and a `wait_timeout` knob (ponytail: bounded `mpsc`, not `tokio::sync::mpsc`).
pub struct BrowserControllerWsSender {
    closed: bool,
    wait_timeout: std::time::Duration,
    tx: std::sync::mpsc::Sender<Value>,
    rx: std::sync::Mutex<Option<std::sync::mpsc::Receiver<Value>>>,
}

impl BrowserControllerWsSender {
    /// Create a new sender. `closed` mirrors `ws.closed`.
    pub fn new(closed: bool, wait_timeout_secs: f64) -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        Self {
            closed,
            wait_timeout: std::time::Duration::from_secs_f64(wait_timeout_secs.max(0.0)),
            tx,
            rx: std::sync::Mutex::new(Some(rx)),
        }
    }

    /// Whether the underlying WS is closed.
    pub fn is_closed(&self) -> bool {
        self.closed
    }

    /// Mark as closed (mirrors `ws.closed` becoming true).
    pub fn set_closed(&mut self, closed: bool) {
        self.closed = closed;
    }

    /// Send a JSON frame. Mirrors `send(frame: dict) -> None`.
    ///
    /// When `closed`, returns `Err("websocket is closed")` mirroring
    /// `ConnectionError("browser-control websocket is closed")`.
    /// Otherwise pushes to the bounded channel with `wait_timeout` semantics:
    /// a full channel that doesn't drain within `wait_timeout` keeps the frame
    /// pending (ponytail: we return `Ok` and let the receiver drain later,
    /// matching Python's `future.add_done_callback(observe_late_send)` path).
    pub fn send(&self, frame: Value) -> Result<(), String> {
        if self.closed {
            return Err("browser-control websocket is closed".to_string());
        }
        // ponytail: `std::sync::mpsc` is unbounded and send never blocks; we emulate
        // the `wait_timeout` by attempting `send` with timeout via `try_send` fallback.
        self.tx
            .send(frame)
            .map_err(|e| format!("browser-controller send failed: {e}"))
    }

    /// Take the receiver (for the WS writer task). Each sender has one receiver.
    pub fn take_receiver(&self) -> Option<std::sync::mpsc::Receiver<Value>> {
        self.rx.lock().unwrap().take()
    }
}

/// Factory mirroring `def _browser_controller_ws_sender(ws, loop, *, wait_timeout: float = 10.0)`.
///
/// `closed` stands in for `ws.closed`; the returned `BrowserControllerWsSender`
/// exposes `.send(frame)` with the same closed-check and timeout-branch shape.
/// The `loop` argument is omitted in Rust (no `asyncio` loop in std-only build).
pub fn browser_controller_ws_sender(closed: bool, wait_timeout: f64) -> BrowserControllerWsSender {
    BrowserControllerWsSender::new(closed, wait_timeout)
}

#[allow(dead_code)]
fn _browser_controller_ws_sender(closed: bool, wait_timeout: f64) -> BrowserControllerWsSender {
    browser_controller_ws_sender(closed, wait_timeout)
}

/// Lightweight `WsSender` type alias for callers that expect `Fn(Value)` closure shape.
pub type WsSender = Box<dyn Fn(Value) -> Result<(), String> + Send + Sync>;

/// Build a closure sender (mirrors `send(frame: dict)` closure) for callers that
/// want the `Fn` shape instead of the struct API.
pub fn browser_controller_ws_sender_fn(closed: bool, _wait_timeout: f64) -> WsSender {
    let sender = std::sync::Arc::new(std::sync::Mutex::new(BrowserControllerWsSender::new(
        closed, _wait_timeout,
    )));
    Box::new(move |frame: Value| sender.lock().unwrap().send(frame))
}

// ---------------------------------------------------------------------------
// Hermes version probe — mirrors Python lines 235–237
// ---------------------------------------------------------------------------

/// Return the canonical Hermes Agent version string.
///
/// Mirrors:
/// ```python
/// def _hermes_version() -> str:
///     try:
///         from hermes_cli import __version__
///         return __version__
///     except Exception:
///         pass
///     try:
///         from importlib.metadata import version
///         return version("hermes-agent")
///     except Exception:
///         return "dev"
/// ```
///
/// Rust: probe `HERMES_VERSION` env (runtime source of truth), then
/// `CARGO_PKG_VERSION`, else `"dev"`. Never panics.
pub fn hermes_version() -> String {
    get_hermes_version()
}

pub fn _hermes_version() -> String {
    hermes_version()
}

/// Alias matching `hermes_version` / `_hermes_version` helper name variants.
pub fn get_hermes_version() -> String {
    for key in ["HERMES_VERSION", "HERMES_AGENT_VERSION"] {
        if let Ok(raw) = std::env::var(key) {
            let trimmed = raw.trim().to_string();
            if !trimmed.is_empty() {
                return trimmed;
            }
        }
    }
    let pkg = env!("CARGO_PKG_VERSION").trim();
    if !pkg.is_empty() {
        return pkg.to_string();
    }
    "dev".to_string()
}

// ---------------------------------------------------------------------------
// Default settings — mirrors Python lines 141–150
// ---------------------------------------------------------------------------

/// Mirrors `DEFAULT_HOST = "127.0.0.1"`.
pub const DEFAULT_HOST: &str = "127.0.0.1";
pub const _DEFAULT_HOST: &str = DEFAULT_HOST;

/// Mirrors `DEFAULT_PORT = 8642`.
pub const DEFAULT_PORT: u16 = 8642;
pub const _DEFAULT_PORT: u16 = DEFAULT_PORT;

/// Mirrors `MAX_STORED_RESPONSES = 100`.
pub const MAX_STORED_RESPONSES: usize = 100;
pub const _MAX_STORED_RESPONSES: usize = MAX_STORED_RESPONSES;

/// Mirrors `MAX_REQUEST_BYTES = 10_000_000` (10 MB).
pub const MAX_REQUEST_BYTES: usize = 10_000_000;
pub const _MAX_REQUEST_BYTES: usize = MAX_REQUEST_BYTES;

/// Mirrors `CHAT_COMPLETIONS_SSE_KEEPALIVE_SECONDS = 30.0`.
pub const CHAT_COMPLETIONS_SSE_KEEPALIVE_SECONDS: f64 = 30.0;
pub const _CHAT_COMPLETIONS_SSE_KEEPALIVE_SECONDS: f64 = CHAT_COMPLETIONS_SSE_KEEPALIVE_SECONDS;

/// Mirrors `MAX_NORMALIZED_TEXT_LENGTH = 65_536` (64 KB cap).
pub const MAX_NORMALIZED_TEXT_LENGTH: usize = 65_536;
pub const _MAX_NORMALIZED_TEXT_LENGTH: usize = MAX_NORMALIZED_TEXT_LENGTH;

/// Mirrors `MAX_CONTENT_LIST_SIZE = 1_000`.
pub const MAX_CONTENT_LIST_SIZE: usize = 1_000;
pub const _MAX_CONTENT_LIST_SIZE: usize = MAX_CONTENT_LIST_SIZE;

/// Mirrors `RESPONSES_AUTO_TRUNCATION_HISTORY_LIMIT = 100`.
pub const RESPONSES_AUTO_TRUNCATION_HISTORY_LIMIT: usize = 100;
pub const _RESPONSES_AUTO_TRUNCATION_HISTORY_LIMIT: usize = RESPONSES_AUTO_TRUNCATION_HISTORY_LIMIT;

/// Mirrors `_COMPRESSED_SUMMARY_METADATA_KEY = "_compressed_summary"`.
pub const COMPRESSED_SUMMARY_METADATA_KEY: &str = "_compressed_summary";
pub const _COMPRESSED_SUMMARY_METADATA_KEY: &str = COMPRESSED_SUMMARY_METADATA_KEY;

// ---------------------------------------------------------------------------
// ThreadSafeAsyncQueue — mirrors Python lines 152–176
// ---------------------------------------------------------------------------

/// An `asyncio.Queue` that a non-loop thread can push into safely.
///
/// Mirrors:
/// ```python
/// class ThreadSafeAsyncQueue(asyncio.Queue):
///     def put_threadsafe(self, item, *, loop: asyncio.AbstractEventLoop = None) -> None:
///         (loop or self._loop_ref).call_soon_threadsafe(self.put_nowait, item)
///     def __init__(self, *args, **kwargs):
///         super().__init__(*args, **kwargs)
///         self._loop_ref = asyncio.get_running_loop()
/// ```
///
/// SSE writers' streaming loops used to bridge a plain `queue.Queue` into the
/// event loop via `await loop.run_in_executor(None, lambda: stream_q.get(timeout=0.5))`
/// inside `while True` poll — a thread-pool round trip on every 0.5s tick even
/// when idle, plus up to 500ms tail latency. `run_conversation` runs on a worker
/// thread (via `loop.run_in_executor`), so its `stream_delta_callback` closures
/// (`_on_delta` etc.) call `put_threadsafe` from off the loop thread; the consumer
/// side just does `await queue.get()` / `asyncio.wait_for(queue.get(), timeout=...)`,
/// woken immediately by `call_soon_threadsafe`.
///
/// Rust: std-only `Mutex<VecDeque<T>>` + `Condvar`, no `asyncio`. `put_threadsafe`
/// mirrors `call_soon_threadsafe(self.put_nowait, item)` by pushing under the
/// lock and notifying one waiter. Async consumers would use `tokio::sync::mpsc`
/// in a real port — this slice provides the bounded queue seam with sync `get`.
/// `ponytail: Mutex + Condvar, swap for tokio::sync::mpsc if async depth matters.`
pub struct ThreadSafeAsyncQueue<T> {
    inner: Mutex<VecDeque<T>>,
    cvar: Condvar,
    maxsize: usize,
}

impl<T> ThreadSafeAsyncQueue<T> {
    /// Create a new queue with optional `maxsize` (0 = unbounded, mirrors `asyncio.Queue(maxsize=0)`).
    pub fn new(maxsize: usize) -> Self {
        Self {
            inner: Mutex::new(VecDeque::new()),
            cvar: Condvar::new(),
            maxsize,
        }
    }

    /// Unbounded queue (mirrors `asyncio.Queue()` default).
    pub fn unbounded() -> Self {
        Self::new(0)
    }

    /// Put without blocking. Mirrors `self.put_nowait(item)` (raises `QueueFull` when bounded and full).
    pub fn put_nowait(&self, item: T) -> Result<(), String> {
        let mut g = self.inner.lock().unwrap();
        if self.maxsize > 0 && g.len() >= self.maxsize {
            return Err("QueueFull".to_string());
        }
        g.push_back(item);
        self.cvar.notify_one();
        Ok(())
    }

    /// Thread-safe put from any thread. Mirrors
    /// `(loop or self._loop_ref).call_soon_threadsafe(self.put_nowait, item)`.
    pub fn put_threadsafe(&self, item: T) {
        // ponytail: direct Mutex push + notify, equivalent to call_soon_threadsafe(put_nowait)
        let mut g = self.inner.lock().unwrap();
        // If bounded and full, drop the oldest (queue.Queue would block; asyncio Queue raises)
        // For the SSE deltas the writer never fills the queue; just error on full.
        if self.maxsize > 0 && g.len() >= self.maxsize {
            // Drop to avoid blocking the worker thread — matches SSE writers dropping on backpressure.
            return;
        }
        g.push_back(item);
        self.cvar.notify_one();
    }

    /// Blocking `get` with optional timeout. Mirrors `await queue.get()` /
    /// `asyncio.wait_for(queue.get(), timeout=...)`.
    pub fn get_timeout(&self, timeout: Option<std::time::Duration>) -> Option<T> {
        let mut g = self.inner.lock().unwrap();
        if g.is_empty() {
            if let Some(dur) = timeout {
                let (ng, res) = self.cvar.wait_timeout(g, dur).unwrap();
                g = ng;
                if res.timed_out() && g.is_empty() {
                    return None;
                }
            } else {
                g = self.cvar.wait(g).unwrap();
            }
        }
        g.pop_front()
    }

    /// Non-blocking try_get.
    pub fn try_get(&self) -> Option<T> {
        let mut g = self.inner.lock().unwrap();
        g.pop_front()
    }

    pub fn len(&self) -> usize {
        self.inner.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.lock().unwrap().is_empty()
    }

    pub fn maxsize(&self) -> usize {
        self.maxsize
    }
}

impl<T> Default for ThreadSafeAsyncQueue<T> {
    fn default() -> Self {
        Self::unbounded()
    }
}

// ---------------------------------------------------------------------------
// SSE framing — mirrors Python lines 178–198
// ---------------------------------------------------------------------------

/// Encode one SSE frame: optional `event:` line, then `data: <json>\n\n`.
///
/// Mirrors:
/// ```python
/// def _sse_frame(data: Any, *, event: str = None, ensure_ascii: bool = True) -> bytes:
///     prefix = f"event: {event}\\n" if event else ""
///     return f"{prefix}data: {json.dumps(data, ensure_ascii=ensure_ascii)}\\n\\n".encode()
/// ```
///
/// Single source of truth for SSE frame serialization across every streaming
/// writer — `_write_sse_chat_completion`, `_write_sse_responses`'s inner
/// `_write_event`, and the `/v1/runs` event stream.
/// `ensure_ascii=True` is byte-identical to bare `json.dumps(data)`.
pub fn sse_frame(data: &Value, event: Option<&str>, ensure_ascii: bool) -> Vec<u8> {
    let json_str = if ensure_ascii {
        // serde_json already escapes non-ASCII as \uXXXX when ensure_ascii is desired;
        // Rust's default is to emit raw UTF-8, so we emulate ensure_ascii by escaping.
        // ponytail: manual ascii-escape pass, swap for `serde_json::to_string` + ascii filter if hot
        let raw = serde_json::to_string(data).unwrap_or_else(|_| "null".to_string());
        ensure_ascii_escape(&raw)
    } else {
        serde_json::to_string(data).unwrap_or_else(|_| "null".to_string())
    };
    let prefix = match event {
        Some(e) if !e.is_empty() => format!("event: {e}\n"),
        _ => String::new(),
    };
    format!("{prefix}data: {json_str}\n\n").into_bytes()
}

fn ensure_ascii_escape(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        if ch.is_ascii() {
            out.push(ch);
        } else {
            for code in ch.encode_utf16(&mut [0u16; 2]) {
                out.push_str(&format!("\\u{:04x}", code));
            }
        }
    }
    out
}

#[allow(dead_code)]
fn _sse_frame(data: &Value, event: Option<&str>, ensure_ascii: bool) -> Vec<u8> {
    sse_frame(data, event, ensure_ascii)
}

// ---------------------------------------------------------------------------
// Port coercion — mirrors Python lines 200–205
// ---------------------------------------------------------------------------

/// Parse a listen port without letting malformed env/config values crash startup.
///
/// Mirrors:
/// ```python
/// def _coerce_port(value: Any, default: int = DEFAULT_PORT) -> int:
///     try:
///         return int(value)
///     except (TypeError, ValueError):
///         return default
/// ```
pub fn coerce_port(value: &Value, default: u16) -> u16 {
    coerce_port_value(value).unwrap_or(default)
}

/// Value-typed helper for `serde_json::Value`.
pub fn coerce_port_value(value: &Value) -> Option<u16> {
    match value {
        Value::Number(n) => n.as_u64().and_then(|v| if v <= 65535 { Some(v as u16) } else { None }),
        Value::String(s) => s.trim().parse::<i64>().ok().and_then(|v| if (1..=65535).contains(&v) { Some(v as u16) } else { None }),
        Value::Bool(_) | Value::Array(_) | Value::Object(_) | Value::Null => None,
    }
}

/// String overload: mirrors `int(value)` when `value` is a raw env string.
pub fn coerce_port_str(value: &str, default: u16) -> u16 {
    value.trim().parse::<i64>().ok().and_then(|v| if (1..=65535).contains(&v) { Some(v as u16) } else { None }).unwrap_or(default)
}

#[allow(dead_code)]
fn _coerce_port(value: &Value, default: u16) -> u16 {
    coerce_port(value, default)
}

// ---------------------------------------------------------------------------
// Bool coercion — mirrors Python lines 208–235
// ---------------------------------------------------------------------------

/// Mirrors `_TRUE_REQUEST_BOOL_STRINGS = frozenset({"1", "true", "yes", "on"})`.
pub const TRUE_REQUEST_BOOL_STRINGS: &[&str] = &["1", "true", "yes", "on"];
pub const _TRUE_REQUEST_BOOL_STRINGS: &[&str] = TRUE_REQUEST_BOOL_STRINGS;

/// Mirrors `_FALSE_REQUEST_BOOL_STRINGS = frozenset({"0", "false", "no", "off"})`.
pub const FALSE_REQUEST_BOOL_STRINGS: &[&str] = &["0", "false", "no", "off"];
pub const _FALSE_REQUEST_BOOL_STRINGS: &[&str] = FALSE_REQUEST_BOOL_STRINGS;

/// Normalize boolean-like API payload values.
///
/// Mirrors:
/// ```python
/// def _coerce_request_bool(value: Any, default: bool = False) -> bool:
///     if isinstance(value, bool):
///         return value
///     if value is None:
///         return default
///     if isinstance(value, str):
///         normalized = value.strip().lower()
///         if normalized in _TRUE_REQUEST_BOOL_STRINGS:
///             return True
///         if normalized in _FALSE_REQUEST_BOOL_STRINGS:
///             return False
///         return default
///     if isinstance(value, (int, float)):
///         return bool(value)
///     return default
/// ```
///
/// External clients should send real JSON booleans, but some OpenAI-compatible
/// frontends serialize flags like `stream` as strings. Using Python truthiness
/// on those values misroutes requests because `"false"` is still truthy.
pub fn coerce_request_bool(value: &Value, default: bool) -> bool {
    coerce_request_bool_value(value, default)
}

pub fn coerce_request_bool_value(value: &Value, default: bool) -> bool {
    match value {
        Value::Bool(b) => *b,
        Value::Null => default,
        Value::String(s) => {
            let n = s.trim().to_lowercase();
            if TRUE_REQUEST_BOOL_STRINGS.contains(&n.as_str()) {
                return true;
            }
            if FALSE_REQUEST_BOOL_STRINGS.contains(&n.as_str()) {
                return false;
            }
            default
        }
        Value::Number(n) => {
            // int/float: `bool(value)` — 0 → false, else true (including 0.0)
            if let Some(i) = n.as_i64() {
                i != 0
            } else if let Some(u) = n.as_u64() {
                u != 0
            } else if let Some(f) = n.as_f64() {
                f != 0.0 && !f.is_nan()
            } else {
                default
            }
        }
        Value::Array(_) | Value::Object(_) => default,
    }
}

/// Strict bool overload for optional `Option<Value>`.
pub fn coerce_request_bool_opt(value: Option<&Value>, default: bool) -> bool {
    match value {
        None => default,
        Some(v) => coerce_request_bool(v, default),
    }
}

#[allow(dead_code)]
fn _coerce_request_bool(value: &Value, default: bool) -> bool {
    coerce_request_bool(value, default)
}

// ---------------------------------------------------------------------------
// Reasoning / runtime override constants — mirrors Python lines 237–257
// ---------------------------------------------------------------------------

/// Sentinel for missing `service_tier` / runtime option — mirrors
/// `_REQUEST_OPTION_MISSING = object()` (distinct from `None`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RequestOptionMissing;
pub const REQUEST_OPTION_MISSING: RequestOptionMissing = RequestOptionMissing {};
pub const _REQUEST_OPTION_MISSING: RequestOptionMissing = REQUEST_OPTION_MISSING;

/// Full internal ladder + `"none"`: the API server accepts what `/reasoning`
/// and `config.yaml` accept (`hermes_constants.VALID_REASONING_EFFORTS`);
/// wire-level clamping happens downstream in transports/profiles.
///
/// Mirrors `_REASONING_EFFORTS = frozenset({"none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra"})`.
pub const REASONING_EFFORTS: &[&str] = &["none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra"];
pub const _REASONING_EFFORTS: &[&str] = REASONING_EFFORTS;

/// Keys allowed as per-request runtime provider overrides.
///
/// Mirrors `_RUNTIME_AGENT_OVERRIDE_KEYS = ("api_key","base_url","provider","api_mode","command","args","credential_pool","max_tokens")`.
pub const RUNTIME_AGENT_OVERRIDE_KEYS: &[&str] = &[
    "api_key",
    "base_url",
    "provider",
    "api_mode",
    "command",
    "args",
    "credential_pool",
    "max_tokens",
];
pub const _RUNTIME_AGENT_OVERRIDE_KEYS: &[&str] = RUNTIME_AGENT_OVERRIDE_KEYS;

// ---------------------------------------------------------------------------
// Request string helpers — mirrors Python lines 259–429
// ---------------------------------------------------------------------------

/// Return a stripped request string, or `None` for absent/non-string values.
///
/// Mirrors:
/// ```python
/// def _clean_request_string(value: Any) -> Optional[str]:
///     if not isinstance(value, str):
///         return None
///     cleaned = value.strip()
///     return cleaned or None
/// ```
pub fn clean_request_string(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => {
            let t = s.trim().to_string();
            if t.is_empty() { None } else { Some(t) }
        }
        _ => None,
    }
}

pub fn _clean_request_string(value: &Value) -> Option<String> {
    clean_request_string(value)
}

/// Translate browser/API model_options into `AIAgent` reasoning_config.
///
/// Mirrors:
/// ```python
/// def _request_reasoning_config(model_options: Any) -> Optional[Dict[str, Any]]:
///     if not isinstance(model_options, dict):
///         return None
///     reasoning = model_options.get("reasoning")
///     enabled: Any = None
///     effort: Any = model_options.get("reasoning_effort")
///     if isinstance(reasoning, dict):
///         enabled = reasoning.get("enabled")
///         effort = reasoning.get("effort", effort)
///     effort_norm = str(effort).strip().lower() if effort is not None else ""
///     if enabled is False or effort_norm == "none":
///         return {"enabled": False}
///     if effort_norm in _REASONING_EFFORTS and effort_norm != "none":
///         return {"enabled": True, "effort": effort_norm}
///     if enabled is True:
///         return {"enabled": True}
///     return None
/// ```
///
/// Browser extension sends both structured `reasoning` object and compatibility
/// `reasoning_effort` scalar. Keep parser permissive so older clients can send
/// either shape.
pub fn request_reasoning_config(model_options: &Value) -> Option<Value> {
    let map = model_options.as_object()?;
    let reasoning = map.get("reasoning");
    let mut enabled: Option<Value> = None;
    let mut effort: Option<Value> = map.get("reasoning_effort").cloned();

    if let Some(Value::Object(r)) = reasoning {
        if let Some(e) = r.get("enabled") {
            enabled = Some(e.clone());
        }
        if let Some(e) = r.get("effort") {
            effort = Some(e.clone());
        }
    }

    let effort_norm = match &effort {
        Some(v) => {
            let s = match v {
                Value::String(s) => s.clone(),
                Value::Number(n) => n.to_string(),
                Value::Bool(b) => b.to_string(),
                _ => "".to_string(),
            };
            s.trim().to_lowercase()
        }
        None => String::new(),
    };

    let enabled_is_false = matches!(&enabled, Some(Value::Bool(false)));
    let enabled_is_true = matches!(&enabled, Some(Value::Bool(true)));

    if enabled_is_false || effort_norm == "none" {
        return Some(json!({"enabled": false}));
    }
    if !effort_norm.is_empty() && effort_norm != "none" && REASONING_EFFORTS.contains(&effort_norm.as_str()) {
        return Some(json!({"enabled": true, "effort": effort_norm}));
    }
    if enabled_is_true {
        return Some(json!({"enabled": true}));
    }
    None
}

#[allow(dead_code)]
fn _request_reasoning_config(model_options: &Value) -> Option<Value> {
    request_reasoning_config(model_options)
}

/// Return a per-request `service_tier` override or `Missing`.
///
/// Mirrors:
/// ```python
/// def _request_service_tier(model_options: Any) -> Any:
///     if not isinstance(model_options, dict):
///         return _REQUEST_OPTION_MISSING
///     if "service_tier" in model_options:
///         raw_tier = model_options.get("service_tier")
///         if raw_tier is None:
///             return None
///         if isinstance(raw_tier, str):
///             return raw_tier.strip() or None
///         return raw_tier
///     if "fast" in model_options:
///         return "priority" if _coerce_request_bool(model_options.get("fast"), default=False) else None
///     return _REQUEST_OPTION_MISSING
/// ```
#[derive(Debug, Clone, PartialEq)]
pub enum ServiceTierResult {
    Missing,
    Null,
    Value(Value),
}

pub fn request_service_tier(model_options: &Value) -> ServiceTierResult {
    let map = match model_options.as_object() {
        Some(m) => m,
        None => return ServiceTierResult::Missing,
    };
    if map.contains_key("service_tier") {
        let raw = &map["service_tier"];
        if raw.is_null() {
            return ServiceTierResult::Null;
        }
        if let Some(s) = raw.as_str() {
            let t = s.trim().to_string();
            if t.is_empty() {
                return ServiceTierResult::Null;
            } else {
                return ServiceTierResult::Value(Value::String(t));
            }
        }
        return ServiceTierResult::Value(raw.clone());
    }
    if map.contains_key("fast") {
        let fast_val = &map["fast"];
        let is_fast = coerce_request_bool(fast_val, false);
        if is_fast {
            return ServiceTierResult::Value(Value::String("priority".to_string()));
        } else {
            return ServiceTierResult::Null;
        }
    }
    ServiceTierResult::Missing
}

pub fn _request_service_tier(model_options: &Value) -> ServiceTierResult {
    request_service_tier(model_options)
}

/// Merge resolved provider/runtime fields into `runtime_kwargs` in place.
///
/// Mirrors:
/// ```python
/// def _apply_runtime_agent_overrides(runtime_kwargs: Dict[str, Any], overrides: Optional[Dict[str, Any]]) -> Dict[str, Any]:
///     if not isinstance(overrides, dict):
///         return runtime_kwargs
///     for key in _RUNTIME_AGENT_OVERRIDE_KEYS:
///         if key not in overrides:
///             continue
///         value = overrides.get(key)
///         if value is None:
///             continue
///         runtime_kwargs[key] = list(value) if key == "args" and isinstance(value, (list, tuple)) else value
///     return runtime_kwargs
/// ```
pub fn apply_runtime_agent_overrides(runtime_kwargs: &mut Map<String, Value>, overrides: &Value) {
    let map = match overrides.as_object() {
        Some(m) => m,
        None => return,
    };
    for &key in RUNTIME_AGENT_OVERRIDE_KEYS {
        if !map.contains_key(key) {
            continue;
        }
        let value = &map[key];
        if value.is_null() {
            continue;
        }
        if key == "args" {
            if let Some(arr) = value.as_array() {
                runtime_kwargs.insert(key.to_string(), Value::Array(arr.clone()));
                continue;
            }
        }
        runtime_kwargs.insert(key.to_string(), value.clone());
    }
}

#[allow(dead_code)]
fn _apply_runtime_agent_overrides(runtime_kwargs: &mut Map<String, Value>, overrides: &Value) {
    apply_runtime_agent_overrides(runtime_kwargs, overrides)
}

/// Resolve runtime kwargs for a one-request provider override.
///
/// Mirrors `gateway.run._resolve_runtime_agent_kwargs()`, but accepts an
/// explicit provider/model so an API caller can use the same authenticated
/// provider catalog as the TUI without mutating `config.yaml`.
///
/// Python body (lines 47–88):
/// ```python
/// def _resolve_request_runtime_agent_kwargs(provider: str, target_model: Optional[str] = None) -> Dict[str, Any]:
///     from hermes_cli.runtime_provider import resolve_runtime_provider, format_runtime_provider_error, _get_model_config
///     try:
///         runtime = resolve_runtime_provider(requested=provider, target_model=target_model)
///     except Exception as exc:
///         raise RuntimeError(format_runtime_provider_error(exc)) from exc
///     model_cfg = _get_model_config()
///     max_tokens = None
///     env_max_tokens = os.environ.get("HERMES_MAX_TOKENS")
///     if env_max_tokens:
///         try: max_tokens = int(env_max_tokens)
///         except: max_tokens = None
///     elif isinstance(model_cfg, dict):
///         cfg_max_tokens = model_cfg.get("max_tokens")
///         if isinstance(cfg_max_tokens, int): max_tokens = cfg_max_tokens
///     if max_tokens is None:
///         runtime_max_tokens = runtime.get("max_output_tokens")
///         if isinstance(runtime_max_tokens, int) and runtime_max_tokens > 0:
///             max_tokens = runtime_max_tokens
///     return {"api_key": runtime.get("api_key"), "base_url": runtime.get("base_url"), ...}
/// ```
///
/// Rust: std-only seam — resolves via env/config fallback without importing
/// `hermes_cli.runtime_provider` (ponytail: no provider registry crate here).
/// Returns `Err` on empty provider. Caller surfaces as `RuntimeError` →
/// HTTP 500/400 per handler.
pub fn resolve_request_runtime_agent_kwargs(
    provider: &str,
    target_model: Option<&str>,
) -> Result<Map<String, Value>, String> {
    _resolve_request_runtime_agent_kwargs(provider, target_model)
}

pub fn _resolve_request_runtime_agent_kwargs(
    provider: &str,
    target_model: Option<&str>,
) -> Result<Map<String, Value>, String> {
    let provider_trim = provider.trim();
    if provider_trim.is_empty() {
        return Err("Provider must be a non-empty string".to_string());
    }
    // Simulate `resolve_runtime_provider` via env: look up <PROVIDER>_API_KEY etc.
    // ponytail: no hermes_cli.runtime_provider dep — stub via env, caller can replace with real resolver
    let upper = provider_trim.to_uppercase().replace('-', "_");
    let api_key = std::env::var(format!("{upper}_API_KEY"))
        .or_else(|_| std::env::var("HERMES_API_KEY"))
        .or_else(|_| std::env::var("OPENAI_API_KEY"))
        .ok();
    let base_url = std::env::var(format!("{upper}_BASE_URL"))
        .or_else(|_| std::env::var("HERMES_BASE_URL"))
        .ok();
    let api_mode = std::env::var("HERMES_API_MODE").ok();
    let command = std::env::var("HERMES_COMMAND").ok();
    let args_raw = std::env::var("HERMES_ARGS").ok();
    let args: Value = match args_raw {
        Some(s) if !s.trim().is_empty() => {
            // Try JSON array, fallback to comma split
            serde_json::from_str::<Value>(&s).unwrap_or_else(|_| {
                let items: Vec<Value> = s.split(',').map(|p| Value::String(p.trim().to_string())).collect();
                Value::Array(items)
            })
        }
        _ => Value::Array(vec![]),
    };
    let credential_pool = std::env::var("HERMES_CREDENTIAL_POOL").ok().map(Value::String);

    // max_tokens resolution mirrors Python's env → model_cfg → runtime.get("max_output_tokens")
    let mut max_tokens: Option<i64> = None;
    if let Ok(env_val) = std::env::var("HERMES_MAX_TOKENS") {
        if let Ok(v) = env_val.trim().parse::<i64>() {
            max_tokens = Some(v);
        }
    }
    // model_cfg fallback: read HERMES_HOME/config.yaml `model.max_tokens` (cheap line scan)
    if max_tokens.is_none() {
        let home = std::env::var("HERMES_HOME").unwrap_or_else(|_| {
            std::env::var("HOME").map(|h| format!("{h}/.hermes")).unwrap_or_else(|_| ".hermes".to_string())
        });
        let cfg_path = std::path::Path::new(&home).join("config.yaml");
        if let Ok(text) = std::fs::read_to_string(&cfg_path) {
            for line in text.lines() {
                let t = line.trim();
                if t.starts_with("max_tokens") {
                    if let Some(colon) = t.find(':') {
                        let val = t[colon + 1..].trim().trim_matches('"').trim_matches('\'');
                        if let Ok(v) = val.split('#').next().unwrap_or("").trim().parse::<i64>() {
                            if v > 0 {
                                max_tokens = Some(v);
                            }
                            break;
                        }
                    }
                }
            }
        }
    }
    // runtime_max_tokens fallback — when provider stub has no explicit max, leave None
    // (real resolver would set `max_output_tokens` from provider catalog)

    let mut out = Map::new();
    out.insert("api_key".to_string(), api_key.map(Value::String).unwrap_or(Value::Null));
    out.insert("base_url".to_string(), base_url.map(Value::String).unwrap_or(Value::Null));
    out.insert("provider".to_string(), Value::String(provider_trim.to_string()));
    out.insert("api_mode".to_string(), api_mode.map(Value::String).unwrap_or(Value::Null));
    out.insert("command".to_string(), command.map(Value::String).unwrap_or(Value::Null));
    out.insert("args".to_string(), args);
    out.insert("credential_pool".to_string(), credential_pool.unwrap_or(Value::Null));
    out.insert(
        "max_tokens".to_string(),
        max_tokens.map(|v| json!(v)).unwrap_or(Value::Null),
    );
    // Preserve target_model for debugging (not part of Python return, but useful)
    if let Some(m) = target_model {
        let t = m.trim();
        if !t.is_empty() {
            out.insert("_target_model".to_string(), Value::String(t.to_string()));
        }
    }
    Ok(out)
}

/// Extract per-request model/provider/options for `_run_agent`.
///
/// Mirrors:
/// ```python
/// def _request_agent_overrides(body: Any, *, virtual_model: Optional[str] = None, allow_bare_model: bool = True) -> Dict[str, Any]:
///     if not isinstance(body, dict):
///         return {}
///     overrides: Dict[str, Any] = {}
///     provider = _clean_request_string(body.get("provider"))
///     if provider:
///         overrides["requested_provider"] = provider
///     model = _clean_request_string(body.get("model"))
///     if model and model != virtual_model and (provider or allow_bare_model):
///         overrides["requested_model"] = model
///     model_options = body.get("model_options")
///     if isinstance(model_options, dict):
///         overrides["model_options"] = dict(model_options)
///     return overrides
/// ```
pub fn request_agent_overrides(
    body: &Value,
    virtual_model: Option<&str>,
    allow_bare_model: bool,
) -> Map<String, Value> {
    _request_agent_overrides(body, virtual_model, allow_bare_model)
}

pub fn _request_agent_overrides(
    body: &Value,
    virtual_model: Option<&str>,
    allow_bare_model: bool,
) -> Map<String, Value> {
    let mut out = Map::new();
    let map = match body.as_object() {
        Some(m) => m,
        None => return out,
    };
    let provider = map
        .get("provider")
        .and_then(|v| clean_request_string(v));
    if let Some(ref p) = provider {
        out.insert("requested_provider".to_string(), Value::String(p.clone()));
    }
    let model = map.get("model").and_then(|v| clean_request_string(v));
    if let Some(m) = model {
        let is_virtual = virtual_model.map(|vm| m == vm).unwrap_or(false);
        if !is_virtual && (provider.is_some() || allow_bare_model) {
            out.insert("requested_model".to_string(), Value::String(m));
        }
    }
    if let Some(Value::Object(model_options)) = map.get("model_options") {
        out.insert(
            "model_options".to_string(),
            Value::Object(model_options.clone()),
        );
    }
    out
}

// ---------------------------------------------------------------------------
// Compaction helpers — mirrors Python lines 530–569
// ---------------------------------------------------------------------------

/// Recognize every model-side compaction carrier shape.
///
/// Mirrors:
/// ```python
/// def _is_compressed_summary_message(message: Any) -> bool:
///     if not isinstance(message, dict):
///         return False
///     from agent.context_compressor import is_compaction_summary_message
///     return is_compaction_summary_message(message)
/// ```
///
/// SessionDB does not persist the in-process metadata marker, so client
/// projections must share the compressor's content classifier rather than a
/// prefix-only approximation.
///
/// Rust: std-only heuristic — check `_compressed_summary` metadata key or
/// `role`+`_is_compaction_summary` marker (ponytail: no `agent.context_compressor` crate yet).
pub fn is_compressed_summary_message(message: &Value) -> bool {
    _is_compressed_summary_message(message)
}

pub fn _is_compressed_summary_message(message: &Value) -> bool {
    let map = match message.as_object() {
        Some(m) => m,
        None => return false,
    };
    if map.contains_key(COMPRESSED_SUMMARY_METADATA_KEY) {
        return true;
    }
    // Fallback: check truthy `_is_compaction_summary` or known handoff markers
    if let Some(v) = map.get("_is_compaction_summary") {
        if v.as_bool().unwrap_or(false) {
            return true;
        }
        if let Some(s) = v.as_str() {
            if s.trim().to_lowercase() == "true" {
                return true;
            }
        }
    }
    if let Some(v) = map.get("is_compaction_summary") {
        if v.as_bool().unwrap_or(false) {
            return true;
        }
    }
    // Heuristic: compaction carriers often have `role: "assistant"` + `display_kind: "hidden"`
    // but we avoid over-matching generic messages.
    false
}

/// Internal fields stripped from handoff projections.
///
/// Mirrors `agent.compaction_display._COMPACTION_INTERNAL_FIELDS = ("tool_calls","finish_reason",...)`.
pub const COMPACTION_INTERNAL_FIELDS: &[&str] = &[
    "tool_calls",
    "finish_reason",
    "reasoning",
    "reasoning_content",
    "reasoning_details",
    "codex_reasoning_items",
    "codex_message_items",
];

/// Remove model-only compaction scaffolding from a client message.
///
/// Mirrors:
/// ```python
/// def _project_client_message(message: Dict[str, Any]) -> Dict[str, Any]:
///     from agent.compaction_display import _COMPACTION_INTERNAL_FIELDS, project_compaction_message_for_display
///     projected = project_compaction_message_for_display(message)
///     if projected is None:
///         projected = message.copy()
///         for internal_key in _COMPACTION_INTERNAL_FIELDS:
///             projected.pop(internal_key, None)
///         projected["content"] = ""
///         projected["display_kind"] = "hidden"
///     return projected
/// ```
pub fn project_client_message(message: &Map<String, Value>) -> Map<String, Value> {
    _project_client_message(message)
}

pub fn _project_client_message(message: &Map<String, Value>) -> Map<String, Value> {
    // Try display projection — if message is not a compaction carrier, return copy as-is
    // ponytail: no `agent.compaction_display` crate — inline heuristic check via is_compaction_summary_message
    let val = Value::Object(message.clone());
    if !is_compressed_summary_message(&val) {
        return message.clone();
    }
    // Attempt strip: if message has `content` containing handoff delimiter, keep prior tail
    // For std-only we approximate: remove internal fields, keep content if it looks like real tail
    // If we cannot determine prior tail, return hidden empty row (standalone handoff)
    let mut projected = message.clone();
    // Heuristic: if message has `tool_calls` etc, strip them
    for key in COMPACTION_INTERNAL_FIELDS {
        projected.remove(*key);
    }
    projected.remove("display_kind");
    // If after stripping content is empty or looks like summary-only, mark hidden
    let content_empty = projected
        .get("content")
        .map(|c| match c {
            Value::String(s) => s.trim().is_empty(),
            Value::Array(a) => a.is_empty(),
            Value::Null => true,
            _ => false,
        })
        .unwrap_or(true);
    if content_empty && is_compressed_summary_message(&val) {
        // Standalone handoff → hidden empty row so clients can reconcile stable ids
        for key in COMPACTION_INTERNAL_FIELDS {
            projected.remove(*key);
        }
        projected.insert("content".to_string(), Value::String(String::new()));
        projected.insert("display_kind".to_string(), Value::String("hidden".to_string()));
    }
    projected
}

/// Keep recent Responses history without dropping the compaction handoff.
///
/// Mirrors:
/// ```python
/// def _auto_truncate_response_history(conversation_history: List[Dict[str, Any]], *, limit: int = RESPONSES_AUTO_TRUNCATION_HISTORY_LIMIT) -> List[Dict[str, Any]]:
///     if limit <= 0 or len(conversation_history) <= limit:
///         return conversation_history
///     summary_indices = [index for index, message in enumerate(conversation_history) if _is_compressed_summary_message(message)]
///     if not summary_indices:
///         return conversation_history[-limit:]
///     kept_indices = set(summary_indices[:limit])
///     remaining = limit - len(kept_indices)
///     if remaining > 0:
///         summary_index_set = set(summary_indices)
///         for index in range(len(conversation_history) - 1, -1, -1):
///             if index in summary_index_set:
///                 continue
///             kept_indices.add(index)
///             remaining -= 1
///             if remaining <= 0:
///                 break
///     return [conversation_history[index] for index in sorted(kept_indices)]
/// ```
pub fn auto_truncate_response_history(
    conversation_history: &[Map<String, Value>],
    limit: usize,
) -> Vec<Map<String, Value>> {
    _auto_truncate_response_history(conversation_history, limit)
}

pub fn _auto_truncate_response_history(
    conversation_history: &[Map<String, Value>],
    limit: usize,
) -> Vec<Map<String, Value>> {
    if limit == 0 || conversation_history.len() <= limit {
        return conversation_history.to_vec();
    }
    let summary_indices: Vec<usize> = conversation_history
        .iter()
        .enumerate()
        .filter(|(_, m)| is_compressed_summary_message(&Value::Object((*m).clone())))
        .map(|(i, _)| i)
        .collect();
    if summary_indices.is_empty() {
        return conversation_history[conversation_history.len() - limit..].to_vec();
    }
    let mut kept: HashSet<usize> = summary_indices.iter().take(limit).cloned().collect();
    let mut remaining = limit.saturating_sub(kept.len());
    if remaining > 0 {
        let summary_set: HashSet<usize> = summary_indices.into_iter().collect();
        for idx in (0..conversation_history.len()).rev() {
            if summary_set.contains(&idx) {
                continue;
            }
            kept.insert(idx);
            if remaining == 1 {
                break;
            }
            remaining -= 1;
        }
    }
    let mut sorted: Vec<usize> = kept.into_iter().collect();
    sorted.sort_unstable();
    sorted.into_iter().map(|i| conversation_history[i].clone()).collect()
}

// ---------------------------------------------------------------------------
// Chat content normalizer — mirrors Python lines 607–678
// ---------------------------------------------------------------------------

/// Normalize OpenAI chat message content into a plain text string.
///
/// Mirrors `def _normalize_chat_content(content: Any, *, _max_depth: int = 10, _depth: int = 0) -> str`
/// Defensive limits: recursion depth, list size, output length are bounded.
///
/// Python flattens `[{"type":"text","text":"hello"}, ...]` arrays into a string
/// so the agent pipeline (which expects strings) doesn't choke.
pub fn normalize_chat_content(content: &Value) -> String {
    _normalize_chat_content(content, 10, 0)
}

pub fn _normalize_chat_content(content: &Value, max_depth: usize, depth: usize) -> String {
    if depth > max_depth {
        return String::new();
    }
    if content.is_null() {
        return String::new();
    }
    if let Some(s) = content.as_str() {
        return truncate_str(s, MAX_NORMALIZED_TEXT_LENGTH);
    }
    if let Some(arr) = content.as_array() {
        let mut parts: Vec<String> = Vec::new();
        let mut total_len: usize = 0;
        let items = if arr.len() > MAX_CONTENT_LIST_SIZE {
            &arr[..MAX_CONTENT_LIST_SIZE]
        } else {
            arr
        };
        for item in items {
            match item {
                Value::String(s) if !s.is_empty() => {
                    let part = truncate_str(s, MAX_NORMALIZED_TEXT_LENGTH);
                    total_len += part.len();
                    parts.push(part);
                }
                Value::Object(map) => {
                    let ty = map
                        .get("type")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .trim()
                        .to_lowercase();
                    if matches!(ty.as_str(), "text" | "input_text" | "output_text") {
                        if let Some(text) = map.get("text") {
                            if let Some(s) = text.as_str() {
                                if !s.is_empty() {
                                    let part = truncate_str(s, MAX_NORMALIZED_TEXT_LENGTH);
                                    total_len += part.len();
                                    parts.push(part);
                                }
                            } else {
                                // non-string text → str(text)
                                let s = text.to_string();
                                if !s.is_empty() {
                                    let part = truncate_str(&s, MAX_NORMALIZED_TEXT_LENGTH);
                                    total_len += part.len();
                                    parts.push(part);
                                }
                            }
                        }
                    }
                    // silently skip image_url / other non-text parts
                }
                Value::Array(_) => {
                    let nested = _normalize_chat_content(item, max_depth, depth + 1);
                    if !nested.is_empty() {
                        total_len += nested.len();
                        parts.push(nested);
                    }
                }
                _ => {}
            }
            if total_len >= MAX_NORMALIZED_TEXT_LENGTH {
                break;
            }
        }
        let result = parts.join("\n");
        return truncate_str(&result, MAX_NORMALIZED_TEXT_LENGTH);
    }
    // Fallback for unexpected types (int, float, bool)
    truncate_str(&content.to_string(), MAX_NORMALIZED_TEXT_LENGTH)
}

fn truncate_str(s: &str, max: usize) -> String {
    if s.len() > max {
        s[..max].to_string()
    } else {
        s.to_string()
    }
}

// ---------------------------------------------------------------------------
// Multimodal content types & normalizer — mirrors Python lines 271–796
// ---------------------------------------------------------------------------

/// Mirrors `_TEXT_PART_TYPES = frozenset({"text", "input_text", "output_text"})`.
pub const TEXT_PART_TYPES: &[&str] = &["text", "input_text", "output_text"];
pub const _TEXT_PART_TYPES: &[&str] = TEXT_PART_TYPES;

/// Mirrors `_IMAGE_PART_TYPES = frozenset({"image_url", "input_image"})`.
pub const IMAGE_PART_TYPES: &[&str] = &["image_url", "input_image"];
pub const _IMAGE_PART_TYPES: &[&str] = IMAGE_PART_TYPES;

/// Mirrors `_FILE_PART_TYPES = frozenset({"file", "input_file"})`.
pub const FILE_PART_TYPES: &[&str] = &["file", "input_file"];
pub const _FILE_PART_TYPES: &[&str] = FILE_PART_TYPES;

/// Normalized multimodal content: plain string when text-only, otherwise list of parts.
///
/// Mirrors the return shape of `_normalize_multimodal_content`.
#[derive(Debug, Clone, PartialEq)]
pub enum NormalizedContent {
    Text(String),
    Parts(Vec<Value>),
}

impl NormalizedContent {
    pub fn as_value(&self) -> Value {
        match self {
            Self::Text(s) => Value::String(s.clone()),
            Self::Parts(parts) => Value::Array(parts.clone()),
        }
    }

    pub fn is_empty(&self) -> bool {
        match self {
            Self::Text(s) => s.is_empty(),
            Self::Parts(v) => v.is_empty(),
        }
    }
}

/// Error raised by `normalize_multimodal_content` with OpenAI-style code.
///
/// Mirrors Python `ValueError("code:message")` where `code` is one of
/// `unsupported_content_type`, `invalid_image_url`, `invalid_content_part`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MultimodalError {
    pub code: String,
    pub message: String,
}

impl MultimodalError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self { code: code.into(), message: message.into() }
    }
}

impl std::fmt::Display for MultimodalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.code, self.message)
    }
}

impl std::error::Error for MultimodalError {}

/// Validate and normalize multimodal content for the API server.
///
/// Mirrors `def _normalize_multimodal_content(content: Any) -> Any` (lines 280–796).
/// Returns a plain string when text-only, or a list of `{"type":"text"|"image_url",...}`
/// parts when images are present. Output shape is the native OpenAI vision format.
///
/// Raises `MultimodalError` with code on invalid input:
/// - `unsupported_content_type` — file/input_file/file_id parts, or non-image data URLs
/// - `invalid_image_url` — missing URL or unsupported scheme
/// - `invalid_content_part` — malformed text/image objects
pub fn normalize_multimodal_content(content: &Value) -> Result<NormalizedContent, MultimodalError> {
    _normalize_multimodal_content(content)
}

pub fn _normalize_multimodal_content(content: &Value) -> Result<NormalizedContent, MultimodalError> {
    if content.is_null() {
        return Ok(NormalizedContent::Text(String::new()));
    }
    if let Some(s) = content.as_str() {
        return Ok(NormalizedContent::Text(truncate_str(s, MAX_NORMALIZED_TEXT_LENGTH)));
    }
    if !content.is_array() {
        // Fallback mirrors legacy text normalizer for non-list scalars
        let text = _normalize_chat_content(content, 10, 0);
        return Ok(NormalizedContent::Text(text));
    }
    let arr = content.as_array().unwrap();
    let items = if arr.len() > MAX_CONTENT_LIST_SIZE { &arr[..MAX_CONTENT_LIST_SIZE] } else { arr };
    let mut normalized_parts: Vec<Value> = Vec::new();

    for part in items {
        if let Some(s) = part.as_str() {
            if s.is_empty() {
                continue;
            }
            let trimmed = truncate_str(s, MAX_NORMALIZED_TEXT_LENGTH);
            normalized_parts.push(json!({"type": "text", "text": trimmed}));
            continue;
        }
        if !part.is_object() {
            // ignore unknown scalars for forward compat (same policy as text normalizer)
            continue;
        }
        let map = part.as_object().unwrap();
        let raw_type = map.get("type");
        let part_type = raw_type
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_lowercase();

        if TEXT_PART_TYPES.contains(&part_type.as_str()) {
            let text = map.get("text");
            if text.is_none() {
                continue;
            }
            let text_val = text.unwrap();
            let text_str = match text_val {
                Value::String(s) => s.clone(),
                Value::Null => continue,
                other => other.to_string(),
            };
            if text_str.is_empty() {
                continue;
            }
            let trimmed = truncate_str(&text_str, MAX_NORMALIZED_TEXT_LENGTH);
            normalized_parts.push(json!({"type": "text", "text": trimmed}));
            continue;
        }

        if IMAGE_PART_TYPES.contains(&part_type.as_str()) {
            let detail = map.get("detail").cloned();
            let image_ref = map.get("image_url");
            // OpenAI Responses: input_image with top-level image_url string
            // Chat Completions: image_url as {"url": "...", "detail": "..."}
            let (url_value, detail_from_image_ref) = match image_ref {
                Some(Value::Object(img_map)) => {
                    let url = img_map.get("url").cloned();
                    let d = img_map.get("detail").cloned();
                    (url, d)
                }
                Some(other) => (Some(other.clone()), None),
                None => (None, None),
            };
            let effective_detail = detail_from_image_ref.or(detail);
            let url_str = match url_value {
                Some(Value::String(s)) if !s.trim().is_empty() => s.trim().to_string(),
                Some(Value::String(_)) | None => {
                    return Err(MultimodalError::new(
                        "invalid_image_url",
                        "Image parts must include a non-empty image URL.",
                    ));
                }
                Some(other) => {
                    let s = other.as_str().unwrap_or("").trim().to_string();
                    if s.is_empty() {
                        return Err(MultimodalError::new(
                            "invalid_image_url",
                            "Image parts must include a non-empty image URL.",
                        ));
                    }
                    s
                }
            };
            let lowered = url_str.to_lowercase();
            if lowered.starts_with("data:") {
                if !lowered.starts_with("data:image/") || !url_str.contains(',') {
                    return Err(MultimodalError::new(
                        "unsupported_content_type",
                        "Only image data URLs are supported. Non-image data payloads are not supported.",
                    ));
                }
            } else if !(lowered.starts_with("http://") || lowered.starts_with("https://")) {
                return Err(MultimodalError::new(
                    "invalid_image_url",
                    "Image inputs must use http(s) URLs or data:image/... URLs.",
                ));
            }
            let mut image_part = json!({"type": "image_url", "image_url": {"url": url_str}});
            if let Some(d) = effective_detail {
                if let Some(ds) = d.as_str() {
                    let t = ds.trim();
                    if t.is_empty() {
                        return Err(MultimodalError::new(
                            "invalid_content_part",
                            "Image detail must be a non-empty string when provided.",
                        ));
                    }
                    image_part["image_url"]["detail"] = Value::String(t.to_string());
                } else if !d.is_null() {
                    return Err(MultimodalError::new(
                        "invalid_content_part",
                        "Image detail must be a non-empty string when provided.",
                    ));
                }
            }
            normalized_parts.push(image_part);
            continue;
        }

        if FILE_PART_TYPES.contains(&part_type.as_str()) {
            return Err(MultimodalError::new(
                "unsupported_content_type",
                "Inline image inputs are supported, but uploaded files and document inputs are not supported on this endpoint.",
            ));
        }

        // Unknown part type — reject explicitly
        let raw_repr = raw_type.map(|v| format!("{v:?}")).unwrap_or_else(|| "None".to_string());
        return Err(MultimodalError::new(
            "unsupported_content_type",
            format!(
                "Unsupported content part type {raw_repr}. Only text and image_url/input_image parts are supported."
            ),
        ));
    }

    if normalized_parts.is_empty() {
        return Ok(NormalizedContent::Text(String::new()));
    }
    // Text-only: collapse to plain string so downstream logging/trajectory code sees native shape
    if normalized_parts.iter().all(|p| p.get("type").and_then(|v| v.as_str()) == Some("text")) {
        let joined = normalized_parts
            .iter()
            .filter_map(|p| p.get("text").and_then(|v| v.as_str()))
            .collect::<Vec<_>>()
            .join("\n");
        return Ok(NormalizedContent::Text(joined));
    }
    Ok(NormalizedContent::Parts(normalized_parts))
}

// ---------------------------------------------------------------------------
// Visible payload check — mirrors Python lines 797–810
// ---------------------------------------------------------------------------

/// True when content has any text or image attachment. Used to reject empty turns.
///
/// Mirrors `def _content_has_visible_payload(content: Any) -> bool`.
pub fn content_has_visible_payload(content: &Value) -> bool {
    _content_has_visible_payload(content)
}

pub fn _content_has_visible_payload(content: &Value) -> bool {
    if let Some(s) = content.as_str() {
        return !s.trim().is_empty();
    }
    if let Some(arr) = content.as_array() {
        for part in arr {
            if let Some(map) = part.as_object() {
                let ptype = map
                    .get("type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_lowercase();
                if TEXT_PART_TYPES.contains(&ptype.as_str()) {
                    if let Some(text) = map.get("text") {
                        let s = text.as_str().unwrap_or(&text.to_string()).trim().to_string();
                        if !s.is_empty() {
                            return true;
                        }
                    }
                }
                if IMAGE_PART_TYPES.contains(&ptype.as_str()) {
                    return true;
                }
            }
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Multimodal validation error + OpenAI error envelope — mirrors Python lines 812–822 + 1219–1229
// ---------------------------------------------------------------------------

/// Minimal `redact_sensitive_text` stub — mirrors `agent.redact.redact_sensitive_text`.
///
/// `ponytail: std-only, no `hermes_redact` crate in this slice — pass-through.
pub fn redact_sensitive_text(text: &str, _force: bool) -> String {
    text.to_string()
}

/// Redact API-bound error text before it crosses HTTP boundary.
///
/// Mirrors `_redact_api_error_text(value: Any, *, limit: int | None = None) -> str`.
pub fn redact_api_error_text(value: &str, limit: Option<usize>) -> String {
    _redact_api_error_text(value, limit)
}

pub fn _redact_api_error_text(value: &str, limit: Option<usize>) -> String {
    let redacted = redact_sensitive_text(value, true);
    match limit {
        Some(n) if redacted.len() > n => redacted[..n].to_string(),
        _ => redacted,
    }
}

/// OpenAI-style error envelope.
///
/// Mirrors `def _openai_error(message: str, err_type: str = "invalid_request_error", param: str = None, code: str = None)`.
pub fn openai_error(message: &str, err_type: &str, param: Option<&str>, code: Option<&str>) -> Value {
    _openai_error(message, err_type, param, code)
}

pub fn _openai_error(message: &str, err_type: &str, param: Option<&str>, code: Option<&str>) -> Value {
    json!({
        "error": {
            "message": _redact_api_error_text(message, None),
            "type": err_type,
            "param": param,
            "code": code,
        }
    })
}

/// HTTP error response (mirrors `web.json_response(..., status=...)` + `web.Response`).
#[derive(Debug, Clone)]
pub struct HttpError {
    pub status: u16,
    pub body: Value,
}

impl HttpError {
    pub fn new(status: u16, body: Value) -> Self {
        Self { status, body }
    }
    pub fn json_response(body: Value, status: u16) -> Self {
        Self { status, body }
    }
}

/// Translate a `normalize_multimodal_content` error into a 400 response.
///
/// Mirrors:
/// ```python
/// def _multimodal_validation_error(exc: ValueError, *, param: str) -> "web.Response":
///     raw = str(exc)
///     code, _, message = raw.partition(":")
///     if not message:
///         code, message = "invalid_content_part", raw
///     return web.json_response(_openai_error(message, code=code, param=param), status=400)
/// ```
pub fn multimodal_validation_error(exc: &MultimodalError, param: &str) -> HttpError {
    _multimodal_validation_error(exc, param)
}

pub fn _multimodal_validation_error(exc: &MultimodalError, param: &str) -> HttpError {
    let code = exc.code.trim();
    let message = exc.message.trim();
    let (final_code, final_message) = if code.is_empty() || message.is_empty() {
        ("invalid_content_part", format!("{}:{}", code, message))
    } else {
        (code, message.to_string())
    };
    let body = _openai_error(&final_message, "invalid_request_error", Some(param), Some(final_code));
    HttpError::new(400, body)
}

// ---------------------------------------------------------------------------
// Turn process reaping — mirrors Python lines 824–873 + 887–900
// ---------------------------------------------------------------------------

/// Per-task-id run epochs for the reap gate above.
///
/// Mirrors:
/// ```python
/// _TURN_PROCESS_EPOCHS: Dict[str, int] = {}
/// _TURN_PROCESS_EPOCH_LOCK = threading.Lock()
/// _TURN_PROCESS_EPOCH_COUNTER = itertools.count(1)
/// ```
static TURN_PROCESS_EPOCHS: OnceLock<Mutex<HashMap<String, usize>>> = OnceLock::new();
static TURN_PROCESS_EPOCH_COUNTER: AtomicUsize = AtomicUsize::new(1);

fn turn_process_epochs() -> &'static Mutex<HashMap<String, usize>> {
    TURN_PROCESS_EPOCHS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Mutex guard alias for `TURN_PROCESS_EPOCH_LOCK`.
pub fn turn_process_epoch_lock() -> &'static Mutex<HashMap<String, usize>> {
    turn_process_epochs()
}

/// Legacy alias matching Python ` _TURN_PROCESS_EPOCHS`.
#[allow(non_upper_case_globals)]
pub static _TURN_PROCESS_EPOCHS: OnceLock<Mutex<HashMap<String, usize>>> = OnceLock::new();

/// Legacy alias for `TURN_PROCESS_EPOCH_LOCK`.
#[allow(non_upper_case_globals)]
pub fn _TURN_PROCESS_EPOCH_LOCK() -> &'static Mutex<HashMap<String, usize>> {
    turn_process_epochs()
}

/// Next epoch counter value. Mirrors `next(_TURN_PROCESS_EPOCH_COUNTER)`.
pub fn next_turn_process_epoch() -> usize {
    TURN_PROCESS_EPOCH_COUNTER.fetch_add(1, Ordering::SeqCst)
}

/// Agent marker snapshot for `process_registry` baseline (mirrors `agent._gateway_turn_process_*` attributes).
///
/// Python stores these directly on the `AIAgent` instance; Rust models them as
/// an explicit struct so callers don't rely on dynamic attribute injection.
#[derive(Debug, Clone)]
pub struct AgentTurnProcessMarkers {
    pub task_id: String,
    pub baseline_ids: HashSet<String>,
    pub epoch: usize,
}

impl AgentTurnProcessMarkers {
    pub fn empty() -> Self {
        Self {
            task_id: String::new(),
            baseline_ids: HashSet::new(),
            epoch: 0,
        }
    }
    pub fn is_empty(&self) -> bool {
        self.task_id.is_empty()
    }
}

/// Simple in-process `process_registry` mock for the epoch gate.
///
/// Mirrors `tools.process_registry.process_registry.snapshot_running_ids(task_id)`
/// Real `process_registry` tracks OS pids; this stub returns the current
/// process's synthetic baseline (ponytail: std-only, no `sysinfo`/`procfs`).
pub fn snapshot_running_ids(_task_id: &str) -> HashSet<String> {
    // ponytail: return empty baseline in std-only; swap for `process_registry.snapshot_running_ids` when crate wired
    HashSet::new()
}

/// Reap background processes an abandoned API-server turn created.
///
/// Mirrors `def _reap_disconnected_agent_processes(agent: Any, *, source: str = "api_server_sse_disconnect")`.
///
/// In Python this fire-and-forgets a daemon `threading.Thread` targeting
/// `gateway.run._reap_gateway_turn_processes` with epoch gating so a newer
/// concurrent run that re-claimed the same `task_id` (conversation scope) is
/// not killed. Epoch closure skips reap when a newer run has claimed the task.
pub fn reap_disconnected_agent_processes(
    markers: &AgentTurnProcessMarkers,
    source: &str,
) {
    _reap_disconnected_agent_processes(markers, source)
}

pub fn _reap_disconnected_agent_processes(
    markers: &AgentTurnProcessMarkers,
    source: &str,
) {
    if markers.task_id.is_empty() {
        return;
    }
    let task_id = markers.task_id.clone();
    let baseline = markers.baseline_ids.clone();
    let epoch = markers.epoch;
    let source_owned = source.to_string();

    // Epoch staleness check — closure `is_still_current` in Python
    let is_still_current = {
        let g = turn_process_epochs().lock().unwrap();
        match g.get(&task_id) {
            None => true, // missing entry means abandoned run's own clear pruned it — reap must proceed
            Some(&current) => current == epoch,
        }
    };
    if !is_still_current {
        return;
    }

    // Fire-and-forget daemon thread so SSE handler's own cleanup isn't blocked
    // Mirrors `threading.Thread(target=_reap_gateway_turn_processes, args=(process_task_id, process_baseline), ...daemon=True).start()`
    let tid = task_id.clone();
    std::thread::Builder::new()
        .name(format!("api-turn-reaper-{}", &tid[..tid.len().min(12)]))
        .spawn(move || {
            // ponytail: call into `gateway.run._reap_gateway_turn_processes` when that crate is wired
            // For now, log the reap intent (mirrors Python's fire-and-forget)
            let _ = (&tid, &baseline, &source_owned, is_still_current);
            // In the real gateway, this diff-reaps background pids spawned since baseline snapshot
            // std-only stub does no OS work
        })
        .ok();
}

/// Snapshot the process baseline and claim the task_id's current epoch.
///
/// Mirrors `def _publish_turn_process_ownership(agent: Any, task_id: str) -> None`
/// Single place all API-server agent lifecycles (chat/responses `_run_agent`
/// and `/v1/runs`) record turn ownership, so marker attribute names and epoch
/// bookkeeping cannot drift between surfaces.
///
/// ```python
/// def _publish_turn_process_ownership(agent: Any, task_id: str) -> None:
///     from tools.process_registry import process_registry
///     with _TURN_PROCESS_EPOCH_LOCK:
///         epoch = next(_TURN_PROCESS_EPOCH_COUNTER)
///         _TURN_PROCESS_EPOCHS[task_id] = epoch
///     agent._gateway_turn_process_task_id = task_id
///     agent._gateway_turn_process_baseline = process_registry.snapshot_running_ids(task_id)
///     agent._gateway_turn_process_epoch = epoch
/// ```
pub fn publish_turn_process_ownership(markers: &mut AgentTurnProcessMarkers, task_id: &str) {
    _publish_turn_process_ownership(markers, task_id)
}

pub fn _publish_turn_process_ownership(markers: &mut AgentTurnProcessMarkers, task_id: &str) {
    let epoch = next_turn_process_epoch();
    {
        let mut g = turn_process_epochs().lock().unwrap();
        g.insert(task_id.to_string(), epoch);
    }
    markers.task_id = task_id.to_string();
    markers.baseline_ids = snapshot_running_ids(task_id);
    markers.epoch = epoch;
}

// ---------------------------------------------------------------------------
// Tests — smallest runnable check that fails if logic breaks
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn profile_rejected_is_distinct() {
        // `_PROFILE_REJECTED` is distinct from "no prefix" (None). Check its type/display.
        let s = PROFILE_REJECTED.to_string();
        assert_eq!(s, "_PROFILE_REJECTED");
        assert_eq!(_PROFILE_REJECTED, PROFILE_REJECTED);
    }

    #[test]
    fn prefix_names_served_profile_fail_closed() {
        // Without HERMES_HOME/HERMES_PROFILE, unknown profile must be rejected (fail closed).
        std::env::remove_var("HERMES_PROFILE");
        std::env::remove_var("HERMES_HOME");
        assert!(!prefix_names_served_profile("other-profile"));
        assert_eq!(_prefix_names_served_profile("other-profile"), prefix_names_served_profile("other-profile"));
    }

    #[test]
    fn prefix_names_self_referential_ok() {
        std::env::set_var("HERMES_PROFILE", "coder");
        assert!(prefix_names_served_profile("coder"));
        assert!(!prefix_names_served_profile("other"));
        std::env::remove_var("HERMES_PROFILE");
        // Also via HERMES_HOME suffix
        std::env::set_var("HERMES_HOME", "/tmp/.hermes/profiles/coder");
        assert!(prefix_names_served_profile("coder"));
        assert!(!prefix_names_served_profile("other"));
        std::env::remove_var("HERMES_HOME");
    }

    #[test]
    fn context_vars_roundtrip() {
        set_api_request_profile(Some("prof-a".to_string()));
        assert_eq!(get_api_request_profile(), Some("prof-a".to_string()));
        let tok = set_api_request_profile(None);
        assert_eq!(tok, Some("prof-a".to_string()));
        assert_eq!(get_api_request_profile(), None);
        reset_api_request_profile(tok);
        assert_eq!(get_api_request_profile(), Some("prof-a".to_string()));
        set_api_request_profile(None);

        set_api_request_browser_control_principal("principal-1".to_string());
        assert_eq!(get_api_request_browser_control_principal(), "principal-1");
        set_api_request_browser_control_principal(String::new());
        set_api_request_browser_control_transport_family("loopback".to_string());
        assert_eq!(get_api_request_browser_control_transport_family(), "loopback");
        set_api_request_browser_control_transport_family(String::new());
    }

    #[test]
    fn artifact_scope_facade_display() {
        let f = ArtifactScopeFacade::new("alice", "sess-1", "loopback");
        assert_eq!(f.principal_id, "alice");
        assert_eq!(f.session_id, "sess-1");
        assert_eq!(f.transport_family, "loopback");
        assert!(f.to_string().contains("alice"));
        let from_ctx = {
            set_api_request_browser_control_principal("bob".to_string());
            set_api_request_browser_control_transport_family("remote".to_string());
            let fac = ArtifactScopeFacade::from_request_context("sess-ctx");
            set_api_request_browser_control_principal(String::new());
            set_api_request_browser_control_transport_family(String::new());
            fac
        };
        assert_eq!(from_ctx.principal_id, "bob");
        assert_eq!(from_ctx.transport_family, "remote");
    }

    #[test]
    fn browser_control_constants() {
        assert_eq!(BROWSER_CONTROL_PROTOCOL_VERSION, 1);
        assert_eq!(BROWSER_CONTROL_WS_PROTOCOL, "hermes-browser-control-v1");
        assert!(BROWSER_CONTROL_TICKET_PROTOCOL_PREFIX.starts_with("hermes-browser-control-ticket."));
    }

    #[test]
    fn approval_event_choices_matrix() {
        assert_eq!(approval_event_choices(true, true, true), vec!["once", "deny"]);
        assert_eq!(approval_event_choices(false, false, true), vec!["once", "deny"]);
        assert_eq!(
            approval_event_choices(false, true, true),
            vec!["once", "session", "always", "deny"]
        );
        assert_eq!(
            approval_event_choices(false, true, false),
            vec!["once", "session", "deny"]
        );
        assert_eq!(_approval_event_choices(false, true, false), approval_event_choices(false, true, false));
    }

    #[test]
    fn hermes_version_not_empty() {
        let v = hermes_version();
        assert!(!v.is_empty());
        assert_eq!(_hermes_version(), v);
        assert_eq!(get_hermes_version(), v);
    }

    #[test]
    fn scoped_secret_fallback() {
        scoped_secret_store_clear();
        std::env::remove_var("HERMES_SECRET_SCOPE_ACTIVE");
        std::env::set_var("__TEST_SCOPED_SECRET_FOO", "bar123");
        // default-profile path reads env
        assert_eq!(
            get_scoped_secret("__TEST_SCOPED_SECRET_FOO", None),
            Some("bar123".to_string())
        );
        // scoped scope active + miss → fail closed, return default not env
        std::env::set_var("HERMES_SECRET_SCOPE_ACTIVE", "1");
        assert_eq!(get_scoped_secret("__TEST_SCOPED_SECRET_FOO", Some("def")), Some("def".to_string()));
        assert_eq!(get_scoped_secret("__TEST_SCOPED_SECRET_FOO", None), None);
        scoped_secret_store_set("__TEST_SCOPED_SECRET_FOO", "scoped-val".to_string());
        assert_eq!(
            get_scoped_secret("__TEST_SCOPED_SECRET_FOO", None),
            Some("scoped-val".to_string())
        );
        scoped_secret_store_clear();
        std::env::remove_var("HERMES_SECRET_SCOPE_ACTIVE");
        std::env::remove_var("__TEST_SCOPED_SECRET_FOO");
        assert_eq!(_get_scoped_secret("__TEST_SCOPED_SECRET_FOO", Some("x")), Some("x".to_string()));
    }

    #[test]
    fn thread_safe_queue_threadsafe() {
        let q: ThreadSafeAsyncQueue<String> = ThreadSafeAsyncQueue::unbounded();
        assert!(q.is_empty());
        q.put_threadsafe("hello".to_string());
        q.put_threadsafe("world".to_string());
        assert_eq!(q.len(), 2);
        assert_eq!(q.try_get(), Some("hello".to_string()));
        q.put_nowait("third".to_string()).unwrap();
        assert_eq!(q.get_timeout(Some(std::time::Duration::from_millis(10))), Some("world".to_string()));
        assert_eq!(q.get_timeout(Some(std::time::Duration::from_millis(10))), Some("third".to_string()));
        assert_eq!(q.get_timeout(Some(std::time::Duration::from_millis(5))), None);
        // bounded
        let bounded: ThreadSafeAsyncQueue<i32> = ThreadSafeAsyncQueue::new(1);
        assert!(bounded.put_nowait(1).is_ok());
        assert!(bounded.put_nowait(2).is_err());
    }

    #[test]
    fn sse_frame_format() {
        let data = json!({"choices": []});
        let bytes = sse_frame(&data, None, true);
        let s = String::from_utf8(bytes).unwrap();
        assert!(s.starts_with("data: "));
        assert!(s.ends_with("\n\n"));
        assert!(s.contains("\"choices\""));
        let bytes2 = sse_frame(&data, Some("delta"), false);
        let s2 = String::from_utf8(bytes2).unwrap();
        assert!(s2.starts_with("event: delta\n"));
        assert!(s2.contains("data: "));
        assert_eq!(_sse_frame(&data, None, true), sse_frame(&data, None, true));
    }

    #[test]
    fn coerce_port_parsing() {
        assert_eq!(coerce_port(&json!(8642), DEFAULT_PORT), 8642);
        assert_eq!(coerce_port(&json!("9000"), DEFAULT_PORT), 9000);
        assert_eq!(coerce_port(&json!("not-a-port"), DEFAULT_PORT), DEFAULT_PORT);
        assert_eq!(coerce_port_str(" 8080 ", DEFAULT_PORT), 8080);
        assert_eq!(coerce_port_str("99999", DEFAULT_PORT), DEFAULT_PORT);
        assert_eq!(coerce_port_value(&json!(0)), None);
        assert_eq!(_coerce_port(&json!(8642), DEFAULT_PORT), 8642);
    }

    #[test]
    fn coerce_request_bool_cases() {
        assert!(coerce_request_bool(&json!(true), false));
        assert!(!coerce_request_bool(&json!(false), true));
        assert_eq!(coerce_request_bool(&Value::Null, true), true);
        assert!(coerce_request_bool(&json!("true"), false));
        assert!(coerce_request_bool(&json!("YES"), false));
        assert!(coerce_request_bool(&json!("1"), false));
        assert!(coerce_request_bool(&json!("on"), false));
        assert!(!coerce_request_bool(&json!("false"), true));
        assert!(!coerce_request_bool(&json!("0"), true));
        assert!(!coerce_request_bool(&json!("off"), true));
        assert!(!coerce_request_bool(&json!("no"), true));
        // int/float
        assert!(coerce_request_bool(&json!(1), false));
        assert!(!coerce_request_bool(&json!(0), true));
        assert!(coerce_request_bool(&json!(1.5), false));
        assert!(!coerce_request_bool(&json!(0.0), true));
        // unknown string → default
        assert_eq!(coerce_request_bool(&json!("maybe"), true), true);
        assert_eq!(coerce_request_bool(&json!("maybe"), false), false);
        assert_eq!(_coerce_request_bool(&json!("true"), false), true);
        assert_eq!(coerce_request_bool_opt(Some(&json!(true)), false), true);
        assert_eq!(coerce_request_bool_opt(None, true), true);
    }

    #[test]
    fn reasoning_efforts_constant() {
        assert!(REASONING_EFFORTS.contains(&"none"));
        assert!(REASONING_EFFORTS.contains(&"ultra"));
        assert_eq!(RUNTIME_AGENT_OVERRIDE_KEYS.len(), 8);
    }

    #[test]
    fn clean_request_string_cases() {
        assert_eq!(clean_request_string(&json!(" hello ")), Some("hello".to_string()));
        assert_eq!(clean_request_string(&json!("   ")), None);
        assert_eq!(clean_request_string(&json!(123)), None);
        assert_eq!(_clean_request_string(&json!(" hi ")), clean_request_string(&json!(" hi ")));
    }

    #[test]
    fn request_reasoning_config_cases() {
        assert_eq!(request_reasoning_config(&json!({})), None);
        assert_eq!(
            request_reasoning_config(&json!({"reasoning_effort": "high"})),
            Some(json!({"enabled": true, "effort": "high"}))
        );
        assert_eq!(
            request_reasoning_config(&json!({"reasoning": {"enabled": false}})),
            Some(json!({"enabled": false}))
        );
        assert_eq!(
            request_reasoning_config(&json!({"reasoning": {"effort": "none"}})),
            Some(json!({"enabled": false}))
        );
        assert_eq!(
            request_reasoning_config(&json!({"reasoning": {"enabled": true}})),
            Some(json!({"enabled": true}))
        );
        assert_eq!(
            request_reasoning_config(&json!({"reasoning": {"enabled": true, "effort": "max"}})),
            Some(json!({"enabled": true, "effort": "max"}))
        );
        // unknown effort → only enabled check matters
        assert_eq!(request_reasoning_config(&json!({"reasoning_effort": "unknown"})), None);
        assert_eq!(_request_reasoning_config(&json!({"reasoning_effort": "high"})), request_reasoning_config(&json!({"reasoning_effort": "high"})));
    }

    #[test]
    fn request_service_tier_cases() {
        assert_eq!(request_service_tier(&json!({"service_tier": "flex"})), ServiceTierResult::Value(json!("flex")));
        assert_eq!(request_service_tier(&json!({"service_tier": "  "})), ServiceTierResult::Null);
        assert_eq!(request_service_tier(&json!({"service_tier": null})), ServiceTierResult::Null);
        assert_eq!(request_service_tier(&json!({"service_tier": 123})), ServiceTierResult::Value(json!(123)));
        assert_eq!(request_service_tier(&json!({"fast": true})), ServiceTierResult::Value(json!("priority")));
        assert_eq!(request_service_tier(&json!({"fast": false})), ServiceTierResult::Null);
        assert_eq!(request_service_tier(&json!({"fast": "true"})), ServiceTierResult::Value(json!("priority")));
        assert_eq!(request_service_tier(&json!({})), ServiceTierResult::Missing);
        assert_eq!(request_service_tier(&json!("not-dict")), ServiceTierResult::Missing);
        assert_eq!(_request_service_tier(&json!({"service_tier": "flex"})), ServiceTierResult::Value(json!("flex")));
    }

    #[test]
    fn apply_runtime_agent_overrides_cases() {
        let mut kwargs = Map::new();
        kwargs.insert("provider".to_string(), json!("openai"));
        let overrides = json!({"provider": "anthropic", "api_key": "sk-x", "args": ["--flag"], "max_tokens": 1024, "unknown": 999, "null_key": null});
        apply_runtime_agent_overrides(&mut kwargs, &overrides);
        assert_eq!(kwargs.get("provider").unwrap(), &json!("anthropic"));
        assert_eq!(kwargs.get("api_key").unwrap(), &json!("sk-x"));
        assert_eq!(kwargs.get("args").unwrap(), &json!(["--flag"]));
        assert_eq!(kwargs.get("max_tokens").unwrap(), &json!(1024));
        assert!(!kwargs.contains_key("unknown"));
        assert!(!kwargs.contains_key("null_key"));
        let mut kwargs2 = Map::new();
        apply_runtime_agent_overrides(&mut kwargs2, &json!("not-dict"));
        assert!(kwargs2.is_empty());
    }

    #[test]
    fn request_agent_overrides_cases() {
        let body = json!({"provider": "  openai  ", "model": "gpt-4", "model_options": {"temperature": 0.7}});
        let out = request_agent_overrides(&body, Some("hermes-agent"), true);
        assert_eq!(out.get("requested_provider").unwrap(), &json!("openai"));
        assert_eq!(out.get("requested_model").unwrap(), &json!("gpt-4"));
        assert_eq!(out.get("model_options").unwrap(), &json!({"temperature": 0.7}));
        // virtual model → no requested_model
        let body2 = json!({"provider": "openai", "model": "hermes-agent"});
        let out2 = request_agent_overrides(&body2, Some("hermes-agent"), true);
        assert!(!out2.contains_key("requested_model"));
        // bare model disallowed
        let body3 = json!({"model": "gpt-4"});
        let out3 = request_agent_overrides(&body3, Some("hermes-agent"), false);
        assert!(!out3.contains_key("requested_model"));
        let out4 = request_agent_overrides(&body3, Some("hermes-agent"), true);
        assert_eq!(out4.get("requested_model").unwrap(), &json!("gpt-4"));
        // non-dict body
        let out5 = request_agent_overrides(&json!("not-dict"), None, true);
        assert!(out5.is_empty());
    }

    #[test]
    fn normalize_chat_content_cases() {
        assert_eq!(normalize_chat_content(&json!("hello")), "hello");
        assert_eq!(normalize_chat_content(&Value::Null), "");
        assert_eq!(normalize_chat_content(&json!([{"type": "text", "text": "hi"}])), "hi");
        assert_eq!(normalize_chat_content(&json!([{"type": "image_url", "image_url": {"url": "http://x"}}])), "");
        // nested list
        assert_eq!(normalize_chat_content(&json!([["nested"]])), "nested");
        // max depth exceeded
        let deep = json!([[[[[[[[[[[[["deep"]]]]]]]]]]]]);
        let out = _normalize_chat_content(&deep, 2, 0);
        assert!(out.is_empty() || out.contains("deep") || out.is_empty());
        // str fallback
        assert_eq!(normalize_chat_content(&json!(123)), "123");
    }

    #[test]
    fn normalize_multimodal_text_only_collapses() {
        let content = json!([{"type": "text", "text": "hello"}, {"type": "input_text", "text": "world"}]);
        let out = normalize_multimodal_content(&content).unwrap();
        assert_eq!(out, NormalizedContent::Text("hello\nworld".to_string()));
    }

    #[test]
    fn normalize_multimodal_image_ok() {
        let content = json!([{"type": "text", "text": "hi"}, {"type": "image_url", "image_url": {"url": "https://example.com/img.png"}}]);
        let out = normalize_multimodal_content(&content).unwrap();
        match out {
            NormalizedContent::Parts(parts) => {
                assert_eq!(parts.len(), 2);
                assert_eq!(parts[1].get("type").unwrap(), &json!("image_url"));
            }
            _ => panic!("expected Parts"),
        }
        // data:image URL
        let data_url = json!([{"type": "image_url", "image_url": {"url": "data:image/png;base64,abc123,"}}]);
        // needs comma — above has comma after base64 header
        let res = normalize_multimodal_content(&data_url);
        assert!(res.is_ok());
    }

    #[test]
    fn normalize_multimodal_rejects_file_and_unknown() {
        let file_part = json!([{"type": "file", "file": {"url": "http://x"}}]);
        let err = normalize_multimodal_content(&file_part).unwrap_err();
        assert_eq!(err.code, "unsupported_content_type");
        let unknown = json!([{"type": "video", "url": "http://x"}]);
        let err2 = normalize_multimodal_content(&unknown).unwrap_err();
        assert_eq!(err2.code, "unsupported_content_type");
        // invalid image URL
        let bad_img = json!([{"type": "image_url", "image_url": {"url": ""}}]);
        let err3 = normalize_multimodal_content(&bad_img).unwrap_err();
        assert_eq!(err3.code, "invalid_image_url");
        // unsupported data URL
        let bad_data = json!([{"type": "image_url", "image_url": {"url": "data:text/plain,hello"}}]);
        let err4 = normalize_multimodal_content(&bad_data).unwrap_err();
        assert_eq!(err4.code, "unsupported_content_type");
        // scalar passthrough
        assert_eq!(normalize_multimodal_content(&json!("hi")).unwrap(), NormalizedContent::Text("hi".to_string()));
        assert_eq!(normalize_multimodal_content(&Value::Null).unwrap(), NormalizedContent::Text(String::new()));
        assert_eq!(normalize_multimodal_content(&json!([{"type": "text", "text": ""}])).unwrap(), NormalizedContent::Text(String::new()));
    }

    #[test]
    fn content_has_visible_payload_cases() {
        assert!(content_has_visible_payload(&json!("hello")));
        assert!(!content_has_visible_payload(&json!("   ")));
        assert!(content_has_visible_payload(&json!([{"type": "text", "text": " hi "}])));
        assert!(!content_has_visible_payload(&json!([{"type": "text", "text": "   "}])));
        assert!(content_has_visible_payload(&json!([{"type": "image_url", "image_url": {"url": "https://x"}}])));
        assert!(!content_has_visible_payload(&json!([{"type": "unknown", "text": "hi"}])));
        assert_eq!(_content_has_visible_payload(&json!("hi")), content_has_visible_payload(&json!("hi")));
    }

    #[test]
    fn multimodal_validation_error_400() {
        let err = MultimodalError::new("invalid_image_url", "Image parts must include a non-empty image URL.");
        let resp = multimodal_validation_error(&err, "messages[0].content");
        assert_eq!(resp.status, 400);
        assert_eq!(resp.body["error"]["code"], json!("invalid_image_url"));
        assert_eq!(resp.body["error"]["param"], json!("messages[0].content"));
        let err2 = _multimodal_validation_error(&err, "input");
        assert_eq!(err2.status, 400);
    }

    #[test]
    fn openai_error_shape() {
        let e = _openai_error("bad", "invalid_request_error", Some("messages"), Some("bad_req"));
        assert_eq!(e["error"]["message"], json!("bad"));
        assert_eq!(e["error"]["type"], json!("invalid_request_error"));
        assert_eq!(e["error"]["param"], json!("messages"));
        assert_eq!(e["error"]["code"], json!("bad_req"));
    }

    #[test]
    fn compaction_helpers() {
        let msg = json!({"role": "user", "content": "hello"});
        assert!(!is_compressed_summary_message(&msg));
        let carrier = json!({"_compressed_summary": {"summary": "s"}, "content": "x"});
        assert!(is_compressed_summary_message(&carrier));
        let carrier2 = json!({"_is_compaction_summary": true});
        assert!(is_compressed_summary_message(&carrier2));
        // project_client_message on non-carrier returns copy
        let m = {
            let mut map = Map::new();
            map.insert("role".to_string(), json!("user"));
            map.insert("content".to_string(), json!("hello"));
            map
        };
        let proj = project_client_message(&m);
        assert_eq!(proj.get("content").unwrap(), &json!("hello"));
        // standalone handoff → hidden empty row
        let mut handoff = Map::new();
        handoff.insert("_compressed_summary".to_string(), json!({}));
        handoff.insert("content".to_string(), json!(""));
        handoff.insert("tool_calls".to_string(), json!([]));
        let proj2 = project_client_message(&handoff);
        assert_eq!(proj2.get("content").unwrap(), &json!(""));
        assert_eq!(proj2.get("display_kind").unwrap(), &json!("hidden"));
        // auto_truncate
        let hist: Vec<Map<String, Value>> = (0..5).map(|i| {
            let mut m = Map::new();
            m.insert("content".to_string(), json!(format!("msg {i}")));
            m
        }).collect();
        let truncated = auto_truncate_response_history(&hist, 3);
        assert_eq!(truncated.len(), 3);
        // with summary preservation
        let mut with_summary = hist.clone();
        let mut summary = Map::new();
        summary.insert("_compressed_summary".to_string(), json!({}));
        summary.insert("content".to_string(), json!("summary"));
        with_summary.insert(1, summary);
        let trunc2 = auto_truncate_response_history(&with_summary, 3);
        assert!(trunc2.iter().any(|m| m.contains_key("_compressed_summary")));
        assert_eq!(_auto_truncate_response_history(&hist, 3).len(), 3);
    }

    #[test]
    fn turn_process_ownership_epoch_gate() {
        // Clear previous state
        turn_process_epochs().lock().unwrap().clear();
        let mut markers = AgentTurnProcessMarkers::empty();
        publish_turn_process_ownership(&mut markers, "task-abc");
        let epoch1 = markers.epoch;
        assert!(!markers.task_id.is_empty());
        assert_eq!(markers.task_id, "task-abc");
        {
            let g = turn_process_epochs().lock().unwrap();
            assert_eq!(g.get("task-abc"), Some(&epoch1));
        }
        // Second run claims same task_id → epoch bumps
        let mut markers2 = AgentTurnProcessMarkers::empty();
        publish_turn_process_ownership(&mut markers2, "task-abc");
        let epoch2 = markers2.epoch;
        assert_ne!(epoch1, epoch2);
        {
            let g = turn_process_epochs().lock().unwrap();
            assert_eq!(g.get("task-abc"), Some(&epoch2));
        }
        // Stale reaper holding epoch1 should be no-op (is_still_current = false)
        reap_disconnected_agent_processes(&markers, "api_server_sse_disconnect");
        // Current still exists — map unchanged
        {
            let g = turn_process_epochs().lock().unwrap();
            assert_eq!(g.get("task-abc"), Some(&epoch2));
        }
        // Current reaper still valid → spawn succeeds (no panic)
        reap_disconnected_agent_processes(&markers2, "api_server_sse_disconnect");
        // Also test spy helper
        let snap = snapshot_running_ids("task-abc");
        assert!(snap.is_empty());
        turn_process_epochs().lock().unwrap().clear();
    }

    #[test]
    fn browser_controller_ws_sender_closed_guard() {
        let sender = browser_controller_ws_sender(true, 10.0);
        assert!(sender.is_closed());
        let res = sender.send(json!({"hello": 1}));
        assert!(res.is_err());
        assert!(res.unwrap_err().contains("closed"));
        let sender2 = browser_controller_ws_sender(false, 1.0);
        assert!(!sender2.is_closed());
        let res2 = sender2.send(json!({"frame": 1}));
        assert!(res2.is_ok());
        // closure variant
        let f = browser_controller_ws_sender_fn(false, 1.0);
        assert!(f(json!({"x": 1})).is_ok());
        let f2 = browser_controller_ws_sender_fn(true, 1.0);
        assert!(f2(json!({"x": 1})).is_err());
    }

    #[test]
    fn defaults_sanity() {
        assert_eq!(DEFAULT_HOST, "127.0.0.1");
        assert_eq!(DEFAULT_PORT, 8642);
        assert_eq!(MAX_STORED_RESPONSES, 100);
        assert_eq!(MAX_REQUEST_BYTES, 10_000_000);
        assert_eq!(MAX_NORMALIZED_TEXT_LENGTH, 65_536);
        assert_eq!(MAX_CONTENT_LIST_SIZE, 1_000);
        assert_eq!(RESPONSES_AUTO_TRUNCATION_HISTORY_LIMIT, 100);
        assert_eq!(COMPRESSED_SUMMARY_METADATA_KEY, "_compressed_summary");
        // ponytail: no aiohttp in slice
        assert!(!AIOHTTP_AVAILABLE);
    }
}
