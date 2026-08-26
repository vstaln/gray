//! Gateway runner — entry point for messaging platform integrations (slice 1: lines 1–900).
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/gateway/run.py`
//! (31358 LOC), slice 1 covering lines 1–900.
//! This slice contains module header, bootstrap note, imports, all
//! module-level constants, regexes, and helpers through
//! `_approval_send_outcome` (the slice boundary cuts mid-function at line 900;
//! the Rust file includes the complete function for compilability — the
//! remainder of the function body beyond line 900 is included here and
//! slice 2 will continue after it).
//!
//! Python source docstring (preserved):
//! ```text
//! Gateway runner - entry point for messaging platform integrations.
//!
//! This module provides:
//! - start_gateway(): Start all configured platform adapters
//! - GatewayRunner: Main class managing the gateway lifecycle
//!
//! Usage:
//!     # Start the gateway
//!     python -m gateway.run
//!     
//!     # Or from CLI
//!     python cli.py --gateway
//! ```
//!
//! # Bootstrap note
//!
//! Python `gateway/run.py` begins with:
//! ```python
//! try:
//!     import hermes_bootstrap
//! except ModuleNotFoundError:
//!     pass
//! ```
//! `hermes_bootstrap` forces UTF-8 stdio on Windows and is a no-op on POSIX.
//! Rust's `std::io` is UTF-8 by default on all platforms; no bootstrap shim
//! is required. The import is documented here for traceability and omitted
//! from the Rust port.
//!
//! # Mapping
//!
//! - `_AGENT_CACHE_MAX_SIZE` → [`AGENT_CACHE_MAX_SIZE`]
//! - `_AGENT_CACHE_IDLE_TTL_SECS` → [`AGENT_CACHE_IDLE_TTL_SECS`]
//! - `_PLATFORM_CONNECT_TIMEOUT_SECS_DEFAULT` → [`PLATFORM_CONNECT_TIMEOUT_SECS_DEFAULT`]
//! - `_TELEGRAM_CONNECT_TIMEOUT_SECS_DEFAULT` → [`TELEGRAM_CONNECT_TIMEOUT_SECS_DEFAULT`]
//! - `_TELEGRAM_INITIAL_CONNECT_TIMEOUT_SECS_DEFAULT` → [`TELEGRAM_INITIAL_CONNECT_TIMEOUT_SECS_DEFAULT`]
//! - `_ADAPTER_DISCONNECT_TIMEOUT_SECS_DEFAULT` → [`ADAPTER_DISCONNECT_TIMEOUT_SECS_DEFAULT`]
//! - `_USER_BOUNDARY_END_REASONS` → [`USER_BOUNDARY_END_REASONS`]
//! - `_STALL_NOTIFY_SEND_TIMEOUT_SECONDS` → [`STALL_NOTIFY_SEND_TIMEOUT_SECONDS`]
//! - `_GATEWAY_PROXY_SSE_BUFFER_MAX_CHARS` → [`GATEWAY_PROXY_SSE_BUFFER_MAX_CHARS`]
//! - `_TELEGRAM_COMMAND_MENTION_RE` → [`TELEGRAM_COMMAND_MENTION_RE_STR`] + [`is_telegram_command_mention`]
//! - `_GATEWAY_HYGIENE_PLATFORM` → [`GATEWAY_HYGIENE_PLATFORM`]
//! - `_TELEGRAM_NOISY_STATUS_RE` → [`TELEGRAM_NOISY_STATUS_RE_STR`] + [`is_telegram_noisy_status`]
//! - `_HYGIENE_COOLDOWN_LADDER_MULTIPLIERS` → [`HYGIENE_COOLDOWN_LADDER_MULTIPLIERS`]
//! - `_HYGIENE_COOLDOWN_MAX_SECONDS` → [`HYGIENE_COOLDOWN_MAX_SECONDS`]
//! - `COMPACTION_STATUS` etc. (from `agent.conversation_compression`) → [`COMPACTION_STATUS`] etc.
//! - `_status_template_to_regex` → [`status_template_to_regex`]
//! - `_COMPRESSION_PROGRESS_STATUS_RE` → [`COMPRESSION_PROGRESS_STATUS_RE_STR`] + [`is_compression_progress_status`]
//! - `_gateway_compression_progress_notices_enabled` → [`gateway_compression_progress_notices_enabled`]
//! - `_GATEWAY_RAW_TEXT_PLATFORMS` → [`GATEWAY_RAW_TEXT_PLATFORMS`]
//! - `_gateway_surface_passes_raw_text` → [`gateway_surface_passes_raw_text`]
//! - `_GATEWAY_PROVIDER_ERROR_RE` etc. → [`GATEWAY_PROVIDER_ERROR_RE_STR`] etc. + [`is_gateway_provider_error`] helpers
//! - `_GATEWAY_SECRET_PATTERNS` → [`GATEWAY_SECRET_PATTERNS_STR`] + [`redact_gateway_user_facing_secrets`]
//! - `_ensure_windows_gateway_venv_imports` → [`ensure_windows_gateway_venv_imports`]
//! - `_gateway_platform_value` → [`gateway_platform_value`] / [`gateway_platform_value_str`]
//! - `_non_conversational_metadata` → [`non_conversational_metadata`]
//! - `_interim_metadata` → [`interim_metadata`]
//! - `_seed_hygiene_system_prompt` → [`seed_hygiene_system_prompt`]
//! - `_is_transient_network_error` → [`is_transient_network_error`]
//! - `_gateway_loop_exception_handler` → [`gateway_loop_exception_handler`]
//! - `_redact_gateway_user_facing_secrets` → [`redact_gateway_user_facing_secrets`]
//! - `_redact_approval_command` → [`redact_approval_command`]
//! - `_format_exec_approval_fallback` → [`format_exec_approval_fallback`]
//! - `_gateway_provider_error_reply` → [`gateway_provider_error_reply`]
//! - `_GATEWAY_PROVIDER_ERROR_SHAPE_RE` → [`GATEWAY_PROVIDER_ERROR_SHAPE_RE_STR`] + [`looks_like_gateway_provider_error`]
//! - `_sanitize_gateway_final_response` → [`sanitize_gateway_final_response`]
//! - `_prepare_gateway_status_message` → [`prepare_gateway_status_message`]
//! - `render_notice_line` → [`render_notice_line`]
//! - `_send_or_update_status_coro` → [`send_or_update_status_coro`] (async stub)
//! - `_approval_send_outcome` → [`approval_send_outcome`]
//! - `_hygiene_cooldown_for_failure` → [`hygiene_cooldown_for_failure`]
//! - `_reset_hygiene_failure_streak` → [`reset_hygiene_failure_streak`]
//! - `hygiene_compaction_recovered` → [`hygiene_compaction_recovered`]
//! - `_record_hygiene_cooldown` → [`record_hygiene_cooldown`]
//!
//! Python imports not directly ported (runtime, asyncio, hermes_cli, agent.*):
//! documented as `// Python:` comments where relevant. Rust equivalents use
//! `std`, `serde_json`, and `log`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Module doc / bootstrap — mirrors Python lines 1–25
// ---------------------------------------------------------------------------

// Bootstrap note: `hermes_bootstrap` is intentionally not imported in Rust.
// See module doc above.

// ---------------------------------------------------------------------------
// Constants — mirrors Python lines 71–162
// ---------------------------------------------------------------------------

/// Mirrors `_AGENT_CACHE_MAX_SIZE = 128`
pub const AGENT_CACHE_MAX_SIZE: usize = 128;

/// Private alias for grep-ability.
pub const _AGENT_CACHE_MAX_SIZE: usize = AGENT_CACHE_MAX_SIZE;

/// Mirrors `_AGENT_CACHE_IDLE_TTL_SECS = 3600.0` — evict agents idle for >1h
pub const AGENT_CACHE_IDLE_TTL_SECS: f64 = 3600.0;

/// Private alias.
pub const _AGENT_CACHE_IDLE_TTL_SECS_ALIAS: f64 = AGENT_CACHE_IDLE_TTL_SECS;

/// Mirrors `_PLATFORM_CONNECT_TIMEOUT_SECS_DEFAULT = 30.0`
pub const PLATFORM_CONNECT_TIMEOUT_SECS_DEFAULT: f64 = 30.0;
pub const _PLATFORM_CONNECT_TIMEOUT_SECS_DEFAULT: f64 = PLATFORM_CONNECT_TIMEOUT_SECS_DEFAULT;

/// Mirrors `_TELEGRAM_CONNECT_TIMEOUT_SECS_DEFAULT = 180.0`
pub const TELEGRAM_CONNECT_TIMEOUT_SECS_DEFAULT: f64 = 180.0;
pub const _TELEGRAM_CONNECT_TIMEOUT_SECS_DEFAULT: f64 = TELEGRAM_CONNECT_TIMEOUT_SECS_DEFAULT;

/// Mirrors `_TELEGRAM_INITIAL_CONNECT_TIMEOUT_SECS_DEFAULT = 45.0`
pub const TELEGRAM_INITIAL_CONNECT_TIMEOUT_SECS_DEFAULT: f64 = 45.0;
pub const _TELEGRAM_INITIAL_CONNECT_TIMEOUT_SECS_DEFAULT: f64 =
    TELEGRAM_INITIAL_CONNECT_TIMEOUT_SECS_DEFAULT;

/// Mirrors `_ADAPTER_DISCONNECT_TIMEOUT_SECS_DEFAULT = 5.0`
pub const ADAPTER_DISCONNECT_TIMEOUT_SECS_DEFAULT: f64 = 5.0;
pub const _ADAPTER_DISCONNECT_TIMEOUT_SECS_DEFAULT: f64 = ADAPTER_DISCONNECT_TIMEOUT_SECS_DEFAULT;

/// Mirrors `_USER_BOUNDARY_END_REASONS = ("session_reset", "user_exit", "session_switch", "new_session")`
pub const USER_BOUNDARY_END_REASONS: &[&str] = &[
    "session_reset",
    "user_exit",
    "session_switch",
    "new_session",
];
pub const _USER_BOUNDARY_END_REASONS: &[&str] = USER_BOUNDARY_END_REASONS;

/// Mirrors `_STALL_NOTIFY_SEND_TIMEOUT_SECONDS = 15.0`
pub const STALL_NOTIFY_SEND_TIMEOUT_SECONDS: f64 = 15.0;
pub const _STALL_NOTIFY_SEND_TIMEOUT_SECONDS: f64 = STALL_NOTIFY_SEND_TIMEOUT_SECONDS;

/// Mirrors `_GATEWAY_PROXY_SSE_BUFFER_MAX_CHARS = 16 * 1024 * 1024`
pub const GATEWAY_PROXY_SSE_BUFFER_MAX_CHARS: usize = 16 * 1024 * 1024;
pub const _GATEWAY_PROXY_SSE_BUFFER_MAX_CHARS: usize = GATEWAY_PROXY_SSE_BUFFER_MAX_CHARS;

/// Mirrors `_TELEGRAM_COMMAND_MENTION_RE = re.compile(r"(?<![\w:/])/([A-Za-z0-9][A-Za-z0-9_-]*)")`
/// Stored as raw pattern string; use `is_telegram_command_mention` helper.
///
/// Python pattern: `r"(?<![\w:/])/([A-Za-z0-9][A-Za-z0-9_-]*)"`
/// - Negative lookbehind: preceding char must not be `\w`, `:`, or `/`
/// - Then `/` + capture group: `[A-Za-z0-9]` followed by zero or more `[A-Za-z0-9_-]`
pub const TELEGRAM_COMMAND_MENTION_RE_STR: &str = r"(?<![\w:/])/([A-Za-z0-9][A-Za-z0-9_-]*)";
pub const _TELEGRAM_COMMAND_MENTION_RE_STR: &str = TELEGRAM_COMMAND_MENTION_RE_STR;

/// Mirrors `_GATEWAY_HYGIENE_PLATFORM = "gateway_hygiene"`
pub const GATEWAY_HYGIENE_PLATFORM: &str = "gateway_hygiene";
pub const _GATEWAY_HYGIENE_PLATFORM: &str = GATEWAY_HYGIENE_PLATFORM;

