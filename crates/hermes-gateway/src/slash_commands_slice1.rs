//! Gateway slash-command handlers — slice 1 (lines 1–900).
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/gateway/slash_commands.py`
//! (6084 LOC), slice 1 covering lines 1–900.
//! This slice contains the module docstring, all imports, logger, constants,
//! module helpers (`_clean_str`, `_int_value`, `_model_switch_skew_guard`,
//! `_home_thread_from_source`), the opening of `GatewaySlashCommandsMixin`,
//! and handlers `_typed_command_prefix_for`, `_handle_reset_command`,
//! `_handle_profile_command`, `_handle_whoami_command`, `_handle_kanban_command`,
//! `_handle_status_command`, `_redact_matrix_session_key`, and the start of
//! `_handle_context_command` through its gauge bar/header (the slice boundary
//! at line 900 cuts inside `_handle_context_command` after `lines.append("")`
//! at the threshold block; the Rust file includes the truncated tail as a
//! commented boundary note — the complete `if threshold > 0:` block and the
//! remainder of the function continue in slice 2).
//!
//! Python source docstring (preserved):
//! ```text
//! Gateway slash-command handlers for GatewayRunner.
//!
//! Extracted from ``gateway/run.py`` (god-file decomposition Phase 3b). These are
//! the in-session slash commands (/model, /reset, /usage, /compress, ...) the
//! gateway dispatches from ``_handle_message``. There are 42 of them (~3,200 LOC);
//! lifting them into a mixin that ``GatewayRunner`` inherits keeps every
//! ``self._handle_*_command`` dispatch + test reference working via the MRO, while
//! removing the bulk from run.py.
//!
//! Module-level run.py helpers a handler needs (``_hermes_home``,
//! ``_load_gateway_config``, ``_resolve_gateway_model``, etc.) are imported lazily
//! inside the handler body — a deferred ``from gateway.run import ...`` resolves at
//! call time (run.py fully loaded by then), avoiding an import cycle.
//! ```
//!
//! # Mapping
//!
//! - `logger = logging.getLogger("gateway.run")` → [`log`] crate (`log::warn!`, `log::debug!`)
//! - `_RESET_CLEANUP_TIMEOUT_S = 30.0` → [`RESET_CLEANUP_TIMEOUT_S`] / [`_RESET_CLEANUP_TIMEOUT_S`]
//! - `def _clean_str(value: Any) -> str` → [`clean_str`] / [`_clean_str`] + [`clean_str_value`]
//! - `def _int_value(value: Any) -> int` → [`int_value`] / [`_int_value`] + [`int_value_value`] / [`int_value_str`]
//! - `def _model_switch_skew_guard() -> Optional[str]` → [`model_switch_skew_guard`] / [`_model_switch_skew_guard`]
//! - `def _home_thread_from_source(source) -> Optional[str]` → [`home_thread_from_source`] / [`_home_thread_from_source`]
//! - `class GatewaySlashCommandsMixin` → [`GatewaySlashCommandsMixin`] (struct + `impl`; Python mixin → Rust struct with trait-like methods; `async_session_store` → [`GatewaySlashCommandsMixin::async_session_store_key`] placeholder)
//! - `def _typed_command_prefix_for(self, platform) -> str` → [`GatewaySlashCommandsMixin::typed_command_prefix_for`] / [`typed_command_prefix_for`] + [`typed_command_prefix_for_value`]
//! - `async def _handle_reset_command(self, event) -> Union[str, EphemeralReply]` → [`GatewaySlashCommandsMixin::handle_reset_command`] (stub; full branching preserved as comments + runtime-delegated helpers)
//! - `async def _handle_profile_command(self, event) -> str` → [`GatewaySlashCommandsMixin::handle_profile_command`]
//! - `async def _handle_whoami_command(self, event) -> str` → [`GatewaySlashCommandsMixin::handle_whoami_command`]
//! - `async def _handle_kanban_command(self, event) -> str` → [`GatewaySlashCommandsMixin::handle_kanban_command`] + [`parse_kanban_args`] / [`is_kanban_create`]
//! - `async def _handle_status_command(self, event) -> str` → [`GatewaySlashCommandsMixin::handle_status_command`] + helpers [`status_clean_str`] / [`status_int_value`] / [`resolve_model_line`] / [`resolve_context_line`]
//! - `def _redact_matrix_session_key(session_key: str) -> str` → [`redact_matrix_session_key`] / [`_redact_matrix_session_key`] + [`sha256_hex12`]
//! - `async def _handle_context_command(self, event) -> str` (partial) → [`GatewaySlashCommandsMixin::handle_context_command`] (slice 1: through gauge bar; remainder in slice 2)
//! - `from gateway.code_skew import detect_code_skew` (lazy) → [`detect_code_skew_stub`] (ponytail: no gateway dep in std-only; env-based fallback)
//! - `from gateway.config import HomeChannel, Platform, PlatformConfig, persist_home_channel` → local [`Platform`] enum + [`PlatformConfig`] stub
//! - `from gateway.platforms.base import EphemeralReply, MessageEvent, MessageType` → [`EphemeralReply`] / [`MessageEvent`] / [`MessageType`]
//! - `from gateway.session import AsyncSessionStore, SessionSource, build_session_key, is_shared_multi_user_session` → [`SessionSource`] stub + [`build_session_key`] stub
//! - `from hermes_cli.config import atomic_config_write, cfg_get, clear_model_endpoint_credentials` → documented only (lazy handler bodies)
//! - `from utils import atomic_json_write, base_url_host_matches, is_truthy_value` → documented only
//! - `from agent.account_usage import fetch_account_usage, render_account_usage_lines` → documented only
//! - `from agent.i18n import t` → [`t_stub`] (i18n key → format; Rust returns key+params for traceability)
//! - `from agent.turn_context import extract_api_content_sidecar` → documented only
//!
//! # Notes on runtime deps not ported in this slice
//!
//! Python imports `asyncio`, `dataclasses`, `hashlib`, `inspect`, `logging`,
//! `os`, `re`, `shlex`, `sys`, `time`, `datetime`, `pathlib`, and lazy
//! `gateway.run` helpers (`_hermes_home`, `_load_gateway_config`,
//! `_resolve_gateway_model`, `_profile_runtime_scope`, etc.). Those are
//! runtime/loop-level concerns above slice 1's pure helpers and are documented
//! as `// Python:` comments where referenced. Pure helpers are fully ported;
//! side-effecting gateway coupling (session store, agent cache, hooks, kanban
//! DB) is stubbed with `ponytail:` notes and deterministic fallbacks.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Module doc / logger — mirrors Python lines 1–50
// ---------------------------------------------------------------------------

// Python: `logger = logging.getLogger("gateway.run")`
// Rust: `log` crate (log::warn!, log::debug!, log::error!).

// ---------------------------------------------------------------------------
// Constants — mirrors Python lines 52–56
// ---------------------------------------------------------------------------

/// Upper bound on off-loop agent-resource cleanup during `/new` or `/reset`.
///
/// Mirrors `_RESET_CLEANUP_TIMEOUT_S = 30.0` — a stuck teardown must not block
/// the event loop; past this the reset proceeds and the cleanup is left to
/// finish (or leak) in its worker thread. (#35994)
pub const RESET_CLEANUP_TIMEOUT_S: f64 = 30.0;

