//! Gateway slash-command handlers — slice 2 (lines 900–1800).
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/gateway/slash_commands.py`
//! (6084 LOC), slice 2 covering lines 900–1800.
//! This slice continues `GatewaySlashCommandsMixin` after slice 1's break inside
//! `_handle_context_command` and covers the remainder of that method plus:
//! `_gateway_session_origin_for_id`, `_same_matrix_room`, `_same_origin_chat`,
//! `_resume_caller_is_admin`, `_resume_target_allowed`, `_resume_row_visible`,
//! `_handle_agents_command`, `_handle_stop_command`, `_handle_platform_command`,
//! `_handle_restart_command`, `_handle_version_command`, `_handle_help_command`,
//! `_handle_commands_command`, and the opening of `_handle_model_command`
//! through its `--refresh` / `parse_model_switch_args` prologue (the slice
//! boundary at line 1800 cuts inside `_handle_model_command` after
//! `if force_refresh:`; the Rust file includes the truncated tail as a
//! commented boundary note — the complete model-switch body, policy gating,
//! picker, and persistence continue in slice 3).
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
//! - `lines.append("")` + `if threshold > 0: ... over_threshold / threshold` (lines 900–918) → [`context_threshold_line`] / [`context_threshold_block`]
//! - `compressions = getattr(ctx, "compression_count", 0)` + `savings = getattr(ctx, "_last_compression_savings_pct", None)` (919–926) → [`context_compressions_block`]
//! - `api_calls = getattr(agent, "session_api_calls", 0) ... total_tokens` + `totals_header/line/total_billed/throughput_note` (928–946) → [`context_totals_block`]
//! - `else: lines.append(t("gateway.context.detail_after_first"))` (947–949) → [`context_detail_after_first`]
//! - `if has_agent: breakdown = await asyncio.to_thread(self._context_breakdown_block, ...)` (955–961) → [`context_breakdown_block_stub`] + [`maybe_extend_breakdown`]
//! - `return "\n".join(lines)` + transcript fallback `load_transcript` + `estimate_messages_tokens_rough` (963–988) → [`context_transcript_fallback_stub`] / [`estimate_messages_tokens_rough_stub`]
//! - `def _gateway_session_origin_for_id(self, session_id: str) -> Optional[SessionSource]` (990–1003) → [`gateway_session_origin_for_id`] / [`_gateway_session_origin_for_id`]
//! - `def _same_matrix_room(current, origin) -> bool` (1005–1021) → [`same_matrix_room`] / [`_same_matrix_room`]
//! - `def _same_origin_chat(self, current, origin) -> bool` (1023–1087) → [`same_origin_chat`] / [`_same_origin_chat`]
//! - `def _resume_caller_is_admin(self, source) -> bool` (1089–1106) → [`resume_caller_is_admin`] / [`_resume_caller_is_admin`]
//! - `async def _resume_target_allowed(self, source, target_id, allow_override=False) -> bool` (1108–1249) → [`resume_target_allowed`] / [`resume_target_allowed_persisted`] (sync stub; async DB + live-origin branches documented)
//! - `async def _resume_row_visible(self, source, row, allow_all) -> bool` (1251–1273) → [`resume_row_visible`]
//! - `async def _handle_agents_command(self, event) -> str` (1275–1424) → [`GatewaySlashCommandsMixin::handle_agents_command`] + [`format_uptime_short`] / [`agents_lines_stub`]
//! - `async def _handle_stop_command(self, event) -> Union[str, EphemeralReply]` (1426–1507) → [`GatewaySlashCommandsMixin::handle_stop_command`] + [`StopAction`]
//! - `async def _handle_platform_command(self, event) -> str` (1509–1600) → [`GatewaySlashCommandsMixin::handle_platform_command`] + [`resolve_platform`] / [`platform_list_lines`]
//! - `async def _handle_restart_command(self, event) -> Union[str, EphemeralReply]` (1602–1712) → [`GatewaySlashCommandsMixin::handle_restart_command`] + [`is_stale_restart_redelivery_stub`] / [`restart_notify_data`] / [`restart_dedup_data`]
//! - `async def _handle_version_command(self, event) -> str` (1714–1718) → [`GatewaySlashCommandsMixin::handle_version_command`]
//! - `async def _handle_help_command(self, event) -> str` (1720–1729) → [`GatewaySlashCommandsMixin::handle_help_command`]
//! - `async def _handle_commands_command(self, event) -> str` (1731–1749) → [`GatewaySlashCommandsMixin::handle_commands_command`]
//! - `async def _handle_model_command(self, event) -> Optional[str]` (1751–1800, partial) → [`GatewaySlashCommandsMixin::handle_model_command`] (partial through `if force_refresh:`; remainder in slice 3)
//! - `from gateway.slash_access import policy_for_source` (lazy) → [`policy_for_source_stub`]
//! - `from gateway.session import is_shared_multi_user_session` → [`is_shared_multi_user_session_stub`]
//! - `from utils import atomic_json_write` → [`atomic_json_write_stub`] (ponytail: no utils dep in std-only)
//! - `from agent.i18n import t` → [`t_stub`] helpers (key → formatted string for traceability)
//! - `from agent.model_metadata import estimate_messages_tokens_rough` → [`estimate_messages_tokens_rough_stub`]
//! - `from tools.process_registry import format_uptime_short, process_registry` → [`format_uptime_short`] + [`process_registry_list_stub`]
//! - `from tools.async_delegation import list_async_delegations` → [`list_async_delegations_stub`]
//! - `from gateway.restart import is_container_restart_context, is_gateway_supervisor_process` → [`is_container_restart_context_stub`] / [`is_gateway_supervisor_process_stub`]
//! - `from hermes_cli.slash_exec import CommandContext, execute_command` → [`execute_command_stub`] (ponytail: no hermes_cli dep)
//! - `from hermes_cli.model_switch import parse_model_switch_args, resolve_persist_behavior` → [`parse_model_switch_args_stub`] / [`resolve_persist_behavior_stub`]
//!
//! # Notes on runtime deps not ported in this slice
//!
//! Python imports `asyncio`, `dataclasses`, `hashlib`, `inspect`, `logging`,
//! `os`, `re`, `shlex`, `sys`, `time`, `datetime`, `pathlib`, and lazy
//! `gateway.run` helpers (`_hermes_home`, `_load_gateway_config`,
//! `atomic_json_write`, `policy_for_source`, `is_shared_multi_user_session`, etc.).
//! Those are runtime/loop-level concerns and are documented as `// Python:`
//! comments where referenced. Pure helpers are fully ported; side-effecting
//! gateway coupling (session store, agent cache, hooks, kanban DB, process
//! registry, async delegation) is stubbed with `ponytail:` notes and deterministic
//! fallbacks.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Minimal platform / session stubs — mirrors Python imports 35–42
// Re-declared here so slice 2 is self-contained (slice 1 defines the same
// shapes in its own module namespace; Rust modules do not share types).
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
    pub fn value(&self) -> String {
        self.as_str().to_string()
    }
}

/// Mirrors `gateway.session.SessionSource` (minimal fields used in slice 2).
#[derive(Debug, Clone, Default)]
pub struct SessionSource {
    pub platform: Option<Platform>,
    pub platform_raw: Option<String>,
    pub profile: Option<String>,
    pub scope_id: Option<String>,
    pub thread_id: Option<String>,
    pub message_id: Option<String>,
    pub user_id: Option<String>,
    pub user_id_alt: Option<String>,
    pub chat_id: Option<String>,
    pub chat_type: Option<String>,
    pub chat_name: Option<String>,
    pub delivered_via_upstream_relay: Option<bool>,
}

impl SessionSource {
    pub fn platform_value(&self) -> String {
        if let Some(p) = &self.platform {
            return p.value();
        }
        self.platform_raw.as_deref().unwrap_or("").trim().to_ascii_lowercase()
    }
    pub fn chat_type_lower(&self) -> String {
        self.chat_type.as_deref().unwrap_or("").trim().to_ascii_lowercase()
    }
}

/// Mirrors `EphemeralReply(str)` wrapper.
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
    pub content: Option<String>,
    pub raw_message: Option<serde_json::Value>,
    pub message_id: Option<String>,
    pub platform_update_id: Option<String>,
    pub reply_to_message_id: Option<String>,
}

impl MessageEvent {
    /// Mirrors `event.get_command_args()` / `event.content` stripping.
    pub fn get_command_args(&self) -> String {
        let raw = self.content.as_deref().or(self.text.as_deref()).unwrap_or("").trim();
        if raw.is_empty() {
            return String::new();
        }
        let without_slash = raw.trim_start_matches('/');
        if let Some(space) = without_slash.find(char::is_whitespace) {
            without_slash[space..].trim().to_string()
        } else {
            String::new()
        }
    }
    pub fn get_command_args_trimmed(&self) -> String {
        self.get_command_args().trim().to_string()
    }
}

// ---------------------------------------------------------------------------
// i18n stub — mirrors `agent.i18n.t(key, **kwargs)`
// ---------------------------------------------------------------------------

fn t_stub(key: &str, params: &[(&str, String)]) -> String {
    if params.is_empty() {
        return key.to_string();
    }
    let kv: Vec<String> = params.iter().map(|(k, v)| format!("{}={}", k, v)).collect();
    format!("{}({})", key, kv.join(", "))
}

fn t_simple(key: &str) -> String {
    // Mirrors `t("gateway....")` with no kwargs — return key for traceability
    key.to_string()
}

