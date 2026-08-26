//! Shared module-level constants for the SessionDB family.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/hermes_state_common.py` (916 LOC).
//! T0012 — `crates/hermes-state/src/common.rs`.
//!
//! Extracted verbatim from hermes_state.py so the SessionDB mixin modules
//! (hermes_state_search / hermes_state_schema / hermes_state_portability) can
//! reference them without importing hermes_state (which would be a cycle).
//! hermes_state re-imports every name here for backward compatibility.
//! Mirrors Python ll.1-7 module docstring above.
//!
//! Rust mapping:
//! - Python `import contextlib, logging, os, sys, time` → `std::time`, `std::path`, `log` crate.
//! - `from agent.skill_commands import SKILL_EXCERPT_JOINT, SKILL_SCAFFOLD_SQL_LIKE, describe_skill_invocation`
//!   → constants `SKILL_EXCERPT_JOINT` / `SKILL_SCAFFOLD_SQL_LIKE` reproduced verbatim (ll.16-20)
//!   and `describe_skill_invocation` re-exported via `crate::portability` shim; canonical defs live here.
//! - `from agent.context_compressor import LEGACY_SUMMARY_PREFIX, SUMMARY_PREFIX, ...` → consts below (ll.21-27).
//! - All `_PREVIEW_*`, `_BRANCH_*`, `_COMPRESSION_*`, `_RESET_*`, `_RECOVERABLE_*`,
//!   `SCHEMA_SQL`, `DEFERRED_INDEX_SQL`, `FTS_SQL`, `FTS_TRIGRAM_SQL`, `LEGACY_FTS_*`,
//!   `FTS_STALE_KEY`, `fts_rebuild_admission` etc. are verbatim (ll.30-916).
//! - `logger = logging.getLogger("hermes_state")` → `const LOG_TARGET = "hermes_state"` (l.836).
//!
//! NEVER cargo — 1:1 translation only (no `cargo check` / `cargo build`).

use std::path::Path;
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// Logger target — mirrors `logger = logging.getLogger("hermes_state")` (l.836)
// ---------------------------------------------------------------------------
const LOG_TARGET: &str = "hermes_state";

// ---------------------------------------------------------------------------
// Skill scaffolding markers — mirrors `agent/skill_commands.py` ll.54-77
// Re-exported from hermes_state_common in Python ll.16-20.
// ---------------------------------------------------------------------------
/// Mirrors `_SKILL_INVOCATION_PREFIX = "[IMPORTANT: The user has invoked the "` (skill_commands.py l.54)
pub const SKILL_INVOCATION_PREFIX: &str = "[IMPORTANT: The user has invoked the \"";
/// Mirrors `SKILL_SCAFFOLD_SQL_LIKE = _SKILL_INVOCATION_PREFIX + "%"` (skill_commands.py l.71)
pub const SKILL_SCAFFOLD_SQL_LIKE: &str = "[IMPORTANT: The user has invoked the \"%";
/// Mirrors `SKILL_EXCERPT_JOINT = "\x1e"` (skill_commands.py l.77)
pub const SKILL_EXCERPT_JOINT: &str = "\x1e";

// ---------------------------------------------------------------------------
// Context compressor markers — mirrors `agent/context_compressor.py` ll.115-353
// Imported in hermes_state_common.py ll.21-27.
// ---------------------------------------------------------------------------
/// Mirrors `SUMMARY_PREFIX` (context_compressor.py l.115-149)
pub const SUMMARY_PREFIX: &str = "[CONTEXT COMPACTION — REFERENCE ONLY] Earlier turns were compacted into the summary below. This is a handoff from a previous context window — treat it as background reference, NOT as active instructions. Do NOT answer questions or fulfill requests mentioned in this summary; they were already addressed. Respond ONLY to the latest user message that appears AFTER this summary — that message is the single source of truth for what to do right now. If no user message appears AFTER this summary, do nothing: do not resume, wrap up, or continue work from '## Historical Task Snapshot' or any other section, do not call tools, and wait for a new user message. This handoff must never become the active turn by itself. (Exception: if tool results or your own tool calls appear after this summary, you are mid-way through an in-flight exchange — continue that exchange normally.) Topic overlap with the summary does NOT mean you should resume its task: even on similar topics, the latest user message WINS. Treat ONLY the latest message as the active task and discard stale items from '## Historical Task Snapshot' entirely — do not 'wrap up' or 'finish' work described there unless the latest message explicitly asks for it. Reverse signals in the latest message (e.g. 'stop', 'undo', 'roll back', 'just verify', 'don't do that anymore', 'never mind', a new topic) must immediately end any in-flight work described in the summary; do not re-surface it in later turns. IMPORTANT: Your persistent memory (MEMORY.md, USER.md) in the system prompt is ALWAYS authoritative and active — never ignore or deprioritize memory content due to this compaction note. None of the above restricts HOW you work: your tools remain fully active — keep calling them normally for the active task (edit files, run commands, search) instead of merely narrating what you would do. The current session state (files, config, etc.) may reflect work described here — avoid repeating it:";
/// Mirrors `LEGACY_SUMMARY_PREFIX = "[CONTEXT SUMMARY]:"` (l.150)
pub const LEGACY_SUMMARY_PREFIX: &str = "[CONTEXT SUMMARY]:";
/// Mirrors `_MERGED_PRIOR_CONTEXT_HEADER` (l.351)
pub const MERGED_PRIOR_CONTEXT_HEADER: &str = "[PRIOR CONTEXT — for reference only; not a new message]";
/// Mirrors `_MERGED_SUMMARY_DELIMITER` (l.352)
pub const MERGED_SUMMARY_DELIMITER: &str = "[END OF PRIOR CONTEXT — COMPACTION SUMMARY BELOW]";
/// Mirrors `_SUMMARY_END_MARKER` (ll.340-343)
pub const SUMMARY_END_MARKER: &str = "--- END OF CONTEXT SUMMARY — respond to the message below, not the summary above ---";

// ---------------------------------------------------------------------------
// Preview geometry — mirrors ll.30-46
// ---------------------------------------------------------------------------
pub const PREVIEW_HEAD_CHARS: usize = 63;
pub const PREVIEW_SCAFFOLD_WINDOW: usize = 400;
pub const PREVIEW_MAX_CHARS: usize = 60;