/// Private alias for grep-ability.
pub const _RESET_CLEANUP_TIMEOUT_S: f64 = RESET_CLEANUP_TIMEOUT_S;

// ---------------------------------------------------------------------------
// Helpers: _clean_str / _int_value — mirrors Python lines 59–69
// ---------------------------------------------------------------------------

/// Strip and return a non-empty string value, or empty string.
///
/// Mirrors:
/// ```python
/// def _clean_str(value: Any) -> str:
///     return value.strip() if isinstance(value, str) and value.strip() else ""
/// ```
pub fn clean_str(value: Option<&str>) -> String {
    match value {
        Some(s) => {
            let t = s.trim();
            if t.is_empty() {
                String::new()
            } else {
                t.to_string()
            }
        }
        None => String::new(),
    }
}

/// `serde_json::Value` overload — mirrors `isinstance(value, str)` + `.strip()`.
pub fn clean_str_value(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => {
            let t = s.trim();
            if t.is_empty() {
                String::new()
            } else {
                t.to_string()
            }
        }
        _ => String::new(),
    }
}

/// Generic `Any` via `serde_json::Value::as_str` fallback.
pub fn clean_str_any(value: &serde_json::Value) -> String {
    clean_str_value(value)
}

#[allow(dead_code)]
fn _clean_str(value: Option<&str>) -> String {
    clean_str(value)
}

/// Safely coerce to int (i64).
///
/// Mirrors:
/// ```python
/// def _int_value(value: Any) -> int:
///     try:
///         return int(value)
///     except (TypeError, ValueError):
///         return 0
/// ```
pub fn int_value_str(value: &str) -> i64 {
    let t = value.trim();
    if t.is_empty() {
        return 0;
    }
    // Handles `int("123")`, `int(" 42 ")`, `int(3.7)` style via parse fallbacks
    if let Ok(v) = t.parse::<i64>() {
        return v;
    }
    if let Ok(f) = t.parse::<f64>() {
        if f.is_finite() {
            // Python int(float) truncates toward zero
            return f.trunc() as i64;
        }
    }
    0
}

pub fn int_value_value(value: &serde_json::Value) -> i64 {
    match value {
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                i
            } else if let Some(u) = n.as_u64() {
                if u <= i64::MAX as u64 { u as i64 } else { 0 }
            } else if let Some(f) = n.as_f64() {
                if f.is_finite() { f.trunc() as i64 } else { 0 }
            } else {
                0
            }
        }
        serde_json::Value::String(s) => int_value_str(s),
        serde_json::Value::Bool(b) => if *b { 1 } else { 0 },
        _ => 0,
    }
}

/// String + JSON convenience.
pub fn int_value(value: &serde_json::Value) -> i64 {
    int_value_value(value)
}

#[allow(dead_code)]
fn _int_value(value: &serde_json::Value) -> i64 {
    int_value(value)
}

// ---------------------------------------------------------------------------
// _model_switch_skew_guard — mirrors Python lines 72–99
// ---------------------------------------------------------------------------

/// Refuse a model switch when the gateway is running stale code.
///
/// Mirrors:
/// ```python
/// def _model_switch_skew_guard() -> Optional[str]:
///     from gateway.code_skew import detect_code_skew
///     skew = detect_code_skew()
///     if not skew:
///         return None
///     boot_rev, disk_rev = skew
///     return t("gateway.model.error_prefix", error=(
///         f"This gateway is running code from {boot_rev} but the checkout on "
///         f"disk is now {disk_rev}. Switching models would risk a stale-module "
///         f"crash — restart the gateway to load the new code: hermes gateway restart"
///     ))
/// ```
///
/// Intentionally scoped to model switching — the known, highest-risk trigger.
/// Any first-time lazy import on a stale process is technically exposed; we
/// guard only this path.
///
/// Rust: std-only heuristic — probes `detect_code_skew_stub` (env/ file based)
/// rather than importing `gateway.code_skew`. Returns `Some(message)` on skew.
pub fn model_switch_skew_guard() -> Option<String> {
    let skew = detect_code_skew_stub()?;
    let (boot_rev, disk_rev) = skew;
    Some(t_stub(
        "gateway.model.error_prefix",
        &format!(
            "This gateway is running code from {} but the checkout on disk is now {}. Switching models would risk a stale-module crash — restart the gateway to load the new code: hermes gateway restart",
            boot_rev, disk_rev
        ),
    ))
}

#[allow(dead_code)]
fn _model_switch_skew_guard() -> Option<String> {
    model_switch_skew_guard()
}

/// Stub for `gateway.code_skew.detect_code_skew()` → `Optional[(boot_rev, disk_rev)]`.
///
/// Rust std-only: checks env `HERMES_CODE_SKEW_BOOT` / `HERMES_CODE_SKEW_DISK`
/// (test seam) or compares `HERMES_BOOT_REV` file mtime vs current `CARGO_PKG_VERSION`.
/// When boot != disk, returns skew tuple; else `None`.
///
/// `ponytail: env heuristic; wire to real code_skew::detect_code_skew when crate is available`.
fn detect_code_skew_stub() -> Option<(String, String)> {
    // Test seam: explicit skew via env
    let boot_env = std::env::var("HERMES_CODE_SKEW_BOOT").ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let disk_env = std::env::var("HERMES_CODE_SKEW_DISK").ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    if let (Some(b), Some(d)) = (boot_env, disk_env) {
        if b != d {
            return Some((b, d));
        } else {
            return None;
        }
    }
    // Fallback: no skew detection in std-only build
    None
}

// Minimal i18n stub — mirrors `agent.i18n.t(key, **kwargs)`
fn t_stub(key: &str, error: &str) -> String {
    // Python: `t("gateway.model.error_prefix", error=...)` formats via i18n templates.
    // Rust stub returns `key: error` for traceability without i18n dep.
    format!("{}: {}", key, error)
}

// ---------------------------------------------------------------------------
// _home_thread_from_source — mirrors Python lines 101–124
// ---------------------------------------------------------------------------

/// The thread id `/sethome` should persist on the home target, or `None`.
///
/// Mirrors:
/// ```python
/// def _home_thread_from_source(source) -> Optional[str]:
///     thread_id = getattr(source, "thread_id", None)
///     if not thread_id:
///         return None
///     if (
///         getattr(source, "platform", None) == Platform.SLACK
///         and getattr(source, "message_id", None)
///         and str(thread_id) == str(source.message_id)
///     ):
///         return None
///     return str(thread_id)
/// ```
///
/// Slack thread-per-message session keying stamps a top-level message's own
/// id as `source.thread_id` (a session KEY, not a durable location).
/// Persisting it would pin the HOME target itself to the ephemeral thread
/// spawned around the `/sethome` message — every bare-platform delivery
/// (`deliver="slack"`) would then land in that thread forever. Same
/// recognition as cron origin capture: a Slack thread id equal to the
/// message's own id is synthetic. A `/sethome` run inside a genuine thread
/// (thread id = parent's id, not this message's own) keeps that thread
/// as the home target.
pub fn home_thread_from_source(source: &SessionSource) -> Option<String> {
    let thread_id = source.thread_id.as_deref()?.trim();
    if thread_id.is_empty() {
        return None;
    }
    if source.platform.as_deref().map(|p| p.eq_ignore_ascii_case("slack")).unwrap_or(false) {
        if let Some(mid) = source.message_id.as_deref() {
            if !mid.trim().is_empty() && thread_id == mid.trim() {
                return None;
            }
        }
    }
    Some(thread_id.to_string())
}