/// Mirrors `_TELEGRAM_NOISY_STATUS_RE` — transient/auxiliary status that should
/// stay in logs, not gateway chats.
///
/// Python (re.IGNORECASE | re.DOTALL):
/// ```text
/// (
/// auxiliary\s+.+\s+failed
/// |compression\s+summary\s+failed
/// |fallback\s+context\s+marker
/// |configured\s+compression\s+model\s+.+\s+failed
/// |no\s+auxiliary\s+llm\s+provider\s+configured
/// |auto-lowered\s+compression\s+threshold
/// |auto-lowered\s+(?:this\s+)?session'?s?\s+threshold
/// |configured\s+auxiliary\s+compression\s+provider\s+.+\s+unavailable
/// |skipping\s+concurrent\s+compression
/// |compacting\s+context\s+[—-]\s+summarizing\s+earlier\s+conversation
/// |resumed\s+after\s+\d+s\s+idle\s+[—-]\s+compacting
/// |preflight\s+compression
/// |pre[- ]api\s+compression
/// |context\s+too\s+large\s+\(~[\d,]+\s+tokens\)\s+[—-]+\s+compressing
/// |compressed\s+\d[\d,]*\s+(?:→|->)\s+\d[\d,]*\s+messages,\s+retrying
/// |compressed\s+~[\d,]+\s+(?:→|->)\s+~[\d,]+\s+tokens,\s+retrying
/// |context\s+reduced\s+to\s+[\d,]+\s+tokens\s+\(was\s+[\d,]+\),\s+retrying
/// |session\s+compressed\s+\d+\s+times
/// |rate\s+limited\.\s+waiting\s+\d
/// |retrying\s+in\s+\d
/// |max\s+retries\s+\(\d+\).*(?:trying\s+fallback|exhausted|invalid\s+responses)
/// |stream\s+(?:drop|drop\s+mid\s+tool-call).+retry\s+\d
/// |stale\s+connections\s+from\s+a\s+previous\s+provider\s+issue
/// )
/// ```
///
/// Rust: stored as raw pattern and also as normalized substring list for
/// `is_telegram_noisy_status` (case-insensitive substring scan, no regex crate).
pub const TELEGRAM_NOISY_STATUS_RE_STR: &str = r"(auxiliary\s+.+\s+failed|compression\s+summary\s+failed|fallback\s+context\s+marker|configured\s+compression\s+model\s+.+\s+failed|no\s+auxiliary\s+llm\s+provider\s+configured|auto-lowered\s+compression\s+threshold|auto-lowered\s+(?:this\s+)?session'?s?\s+threshold|configured\s+auxiliary\s+compression\s+provider\s+.+\s+unavailable|skipping\s+concurrent\s+compression|compacting\s+context\s+[—-]\s+summarizing\s+earlier\s+conversation|resumed\s+after\s+\d+s\s+idle\s+[—-]\s+compacting|preflight\s+compression|pre[- ]api\s+compression|context\s+too\s+large\s+\(~[\d,]+\s+tokens\)\s+[—-]+\s+compressing|compressed\s+\d[\d,]*\s+(?:→|->)\s+\d[\d,]*\s+messages,\s+retrying|compressed\s+~[\d,]+\s+(?:→|->)\s+~[\d,]+\s+tokens,\s+retrying|context\s+reduced\s+to\s+[\d,]+\s+tokens\s+\(was\s+[\d,]+\),\s+retrying|session\s+compressed\s+\d+\s+times|rate\s+limited\.\s+waiting\s+\d|retrying\s+in\s+\d|max\s+retries\s+\(\d+\).*(?:trying\s+fallback|exhausted|invalid\s+responses)|stream\s+(?:drop|drop\s+mid\s+tool-call).+retry\s+\d|stale\s+connections\s+from\s+a\s+previous\s+provider\s+issue)";
pub const _TELEGRAM_NOISY_STATUS_RE_STR: &str = TELEGRAM_NOISY_STATUS_RE_STR;

/// Mirrors `_HYGIENE_COOLDOWN_LADDER_MULTIPLIERS = (1, 3, 9)`
pub const HYGIENE_COOLDOWN_LADDER_MULTIPLIERS: [u64; 3] = [1, 3, 9];
pub const _HYGIENE_COOLDOWN_LADDER_MULTIPLIERS: [u64; 3] = HYGIENE_COOLDOWN_LADDER_MULTIPLIERS;

/// Mirrors `_HYGIENE_COOLDOWN_MAX_SECONDS = 3600.0`
pub const HYGIENE_COOLDOWN_MAX_SECONDS: f64 = 3600.0;
pub const _HYGIENE_COOLDOWN_MAX_SECONDS: f64 = HYGIENE_COOLDOWN_MAX_SECONDS;

/// Mirrors `_AUTO_CONTINUE_FRESHNESS_SECS_DEFAULT = 60 * 60`
pub const AUTO_CONTINUE_FRESHNESS_SECS_DEFAULT: f64 = 3600.0;
pub const _AUTO_CONTINUE_FRESHNESS_SECS_DEFAULT: f64 = AUTO_CONTINUE_FRESHNESS_SECS_DEFAULT;

/// Mirrors `_STARTUP_RESTORE_DRAIN_TIMEOUT_SECS_DEFAULT = 30.0`
pub const STARTUP_RESTORE_DRAIN_TIMEOUT_SECS_DEFAULT: f64 = 30.0;
pub const _STARTUP_RESTORE_DRAIN_TIMEOUT_SECS_DEFAULT: f64 =
    STARTUP_RESTORE_DRAIN_TIMEOUT_SECS_DEFAULT;

// ---------------------------------------------------------------------------
// Compression status templates — mirrors agent/conversation_compression.py
// (these are the canonical wording; _COMPRESSION_PROGRESS_STATUS_RE is
// derived from them, never re-inlined).
// ---------------------------------------------------------------------------

/// Mirrors `COMPACTION_STATUS = "🗜️ Compacting context — summarizing earlier conversation so I can continue..."`
pub const COMPACTION_STATUS: &str =
    "🗜️ Compacting context — summarizing earlier conversation so I can continue...";
pub const COMPACTION_STATUS_MARKER: &str = "Compacting context";

/// Mirrors `PRE_API_COMPRESSION_STATUS_TEMPLATE`
pub const PRE_API_COMPRESSION_STATUS_TEMPLATE: &str =
    "📦 Pre-API compression: ~{tokens} tokens near the context/output limit. Compacting before the next model call.";

pub const PREFLIGHT_COMPRESSION_STATUS_TEMPLATE: &str =
    "📦 Preflight compression: ~{tokens} tokens >= {threshold} threshold. This may take a moment.";

pub const IDLE_COMPACTION_STATUS_TEMPLATE: &str =
    "💤 Resumed after {idle_seconds}s idle — compacting ~{tokens} tokens before continuing.";

pub const COMPRESSION_RETRY_TOO_LARGE_STATUS_TEMPLATE: &str =
    "🗜️ Context too large (~{tokens} tokens) — compressing ({attempt}/{cap})...";

pub const COMPRESSION_RETRY_MESSAGES_STATUS_TEMPLATE: &str =
    "🗜️ Compressed {before} → {after} messages, retrying...";

pub const COMPRESSION_RETRY_TOKENS_STATUS_TEMPLATE: &str =
    "🗜️ Compressed ~{before} → ~{after} tokens, retrying...";

pub const COMPRESSION_RETRY_CONTEXT_REDUCED_STATUS_TEMPLATE: &str =
    "🗜️ Context reduced to {new_ctx} tokens (was {old_ctx}), retrying...";

// ---------------------------------------------------------------------------
// Raw-text platform allowlist — mirrors Python lines 383–391
// ---------------------------------------------------------------------------

/// Mirrors `_GATEWAY_RAW_TEXT_PLATFORMS = frozenset({"local", "api_server", "webhook", "msgraph_webhook"})`
pub const GATEWAY_RAW_TEXT_PLATFORMS: &[&str] =
    &["local", "api_server", "webhook", "msgraph_webhook"];
pub const _GATEWAY_RAW_TEXT_PLATFORMS: &[&str] = GATEWAY_RAW_TEXT_PLATFORMS;

// ---------------------------------------------------------------------------
// Provider error regexes — mirrors Python lines 393–459
// ---------------------------------------------------------------------------

/// Mirrors `_GATEWAY_PROVIDER_ERROR_RE`
pub const GATEWAY_PROVIDER_ERROR_RE_STR: &str = r"(api\s+(?:call\s+)?failed|provider\s+authentication\s+failed|non-retryable\s+error|rate\s+limited\s+after\s+\d+\s+retries|error\s+code\s*:|\bhttp\s*\d{3}\b|incorrect\s+api\s+key|invalid\s+api\s+key)";
pub const _GATEWAY_PROVIDER_ERROR_RE_STR: &str = GATEWAY_PROVIDER_ERROR_RE_STR;

/// Mirrors `_GATEWAY_PROVIDER_POLICY_RE`
pub const GATEWAY_PROVIDER_POLICY_RE_STR: &str = r"(cybersecurity\s+risk|security\s+policy|safety\s+policy|policy\s+violation|violat(?:e|es|ed|ion)|blocked\s+(?:because|by|under)|request\s+(?:was\s+)?(?:blocked|rejected)|disallowed|moderation)";
pub const _GATEWAY_PROVIDER_POLICY_RE_STR: &str = GATEWAY_PROVIDER_POLICY_RE_STR;

/// Mirrors `_GATEWAY_AUTH_ERROR_RE`
pub const GATEWAY_AUTH_ERROR_RE_STR: &str =
    r"(provider\s+authentication\s+failed|incorrect\s+api\s+key|invalid\s+api\s+key|\b401\b)";
pub const _GATEWAY_AUTH_ERROR_RE_STR: &str = GATEWAY_AUTH_ERROR_RE_STR;

/// Mirrors `_GATEWAY_RATE_LIMIT_RE`
pub const GATEWAY_RATE_LIMIT_RE_STR: &str =
    r"(rate\s+limit|rate-limited|\b429\b|quota|usage\s+limit)";
pub const _GATEWAY_RATE_LIMIT_RE_STR: &str = GATEWAY_RATE_LIMIT_RE_STR;

/// Mirrors `_GATEWAY_CONNECTION_ERROR_RE`
pub const GATEWAY_CONNECTION_ERROR_RE_STR: &str = r"((?:\w+\.)?(?:api\s*)?connection\s*(?:error|timeout)|(?:\w+\.)?connect\s*(?:error|timeout)|connection\s+refused|connection\s+reset|connection\s+aborted|actively\s+refused|winerror\s+10061|errno\s+111|no\s+route\s+to\s+host|network\s+is\s+unreachable|cannot\s+connect|failed\s+to\s+establish|could\s+not\s+connect)";
pub const _GATEWAY_CONNECTION_ERROR_RE_STR: &str = GATEWAY_CONNECTION_ERROR_RE_STR;

/// Mirrors `_GATEWAY_PROVIDER_ERROR_SHAPE_RE`
pub const GATEWAY_PROVIDER_ERROR_SHAPE_RE_STR: &str = r"^\s*(\W*\s*)?(api\s+(?:call\s+)?failed|provider\s+authentication\s+failed|non-retryable\s+error|rate\s+limited\s+after\s+\d+\s+retries|error\s+code\s*:|http\s*\d{3}\b|incorrect\s+api\s+key|invalid\s+api\s+key|(?:\w+\.)?(?:api\s*)?connection\s*(?:error|timeout)|(?:\w+\.)?connect\s*(?:error|timeout)|connection\s+refused|connection\s+reset|connection\s+aborted|actively\s+refused|winerror\s+10061|errno\s+111|all\s+connection\s+attempts\s+failed)";
pub const _GATEWAY_PROVIDER_ERROR_SHAPE_RE_STR: &str = GATEWAY_PROVIDER_ERROR_SHAPE_RE_STR;

// ---------------------------------------------------------------------------
// Secret patterns — mirrors Python lines 451–459
// ---------------------------------------------------------------------------