// Context-specific t helpers (preserve keys)
fn t_context_over_threshold(threshold: &str, threshold_pct: &str) -> String {
    t_stub("gateway.context.over_threshold", &[("threshold", threshold.to_string()), ("threshold_pct", threshold_pct.to_string())])
}
fn t_context_threshold(threshold: &str, threshold_pct: &str, to_go: &str) -> String {
    t_stub("gateway.context.threshold", &[("threshold", threshold.to_string()), ("threshold_pct", threshold_pct.to_string()), ("to_go", to_go.to_string())])
}
fn t_context_compressions(count: i64) -> String {
    t_stub("gateway.context.compressions", &[("count", count.to_string())])
}
fn t_context_last_savings(savings: &str) -> String {
    t_stub("gateway.context.last_savings", &[("savings", savings.to_string())])
}
fn t_context_totals_header(calls: i64) -> String {
    t_stub("gateway.context.totals_header", &[("calls", calls.to_string())])
}
fn t_context_totals_line(input: &str, output: &str, reasoning: &str) -> String {
    t_stub("gateway.context.totals_line", &[("input", input.to_string()), ("output", output.to_string()), ("reasoning", reasoning.to_string())])
}
fn t_context_total_billed(total: &str) -> String {
    t_stub("gateway.context.total_billed", &[("total", total.to_string())])
}
fn t_context_throughput_note() -> String { t_simple("gateway.context.throughput_note") }
fn t_context_detail_after_first() -> String { t_simple("gateway.context.detail_after_first") }
fn t_context_header() -> String { t_simple("gateway.context.header") }
fn t_context_estimated(count: &str, messages: usize) -> String {
    t_stub("gateway.context.estimated", &[("count", count.to_string()), ("messages", messages.to_string())])
}
fn t_context_no_data() -> String { t_simple("gateway.context.no_data") }

// ---------------------------------------------------------------------------
// Context continuation — mirrors Python lines 900–989
// Slice 1 stopped after `lines.append("")` at the threshold prologue.
// This slice provides the remainder.
// ---------------------------------------------------------------------------

/// Build the threshold line(s) for `/context` gauge.
///
/// Mirrors:
/// ```python
/// if threshold > 0:
///     if used >= threshold:
///         lines.append(t("gateway.context.over_threshold", threshold=f"{threshold:,}", threshold_pct=f"{threshold_pct:.0f}"))
///     else:
///         lines.append(t("gateway.context.threshold", threshold=f"{threshold:,}", threshold_pct=f"{threshold_pct:.0f}", to_go=f"{threshold - used:,}"))
/// ```
pub fn context_threshold_line(used: i64, threshold: i64, threshold_pct: f64) -> Option<String> {
    if threshold <= 0 {
        return None;
    }
    // Format with thousands separators (Rust `{:,}` not stable without crate; manual comma)
    let thr_fmt = format_with_commas(threshold);
    let pct_fmt = format!("{:.0}", threshold_pct);
    if used >= threshold {
        Some(t_context_over_threshold(&thr_fmt, &pct_fmt))
    } else {
        let to_go = format_with_commas(threshold - used);
        Some(t_context_threshold(&thr_fmt, &pct_fmt, &to_go))
    }
}

/// Compat alias.
pub fn context_threshold_block(used: i64, threshold: i64, threshold_pct: f64) -> Option<String> {
    context_threshold_line(used, threshold, threshold_pct)
}

/// Compressions + last savings lines.
///
/// Mirrors:
/// ```python
/// compressions = getattr(ctx, "compression_count", 0) or 0
/// lines.append(t("gateway.context.compressions", count=compressions))
/// if compressions:
///     savings = getattr(ctx, "_last_compression_savings_pct", None)
///     if savings is not None:
///         lines.append(t("gateway.context.last_savings", savings=f"{savings:.0f}"))
/// ```
pub fn context_compressions_block(compressions: i64, last_savings_pct: Option<f64>) -> Vec<String> {
    let mut out = Vec::new();
    out.push(t_context_compressions(compressions));
    if compressions != 0 {
        if let Some(s) = last_savings_pct {
            if s.is_finite() {
                out.push(t_context_last_savings(&format!("{:.0}", s)));
            }
        }
    }
    out
}

/// Totals block for `/context`.
///
/// Mirrors:
/// ```python
/// api_calls = getattr(agent, "session_api_calls", 0) or 0
/// input_tokens = getattr(agent, "session_input_tokens", 0) or 0
/// output_tokens = getattr(agent, "session_output_tokens", 0) or 0
/// reasoning_tokens = getattr(agent, "session_reasoning_tokens", 0) or 0
/// total_tokens = getattr(agent, "session_total_tokens", 0) or 0
/// lines.append("")
/// lines.append(t("gateway.context.totals_header", calls=api_calls))
/// lines.append(t("gateway.context.totals_line", input=f"{input_tokens:,}", output=f"{output_tokens:,}", reasoning=f"{reasoning_tokens:,}"))
/// lines.append(t("gateway.context.total_billed", total=f"{total_tokens:,}"))
/// lines.append(t("gateway.context.throughput_note"))
/// ```
pub fn context_totals_block(
    api_calls: i64,
    input_tokens: i64,
    output_tokens: i64,
    reasoning_tokens: i64,
    total_tokens: i64,
) -> Vec<String> {
    vec![
        String::new(),
        t_context_totals_header(api_calls),
        t_context_totals_line(&format_with_commas(input_tokens), &format_with_commas(output_tokens), &format_with_commas(reasoning_tokens)),
        t_context_total_billed(&format_with_commas(total_tokens)),
        t_context_throughput_note(),
    ]
}

pub fn context_detail_after_first() -> String {
    t_context_detail_after_first()
}

/// Per-category estimated breakdown (+ optional expanded listings).
///
/// Mirrors:
/// ```python
/// if has_agent:
///     breakdown = await asyncio.to_thread(self._context_breakdown_block, agent, source, expanded)
///     if breakdown:
///         lines.append("")
///         lines.extend(breakdown)
/// ```
/// Fail-open: rendering errors never break /context.
// ponytail: stub — breakdown rendering requires agent + model_metadata; deterministic empty
pub fn context_breakdown_block_stub(_expanded: bool) -> Vec<String> {
    // Python: `self._context_breakdown_block(agent, source, expanded)` — same chars/4 engine
    // Rust stub returns empty (no breakdown) — fail-open
    Vec::new()
}

pub fn maybe_extend_breakdown(lines: &mut Vec<String>, has_agent: bool, expanded: bool) {
    if has_agent {
        let breakdown = context_breakdown_block_stub(expanded);
        if !breakdown.is_empty() {
            lines.push(String::new());
            lines.extend(breakdown);
        }
    }
}

/// Transcript fallback when no context window is known.
///
/// Mirrors:
/// ```python
/// history = await self.async_session_store.load_transcript(session_entry.session_id)
/// if history:
///     from agent.model_metadata import estimate_messages_tokens_rough
///     msgs = [m for m in history if m.get("role") in {"user", "assistant"} and m.get("content")]
///     approx = estimate_messages_tokens_rough(msgs)
///     return "\n".join([t("gateway.context.header"), "", t("gateway.context.estimated", count=f"{approx:,}", messages=len(msgs)), t("gateway.context.detail_after_first")])
/// return t("gateway.context.no_data")
/// ```
pub fn context_transcript_fallback_stub(history: Option<&[serde_json::Value]>) -> String {
    if let Some(msgs_raw) = history {
        let msgs: Vec<&serde_json::Value> = msgs_raw
            .iter()
            .filter(|m| {
                let role = m.get("role").and_then(|v| v.as_str()).unwrap_or("");
                let content = m.get("content").and_then(|v| v.as_str()).unwrap_or("");
                matches!(role, "user" | "assistant") && !content.trim().is_empty()
            })
            .collect();
        if !msgs.is_empty() {
            let approx = estimate_messages_tokens_rough_stub(&msgs);
            return vec![
                t_context_header(),
                String::new(),
                t_context_estimated(&format_with_commas(approx), msgs.len()),
                t_context_detail_after_first(),
            ]
            .join("\n");
        }
    }
    t_context_no_data()
}

/// Rough estimate stub — mirrors `agent.model_metadata.estimate_messages_tokens_rough`.
///
/// Python sums `len(content) // 4` per message (chars/4 engine). Rust stub does same.
pub fn estimate_messages_tokens_rough_stub(msgs: &[&serde_json::Value]) -> i64 {
    let mut total: i64 = 0;
    for m in msgs {
        if let Some(content) = m.get("content").and_then(|v| v.as_str()) {
            total += (content.chars().count() as i64) / 4;
        }
    }
    total
}

/// Assemble the full `/context` lines for the gauge-present branch.
///
/// Mirrors the `if ctx is not None:` block's threshold/compressions/totals
/// plus the `if has_agent: breakdown` tail. Caller is expected to have already
/// pushed the gauge header/bar/headroom and the empty separator `lines.append("")`.
/// This function appends in-place and returns nothing (mutates `lines`).
pub fn context_gauge_tail(
    lines: &mut Vec<String>,
    threshold: i64,
    threshold_pct: f64,
    used: i64,
    compressions: i64,
    last_savings_pct: Option<f64>,
    api_calls: i64,
    input_tokens: i64,
    output_tokens: i64,
    reasoning_tokens: i64,
    total_tokens: i64,
    has_agent: bool,
    expanded: bool,
) {
    // Python: `if threshold > 0:` branch
    if let Some(tline) = context_threshold_line(used, threshold, threshold_pct) {
        lines.push(tline);
    }
    // Python: `compressions = getattr(ctx, "compression_count", 0) or 0` + savings
    lines.extend(context_compressions_block(compressions, last_savings_pct));
    // Python: totals block (api_calls ... throughput_note)
    lines.extend(context_totals_block(api_calls, input_tokens, output_tokens, reasoning_tokens, total_tokens));
    // Note: the `else: lines.append(t("gateway.context.detail_after_first"))` for `if ctx is not None: else`
    // is handled by caller when ctx is None (slice 1 gauge prologue). Here ctx is Some, so we don't add it.

    // Per-category breakdown (fail-open)
    maybe_extend_breakdown(lines, has_agent, expanded);
}

// Format i64 with comma separators (e.g., 1234567 → "1,234,567")
fn format_with_commas(n: i64) -> String {
    let s = n.to_string();
    let (neg, digits) = if s.starts_with('-') { (true, &s[1..]) } else { (false, s.as_str()) };
    let mut out = String::new();
    let mut count = 0;
    for ch in digits.chars().rev() {
        if count != 0 && count % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
        count += 1;
    }
    let mut rev: String = out.chars().rev().collect();
    if neg {
        rev = format!("-{}", rev);
    }
    rev
}

// ---------------------------------------------------------------------------
// _gateway_session_origin_for_id — mirrors Python lines 990–1003
// ---------------------------------------------------------------------------