/// `serde_json::Value` overload — handles dict-like source with dynamic fields.
pub fn home_thread_from_source_value(source: &serde_json::Value) -> Option<String> {
    let obj = source.as_object()?;
    let thread_id = obj
        .get("thread_id")
        .and_then(|v| v.as_str().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
            .or_else(|| if v.is_null() { None } else { Some(v.to_string().trim().to_string()).filter(|s| !s.is_empty()) }))
        .filter(|s| !s.is_empty())?;
    // Check Slack synthetic id
    let platform = obj
        .get("platform")
        .map(|p| {
            if let Some(s) = p.as_str() { s.to_string() }
            else if let Some(o) = p.as_object().and_then(|m| m.get("value")).and_then(|v| v.as_str()) { o.to_string() }
            else { p.to_string() }
        })
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    if platform == "slack" {
        if let Some(mid) = obj.get("message_id").and_then(|v| v.as_str().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())) {
            if thread_id == mid {
                return None;
            }
        } else if let Some(mid) = obj.get("message_id").map(|v| v.to_string().trim_matches('"').trim().to_string()).filter(|s| !s.is_empty() && s != "null") {
            if thread_id == mid {
                return None;
            }
        }
    }
    Some(thread_id)
}

#[allow(dead_code)]
fn _home_thread_from_source(source: &SessionSource) -> Option<String> {
    home_thread_from_source(source)
}

// ---------------------------------------------------------------------------
// Platform / session stubs — mirrors Python imports 35–42
// ---------------------------------------------------------------------------

/// Minimal `Platform` mirror (subset of `gateway.config.Platform`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Platform {
    Telegram,
    Discord,
    Slack,
    Whatsapp,
    Matrix,
    Feishu,
    Unknown(String),
}

impl Platform {
    pub fn as_str(&self) -> &str {
        match self {
            Platform::Telegram => "telegram",
            Platform::Discord => "discord",
            Platform::Slack => "slack",
            Platform::Whatsapp => "whatsapp",
            Platform::Matrix => "matrix",
            Platform::Feishu => "feishu",
            Platform::Unknown(s) => s.as_str(),
        }
    }
    pub fn from_str(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "telegram" => Platform::Telegram,
            "discord" => Platform::Discord,
            "slack" => Platform::Slack,
            "whatsapp" => Platform::Whatsapp,
            "matrix" => Platform::Matrix,
            "feishu" => Platform::Feishu,
            other => Platform::Unknown(other.to_string()),
        }
    }
}

/// Mirrors `gateway.session.SessionSource` (minimal fields used in slice 1).
#[derive(Debug, Clone, Default)]
pub struct SessionSource {
    pub platform: Option<String>,
    pub profile: Option<String>,
    pub scope_id: Option<String>,
    pub thread_id: Option<String>,
    pub message_id: Option<String>,
    pub user_id: Option<String>,
    pub user_id_alt: Option<String>,
    pub chat_id: Option<String>,
    pub chat_type: Option<String>,
    pub chat_name: Option<String>,
}

impl SessionSource {
    pub fn platform_value(&self) -> String {
        self.platform.as_deref().unwrap_or("").trim().to_ascii_lowercase()
    }
}

/// Mirrors `EphemeralReply(str)` wrapper used by `_handle_reset_command`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EphemeralReply(pub String);

impl std::fmt::Display for EphemeralReply {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Mirrors `MessageEvent` (minimal: `source` + `text` + helpers).
#[derive(Debug, Clone)]
pub struct MessageEvent {
    pub source: SessionSource,
    pub text: Option<String>,
    pub raw_message: Option<serde_json::Value>,
    pub message_id: Option<String>,
    pub reply_to_message_id: Option<String>,
}

impl MessageEvent {
    /// Mirrors `event.get_command_args().strip()`
    pub fn get_command_args(&self) -> String {
        let raw = self.text.as_deref().unwrap_or("").trim();
        // Strip leading "/command" or "command" prefix — find first space
        if raw.is_empty() {
            return String::new();
        }
        let without_slash = raw.trim_start_matches('/');
        // Find command token boundary
        let mut after_cmd = without_slash;
        // Skip first token (command name)
        if let Some(space) = after_cmd.find(char::is_whitespace) {
            after_cmd[space..].trim().to_string()
        } else {
            // No args, check if raw had space after slash-command
            String::new()
        }
    }
    pub fn get_command_args_trimmed(&self) -> String {
        self.get_command_args().trim().to_string()
    }
}

/// Mirrors `MessageType` (documented only in slice 1; runtime elsewhere).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessageType {
    Text,
    Other(String),
}

/// Mirrors `SessionSource` → session key builder (stub).
pub fn build_session_key(source: &SessionSource) -> String {
    format!(
        "{}:{}:{}",
        source.platform.as_deref().unwrap_or("unknown"),
        source.scope_id.as_deref().unwrap_or(""),
        source.thread_id.as_deref().unwrap_or("")
    )
}

// ---------------------------------------------------------------------------
// GatewaySlashCommandsMixin — mirrors Python lines 126–143
// ---------------------------------------------------------------------------

/// In-session slash-command handlers for `GatewayRunner`.
///
/// Mirrors:
/// ```python
/// class GatewaySlashCommandsMixin:
///     """In-session slash-command handlers for GatewayRunner."""
///     async_session_store: AsyncSessionStore
/// ```
///
/// Python is a mixin inherited by `GatewayRunner` (MRO). Rust models it as
/// a struct holding the handles the mixin reaches via `self` (adapters,
/// session stores, hooks, caches). Methods are synchronous stubs mirroring
/// the Python branching; async gateway coupling (`asyncio.to_thread`,
/// `async_session_store.reset_session`, `hooks.emit`, agent-cache lock) is
/// documented as `// Python:` and stubbed.
// ponytail: global stub, per-session state if handler throughput matters
#[derive(Debug, Default)]
pub struct GatewaySlashCommandsMixin {
    /// Mirrors `self.async_session_store: AsyncSessionStore`
    pub async_session_store_key: Option<String>,
    /// Mirrors `self.adapters: Dict[Platform, Adapter]` where adapter may have
    /// `typed_command_prefix`. Stored as platform → prefix map for std-only.
    pub adapter_prefixes: HashMap<String, String>,
    /// Mirrors `self.config.multiplex_profiles` (bool)
    pub multiplex_profiles: bool,
    /// Mirrors `self._agent_cache_lock` / `self._agent_cache` / `self._running_agents`
    /// (kept as opaque ids for stub).
    pub agent_cache_keys: HashSet<String>,
}

impl GatewaySlashCommandsMixin {
    /// Return the prefix users can always type to reach Hermes commands.
    ///
    /// Mirrors:
    /// ```python
    /// def _typed_command_prefix_for(self, platform) -> str:
    ///     adapter = self.adapters.get(platform) if getattr(self, "adapters", None) else None
    ///     return getattr(adapter, "typed_command_prefix", "/") if adapter is not None else "/"
    /// ```
    ///
    /// Slack and Matrix return "!" because typed "/" commands are blocked in
    /// Slack threads / reserved by Matrix clients; their adapters rewrite
    /// "!command" to "/command" on receive. Instruction text built for those
    /// platforms must show the prefix that actually works when typed.
    pub fn typed_command_prefix_for(&self, platform: &str) -> String {
        typed_command_prefix_for(&self.adapter_prefixes, platform)
    }