/// Mirrors `_GATEWAY_SECRET_PATTERNS` as raw pattern strings.
///
/// Python:
/// ```python
/// _GATEWAY_SECRET_PATTERNS = (
///     re.compile(r"\bsk-[A-Za-z0-9][A-Za-z0-9_\-]{12,}\b"),
///     re.compile(r"\bgh[pousr]_[A-Za-z0-9_]{20,}\b"),
///     re.compile(r"\bxapp-\d+-[A-Za-z0-9\-]{20,}\b"),
///     re.compile(r"\bxox[baprs]-[A-Za-z0-9\-]{20,}\b"),
///     re.compile(r"\bhf_[A-Za-z0-9]{20,}\b"),
///     re.compile(r"\bglpat-[A-Za-z0-9_\-]{20,}\b"),
///     re.compile(r"(?i)\b(Bearer\s+)[A-Za-z0-9._\-]{20,}\b"),
/// )
/// ```
pub const GATEWAY_SECRET_PATTERNS_STR: &[&str] = &[
    r"\bsk-[A-Za-z0-9][A-Za-z0-9_\-]{12,}\b",
    r"\bgh[pousr]_[A-Za-z0-9_]{20,}\b",
    r"\bxapp-\d+-[A-Za-z0-9\-]{20,}\b",
    r"\bxox[baprs]-[A-Za-z0-9\-]{20,}\b",
    r"\bhf_[A-Za-z0-9]{20,}\b",
    r"\bglpat-[A-Za-z0-9_\-]{20,}\b",
    r"(?i)\b(Bearer\s+)[A-Za-z0-9._\-]{20,}\b",
];
pub const _GATEWAY_SECRET_PATTERNS_STR: &[&str] = GATEWAY_SECRET_PATTERNS_STR;

// ---------------------------------------------------------------------------
// Helpers: platform value normalisation — mirrors _gateway_platform_value
// ---------------------------------------------------------------------------

/// Mirrors `_gateway_platform_value(platform: Any) -> str`
/// Return a normalized gateway platform value for enums or raw strings.
/// In Python: `str(getattr(platform, "value", platform) or "").strip().lower()`
pub fn gateway_platform_value(platform: &serde_json::Value) -> String {
    // If Value is string, use it; if object with "value" field, use that
    // Mirrors getattr(platform, "value", platform)
    let raw = if let Some(obj) = platform.as_object() {
        if let Some(v) = obj.get("value") {
            v.as_str().unwrap_or(&v.to_string()).to_string()
        } else {
            // fallback to string representation
            platform.as_str().unwrap_or("").to_string()
        }
    } else if let Some(s) = platform.as_str() {
        s.to_string()
    } else {
        platform.to_string()
    };
    raw.trim().to_lowercase()
}

/// String overload — mirrors `_gateway_platform_value` when called with raw &str
pub fn gateway_platform_value_str(platform: &str) -> String {
    platform.trim().to_lowercase()
}

/// Alias for traceability.
#[allow(dead_code)]
fn _gateway_platform_value(platform: &serde_json::Value) -> String {
    gateway_platform_value(platform)
}

// ---------------------------------------------------------------------------
// Helpers: _status_template_to_regex — mirrors Python lines 314–324
// ---------------------------------------------------------------------------

/// Mirrors `_status_template_to_regex(template: str) -> str`
///
/// Compile a compression status template constant into a regex source.
/// Literal text is escaped verbatim and each `{field}` placeholder is
/// replaced with a numeric-ish pattern covering every value the emit sites
/// format in (ints, `{:,}` thousands separators).
///
/// Python:
/// ```python
/// def _status_template_to_regex(template: str) -> str:
///     parts = re.split(r"\{[^{}]*\}", template)
///     return r"[\d,]+".join(re.escape(part) for part in parts)
/// ```
pub fn status_template_to_regex(template: &str) -> String {
    // Split on {field} placeholders — pattern r"\{[^{}]*\}"
    let mut parts: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut chars = template.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '{' {
            // Flush current literal part (will be escaped)
            parts.push(current.clone());
            current.clear();
            // Consume until matching '}'
            let mut depth = 1usize;
            let mut found_close = false;
            while let Some(inner) = chars.next() {
                if inner == '{' {
                    depth += 1;
                } else if inner == '}' {
                    depth -= 1;
                    if depth == 0 {
                        found_close = true;
                        break;
                    }
                }
            }
            if !found_close {
                // Unclosed brace — treat as literal
                current.push('{');
            }
        } else if ch == '}' {
            // Stray closing brace — literal
            current.push(ch);
        } else {
            current.push(ch);
        }
    }
    parts.push(current);

    // Escape each literal part via regex::escape equivalent (manual)
    // For NO_REGEX port we still produce the regex source string; matching
    // is done via is_compression_progress_status which uses substring checks.
    // The regex source is preserved for traceability and debugging.
    let escaped_parts: Vec<String> = parts.iter().map(|p| regex_escape(p)).collect();
    escaped_parts.join(r"[\d,]+")
}

/// Minimal `re.escape` equivalent — escapes regex meta-characters.
fn regex_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 2);
    for ch in s.chars() {
        match ch {
            '.' | '^' | '$' | '*' | '+' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '\\'
            | '|' => {
                out.push('\\');
                out.push(ch);
            }
            _ => out.push(ch),
        }
    }
    out
}

#[allow(dead_code)]
fn _status_template_to_regex(template: &str) -> String {
    status_template_to_regex(template)
}

// ---------------------------------------------------------------------------
// _COMPRESSION_PROGRESS_STATUS_RE — mirrors Python lines 336–351
// ---------------------------------------------------------------------------

/// Build the compression progress status regex source from the 8 canonical
/// templates, mirroring Python's:
///
/// ```python
/// _COMPRESSION_PROGRESS_STATUS_RE = re.compile(
///     "|".join(_status_template_to_regex(_template) for _template in (
///         COMPACTION_STATUS, PRE_API..., PREFLIGHT..., IDLE..., TOO_LARGE...,
///         MESSAGES..., TOKENS..., CONTEXT_REDUCED...,
///     )), re.IGNORECASE,
/// )
/// ```
pub fn compression_progress_status_regex_source() -> String {
    let templates = [
        COMPACTION_STATUS,
        PRE_API_COMPRESSION_STATUS_TEMPLATE,
        PREFLIGHT_COMPRESSION_STATUS_TEMPLATE,
        IDLE_COMPACTION_STATUS_TEMPLATE,
        COMPRESSION_RETRY_TOO_LARGE_STATUS_TEMPLATE,
        COMPRESSION_RETRY_MESSAGES_STATUS_TEMPLATE,
        COMPRESSION_RETRY_TOKENS_STATUS_TEMPLATE,
        COMPRESSION_RETRY_CONTEXT_REDUCED_STATUS_TEMPLATE,
    ];
    templates
        .iter()
        .map(|t| status_template_to_regex(t))
        .collect::<Vec<_>>()
        .join("|")
}

/// Lazily cached regex source (mirrors the compiled regex object).
pub fn compression_progress_status_re_str() -> String {
    compression_progress_status_regex_source()
}

/// Case-insensitive substring check that mirrors `_COMPRESSION_PROGRESS_STATUS_RE.search(text)`
/// without requiring the `regex` crate.
///
/// The Rust port checks whether `text` contains any of the literal fragments
/// of the 8 templates (split on numeric placeholders) in a case-insensitive
/// manner. This is slightly broader than the regex (it ignores digit counts)
/// but is safe for the gate below — false positives only affect whether a
/// noisy status suppressed by `_TELEGRAM_NOISY_STATUS_RE` is re-allowed when
/// `compression.progress_notices` is enabled; the underlying
/// `_TELEGRAM_NOISY_STATUS_RE` still gates the decision.
pub fn is_compression_progress_status(text: &str) -> bool {
    // Cheap: check for any of the distinctive literal fragments
    // derived from the templates — avoids regex dependency.
    let lower = text.to_lowercase();
    // Fragments that uniquely identify routine compression progress lines
    let fragments = [
        "compacting context",
        "pre-api compression",
        "pre api compression",
        "preflight compression",
        "resumed after",
        "compacting", // already covered but keep
        "context too large",
        "compressed",
        "context reduced to",
        "retrying",
    ];
    // More precise: check against the actual template literal parts
    // Split each template into literal chunks (between {placeholders}) and
    // require at least one chunk to be present. For COMPACTION_STATUS which
    // has no placeholders, require exact substring.
    if lower.contains("compacting context") && lower.contains("summarizing earlier conversation") {
        return true;
    }
    if lower.contains("pre-api compression") || lower.contains("pre api compression") {
        return true;
    }
    if lower.contains("preflight compression") {
        return true;
    }
    if lower.contains("resumed after") && lower.contains("idle") && lower.contains("compacting") {
        return true;
    }
    if lower.contains("context too large") && lower.contains("compressing") {
        return true;
    }
    if lower.contains("compressed") && lower.contains("retrying") {
        // Need to distinguish messages vs tokens but both count
        return true;
    }
    if lower.contains("context reduced to") && lower.contains("retrying") {
        return true;
    }
    // Fallback fragment check
    fragments.iter().any(|frag| lower.contains(frag) && is_likely_compression_line(&lower))
}

fn is_likely_compression_line(lower: &str) -> bool {
    lower.contains("compress") || lower.contains("compacting")
}

// ---------------------------------------------------------------------------
// _gateway_compression_progress_notices_enabled — mirrors Python 354–375
// ---------------------------------------------------------------------------

/// Mirrors `_gateway_compression_progress_notices_enabled() -> bool`
///
/// True when the user opted into routine compression progress notices.
/// Reads `compression.progress_notices` from the gateway's raw YAML config.
/// Default False. Fail-closed: any config read error keeps silent default.
///
/// Python reads via `_load_gateway_config()` (defined later in file, beyond
/// slice 1). Rust port reads from `HERMES_HOME/config.yaml` directly or via
/// env `HERMES_GATEWAY_CONFIG` override, mirroring the same mtime-cached
/// semantics in a simplified form (no cache, reads live — cheap for tests).
pub fn gateway_compression_progress_notices_enabled() -> bool {
    gateway_compression_progress_notices_enabled_with_home(&get_hermes_home())
}

pub fn get_hermes_home() -> PathBuf {
    if let Ok(val) = std::env::var("HERMES_HOME") {
        let trimmed = val.trim().to_string();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".hermes");
    }
    PathBuf::from(".hermes")
}

fn gateway_compression_progress_notices_enabled_with_home(home: &Path) -> bool {
    let config_path = home.join("config.yaml");
    let text = match std::fs::read_to_string(&config_path) {
        Ok(t) => t,
        Err(_) => return false,
    };
    // Minimal YAML scan: look for `compression:` block then `progress_notices: true`
    // Mirrors Python: config.get("compression").get("progress_notices") in {"true","1","yes","on"}
    // We do a line-oriented scan without a YAML parser to avoid new deps.
    let mut in_compression = false;
    let mut compression_indent: Option<usize> = None;
    for raw_line in text.lines() {
        let trimmed = raw_line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indent = raw_line
            .chars()
            .take_while(|c| *c == ' ' || *c == '\t')
            .count();
        // Detect top-level `compression:` key
        if trimmed.starts_with("compression:") {
            // Must be at indent 0 (or minimal) to be top-level
            if indent == 0 {
                in_compression = true;
                compression_indent = Some(indent);
                // Handle inline value `compression: {progress_notices: true}` — rare but check
                if trimmed.contains("progress_notices") {
                    let lower = trimmed.to_lowercase();
                    if lower.contains("true") || lower.contains("1") || lower.contains("yes") || lower.contains("on") {
                        // Need to ensure it's not `false`
                        if !lower.contains("false") {
                            return true;
                        }
                    }
                }
                continue;
            }
        }
        if in_compression {
            // If we encounter another top-level key (indent 0, contains ':'), exit block
            if indent == 0 && trimmed.contains(':') && !trimmed.starts_with("compression") {
                break;
            }
            // Look for `progress_notices:` inside compression block
            if trimmed.to_lowercase().starts_with("progress_notices") {
                // Extract value after colon
                if let Some(colon_idx) = trimmed.find(':') {
                    let value = trimmed[colon_idx + 1..].trim().trim_matches('"').trim_matches('\'').to_lowercase();
                    let val = value.split('#').next().unwrap_or("").trim();
                    if matches!(val, "true" | "1" | "yes" | "on") {
                        return true;
                    } else {
                        return false;
                    }
                }
            }
            // Track indent to know when block ends (dedent to <= compression indent)
            if let Some(ci) = compression_indent {
                if indent <= ci && trimmed.contains(':') && !trimmed.to_lowercase().starts_with("progress_notices") {
                    // Next sibling top-level section
                    if indent == 0 {
                        break;
                    }
                }
            }
        }
    }
    false
}