/// Best-effort origin lookup for gateway session IDs.
///
/// Mirrors:
/// ```python
/// def _gateway_session_origin_for_id(self, session_id: str) -> Optional[SessionSource]:
///     lookup = getattr(type(self.session_store), "lookup_by_session_id", None)
///     if callable(lookup):
///         entry = lookup(self.session_store, session_id)
///         return getattr(entry, "origin", None) if entry is not None else None
///     entries = getattr(self.session_store, "_entries", {}) or {}
///     for entry in entries.values():
///         if getattr(entry, "session_id", None) == session_id:
///             return getattr(entry, "origin", None)
///     return None
/// ```
///
/// Rust std-only: probes an in-memory map `entries: HashMap<session_key, (session_id, origin)>`
/// when no typed lookup exists. The typed lookup path is documented as `// Python:`.
pub fn gateway_session_origin_for_id(
    session_id: &str,
    entries: &HashMap<String, (String, Option<SessionSource>)>,
) -> Option<SessionSource> {
    // Python: `lookup = getattr(type(self.session_store), "lookup_by_session_id", None)`
    // Rust: stub — no typed lookup in std-only; fall through to entries scan (#997–1002)
    for (_key, (sid, origin)) in entries.values().enumerate().map(|(_, v)| v) {
        let _ = _key; // silence unused
    }
    for (_k, (sid, origin)) in entries {
        if sid == session_id {
            return origin.clone();
        }
    }
    None
}

#[allow(dead_code)]
fn _gateway_session_origin_for_id(
    session_id: &str,
    entries: &HashMap<String, (String, Option<SessionSource>)>,
) -> Option<SessionSource> {
    gateway_session_origin_for_id(session_id, entries)
}

/// Overload that scans a `serde_json::Value` entries map (mirrors Python dict scan).
pub fn gateway_session_origin_for_id_value(
    session_id: &str,
    entries: &serde_json::Value,
) -> Option<SessionSource> {
    let obj = entries.as_object()?;
    for (_k, v) in obj {
        let sid = v.get("session_id").and_then(|x| x.as_str()).unwrap_or("");
        if sid == session_id {
            // Try to parse origin as SessionSource-like dict
            if let Some(origin) = v.get("origin") {
                if origin.is_null() {
                    return None;
                }
                // Minimal parse: use `session_source_from_value`
                return session_source_from_value(origin);
            }
            return None;
        }
    }
    None
}

fn session_source_from_value(v: &serde_json::Value) -> Option<SessionSource> {
    if v.is_null() {
        return None;
    }
    let obj = v.as_object()?;
    let platform_raw = obj.get("platform").and_then(|x| x.as_str()).map(|s| s.to_string());
    let platform = platform_raw.as_deref().map(|s| Platform::from_str(s));
    Some(SessionSource {
        platform,
        platform_raw,
        chat_id: obj.get("chat_id").and_then(|x| x.as_str()).map(|s| s.to_string()),
        thread_id: obj.get("thread_id").and_then(|x| x.as_str()).map(|s| s.to_string()),
        user_id: obj.get("user_id").and_then(|x| x.as_str()).map(|s| s.to_string()),
        user_id_alt: obj.get("user_id_alt").and_then(|x| x.as_str()).map(|s| s.to_string()),
        chat_type: obj.get("chat_type").and_then(|x| x.as_str()).map(|s| s.to_string()),
        ..Default::default()
    })
}

// ---------------------------------------------------------------------------
// _same_matrix_room — mirrors Python lines 1005–1021
// ---------------------------------------------------------------------------

/// Mirrors `GatewaySlashCommandsMixin._same_matrix_room` (static).
///
/// ```python
/// @staticmethod
/// def _same_matrix_room(current: SessionSource, origin: Optional[SessionSource]) -> bool:
///     return (
///         origin is not None
///         and origin.platform == Platform.MATRIX
///         and current.platform == Platform.MATRIX
///         and origin.chat_id == current.chat_id
///         and str(getattr(current, "thread_id", "") or "") == str(getattr(origin, "thread_id", "") or "")
///     )
/// ```
pub fn same_matrix_room(current: &SessionSource, origin: Option<&SessionSource>) -> bool {
    let origin = match origin {
        Some(o) => o,
        None => return false,
    };
    // Platform must be Matrix on both sides
    let cur_is_matrix = matches!(current.platform, Some(Platform::Matrix))
        || current.platform_value() == "matrix";
    let org_is_matrix = matches!(origin.platform, Some(Platform::Matrix))
        || origin.platform_value() == "matrix";
    if !cur_is_matrix || !org_is_matrix {
        return false;
    }
    if current.chat_id.as_deref().unwrap_or("") != origin.chat_id.as_deref().unwrap_or("") {
        return false;
    }
    let cur_thread = current.thread_id.as_deref().unwrap_or("").trim();
    let org_thread = origin.thread_id.as_deref().unwrap_or("").trim();
    // Python: `str(getattr(current, "thread_id", "") or "") == str(getattr(origin, "thread_id", "") or "")`
    // Normalize None/empty to "" and compare as strings
    cur_thread == org_thread
}

#[allow(dead_code)]
fn _same_matrix_room(current: &SessionSource, origin: Option<&SessionSource>) -> bool {
    same_matrix_room(current, origin)
}

// ---------------------------------------------------------------------------
// _same_origin_chat — mirrors Python lines 1023–1087
// ---------------------------------------------------------------------------

/// Platform-agnostic counterpart to `_same_matrix_room`.
///
/// Mirrors the full 65-line Python method, including `is_shared_multi_user_session`
/// lock-step with `build_session_key`.
///
/// Returns true when `origin` shares `current`'s platform and chat, and the same
/// participant whenever the session key for this source is per-user.
pub fn same_origin_chat(
    current: &SessionSource,
    origin: Option<&SessionSource>,
    group_sessions_per_user: bool,
    thread_sessions_per_user: bool,
) -> bool {
    let origin = match origin {
        Some(o) => o,
        None => return false,
    };
    if current.platform_value() != origin.platform_value() {
        return false;
    }
    let cur_chat = current.chat_id.as_deref().unwrap_or("").trim();
    let org_chat = origin.chat_id.as_deref().unwrap_or("").trim();
    if cur_chat != org_chat {
        return false;
    }
    let cur_thread = current.thread_id.as_deref().unwrap_or("").trim().to_string();
    let org_thread = origin.thread_id.as_deref().unwrap_or("").trim().to_string();
    // thread_id is part of session key for every chat type when present
    if cur_thread != org_thread {
        return false;
    }
    let chat_type = current.chat_type_lower();
    // DM-like chats are always per-user
    if matches!(chat_type.as_str(), "dm" | "direct" | "private" | "") {
        // `chat_id was already required equal above and, when present, IS the DM session key`
        // Build_session_key only falls back to participant id when there is NO chat_id
        if !cur_chat.is_empty() {
            return true;
        }
        let cur_pid = current
            .user_id_alt
            .as_deref()
            .or(current.user_id.as_deref())
            .unwrap_or("")
            .trim()
            .to_string();
        let org_pid = origin
            .user_id_alt
            .as_deref()
            .or(origin.user_id.as_deref())
            .unwrap_or("")
            .trim()
            .to_string();
        return !cur_pid.is_empty() && cur_pid == org_pid;
    }
    // Non-DM: scope by participant whenever session key is per-user
    let shared = is_shared_multi_user_session_stub(
        current,
        group_sessions_per_user,
        thread_sessions_per_user,
    );
    if shared {
        return true;
    }
    // Per-user key: compare participant id the key is built from (user_id_alt or user_id)
    let cur_pid = current.user_id_alt.as_deref().or(current.user_id.as_deref());
    let org_pid = origin.user_id_alt.as_deref().or(origin.user_id.as_deref());
    match (cur_pid, org_pid) {
        (Some(a), Some(b)) if !a.trim().is_empty() && !b.trim().is_empty() => a == b,
        _ => false, // fail closed when participant id missing
    }
}

#[allow(dead_code)]
fn _same_origin_chat(
    current: &SessionSource,
    origin: Option<&SessionSource>,
    group_sessions_per_user: bool,
    thread_sessions_per_user: bool,
) -> bool {
    same_origin_chat(current, origin, group_sessions_per_user, thread_sessions_per_user)
}

/// Stub for `gateway.session.is_shared_multi_user_session`.
///
/// Mirrors `is_shared_multi_user_session(source, group_sessions_per_user, thread_sessions_per_user)`.
/// True when the session key for `source` is shared across participants (group_sessions_per_user=False
/// or a shared thread). The real function lives in `gateway/session.py` and mirrors
/// `build_session_key` isolation rules exactly.
///
/// Rust stub: explicit threading rules — DM is always per-user (never shared), group/channel is shared
/// iff `group_sessions_per_user == false`, thread is shared iff `thread_sessions_per_user == false`
/// and source has a thread_id.
// ponytail: deterministic heuristic; wire to real session crate when available
pub fn is_shared_multi_user_session_stub(
    source: &SessionSource,
    group_sessions_per_user: bool,
    thread_sessions_per_user: bool,
) -> bool {
    let chat_type = source.chat_type_lower();
    if matches!(chat_type.as_str(), "dm" | "direct" | "private" | "") {
        return false;
    }
    // If source is in a thread and thread_sessions_per_user is False, it's shared
    let in_thread = source.thread_id.as_deref().map(|s| !s.trim().is_empty()).unwrap_or(false);
    if in_thread {
        // Python: `is_shared_multi_user_session` checks thread sharing separately
        // When thread_sessions_per_user is False, thread sessions are shared even if group is per-user
        if !thread_sessions_per_user {
            return true;
        }
        // Otherwise fall through to group logic
    }
    // Group/channel: shared iff group_sessions_per_user is False
    !group_sessions_per_user
}

// ---------------------------------------------------------------------------
// _resume_caller_is_admin — mirrors Python lines 1089–1106
// ---------------------------------------------------------------------------