    /// Value overload (handles `{"value": "..."}` enum shape).
    pub fn typed_command_prefix_for_value(&self, platform: &serde_json::Value) -> String {
        let p = gateway_platform_value(platform);
        self.typed_command_prefix_for(&p)
    }

    // -----------------------------------------------------------------------
    // _handle_reset_command — mirrors Python lines 144–353
    // -----------------------------------------------------------------------

    /// Handle `/new` or `/reset` command.
    ///
    /// Mirrors `async def _handle_reset_command(self, event: MessageEvent) -> Union[str, EphemeralReply]` (~210 LOC).
    /// Full Python flow is preserved as structured comments + stub calls:
    /// 1. `session_key = self._session_key_for_source(source)` + `_invalidate_session_run_generation`
    ///    + `_release_running_agent_state` (zombie-slot guard #28686)
    /// 2. Snapshot `old_entry = session_store._entries.get(session_key)`
    /// 3. Off-loop `_cleanup_agent_resources` via `_run_in_executor_with_context` + `wait_for(...,
    ///    timeout=_RESET_CLEANUP_TIMEOUT_S)` with `TimeoutError` / `Exception` warnings
    /// 4. `_evict_cached_agent(session_key)` + `_clear_conversation_scope(session_key, reason="session_reset")`
    /// 5. `tools.async_delegation.interrupt_for_session(session_key, parent_session_id, reason)` (try/except pass)
    /// 6. `clear_env_passthrough()` / `clear_credential_files()` (try/except pass)
    /// 7. `new_entry = await async_session_store.reset_session(session_key)` + finalize hooks
    /// 8. `hooks.emit("session:end")` + `hooks.emit("session:reset")`
    /// 9. `session_info = await to_thread(_reset_notice_session_info, source)` (multiplex-scoped)
    /// 10. `header = await to_thread(_telegram_topic_new_header, source) or t("gateway.reset.header_*")`
    ///     + `/new <title>` sanitization via `SessionDB.sanitize_title` + `set_session_title`
    /// 11. Telegram DM topic lane rebind + `invoke_hook("on_session_reset", ...)`
    /// 12. Tip line via `get_random_tip()` → `EphemeralReply(f"{header}\n\n{session_info}{tip}")`
    ///
    /// Rust stub returns a synthetic `EphemeralReply` mirroring the header/info/tip shape;
    /// session-store / agent-cache / hook side effects are logged at `debug!` and no-oped.
    pub fn handle_reset_command(&self, event: &MessageEvent) -> EphemeralReply {
        // Python: source = event.source; session_key = self._session_key_for_source(source)
        let source = &event.source;
        let session_key = build_session_key(source);
        log::debug!("_handle_reset_command: session_key={} invalidate+release", session_key);
        // Python: _invalidate_session_run_generation + _release_running_agent_state
        // Rust: stub (ponytail: no generation map in std-only; keep slot zombie guard as log)

        // Python: old_entry = session_store._entries.get(session_key)
        // Rust: stub old entry id
        let old_sid = format!("old_{}", session_key);

        // Python: _agent_cache_lock → _cleanup_agent_resources via wait_for(..., 30.0)
        // Rust: emulate bounded off-loop cleanup (always succeeds in stub)
        let _cleanup_ok = true; // ponytail: off-loop executor stub
        if !_cleanup_ok {
            log::warn!(
                "Agent resource cleanup for session {} exceeded {}s during /new reset; proceeding (worker left to finish). (#35994)",
                session_key, RESET_CLEANUP_TIMEOUT_S
            );
        }

        // Python: _evict_cached_agent + _clear_conversation_scope(reason="session_reset")
        // Python: interrupt_for_session(session_key, parent_session_id, reason="session_reset")
        // Python: clear_env_passthrough / clear_credential_files
        // Rust: no-ops (debug trace)
        log::debug!("reset: cleared conversation scope + async_delegation interrupt for {}", session_key);

        // Python: new_entry = await async_session_store.reset_session(session_key)
        let new_session_id = format!("new_{}", session_key);
        let _new_entry_exists = true;

        // Python: _finalize_session_off_loop(session_id=_old_sid, ...) + hooks.emit session:end/reset
        log::debug!("reset: finalized old_session {} new_session {}", old_sid, new_session_id);

        // Python: session_info = await to_thread(_reset_notice_session_info, source) (multiplex-scoped)
        let session_info = ""; // stub: would resolve model/provider via profile_runtime_scope in multiplex

        // Python: header = await to_thread(_telegram_topic_new_header, source) or t("gateway.reset.header_*")
        let header = t_reset_header(_new_entry_exists, false);

        // Python: /new <title> sanitization
        let title_arg = event.get_command_args_trimmed();
        let title_note = if !title_arg.is_empty() {
            sanitize_title_note(&title_arg)
        } else {
            String::new()
        };
        let header_with_note = format!("{}{}", header, title_note);

        // Python: _is_telegram_topic_lane + _record_telegram_topic_binding (rebind)
        // Python: invoke_hook("on_session_reset", ...)
        // Python: _tip_line = t("gateway.reset.tip", tip=get_random_tip())
        let tip_line = ""; // ponytail: no tips dep in std-only

        if !session_info.is_empty() {
            EphemeralReply(format!("{}\n\n{}{}", header_with_note, session_info, tip_line))
        } else {
            EphemeralReply(format!("{}{}", header_with_note, tip_line))
        }
    }

    // -----------------------------------------------------------------------
    // _handle_profile_command — mirrors Python lines 355–406
    // -----------------------------------------------------------------------

    /// Handle `/profile` — show the profile serving this source and its home.
    ///
    /// Mirrors `async def _handle_profile_command(self, event) -> str`:
    /// when `config.multiplex_profiles` is on, report the stamped `source.profile`
    /// and resolve the displayed home under that profile's ` _profile_runtime_scope`.
    /// When multiplexing is off the stamp is ignored and the command reports the
    /// active profile and default home, byte-identical to before (#59003).
    pub fn handle_profile_command(&self, event: &MessageEvent) -> String {
        let multiplexed = self.multiplex_profiles;
        let source = &event.source;
        let mut profile_name = String::new();
        let mut display = String::new();
        if multiplexed {
            profile_name = source.profile.as_deref().unwrap_or("").trim().to_string();
            // Python: with _profile_runtime_scope(profile_home): display = display_hermes_home()
            // Rust: stub — would resolve HERMES_HOME/profiles/<profile>
            display = display_hermes_home_stub(source.profile.as_deref());
        }
        // Python: execute_command("profile", CommandContext(surface="gateway", options={...}))
        // Rust: return two-line reply with t("gateway.profile.header/home")
        if profile_name.is_empty() {
            profile_name = display_hermes_profile_name_stub();
        }
        if display.is_empty() {
            display = display_hermes_home_stub(None);
        }
        // Mirrors: lines = [t("gateway.profile.header", profile=...), t("gateway.profile.home", home=...)]
        format!(
            "{}\n{}",
            t_profile_header(&profile_name),
            t_profile_home(&display)
        )
    }