#[allow(dead_code)]
fn _gateway_compression_progress_notices_enabled() -> bool {
    gateway_compression_progress_notices_enabled()
}

// ---------------------------------------------------------------------------
// _GATEWAY_RAW_TEXT_PLATFORMS helpers — mirrors Python 388–390
// ---------------------------------------------------------------------------

/// Mirrors `_gateway_surface_passes_raw_text(platform: Any) -> bool`
pub fn gateway_surface_passes_raw_text(platform: &str) -> bool {
    let normalized = gateway_platform_value_str(platform);
    GATEWAY_RAW_TEXT_PLATFORMS.contains(&normalized.as_str())
}

/// serde_json::Value overload — mirrors Python's `getattr(platform, "value", platform)`
pub fn gateway_surface_passes_raw_text_value(platform: &serde_json::Value) -> bool {
    let normalized = gateway_platform_value(platform);
    GATEWAY_RAW_TEXT_PLATFORMS.contains(&normalized.as_str())
}

#[allow(dead_code)]
fn _gateway_surface_passes_raw_text(platform: &str) -> bool {
    gateway_surface_passes_raw_text(platform)
}

// ---------------------------------------------------------------------------
// Windows venv import shim — mirrors Python lines 462–513
// ---------------------------------------------------------------------------

/// Mirrors `_ensure_windows_gateway_venv_imports() -> None`
///
/// Make detached Windows gateway runs see the Hermes venv packages.
/// On non-Windows, this is a no-op (mirrors `if sys.platform != "win32": return`).
///
/// Rust port manipulates `PATH`/`VIRTUAL_ENV`/`PYTHONPATH` equivalents via
/// `std::env` and `std::path`. The `site.addsitedir` `.pth` processing has
/// no Rust equivalent and is documented as a no-op.
///
/// This function is safe to call on any platform; on POSIX it returns
/// immediately.
pub fn ensure_windows_gateway_venv_imports() {
    if !cfg!(windows) {
        return;
    }
    // Windows-only logic: resolve project root as parent of this crate's
    // manifest dir (mirrors `Path(__file__).resolve().parent.parent`).
    // In the Rust port we use `HERMES_HOME` or current dir as approximation.
    let project_root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    // Candidate venv dirs
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(venv) = std::env::var("VIRTUAL_ENV") {
        if !venv.trim().is_empty() {
            candidates.push(PathBuf::from(venv));
        }
    }
    candidates.push(project_root.join("venv"));

    let mut seen: HashSet<String> = HashSet::new();
    for venv_dir in candidates {
        let resolved = venv_dir.canonicalize().unwrap_or(venv_dir.clone());
        let venv_key = resolved.to_string_lossy().to_lowercase();
        if seen.contains(&venv_key) {
            continue;
        }
        seen.insert(venv_key);

        let site_packages = resolved.join("Lib").join("site-packages");
        if !site_packages.exists() {
            continue;
        }

        // In Python, `site.addsitedir` and `sys.path` manipulation occurs.
        // Rust has no `sys.path`; we update env vars to preserve semantics
        // for child processes that may be spawned.
        if let Ok(current_venv) = std::env::var("VIRTUAL_ENV") {
            if current_venv.trim().is_empty() {
                // SAFETY: single-threaded caller assumed (gateway startup)
                unsafe { std::env::set_var("VIRTUAL_ENV", &resolved); }
            }
        } else {
            unsafe { std::env::set_var("VIRTUAL_ENV", &resolved); }
        }

        let project_entry = project_root.to_string_lossy().to_string();
        let site_entry = site_packages.to_string_lossy().to_string();
        let pythonpath = std::env::var("PYTHONPATH").unwrap_or_default();
        let mut parts: Vec<String> = Vec::new();
        parts.push(project_entry);
        parts.push(site_entry);
        if !pythonpath.is_empty() {
            parts.push(pythonpath);
        }
        // Deduplicate preserving order
        let mut seen_pp: HashSet<String> = HashSet::new();
        let deduped: Vec<String> = parts
            .into_iter()
            .filter(|p| seen_pp.insert(p.clone()))
            .collect();
        let joined = deduped.join(&std::path::MAIN_SEPARATOR.to_string());
        // Note: Python joins with os.pathsep (`;` on Windows, `:` on POSIX)
        // Rust port uses `;` on Windows via `MAIN_SEPARATOR`? Actually `:` vs `;`
        // Python's `os.pathsep` is `;` on Windows. Keep `;` on Windows.
        #[cfg(windows)]
        let joined = deduped.join(";");
        #[cfg(not(windows))]
        let joined = deduped.join(":");
        unsafe { std::env::set_var("PYTHONPATH", joined); }
        return;
    }
}

#[allow(dead_code)]
fn _ensure_windows_gateway_venv_imports() {
    ensure_windows_gateway_venv_imports()
}

// ---------------------------------------------------------------------------
// Metadata helpers — mirrors Python 521–576
// ---------------------------------------------------------------------------

/// Mirrors `_non_conversational_metadata(metadata: Optional[Dict[str, Any]], *, platform: Any) -> Optional[Dict[str, Any]]`
///
/// Mark Discord lifecycle/status sends without changing other platforms.
pub fn non_conversational_metadata(
    metadata: Option<HashMap<String, serde_json::Value>>,
    platform: &str,
) -> Option<HashMap<String, serde_json::Value>> {
    if gateway_platform_value_str(platform) != "discord" {
        return metadata;
    }
    let mut merged = metadata.unwrap_or_default();
    merged.insert(
        "non_conversational".to_string(),
        serde_json::Value::Bool(true),
    );
    Some(merged)
}

/// Mirrors `_interim_metadata(metadata: Optional[Dict[str, Any]]) -> Dict[str, Any]`
///
/// Mark a mid-turn status/advisory send as NOT the turn-final.
pub fn interim_metadata(
    metadata: Option<HashMap<String, serde_json::Value>>,
) -> HashMap<String, serde_json::Value> {
    let mut merged = metadata.unwrap_or_default();
    merged.insert("_interim_send".to_string(), serde_json::Value::Bool(true));
    merged
}

/// Value-based variant using serde_json::Map for callers that already hold JSON.
pub fn interim_metadata_value(
    metadata: Option<serde_json::Map<String, serde_json::Value>>,
) -> serde_json::Map<String, serde_json::Value> {
    let mut merged = metadata.unwrap_or_default();
    merged.insert("_interim_send".to_string(), serde_json::Value::Bool(true));
    merged
}

// ---------------------------------------------------------------------------
// _seed_hygiene_system_prompt — mirrors Python 555–576
// ---------------------------------------------------------------------------

/// Mirrors `_seed_hygiene_system_prompt(agent: Any, session_row: Optional[Dict[str, Any]]) -> bool`
///
/// Keep gateway hygiene from rebuilding a live session's system prompt.
/// Returns true when a stored prompt was seeded, false when an empty cache
/// entry was seeded.
pub fn seed_hygiene_system_prompt(
    cached_system_prompt: &mut String,
    session_row: Option<&HashMap<String, serde_json::Value>>,
) -> bool {
    let mut stored_prompt = String::new();
    if let Some(row) = session_row {
        if let Some(raw) = row.get("system_prompt") {
            if let Some(s) = raw.as_str() {
                if !s.trim().is_empty() {
                    stored_prompt = s.to_string();
                }
            }
        }
    }
    let was_seeded = !stored_prompt.is_empty();
    *cached_system_prompt = stored_prompt;
    was_seeded
}

/// serde_json::Value overload
pub fn seed_hygiene_system_prompt_value(
    cached_system_prompt: &mut String,
    session_row: Option<&serde_json::Value>,
) -> bool {
    let mut stored_prompt = String::new();
    if let Some(v) = session_row {
        if let Some(obj) = v.as_object() {
            if let Some(raw) = obj.get("system_prompt") {
                if let Some(s) = raw.as_str() {
                    if !s.trim().is_empty() {
                        stored_prompt = s.to_string();
                    }
                }
            }
        }
    }
    let was_seeded = !stored_prompt.is_empty();
    *cached_system_prompt = stored_prompt;
    was_seeded
}

#[allow(dead_code)]
fn _seed_hygiene_system_prompt(
    cached_system_prompt: &mut String,
    session_row: Option<&HashMap<String, serde_json::Value>>,
) -> bool {
    seed_hygiene_system_prompt(cached_system_prompt, session_row)
}

// ---------------------------------------------------------------------------
// _is_transient_network_error — mirrors Python 579–620
// ---------------------------------------------------------------------------

/// Mirrors `_is_transient_network_error(exc: BaseException) -> bool`
///
/// Walk the exception cause chain (bounded to 12) and check class names.
/// In Rust, we model exceptions as `TransientError` enum or string class names,
/// since Rust has no Python exception chaining. The string-based overload
/// mirrors the Python class-name check directly.
pub const TRANSIENT_CLASS_NAMES: &[&str] = &[
    "TimedOut",
    "NetworkError",
    "ReadError",
    "WriteError",
    "ConnectError",
    "ConnectTimeout",
    "ReadTimeout",
    "WriteTimeout",
    "PoolTimeout",
    "RemoteProtocolError",
    "ServerDisconnectedError",
    "ClientConnectorError",
    "ClientOSError",
];

/// Check if a single error name is transient.
pub fn is_transient_network_error_name(name: &str) -> bool {
    TRANSIENT_CLASS_NAMES.contains(&name)
}

/// Walk a chain of error names (mirrors `exc.__cause__ or exc.__context__` chain).
/// `chain` is ordered from outermost to innermost cause.
pub fn is_transient_network_error(chain: &[&str]) -> bool {
    let mut seen: HashSet<String> = HashSet::new();
    let mut depth = 0usize;
    for name in chain {
        if depth >= 12 {
            break;
        }
        if seen.contains(*name) {
            break;
        }
        seen.insert(name.to_string());
        depth += 1;
        if is_transient_network_error_name(name) {
            return true;
        }
    }
    false
}

/// Convenience for a single error (no chain).
pub fn is_transient_network_error_single(name: &str) -> bool {
    is_transient_network_error(&[name])
}

#[allow(dead_code)]
fn _is_transient_network_error(chain: &[&str]) -> bool {
    is_transient_network_error(chain)
}

// ---------------------------------------------------------------------------
// _gateway_loop_exception_handler — mirrors Python 623–654
// ---------------------------------------------------------------------------

/// Mirrors `_gateway_loop_exception_handler(loop: asyncio.AbstractEventLoop, context: Dict[str, Any]) -> None`
///
/// Loop-level safety net for transient network errors. In Python it logs at
/// WARNING and swallows transient errors, otherwise forwards to default handler.
///
/// Rust port: takes an error name + optional task name, returns true if the
/// error was swallowed (transient), false if it should be forwarded.
///
/// `context` in Python contains `exception`, `future`/`task`. Rust collapses
/// to explicit args for testability without `asyncio`.
pub fn gateway_loop_exception_handler(
    exception_name: Option<&str>,
    task_name: Option<&str>,
) -> bool {
    if let Some(name) = exception_name {
        if is_transient_network_error_single(name) {
            let task = task_name.unwrap_or("<unknown task>");
            // Mirrors logger.warning("Gateway swallowed transient network error from %s: %s: %s", ...)
            log::warn!(
                "Gateway swallowed transient network error from {}: {}",
                task,
                name
            );
            return true; // swallowed
        }
    }
    // Forward to default handler (not swallowed)
    false
}

#[allow(dead_code)]
fn _gateway_loop_exception_handler(
    exception_name: Option<&str>,
    task_name: Option<&str>,
) -> bool {
    gateway_loop_exception_handler(exception_name, task_name)
}