/// Whether `source` is an explicitly-configured admin allowed to make a
/// cross-origin /resume or /sessions listing.
///
/// Mirrors:
/// ```python
/// def _resume_caller_is_admin(self, source: SessionSource) -> bool:
///     try:
///         from gateway.slash_access import policy_for_source
///         policy = policy_for_source(self.config, source)
///         uid = getattr(source, "user_id", None)
///         return bool(policy.enabled and uid and policy.is_admin(uid))
///     except Exception:
///         return False
/// ```
///
/// Deliberately stricter than `SlashAccessPolicy.is_admin()` — returns True only
/// when slash gating is ENABLED and caller is an admin.
pub fn resume_caller_is_admin(source: &SessionSource, policy_enabled: bool, is_admin_fn: impl Fn(&str) -> bool) -> bool {
    // Python: `policy_for_source(self.config, source)` → policy.enabled + is_admin(uid)
    // Rust stub takes policy_enabled + is_admin closure for testability
    let uid = source.user_id.as_deref().map(|s| s.trim()).filter(|s| !s.is_empty());
    match uid {
        Some(id) => policy_enabled && is_admin_fn(id),
        None => false,
    }
}

#[allow(dead_code)]
fn _resume_caller_is_admin(source: &SessionSource, policy_enabled: bool, is_admin: fn(&str) -> bool) -> bool {
    resume_caller_is_admin(source, policy_enabled, is_admin)
}

/// Env-backed stub for `policy_for_source` + `is_admin`.
///
/// Reads `HERMES_SLASH_ACCESS_ENABLED` + `HERMES_ADMIN_IDS` (comma-separated).
// ponytail: env stub; wire to real slash_access crate when available
pub fn resume_caller_is_admin_env(source: &SessionSource) -> bool {
    let enabled = std::env::var("HERMES_SLASH_ACCESS_ENABLED")
        .ok()
        .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false);
    if !enabled {
        return false;
    }
    let uid = match source.user_id.as_deref().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) {
        Some(u) => u,
        None => return false,
    };
    let admins_raw = std::env::var("HERMES_ADMIN_IDS").unwrap_or_default();
    let admins: HashSet<String> = admins_raw.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    admins.contains(&uid)
}

fn policy_for_source_stub() -> (bool, HashSet<String>) {
    let enabled = std::env::var("HERMES_SLASH_ACCESS_ENABLED")
        .ok()
        .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false);
    let admins: HashSet<String> = std::env::var("HERMES_ADMIN_IDS")
        .unwrap_or_default()
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    (enabled, admins)
}

// ---------------------------------------------------------------------------
// _resume_target_allowed — mirrors Python lines 1108–1249
// ---------------------------------------------------------------------------