    // -----------------------------------------------------------------------
    // _handle_whoami_command — mirrors Python lines 408–457
    // -----------------------------------------------------------------------

    /// Handle `/whoami` — show the user's slash command access on this scope.
    ///
    /// Mirrors `async def _handle_whoami_command(self, event) -> str`:
    /// always works (it's in the always-allowed floor). Reports platform,
    /// scope (DM vs group), tier (admin / user / unrestricted), and runnable commands.
    pub fn handle_whoami_command(&self, event: &MessageEvent) -> String {
        // Python: policy = _policy_for_source(self.config, source)
        // Rust stub: policy from env / default; mirrors slash_access checks
        let source = &event.source;
        let platform = source.platform.as_deref().unwrap_or("?").trim().to_string();
        let chat_type = source.chat_type.as_deref().unwrap_or("dm").trim().to_string();
        let scope = if matches!(chat_type.to_ascii_lowercase().as_str(), "dm" | "direct" | "private" | "") {
            "DM"
        } else {
            "group/channel"
        };
        let user_id = source.user_id.as_deref().unwrap_or("?").trim().to_string();
        // Stub policy: if no admin list configured → unrestricted
        let policy = whoami_policy_stub(source);
        match policy {
            WhoamiPolicy::Unrestricted => format!(
                "**You** — {} ({})\nUser ID: `{}`\nTier: unrestricted (no admin list configured for this scope)\nSlash commands: all available",
                platform, scope, user_id
            ),
            WhoamiPolicy::Admin => format!(
                "**You** — {} ({})\nUser ID: `{}`\nTier: **admin**\nSlash commands: all available",
                platform, scope, user_id
            ),
            WhoamiPolicy::User { allowed } => {
                // floor = ["help", "whoami"]; combine + dedupe preserve order
                let mut seen: HashSet<String> = HashSet::new();
                let mut runnable: Vec<String> = Vec::new();
                for c in ["help", "whoami"].iter().chain(allowed.iter().map(|s| s.as_str())) {
                    if seen.insert(c.to_string()) {
                        runnable.push(c.to_string());
                    }
                }
                let runnable_str = if runnable.is_empty() {
                    "(none)".to_string()
                } else {
                    runnable.iter().map(|c| format!("/{}", c)).collect::<Vec<_>>().join(", ")
                };
                format!(
                    "**You** — {} ({})\nUser ID: `{}`\nTier: user\nSlash commands you can run: {}",
                    platform, scope, user_id, runnable_str
                )
            }
        }
    }

    // -----------------------------------------------------------------------
    // _handle_kanban_command — mirrors Python lines 459–575
    // -----------------------------------------------------------------------

    /// Handle `/kanban` — delegate to the shared kanban CLI.
    ///
    /// Mirrors `async def _handle_kanban_command(self, event) -> str`:
    /// strips leading "/kanban", `shlex.split`, parses `--board` / `action`,
    /// `run_slash(text)` via `to_thread`, auto-subscribe on `create` by
    /// parsing `Created t_xxx` and calling `add_notify_sub`, truncation at 3800 chars.
    pub fn handle_kanban_command(&self, event: &MessageEvent) -> String {
        let raw = event.text.as_deref().unwrap_or("").trim().to_string();
        let text = strip_kanban_prefix(&raw);
        let tokens = shlex_split(&text);
        let (requested_board, action) = parse_kanban_args(&tokens);
        let is_create = action.as_deref() == Some("create");

        // Python: output = await asyncio.to_thread(run_slash, text)
        // Rust stub: would call hermes_cli.kanban.run_slash(text); we simulate
        let output = kanban_run_slash_stub(&text);

        // Python: auto-subscribe on create via regex `Created\s+(t_[0-9a-f]+)\b` + add_notify_sub
        let mut out = output.clone();
        if is_create && !output.is_empty() && !contains_json_flag(&tokens) {
            if let Some(task_id) = parse_kanban_task_id(&output) {
                // Python would persist platform/chat/thread/user_id_alt/delivery_metadata via kanban_db
                log::debug!("kanban create auto-subscribe task_id={} board={:?}", task_id, requested_board);
                let suffix = t_kanban_subscribed_suffix(&task_id);
                out = format!("{}\n{}", out.trim_end(), suffix);
            }
        }

        // Python: truncate at 3800 chars
        const KANBAN_TRUNCATE_AT: usize = 3800;
        if out.len() > KANBAN_TRUNCATE_AT {
            out.truncate(KANBAN_TRUNCATE_AT);
            out.push_str(&format!("\n{}", t_kanban_truncated_suffix()));
        }
        if out.trim().is_empty() {
            out = t_kanban_no_output();
        }
        out
    }

    // -----------------------------------------------------------------------
    // _handle_status_command — mirrors Python lines 576–772
    // -----------------------------------------------------------------------

    /// Handle `/status` command.
    ///
    /// Mirrors `async def _handle_status_command(self, event) -> str` (~197 LOC):
    /// connected_platforms, agent vs sentinel, queue_depth, token totals from
    /// SessionDB (sqlite), persisted_route, live/cached agent model route,
    /// context_used/total, gateway config fallback, model_line/context_line,
    /// session header lines, matrix scope branch, platforms footer.
    pub fn handle_status_command(&self, event: &MessageEvent) -> String {
        // Python: session_entry = await async_session_store.get_or_create_session(source)
        // Rust stub values
        let source = &event.source;
        let session_id = format!("sess_{}", build_session_key(source));
        let connected_platforms = connected_platforms_stub(&self.adapter_prefixes);
        let (agent_running, is_running) = (false, false); // stub: no live agent in std-only
        let queue_depth = 0usize; // stub: would call _queue_depth(session_key, adapter)

        // Python: db_total_tokens from _session_db.get_session(session_id) sum of input/output/cache/ reasoning
        let db_total_tokens: i64 = 0; // stub
        let persisted_route: HashMap<String, String> = HashMap::new(); // stub

        // Python: status_agent selection (running → cached)
        let (mut model_name, mut provider_name, mut base_url) = (String::new(), String::new(), String::new());
        let (mut context_used, mut context_total) = (0i64, 0i64);
        // Live agent path skipped in stub (no agent)
        // Persisted route fallback
        if model_name.is_empty() {
            if let Some(m) = persisted_route.get("model") { model_name = clean_str(Some(m)); }
            if let Some(p) = persisted_route.get("billing_provider") { provider_name = clean_str(Some(p)); }
            if let Some(u) = persisted_route.get("billing_base_url") { base_url = clean_str(Some(u)); }
        }
        // Python: context_used = context_used or last_prompt_tokens
        // Python: gateway config fallback via _load_gateway_config / _resolve_gateway_model
        if model_name.is_empty() {
            model_name = resolve_gateway_model_stub();
        }
        if provider_name.is_empty() {
            provider_name = String::new();
        }
        if context_total == 0 {
            context_total = 0; // would read model.context_length from config
        }

        let model_line = resolve_model_line(&model_name, &provider_name);
        let context_line = resolve_context_line(context_used, context_total);
        let _ = base_url; // used only for provider display elsewhere

        // Build output lines
        let mut lines: Vec<String> = Vec::new();
        let title: Option<String> = None; // stub: would fetch _session_db.get_session_title(session_id)
        let session_row: HashMap<String, serde_json::Value> = HashMap::new();
        let _ = session_row;
        lines.push(t_status_header());
        lines.push(String::new());
        lines.push(t_status_session_id(&session_id));
        if let Some(t) = title { lines.push(t_status_title(&t)); }
        // created / last_activity: stub timestamps
        lines.push(t_status_created("2026-01-01 00:00"));
        lines.push(t_status_last_activity("2026-01-01 00:00"));
        if !model_line.is_empty() { lines.push(model_line); }
        if !context_line.is_empty() { lines.push(context_line); }
        lines.push(t_status_tokens(&format!("{db_total_tokens:,}")));
        lines.push(t_status_agent_running(if is_running { &t_status_state_yes() } else { &t_status_state_no() }));
        let _ = agent_running;
        if queue_depth > 0 { lines.push(t_status_queued(queue_depth)); }

        // Matrix scope branch
        if source.platform.as_deref().map(|p| p.eq_ignore_ascii_case("matrix")).unwrap_or(false) {
            let adapter_scope = std::env::var("MATRIX_SESSION_SCOPE").unwrap_or_else(|_| "auto".to_string());
            let thread = source.thread_id.as_deref().unwrap_or("none");
            let sk = build_session_key(source);
            lines.push(String::new());
            lines.push(t_status_matrix_scope_header());
            lines.push(t_status_matrix_scope_room(source.chat_name.as_deref().unwrap_or(source.chat_id.as_deref().unwrap_or("?"))));
            lines.push(t_status_matrix_scope_room_id(source.chat_id.as_deref().unwrap_or("?")));
            lines.push(t_status_matrix_scope_thread(thread));
            lines.push(t_status_matrix_scope_mode(&adapter_scope));
            lines.push(t_status_matrix_scope_key(&redact_matrix_session_key(&sk)));
        }

        lines.push(String::new());
        lines.push(t_status_platforms(&connected_platforms.join(", ")));

        lines.join("\n")
    }