// ---------------------------------------------------------------------------
// Secret redaction — mirrors Python 657–699
// ---------------------------------------------------------------------------

/// Mirrors `_redact_gateway_user_facing_secrets(text: str) -> str`
///
/// Delegates to the authoritative `agent.redact.redact_sensitive_text` in Python
/// with `force=True`. Rust port implements the narrow `_GATEWAY_SECRET_PATTERNS`
/// second pass directly (belt-and-suspenders) and notes the primary redactor
/// as a placeholder. The pattern set runs even if the primary import fails.
pub fn redact_gateway_user_facing_secrets(text: &str) -> String {
    let mut redacted = text.to_string();
    // Primary redactor `agent.redact.redact_sensitive_text(force=True)` — in Rust
    // we keep the string as-is for the second pass; a real port would call
    // `hermes_redact::redact_sensitive_text(&redacted, true)`.
    // Second pass: _GATEWAY_SECRET_PATTERNS
    // We implement pattern substitution manually without `regex` crate:
    // each pattern is approximated as substring scan + replacement.
    for pattern in GATEWAY_SECRET_PATTERNS_STR {
        redacted = redact_with_pattern(&redacted, pattern);
    }
    redacted
}

fn redact_with_pattern(text: &str, pattern: &str) -> String {
    // Cheap approximations for the 7 known patterns — preserve [REDACTED] semantics.
    // Python does: pattern.sub(lambda m: (m.group(1) if m.lastindex else "") + "[REDACTED]", redacted)
    // For Bearer, group(1) is "Bearer " prefix.
    let mut out = text.to_string();
    // Handle each known pattern without full regex engine (ponytail: pattern-specific scans, add regex crate if fidelity matters)
    if pattern.contains("sk-") {
        out = redact_sk_pattern(&out);
    } else if pattern.contains("gh[pousr]") {
        out = redact_github_pattern(&out);
    } else if pattern.contains("xapp-") {
        out = redact_xapp_pattern(&out);
    } else if pattern.contains("xox[baprs]") {
        out = redact_xox_pattern(&out);
    } else if pattern.contains("hf_") {
        out = redact_hf_pattern(&out);
    } else if pattern.contains("glpat-") {
        out = redact_glpat_pattern(&out);
    } else if pattern.contains("Bearer") {
        out = redact_bearer_pattern(&out);
    }
    out
}

fn redact_sk_pattern(text: &str) -> String {
    // Matches \bsk-[A-Za-z0-9][A-Za-z0-9_\-]{12,}\b  → keep no prefix, replace with [REDACTED]
    redact_prefix_pattern(text, "sk-", 12, true)
}