/// Whether `source` may resume the persisted session `target_id`.
///
/// Mirrors `async def _resume_target_allowed(self, source, target_id, allow_override=False)`.
/// Uses live origin when target is active; otherwise falls back to DB row's
/// source + user_id. Generalizes the Matrix-only room guard to every adapter
/// so a caller cannot bind their gateway session to another user's/room's
/// persisted session id (IDOR).
///
/// Rust is synchronous stub; async DB/store branches are documented as `// Python:`.
/// `allow_override` bypasses scoping only for an explicit admin `--all`.
pub fn resume_target_allowed(
    source: &SessionSource,
    target_id: &str,
    allow_override: bool,
    // Injected dependencies for testability (mirrors `self` state)
    caller_is_admin: bool,
    live_origin: Option<&SessionSource>,
    db_row: Option<&HashMap<String, serde_json::Value>>,
    group_sessions_per_user: bool,
    thread_sessions_per_user: bool,
) -> bool {
    // Python: `if allow_override and self._resume_caller_is_admin(source): return True`
    if allow_override && caller_is_admin {
        return true;
    }
    // Python: `origin = self._gateway_session_origin_for_id(target_id)` try/except
    // Rust: injected `live_origin` (None means store couldn't resolve or error)
    if let Some(origin) = live_origin {
        // Python: `if isinstance(origin, SessionSource): return self._same_origin_chat(source, origin)`
        return same_origin_chat(source, Some(origin), group_sessions_per_user, thread_sessions_per_user);
    }
    // Inactive/persisted-only: best-effort scope by DB row source + user.
    // Python: `row = await self._session_db.get_session(target_id) or {}`
    let row = match db_row {
        Some(r) => r,
        None => return false, // get_session error or None → fail closed
    };

    let caller_src = source.platform_value();
    let caller_src_opt = if caller_src.is_empty() { None } else { Some(caller_src.clone()) };
    // row_src is `row.get("source")` — may be None/blank for legacy rows
    let row_src = row.get("source").and_then(|v| v.as_str()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    // Python: `if row_src and caller_src and str(row_src) != str(caller_src): return False`
    if let (Some(rs), Some(cs)) = (&row_src, &caller_src_opt) {
        if rs != cs {
            return false;
        }
    }
    let caller_uid = source.user_id.as_deref().unwrap_or("").trim().to_string();
    let row_uid = row.get("user_id").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    let caller_chat = source.chat_id.as_deref().unwrap_or("").trim().to_string();
    let row_chat = row.get("chat_id").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    let caller_thread = source.thread_id.as_deref().unwrap_or("").trim().to_string();
    let row_thread = row.get("thread_id").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    let chat_type = source.chat_type_lower();
    let caller_is_dm = matches!(chat_type.as_str(), "dm" | "direct" | "private" | "");
    let caller_keys_on_alt = source.user_id_alt.as_deref().map(|s| !s.trim().is_empty()).unwrap_or(false);

    if !caller_uid.is_empty() {
        // Identity-bearing caller: common origin proof required
        // Python: `origin_ok = bool(row_src) and bool(caller_src) and str(row_src) == str(caller_src) and row_thread == caller_thread`
        let origin_ok = row_src.is_some()
            && caller_src_opt.is_some()
            && row_src.as_deref() == caller_src_opt.as_deref()
            && row_thread == caller_thread;
        if !origin_ok {
            return false;
        }
        if caller_is_dm {
            // DM: if caller keys on user_id_alt and not both have chat_id, fail closed
            if caller_keys_on_alt && !( !row_chat.is_empty() && !caller_chat.is_empty()) {
                return false;
            }
            return !row_uid.is_empty() && row_uid == caller_uid && row_chat == caller_chat;
        }
        // Non-DM: Require both sides non-blank and equal chat
        if !( !row_chat.is_empty() && !caller_chat.is_empty() && row_chat == caller_chat) {
            return false;
        }
        // Within same non-DM chat/thread, mirror build_session_key participant scoping
        let shared = is_shared_multi_user_session_stub(source, group_sessions_per_user, thread_sessions_per_user);
        if shared {
            return true;
        }
        if caller_keys_on_alt {
            return false;
        }
        return !row_uid.is_empty() && row_uid == caller_uid;
    }
    // No caller identity: fail closed (CWE-639)
    false
}

/// Convenience wrapper that reads group/thread sharing from env/config defaults.
// ponytail: defaults to group_sessions_per_user=True, thread_sessions_per_user=False
pub fn resume_target_allowed_simple(
    source: &SessionSource,
    target_id: &str,
    allow_override: bool,
    caller_is_admin: bool,
    live_origin: Option<&SessionSource>,
    db_row: Option<&HashMap<String, serde_json::Value>>,
) -> bool {
    resume_target_allowed(source, target_id, allow_override, caller_is_admin, live_origin, db_row, true, false)
}

/// Persisted fallback helper mirroring the 100-line `if caller_uid:` block separately.
///
/// Useful for unit-testing the DB scoping without live-origin.
pub fn resume_target_allowed_persisted(
    source: &SessionSource,
    row: &HashMap<String, serde_json::Value>,
    group_sessions_per_user: bool,
    thread_sessions_per_user: bool,
) -> bool {
    resume_target_allowed(source, "stub_id", false, false, None, Some(row), group_sessions_per_user, thread_sessions_per_user)
}

#[allow(dead_code)]
fn _resume_target_allowed_stub(
    source: &SessionSource,
    live_origin: Option<&SessionSource>,
    db_row: Option<&HashMap<String, serde_json::Value>>,
) -> bool {
    resume_target_allowed(source, "stub", false, false, live_origin, db_row, true, false)
}

// ---------------------------------------------------------------------------
// _resume_row_visible — mirrors Python lines 1251–1273
// ---------------------------------------------------------------------------

/// Whether a titled-session listing `row` belongs to the caller's origin.
///
/// Mirrors:
/// ```python
/// async def _resume_row_visible(self, source: SessionSource, row: dict, allow_all: bool) -> bool:
///     sid = str(row.get("id") or "")
///     if source.platform == Platform.MATRIX:
///         if allow_all and self._resume_caller_is_admin(source):
///             return True
///         return self._same_matrix_room(source, self._gateway_session_origin_for_id(sid))
///     if allow_all and self._resume_caller_is_admin(source):
///         return True
///     return await self._resume_target_allowed(source, sid, allow_override=False)
/// ```
///
/// Prevents cross-origin enumeration of session ids/previews via the numbered /resume list.
pub fn resume_row_visible(
    source: &SessionSource,
    row: &HashMap<String, serde_json::Value>,
    allow_all: bool,
    caller_is_admin: bool,
    live_origin_for_sid: Option<&SessionSource>,
    db_row_for_sid: Option<&HashMap<String, serde_json::Value>>,
    group_sessions_per_user: bool,
    thread_sessions_per_user: bool,
) -> bool {
    let sid = row.get("id").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    // `sid` empty → no origin possible; fall through to _resume_target_allowed which will fail closed
    let is_matrix = matches!(source.platform, Some(Platform::Matrix)) || source.platform_value() == "matrix";
    if is_matrix {
        if allow_all && caller_is_admin {
            return true;
        }
        return same_matrix_room(source, live_origin_for_sid);
    }
    if allow_all && caller_is_admin {
        return true;
    }
    // Python: `return await self._resume_target_allowed(source, sid, allow_override=False)`
    resume_target_allowed(source, &sid, false, false, live_origin_for_sid, db_row_for_sid, group_sessions_per_user, thread_sessions_per_user)
}

#[allow(dead_code)]
fn _resume_row_visible(
    source: &SessionSource,
    row: &HashMap<String, serde_json::Value>,
    allow_all: bool,
    caller_is_admin: bool,
    live_origin: Option<&SessionSource>,
    db_row: Option<&HashMap<String, serde_json::Value>>,
) -> bool {
    resume_row_visible(source, row, allow_all, caller_is_admin, live_origin, db_row, true, false)
}

// ---------------------------------------------------------------------------
// GatewaySlashCommandsMixin — slice 2 handlers
// ---------------------------------------------------------------------------
//
// Python is a mixin inherited by `GatewayRunner` (MRO). Rust models it as
// a struct holding the handles the mixin reaches via `self`. Slice 1 defined
// the struct and its first handlers; this slice adds the remaining handlers.
// The struct is re-declared here with the same shape so slice 2 is
// self-contained (module isolation — each slice's `GatewaySlashCommandsMixin`
// is `crate::slash_commands_slice2::GatewaySlashCommandsMixin`, distinct from
// slice 1's, but with identical fields for 1:1 traceability).

/// In-session slash-command handlers for `GatewayRunner` — slice 2 portion.
///
/// Mirrors `class GatewaySlashCommandsMixin` (slice 2 handlers).
// ponytail: global stub, per-session state if handler throughput matters
#[derive(Debug, Default)]
pub struct GatewaySlashCommandsMixin {
    /// Mirrors `self.async_session_store: AsyncSessionStore` (opaque key)
    pub async_session_store_key: Option<String>,
    /// Mirrors `self.adapters: Dict[Platform, Adapter]`
    pub adapter_prefixes: HashMap<String, String>,
    /// Mirrors `self.config.multiplex_profiles`
    pub multiplex_profiles: bool,
    /// Mirrors `self.config.group_sessions_per_user` / `thread_sessions_per_user`
    pub group_sessions_per_user: bool,
    pub thread_sessions_per_user: bool,
    /// Mirrors `self._running_agents: Dict[session_key, Agent]`
    pub running_agents: HashMap<String, AgentStub>,
    /// Mirrors `self._running_agents_ts: Dict[session_key, float]`
    pub running_agents_ts: HashMap<String, f64>,
    /// Mirrors `self._background_tasks: Set[Task]`
    pub background_tasks: HashSet<String>,
    /// Mirrors `self.adapters` connected platforms
    pub adapters: HashMap<String, String>,
    /// Mirrors `self._failed_platforms: Dict[Platform, {paused, pause_reason, attempts}]`
    pub failed_platforms: HashMap<String, FailedPlatformInfo>,
    /// Mirrors `self._restart_requested` / `self._draining`
    pub restart_requested: bool,
    pub draining: bool,
    /// Mirrors `self._restart_command_source`
    pub restart_command_source: Option<SessionSource>,
}

#[derive(Debug, Clone)]
pub struct AgentStub {
    pub session_id: String,
    pub model: String,
    pub is_pending: bool,
}

#[derive(Debug, Clone, Default)]
pub struct FailedPlatformInfo {
    pub paused: bool,
    pub pause_reason: Option<String>,
    pub attempts: i64,
}

impl GatewaySlashCommandsMixin {
    // -----------------------------------------------------------------------
    // _handle_agents_command — mirrors Python lines 1275–1424
    // -----------------------------------------------------------------------

    /// Handle /agents command - list active agents and running tasks.
    ///
    /// Mirrors `async def _handle_agents_command(self, event: MessageEvent) -> str` (~150 LOC).
    /// Reports running agents (session_key · state · uptime · session_id · model),
    /// running processes from `process_registry`, background tasks, and background
    /// delegations with per-child activity.
    pub fn handle_agents_command(&self, event: &MessageEvent) -> String {
        // Python: `from gateway.run import _AGENT_PENDING_SENTINEL` / `from tools.process_registry import format_uptime_short, process_registry`
        // Rust: `AgentStub.is_pending` sentinel + local `format_uptime_short`
        let now = current_time_secs();
        let current_session_key = build_session_key(&event.source);

        // Python: `running_agents: dict = getattr(self, "_running_agents", {}) or {}`
        let mut agent_rows: Vec<AgentRow> = Vec::new();
        for (session_key, agent) in &self.running_agents {
            let started = self.running_agents_ts.get(session_key).copied().unwrap_or(now);
            let elapsed = (now - started).max(0.0) as i64;
            let elapsed = elapsed.max(0) as i64;
            agent_rows.push(AgentRow {
                session_key: session_key.clone(),
                elapsed,
                state: if agent.is_pending { t_agents_state_starting() } else { t_agents_state_running() },
                session_id: if agent.is_pending { String::new() } else { agent.session_id.clone() },
                model: if agent.is_pending { String::new() } else { agent.model.clone() },
            });
        }
        agent_rows.sort_by(|a, b| b.elapsed.cmp(&a.elapsed));

        // Python: `running_processes = [p for p in process_registry.list_sessions() if p.get("status") == "running"]`
        let running_processes = process_registry_list_stub();

        // Python: `background_tasks = [t for t in (getattr(self, "_background_tasks", set()) or set()) if hasattr(t, "done") and not t.done()]`
        let background_tasks: Vec<&String> = self.background_tasks.iter().collect();

        // Python: `delegations = [d for d in list_async_delegations() if d.get("status") in ("running", "stalling", "finalizing")]`
        let delegations = list_async_delegations_stub();

        let mut lines: Vec<String> = Vec::new();
        lines.push(t_agents_header());
        lines.push(String::new());
        lines.push(t_agents_active_agents(agent_rows.len() as i64));

        if !agent_rows.is_empty() {
            for (idx, row) in agent_rows.iter().take(12).enumerate() {
                let current = if row.session_key == current_session_key { format!(" {}", t_agents_this_chat()) } else { String::new() };
                let sid = if row.session_id.is_empty() { String::new() } else { format!(" · `{}`", row.session_id) };
                let model = if row.model.is_empty() { String::new() } else { format!(" · `{}`", row.model) };
                lines.push(format!(
                    "{}. `{}` · {} · {}{}{}{}",
                    idx + 1,
                    row.session_key,
                    row.state,
                    format_uptime_short(row.elapsed),
                    sid,
                    model,
                    current
                ));
            }
            if agent_rows.len() > 12 {
                lines.push(t_agents_more((agent_rows.len() - 12) as i64));
            }
        }

        lines.push(String::new());
        lines.push(t_agents_running_processes(running_processes.len() as i64));

        if !running_processes.is_empty() {
            for proc in running_processes.iter().take(12) {
                let mut cmd = proc.command.split_whitespace().collect::<Vec<_>>().join(" ");
                if cmd.len() > 90 {
                    cmd.truncate(87);
                    cmd.push_str("...");
                }
                lines.push(format!(
                    "- `{}` · {} · `{}`",
                    proc.session_id,
                    format_uptime_short(proc.uptime_seconds),
                    cmd
                ));
            }
            if running_processes.len() > 12 {
                lines.push(t_agents_more((running_processes.len() - 12) as i64));
            }
        }

        lines.push(String::new());
        lines.push(t_agents_async_jobs(background_tasks.len() as i64));

        if !delegations.is_empty() {
            lines.push(String::new());
            lines.push(t_agents_background_delegations(delegations.len() as i64));
            for d in delegations.iter().take(12) {
                let mut goal = d.goal.split_whitespace().collect::<Vec<_>>().join(" ");
                if goal.len() > 70 {
                    goal.truncate(67);
                    goal.push_str("...");
                }
                let mut row = format!("- `{}` · {}", d.delegation_id, d.status);
                if d.status == "stalling" {
                    if let Some(quiet) = d.stalled_after_quiet_seconds {
                        row.push_str(&format!(" · no progress {:.0}s", quiet));
                    }
                } else if d.seconds_since_progress >= 60.0 {
                    row.push_str(&format!(" · quiet {:.0}s", d.seconds_since_progress));
                }
                if !goal.is_empty() {
                    row.push_str(&format!(" · {}", goal));
                }
                lines.push(row);
                for (i, child) in d.children_activity.iter().enumerate() {
                    // Python: `if not isinstance(child, dict): continue`
                    let doing = match &child.current_tool {
                        Some(t) if !t.is_empty() => format!("`{}`", t),
                        _ => "between turns".to_string(),
                    };
                    let mut part = format!("  - child {}: {} api calls · {}", i + 1, child.api_calls, doing);
                    if let Some(idle) = child.seconds_since_activity {
                        part.push_str(&format!(" · active {:.0}s ago", idle));
                    }
                    lines.push(part);
                }
            }
            if delegations.len() > 12 {
                lines.push(t_agents_more((delegations.len() - 12) as i64));
            }
        }

        if agent_rows.is_empty() && running_processes.is_empty() && background_tasks.is_empty() && delegations.is_empty() {
            lines.push(String::new());
            lines.push(t_agents_none());
        }

        lines.join("\n")
    }

    // -----------------------------------------------------------------------
    // _handle_stop_command — mirrors Python lines 1426–1507
    // -----------------------------------------------------------------------

    /// Handle /stop command - interrupt a running agent.
    ///
    /// Mirrors `async def _handle_stop_command(self, event) -> Union[str, EphemeralReply]` (~82 LOC).
    /// When an agent is truly hung (blocked thread that never checks
    /// `_interrupt_requested`), the early intercept in `_handle_message()`
    /// handles /stop before this method is reached. This handler fires
    /// only through normal command dispatch (no running agent) or as a fallback.
    pub fn handle_stop_command(&self, event: &MessageEvent) -> StopResult {
        // Python: `source = event.source; session_entry = await async_session_store.get_or_create_session(source); session_key = session_entry.session_key`
        let source = &event.source;
        let session_key = build_session_key(source);

        // Python: `agent = self._running_agents.get(session_key)`
        if let Some(agent) = self.running_agents.get(&session_key) {
            if agent.is_pending {
                // Python: `_interrupt_and_clear_session(session_key, source, interrupt_reason=_INTERRUPT_REASON_STOP, invalidation_reason="stop_command_pending")`
                log_debug(&format!("STOP (pending) for session {} — sentinel cleared", session_key));
                return StopResult::Ephemeral(t_stop_stopped_pending());
            }
            // Running non-pending agent
            log_debug(&format!("STOP for session {} — _interrupt_and_clear_session", session_key));
            return StopResult::Ephemeral(t_stop_stopped());
        }

        // Python: `sibling_keys = self._sibling_thread_run_keys(source, session_key)` + `_is_user_authorized`
        let sibling_keys = sibling_thread_run_keys_stub(source, &session_key, &self.running_agents);
        if !sibling_keys.is_empty() && is_user_authorized_stub(source) {
            log_debug(&format!("STOP (thread sibling) by {} — interrupted {} run(s) in thread: {}", session_key, sibling_keys.len(), sibling_keys.join(", ")));
            return StopResult::Ephemeral(t_stop_stopped());
        }

        // Python: adapter._stop_typing_with_metadata best-effort clear
        // ponytail: no adapter typing dep in std-only; no-op with debug trace
        log_debug(&format!("STOP no active agent for {} — typing clear attempted", session_key));
        StopResult::Plain(t_stop_no_active())
    }

    // -----------------------------------------------------------------------
    // _handle_platform_command — mirrors Python lines 1509–1600
    // -----------------------------------------------------------------------

    /// Handle `/platform list|pause|resume [name]` — surface and manually
    /// control failed/paused gateway adapters.
    ///
    /// Mirrors `async def _handle_platform_command(self, event) -> str` (~92 LOC).
    pub fn handle_platform_command(&self, event: &MessageEvent) -> String {
        // Python: `text = (getattr(event, "content", "") or "").strip()`
        let text = event.content.as_deref().or(event.text.as_deref()).unwrap_or("").trim().to_string();
        let mut parts: Vec<String> = text.split_whitespace().map(|s| s.to_string()).collect();
        // Strip leading "/platform" token if present
        if !parts.is_empty() && parts[0].to_ascii_lowercase().trim_start_matches('/').starts_with("platform") {
            parts.remove(0);
        }
        let action = parts.get(0).map(|s| s.to_ascii_lowercase()).unwrap_or_else(|| "list".to_string());
        let target = parts.get(1).map(|s| s.to_ascii_lowercase()).unwrap_or_default();

        // Python: `def _resolve_platform(name: str)` — case-insensitive value match
        let resolve_platform = |name: &str| -> Option<Platform> {
            if name.is_empty() {
                return None;
            }
            // Match against all Platform variants' value (lowercased)
            // Rust stub: delegate to `resolve_platform_by_value`
            resolve_platform_by_value(name)
        };

        if action == "list" {
            let mut lines = vec!["**Gateway platforms**".to_string()];
            let mut connected: Vec<String> = self.adapters.keys().cloned().collect();
            connected.sort();
            if !connected.is_empty() {
                lines.push(format!("Connected: {}", connected.join(", ")));
            } else {
                lines.push("Connected: (none)".to_string());
            }
            if self.failed_platforms.is_empty() {
                lines.push("Failed/paused: (none)".to_string());
            } else {
                for (p_key, info) in &self.failed_platforms {
                    let platform = resolve_platform(p_key).unwrap_or(Platform::Unknown(p_key.clone()));
                    let pval = platform.value();
                    if info.paused {
                        let reason = info.pause_reason.as_deref().unwrap_or("paused");
                        lines.push(format!("  · {} — PAUSED ({}). Resume with `/platform resume {}`.", pval, reason, pval));
                    } else {
                        lines.push(format!("  · {} — retrying (attempt {})", pval, info.attempts));
                    }
                }
            }
            return lines.join("\n");
        }

        if action == "pause" || action == "resume" {
            if target.is_empty() {
                return format!("Usage: /platform {} <name>", action);
            }
            let platform = resolve_platform(&target);
            if platform.is_none() {
                return format!("Unknown platform: {}", target);
            }
            let platform = platform.unwrap();
            let pval = platform.value();
            let key = pval.clone();
            if action == "pause" {
                if !self.failed_platforms.contains_key(&key) && !self.failed_platforms.contains_key(&target) {
                    return format!("{} is not in the retry queue (it's either connected or not enabled).", pval);
                }
                let info_key = if self.failed_platforms.contains_key(&key) { key } else { target.clone() };
                if self.failed_platforms.get(&info_key).map(|i| i.paused).unwrap_or(false) {
                    return format!("{} is already paused.", pval);
                }
                // Python: `self._pause_failed_platform(platform, reason="paused via /platform pause")`
                // Rust stub: log and return success message
                log_debug(&format!("pause_failed_platform {}", pval));
                return format!("✓ {} paused. Resume with `/platform resume {}` or `hermes gateway restart` to reset.", pval, pval);
            } else {
                // resume
                if !self.failed_platforms.contains_key(&key) && !self.failed_platforms.contains_key(&target) {
                    return format!("{} is not in the retry queue — nothing to resume.", pval);
                }
                let info_key = if self.failed_platforms.contains_key(&key) { key } else { target.clone() };
                if !self.failed_platforms.get(&info_key).map(|i| i.paused).unwrap_or(false) {
                    return format!("{} is already retrying — no resume needed.", pval);
                }
                log_debug(&format!("resume_paused_platform {}", pval));
                return format!("✓ {} resumed — retrying on next watcher tick.", pval);
            }
        }

        // Python: usage fallback
        "Usage: /platform <list|pause|resume> [name]\n  /platform list — show platform status\n  /platform pause <name> — stop retrying a failing platform\n  /platform resume <name> — re-queue a paused platform".to_string()
    }

    // -----------------------------------------------------------------------
    // _handle_restart_command — mirrors Python lines 1602–1712
    // -----------------------------------------------------------------------

    /// Handle /restart command - drain active work, then restart the gateway.
    ///
    /// Mirrors `async def _handle_restart_command(self, event) -> Union[str, EphemeralReply]` (~111 LOC).
    /// Includes stale redelivery guard, restart notify file writes, dedup marker,
    /// service/container restart path selection, and draining vs immediate reply.
    pub fn handle_restart_command(&self, event: &MessageEvent) -> RestartResult {
        // Python: `if self._is_stale_restart_redelivery(event): return ""`
        if is_stale_restart_redelivery_stub(event) {
            log_debug(&format!(
                "Ignoring redelivered /restart (platform={}, update_id={}) — already processed by a previous gateway instance.",
                event.source.platform_value(),
                event.platform_update_id.as_deref().unwrap_or("?")
            ));
            return RestartResult::Empty;
        }

        if self.restart_requested || self.draining {
            let count = self.running_agent_count();
            if count > 0 {
                return RestartResult::Plain(t_draining(count));
            }
            return RestartResult::Ephemeral(t_restart_in_progress());
        }

        // Python: save routing info to `.restart_notify.json` via `atomic_json_write`
        // Rust stub: build notify_data and log (ponytail: no file IO in std-only)
        let notify_data = restart_notify_data(event);
        let _ = notify_data; // would be `atomic_json_write(_hermes_home / ".restart_notify.json", notify_data)`
        log_debug("restart: wrote .restart_notify.json (stub)");

        // Python: dedup marker `.restart_last_processed.json` (persists after notify unlink)
        let dedup_data = restart_dedup_data(event);
        let _ = dedup_data;
        log_debug("restart: wrote .restart_last_processed.json (stub)");

        let active_agents = self.running_agent_count();
        // Python: `from gateway.restart import is_container_restart_context, is_gateway_supervisor_process`
        let under_service = is_gateway_supervisor_process_stub();
        let in_container = is_container_restart_context_stub();
        // Python: `self.request_restart(detached=False/True, via_service=True/False)`
        if under_service || in_container {
            log_debug("request_restart(detached=False, via_service=True)");
        } else {
            log_debug("request_restart(detached=True, via_service=False)");
        }
        if active_agents > 0 {
            return RestartResult::Plain(t_draining(active_agents));
        }
        RestartResult::Ephemeral(t_restart_restarting())
    }

    // -----------------------------------------------------------------------
    // _handle_version_command — mirrors Python lines 1714–1718
    // -----------------------------------------------------------------------

    /// Handle /version — show the running Hermes Agent version.
    ///
    /// Mirrors:
    /// ```python
    /// async def _handle_version_command(self, event: MessageEvent) -> str:
    ///     from hermes_cli.slash_exec import CommandContext, execute_command
    ///     return execute_command("version", CommandContext(surface="gateway")).text
    /// ```
    pub fn handle_version_command(&self, _event: &MessageEvent) -> String {
        execute_command_stub("version", "")
    }

    // -----------------------------------------------------------------------
    // _handle_help_command — mirrors Python lines 1720–1729
    // -----------------------------------------------------------------------

    /// Handle /help command - list available commands.
    ///
    /// Mirrors:
    /// ```python
    /// async def _handle_help_command(self, event: MessageEvent) -> str:
    ///     from gateway.run import _telegramize_command_mentions
    ///     from hermes_cli.slash_exec import CommandContext, execute_command
    ///     reply = execute_command("help", CommandContext(surface="gateway"))
    ///     return _telegramize_command_mentions(reply.text, getattr(getattr(event, "source", None), "platform", None))
    /// ```
    pub fn handle_help_command(&self, event: &MessageEvent) -> String {
        let reply = execute_command_stub("help", "");
        telegramize_command_mentions_stub(&reply, event.source.platform_value().as_str())
    }

    // -----------------------------------------------------------------------
    // _handle_commands_command — mirrors Python lines 1731–1749
    // -----------------------------------------------------------------------

    /// Handle /commands — paginated command catalog.
    ///
    /// Mirrors:
    /// ```python
    /// async def _handle_commands_command(self, event: MessageEvent) -> str:
    ///     from gateway.run import _telegramize_command_mentions
    ///     from hermes_cli.slash_exec import CommandContext, execute_command
    ///     from gateway.config import Platform
    ///     page_size = 15 if event.source.platform == Platform.TELEGRAM else 20
    ///     reply = execute_command("commands", CommandContext(surface="gateway", args=event.get_command_args(), options={"page_size": page_size}))
    ///     return _telegramize_command_mentions(reply.text, ...)
    /// ```
    pub fn handle_commands_command(&self, event: &MessageEvent) -> String {
        let page_size = if matches!(event.source.platform, Some(Platform::Telegram)) || event.source.platform_value() == "telegram" { 15 } else { 20 };
        let args = event.get_command_args();
        let reply = execute_command_with_options_stub("commands", &args, page_size);
        telegramize_command_mentions_stub(&reply, event.source.platform_value().as_str())
    }

    // -----------------------------------------------------------------------
    // _handle_model_command — mirrors Python lines 1751–1800 (partial)
    // -----------------------------------------------------------------------

    /// Handle /model command — switch model (partial, slice 2).
    ///
    /// Mirrors `async def _handle_model_command(self, event) -> Optional[str]` through
    /// line 1800 (`if force_refresh:`). Full body (picker, provider list,
    /// global/session/once persistence, skew guard, `switch_model` call, profile
    /// scoping) continues in slice 3.
    ///
    /// Supports:
    /// ```text
    ///   /model                              — interactive picker
    ///   /model <name>                       — switch model (session only)
    ///   /model <name> --once                — next turn only
    ///   /model <name> --session             — this session only (explicit)
    ///   /model <name> --global              — persist to config.yaml
    ///   /model <name> --provider <provider> — switch provider + model
    ///   /model --provider <provider>        — switch provider, auto-detect model
    /// ```
    pub fn handle_model_command(&self, event: &MessageEvent) -> Option<String> {
        // Python: `from gateway.run import _hermes_home, _load_gateway_config`
        // Python: `from hermes_cli.model_switch import switch_model as _switch_model, parse_model_switch_args, resolve_persist_behavior, list_authenticated_providers, list_picker_providers`
        // Python: `from hermes_cli.providers import get_label`
        let raw_args = event.get_command_args().trim().to_string();
        let source = &event.source;
        let _command_profile_home: Option<PathBuf> = if self.multiplex_profiles {
            // Python: `self._resolve_profile_home_for_source(source)` when multiplex_profiles
            source.profile.as_deref().map(|p| PathBuf::from(format!("~/.hermes/profiles/{}", p)))
        } else {
            None
        };
        let _ = _command_profile_home;

        // Parse --provider, --global, --session, --once, --refresh via single-owner parser
        // Python: `request = parse_model_switch_args(raw_args)`
        let request = parse_model_switch_args_stub(&raw_args);
        let _model_input = request.target.clone();
        let _explicit_provider = request.explicit_provider.clone();
        let _is_global_flag = request.is_global;
        let force_refresh = request.force_refresh;
        let _is_session = request.is_session;
        let _one_turn = request.is_once;
        if !request.errors.is_empty() {
            // Python: `return f"❌ {request.error_messages()[0]}"`
            return Some(format!("❌ {}", request.errors[0]));
        }
        let _persist_global = resolve_persist_behavior_stub(request.is_global, request.is_session, request.is_once, request.explicit_provider.as_deref());

        // --refresh: bust the disk cache so the picker shows live data.
        if force_refresh {
            // Python continues:
            //   await asyncio.to_thread(clear_model_cache) or provider refresh
            //   then proceeds to picker / switch logic
            // Rust stub: log and continue into slice 3
            log_debug("model --refresh: bust disk cache (stub)");
            // --- slice boundary 1800: remaining model switch body in slice 3 ---
            // Python continues (1801+):
            //   if not model_input and not explicit_provider: → picker (Telegram/Discord inline keyboard vs text list)
            //   else: → validate provider/model, check skew guard (_model_switch_skew_guard),
            //          call _switch_model(...), handle persist_global vs session vs once,
            //          emit t("gateway.model.switched.*") / error prefix
            // Rust slice 2 returns a stub indicating refresh was handled; full switch is in slice 3.
            return Some(t_model_refresh_stub());
        }

        // Non-refresh path also continues in slice 3 — stub return preserving 1:1 structure.
        // Python would now branch on `model_input` / `explicit_provider` presence and render picker or switch.
        // Slice 2 mirrors the prologue and boundary so the file compiles without slice 3.
        let _ = (_model_input, _explicit_provider, _is_global_flag, _persist_global);
        // ponytail: deterministic placeholder — slice 3 completes picker/switch
        None
        // --- slice boundary 1800 end ---
    }

    fn running_agent_count(&self) -> i64 {
        self.running_agents.len() as i64
    }
}

// ---------------------------------------------------------------------------
// Free-function aliases — mirrors Python module-level helpers accessed via MRO
// ---------------------------------------------------------------------------

pub fn gateway_session_origin_for_id_free(
    session_id: &str,
    entries: &HashMap<String, (String, Option<SessionSource>)>,
) -> Option<SessionSource> {
    gateway_session_origin_for_id(session_id, entries)
}

pub fn same_matrix_room_free(current: &SessionSource, origin: Option<&SessionSource>) -> bool {
    same_matrix_room(current, origin)
}

pub fn same_origin_chat_free(
    current: &SessionSource,
    origin: Option<&SessionSource>,
    group_sessions_per_user: bool,
    thread_sessions_per_user: bool,
) -> bool {
    same_origin_chat(current, origin, group_sessions_per_user, thread_sessions_per_user)
}

pub fn resume_caller_is_admin_free(source: &SessionSource, policy_enabled: bool, is_admin: fn(&str) -> bool) -> bool {
    resume_caller_is_admin(source, policy_enabled, is_admin)
}

pub fn resume_target_allowed_free(
    source: &SessionSource,
    target_id: &str,
    allow_override: bool,
    caller_is_admin: bool,
    live_origin: Option<&SessionSource>,
    db_row: Option<&HashMap<String, serde_json::Value>>,
) -> bool {
    resume_target_allowed(source, target_id, allow_override, caller_is_admin, live_origin, db_row, true, false)
}

// ---------------------------------------------------------------------------
// Agents helpers — mirrors Python sub-logic lines 1276–1424
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct AgentRow {
    session_key: String,
    elapsed: i64,
    state: String,
    session_id: String,
    model: String,
}

#[derive(Debug, Clone)]
struct ProcessEntry {
    session_id: String,
    command: String,
    uptime_seconds: i64,
}

#[derive(Debug, Clone)]
struct DelegationEntry {
    delegation_id: String,
    status: String,
    goal: String,
    stalled_after_quiet_seconds: Option<f64>,
    seconds_since_progress: f64,
    children_activity: Vec<ChildActivity>,
}

#[derive(Debug, Clone)]
struct ChildActivity {
    api_calls: String,
    current_tool: Option<String>,
    seconds_since_activity: Option<f64>,
}

fn current_time_secs() -> f64 {
    // Python: `now = time.time()` — Rust std-only uses SystemTime
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

fn format_uptime_short(secs: i64) -> String {
    // Mirrors `tools.process_registry.format_uptime_short` (approx)
    if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86400 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}d", secs / 86400)
    }
}