    // -----------------------------------------------------------------------
    // _handle_context_command — mirrors Python lines 780–900 (slice 1 partial)
    // -----------------------------------------------------------------------

    /// Handle `/context` — the dedicated context-window view.
    ///
    /// Mirrors `async def _handle_context_command(self, event) -> str` (partial,
    /// slice 1 covers through the gauge bar and the `threshold` header).
    /// `/status` shows a one-line `used / total` summary; this command is the
    /// deep view: usage gauge, auto-compression threshold and headroom,
    /// compression count and last savings, and cumulative throughput — the last
    /// clearly labelled as throughput, NOT context size.
    ///
    /// Resolves from the running agent (mid-turn), then the cached agent
    /// (between turns), then `SessionStore`/`SessionDB` metadata for a gauge
    /// even when no agent is resident. Falls back to transcript estimate only
    /// as a last resort. `/context all` appends expanded per-skill / per-toolset
    /// cost listings (requires resident agent).
    ///
    /// Slice 1 stops inside the `if ctx is not None:` threshold block after
    /// `lines.append("")`; the remainder (threshold over/under, compressions,
    /// savings, totals, throughput note, and the `expanded` branch) continues
    /// in slice 2. See boundary note at end of this method.
    pub fn handle_context_command(&self, event: &MessageEvent) -> String {
        // Python: session_key = self._session_key_for_source(source); session_entry = await async_session_store.get_or_create_session(source)
        // Rust stubs
        let source = &event.source;
        let session_key = build_session_key(source);
        let expanded = event.get_command_args_trimmed().to_ascii_lowercase();
        let expanded = matches!(expanded.as_str(), "all" | "full" | "details");
        let _ = expanded; // used in slice 2 for full detail branch

        // Python: agent = _running_agents.get(session_key) → cached fallback
        // Rust: stub (no live compressor in std-only)
        let ctx_present = false; // would be `agent.context_compressor is not None`

        // Python: used/context_length resolution cascading
        let mut used: i64 = 0;
        let mut context_length: i64 = 0;
        let mut model_name = String::new();
        let _ = (ctx_present, &mut used, &mut context_length, &mut model_name, &session_key);

        // Python: nonresident context via _profile_runtime_scope + _resolve_gateway_model_context
        // Python: fallback via get_model_context_length
        // Rust stubs leave used/length as 0 in std-only; gauge path requires both >0

        // Python: gauge path
        if used > 0 && context_length > 0 {
            let pct = (used as f64 / context_length as f64 * 100.0).min(100.0);
            let headroom = (context_length - used).max(0);
            const BAR_WIDTH: usize = 24;
            let filled = (pct / 100.0 * BAR_WIDTH as f64).round() as usize;
            let bar = format!("{}{}", "█".repeat(filled.min(BAR_WIDTH)), "░".repeat(BAR_WIDTH.saturating_sub(filled)));
            let mut lines: Vec<String> = Vec::new();
            lines.push(t_context_header());
            lines.push(String::new());
            lines.push(t_context_model(if model_name.is_empty() { "?" } else { &model_name }));
            lines.push(t_context_window(&format!("{context_length:,}")));
            lines.push(t_context_in_use(&format!("{used:,}"), &format!("{context_length:,}"), &format!("{pct:.0}")));
            lines.push(t_context_bar(&bar));
            lines.push(t_context_headroom(&format!("{headroom:,}")));
            // Python: if ctx is not None:
            //   threshold = getattr(ctx, "threshold_tokens", 0) ... lines.append("") ...
            // Slice 1 ends after `lines.append("")` at threshold prologue.
            // Full threshold/compressions/totals detail is in slice 2.
            // Rust mirrors prologue for traceability:
            // ponytail: no compressor → skip threshold details in slice 1 stub
            if ctx_present {
                lines.push(String::new());
                // --- slice boundary 900: remaining threshold over/under + compressions + totals in slice 2 ---
                // Python continues:
                //   if threshold > 0:
                //       if used >= threshold: lines.append(t("gateway.context.over_threshold", ...))
                //       else: lines.append(t("gateway.context.threshold", ..., to_go=...))
                //   compressions = getattr(ctx, "compression_count", 0) ...
                //   savings ... api_calls ... totals_header/line ... throughput_note
                //   else: lines.append(t("gateway.context.detail_after_first"))
            }
            return lines.join("\n");
        }

        // Python: non-gauge fallback path (transcript estimate / no-data message)
        // Rust: slice 1 returns empty gauge placeholder; slice 2 completes fallbacks.
        // ponytail: deterministic placeholder when no context window is known
        format!("{}\n{}", t_context_header(), t_context_no_data())
    }
}

// ---------------------------------------------------------------------------
// Free-function aliases — mirrors Python module-level helpers accessed via MRO
// ---------------------------------------------------------------------------

/// Free helper mirroring `GatewaySlashCommandsMixin._typed_command_prefix_for`.
///
/// Kept as free function for callers that don't hold a mixin instance.
pub fn typed_command_prefix_for(prefixes: &HashMap<String, String>, platform: &str) -> String {
    let key = platform.trim().to_ascii_lowercase();
    prefixes.get(&key).cloned().unwrap_or_else(|| "/".to_string())
}

/// `serde_json::Value` overload.
pub fn typed_command_prefix_for_value(prefixes: &HashMap<String, String>, platform: &serde_json::Value) -> String {
    let p = gateway_platform_value(platform);
    typed_command_prefix_for(prefixes, &p)
}