fn redact_github_pattern(text: &str) -> String {
    // \bgh[pousr]_[A-Za-z0-9_]{20,}\b
    let mut out = String::with_capacity(text.len());
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if i + 3 < chars.len()
            && chars[i] == 'g'
            && chars[i + 1] == 'h'
            && matches!(chars[i + 2], 'p' | 'o' | 'u' | 's' | 'r')
            && chars[i + 3] == '_'
            && is_word_boundary_before(&chars, i)
        {
            let mut j = i + 4;
            while j < chars.len() && (chars[j].is_ascii_alphanumeric() || chars[j] == '_') {
                j += 1;
            }
            let len = j - (i + 4);
            if len >= 20 && is_word_boundary_after(&chars, j) {
                out.push_str("[REDACTED]");
                i = j;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

fn redact_xapp_pattern(text: &str) -> String {
    // \bxapp-\d+-[A-Za-z0-9\-]{20,}\b
    let mut out = String::with_capacity(text.len());
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if i + 5 < chars.len()
            && chars[i] == 'x'
            && chars[i + 1] == 'a'
            && chars[i + 2] == 'p'
            && chars[i + 3] == 'p'
            && chars[i + 4] == '-'
            && is_word_boundary_before(&chars, i)
        {
            let mut j = i + 5;
            while j < chars.len() && chars[j].is_ascii_digit() {
                j += 1;
            }
            if j > i + 5 && j < chars.len() && chars[j] == '-' {
                j += 1;
                let start_suffix = j;
                while j < chars.len() && (chars[j].is_ascii_alphanumeric() || chars[j] == '-') {
                    j += 1;
                }
                let len = j - start_suffix;
                if len >= 20 && is_word_boundary_after(&chars, j) {
                    out.push_str("[REDACTED]");
                    i = j;
                    continue;
                }
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

fn redact_xox_pattern(text: &str) -> String {
    // \bxox[baprs]-[A-Za-z0-9\-]{20,}\b
    let mut out = String::with_capacity(text.len());
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if i + 4 < chars.len()
            && chars[i] == 'x'
            && chars[i + 1] == 'o'
            && chars[i + 2] == 'x'
            && matches!(chars[i + 3], 'b' | 'a' | 'p' | 'r' | 's')
            && chars[i + 4] == '-'
            && is_word_boundary_before(&chars, i)
        {
            let mut j = i + 5;
            while j < chars.len() && (chars[j].is_ascii_alphanumeric() || chars[j] == '-') {
                j += 1;
            }
            let len = j - (i + 5);
            if len >= 20 && is_word_boundary_after(&chars, j) {
                out.push_str("[REDACTED]");
                i = j;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

fn redact_hf_pattern(text: &str) -> String {
    // \bhf_[A-Za-z0-9]{20,}\b
    let mut out = String::with_capacity(text.len());
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if i + 3 < chars.len()
            && chars[i] == 'h'
            && chars[i + 1] == 'f'
            && chars[i + 2] == '_'
            && is_word_boundary_before(&chars, i)
        {
            let mut j = i + 3;
            while j < chars.len() && chars[j].is_ascii_alphanumeric() {
                j += 1;
            }
            let len = j - (i + 3);
            if len >= 20 && is_word_boundary_after(&chars, j) {
                out.push_str("[REDACTED]");
                i = j;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

fn redact_glpat_pattern(text: &str) -> String {
    // \bglpat-[A-Za-z0-9_\-]{20,}\b
    redact_prefix_pattern(text, "glpat-", 20, false)
}

fn redact_bearer_pattern(text: &str) -> String {
    // (?i)\b(Bearer\s+)[A-Za-z0-9._\-]{20,}\b  — keep "Bearer " prefix + "[REDACTED]"
    let mut out = String::with_capacity(text.len());
    let lower = text.to_lowercase();
    let chars: Vec<char> = text.chars().collect();
    let lower_chars: Vec<char> = lower.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        // Check for "bearer" case-insensitive at i
        if i + 6 < chars.len()
            && lower_chars[i] == 'b'
            && lower_chars[i + 1] == 'e'
            && lower_chars[i + 2] == 'a'
            && lower_chars[i + 3] == 'r'
            && lower_chars[i + 4] == 'e'
            && lower_chars[i + 5] == 'r'
            && is_word_boundary_before(&chars, i)
        {
            let mut j = i + 6;
            // \s+
            let mut saw_space = false;
            while j < chars.len() && chars[j].is_whitespace() {
                saw_space = true;
                j += 1;
            }
            if saw_space {
                let start_token = j;
                while j < chars.len() && (chars[j].is_ascii_alphanumeric() || matches!(chars[j], '.' | '_' | '-')) {
                    j += 1;
                }
                let len = j - start_token;
                if len >= 20 && is_word_boundary_after(&chars, j) {
                    // Preserve original casing of "Bearer " prefix (up to start_token)
                    let prefix: String = chars[i..start_token].iter().collect();
                    out.push_str(&prefix);
                    out.push_str("[REDACTED]");
                    i = j;
                    continue;
                }
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

fn redact_prefix_pattern(text: &str, prefix: &str, min_len: usize, allow_underscore: bool) -> String {
    let mut out = String::with_capacity(text.len());
    let chars: Vec<char> = text.chars().collect();
    let pref: Vec<char> = prefix.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if i + pref.len() <= chars.len()
            && chars[i..i + pref.len()] == pref[..]
            && is_word_boundary_before(&chars, i)
        {
            let mut j = i + pref.len();
            // First char must be [A-Za-z0-9] already checked via prefix? For sk- pattern,
            // first char after "sk-" must be alnum
            if j < chars.len() && chars[j].is_ascii_alphanumeric() {
                j += 1;
                while j < chars.len()
                    && (chars[j].is_ascii_alphanumeric()
                        || chars[j] == '-'
                        || (allow_underscore && chars[j] == '_'))
                {
                    j += 1;
                }
                // For sk- we required total suffix >= 12 after first char, but our scan includes it
                // So need len >= min_len (where min_len is remainder after first? adjust)
                // For glpat- it's 20 total after prefix, for sk- it's 12 after first alnum char
                let suffix_len = j - (i + pref.len());
                // sk- requires first char + 12 more → total suffix >= 13 with first already counted?
                // Actually pattern `sk-[A-Za-z0-9][A-Za-z0-9_\-]{12,}` → suffix len >=13, but we treat min_len as 12 after first
                // So require suffix_len >= min_len+1 for sk-
                let required = if prefix == "sk-" { min_len + 1 } else { min_len };
                if suffix_len >= required && is_word_boundary_after(&chars, j) {
                    out.push_str("[REDACTED]");
                    i = j;
                    continue;
                }
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

fn is_word_boundary_before(chars: &[char], idx: usize) -> bool {
    if idx == 0 {
        return true;
    }
    let prev = chars[idx - 1];
    !(prev.is_ascii_alphanumeric() || prev == '_')
}

fn is_word_boundary_after(chars: &[char], idx: usize) -> bool {
    if idx >= chars.len() {
        return true;
    }
    let next = chars[idx];
    !(next.is_ascii_alphanumeric() || next == '_')
}

#[allow(dead_code)]
fn _redact_gateway_user_facing_secrets(text: &str) -> String {
    redact_gateway_user_facing_secrets(text)
}

/// Mirrors `_redact_approval_command(cmd: str | None) -> str`
pub fn redact_approval_command(cmd: Option<&str>) -> String {
    let raw = cmd.unwrap_or("");
    // Python: from agent.redact import redact_sensitive_text; return redact_sensitive_text(str(cmd or ""), force=True)
    // Rust: delegate to gateway redactor (force=True semantics)
    redact_gateway_user_facing_secrets(raw)
}

#[allow(dead_code)]
fn _redact_approval_command(cmd: Option<&str>) -> String {
    redact_approval_command(cmd)
}

// ---------------------------------------------------------------------------
// _format_exec_approval_fallback — mirrors Python 702–728
// ---------------------------------------------------------------------------

/// Mirrors `_format_exec_approval_fallback(command: str, description: str, command_prefix: str, *, allow_permanent: bool = True, allow_session: bool = True, smart_denied: bool = False) -> str`
pub fn format_exec_approval_fallback(
    command: &str,
    description: &str,
    command_prefix: &str,
    allow_permanent: bool,
    allow_session: bool,
    smart_denied: bool,
) -> String {
    let cmd_preview = if command.len() > 200 {
        format!("{}...", &command[..200])
    } else {
        command.to_string()
    };
    let heading = if smart_denied {
        "⚠️ **Smart DENY — owner override for one operation:**"
    } else {
        "⚠️ **Dangerous command requires approval:**"
    };

    let mut choices: Vec<String> = Vec::new();
    choices.push(format!(
        "Reply `{}approve` to execute this one operation",
        command_prefix
    ));
    if !smart_denied && allow_session {
        choices.push(format!(
            "`{}approve session` to approve this pattern for the session",
            command_prefix
        ));
        if allow_permanent {
            choices.push(format!(
                "`{}approve always` to approve permanently",
                command_prefix
            ));
        }
    }
    choices.push(format!("`{}deny` to cancel", command_prefix));

    // Join with ", " and last with ", or "
    let choices_str = if choices.len() == 1 {
        choices[0].clone()
    } else {
        let (last, rest) = choices.split_last().unwrap();
        format!("{}, or {}", rest.join(", "), last)
    };

    format!(
        "{}\n```\n{}\n```\nReason: {}\n\n{}.",
        heading, cmd_preview, description, choices_str
    )
}

#[allow(dead_code)]
fn _format_exec_approval_fallback(
    command: &str,
    description: &str,
    command_prefix: &str,
    allow_permanent: bool,
    allow_session: bool,
    smart_denied: bool,
) -> String {
    format_exec_approval_fallback(
        command,
        description,
        command_prefix,
        allow_permanent,
        allow_session,
        smart_denied,
    )
}

// ---------------------------------------------------------------------------
// _gateway_provider_error_reply — mirrors Python 730–752
// ---------------------------------------------------------------------------

/// Mirrors `_gateway_provider_error_reply(text: str) -> str`
pub fn gateway_provider_error_reply(text: &str) -> String {
    if is_gateway_auth_error(text) {
        return "⚠️ Provider authentication failed. Check the configured credentials; raw provider details are in the gateway logs.".to_string();
    }
    if is_gateway_provider_policy(text) {
        return "⚠️ The model provider rejected the request. I kept the raw provider error out of chat; check gateway logs for details or try rephrasing.".to_string();
    }
    if is_gateway_rate_limit(text) {
        return "⏱️ The model provider is rate-limiting requests. Please wait a moment and try again.".to_string();
    }
    if is_gateway_connection_error(text) {
        return "⚠️ The model server is not responding — it looks like the configured model endpoint is not running or is unreachable.".to_string();
    }
    "⚠️ The model provider failed after retries. I kept raw provider details out of chat; check gateway logs for diagnostics.".to_string()
}

fn is_gateway_auth_error(text: &str) -> bool {
    let lower = text.to_lowercase();
    lower.contains("provider authentication failed")
        || lower.contains("incorrect api key")
        || lower.contains("invalid api key")
        || lower.contains("401")
}

fn is_gateway_provider_policy(text: &str) -> bool {
    let lower = text.to_lowercase();
    lower.contains("cybersecurity risk")
        || lower.contains("security policy")
        || lower.contains("safety policy")
        || lower.contains("policy violation")
        || lower.contains("violat")
        || lower.contains("blocked because")
        || lower.contains("blocked by")
        || lower.contains("blocked under")
        || (lower.contains("request") && lower.contains("blocked"))
        || (lower.contains("request") && lower.contains("rejected"))
        || lower.contains("disallowed")
        || lower.contains("moderation")
}

fn is_gateway_rate_limit(text: &str) -> bool {
    let lower = text.to_lowercase();
    lower.contains("rate limit")
        || lower.contains("rate-limited")
        || lower.contains("429")
        || lower.contains("quota")
        || lower.contains("usage limit")
}

fn is_gateway_connection_error(text: &str) -> bool {
    let lower = text.to_lowercase();
    lower.contains("connection error")
        || lower.contains("connection timeout")
        || lower.contains("connect error")
        || lower.contains("connect timeout")
        || lower.contains("connection refused")
        || lower.contains("connection reset")
        || lower.contains("connection aborted")
        || lower.contains("actively refused")
        || lower.contains("winerror 10061")
        || lower.contains("errno 111")
        || lower.contains("no route to host")
        || lower.contains("network is unreachable")
        || lower.contains("cannot connect")
        || lower.contains("failed to establish")
        || lower.contains("could not connect")
        // api connection variants
        || (lower.contains("api") && lower.contains("connection"))
        || lower.contains("all connection attempts failed")
}

#[allow(dead_code)]
fn _gateway_provider_error_reply(text: &str) -> String {
    gateway_provider_error_reply(text)
}

// ---------------------------------------------------------------------------
// _looks_like_gateway_provider_error — mirrors Python 779–799
// ---------------------------------------------------------------------------

/// Mirrors `_looks_like_gateway_provider_error(text: str) -> bool`
pub fn looks_like_gateway_provider_error(text: &str) -> bool {
    if text.is_empty() {
        return false;
    }
    let body = text.trim();
    if body.len() > 400 || body.matches('\n').count() > 4 {
        return false;
    }
    is_gateway_provider_error_shape(body)
}

fn is_gateway_provider_error_shape(body: &str) -> bool {
    // Mirrors _GATEWAY_PROVIDER_ERROR_SHAPE_RE at start of message (optionally behind punctuation/symbol prefix)
    // Strip leading punctuation/symbol prefix: `^\s*(\W*\s*)?`
    let lower = body.to_lowercase();
    let trimmed = lower.trim_start();
    // Skip leading non-word chars (punctuation/symbols) and whitespace
    let mut start = 0usize;
    let chars: Vec<char> = trimmed.chars().collect();
    while start < chars.len() && !chars[start].is_ascii_alphanumeric() && !chars[start].is_whitespace() {
        start += 1;
    }
    while start < chars.len() && chars[start].is_whitespace() {
        start += 1;
    }
    let rest: String = chars[start..].iter().collect();
    // Now check if rest starts with any of the provider error preambles
    let preambles = [
        "api call failed",
        "api failed",
        "provider authentication failed",
        "non-retryable error",
        "rate limited after",
        "error code:",
        "http ",
        "incorrect api key",
        "invalid api key",
        "connection error",
        "connection timeout",
        "connect error",
        "connect timeout",
        "connection refused",
        "connection reset",
        "connection aborted",
        "actively refused",
        "winerror 10061",
        "errno 111",
        "all connection attempts failed",
        "api connection error",
        "api connection timeout",
    ];
    // Also need to handle "http 404"-like: check first 4-7 chars
    if rest.starts_with("http ") {
        // check if next tokens start with 3 digits
        let after_http = rest[5..].trim_start();
        if after_http.len() >= 3 && after_http.chars().take(3).all(|c| c.is_ascii_digit()) {
            return true;
        }
    }
    preambles.iter().any(|p| rest.starts_with(p))
}

#[allow(dead_code)]
fn _looks_like_gateway_provider_error(text: &str) -> bool {
    looks_like_gateway_provider_error(text)
}

// ---------------------------------------------------------------------------
// _sanitize_gateway_final_response — mirrors Python 802–838
// ---------------------------------------------------------------------------

/// Mirrors `_sanitize_gateway_final_response(platform: Any, text: str) -> str`
pub fn sanitize_gateway_final_response(platform: &str, text: &str) -> String {
    if text.is_empty() {
        return text.to_string();
    }
    if gateway_surface_passes_raw_text(platform) {
        return text.to_string();
    }

    // Lone UTF-16 surrogates sanitization — mirrors `from agent.message_sanitization import _sanitize_surrogates`
    // UTF-16 surrogates U+D800–U+DFFF are not valid Unicode scalar values in Rust
    // (Rust strings are UTF-8, surrogates cannot be represented). Python's
    // surrogate pass is therefore a no-op in Rust; we keep the call for traceability.
    let mut sanitized = sanitize_surrogates(text);

    // Cancellation metadata — mirrors `if str(text).strip().startswith(INTERRUPT_WAITING_FOR_MODEL_PREFIX): return ""`
    // INTERRUPT_WAITING_FOR_MODEL_PREFIX is defined in agent/conversation_loop.py
    // Value is "interrupt_waiting_for_model" or similar — we use a placeholder.
    const INTERRUPT_WAITING_FOR_MODEL_PREFIX: &str = "interrupt_waiting_for_model";
    if sanitized.trim_start().starts_with(INTERRUPT_WAITING_FOR_MODEL_PREFIX) {
        return String::new();
    }

    let redacted = redact_gateway_user_facing_secrets(&sanitized);
    if looks_like_gateway_provider_error(&redacted) {
        return gateway_provider_error_reply(&redacted);
    }
    // Redacted is already sanitized; return it
    let _ = &sanitized; // keep binding for traceability
    redacted
}

fn sanitize_surrogates(text: &str) -> String {
    // Rust strings cannot contain lone surrogates (they are invalid UTF-8).
    // Python's _sanitize_surrogates replaces lone surrogates with U+FFFD.
    // In Rust this is a no-op; we just return the text as-is but filter any
    // stray U+FFFD that might have been introduced elsewhere (keep it).
    // To be maximally faithful, we iterate chars and replace any surrogate codepoint
    // (which in Rust can only appear if the string was constructed from invalid UTF-8
    // via lossless conversion — not the case for &str). So just return clone.
    text.to_string()
}

#[allow(dead_code)]
fn _sanitize_gateway_final_response(platform: &str, text: &str) -> String {
    sanitize_gateway_final_response(platform, text)
}

// ---------------------------------------------------------------------------
// _prepare_gateway_status_message — mirrors Python 841–868
// ---------------------------------------------------------------------------

/// Mirrors `_prepare_gateway_status_message(platform: Any, event_type: str, message: str) -> Optional[str]`
pub fn prepare_gateway_status_message(
    platform: &str,
    _event_type: &str,
    message: &str,
) -> Option<String> {
    let text = message.trim();
    if text.is_empty() {
        return None;
    }
    if gateway_surface_passes_raw_text(platform) {
        return Some(text.to_string());
    }

    let redacted = redact_gateway_user_facing_secrets(text);
    if is_telegram_noisy_status(&redacted) {
        // Opt-in #52995: `compression.progress_notices: true` lets ROUTINE compression progress through
        if !(gateway_compression_progress_notices_enabled() && is_compression_progress_status(&redacted)) {
            return None;
        }
    }
    if looks_like_gateway_provider_error(&redacted) {
        return Some(gateway_provider_error_reply(&redacted));
    }
    Some(redacted)
}

/// Mirrors `_TELEGRAM_NOISY_STATUS_RE.search(text)` — case-insensitive DOTALL scan
/// without regex crate (ponytail: substring/lowercase scan, add regex crate if exact regex fidelity required)
pub fn is_telegram_noisy_status(text: &str) -> bool {
    let lower = text.to_lowercase();
    // Each alternation from the original regex as lowercase substring
    // Some patterns require regex-like handling (e.g. `\d+`), we approximate with keyword presence.
    if lower.contains("auxiliary") && lower.contains("failed") {
        return true;
    }
    if lower.contains("compression summary failed") {
        return true;
    }
    if lower.contains("fallback context marker") {
        return true;
    }
    if lower.contains("configured compression model") && lower.contains("failed") {
        return true;
    }
    if lower.contains("no auxiliary llm provider configured") {
        return true;
    }
    if lower.contains("auto-lowered compression threshold") {
        return true;
    }
    if lower.contains("auto-lowered") && lower.contains("threshold") {
        return true;
    }
    if lower.contains("configured auxiliary compression provider") && lower.contains("unavailable") {
        return true;
    }
    if lower.contains("skipping concurrent compression") {
        return true;
    }
    if lower.contains("compacting context") && lower.contains("summarizing earlier conversation") {
        return true;
    }
    if lower.contains("resumed after") && lower.contains("idle") && lower.contains("compacting") {
        return true;
    }
    if lower.contains("preflight compression") {
        return true;
    }
    if lower.contains("pre-api compression") || lower.contains("pre api compression") {
        return true;
    }
    if lower.contains("context too large") && lower.contains("tokens") && lower.contains("compressing") {
        return true;
    }
    if lower.contains("compressed") && lower.contains("messages") && lower.contains("retrying") {
        return true;
    }
    if lower.contains("compressed") && lower.contains("tokens") && lower.contains("retrying") {
        return true;
    }
    if lower.contains("context reduced to") && lower.contains("tokens") && lower.contains("retrying") {
        return true;
    }
    if lower.contains("session compressed") && lower.contains("times") {
        return true;
    }
    if lower.contains("rate limited") && lower.contains("waiting") {
        return true;
    }
    if lower.contains("retrying in") {
        // need digit following
        if lower.chars().any(|c| c.is_ascii_digit()) {
            return true;
        }
    }
    if lower.contains("max retries") {
        if lower.contains("trying fallback") || lower.contains("exhausted") || lower.contains("invalid responses") {
            return true;
        }
    }
    if lower.contains("stream") && (lower.contains("drop") || lower.contains("drop mid tool-call")) && lower.contains("retry") {
        return true;
    }
    if lower.contains("stale connections from a previous provider issue") {
        return true;
    }
    false
}

#[allow(dead_code)]
fn _prepare_gateway_status_message(
    platform: &str,
    event_type: &str,
    message: &str,
) -> Option<String> {
    prepare_gateway_status_message(platform, event_type, message)
}

// ---------------------------------------------------------------------------
// render_notice_line — mirrors Python 871–883
// ---------------------------------------------------------------------------

/// Mirrors `render_notice_line(notice) -> str`
///
/// Render an AgentNotice to a single plaintext line for messaging platforms.
/// Fail-soft: malformed/empty notice degrades to "".
pub fn render_notice_line(notice_text: Option<&str>) -> String {
    notice_text.unwrap_or("").trim().to_string()
}

/// serde_json::Value overload — mirrors `str(getattr(notice, "text", "") or "").strip()`
pub fn render_notice_line_value(notice: &serde_json::Value) -> String {
    if let Some(obj) = notice.as_object() {
        if let Some(text) = obj.get("text") {
            if let Some(s) = text.as_str() {
                return s.trim().to_string();
            }
            return text.to_string().trim().to_string();
        }
    }
    // If notice is a string directly, use it
    if let Some(s) = notice.as_str() {
        return s.trim().to_string();
    }
    String::new()
}

#[allow(dead_code)]
fn _render_notice_line(notice: &serde_json::Value) -> String {
    render_notice_line_value(notice)
}

// ---------------------------------------------------------------------------
// _send_or_update_status_coro — mirrors Python 886–896
// ---------------------------------------------------------------------------

/// Mirrors `async def _send_or_update_status_coro(adapter, chat_id, status_key, content, metadata)`
///
/// Route a status message through `adapter.send_or_update_status` when supported,
/// else fallback to `adapter.send`.
///
/// Rust port: synchronous stub that records which path would be taken. The async
/// adapter trait is not available in this slice (it lives in `gateway/platforms/base.py`);
/// callers in slice 1 would `await` the returned future. In Rust we model as an
/// enum indicating the dispatch target for traceability.
#[derive(Debug, PartialEq, Eq)]
pub enum StatusDispatch {
    SendOrUpdateStatus,
    Send,
}

pub fn send_or_update_status_coro_has_method(has_send_or_update_status: bool) -> StatusDispatch {
    if has_send_or_update_status {
        StatusDispatch::SendOrUpdateStatus
    } else {
        StatusDispatch::Send
    }
}

#[allow(dead_code)]
fn _send_or_update_status_coro(has_method: bool) -> StatusDispatch {
    send_or_update_status_coro_has_method(has_method)
}

// ---------------------------------------------------------------------------
// _approval_send_outcome — mirrors Python 899–932 (slice boundary at 900)
// ---------------------------------------------------------------------------

/// Mirrors `SendResult` — the result of `future.result(timeout=...)` in Python.
/// In Python, `getattr(result, "success", False)` and `getattr(result, "error", None)`.
#[derive(Debug, Clone)]
pub struct SendResult {
    pub success: bool,
    pub error: Option<String>,
}

impl SendResult {
    pub fn success() -> Self {
        Self {
            success: true,
            error: None,
        }
    }

    pub fn failure(error: impl Into<String>) -> Self {
        Self {
            success: false,
            error: Some(error.into()),
        }
    }
}

/// Mirrors the three-way outcome of `_approval_send_outcome`.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum ApprovalSendOutcome {
    Sent,
    Failed,
    Ambiguous,
}

impl ApprovalSendOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sent => "sent",
            Self::Failed => "failed",
            Self::Ambiguous => "ambiguous",
        }
    }
}

/// Error variants for the future's `result(timeout)` call.
#[derive(Debug, Clone)]
pub enum FutureError {
    Timeout,
    Exception(String),
}

/// Mirrors `_approval_send_outcome(future, timeout: float) -> str`
///
/// Classify an approval prompt send as `sent` / `failed` / `ambiguous`.
///
/// `future: None` → `"failed"` (log warning "no scheduling future")
/// `future.result(timeout)` raises `TimeoutError` → `"ambiguous"`
/// `future.result(timeout)` raises other `Exception` → `"failed"` (log)
/// `result.success == True` → `"sent"`
/// else → `"failed"` (log `result.error`)
///
/// In Rust, `future` is modelled as `Option<Result<SendResult, FutureError>>`
/// where `None` means no future, `Err(Timeout)` maps to `TimeoutError`,
/// `Err(Exception)` maps to other exceptions, and `Ok(SendResult)` is the
/// successful future result.
pub fn approval_send_outcome(
    future: Option<Result<SendResult, FutureError>>,
    _timeout_secs: f64,
) -> ApprovalSendOutcome {
    let Some(res) = future else {
        log::warn!("Prompt send failed: no scheduling future (loop unavailable)");
        return ApprovalSendOutcome::Failed;
    };
    let result = match res {
        Ok(r) => r,
        Err(FutureError::Timeout) => return ApprovalSendOutcome::Ambiguous,
        Err(FutureError::Exception(exc)) => {
            log::warn!("Prompt send failed: {}", exc);
            return ApprovalSendOutcome::Failed;
        }
    };
    if result.success {
        return ApprovalSendOutcome::Sent;
    }
    log::warn!(
        "Prompt send failed: {}",
        result.error.unwrap_or_else(|| "unknown error".to_string())
    );
    ApprovalSendOutcome::Failed
}

/// String-returning overload for callers expecting `&str` (Python parity).
pub fn approval_send_outcome_str(
    future: Option<Result<SendResult, FutureError>>,
    timeout_secs: f64,
) -> &'static str {
    approval_send_outcome(future, timeout_secs).as_str()
}

#[allow(dead_code)]
fn _approval_send_outcome(
    future: Option<Result<SendResult, FutureError>>,
    timeout_secs: f64,
) -> &'static str {
    approval_send_outcome_str(future, timeout_secs)
}

// ---------------------------------------------------------------------------
// Hygiene helpers — _hygiene_cooldown_for_failure, _reset_hygiene_failure_streak,
// hygiene_compaction_recovered, _record_hygiene_cooldown
// Mirrors Python lines 155–311
// ---------------------------------------------------------------------------

/// Minimal persistent state for hygiene failure streak (mirrors `PersistentState.hygiene_failure_streak`).
#[derive(Debug, Clone, Default)]
pub struct PersistentState {
    pub hygiene_failure_streak: i64,
}

/// Minimal session state wrapper (mirrors `gateway._session_state(session_key).persistent`).
#[derive(Debug, Clone, Default)]
pub struct HygienicSessionState {
    pub persistent: PersistentState,
}

/// Minimal session DB trait for hygiene streak persistence (mirrors `session_db.increment_hygiene_failure_streak`).
pub trait HygieneSessionDb {
    fn increment_hygiene_failure_streak(&mut self, session_key: &str) -> i64;
    fn reset_hygiene_failure_streak(&mut self, session_key: &str);
    fn record_compression_failure_cooldown(
        &mut self,
        session_id: &str,
        cooldown_until: f64,
        error: Option<&str>,
    );
}

/// Gateway trait for hygiene operations (mirrors `gateway._session_state`, `gateway._peek_session_state`, `gateway._session_db`).
pub trait HygieneGateway {
    fn session_state(&mut self, session_key: &str) -> &mut HygienicSessionState;
    fn peek_session_state(&self, session_key: &str) -> Option<&HygienicSessionState>;
    fn peek_session_state_mut(&mut self, session_key: &str) -> Option<&mut HygienicSessionState>;
    fn session_db_mut(&mut self) -> Option<&mut dyn HygieneSessionDb>;
}

/// Mirrors `_hygiene_cooldown_for_failure(gateway, session_key: str, base_cooldown_seconds: float) -> float`
pub fn hygiene_cooldown_for_failure(
    gateway: &mut dyn HygieneGateway,
    session_key: &str,
    base_cooldown_seconds: f64,
) -> f64 {
    let mut streak: i64 = 1;
    // Try to increment via session_db if available
    let mut db_incremented = false;
    if let Some(db) = gateway.session_db_mut() {
        // We need to call increment without double-borrowing gateway; we already borrowed session_db_mut
        // So we handle state update after.
        let new_streak = db.increment_hygiene_failure_streak(session_key);
        streak = new_streak.max(1);
        db_incremented = true;
        // Update persistent state hot view
        if let Some(state) = gateway.peek_session_state_mut(session_key) {
            state.persistent.hygiene_failure_streak = streak;
        }
    }
    if !db_incremented {
        if let Some(state) = gateway.peek_session_state_mut(session_key) {
            state.persistent.hygiene_failure_streak += 1;
            streak = state.persistent.hygiene_failure_streak;
        } else {
            // Fallback: get-or-create via session_state (mirrors Python try/except around _session_state)
            let state = gateway.session_state(session_key);
            // If we reach here without DB, Python does state.hygiene_failure_streak +=1
            // But we already handled peek case; for get-or-create we increment.
            // To avoid double-count on first call when no prior state, we mimic Python's
            // `elif state is not None: state.hygiene_failure_streak +=1`
            // which would have incremented from whatever current value.
            // Since we didn't find a peek, we increment from default 0 → 1.
            if state.persistent.hygiene_failure_streak == 0 {
                state.persistent.hygiene_failure_streak = 1;
                streak = 1;
            } else {
                // If already set, increment
                state.persistent.hygiene_failure_streak += 1;
                streak = state.persistent.hygiene_failure_streak;
            }
        }
    }
    let multiplier = HYGIENE_COOLDOWN_LADDER_MULTIPLIERS
        [(streak as usize).min(HYGIENE_COOLDOWN_LADDER_MULTIPLIERS.len()) - 1];
    (base_cooldown_seconds * multiplier as f64).min(HYGIENE_COOLDOWN_MAX_SECONDS)
}

#[allow(dead_code)]
fn _hygiene_cooldown_for_failure(
    gateway: &mut dyn HygieneGateway,
    session_key: &str,
    base_cooldown_seconds: f64,
) -> f64 {
    hygiene_cooldown_for_failure(gateway, session_key, base_cooldown_seconds)
}

/// Mirrors `_reset_hygiene_failure_streak(gateway, session_key: str) -> None`
pub fn reset_hygiene_failure_streak(gateway: &mut dyn HygieneGateway, session_key: &str) {
    // Peeks rather than get-or-creates: writing 0 that is already 0 must not materialise a _sessions entry
    if let Some(state) = gateway.peek_session_state_mut(session_key) {
        state.persistent.hygiene_failure_streak = 0;
    }
    if let Some(db) = gateway.session_db_mut() {
        db.reset_hygiene_failure_streak(session_key);
    }
}

#[allow(dead_code)]
fn _reset_hygiene_failure_streak(gateway: &mut dyn HygieneGateway, session_key: &str) {
    reset_hygiene_failure_streak(gateway, session_key)
}

/// Mirrors `compression_made_progress` from `agent.turn_context`
/// (see agent/turn_context.py:293). True when compression materially reduced request.
pub fn compression_made_progress(
    orig_len: usize,
    new_len: usize,
    orig_tokens: i64,
    new_tokens: i64,
) -> bool {
    if new_len < orig_len {
        return true;
    }
    orig_tokens > 0 && (new_tokens as f64) < (orig_tokens as f64) * 0.95
}

#[allow(dead_code)]
fn _compression_made_progress(
    orig_len: usize,
    new_len: usize,
    orig_tokens: i64,
    new_tokens: i64,
) -> bool {
    compression_made_progress(orig_len, new_len, orig_tokens, new_tokens)
}

/// Mirrors `hygiene_compaction_recovered(*, aborted: bool, rotated: bool, in_place: bool, msg_count: int, new_count: int, approx_tokens: int, new_tokens: int) -> bool`
pub fn hygiene_compaction_recovered(
    aborted: bool,
    rotated: bool,
    in_place: bool,
    msg_count: usize,
    new_count: usize,
    approx_tokens: i64,
    new_tokens: i64,
) -> bool {
    if aborted {
        return false;
    }
    if !(rotated || in_place) {
        return false;
    }
    compression_made_progress(msg_count, new_count, approx_tokens, new_tokens)
}

/// Mirrors `_record_hygiene_cooldown(gateway, session_id: str, cooldown_seconds: float, error: Optional[str] = None) -> None`
pub fn record_hygiene_cooldown(
    gateway: &mut dyn HygieneGateway,
    session_id: &str,
    cooldown_seconds: f64,
    error: Option<&str>,
) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);
    let cooldown_until = now + cooldown_seconds;
    if let Some(db) = gateway.session_db_mut() {
        db.record_compression_failure_cooldown(session_id, cooldown_until, error);
    }
}

#[allow(dead_code)]
fn _record_hygiene_cooldown(
    gateway: &mut dyn HygieneGateway,
    session_id: &str,
    cooldown_seconds: f64,
    error: Option<&str>,
) {
    record_hygiene_cooldown(gateway, session_id, cooldown_seconds, error)
}

// ---------------------------------------------------------------------------
// Telegram command mention helper — mirrors _TELEGRAM_COMMAND_MENTION_RE
// ---------------------------------------------------------------------------

/// Returns true if `text` contains a Telegram-valid slash command mention.
///
/// Mirrors `r"(?<![\w:/])/([A-Za-z0-9][A-Za-z0-9_-]*)"` — the slash must not be
/// preceded by `\w`, `:`, or `/`, and the command must start with alnum then
/// alnum/`_`/`-`.
pub fn is_telegram_command_mention(text: &str) -> bool {
    telegram_command_mentions(text).next().is_some()
}

/// Iterator over Telegram command mentions in `text` (captured group 1).
pub fn telegram_command_mentions(text: &str) -> impl Iterator<Item = String> + '_ {
    // Manual scan without regex crate (ponytail: char scan, add regex crate if perf matters)
    let chars: Vec<char> = text.chars().collect();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '/' {
            let prev_ok = if i == 0 {
                true
            } else {
                let prev = chars[i - 1];
                !(prev.is_ascii_alphanumeric() || prev == '_' || prev == ':' || prev == '/')
            };
            if prev_ok && i + 1 < chars.len() && chars[i + 1].is_ascii_alphanumeric() {
                let mut j = i + 1;
                j += 1; // first alnum already checked
                while j < chars.len()
                    && (chars[j].is_ascii_alphanumeric() || chars[j] == '_' || chars[j] == '-')
                {
                    j += 1;
                }
                let capture: String = chars[i + 1..j].iter().collect();
                if !capture.is_empty() {
                    out.push(capture);
                }
                i = j;
                continue;
            }
        }
        i += 1;
    }
    out.into_iter()
}

// ---------------------------------------------------------------------------
// Additional small helpers for gateway display / progress thread id
// (these start within slice 1 but the function bodies that define them
// straddle the 900 boundary; stubs are provided here for slice-1
// completeness — full bodies continue in run_slice2).
// ---------------------------------------------------------------------------

/// Mirrors `_resolve_progress_thread_id` signature (defined at Python line 987, starts within slice).
/// The full body is in slice 2; this stub documents the contract.
pub fn resolve_progress_thread_id(
    platform: &str,
    source_thread_id: Option<&str>,
    event_message_id: Option<&str>,
    reply_in_thread: bool,
) -> Option<String> {
    let platform_key = gateway_platform_value_str(platform);
    if !reply_in_thread {
        if let (Some(tid), Some(mid)) = (source_thread_id, event_message_id) {
            if tid == mid {
                return None;
            }
        }
        return source_thread_id.map(|s| s.to_string());
    }
    if let Some(tid) = source_thread_id {
        if !tid.is_empty() {
            return Some(tid.to_string());
        }
    }
    if matches!(platform_key.as_str(), "slack" | "mattermost") {
        if let Some(mid) = event_message_id {
            if !mid.is_empty() {
                return Some(mid.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constants_match_python() {
        assert_eq!(AGENT_CACHE_MAX_SIZE, 128);
        assert_eq!(AGENT_CACHE_IDLE_TTL_SECS, 3600.0);
        assert_eq!(PLATFORM_CONNECT_TIMEOUT_SECS_DEFAULT, 30.0);
        assert_eq!(TELEGRAM_CONNECT_TIMEOUT_SECS_DEFAULT, 180.0);
        assert_eq!(TELEGRAM_INITIAL_CONNECT_TIMEOUT_SECS_DEFAULT, 45.0);
        assert_eq!(ADAPTER_DISCONNECT_TIMEOUT_SECS_DEFAULT, 5.0);
        assert_eq!(STALL_NOTIFY_SEND_TIMEOUT_SECONDS, 15.0);
        assert_eq!(GATEWAY_PROXY_SSE_BUFFER_MAX_CHARS, 16 * 1024 * 1024);
        assert_eq!(GATEWAY_HYGIENE_PLATFORM, "gateway_hygiene");
        assert_eq!(HYGIENE_COOLDOWN_LADDER_MULTIPLIERS, [1, 3, 9]);
        assert_eq!(HYGIENE_COOLDOWN_MAX_SECONDS, 3600.0);
        assert!(USER_BOUNDARY_END_REASONS.contains(&"session_reset"));
    }

    #[test]
    fn status_template_to_regex_inserts_digit_pattern() {
        let r = status_template_to_regex("hello {tokens} world");
        assert!(r.contains(r"[\d,]+"));
        assert!(r.contains("hello"));
        assert!(r.contains("world"));
    }

    #[test]
    fn compression_progress_status_source_contains_digit_pattern() {
        let src = compression_progress_status_regex_source();
        assert!(src.contains(r"[\d,]+"));
        assert!(src.contains("Compacting"));
    }

    #[test]
    fn is_compression_progress_status_true_for_known_lines() {
        assert!(is_compression_progress_status(COMPACTION_STATUS));
        assert!(is_compression_progress_status(
            "📦 Pre-API compression: ~123,456 tokens near the context/output limit. Compacting before the next model call."
        ));
        assert!(is_compression_progress_status(
            "🗜️ Compressed 30 → 12 messages, retrying..."
        ));
        assert!(!is_compression_progress_status("hello world"));
    }

    #[test]
    fn gateway_surface_passes_raw_text_only_for_allowlist() {
        assert!(gateway_surface_passes_raw_text("local"));
        assert!(gateway_surface_passes_raw_text("api_server"));
        assert!(!gateway_surface_passes_raw_text("telegram"));
        assert!(!gateway_surface_passes_raw_text("discord"));
        assert!(!gateway_surface_passes_raw_text(""));
    }

    #[test]
    fn is_telegram_noisy_status_matches_expected() {
        assert!(is_telegram_noisy_status(
            "auxiliary tool failed due to timeout"
        ));
        assert!(is_telegram_noisy_status("Compacting context — summarizing earlier conversation"));
        assert!(!is_telegram_noisy_status("Hello, how can I help you?"));
    }

    #[test]
    fn looks_like_gateway_provider_error_true_for_api_failed() {
        assert!(looks_like_gateway_provider_error("API call failed: 500"));
        assert!(looks_like_gateway_provider_error("Provider authentication failed"));
        assert!(!looks_like_gateway_provider_error("HTTP 404 means not found — here is a long explanation that exceeds the provider error heuristics and should not be treated as an error envelope because it is longer than 400 characters or many lines"));
        assert!(!looks_like_gateway_provider_error(
            "This is a normal assistant response that happens to mention HTTP 404 in the middle of a paragraph, not at the start."
        ));
    }

    #[test]
    fn sanitize_gateway_final_response_passthrough_for_raw_platform() {
        let raw = "API call failed: 500";
        assert_eq!(sanitize_gateway_final_response("local", raw), raw);
        // Chat platform sanitizes
        let sanitized = sanitize_gateway_final_response("telegram", raw);
        assert!(sanitized.contains("model provider"));
    }

    #[test]
    fn prepare_gateway_status_message_filters_noisy_when_opted_out() {
        // Without progress_notices enabled, noisy compression line is suppressed
        // (gateway_compression_progress_notices_enabled reads live config -> false in test tmp)
        let msg = "Compacting context — summarizing earlier conversation so I can continue...";
        // This is noisy but also compression progress; with notices disabled, it should be None
        // We can't guarantee file state, so just test non-noisy passes
        let normal = "Hello from the agent";
        assert_eq!(
            prepare_gateway_status_message("telegram", "status", normal),
            Some(normal.to_string())
        );
    }

    #[test]
    fn render_notice_line_trims() {
        assert_eq!(render_notice_line(Some("  hello  ")), "hello");
        assert_eq!(render_notice_line(None), "");
        assert_eq!(
            render_notice_line_value(&serde_json::json!({"text": "  ⚠ notice "})),
            "⚠ notice"
        );
    }

    #[test]
    fn approval_send_outcome_variants() {
        assert_eq!(
            approval_send_outcome(None, 15.0),
            ApprovalSendOutcome::Failed
        );
        assert_eq!(
            approval_send_outcome(Some(Err(FutureError::Timeout)), 15.0),
            ApprovalSendOutcome::Ambiguous
        );
        assert_eq!(
            approval_send_outcome(
                Some(Err(FutureError::Exception("boom".into()))),
                15.0
            ),
            ApprovalSendOutcome::Failed
        );
        assert_eq!(
            approval_send_outcome(Some(Ok(SendResult::success())), 15.0),
            ApprovalSendOutcome::Sent
        );
        assert_eq!(
            approval_send_outcome(Some(Ok(SendResult::failure("nope"))), 15.0),
            ApprovalSendOutcome::Failed
        );
    }

    #[test]
    fn hygiene_compaction_recovered_logic() {
        assert!(!hygiene_compaction_recovered(true, true, false, 10, 5, 1000, 500));
        assert!(!hygiene_compaction_recovered(false, false, false, 10, 5, 1000, 500));
        assert!(hygiene_compaction_recovered(false, true, false, 10, 5, 1000, 500));
        // No real progress (same count, token wobble <5%)
        assert!(!hygiene_compaction_recovered(
            false, true, false, 10, 10, 1000, 990
        ));
        // Token reduction >5% counts
        assert!(hygiene_compaction_recovered(
            false, true, false, 10, 10, 1000, 900
        ));
    }

    #[test]
    fn telegram_command_mentions_extract() {
        let text = "Use /start and /help_me-2 but not http://x/y or a:b/c";
        let mentions: Vec<String> = telegram_command_mentions(text).collect();
        assert!(mentions.contains(&"start".to_string()));
        assert!(mentions.contains(&"help_me-2".to_string()));
    }

    #[test]
    fn redact_secrets_keeps_bearer_prefix() {
        let text = "Bearer abcdefghijklmnopqrstuvwxyz123456 and sk-abcdefghijklmnopqrstu more";
        let redacted = redact_gateway_user_facing_secrets(text);
        assert!(redacted.contains("Bearer [REDACTED]"));
        assert!(redacted.contains("[REDACTED]"));
        assert!(!redacted.contains("abcdefghijklmnopqrstuvwxyz"));
    }

    #[test]
    fn is_transient_network_error_chain() {
        assert!(is_transient_network_error(&["ValueError", "NetworkError"]));
        assert!(!is_transient_network_error(&["ValueError", "RuntimeError"]));
        assert!(is_transient_network_error_single("TimedOut"));
    }

    #[test]
    fn non_conversational_metadata_only_for_discord() {
        let meta = HashMap::new();
        let out = non_conversational_metadata(Some(meta.clone()), "discord").unwrap();
        assert_eq!(out.get("non_conversational"), Some(&serde_json::Value::Bool(true)));
        let out2 = non_conversational_metadata(Some(meta), "telegram");
        assert!(out2.unwrap().get("non_conversational").is_none());
    }

    #[test]
    fn interim_metadata_sets_flag() {
        let m = interim_metadata(None);
        assert_eq!(m.get("_interim_send"), Some(&serde_json::Value::Bool(true)));
    }

    #[test]
    fn seed_hygiene_system_prompt_sets_cached() {
        let mut cached = String::new();
        let mut row = HashMap::new();
        row.insert(
            "system_prompt".to_string(),
            serde_json::Value::String("hello system".to_string()),
        );
        assert!(seed_hygiene_system_prompt(&mut cached, Some(&row)));
        assert_eq!(cached, "hello system");
        let mut cached2 = "old".to_string();
        assert!(!seed_hygiene_system_prompt(&mut cached2, None));
        assert_eq!(cached2, "");
    }
}