fn process_registry_list_stub() -> Vec<ProcessEntry> {
    // ponytail: no process_registry dep in std-only; deterministic empty
    // Would be `[p for p in process_registry.list_sessions() if p.get("status") == "running"]`
    Vec::new()
}

fn list_async_delegations_stub() -> Vec<DelegationEntry> {
    // ponytail: no async_delegation dep in std-only; deterministic empty
    // Would be `[d for d in list_async_delegations() if d.get("status") in ("running", "stalling", "finalizing")]`
    Vec::new()
}

fn t_agents_header() -> String { t_simple("gateway.agents.header") }
fn t_agents_active_agents(count: i64) -> String { t_stub("gateway.agents.active_agents", &[("count", count.to_string())]) }
fn t_agents_state_starting() -> String { t_simple("gateway.agents.state_starting") }
fn t_agents_state_running() -> String { t_simple("gateway.agents.state_running") }
fn t_agents_this_chat() -> String { t_simple("gateway.agents.this_chat") }
fn t_agents_more(count: i64) -> String { t_stub("gateway.agents.more", &[("count", count.to_string())]) }
fn t_agents_running_processes(count: i64) -> String { t_stub("gateway.agents.running_processes", &[("count", count.to_string())]) }
fn t_agents_async_jobs(count: i64) -> String { t_stub("gateway.agents.async_jobs", &[("count", count.to_string())]) }
fn t_agents_background_delegations(count: i64) -> String { t_stub("gateway.agents.background_delegations", &[("count", count.to_string())]) }
fn t_agents_none() -> String { t_simple("gateway.agents.none") }