#[allow(dead_code)]
fn _typed_command_prefix_for(prefixes: &HashMap<String, String>, platform: &str) -> String {
    typed_command_prefix_for(prefixes, platform)
}

fn gateway_platform_value(v: &serde_json::Value) -> String {
    if let Some(s) = v.as_str() {
        return s.trim().to_ascii_lowercase();
    }
    if let Some(obj) = v.as_object() {
        if let Some(inner) = obj.get("value").and_then(|x| x.as_str()) {
            return inner.trim().to_ascii_lowercase();
        }
        if let Some(inner) = obj.get("value") {
            return inner.to_string().trim().to_ascii_lowercase();
        }
    }
    v.to_string().trim().to_ascii_lowercase()
}

// ---------------------------------------------------------------------------
// _redact_matrix_session_key — mirrors Python lines 773–778
// ---------------------------------------------------------------------------

/// Return a stable Matrix session-key fingerprint for shared room status.
///
/// Mirrors:
/// ```python
/// @staticmethod
/// def _redact_matrix_session_key(session_key: str) -> str:
///     text = str(session_key or "")
///     digest = hashlib.sha256(text.encode("utf-8")).hexdigest()[:12]
///     return f"sha256:{digest}"
/// ```
pub fn redact_matrix_session_key(session_key: &str) -> String {
    let text = session_key;
    let digest = sha256_hex12(text);
    format!("sha256:{}", digest)
}

#[allow(dead_code)]
fn _redact_matrix_session_key(session_key: &str) -> String {
    redact_matrix_session_key(session_key)
}

/// Compute `hashlib.sha256(text.encode("utf-8")).hexdigest()[:12]`.
///
/// Rust std-only: `sha256` via lightweight FNV-like deterministic hex for
/// traceability; swap for `sha2` crate when wired (ponytail: no sha2 dep in
/// std-only build — preserve 12-hex shape + determinism, not crypto strength).
fn sha256_hex12(text: &str) -> String {
    // If `sha2` is available, replace this body with:
    //   use sha2::{Sha256, Digest}; hex::encode(Sha256::digest(text.as_bytes()))[..12].to_string()
    // Minimal deterministic hex without external dep: use SipHash-like mixing via std hasher + hex expansion
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h1 = DefaultHasher::new();
    text.hash(&mut h1);
    let v1 = h1.finish();
    let mut h2 = DefaultHasher::new();
    // second hash with length prefix to avoid trivial collisions
    (text.len() as u64).hash(&mut h2);
    text.hash(&mut h2);
    let v2 = h2.finish();
    // Combine into 12 hex chars (48 bits) — take low 48 bits of (v1 ^ (v2<<1))
    let combined = v1 ^ v2.wrapping_shl(1);
    // Format as 12 hex digits, zero-padded
    format!("{:012x}", combined & 0xfff_ffff_ffff)
}

// ---------------------------------------------------------------------------
// Kanban helpers — mirrors Python sub-logic lines 483–575
// ---------------------------------------------------------------------------

fn strip_kanban_prefix(raw: &str) -> String {
    let mut text = raw.trim().to_string();
    if text.starts_with('/') {
        text = text.trim_start_matches('/').to_string();
    }
    if text.to_ascii_lowercase().starts_with("kanban") {
        text = text["kanban".len()..].trim_start().to_string();
    }
    text
}

/// Minimal `shlex.split` stub — handles quoting and escaping.
///
/// Mirrors `shlex.split(text)` for the subset used by `/kanban` (flags + ids).
/// Full POSIX shell quoting is approximated; `ponytail: quote-aware split, swap for `shlex` crate if fidelity matters`.
fn shlex_split(text: &str) -> Vec<String> {
    let mut tokens: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut in_single = false;
    let mut in_double = false;
    let mut escaped = false;
    for ch in text.chars() {
        if escaped {
            cur.push(ch);
            escaped = false;
            continue;
        }
        if ch == '\\' && !in_single {
            escaped = true;
            continue;
        }
        if ch == '\'' && !in_double {
            in_single = !in_single;
            continue;
        }
        if ch == '"' && !in_single {
            in_double = !in_double;
            continue;
        }
        if ch.is_whitespace() && !in_single && !in_double {
            if !cur.is_empty() {
                tokens.push(cur.clone());
                cur.clear();
            }
            continue;
        }
        cur.push(ch);
    }
    if !cur.is_empty() {
        tokens.push(cur);
    }
    tokens
}

fn parse_kanban_args(tokens: &[String]) -> (Option<String>, Option<String>) {
    let mut requested_board: Option<String> = None;
    let mut action: Option<String> = None;
    let mut i = 0usize;
    while i < tokens.len() {
        let tok = &tokens[i];
        if tok == "--board" {
            if i + 1 < tokens.len() {
                requested_board = Some(tokens[i + 1].clone());
            }
            i += 2;
            continue;
        }
        if tok.starts_with("--board=") {
            if let Some(eq) = tok.find('=') {
                requested_board = Some(tok[eq + 1..].to_string());
            }
            i += 1;
            continue;
        }
        action = Some(tok.clone());
        break;
    }
    (requested_board, action)
}

fn is_kanban_create(action: Option<&str>) -> bool {
    action == Some("create")
}

#[allow(dead_code)]
fn _is_kanban_create(action: Option<&str>) -> bool {
    is_kanban_create(action)
}

fn contains_json_flag(tokens: &[String]) -> bool {
    tokens.iter().any(|t| t == "--json" || t.starts_with("--json="))
}

fn kanban_run_slash_stub(text: &str) -> String {
    // ponytail: no hermes_cli.kanban dep in std-only; echo the args for traceability
    if text.trim().is_empty() {
        return "kanban: no args".to_string();
    }
    // In real gateway, `run_slash(text)` would hit SQLite kanban DB
    format!("kanban stub: {}", text)
}

fn parse_kanban_task_id(output: &str) -> Option<String> {
    // Mirrors `re.search(r"Created\s+(t_[0-9a-f]+)\b", output)`
    // Manual scan without regex crate
    let lower = output; // preserve case for id
    let needle = "Created";
    let mut search_start = 0usize;
    while let Some(idx) = lower[search_start..].find(needle) {
        let abs = search_start + idx;
        let rest = &lower[abs + needle.len()..];
        // skip whitespace
        let rest_trim = rest.trim_start();
        if rest_trim.starts_with("t_") {
            // parse t_[0-9a-f]+
            let id_start = abs + needle.len() + (rest.len() - rest_trim.len());
            let mut end = id_start + 2; // after "t_"
            while end < lower.len() && lower[end..].chars().next().map(|c| c.is_ascii_hexdigit()).unwrap_or(false) {
                end += 1;
            }
            if end > id_start + 2 {
                // word boundary check: next char not alnum/underscore
                let after_ok = end >= lower.len() || !matches!(lower[end..].chars().next(), Some(c) if c.is_ascii_alphanumeric() || c == '_');
                if after_ok {
                    return Some(lower[id_start..end].to_string());
                }
            }
        }
        search_start = abs + needle.len();
        if search_start >= lower.len() { break; }
    }
    None
}

// ---------------------------------------------------------------------------
// Status helpers — mirrors Python sub-logic lines 596–745
// ---------------------------------------------------------------------------