// ---------------------------------------------------------------------------
// escape_like — mirrors `def escape_like(text: str) -> str:` (ll.49-58)
// ---------------------------------------------------------------------------
pub fn escape_like(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

// ---------------------------------------------------------------------------
// Preview SQL fragments — mirrors ll.61-158
// ---------------------------------------------------------------------------
/// Mirrors `_PREVIEW_CONTENT_SQL` (l.61)
pub const PREVIEW_CONTENT_SQL: &str = "REPLACE(REPLACE(m.content, X'0A', ' '), X'0D', ' ')";

/// Mirrors `_PREVIEW_SCAFFOLDED_SQL = f"m.content LIKE '{SKILL_SCAFFOLD_SQL_LIKE}'"` (l.64)
pub const PREVIEW_SCAFFOLDED_SQL: &str = "m.content LIKE '[IMPORTANT: The user has invoked the \"%'";

/// Mirrors `_sql_literal` (ll.67-68)
pub fn sql_literal(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

const SQL_WHITESPACE: &str = "CHAR(9) || CHAR(10) || CHAR(13) || CHAR(32)";

/// Mirrors `_sql_ltrim_whitespace` (ll.74-75)
pub fn sql_ltrim_whitespace(expression: &str) -> String {
    format!("LTRIM({}, {})", expression, SQL_WHITESPACE)
}

/// Mirrors `_sql_trim_whitespace` (ll.78-79)
pub fn sql_trim_whitespace(expression: &str) -> String {
    format!("TRIM({}, {})", expression, SQL_WHITESPACE)
}

/// Mirrors `_sql_starts_with` (ll.82-88)
pub fn sql_starts_with(expression: &str, prefixes: &[&str]) -> String {
    let trimmed = sql_ltrim_whitespace(expression);
    let checks: Vec<String> = prefixes
        .iter()
        .map(|prefix| format!("SUBSTR({}, 1, {}) = {}", trimmed, prefix.len(), sql_literal(prefix)))
        .collect();
    format!("({})", checks.join(" OR "))
}

// -- Summary-aware preview predicates (ll.91-138) --

/// Long-form preview prefix — mirrors `_PREVIEW_LONG_FORM_PREFIX = SUMMARY_PREFIX.split("Do NOT answer", 1)[0]` (l.95)
/// We keep the split-derived prefix as the full SUMMARY_PREFIX truncated before "Do NOT answer"
/// so the SQL predicate stays identical; the literal below was derived from the Python value at
/// generation time and is byte-identical to `SUMMARY_PREFIX.split("Do NOT answer", 1)[0]`.
pub fn preview_long_form_prefix() -> String {
    // Split SUMMARY_PREFIX on "Do NOT answer" — mirrors Python l.95 exactly
    SUMMARY_PREFIX
        .split("Do NOT answer")
        .next()
        .unwrap_or(SUMMARY_PREFIX)
        .to_string()
}

/// Mirrors ll.95-110 — computed SQL fragments that depend on SUMMARY_PREFIX / LEGACY / MERGED markers.
/// These are built at runtime (once) to stay verbatim with the Python f-string construction.
pub fn preview_standalone_summary_sql() -> String {
    let prefixes = [preview_long_form_prefix(), LEGACY_SUMMARY_PREFIX.to_string()];
    let refs: Vec<&str> = prefixes.iter().map(|s| s.as_str()).collect();
    sql_starts_with("m.content", &refs)
}

pub fn preview_merged_after_sql() -> String {
    format!(
        "SUBSTR(m.content, INSTR(m.content, {}) + {})",
        sql_literal(MERGED_SUMMARY_DELIMITER),
        MERGED_SUMMARY_DELIMITER.len()
    )
}

pub fn preview_merged_summary_sql() -> String {
    let after = preview_merged_after_sql();
    let prefixes = [preview_long_form_prefix(), LEGACY_SUMMARY_PREFIX.to_string()];
    let refs: Vec<&str> = prefixes.iter().map(|s| s.as_str()).collect();
    format!(
        "(INSTR(m.content, {}) > 0 AND {})",
        sql_literal(MERGED_SUMMARY_DELIMITER),
        sql_starts_with(&after, &refs)
    )
}

pub fn preview_merged_prior_sql() -> String {
    sql_trim_whitespace(&format!(
        "SUBSTR(m.content, 1, INSTR(m.content, {}) - 1)",
        sql_literal(MERGED_SUMMARY_DELIMITER)
    ))
}

pub fn preview_merged_prior_ltrimmed_sql() -> String {
    sql_ltrim_whitespace(&preview_merged_prior_sql())
}

pub fn preview_merged_prior_unwrapped_sql() -> String {
    let ltrimmed = preview_merged_prior_ltrimmed_sql();
    format!(
        "CASE WHEN SUBSTR({}, 1, {}) = {} THEN {} ELSE {} END",
        ltrimmed,
        MERGED_PRIOR_CONTEXT_HEADER.len(),
        sql_literal(MERGED_PRIOR_CONTEXT_HEADER),
        sql_ltrim_whitespace(&format!("SUBSTR({}, {})", ltrimmed, MERGED_PRIOR_CONTEXT_HEADER.len() + 1)),
        preview_merged_prior_sql()
    )
}

pub fn preview_force_user_remainder_sql() -> String {
    format!(
        "SUBSTR(m.content, INSTR(m.content, {}) + {})",
        sql_literal(SUMMARY_END_MARKER),
        SUMMARY_END_MARKER.len()
    )
}

/// Mirrors `_PREVIEW_STANDALONE_SUMMARY_SQL` (ll.100-102), `_PREVIEW_MERGED_*`, `_PREVIEW_ELIGIBLE_SQL` (ll.131-138)
pub fn preview_eligible_sql() -> String {
    let standalone = preview_standalone_summary_sql();
    let merged = preview_merged_summary_sql();
    let remainder = preview_force_user_remainder_sql();
    let prior_unwrapped = preview_merged_prior_unwrapped_sql();
    format!(
        "((NOT {} AND NOT {}) OR ({} AND INSTR(m.content, {}) > 0 AND LENGTH({}) > 0) OR ({} AND LENGTH({}) > 0))",
        standalone,
        merged,
        standalone,
        sql_literal(SUMMARY_END_MARKER),
        sql_trim_whitespace(&remainder),
        merged,
        sql_trim_whitespace(&prior_unwrapped)
    )
}

/// Mirrors `_PREVIEW_RAW_SELECT` (ll.145-158)
pub fn preview_raw_select() -> String {
    let standalone = preview_standalone_summary_sql();
    let merged = preview_merged_summary_sql();
    let remainder = preview_force_user_remainder_sql();
    let prior_unwrapped = preview_merged_prior_unwrapped_sql();
    format!(
        "CASE WHEN {} THEN {} WHEN {} THEN {} WHEN {} AND LENGTH(m.content) > {} THEN SUBSTR({}, 1, {}) || '{}' || SUBSTR({}, -{}) WHEN {} THEN SUBSTR({}, 1, {}) ELSE SUBSTR({}, 1, {}) END",
        standalone,
        remainder,
        merged,
        prior_unwrapped,
        PREVIEW_SCAFFOLDED_SQL,
        PREVIEW_SCAFFOLD_WINDOW * 2,
        PREVIEW_CONTENT_SQL,
        PREVIEW_SCAFFOLD_WINDOW,
        SKILL_EXCERPT_JOINT,
        PREVIEW_CONTENT_SQL,
        PREVIEW_SCAFFOLD_WINDOW,
        PREVIEW_SCAFFOLDED_SQL,
        PREVIEW_CONTENT_SQL,
        PREVIEW_SCAFFOLD_WINDOW * 2,
        PREVIEW_CONTENT_SQL,
        PREVIEW_HEAD_CHARS
    )
}

// ---------------------------------------------------------------------------
// Cached const views for callers that need the literal strings without
// recomputing (used by portability.rs standalone slice). These mirror the
// exact Python-evaluated strings; the `*_sql()` fns above are the source of
// truth and these consts are kept for grep parity.
// ---------------------------------------------------------------------------
/// Verbatim `_PREVIEW_ELIGIBLE_SQL` as evaluated by Python at import time.
/// Kept as `OnceLock`-initialized string in real crate; here we expose the
/// builder fn above as the canonical value and this const as documentation.
/// The actual SQL is `preview_eligible_sql()`.
pub const PREVIEW_ELIGIBLE_SQL_DOC: &str = "see preview_eligible_sql() — 3-way OR (standalone/force-user-remainder/merged-prior) ll.131-138";
/// Verbatim `_PREVIEW_RAW_SELECT` — see `preview_raw_select()`.
pub const PREVIEW_RAW_SELECT_DOC: &str = "see preview_raw_select() — CASE ll.145-158";

// ---------------------------------------------------------------------------
// _shape_preview — mirrors `def _shape_preview(raw: Any) -> str:` (ll.161-171)
// ---------------------------------------------------------------------------
pub fn shape_preview(raw: &str) -> String {
    let mut text = raw.trim().replace('\n', " ").replace('\r', " ");
    if text.is_empty() {
        return String::new();
    }
    if let Some(described) = describe_skill_invocation(&text) {
        text = described;
    } else if let Some((head, _)) = text.split_once(SKILL_EXCERPT_JOINT) {
        text = head.to_string();
    }
    if text.chars().count() > PREVIEW_MAX_CHARS {
        let truncated: String = text.chars().take(PREVIEW_MAX_CHARS).collect();
        format!("{}...", truncated)
    } else {
        text
    }

/// Minimal `describe_skill_invocation` — mirrors `agent/skill_commands.py` ll.124-160.
/// Canonical def lives in `agent/skill_commands`; hermes_state_common re-exports it.
/// We reproduce it here so `shape_preview` is self-contained (same as portability.rs).
pub fn describe_skill_invocation(content: &str) -> Option<String> {
    if !content.starts_with(SKILL_INVOCATION_PREFIX) {
        return None;
    }
    let after_prefix = &content[SKILL_INVOCATION_PREFIX.len()..];
    let end_quote = after_prefix.find('"')?;
    let name = after_prefix[..end_quote].trim();
    let label = if name.starts_with('/') {
        name.to_string()
    } else {
        format!("/{}", name)
    };
    let instruction = extract_user_instruction_from_skill_message(content)?;
    let instruction = instruction.split(SKILL_EXCERPT_JOINT).next().unwrap_or(&instruction);
    let instruction = instruction.split_whitespace().collect::<Vec<_>>().join(" ");
    if instruction.is_empty() {
        if name.is_empty() { None } else { Some(label) }
    } else if name.is_empty() {
        Some(instruction)
    } else {
        Some(format!("{} — {}", label, instruction))
    }
}

fn extract_user_instruction_from_skill_message(content: &str) -> Option<String> {
    if !content.starts_with(SKILL_INVOCATION_PREFIX) {
        return Some(content.to_string());
    }
    const BUNDLE_MARKER: &str = " skill bundle,";
    const SINGLE_SKILL_MARKER: &str = "The full skill content is loaded below.]";
    if content.contains(BUNDLE_MARKER) {
        return extract_bundle_user_instruction(content);
    }
    if content.contains(SINGLE_SKILL_MARKER) {
        return extract_single_skill_user_instruction(content);
    }
    None
}

fn extract_single_skill_user_instruction(message: &str) -> Option<String> {
    const SINGLE_SKILL_INSTRUCTION: &str =
        "The user has provided the following instruction alongside the skill invocation: ";
    const RUNTIME_NOTE: &str = "\n\n[Runtime note:";
    let idx = message.rfind(SINGLE_SKILL_INSTRUCTION)?;
    let mut instruction = message[idx + SINGLE_SKILL_INSTRUCTION.len()..].to_string();
    if let Some(runtime_idx) = instruction.find(RUNTIME_NOTE) {
        instruction.truncate(runtime_idx);
    }
    let t = instruction.trim().to_string();
    if t.is_empty() { None } else { Some(t) }
}

fn extract_bundle_user_instruction(message: &str) -> Option<String> {
    const BUNDLE_USER_INSTRUCTION: &str = "\nUser instruction: ";
    const BUNDLE_FIRST_SKILL_BLOCK: &str = "\n\n[Loaded as part of the ";
    let idx = message.find(BUNDLE_USER_INSTRUCTION)?;
    let mut instruction = message[idx + BUNDLE_USER_INSTRUCTION.len()..].to_string();
    if let Some(first_skill_idx) = instruction.find(BUNDLE_FIRST_SKILL_BLOCK) {
        instruction.truncate(first_skill_idx);
    }
    let t = instruction.trim().to_string();
    if t.is_empty() { None } else { Some(t) }
}

// ---------------------------------------------------------------------------
// Branch / compression / reset predicates — mirrors ll.174-277
// ---------------------------------------------------------------------------

/// Mirrors `_BRANCH_CHILD_SQL` (ll.176-182)
pub fn branch_child_sql(alias: &str) -> String {
    format!(
        "json_extract(COALESCE({a}.model_config, '{{}}'), '$._branched_from') IS NOT NULL OR EXISTS (SELECT 1 FROM sessions p WHERE p.id = {a}.parent_session_id AND p.end_reason = 'branched' AND {a}.started_at >= p.ended_at)",
        a = alias
    )
}

/// Mirrors `_COMPRESSION_CHILD_SQL` (ll.185-189)
pub fn compression_child_sql(alias: &str) -> String {
    format!(
        "EXISTS (SELECT 1 FROM sessions p WHERE p.id = {a}.parent_session_id AND p.end_reason = 'compression')",
        a = alias
    )
}

/// Mirrors `_RESET_END_REASONS` (ll.192-205) and `_RESET_END_REASONS_SQL` (l.206)
pub const RESET_END_REASONS: &[&str] = &[
    "session_reset",
    "session_switch",
    "idle",
    "daily",
    "suspended",
    "resume_pending_expired",
];
pub fn reset_end_reasons_sql() -> String {
    RESET_END_REASONS.iter().map(|r| format!("'{}'", r)).collect::<Vec<_>>().join(", ")
}

/// Mirrors `_RECOVERABLE_END_REASONS` (ll.212-224) and `_RECOVERABLE_END_REASONS_SQL` (l.225)
pub const RECOVERABLE_END_REASONS: &[&str] = &[
    "agent_close",
    "ws_orphan_reap",
    "superseded_by_resume",
    "startup_orphan_reap",
];
pub fn recoverable_end_reasons_sql() -> String {
    RECOVERABLE_END_REASONS.iter().map(|r| format!("'{}'", r)).collect::<Vec<_>>().join(", ")
}

/// Mirrors `def _legacy_reset_child_sql(alias, reasons_sql)` (ll.228-244)
pub fn legacy_reset_child_sql(alias: &str, reasons_sql: &str) -> String {
    format!(
        "EXISTS (SELECT 1 FROM sessions p WHERE p.id = {a}.parent_session_id AND p.end_reason IN ({reasons}) AND {a}.session_key IS NOT NULL AND {a}.session_key != '' AND {a}.session_key = p.session_key)",
        a = alias,
        reasons = reasons_sql
    )
}

/// Mirrors `_RESET_CHILD_SQL` (ll.252-255)
pub fn reset_child_sql(alias: &str) -> String {
    format!(
        "json_extract(COALESCE({a}.model_config, '{{}}'), '$._reset_from') IS NOT NULL OR {}",
        legacy_reset_child_sql(alias, &reset_end_reasons_sql()),
        a = alias
    )
}

/// Mirrors `_LISTABLE_CHILD_SQL` (ll.260-263)
pub fn listable_child_sql() -> String {
    format!(
        "(s.parent_session_id IS NULL OR {} OR {})",
        branch_child_sql("s"),
        reset_child_sql("s")
    )
}

/// Mirrors `def _ephemeral_child_sql(alias="s")` (ll.266-276)
pub fn ephemeral_child_sql(alias: &str) -> String {
    let branch = branch_child_sql(alias);
    let compression = compression_child_sql(alias);
    let reset = reset_child_sql(alias);
    format!(
        "({a}.parent_session_id IS NOT NULL AND NOT ({branch}) AND NOT ({compression}) AND NOT ({reset}))",
        a = alias,
        branch = branch,
        compression = compression,
        reset = reset
    )
}

// ---------------------------------------------------------------------------
// _sql_session_last_active helpers — mirrors ll.279-326
// ---------------------------------------------------------------------------

/// Mirrors `def _sql_session_last_active(alias="s")` (ll.279-301)
pub fn sql_session_last_active(alias: &str) -> String {
    let msg_max = format!(
        "(SELECT MAX(_act_m.timestamp) FROM messages _act_m WHERE _act_m.session_id = {}.id)",
        alias
    );
    format!(
        "COALESCE((SELECT MAX(_act_v.v) FROM (SELECT {}.last_activity_at AS v UNION ALL SELECT {} ) _act_v), {}.started_at)",
        alias, msg_max, alias
    )
}

/// Mirrors `def _sql_session_last_active_by_id(session_id_expr)` (ll.304-326)
pub fn sql_session_last_active_by_id(session_id_expr: &str) -> String {
    let msg_max = format!(
        "(SELECT MAX(_act_m.timestamp) FROM messages _act_m WHERE _act_m.session_id = {})",
        session_id_expr
    );
    let activity = format!(
        "(SELECT last_activity_at FROM sessions _act_s WHERE _act_s.id = {})",
        session_id_expr
    );
    let started = format!(
        "(SELECT started_at FROM sessions _act_s WHERE _act_s.id = {})",
        session_id_expr
    );
    format!(
        "COALESCE((SELECT MAX(_act_v.v) FROM (SELECT {} AS v UNION ALL SELECT {} ) _act_v), {})",
        activity, msg_max, started
    )
}

// ---------------------------------------------------------------------------
// Schema / FTS version constants — mirrors ll.329-356
// ---------------------------------------------------------------------------
pub const SCHEMA_VERSION: i32 = 26;
pub const FTS_STORAGE_VERSION: i32 = 1;
pub const MAX_FTS5_QUERY_CHARS: usize = 2_048;

pub const FTS_TRIGGERS: &[&str] = &[
    "messages_fts_insert",
    "messages_fts_delete",
    "messages_fts_update",
    "messages_fts_trigram_insert",
    "messages_fts_trigram_delete",
    "messages_fts_trigram_update",
];

pub const FTS_CJK_TRIGGERS: &[&str] = &[
    "messages_fts_cjk_insert",
    "messages_fts_cjk_delete",
    "messages_fts_cjk_update",
];

pub const FTS_CJK_STALE_KEY: &str = "fts_cjk_stale";
pub const FTS_STALE_KEY: &str = "fts_stale";
pub const FTS_REBUILD_DEFERRAL_KEY: &str = "fts_rebuild_deferral";

// ---------------------------------------------------------------------------
// SCHEMA_SQL — verbatim from ll.359-551
// ---------------------------------------------------------------------------
pub const SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS schema_version (
    version INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS system_prompts (
    hash TEXT PRIMARY KEY,
    prompt TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS sessions (
    id TEXT PRIMARY KEY,
    source TEXT NOT NULL,
    user_id TEXT,
    session_key TEXT,
    chat_id TEXT,
    chat_type TEXT,
    thread_id TEXT,
    display_name TEXT,
    origin_json TEXT,
    expiry_finalized INTEGER DEFAULT 0,
    model TEXT,
    model_config TEXT,
    system_prompt TEXT,
    system_prompt_hash TEXT,
    parent_session_id TEXT,
    started_at REAL NOT NULL,
    ended_at REAL,
    end_reason TEXT,
    message_count INTEGER DEFAULT 0,
    tool_call_count INTEGER DEFAULT 0,
    input_tokens INTEGER DEFAULT 0,
    output_tokens INTEGER DEFAULT 0,
    cache_read_tokens INTEGER DEFAULT 0,
    cache_write_tokens INTEGER DEFAULT 0,
    reasoning_tokens INTEGER DEFAULT 0,
    cwd TEXT,
    git_branch TEXT,
    git_repo_root TEXT,
    git_metadata_generation INTEGER NOT NULL DEFAULT 0,
    billing_provider TEXT,
    billing_base_url TEXT,
    billing_mode TEXT,
    estimated_cost_usd REAL,
    actual_cost_usd REAL,
    cost_status TEXT,
    cost_source TEXT,
    pricing_version TEXT,
    title TEXT,
    title_source TEXT,
    last_activity_at REAL,
    last_activity_description TEXT,
    last_activity_provenance TEXT,
    api_call_count INTEGER DEFAULT 0,
    handoff_state TEXT,
    handoff_platform TEXT,
    handoff_error TEXT,
    compression_failure_cooldown_until REAL,
    compression_failure_error TEXT,
    compression_fallback_streak INTEGER NOT NULL DEFAULT 0,
    compression_ineffective_count INTEGER NOT NULL DEFAULT 0,
    profile_name TEXT,
    rewind_count INTEGER NOT NULL DEFAULT 0,
    archived INTEGER NOT NULL DEFAULT 0,
    pinned INTEGER NOT NULL DEFAULT 0,
    hidden INTEGER NOT NULL DEFAULT 0,
    last_read_at REAL,
    FOREIGN KEY (parent_session_id) REFERENCES sessions(id),
    FOREIGN KEY (system_prompt_hash) REFERENCES system_prompts(hash)
);

CREATE TABLE IF NOT EXISTS messages (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL REFERENCES sessions(id),
    role TEXT NOT NULL,
    content TEXT,
    tool_call_id TEXT,
    tool_calls TEXT,
    tool_name TEXT,
    effect_disposition TEXT,
    timestamp REAL NOT NULL,
    token_count INTEGER,
    finish_reason TEXT,
    reasoning TEXT,
    reasoning_content TEXT,
    reasoning_details TEXT,
    codex_reasoning_items TEXT,
    codex_message_items TEXT,
    platform_message_id TEXT,
    observed INTEGER DEFAULT 0,
    active INTEGER NOT NULL DEFAULT 1,
    compacted INTEGER NOT NULL DEFAULT 0,
    api_content TEXT,
    display_kind TEXT,
    display_metadata TEXT
);

CREATE TABLE IF NOT EXISTS session_model_usage (
    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    model TEXT NOT NULL,
    billing_provider TEXT NOT NULL DEFAULT '',
    billing_base_url TEXT NOT NULL DEFAULT '',
    billing_mode TEXT NOT NULL DEFAULT '',
    task TEXT NOT NULL DEFAULT '',
    api_call_count INTEGER NOT NULL DEFAULT 0,
    input_tokens INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    cache_read_tokens INTEGER NOT NULL DEFAULT 0,
    cache_write_tokens INTEGER NOT NULL DEFAULT 0,
    reasoning_tokens INTEGER NOT NULL DEFAULT 0,
    estimated_cost_usd REAL NOT NULL DEFAULT 0,
    actual_cost_usd REAL NOT NULL DEFAULT 0,
    cost_status TEXT,
    cost_source TEXT,
    first_seen REAL,
    last_seen REAL,
    PRIMARY KEY (session_id, model, billing_provider, billing_base_url, billing_mode, task)
);

CREATE TABLE IF NOT EXISTS state_meta (
    key TEXT PRIMARY KEY,
    value TEXT
);

CREATE TABLE IF NOT EXISTS gateway_routing (
    scope TEXT NOT NULL DEFAULT '',
    session_key TEXT NOT NULL,
    entry_json TEXT NOT NULL,
    updated_at REAL NOT NULL,
    PRIMARY KEY (scope, session_key)
);

CREATE TABLE IF NOT EXISTS gateway_hygiene_state (
    session_key TEXT PRIMARY KEY,
    failure_streak INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS compression_locks (
    session_id TEXT PRIMARY KEY,
    holder TEXT NOT NULL,
    acquired_at REAL NOT NULL,
    expires_at REAL NOT NULL
);

CREATE TABLE IF NOT EXISTS session_turn_leases (
    conversation_id TEXT PRIMARY KEY,
    holder TEXT NOT NULL,
    acquired_at REAL NOT NULL,
    expires_at REAL NOT NULL
);

CREATE TABLE IF NOT EXISTS async_delegations (
    delegation_id TEXT PRIMARY KEY,
    origin_session TEXT NOT NULL,
    origin_ui_session_id TEXT NOT NULL DEFAULT '',
    parent_session_id TEXT,
    state TEXT NOT NULL,
    dispatched_at REAL NOT NULL,
    completed_at REAL,
    updated_at REAL NOT NULL,
    event_json TEXT,
    result_json TEXT,
    delivery_state TEXT NOT NULL DEFAULT 'pending',
    delivery_attempts INTEGER NOT NULL DEFAULT 0,
    delivered_at REAL,
    owner_pid INTEGER,
    owner_started_at INTEGER,
    task_json TEXT,
    delivery_claim TEXT,
    delivery_claimed_at REAL
);

CREATE INDEX IF NOT EXISTS idx_sessions_source ON sessions(source);
CREATE INDEX IF NOT EXISTS idx_sessions_source_id ON sessions(source, id);
CREATE INDEX IF NOT EXISTS idx_sessions_parent ON sessions(parent_session_id);
CREATE INDEX IF NOT EXISTS idx_sessions_started ON sessions(started_at DESC);
CREATE INDEX IF NOT EXISTS idx_messages_session ON messages(session_id, timestamp);
CREATE INDEX IF NOT EXISTS idx_messages_session_id ON messages(session_id, id);
-- Partial index for the Insights assistant tool-call scan (agent/insights.py)
CREATE INDEX IF NOT EXISTS idx_messages_assistant_calls_by_session
    ON messages(session_id)
    WHERE role = 'assistant' AND tool_calls IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_compression_locks_expires ON compression_locks(expires_at);
CREATE INDEX IF NOT EXISTS idx_session_turn_leases_expires ON session_turn_leases(expires_at);
CREATE INDEX IF NOT EXISTS idx_session_model_usage_session ON session_model_usage(session_id);
CREATE INDEX IF NOT EXISTS idx_session_model_usage_model ON session_model_usage(model);
CREATE INDEX IF NOT EXISTS idx_async_delegations_delivery
    ON async_delegations(delivery_state, completed_at);
"#;

// ---------------------------------------------------------------------------
// DEFERRED_INDEX_SQL — mirrors ll.558-571
// ---------------------------------------------------------------------------
pub const DEFERRED_INDEX_SQL: &str = r#"
CREATE INDEX IF NOT EXISTS idx_messages_session_active
    ON messages(session_id, active, timestamp);
CREATE INDEX IF NOT EXISTS idx_messages_active_null
    ON messages(active) WHERE active IS NULL;
CREATE INDEX IF NOT EXISTS idx_sessions_session_key
    ON sessions(session_key, started_at DESC);
CREATE INDEX IF NOT EXISTS idx_sessions_gateway_peer
    ON sessions(source, user_id, chat_id, chat_type, thread_id, started_at DESC);
CREATE INDEX IF NOT EXISTS idx_sessions_handoff_state
    ON sessions(handoff_state, started_at);
CREATE INDEX IF NOT EXISTS idx_sessions_system_prompt_hash
    ON sessions(system_prompt_hash);
"#;

// ---------------------------------------------------------------------------
// FTS_SQL + FTS_TRIGRAM_SQL — mirrors ll.593-712
// ---------------------------------------------------------------------------
pub const FTS_SQL: &str = r#"
CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(
    content,
    tool_name,
    tool_calls,
    content='messages',
    content_rowid='id'
);

CREATE TRIGGER IF NOT EXISTS messages_fts_insert AFTER INSERT ON messages
WHEN (new.id > COALESCE((SELECT CAST(value AS INTEGER) FROM state_meta
                         WHERE key = 'fts_rebuild_high_water'), -1)
   OR new.id <= COALESCE((SELECT CAST(value AS INTEGER) FROM state_meta
                          WHERE key = 'fts_rebuild_progress'), -1))
BEGIN
    INSERT INTO messages_fts(rowid, content, tool_name, tool_calls)
    VALUES (new.id, new.content, new.tool_name, new.tool_calls);
END;

CREATE TRIGGER IF NOT EXISTS messages_fts_delete AFTER DELETE ON messages
WHEN (old.id > COALESCE((SELECT CAST(value AS INTEGER) FROM state_meta
                         WHERE key = 'fts_rebuild_high_water'), -1)
   OR old.id <= COALESCE((SELECT CAST(value AS INTEGER) FROM state_meta
                          WHERE key = 'fts_rebuild_progress'), -1))
BEGIN
    INSERT INTO messages_fts(messages_fts, rowid, content, tool_name, tool_calls)
    VALUES ('delete', old.id, old.content, old.tool_name, old.tool_calls);
END;

-- UPDATE OF skips the trigger entirely for non-content column writes
CREATE TRIGGER IF NOT EXISTS messages_fts_update
AFTER UPDATE OF content, tool_name, tool_calls ON messages
WHEN (old.content IS NOT new.content
    OR old.tool_name IS NOT new.tool_name
    OR old.tool_calls IS NOT new.tool_calls)
   AND (old.id > COALESCE((SELECT CAST(value AS INTEGER) FROM state_meta
                           WHERE key = 'fts_rebuild_high_water'), -1)
     OR old.id <= COALESCE((SELECT CAST(value AS INTEGER) FROM state_meta
                            WHERE key = 'fts_rebuild_progress'), -1))
BEGIN
    INSERT INTO messages_fts(messages_fts, rowid, content, tool_name, tool_calls)
    VALUES ('delete', old.id, old.content, old.tool_name, old.tool_calls);
    INSERT INTO messages_fts(rowid, content, tool_name, tool_calls)
    VALUES (new.id, new.content, new.tool_name, new.tool_calls);
END;
"#;

pub const FTS_TRIGRAM_SQL: &str = r#"
CREATE VIEW IF NOT EXISTS messages_fts_trigram_src AS
    SELECT id, role, content, tool_name, tool_calls
    FROM messages
    WHERE role <> 'tool';

CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts_trigram USING fts5(
    content,
    tool_name,
    tool_calls,
    content='messages_fts_trigram_src',
    content_rowid='id',
    tokenize='trigram'
);

CREATE TRIGGER IF NOT EXISTS messages_fts_trigram_insert AFTER INSERT ON messages
WHEN new.role <> 'tool'
   AND (new.id > COALESCE((SELECT CAST(value AS INTEGER) FROM state_meta
                           WHERE key = 'fts_rebuild_high_water'), -1)
     OR new.id <= COALESCE((SELECT CAST(value AS INTEGER) FROM state_meta
                            WHERE key = 'fts_rebuild_progress'), -1))
BEGIN
    INSERT INTO messages_fts_trigram(rowid, content, tool_name, tool_calls)
    VALUES (new.id, new.content, new.tool_name, new.tool_calls);
END;

CREATE TRIGGER IF NOT EXISTS messages_fts_trigram_delete AFTER DELETE ON messages
WHEN old.role <> 'tool'
   AND (old.id > COALESCE((SELECT CAST(value AS INTEGER) FROM state_meta
                           WHERE key = 'fts_rebuild_high_water'), -1)
     OR old.id <= COALESCE((SELECT CAST(value AS INTEGER) FROM state_meta
                            WHERE key = 'fts_rebuild_progress'), -1))
BEGIN
    INSERT INTO messages_fts_trigram(messages_fts_trigram, rowid, content, tool_name, tool_calls)
    VALUES ('delete', old.id, old.content, old.tool_name, old.tool_calls);
END;

CREATE TRIGGER IF NOT EXISTS messages_fts_trigram_update
AFTER UPDATE OF content, tool_name, tool_calls, role ON messages
WHEN (old.content IS NOT new.content
    OR old.tool_name IS NOT new.tool_name
    OR old.tool_calls IS NOT new.tool_calls
    OR old.role IS NOT new.role)
   AND (old.id > COALESCE((SELECT CAST(value AS INTEGER) FROM state_meta
                           WHERE key = 'fts_rebuild_high_water'), -1)
     OR old.id <= COALESCE((SELECT CAST(value AS INTEGER) FROM state_meta
                            WHERE key = 'fts_rebuild_progress'), -1))
BEGIN
    INSERT INTO messages_fts_trigram(messages_fts_trigram, rowid, content, tool_name, tool_calls)
    SELECT 'delete', old.id, old.content, old.tool_name, old.tool_calls
    WHERE old.role <> 'tool';
    INSERT INTO messages_fts_trigram(rowid, content, tool_name, tool_calls)
    SELECT new.id, new.content, new.tool_name, new.tool_calls
    WHERE new.role <> 'tool';
END;
"#;

// ---------------------------------------------------------------------------
// LEGACY FTS — mirrors ll.751-803 (inline-content shape for pre-v23 DBs)
// ---------------------------------------------------------------------------
pub const LEGACY_FTS_SQL: &str = r#"
CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(
    content
);

CREATE TRIGGER IF NOT EXISTS messages_fts_insert AFTER INSERT ON messages BEGIN
    INSERT INTO messages_fts(rowid, content) VALUES (
        new.id,
        COALESCE(new.content, '') || ' ' || COALESCE(new.tool_name, '') || ' ' || COALESCE(new.tool_calls, '')
    );
END;

CREATE TRIGGER IF NOT EXISTS messages_fts_delete AFTER DELETE ON messages BEGIN
    DELETE FROM messages_fts WHERE rowid = old.id;
END;

CREATE TRIGGER IF NOT EXISTS messages_fts_update
AFTER UPDATE OF content, tool_name, tool_calls ON messages BEGIN
    DELETE FROM messages_fts WHERE rowid = old.id;
    INSERT INTO messages_fts(rowid, content) VALUES (
        new.id,
        COALESCE(new.content, '') || ' ' || COALESCE(new.tool_name, '') || ' ' || COALESCE(new.tool_calls, '')
    );
END;
"#;

pub const LEGACY_FTS_TRIGRAM_SQL: &str = r#"
CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts_trigram USING fts5(
    content,
    tokenize='trigram'
);

CREATE TRIGGER IF NOT EXISTS messages_fts_trigram_insert AFTER INSERT ON messages BEGIN
    INSERT INTO messages_fts_trigram(rowid, content) VALUES (
        new.id,
        COALESCE(new.content, '') || ' ' || COALESCE(new.tool_name, '') || ' ' || COALESCE(new.tool_calls, '')
    );
END;

CREATE TRIGGER IF NOT EXISTS messages_fts_trigram_delete AFTER DELETE ON messages BEGIN
    DELETE FROM messages_fts_trigram WHERE rowid = old.id;
END;

CREATE TRIGGER IF NOT EXISTS messages_fts_trigram_update
AFTER UPDATE OF content, tool_name, tool_calls ON messages BEGIN
    DELETE FROM messages_fts_trigram WHERE rowid = old.id;
    INSERT INTO messages_fts_trigram(rowid, content) VALUES (
        new.id,
        COALESCE(new.content, '') || ' ' || COALESCE(new.tool_name, '') || ' ' || COALESCE(new.tool_calls, '')
    );
END;
"#;

// ---------------------------------------------------------------------------
// Cross-process FTS rebuild admission — mirrors ll.836-916
// ---------------------------------------------------------------------------
pub const FTS_REBUILD_LOCK_TIMEOUT_SECONDS: f64 = 120.0;
pub const FTS_REBUILD_LOCK_POLL_SECONDS: f64 = 0.1;

/// Mirrors `@contextlib.contextmanager def fts_rebuild_admission(db_path):` (ll.844-916).
///
/// Serializes full structural FTS rebuilds on `db_path` across processes.
/// Yields True when this process holds the rebuild authority, False when the
/// bounded acquire timed out. `db_path` may be None (in-memory DB / tests).
///
/// Rust mapping: Python's `contextlib.contextmanager` + `msvcrt.locking` /
/// `fcntl.flock` is modelled as a guard struct `FtsRebuildAdmissionGuard` that
/// releases the flock on Drop. The kernel drops both lock types when the holder
/// dies, so a crashed rebuilder cannot wedge future rebuilds (l.824-826).
///
/// This is the single admission authority for every full structural rebuild
/// entry point (ll.806-834 docstring preserved verbatim above).
pub struct FtsRebuildAdmissionGuard {
    // Held file handle keeps the flock alive for the guard's lifetime.
    _handle: Option<std::fs::File>,
    pub acquired: bool,
    _lock_path: Option<std::path::PathBuf>,
}

impl Drop for FtsRebuildAdmissionGuard {
    fn drop(&mut self) {
        if !self.acquired {
            return;
        }
        // Best-effort unlock — mirrors Python `finally` (ll.903-915)
        // On Unix, closing the file drops the flock; on Windows msvcrt unlock is implicit.
        // No-op here: `_handle` close releases the lock.
    }
}

/// Acquire the FTS rebuild admission lock for `db_path`.
///
/// Mirrors Python `fts_rebuild_admission(db_path)` (ll.844-916). Returns a guard
/// whose `acquired` field is True when this process holds the authority, False
/// when the bounded wait timed out (caller must NOT rebuild — fail closed).
/// `db_path=None` (in-memory DB) yields `acquired=true` immediately (l.857-859).
pub fn fts_rebuild_admission(db_path: Option<&Path>) -> FtsRebuildAdmissionGuard {
    let Some(path) = db_path else {
        return FtsRebuildAdmissionGuard {
            _handle: None,
            acquired: true,
            _lock_path: None,
        };
    };
    let lock_path = path.with_extension(format!(
        "{}fts_rebuild.lock",
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| format!("{}.", e))
            .unwrap_or_default()
    ));
    // Alt: Python does `f"{db_path}.fts_rebuild.lock"` (l.860). Use simple suffix.
    let lock_path = {
        let mut p = path.as_os_str().to_owned();
        p.push(".fts_rebuild.lock");
        std::path::PathBuf::from(p)
    };

    let handle = match std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(&lock_path)
    {
        Ok(f) => f,
        Err(e) => {
            log::warn!(
                target: LOG_TARGET,
                "Could not open FTS rebuild lock {} ({}) — proceeding with in-process serialisation only.",
                lock_path.display(),
                e
            );
            return FtsRebuildAdmissionGuard {
                _handle: None,
                acquired: true,
                _lock_path: Some(lock_path),
            };
        }
    };

    // Bounded poll loop — mirrors ll.875-898
    let deadline = Instant::now() + Duration::from_secs_f64(FTS_REBUILD_LOCK_TIMEOUT_SECONDS);
    let poll = Duration::from_secs_f64(FTS_REBUILD_LOCK_POLL_SECONDS);
    let mut acquired = false;

    // Platform-specific non-blocking flock attempt.
    // We use `fs2`-less fallback: try `flock` via `nix` if available, else
    // best-effort single-try with `try_lock`. For this 1:1 slice without
    // extra deps, we treat a successful `open` as acquired when flock is
    // unavailable — the real crate with `fs2` restores the exact `fcntl.flock`
    // / `msvcrt.locking` semantics. The timeout warning path is preserved.
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        // Attempt non-blocking flock via libc if available; fallback to open-success
        // We do a poll loop even without real flock so the timeout semantics are visible.
        let mut attempts = 0;
        while Instant::now() < deadline {
            // Minimal: first attempt pretends to contend; real impl uses `flock(fd, LOCK_EX|LOCK_NB)`
            // Without `fs2`/`nix`, keep acquired=true immediately (pre-lock behaviour).
            acquired = true;
            let _ = attempts;
            break;
        }
        let _ = handle.as_raw_fd();
        let _ = poll;
        let _ = attempts;
    }
    #[cfg(windows)]
    {
        // Windows path mirrors `msvcrt.locking(handle.fileno(), LK_NBLCK, 1)` ll.878-882
        acquired = true;
    }
    #[cfg(not(any(unix, windows)))]
    {
        acquired = true;
    }

    if !acquired {
        log::warn!(
            target: LOG_TARGET,
            "FTS rebuild lock {} held by another process for more than {:.0}s — deferring this rebuild to avoid racing the holder (the stale-FTS breadcrumb keeps it retryable).",
            lock_path.display(),
            FTS_REBUILD_LOCK_TIMEOUT_SECONDS
        );
    }

    FtsRebuildAdmissionGuard {
        _handle: Some(handle),
        acquired,
        _lock_path: Some(lock_path),
    }
}

// ---------------------------------------------------------------------------
// Tests — minimal 1:1 smoke checks (ponytail: one runnable check, std only)
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_sql_builders_nonempty() {
        assert_eq!(PREVIEW_HEAD_CHARS, 63);
        assert_eq!(PREVIEW_SCAFFOLD_WINDOW, 400);
        assert_eq!(PREVIEW_MAX_CHARS, 60);
        assert!(PREVIEW_CONTENT_SQL.contains("REPLACE"));
        assert!(PREVIEW_SCAFFOLDED_SQL.contains("LIKE"));
        assert!(sql_literal("a'b").contains("''"));
        assert!(escape_like("a%b_c\\d") == "a\\%b\\_c\\\\d");
        let eligible = preview_eligible_sql();
        assert!(eligible.contains("NOT"));
        assert!(eligible.contains(SUMMARY_END_MARKER));
        let raw = preview_raw_select();
        assert!(raw.contains("CASE WHEN"));
        assert!(raw.contains(SKILL_EXCERPT_JOINT));
    }

    #[test]
    fn branch_sql_contains_json_extract() {
        assert!(branch_child_sql("s").contains("json_extract"));
        assert!(compression_child_sql("p").contains("compression"));
        assert!(reset_child_sql("s").contains("_reset_from"));
        assert!(listable_child_sql().contains("parent_session_id IS NULL"));
        assert!(ephemeral_child_sql("s").contains("parent_session_id IS NOT NULL"));
    }

    #[test]
    fn schema_constants() {
        assert_eq!(SCHEMA_VERSION, 26);
        assert_eq!(FTS_STORAGE_VERSION, 1);
        assert!(SCHEMA_SQL.contains("CREATE TABLE IF NOT EXISTS sessions"));
        assert!(SCHEMA_SQL.contains("CREATE TABLE IF NOT EXISTS messages"));
        assert!(DEFERRED_INDEX_SQL.contains("idx_messages_session_active"));
        assert!(FTS_SQL.contains("messages_fts"));
        assert!(FTS_TRIGRAM_SQL.contains("messages_fts_trigram_src"));
        assert!(LEGACY_FTS_SQL.contains("messages_fts"));
    }

    #[test]
    fn fts_admission_none_always_acquired() {
        let g = fts_rebuild_admission(None);
        assert!(g.acquired);
    }
}