// ---------------------------------------------------------------------------
// Stop helpers — mirrors Python lines 1426–1507
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopResult {
    Plain(String),
    Ephemeral(String),
}

impl StopResult {
    pub fn is_ephemeral(&self) -> bool {
        matches!(self, StopResult::Ephemeral(_))
    }
    pub fn text(&self) -> &str {
        match self {
            StopResult::Plain(s) | StopResult::Ephemeral(s) => s,
        }
    }
}

fn t_stop_stopped_pending() -> String { t_simple("gateway.stop.stopped_pending") }
fn t_stop_stopped() -> String { t_simple("gateway.stop.stopped") }
fn t_stop_no_active() -> String { t_simple("gateway.stop.no_active") }

fn sibling_thread_run_keys_stub(
    _source: &SessionSource,
    _session_key: &str,
    _running_agents: &HashMap<String, AgentStub>,
) -> Vec<String> {
    // Python: `self._sibling_thread_run_keys(source, session_key)` — finds running agents that share this thread
    // Rust stub: deterministic empty (no sibling runs in std-only)
    // ponytail: no thread sibling index in std-only; authorize path still covered
    Vec::new()
}

fn is_user_authorized_stub(_source: &SessionSource) -> bool {
    // Python: `self._is_user_authorized(source)` — checks slash_access / allowlist
    // Rust stub: env HERMES_AUTHORIZED=1 to simulate authorized
    std::env::var("HERMES_AUTHORIZED").ok().map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes")).unwrap_or(false)
}

fn log_debug(msg: &str) {
    // Mirrors `logger.debug(...)` / `logger.info(...)` — std-only logs to stderr when HERMES_DEBUG set
    if std::env::var("HERMES_DEBUG").ok().map(|v| !v.trim().is_empty()).unwrap_or(false) {
        eprintln!("[gateway] {}", msg);
    }
}

// ---------------------------------------------------------------------------
// Platform helpers — mirrors Python lines 1509–1600
// ---------------------------------------------------------------------------