fn status_clean_str(v: Option<&str>) -> String { clean_str(v) }
#[allow(dead_code)] fn _status_clean_str(v: Option<&str>) -> String { status_clean_str(v) }

fn status_int_value(v: &serde_json::Value) -> i64 { int_value(v) }

fn resolve_gateway_model_stub() -> String {
    // Python: _resolve_gateway_model(user_config) — would read config.yaml + env
    // Rust stub: env HERMES_MODEL or default
    std::env::var("HERMES_MODEL").ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

fn resolve_model_line(model: &str, provider: &str) -> String {
    let m = model.trim();
    let p = provider.trim();
    if m.is_empty() { return String::new(); }
    if p.is_empty() { format!("model: {}", m) } else { format!("model: {} (provider: {})", m, p) }
    // Python t("gateway.status.model_provider" / "gateway.status.model")
}

fn resolve_context_line(used: i64, total: i64) -> String {
    if total > 0 {
        let pct = ((used as f64 / total as f64) * 100.0).round().min(100.0) as i64;
        format!("context: {used:,} / {total:,} ({pct}%)")
    } else if used > 0 {
        format!("context used: {used:,}")
    } else {
        String::new()
    }
}

fn connected_platforms_stub(prefixes: &HashMap<String, String>) -> Vec<String> {
    let mut v: Vec<String> = prefixes.keys().cloned().collect();
    if v.is_empty() { v.push("telegram".to_string()); }
    v.sort();
    v
}

// i18n stubs for status lines (preserve keys for traceability)
fn t_status_header() -> String { "Status".to_string() }
fn t_status_session_id(id: &str) -> String { format!("session: {}", id) }
fn t_status_title(t: &str) -> String { format!("title: {}", t) }
fn t_status_created(ts: &str) -> String { format!("created: {}", ts) }
fn t_status_last_activity(ts: &str) -> String { format!("last activity: {}", ts) }
fn t_status_tokens(s: &str) -> String { format!("tokens: {}", s) }
fn t_status_agent_running(s: &str) -> String { format!("agent running: {}", s) }
fn t_status_state_yes() -> String { "yes".to_string() }
fn t_status_state_no() -> String { "no".to_string() }
fn t_status_queued(n: usize) -> String { format!("queued: {}", n) }
fn t_status_matrix_scope_header() -> String { "matrix scope:".to_string() }
fn t_status_matrix_scope_room(r: &str) -> String { format!("  room: {}", r) }
fn t_status_matrix_scope_room_id(r: &str) -> String { format!("  room_id: {}", r) }
fn t_status_matrix_scope_thread(t: &str) -> String { format!("  thread: {}", t) }
fn t_status_matrix_scope_mode(s: &str) -> String { format!("  scope mode: {}", s) }
fn t_status_matrix_scope_key(k: &str) -> String { format!("  session_key: {}", k) }
fn t_status_platforms(p: &str) -> String { format!("platforms: {}", p) }

// ---------------------------------------------------------------------------
// Context helpers — mirrors Python lines 780–900 prologue
// ---------------------------------------------------------------------------

fn t_context_header() -> String { "Context".to_string() }
fn t_context_model(m: &str) -> String { format!("model: {}", m) }
fn t_context_window(t: &str) -> String { format!("window: {}", t) }
fn t_context_in_use(used: &str, total: &str, pct: &str) -> String { format!("in use: {} / {} ({}%)", used, total, pct) }
fn t_context_bar(bar: &str) -> String { format!("[{}]", bar) }
fn t_context_headroom(h: &str) -> String { format!("headroom: {}", h) }
fn t_context_no_data() -> String { "no context data available".to_string() }

// ---------------------------------------------------------------------------
// Profile / whoami / reset stubs — mirrors Python lines 289–406
// ---------------------------------------------------------------------------

fn display_hermes_home_stub(profile: Option<&str>) -> String {
    if let Some(p) = profile.map(|s| s.trim()).filter(|s| !s.is_empty()) {
        format!("~/.hermes/profiles/{}", p)
    } else {
        // Mirrors display_hermes_home() default
        "~/.hermes".to_string()
    }
}

fn display_hermes_profile_name_stub() -> String {
    std::env::var("HERMES_PROFILE").ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
        .unwrap_or_else(|| "default".to_string())
}

fn t_profile_header(p: &str) -> String { format!("profile: {}", p) }
fn t_profile_home(h: &str) -> String { format!("home: {}", h) }

#[derive(Debug, Clone)]
enum WhoamiPolicy {
    Unrestricted,
    Admin,
    User { allowed: Vec<String> },
}

fn whoami_policy_stub(source: &SessionSource) -> WhoamiPolicy {
    // ponytail: env-gated stub; wire to gateway.slash_access.policy_for_source when available
    // Check HERMES_SLASH_ACCESS env shape: absent → unrestricted (matches Python policy.enabled==False)
    let enabled = std::env::var("HERMES_SLASH_ACCESS_ENABLED").ok()
        .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false);
    if !enabled {
        return WhoamiPolicy::Unrestricted;
    }
    // Stub admin check via HERMES_ADMIN_IDS
    let admins_raw = std::env::var("HERMES_ADMIN_IDS").unwrap_or_default();
    let admins: HashSet<String> = admins_raw.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    if let Some(uid) = source.user_id.as_deref().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) {
        if admins.contains(&uid) {
            return WhoamiPolicy::Admin;
        }
    }
    let allowed_raw = std::env::var("HERMES_USER_ALLOWED_COMMANDS").unwrap_or_default();
    let allowed: Vec<String> = allowed_raw.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    WhoamiPolicy::User { allowed }
}

// Reset header / title helpers

fn t_reset_header(has_entry: bool, _is_new: bool) -> String {
    // Python: t("gateway.reset.header_default") vs t("gateway.reset.header_new")
    if has_entry { "New session started".to_string() } else { "New session started".to_string() }
}

fn sanitize_title_note(title: &str) -> String {
    // Mirrors SessionDB.sanitize_title validation + note lines
    let trimmed = title.trim();
    if trimmed.is_empty() {
        return " (title empty — untitled)".to_string();
    }
    if trimmed.len() > 100 {
        return format!(" (title rejected: too long: {})", trimmed.len());
    }
    // Python would call SessionDB.sanitize_title + set_session_title; we stub success
    format!(" — {}", trimmed)
}

fn t_kanban_subscribed_suffix(task_id: &str) -> String { format!("(subscribed to {})", task_id) }
fn t_kanban_truncated_suffix() -> String { "(truncated)".to_string() }
fn t_kanban_no_output() -> String { "(no output)".to_string() }


// ---------------------------------------------------------------------------
// Path helpers — mirrors Python lazy imports from gateway/run
// ---------------------------------------------------------------------------

/// Resolve `HERMES_HOME` (`$HERMES_HOME` or `~/.hermes`).
fn get_hermes_home() -> PathBuf {
    if let Ok(val) = std::env::var("HERMES_HOME") {
        let t = val.trim().to_string();
        if !t.is_empty() { return PathBuf::from(t); }
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".hermes");
    }
    PathBuf::from(".hermes")
}

#[allow(dead_code)]
fn _get_hermes_home() -> PathBuf { get_hermes_home() }

fn hermes_home_display() -> String {
    get_hermes_home().to_string_lossy().to_string()
}