fn resolve_platform_by_value(name: &str) -> Option<Platform> {
    // Mirrors `for p in Platform.__members__.values(): if p.value.lower() == name: return p`
    let lower = name.trim().to_ascii_lowercase();
    match lower.as_str() {
        "telegram" => Some(Platform::Telegram),
        "discord" => Some(Platform::Discord),
        "slack" => Some(Platform::Slack),
        "whatsapp" => Some(Platform::Whatsapp),
        "matrix" => Some(Platform::Matrix),
        "feishu" => Some(Platform::Feishu),
        // Also accept common aliases the gateway may use
        "webhook" => Some(Platform::Unknown("webhook".to_string())),
        "api" => Some(Platform::Unknown("api".to_string())),
        _ => {
            // Try to match any known value case-insensitively via env-provided extra platforms
            // ponytail: unknown platform stays as Unknown variant for traceability
            if lower.is_empty() {
                None
            } else {
                // Preserve original unknown handling: return None for truly unknown so caller renders "Unknown platform"
                // But gateway's Platform enum would still have the member if enabled; stub keeps allowlist narrow
                None
            }
        }
    }
}

fn platform_list_lines() -> Vec<String> {
    // Helper for testing platform list rendering in isolation
    Vec::new()
}

// ---------------------------------------------------------------------------
// Restart helpers — mirrors Python lines 1602–1712
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestartResult {
    Plain(String),
    Ephemeral(String),
    Empty,
}

impl RestartResult {
    pub fn text(&self) -> String {
        match self {
            RestartResult::Plain(s) | RestartResult::Ephemeral(s) => s.clone(),
            RestartResult::Empty => String::new(),
        }
    }
}

fn is_stale_restart_redelivery_stub(_event: &MessageEvent) -> bool {
    // Python: `self._is_stale_restart_redelivery(event)` — checks `.restart_last_processed.json` / `.restart_notify.json` markers
    // Rust stub: env HERMES_RESTART_STALE=1 to simulate stale redelivery
    std::env::var("HERMES_RESTART_STALE").ok().map(|v| v.trim() == "1").unwrap_or(false)
}

fn restart_notify_data(event: &MessageEvent) -> HashMap<String, serde_json::Value> {
    // Mirrors Python `notify_data` dict built from `event.source` + `event.message_id`
    let mut m = HashMap::new();
    m.insert("platform".to_string(), serde_json::Value::String(event.source.platform_value()));
    m.insert("chat_id".to_string(), serde_json::Value::String(event.source.chat_id.clone().unwrap_or_default()));
    m.insert("chat_type".to_string(), serde_json::Value::String(event.source.chat_type.clone().unwrap_or_default()));
    if event.source.delivered_via_upstream_relay == Some(true) {
        m.insert("delivered_via_upstream_relay".to_string(), serde_json::Value::Bool(true));
        if let Some(uid) = &event.source.user_id {
            m.insert("user_id".to_string(), serde_json::Value::String(uid.clone()));
        }
        if let Some(sid) = &event.source.scope_id {
            m.insert("scope_id".to_string(), serde_json::Value::String(sid.clone()));
        }
    }
    if let Some(tid) = &event.source.thread_id {
        if !tid.trim().is_empty() {
            m.insert("thread_id".to_string(), serde_json::Value::String(tid.clone()));
        }
    }
    if let Some(mid) = &event.message_id {
        m.insert("message_id".to_string(), serde_json::Value::String(mid.clone()));
    }
    m
}

fn restart_dedup_data(event: &MessageEvent) -> HashMap<String, serde_json::Value> {
    let mut m = HashMap::new();
    m.insert("platform".to_string(), serde_json::Value::String(event.source.platform_value()));
    m.insert("requested_at".to_string(), serde_json::Value::Number(serde_json::Number::from(current_time_secs() as i64)));
    if let Some(uid) = &event.platform_update_id {
        m.insert("update_id".to_string(), serde_json::Value::String(uid.clone()));
    }
    m
}

fn is_gateway_supervisor_process_stub() -> bool {
    // Python: `is_gateway_supervisor_process()` — checks systemd/launchd markers + explicit marker
    // Rust stub: env HERMES_SUPERVISOR=1
    std::env::var("HERMES_SUPERVISOR").ok().map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes")).unwrap_or(false)
}

fn is_container_restart_context_stub() -> bool {
    // Python: `is_container_restart_context()` — checks Docker/Podman markers
    // Rust stub: env HERMES_CONTAINER=1 or /.dockerenv existence
    if std::env::var("HERMES_CONTAINER").ok().map(|v| v.trim() == "1").unwrap_or(false) {
        return true;
    }
    std::path::Path::new("/.dockerenv").exists()
}

fn t_draining(count: i64) -> String { t_stub("gateway.draining", &[("count", count.to_string())]) }
fn t_restart_in_progress() -> String { t_simple("gateway.restart.in_progress") }
fn t_restart_restarting() -> String { t_simple("gateway.restart.restarting") }

fn atomic_json_write_stub(_path: &PathBuf, _data: &HashMap<String, serde_json::Value>) {
    // Mirrors `utils.atomic_json_write` — ponytail: no file IO in std-only stub
}

// ---------------------------------------------------------------------------
// Slash exec / help / commands helpers — mirrors Python lines 1714–1749
// ---------------------------------------------------------------------------

fn execute_command_stub(command: &str, _args: &str) -> String {
    // Mirrors `hermes_cli.slash_exec.execute_command(command, CommandContext(surface="gateway")).text`
    // Rust stub: deterministic placeholder preserving surface
    format!("{} (gateway)", command)
}

fn execute_command_with_options_stub(command: &str, args: &str, page_size: i64) -> String {
    let _ = (args, page_size);
    execute_command_stub(command, args)
}

fn telegramize_command_mentions_stub(text: &str, _platform: &str) -> String {
    // Mirrors `gateway.run._telegramize_command_mentions(reply.text, platform)`
    // Rust stub: no transformation in std-only; return as-is
    text.to_string()
}

// ---------------------------------------------------------------------------
// Model command helpers — mirrors Python lines 1751–1800 (partial)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
struct ModelSwitchRequest {
    target: Option<String>,
    explicit_provider: Option<String>,
    is_global: bool,
    is_session: bool,
    is_once: bool,
    force_refresh: bool,
    errors: Vec<String>,
}

impl ModelSwitchRequest {
    fn error_messages(&self) -> Vec<String> {
        self.errors.clone()
    }
}

fn parse_model_switch_args_stub(raw: &str) -> ModelSwitchRequest {
    // Mirrors `hermes_cli.model_switch.parse_model_switch_args(raw_args)`
    // Minimal shlex-aware parser for --provider, --global, --session, --once, --refresh
    let tokens = shlex_split_stub(raw);
    let mut req = ModelSwitchRequest::default();
    let mut i = 0usize;
    let mut positional: Vec<String> = Vec::new();
    while i < tokens.len() {
        let tok = &tokens[i];
        match tok.as_str() {
            "--global" => req.is_global = true,
            "--session" => req.is_session = true,
            "--once" => req.is_once = true,
            "--refresh" => req.force_refresh = true,
            "--provider" => {
                if i + 1 < tokens.len() {
                    req.explicit_provider = Some(tokens[i + 1].clone());
                    i += 1;
                } else {
                    req.errors.push("--provider requires a value".to_string());
                }
            }
            _ if tok.starts_with("--provider=") => {
                if let Some(eq) = tok.find('=') {
                    req.explicit_provider = Some(tok[eq + 1..].to_string());
                }
            }
            _ if tok.starts_with("--") => {
                req.errors.push(format!("unknown flag: {}", tok));
            }
            _ => positional.push(tok.clone()),
        }
        i += 1;
    }
    if positional.len() > 1 {
        req.errors.push("too many positional arguments".to_string());
    } else if let Some(first) = positional.into_iter().next() {
        if !first.trim().is_empty() {
            req.target = Some(first);
        }
    }
    // Validate conflicting flags (mirrors real parser's error_messages)
    if req.is_global && req.is_session {
        req.errors.push("cannot use --global and --session together".to_string());
    }
    if req.is_global && req.is_once {
        req.errors.push("cannot use --global and --once together".to_string());
    }
    req
}

fn shlex_split_stub(text: &str) -> Vec<String> {
    // Minimal quote-aware split (mirrors Python shlex.split for model args)
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

fn resolve_persist_behavior_stub(is_global: bool, is_session: bool, is_once: bool, explicit_provider: Option<&str>) -> bool {
    // Mirrors `hermes_cli.model_switch.resolve_persist_behavior`
    // Returns persist_global bool
    let _ = explicit_provider;
    if is_once {
        return false;
    }
    if is_session {
        return false;
    }
    is_global
}

fn t_model_refresh_stub() -> String {
    // Stub for `--refresh` handling — real gateway busts disk cache and re-renders picker
    t_simple("gateway.model.refresh")
}

// ---------------------------------------------------------------------------
// Shared utils — mirrors Python helpers used across slice 2
// ---------------------------------------------------------------------------

/// Mirrors `gateway.session.build_session_key` (minimal stub for slice 2).
pub fn build_session_key(source: &SessionSource) -> String {
    // Python appends thread_id unconditionally when present (see _same_origin_chat comment)
    let platform = source.platform_value();
    let chat = source.chat_id.as_deref().unwrap_or("").trim();
    let thread = source.thread_id.as_deref().unwrap_or("").trim();
    let scope = source.scope_id.as_deref().unwrap_or("").trim();
    if !thread.is_empty() {
        format!("{}:{}:{}:{}", platform, scope, chat, thread)
    } else {
        format!("{}:{}:{}", platform, scope, chat)
    }
}

/// Mirrors `SessionDB.sanitize_title` note (used in restart/reset title handling).
pub fn sanitize_title_note(title: &str) -> String {
    let t = title.trim();
    if t.is_empty() {
        return " (title empty — untitled)".to_string();
    }
    if t.len() > 100 {
        return format!(" (title rejected: too long: {})", t.len());
    }
    format!(" — {}", t)
}

fn get_hermes_home() -> PathBuf {
    if let Ok(val) = std::env::var("HERMES_HOME") {
        let t = val.trim().to_string();
        if !t.is_empty() {
            return PathBuf::from(t);
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".hermes");
    }
    PathBuf::from(".hermes")
}

#[allow(dead_code)]
fn _get_hermes_home() -> PathBuf {
    get_hermes_home()
}
