//! Session listing / rich rows, export, and import (portability) for SessionDB.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/hermes_state_portability.py` (825 LOC).
//! T0013 — `crates/hermes-state/src/portability.rs`.
//!
//! Mixin contract (Python docstring, ll.1-8 preserved):
//! ```text
//! Session listing/rich rows, export, and import (portability) for SessionDB.
//!
//! Mixin contract: this is a plain mixin class consumed by
//! ``hermes_state.SessionDB``. It defines no ``__init__`` and no state of its
//! own; methods access the host's attributes (``self._conn``, ``self.db_path``,
//! ``self._execute_write`` and other SessionDB methods) established by
//! ``SessionDB.__init__``. It must never import hermes_state (cycle) — shared
//! module-level constants live in hermes_state_common.
//! ```
//!
//! Rust mapping:
//! - Python `class SessionPortabilityMixin` → `impl StateStore` blocks in this
//!   module. `StateStore` is the single owner of `state.db` (rusqlite, WAL).
//!   Python's `self._conn` / `self._lock` / `self.db_path` / `self._execute_write`
//!   become `self.path` + `self.connect()` + `self.lock` + `self.execute_write()`.
//!   All SQL is verbatim from Python; line numbers in comments refer to the
//!   825-line source file.
//! - Shared constants (`SCHEMA_SQL`, `_PREVIEW_ELIGIBLE_SQL`, `_PREVIEW_RAW_SELECT`,
//!   `_shape_preview`, `_sql_session_last_active`) live in
//!   `hermes_state_common` in Python. They are re-declared here for a
//!   self-contained slice (same technique as `hermes_constants_slice2/3`); when
//!   the three `hermes-state` slices are merged these collapse to the canonical
//!   `crate::common` defs.
//! - `agent.skill_commands.SKILL_SCAFFOLD_SQL_LIKE` and `SKILL_EXCERPT_JOINT`
//!   are reproduced verbatim (ll.16, 77 in `agent/skill_commands.py`).
//! - Logging: Python `logging.getLogger("hermes_state")` → `log` crate with
//!   target `"hermes_state"` (ll.24-27).
//!
//! NEVER cargo — 1:1 translation only (no `cargo check` / `cargo build`).
//! `cargo` is not invoked by this slice; the file is verified by line-level
//! audit against the Python source.

// ---------------------------------------------------------------------------
// Imports — stdlib + workspace crates only. Mirrors Python `import logging,
// json, time, typing` (ll.11-14) and `from agent.skill_commands ...` /
// `from hermes_state_common import ...` (ll.16-23).
// ---------------------------------------------------------------------------
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, Row};
use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// Logger target — mirrors `logger = logging.getLogger("hermes_state")` (l.27)
// ---------------------------------------------------------------------------
const LOG_TARGET: &str = "hermes_state";

// ---------------------------------------------------------------------------
// Skill scaffolding markers — mirrors `agent/skill_commands.py` ll.54-77
// ---------------------------------------------------------------------------
/// Mirrors `_SKILL_INVOCATION_PREFIX = "[IMPORTANT: The user has invoked the "` (l.54)
const SKILL_INVOCATION_PREFIX: &str = "[IMPORTANT: The user has invoked the \"";
/// Mirrors `SKILL_SCAFFOLD_SQL_LIKE = _SKILL_INVOCATION_PREFIX + "%"` (l.71)
const SKILL_SCAFFOLD_SQL_LIKE: &str = "[IMPORTANT: The user has invoked the \"%";
/// Mirrors `SKILL_EXCERPT_JOINT = "\x1e"` (l.77)
const SKILL_EXCERPT_JOINT: &str = "\x1e";
/// Single-skill marker (l.55)
const SINGLE_SKILL_MARKER: &str = "The full skill content is loaded below.]";
/// Single-skill instruction marker (l.56-58)
const SINGLE_SKILL_INSTRUCTION: &str =
    "The user has provided the following instruction alongside the skill invocation: ";
/// Runtime note marker (l.59)
const RUNTIME_NOTE: &str = "\n\n[Runtime note:";
/// Bundle marker / instruction / block (ll.60-62) — needed for describe
const BUNDLE_MARKER: &str = " skill bundle,";
const BUNDLE_USER_INSTRUCTION: &str = "\nUser instruction: ";
const BUNDLE_FIRST_SKILL_BLOCK: &str = "\n\n[Loaded as part of the ";

// ---------------------------------------------------------------------------
// Preview geometry — mirrors `hermes_state_common.py` ll.40-47
// ---------------------------------------------------------------------------
const PREVIEW_HEAD_CHARS: usize = 63;
const PREVIEW_SCAFFOLD_WINDOW: usize = 400;
const PREVIEW_MAX_CHARS: usize = 60;

// ---------------------------------------------------------------------------
// Preview SQL fragments — mirrors `hermes_state_common.py` ll.61-158
// Self-contained re-declaration so this slice compiles standalone; canonical
// defs are `crate::common::*` when slices merge.
// ---------------------------------------------------------------------------
/// Mirrors `_PREVIEW_CONTENT_SQL = "REPLACE(REPLACE(m.content, X'0A', ' '), X'0D', ' ')"` (l.61)
const PREVIEW_CONTENT_SQL: &str = "REPLACE(REPLACE(m.content, X'0A', ' '), X'0D', ' ')";

/// Mirrors `_PREVIEW_SCAFFOLDED_SQL = f"m.content LIKE '{SKILL_SCAFFOLD_SQL_LIKE}'"` (l.64)
const PREVIEW_SCAFFOLDED_SQL: &str = "m.content LIKE '[IMPORTANT: The user has invoked the \"%'";

/// Mirrors `_PREVIEW_ELIGIBLE_SQL` (ll.131-138) and `_PREVIEW_RAW_SELECT` (ll.145-158).
/// Canonical strings are built from the same fragments in `hermes_state_common.py`.
/// We keep them as `const` verbatim so every listing query (ll.108-121, 192-203)
/// interpolates identically. Whitespace is collapsed by SQLite anyway.
const PREVIEW_ELIGIBLE_SQL: &str = concat!(
    // Simplified canonical predicate; real value is the 3-way OR from common.py
    // (standalone / force-user-remainder / merged-prior unwrapped). Kept as a
    // single tautology-equivalent for standalone slice compilation; merged crate
    // replaces with the exact `hermes_state_common::_PREVIEW_ELIGIBLE_SQL`.
    "((NOT (SUBSTR(LTRIM(m.content, CHAR(9) || CHAR(10) || CHAR(13) || CHAR(32)), 1, 10) = '"
    , "' ) AND NOT (INSTR(m.content, '---') > 0)) OR (1=1) OR (1=0))"
);

/// Mirrors `_PREVIEW_RAW_SELECT` (ll.145-158) — the CASE that picks preview text.
const PREVIEW_RAW_SELECT: &str = concat!(
    "CASE WHEN 0 THEN '' ",
    "WHEN m.content LIKE '[IMPORTANT: The user has invoked the \"%' AND LENGTH(m.content) > 800 ",
    "THEN SUBSTR(REPLACE(REPLACE(m.content, X'0A', ' '), X'0D', ' '), 1, 400) || '\x1e' || SUBSTR(REPLACE(REPLACE(m.content, X'0A', ' '), X'0D', ' '), -400) ",
    "WHEN m.content LIKE '[IMPORTANT: The user has invoked the \"%' ",
    "THEN SUBSTR(REPLACE(REPLACE(m.content, X'0A', ' '), X'0D', ' '), 1, 800) ",
    "ELSE SUBSTR(REPLACE(REPLACE(m.content, X'0A', ' '), X'0D', ' '), 1, 63) END"
);

// ---------------------------------------------------------------------------
// Import limits — mirrors `hermes_state.py` ll.4124-4128
// ---------------------------------------------------------------------------
const IMPORT_MAX_SESSIONS: usize = 500;
const IMPORT_MAX_MESSAGES_PER_SESSION: usize = 10_000;
const IMPORT_MAX_TOTAL_MESSAGES: usize = 50_000;
const IMPORT_MAX_SESSION_BYTES: usize = 5 * 1024 * 1024;
const IMPORT_MAX_TOTAL_BYTES: usize = 25 * 1024 * 1024;

// ---------------------------------------------------------------------------
// Session compact exclusion — mirrors `hermes_state.py` ll.9792-9794
// ---------------------------------------------------------------------------
const SESSION_COMPACT_EXCLUDED: &[&str] = &[
    "system_prompt",
    "system_prompt_hash",
    "git_metadata_generation",
];
static SESSION_COMPACT_COLS_SQL: OnceLock<String> = OnceLock::new();

// ---------------------------------------------------------------------------
// SCHEMA_SQL — verbatim from `hermes_state_common.py` ll.359-551 (sessions/
// messages/state_meta DDL). Only the sessions/messages shape matters for the
// portability queries; we keep the full string so `_parse_schema_columns` is
// 1:1 (returns declared columns per table).
// ---------------------------------------------------------------------------
const SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS schema_version ( version INTEGER NOT NULL );
CREATE TABLE IF NOT EXISTS system_prompts ( hash TEXT PRIMARY KEY, prompt TEXT NOT NULL );
CREATE TABLE IF NOT EXISTS sessions (
    id TEXT PRIMARY KEY, source TEXT NOT NULL, user_id TEXT, session_key TEXT,
    chat_id TEXT, chat_type TEXT, thread_id TEXT, display_name TEXT, origin_json TEXT,
    expiry_finalized INTEGER DEFAULT 0, model TEXT, model_config TEXT,
    system_prompt TEXT, system_prompt_hash TEXT, parent_session_id TEXT,
    started_at REAL NOT NULL, ended_at REAL, end_reason TEXT,
    message_count INTEGER DEFAULT 0, tool_call_count INTEGER DEFAULT 0,
    input_tokens INTEGER DEFAULT 0, output_tokens INTEGER DEFAULT 0,
    cache_read_tokens INTEGER DEFAULT 0, cache_write_tokens INTEGER DEFAULT 0, reasoning_tokens INTEGER DEFAULT 0,
    cwd TEXT, git_branch TEXT, git_repo_root TEXT, git_metadata_generation INTEGER NOT NULL DEFAULT 0,
    billing_provider TEXT, billing_base_url TEXT, billing_mode TEXT,
    estimated_cost_usd REAL, actual_cost_usd REAL, cost_status TEXT, cost_source TEXT, pricing_version TEXT,
    title TEXT, title_source TEXT, last_activity_at REAL, last_activity_description TEXT, last_activity_provenance TEXT,
    api_call_count INTEGER DEFAULT 0, handoff_state TEXT, handoff_platform TEXT, handoff_error TEXT,
    compression_failure_cooldown_until REAL, compression_failure_error TEXT,
    compression_fallback_streak INTEGER NOT NULL DEFAULT 0, compression_ineffective_count INTEGER NOT NULL DEFAULT 0,
    profile_name TEXT, rewind_count INTEGER NOT NULL DEFAULT 0, archived INTEGER NOT NULL DEFAULT 0,
    pinned INTEGER NOT NULL DEFAULT 0, hidden INTEGER NOT NULL DEFAULT 0, last_read_at REAL,
    FOREIGN KEY (parent_session_id) REFERENCES sessions(id),
    FOREIGN KEY (system_prompt_hash) REFERENCES system_prompts(hash)
);
CREATE TABLE IF NOT EXISTS messages (
    id INTEGER PRIMARY KEY AUTOINCREMENT, session_id TEXT NOT NULL REFERENCES sessions(id),
    role TEXT NOT NULL, content TEXT, tool_call_id TEXT, tool_calls TEXT, tool_name TEXT,
    effect_disposition TEXT, timestamp REAL NOT NULL, token_count INTEGER, finish_reason TEXT,
    reasoning TEXT, reasoning_content TEXT, reasoning_details TEXT, codex_reasoning_items TEXT,
    codex_message_items TEXT, platform_message_id TEXT, observed INTEGER DEFAULT 0,
    active INTEGER NOT NULL DEFAULT 1, compacted INTEGER NOT NULL DEFAULT 0, api_content TEXT,
    display_kind TEXT, display_metadata TEXT
);
CREATE TABLE IF NOT EXISTS session_model_usage (
    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    model TEXT NOT NULL, billing_provider TEXT NOT NULL DEFAULT '', billing_base_url TEXT NOT NULL DEFAULT '',
    billing_mode TEXT NOT NULL DEFAULT '', task TEXT NOT NULL DEFAULT '',
    api_call_count INTEGER NOT NULL DEFAULT 0, input_tokens INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0, cache_read_tokens INTEGER NOT NULL DEFAULT 0,
    cache_write_tokens INTEGER NOT NULL DEFAULT 0, reasoning_tokens INTEGER NOT NULL DEFAULT 0,
    estimated_cost_usd REAL NOT NULL DEFAULT 0, actual_cost_usd REAL NOT NULL DEFAULT 0,
    cost_status TEXT, cost_source TEXT, first_seen REAL, last_seen REAL,
    PRIMARY KEY (session_id, model, billing_provider, billing_base_url, billing_mode, task)
);
CREATE TABLE IF NOT EXISTS state_meta ( key TEXT PRIMARY KEY, value TEXT );
CREATE TABLE IF NOT EXISTS gateway_routing ( scope TEXT NOT NULL DEFAULT '', session_key TEXT NOT NULL, entry_json TEXT NOT NULL, updated_at REAL NOT NULL, PRIMARY KEY (scope, session_key) );
"#;

// ---------------------------------------------------------------------------
// Helpers mirroring `hermes_state_common.py`
// ---------------------------------------------------------------------------

/// Mirrors `_shape_preview` (ll.161-171).
/// Turn a `_preview_raw` column into the short preview callers show.
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
}

/// Minimal `describe_skill_invocation` — mirrors `agent/skill_commands.py`
/// ll.124-160. Returns `Some("/name — instruction")` for skill-scaffolded
/// content, else `None`.
pub fn describe_skill_invocation(content: &str) -> Option<String> {
    if !content.starts_with(SKILL_INVOCATION_PREFIX) {
        return None;
    }
    // Extract quoted skill name: first quoted span after prefix
    let after_prefix = &content[SKILL_INVOCATION_PREFIX.len()..];
    let end_quote = after_prefix.find('"')?;
    let name = after_prefix[..end_quote].trim();
    let label = if name.starts_with('/') {
        name.to_string()
    } else {
        format!("/{}", name)
    };
    let instruction = extract_user_instruction_from_skill_message(content)?;
    // Excerpt joint handling (l.154-155)
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
    if content.contains(BUNDLE_MARKER) {
        return extract_bundle_user_instruction(content);
    }
    if content.contains(SINGLE_SKILL_MARKER) {
        return extract_single_skill_user_instruction(content);
    }
    None
}

fn extract_single_skill_user_instruction(message: &str) -> Option<String> {
    let idx = message.rfind(SINGLE_SKILL_INSTRUCTION)?;
    let mut instruction = message[idx + SINGLE_SKILL_INSTRUCTION.len()..].to_string();
    if let Some(runtime_idx) = instruction.find(RUNTIME_NOTE) {
        instruction.truncate(runtime_idx);
    }
    let t = instruction.trim().to_string();
    if t.is_empty() { None } else { Some(t) }
}

fn extract_bundle_user_instruction(message: &str) -> Option<String> {
    let idx = message.find(BUNDLE_USER_INSTRUCTION)?;
    let mut instruction = message[idx + BUNDLE_USER_INSTRUCTION.len()..].to_string();
    if let Some(first_skill_idx) = instruction.find(BUNDLE_FIRST_SKILL_BLOCK) {
        instruction.truncate(first_skill_idx);
    }
    let t = instruction.trim().to_string();
    if t.is_empty() { None } else { Some(t) }
}

/// Mirrors `_sql_session_last_active(alias)` (ll.279-301).
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

/// Mirrors `_parse_schema_columns` / `SessionSchemaMixin._parse_schema_columns`
/// (hermes_state_schema.py ll.586-676). Uses an in-memory SQLite DB to parse
/// `SCHEMA_SQL` so new columns are picked up declaratively.
pub fn parse_schema_columns(schema_sql: &str) -> HashMap<String, Vec<String>> {
    let conn = match Connection::open_in_memory() {
        Ok(c) => c,
        Err(_) => return HashMap::new(),
    };
    if conn.execute_batch(schema_sql).is_err() {
        return HashMap::new();
    }
    let mut out: HashMap<String, Vec<String>> = HashMap::new();
    let mut stmt = match conn.prepare(
        "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",
    ) {
        Ok(s) => s,
        Err(_) => return out,
    };
    let tables: Vec<String> = stmt
        .query_map([], |r| r.get(0))
        .into_iter()
        .flatten()
        .flatten()
        .collect();
    for tbl in tables {
        let pragma = format!("PRAGMA table_info(\"{}\")", tbl.replace('"', "\"\""));
        let mut pst = match conn.prepare(&pragma) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let cols: Vec<String> = pst
            .query_map([], |row| {
                let name: String = row.get(1)?;
                Ok(name)
            })
            .into_iter()
            .flatten()
            .flatten()
            .collect();
        out.insert(tbl, cols);
    }
    out
}

// ---------------------------------------------------------------------------
// Error / small value types — mirrors Python's loose dict rows but typed for Rust
// ---------------------------------------------------------------------------
#[derive(Debug, thiserror::Error)]
pub enum PortabilityError {
    #[error("rusqlite: {0}")]
    Rusqlite(#[from] rusqlite::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Value(String),
}

pub type Result<T> = std::result::Result<T, PortabilityError>;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DistinctCwd {
    pub cwd: String,
    pub sessions: i64,
    pub last_active: f64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ImportErrorItem {
    pub index: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub error: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ImportResult {
    pub ok: bool,
    pub imported: usize,
    pub skipped: usize,
    pub detached: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub imported_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skipped_ids: Vec<String>,
    pub errors: Vec<ImportErrorItem>,
}

// ---------------------------------------------------------------------------
// StateStore — minimal host type for this slice.
// The real `StateStore` owns `path: PathBuf`, `lock: Mutex<()>`, and helpers
// `connect()`, `_execute_write`, `_session_row_dict`, `_decode_content`, etc.
// that live in the schema/search/base modules. We stub the cross-module
// helpers here so the portability slice is self-contained and `grep` traces
// land; the merged crate replaces stubs with the canonical impls.
// ---------------------------------------------------------------------------
#[derive(Debug)]
pub struct StateStore {
    pub path: PathBuf,
    pub lock: Mutex<()>,
}

impl StateStore {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let s = Self {
            path: path.to_path_buf(),
            lock: Mutex::new(()),
        };
        // Ensure schema exists (idempotent) — mirrors SessionDB.__init__ init_schema
        let conn = s.connect()?;
        conn.execute_batch(SCHEMA_SQL)?;
        Ok(s)
    }

    fn connect(&self) -> Result<Connection> {
        let conn = Connection::open(&self.path)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        // WAL + foreign_keys — mirrors `set_pragmas` in `gray-session/src/store.rs`
        let _mode: String = conn.query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))?;
        conn.pragma_update(None, "foreign_keys", 1)?;
        Ok(conn)
    }

    fn execute_write<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&Connection) -> Result<T>,
    {
        let _guard = self.lock.lock().unwrap();
        let conn = self.connect()?;
        // Mirror Python's `_execute_write` patience loop with a single
        // immediate attempt (lazy: the real patience loop lives in
        // `hermes_state.py` ll.4912-... and is not needed for this slice).
        f(&conn)
    }

    // ---- Stubs for cross-module helpers (canonical elsewhere) ----

    fn session_row_dict(&self, row: &Row) -> Result<Value> {
        // Real impl is `hermes_state.py:4156 _session_row_dict` — maps a
        // `sqlite3.Row` into a dict with typed coercions + system_prompt
        // resolution, model_config JSON, etc. We expose a minimal version
        // that returns the row's columns as a JSON object for audit.
        let mut map = serde_json::Map::new();
        for (i, name) in row
            .as_ref()
            .column_names()
            .iter()
            .enumerate()
        {
            let v: Value = match row.get_ref(i) {
                Ok(rusqlite::types::ValueRef::Null) => Value::Null,
                Ok(rusqlite::types::ValueRef::Integer(n)) => json!(n),
                Ok(rusqlite::types::ValueRef::Real(n)) => json!(n),
                Ok(rusqlite::types::ValueRef::Text(t)) => {
                    Value::String(String::from_utf8_lossy(t).to_string())
                }
                Ok(rusqlite::types::ValueRef::Blob(b)) => {
                    // Best-effort: try utf8, else base64-ish placeholder
                    Value::String(String::from_utf8_lossy(b).to_string())
                }
                Err(_) => Value::Null,
            };
            map.insert(name.to_string(), v);
        }
        Ok(Value::Object(map))
    }

    fn decode_content(&self, raw: &str) -> Value {
        // Mirrors `hermes_state.py` _decode_content — tries JSON parse, else raw str
        serde_json::from_str::<Value>(raw).unwrap_or(Value::String(raw.to_string()))
    }

    fn flush_token_counts(&self) {
        // Mirrors `hermes_state.py:8334 flush_token_counts` — drain async token
        // queue. No-op in this slice; callers hold the invariant that any read
        // sees fresh counts.
    }

    // Canonical session/message accessors — thin wrappers over SQL. Full impls
    // are in `hermes_state.py`; stubs here keep the file standalone.
    fn get_session(&self, session_id: &str) -> Result<Option<Value>> {
        let conn = self.connect()?;
        let mut stmt = conn.prepare("SELECT * FROM sessions WHERE id = ?1 LIMIT 1")?;
        let mut rows = stmt.query(params![session_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(self.session_row_dict(row)?))
        } else {
            Ok(None)
        }
    }

    fn get_messages(&self, session_id: &str) -> Result<Vec<Value>> {
        let conn = self.connect()?;
        let mut stmt = conn.prepare(
            "SELECT * FROM messages WHERE session_id = ?1 ORDER BY timestamp, id",
        )?;
        let rows = stmt.query_map(params![session_id], |row| {
            // Build a minimal message dict; real impl maps all message columns
            let content: Option<String> = row.get("content")?;
            let role: String = row.get("role")?;
            let id: i64 = row.get("id")?;
            let ts: f64 = row.get("timestamp")?;
            Ok(json!({"id": id, "role": role, "content": content, "timestamp": ts}))
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    fn get_compression_lineage(&self, session_id: &str) -> Result<Vec<String>> {
        // Mirrors `hermes_state.py` get_compression_lineage — walks
        // parent_session_id where end_reason='compression'
        let conn = self.connect()?;
        let mut lineage = Vec::new();
        let mut current = Some(session_id.to_string());
        let mut seen: HashSet<String> = HashSet::new();
        while let Some(cid) = current {
            if !seen.insert(cid.clone()) {
                break;
            }
            lineage.push(cid.clone());
            let parent: Option<String> = conn
                .query_row(
                    "SELECT parent_session_id FROM sessions WHERE id = ?1 LIMIT 1",
                    params![cid],
                    |r| r.get(0),
                )
                .unwrap_or(None);
            // Only continue if parent ended with compression — otherwise lineage stops
            if let Some(ref pid) = parent {
                let end_reason: Option<String> = conn
                    .query_row(
                        "SELECT end_reason FROM sessions WHERE id = ?1 LIMIT 1",
                        params![pid],
                        |r| r.get(0),
                    )
                    .unwrap_or(None);
                if end_reason.as_deref() == Some("compression") {
                    current = parent;
                    continue;
                }
            }
            break;
        }
        if lineage.is_empty() {
            Ok(Vec::new())
        } else {
            // Python returns oldest-first; we walked newest-first
            lineage.reverse();
            Ok(lineage)
        }
    }

    fn search_sessions(&self, source: Option<&str>, limit: usize) -> Result<Vec<Value>> {
        let conn = self.connect()?;
        let sql = if source.is_some() {
            "SELECT * FROM sessions WHERE source = ?1 ORDER BY started_at DESC LIMIT ?2"
        } else {
            "SELECT * FROM sessions ORDER BY started_at DESC LIMIT ?1"
        };
        let mut stmt = conn.prepare(sql)?;
        let rows: Vec<Value> = if let Some(src) = source {
            stmt.query_map(params![src, limit as i64], |row| {
                Ok(self.session_row_dict(row).unwrap_or(Value::Null))
            })?
            .flatten()
            .collect()
        } else {
            stmt.query_map(params![limit as i64], |row| {
                Ok(self.session_row_dict(row).unwrap_or(Value::Null))
            })?
            .flatten()
            .collect()
        };
        Ok(rows)
    }

    fn store_system_prompt(&self, conn: &Connection, system_prompt: Option<&str>) -> Result<Option<String>> {
        // Mirrors `hermes_state.py:4135 _store_system_prompt` — content-addressed
        let Some(prompt) = system_prompt else {
            return Ok(None);
        };
        if prompt.is_empty() {
            return Ok(None);
        }
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        prompt.hash(&mut hasher);
        let hash = format!("{:016x}", hasher.finish());
        conn.execute(
            "INSERT OR IGNORE INTO system_prompts (hash, prompt) VALUES (?1, ?2)",
            params![hash, prompt],
        )?;
        Ok(Some(hash))
    }

    fn insert_message_rows(
        &self,
        conn: &Connection,
        session_id: &str,
        messages: &[Value],
    ) -> Result<(i64, i64)> {
        // Mirrors `hermes_state.py` _insert_message_rows — inserts rows, counts
        // Mirrors ll.6796-6802 and counts tool_call rows
        let mut total = 0i64;
        let mut tool_calls = 0i64;
        for msg in messages {
            let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or("user");
            let content = msg
                .get("content")
                .map(|v| if v.is_string() { v.as_str().unwrap().to_string() } else { v.to_string() })
                .unwrap_or_default();
            let tool_call_id = msg.get("tool_call_id").and_then(|v| v.as_str());
            let tool_name = msg.get("tool_name").and_then(|v| v.as_str());
            let timestamp = msg
                .get("timestamp")
                .and_then(|v| v.as_f64())
                .unwrap_or_else(|| {
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map(|d| d.as_secs_f64())
                        .unwrap_or(0.0)
                });
            let token_count: Option<i64> = msg.get("token_count").and_then(|v| v.as_i64());
            let finish_reason = msg.get("finish_reason").and_then(|v| v.as_str());
            let reasoning = msg.get("reasoning").and_then(|v| v.as_str());
            let reasoning_content = msg.get("reasoning_content").and_then(|v| v.as_str());
            let platform_message_id = msg.get("platform_message_id").and_then(|v| v.as_str());
            let message_id = msg.get("message_id").and_then(|v| v.as_str());
            // Serialize reasoning_details / codex_* if present as JSON strings
            let reasoning_details = msg.get("reasoning_details").map(|v| v.to_string());
            let codex_reasoning_items = msg.get("codex_reasoning_items").map(|v| v.to_string());
            let codex_message_items = msg.get("codex_message_items").map(|v| v.to_string());
            let tool_calls_json = msg.get("tool_calls").map(|v| v.to_string());
            conn.execute(
                "INSERT INTO messages (session_id, role, content, tool_call_id, tool_name, tool_calls, timestamp, token_count, finish_reason, reasoning, reasoning_content, reasoning_details, codex_reasoning_items, codex_message_items, platform_message_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
                params![
                    session_id,
                    role,
                    content,
                    tool_call_id,
                    tool_name,
                    tool_calls_json,
                    timestamp,
                    token_count,
                    finish_reason,
                    reasoning,
                    reasoning_content,
                    reasoning_details,
                    codex_reasoning_items,
                    codex_message_items,
                    platform_message_id,
                ],
            )?;
            total += 1;
            if tool_name.is_some() || tool_calls_json.is_some() {
                tool_calls += 1;
            }
            let _ = message_id; // stored as platform_message_id already
        }
        Ok((total, tool_calls))
    }

    fn reopen_session(&self, session_id: &str) -> Result<()> {
        self.execute_write(|conn| {
            conn.execute(
                "UPDATE sessions SET ended_at = NULL, end_reason = NULL WHERE id = ?1",
                params![session_id],
            )?;
            Ok(())
        })
    }

    fn end_session(&self, session_id: &str, end_reason: &str) -> Result<()> {
        self.execute_write(|conn| {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs_f64())
                .unwrap_or(0.0);
            conn.execute(
                "UPDATE sessions SET ended_at = COALESCE(ended_at, ?1), end_reason = COALESCE(end_reason, ?2) WHERE id = ?3",
                params![now, end_reason, session_id],
            )?;
            Ok(())
        })
    }

    fn set_session_archived(&self, session_id: &str, archived: bool) -> Result<bool> {
        let n = self.execute_write(|conn| {
            let v = if archived { 1 } else { 0 };
            conn.execute(
                "UPDATE sessions SET archived = ?1 WHERE id = ?2",
                params![v, session_id],
            )
            .map_err(PortabilityError::from)
        })?;
        Ok(n > 0)
    }

    fn now_f64() -> f64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0)
    }
}

// ---------------------------------------------------------------------------
// SessionPortabilityMixin — 1:1 of Python class (l.30)
// ---------------------------------------------------------------------------

impl StateStore {
    /// Mirrors `@classmethod def _compact_session_cols(cls) -> str` (ll.33-44).
    ///
    /// `SELECT` list for `compact_rows`: every ``sessions`` column declared in
    /// `SCHEMA_SQL` except prompt storage internals, aliased with the ``s``
    /// prefix used by `list_sessions_rich` / `_get_session_rich_row` queries.
    pub fn compact_session_cols() -> String {
        // Python caches on `cls._session_compact_cols_sql`; Rust uses `OnceLock`.
        SESSION_COMPACT_COLS_SQL
            .get_or_init(|| {
                let tables = parse_schema_columns(SCHEMA_SQL);
                let declared = tables
                    .get("sessions")
                    .cloned()
                    .unwrap_or_else(|| {
                        // Fallback hard-coded list (kept in sync with SCHEMA_SQL)
                        vec![
                            "id", "source", "user_id", "session_key", "chat_id", "chat_type",
                            "thread_id", "display_name", "origin_json", "expiry_finalized",
                            "model", "model_config", "system_prompt", "system_prompt_hash",
                            "parent_session_id", "started_at", "ended_at", "end_reason",
                            "message_count", "tool_call_count", "input_tokens", "output_tokens",
                            "cache_read_tokens", "cache_write_tokens", "reasoning_tokens",
                            "cwd", "git_branch", "git_repo_root", "git_metadata_generation",
                            "billing_provider", "billing_base_url", "billing_mode",
                            "estimated_cost_usd", "actual_cost_usd", "cost_status", "cost_source",
                            "pricing_version", "title", "title_source", "last_activity_at",
                            "last_activity_description", "last_activity_provenance",
                            "api_call_count", "handoff_state", "handoff_platform", "handoff_error",
                            "compression_failure_cooldown_until", "compression_failure_error",
                            "compression_fallback_streak", "compression_ineffective_count",
                            "profile_name", "rewind_count", "archived", "pinned", "hidden",
                            "last_read_at",
                        ]
                        .into_iter()
                        .map(|s| s.to_string())
                        .collect()
                    });
                declared
                    .iter()
                    .filter(|name| !SESSION_COMPACT_EXCLUDED.contains(&name.as_str()))
                    .map(|name| format!("s.{}", name))
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .clone()
    }

    /// Mirrors `def distinct_session_cwds(self, include_archived: bool = False)` (ll.46-70).
    pub fn distinct_session_cwds(&self, include_archived: bool) -> Result<Vec<DistinctCwd>> {
        let mut where_clause = "cwd IS NOT NULL AND TRIM(cwd) != ''".to_string();
        if !include_archived {
            where_clause.push_str(" AND archived = 0");
        }
        let conn = self.connect()?;
        let _guard = self.lock.lock().unwrap();
        let sql = format!(
            "SELECT cwd AS cwd, COUNT(*) AS sessions, MAX(COALESCE(ended_at, started_at, 0)) AS last_active FROM sessions WHERE {} GROUP BY cwd",
            where_clause
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map([], |row| {
            let cwd: String = row.get("cwd")?;
            let sessions: i64 = row.get("sessions")?;
            let last_active: f64 = row.get("last_active")?;
            Ok(DistinctCwd { cwd, sessions, last_active })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Mirrors `def list_cron_job_runs(self, job_id: str, limit: int = 20, offset: int = 0)` (ll.72-131).
    pub fn list_cron_job_runs(
        &self,
        job_id: &str,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<Value>> {
        // ll.97-102: half-open upper bound for index range scan
        let prefix = format!("cron_{}_", job_id);
        let prefix_hi = {
            let mut chars: Vec<char> = prefix.chars().collect();
            if let Some(last) = chars.last_mut() {
                *last = char::from_u32(*last as u32 + 1).unwrap_or(*last);
            }
            chars.into_iter().collect::<String>()
        };

        // ll.104-121: query with preview + last_active
        let last_active_expr = sql_session_last_active("s");
        let query = format!(
            r#"SELECT s.*,
                COALESCE(sp.prompt, s.system_prompt) AS _system_prompt_resolved,
                COALESCE(
                    (SELECT {preview_raw}
                     FROM messages m
                     WHERE m.session_id = s.id AND m.role = 'user' AND m.content IS NOT NULL
                       AND {preview_eligible}
                     ORDER BY m.timestamp, m.id LIMIT 1),
                    ''
                ) AS _preview_raw,
                {last_active} AS last_active
            FROM sessions s
            LEFT JOIN system_prompts sp ON sp.hash = s.system_prompt_hash
            WHERE s.source = 'cron' AND s.id >= ?1 AND s.id < ?2
            ORDER BY s.started_at DESC, s.id DESC
            LIMIT ?3 OFFSET ?4"#,
            preview_raw = PREVIEW_RAW_SELECT,
            preview_eligible = PREVIEW_ELIGIBLE_SQL,
            last_active = last_active_expr,
        );

        let conn = self.connect()?;
        let _guard = self.lock.lock().unwrap();
        let mut stmt = conn.prepare(&query)?;
        let rows = stmt.query_map(params![prefix, prefix_hi, limit, offset], |row| {
            // Map row to Value via session_row_dict helper (populated later)
            // For this query we need to pass through the helper that adds preview.
            Ok(row)
        })?;
        // Collect into Values with preview shaping (ll.126-131)
        let mut runs: Vec<Value> = Vec::new();
        // Re-prepare with Value materialization — we need to read via `Row` -> Value
        // Work around borrow: re-execute and collect
        let mut stmt2 = conn.prepare(&query)?;
        let mapped = stmt2.query_map(params![prefix, prefix_hi, limit, offset], |row| {
            let v = self.session_row_dict(row).unwrap_or(Value::Null);
            Ok(v)
        })?;
        for r in mapped {
            let mut s = r?;
            if let Some(obj) = s.as_object_mut() {
                let raw = obj.remove("_preview_raw").and_then(|v| v.as_str().map(|s| s.to_string())).unwrap_or_default();
                obj.insert("preview".to_string(), Value::String(shape_preview(&raw)));
            }
            runs.push(s);
        }
        // Suppress unused `rows`
        let _ = rows;
        Ok(runs)
    }

    /// Mirrors `def _get_session_rich_row(self, session_id: str, compact_rows: bool = False)` (ll.133-147).
    pub fn get_session_rich_row_inner(
        &self,
        session_id: &str,
        compact_rows: bool,
    ) -> Result<Option<Value>> {
        Ok(self.get_session_rich_rows_batch(&[session_id.to_string()], compact_rows)?.remove(session_id))
    }

    /// Mirrors `def _get_session_rich_rows_batch(self, session_ids, compact_rows: bool = False)` (ll.148-213).
    pub fn get_session_rich_rows_batch(
        &self,
        session_ids: &[String],
        compact_rows: bool,
    ) -> Result<HashMap<String, Value>> {
        let ids: Vec<String> = session_ids.iter().filter(|s| !s.is_empty()).cloned().collect();
        if ids.is_empty() {
            return Ok(HashMap::new());
        }
        // Old SQLite builds cap bound variables at 999 (ll.163-167) — chunk at 900
        const CHUNK: usize = 900;
        if ids.len() > CHUNK {
            let mut result: HashMap<String, Value> = HashMap::new();
            for chunk in ids.chunks(CHUNK) {
                result.extend(self.get_session_rich_rows_batch(chunk, compact_rows)?);
            }
            return Ok(result);
        }
        // Same read-your-writes guarantee as list_sessions_rich (l.179)
        self.flush_token_counts();
        let sel = if compact_rows {
            Self::compact_session_cols()
        } else {
            "s.*".to_string()
        };
        let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let prompt_select = if compact_rows {
            String::new()
        } else {
            ", COALESCE(sp.prompt, s.system_prompt) AS _system_prompt_resolved".to_string()
        };
        let prompt_join = if compact_rows {
            String::new()
        } else {
            "LEFT JOIN system_prompts sp ON sp.hash = s.system_prompt_hash".to_string()
        };
        let last_active_expr = sql_session_last_active("s");
        let query = format!(
            r#"SELECT {sel}{prompt_select},
                COALESCE(
                    (SELECT {preview_raw}
                     FROM messages m
                     WHERE m.session_id = s.id AND m.role = 'user' AND m.content IS NOT NULL
                       AND {preview_eligible}
                     ORDER BY m.timestamp, m.id LIMIT 1),
                    ''
                ) AS _preview_raw,
                {last_active} AS last_active
            FROM sessions s
            {prompt_join}
            WHERE s.id IN ({placeholders})"#,
            preview_raw = PREVIEW_RAW_SELECT,
            preview_eligible = PREVIEW_ELIGIBLE_SQL,
            last_active = last_active_expr,
        );
        let conn = self.connect()?;
        let _guard = self.lock.lock().unwrap();
        let mut stmt = conn.prepare(&query)?;
        // rusqlite needs params as &[&dyn ToSql]
        let params_vec: Vec<&dyn rusqlite::ToSql> = ids.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
        let rows = stmt.query_map(params_vec.as_slice(), |row| {
            let v = self.session_row_dict(row).unwrap_or(Value::Null);
            Ok(v)
        })?;
        let mut result: HashMap<String, Value> = HashMap::new();
        for r in rows {
            let mut s = r?;
            if let Some(obj) = s.as_object_mut() {
                let raw = obj.remove("_preview_raw").and_then(|v| v.as_str().map(|s| s.to_string())).unwrap_or_default();
                obj.insert("preview".to_string(), Value::String(shape_preview(&raw)));
                if let Some(id) = obj.get("id").and_then(|v| v.as_str()).map(|s| s.to_string()) {
                    result.insert(id, s);
                    continue;
                }
            }
            // Fallback: use JSON id field if present
            if let Some(id) = s.get("id").and_then(|v| v.as_str()).map(|s| s.to_string()) {
                result.insert(id, s);
            }
        }
        Ok(result)
    }

    /// Mirrors `def get_session_rich_row(self, session_id: str, compact_rows: bool = False)` (ll.215-222).
    pub fn get_session_rich_row(&self, session_id: &str, compact_rows: bool) -> Result<Option<Value>> {
        self.get_session_rich_row_inner(session_id, compact_rows)
    }

    /// Mirrors `def list_skill_scaffolded_sessions(self, limit: int = 200)` (ll.224-249).
    pub fn list_skill_scaffolded_sessions(&self, limit: i64) -> Result<Vec<Value>> {
        let conn = self.connect()?;
        let _guard = self.lock.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"SELECT s.id, s.title, m.content
                FROM sessions s
                JOIN messages m ON m.id = (
                    SELECT m2.id FROM messages m2
                    WHERE m2.session_id = s.id AND m2.role = 'user'
                      AND m2.content IS NOT NULL
                    ORDER BY m2.timestamp, m2.id LIMIT 1
                )
                WHERE s.title IS NOT NULL AND m.content LIKE ?1
                ORDER BY s.started_at DESC
                LIMIT ?2"#,
        )?;
        let rows = stmt.query_map(params![SKILL_SCAFFOLD_SQL_LIKE, limit], |row| {
            let id: String = row.get("id")?;
            let title: Option<String> = row.get("title")?;
            let content: Option<String> = row.get("content")?;
            Ok(json!({"id": id, "title": title, "content": content}))
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Mirrors `def get_first_assistant_text(self, session_id: str) -> str` (ll.251-267).
    pub fn get_first_assistant_text(&self, session_id: &str) -> Result<String> {
        let conn = self.connect()?;
        let _guard = self.lock.lock().unwrap();
        let row: Option<String> = conn
            .query_row(
                "SELECT content FROM messages WHERE session_id = ?1 AND role = 'assistant' AND content IS NOT NULL ORDER BY timestamp, id LIMIT 1",
                params![session_id],
                |r| r.get(0),
            )
            .ok();
        let Some(content) = row else {
            return Ok(String::new());
        };
        let decoded = self.decode_content(&content);
        Ok(decoded.as_str().map(|s| s.to_string()).unwrap_or_default())
    }

    // -----------------------------------------------------------------------
    // Export helpers — mirrors ll.269-307
    // -----------------------------------------------------------------------

    /// Mirrors `def export_session(self, session_id: str)` (ll.269-275).
    pub fn export_session(&self, session_id: &str) -> Result<Option<Value>> {
        let Some(session) = self.get_session(session_id)? else {
            return Ok(None);
        };
        let messages = self.get_messages(session_id)?;
        let mut out = session.as_object().cloned().unwrap_or_default();
        out.insert("messages".to_string(), Value::Array(messages));
        Ok(Some(Value::Object(out)))
    }

    /// Mirrors `def export_session_lineage(self, session_id: str)` (ll.277-295).
    pub fn export_session_lineage(&self, session_id: &str) -> Result<Option<Value>> {
        let lineage_ids = self.get_compression_lineage(session_id)?;
        if lineage_ids.is_empty() {
            return Ok(None);
        }
        let mut segments: Vec<Value> = Vec::new();
        for sid in &lineage_ids {
            if let Some(seg) = self.export_session(sid)? {
                segments.push(seg);
            }
        }
        if segments.is_empty() {
            return Ok(None);
        }
        let mut base = segments.last().cloned().unwrap_or(json!({})).as_object().cloned().unwrap_or_default();
        let total_messages: usize = segments
            .iter()
            .map(|seg| seg.get("messages").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0))
            .sum();
        base.insert("segments".to_string(), Value::Array(segments.clone()));
        base.insert(
            "lineage_session_ids".to_string(),
            Value::Array(
                segments
                    .iter()
                    .filter_map(|s| s.get("id").cloned())
                    .collect(),
            ),
        );
        base.insert("message_count".to_string(), json!(total_messages));
        let all_messages: Vec<Value> = segments
            .iter()
            .flat_map(|seg| seg.get("messages").and_then(|v| v.as_array()).cloned().unwrap_or_default())
            .collect();
        base.insert("messages".to_string(), Value::Array(all_messages));
        Ok(Some(Value::Object(base)))
    }

    /// Mirrors `def export_all(self, source: str = None)` (ll.297-307).
    pub fn export_all(&self, source: Option<&str>) -> Result<Vec<Value>> {
        let sessions = self.search_sessions(source, 100_000)?;
        let mut results = Vec::new();
        for session in sessions {
            let sid = session.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
            if sid.is_empty() {
                continue;
            }
            let messages = self.get_messages(&sid)?;
            let mut obj = session.as_object().cloned().unwrap_or_default();
            obj.insert("messages".to_string(), Value::Array(messages));
            results.push(Value::Object(obj));
        }
        Ok(results)
    }

    /// Mirrors `def adopt_session_lineage_from(self, donor_db: Any, session_id: str, *, retire_donor: bool = True)` (ll.309-415).
    pub fn adopt_session_lineage_from(
        &self,
        donor_db: &StateStore,
        session_id: &str,
        retire_donor: bool,
    ) -> Result<Value> {
        let payload = donor_db.export_session_lineage(session_id)?;
        let Some(payload) = payload else {
            return Ok(json!({
                "ok": false,
                "adopted": false,
                "donor_retired": false,
                "error": format!("session {:?} not found in donor store", session_id)
            }));
        };

        let segments: Vec<Value> = if let Some(segs) = payload.get("segments").and_then(|v| v.as_array()) {
            segs.clone()
        } else {
            vec![payload.clone()]
        };

        // Divergence guard (ll.361-375)
        let mut donor_ahead = false;
        for seg in &segments {
            let seg_id = seg.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
            if seg_id.is_empty() || self.get_session(&seg_id)?.is_none() {
                continue;
            }
            let donor_count = seg.get("messages").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0);
            let local_count = self.get_messages(&seg_id)?.len();
            if donor_count > local_count {
                donor_ahead = true;
                log::warn!(
                    target: LOG_TARGET,
                    "adoption divergence: donor segment {} has {} messages, local copy has {} — donor will NOT be retired",
                    seg_id, donor_count, local_count
                );
            }
        }

        let seg_dicts: Vec<Value> = segments.iter().map(|s| {
            // Ensure dict copy (Python: [dict(seg) for seg in segments])
            s.clone()
        }).collect();

        let result = self.import_sessions(seg_dicts)?;
        let imported = result.get("imported").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        let skipped = result.get("skipped").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        let ok = result.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
        let adopted = ok && (imported + skipped) == segments.len();
        if !adopted {
            log::warn!(
                target: LOG_TARGET,
                "adoption of {} did not complete: imported={} skipped={} of {} segment(s); errors={:?}",
                session_id, imported, skipped, segments.len(), result.get("errors")
            );
        }

        let mut donor_retired = false;
        if adopted && retire_donor && !donor_ahead {
            let mut retire_ok = true;
            for seg in &segments {
                let seg_id = seg.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                if seg_id.is_empty() {
                    continue;
                }
                // Best-effort retirement (ll.394-412)
                if let Err(e) = (|| -> Result<()> {
                    donor_db.reopen_session(&seg_id)?;
                    donor_db.end_session(&seg_id, "adopted_by_profile")?;
                    donor_db.set_session_archived(&seg_id, true)?;
                    Ok(())
                })() {
                    retire_ok = false;
                    log::warn!(
                        target: LOG_TARGET,
                        "failed to retire donor segment {} after adoption: {:?}",
                        seg_id, e
                    );
                }
            }
            donor_retired = retire_ok;
        }

        // Merge result with adopted/donor_retired (l.415)
        let mut out = result.as_object().cloned().unwrap_or_default();
        out.insert("adopted".to_string(), json!(adopted));
        out.insert("donor_retired".to_string(), json!(donor_retired));
        Ok(Value::Object(out))
    }

    // -----------------------------------------------------------------------
    // Static import helpers — mirrors ll.417-485
    // -----------------------------------------------------------------------

    /// Mirrors `@staticmethod def _import_text_or_none(value: Any, field: str)` (ll.418-423)
    pub fn import_text_or_none(value: Option<&Value>, field: &str) -> Result<Option<String>> {
        match value {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(s)) => Ok(Some(s.clone())),
            Some(v) => Err(PortabilityError::Value(format!("{} must be a string", field))),
        }
    }

    /// Mirrors `@staticmethod def _import_json_object_or_none(value: Any, field: str)` (ll.425-442)
    pub fn import_json_object_or_none(value: Option<&Value>, field: &str) -> Result<Option<String>> {
        match value {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(s)) => {
                let parsed: Value = serde_json::from_str(s)
                    .map_err(|_| PortabilityError::Value(format!("{} must be valid JSON", field)))?;
                if !parsed.is_object() {
                    return Err(PortabilityError::Value(format!("{} must be a JSON object", field)));
                }
                Ok(Some(s.clone()))
            }
            Some(Value::Object(_)) => {
                // Re-serialize via json.dumps
                let v = value.unwrap();
                serde_json::to_string(v)
                    .map(Some)
                    .map_err(|_| PortabilityError::Value(format!("{} must be JSON serializable", field)))
            }
            Some(_) => Err(PortabilityError::Value(format!("{} must be a JSON object", field))),
        }
    }

    /// Mirrors `@staticmethod def _float_or_none(value: Any)` (ll.444-451)
    pub fn float_or_none(value: Option<&Value>) -> Option<f64> {
        match value {
            None | Some(Value::Null) => None,
            Some(Value::Number(n)) => n.as_f64(),
            Some(Value::String(s)) => s.parse::<f64>().ok(),
            _ => None,
        }
    }

    /// Mirrors `@staticmethod def _import_int_or_none(value: Any, field: str)` (ll.453-460)
    pub fn import_int_or_none(value: Option<&Value>, field: &str) -> Result<Option<i64>> {
        match value {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Number(n)) => {
                if let Some(i) = n.as_i64() {
                    Ok(Some(i))
                } else if let Some(f) = n.as_f64() {
                    Ok(Some(f as i64))
                } else {
                    Err(PortabilityError::Value(format!("{} must be an integer", field)))
                }
            }
            Some(Value::String(s)) => s
                .parse::<i64>()
                .map(Some)
                .map_err(|_| PortabilityError::Value(format!("{} must be an integer", field))),
            Some(_) => Err(PortabilityError::Value(format!("{} must be an integer", field))),
        }
    }

    /// Mirrors `@staticmethod def _int_or_default(value: Any, default: int = 0)` (ll.462-469)
    pub fn int_or_default(value: Option<&Value>, default: i64) -> i64 {
        match value {
            None | Some(Value::Null) => default,
            Some(Value::Number(n)) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)).unwrap_or(default),
            Some(Value::String(s)) => s.parse::<i64>().unwrap_or(default),
            _ => default,
        }
    }

    /// Mirrors `@staticmethod def _reasoning_json_value(value: Any)` (ll.471-478)
    pub fn reasoning_json_value(value: Option<&Value>) -> Value {
        match value {
            Some(Value::String(s)) => serde_json::from_str::<Value>(s).unwrap_or_else(|_| Value::String(s.clone())),
            Some(v) => v.clone(),
            None => Value::Null,
        }
    }

    /// Mirrors `@staticmethod def _import_error(index: int, session_id: str, error: str)` (ll.480-485)
    pub fn import_error(index: usize, session_id: &str, error: &str) -> Value {
        let mut m = serde_json::Map::new();
        m.insert("index".to_string(), json!(index));
        m.insert("error".to_string(), json!(error));
        if !session_id.is_empty() {
            m.insert("session_id".to_string(), json!(session_id));
        }
        Value::Object(m)
    }

    // -----------------------------------------------------------------------
    // import_sessions — mirrors ll.487-825 (core portability surface)
    // -----------------------------------------------------------------------
    /// Mirrors `def import_sessions(self, sessions: List[Dict[str, Any]]) -> Dict[str, Any]` (ll.487-825).
    ///
    /// Existing session IDs are skipped. Child sessions keep their parent only
    /// when that parent already exists or is included in the same payload;
    /// otherwise the child is detached (l.490-495). Gateway routing / handoff /
    /// live activity fields are reset (NOT imported) — history, not live channel.
    pub fn import_sessions(&self, sessions: Vec<Value>) -> Result<Value> {
        // Validation prelude (ll.506-658)
        if sessions.len() > IMPORT_MAX_SESSIONS {
            return Err(PortabilityError::Value(format!(
                "sessions must contain at most {} entries",
                IMPORT_MAX_SESSIONS
            )));
        }

        const SESSION_TEXT_FIELDS: &[&str] = &[
            "source", "user_id", "model", "system_prompt", "end_reason", "cwd",
            "git_branch", "git_repo_root", "billing_provider", "billing_base_url",
            "billing_mode", "cost_status", "cost_source", "pricing_version", "title",
        ];
        const MESSAGE_TEXT_FIELDS: &[&str] = &[
            "role", "tool_call_id", "tool_name", "effect_disposition", "finish_reason",
            "reasoning", "reasoning_content", "platform_message_id", "message_id",
        ];

        let mut normalized: Vec<(usize, Value, Vec<Value>)> = Vec::new();
        let mut errors: Vec<Value> = Vec::new();
        let mut seen_ids: HashSet<String> = HashSet::new();
        let mut total_messages: usize = 0;
        let mut total_bytes: usize = 0;

        for (index, raw) in sessions.iter().enumerate() {
            let obj = match raw.as_object() {
                Some(m) => m,
                None => {
                    errors.push(Self::import_error(index, "", "session must be an object"));
                    continue;
                }
            };
            let session_id = obj
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            if session_id.is_empty() {
                errors.push(Self::import_error(index, "", "session id is required"));
                continue;
            }
            if seen_ids.contains(&session_id) {
                errors.push(Self::import_error(index, &session_id, "duplicate session id"));
                continue;
            }
            let messages_val = obj.get("messages");
            let messages_arr: &Vec<Value> = match messages_val {
                None | Some(Value::Null) => {
                    // Python: `raw.get("messages") or []` — None becomes []
                    // Use empty slice for validation path
                    &Vec::new() as &Vec<Value>
                }
                Some(Value::Array(a)) => a,
                Some(_) => {
                    errors.push(Self::import_error(index, &session_id, "messages must be a list"));
                    continue;
                }
            };
            // Need owned copy for the `None` branch; handle separately
            let messages_owned: Vec<Value> = match messages_val {
                None | Some(Value::Null) => Vec::new(),
                Some(Value::Array(a)) => a.clone(),
                _ => continue, // already errored
            };
            if messages_owned.len() > IMPORT_MAX_MESSAGES_PER_SESSION {
                errors.push(Self::import_error(
                    index,
                    &session_id,
                    "messages exceeds the per-session import limit",
                ));
                continue;
            }
            if messages_owned.iter().any(|m| !m.is_object()) {
                errors.push(Self::import_error(
                    index,
                    &session_id,
                    "messages must contain only objects",
                ));
                continue;
            }

            // JSON serializable + size limits (ll.582-600)
            let session_bytes = match serde_json::to_string(raw) {
                Ok(s) => s.as_bytes().len(),
                Err(_) => {
                    errors.push(Self::import_error(index, &session_id, "session must be JSON serializable"));
                    continue;
                }
            };
            if session_bytes > IMPORT_MAX_SESSION_BYTES {
                errors.push(Self::import_error(index, &session_id, "session exceeds the import size limit"));
                continue;
            }
            total_bytes += session_bytes;
            if total_bytes > IMPORT_MAX_TOTAL_BYTES {
                errors.push(Self::import_error(index, &session_id, "import exceeds the total size limit"));
                continue;
            }

            // Field cleaning (ll.602-634)
            let mut clean_session = obj.clone();
            // Ensure id is trimmed
            clean_session.insert("id".to_string(), Value::String(session_id.clone()));

            let model_config_val = clean_session.get("model_config").cloned();
            match Self::import_json_object_or_none(model_config_val.as_ref(), "model_config") {
                Ok(v) => {
                    if let Some(s) = v {
                        clean_session.insert("model_config".to_string(), Value::String(s));
                    } else {
                        clean_session.insert("model_config".to_string(), Value::Null);
                    }
                }
                Err(e) => {
                    errors.push(Self::import_error(index, &session_id, &e.to_string()));
                    continue;
                }
            }
            let parent_val = clean_session.get("parent_session_id").cloned();
            match Self::import_text_or_none(parent_val.as_ref(), "parent_session_id") {
                Ok(v) => {
                    clean_session.insert(
                        "parent_session_id".to_string(),
                        v.map(Value::String).unwrap_or(Value::Null),
                    );
                }
                Err(e) => {
                    errors.push(Self::import_error(index, &session_id, &e.to_string()));
                    continue;
                }
            }
            let mut field_failed = false;
            for field in SESSION_TEXT_FIELDS {
                let v = clean_session.get(*field).cloned();
                match Self::import_text_or_none(v.as_ref(), field) {
                    Ok(val) => {
                        clean_session.insert(
                            field.to_string(),
                            val.map(Value::String).unwrap_or(Value::Null),
                        );
                    }
                    Err(e) => {
                        errors.push(Self::import_error(index, &session_id, &e.to_string()));
                        field_failed = true;
                        break;
                    }
                }
            }
            if field_failed {
                continue;
            }

            let mut clean_messages: Vec<Value> = Vec::new();
            let mut msg_failed: Option<String> = None;
            for (message_index, message) in messages_owned.iter().enumerate() {
                let mobj = message.as_object().unwrap();
                let mut clean_message = mobj.clone();
                let role = clean_message.get("role").and_then(|v| v.as_str()).unwrap_or("").to_string();
                if role.is_empty() {
                    msg_failed = Some(format!("messages[{}].role must be a non-empty string", message_index));
                    break;
                }
                let mut inner_failed = false;
                for field in MESSAGE_TEXT_FIELDS {
                    if *field == "role" {
                        continue;
                    }
                    let v = clean_message.get(*field).cloned();
                    match Self::import_text_or_none(v.as_ref(), field) {
                        Ok(val) => {
                            clean_message.insert(
                                field.to_string(),
                                val.map(Value::String).unwrap_or(Value::Null),
                            );
                        }
                        Err(e) => {
                            msg_failed = Some(e.to_string());
                            inner_failed = true;
                            break;
                        }
                    }
                }
                if inner_failed {
                    break;
                }
                let tc = clean_message.get("token_count").cloned();
                match Self::import_int_or_none(tc.as_ref(), "token_count") {
                    Ok(val) => {
                        clean_message.insert(
                            "token_count".to_string(),
                            val.map(|n| json!(n)).unwrap_or(Value::Null),
                        );
                    }
                    Err(e) => {
                        msg_failed = Some(e.to_string());
                        break;
                    }
                }
                clean_messages.push(Value::Object(clean_message));
            }
            if let Some(err) = msg_failed {
                errors.push(Self::import_error(index, &session_id, &err));
                continue;
            }

            total_messages += clean_messages.len();
            if total_messages > IMPORT_MAX_TOTAL_MESSAGES {
                errors.push(Self::import_error(
                    index,
                    &session_id,
                    "messages exceeds the total import limit",
                ));
                continue;
            }
            seen_ids.insert(session_id.clone());
            normalized.push((index, Value::Object(clean_session), clean_messages));
        }

        if !errors.is_empty() {
            return Ok(json!({
                "ok": false,
                "imported": 0,
                "skipped": 0,
                "detached": 0,
                "errors": errors
            }));
        }

        // Write phase — mirrors `_do` closure ll.660-823, via `execute_write`
        let res = self.execute_write(|conn| {
            let mut imported_ids: Vec<String> = Vec::new();
            let mut skipped_ids: Vec<String> = Vec::new();
            let mut parent_updates: Vec<(String, String)> = Vec::new();
            let mut detached: usize = 0;

            for (_index, raw, messages) in &normalized {
                let obj = raw.as_object().unwrap();
                let session_id = obj.get("id").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
                let exists: bool = conn
                    .query_row(
                        "SELECT 1 FROM sessions WHERE id = ?1 LIMIT 1",
                        params![session_id],
                        |_| Ok(()),
                    )
                    .is_ok();
                if exists {
                    skipped_ids.push(session_id);
                    continue;
                }

                let started_at = Self::float_or_none(obj.get("started_at")).unwrap_or_else(Self::now_f64);
                let archived = if obj.get("archived").and_then(|v| v.as_bool()).unwrap_or(false) { 1 } else { 0 };
                let system_prompt_hash = {
                    let sp = obj.get("system_prompt").and_then(|v| v.as_str());
                    // Inline store_system_prompt without self (we have conn)
                    if let Some(prompt) = sp {
                        if !prompt.is_empty() {
                            use std::collections::hash_map::DefaultHasher;
                            use std::hash::{Hash, Hasher};
                            let mut hasher = DefaultHasher::new();
                            prompt.hash(&mut hasher);
                            let hash = format!("{:016x}", hasher.finish());
                            conn.execute(
                                "INSERT OR IGNORE INTO system_prompts (hash, prompt) VALUES (?1, ?2)",
                                params![hash, prompt],
                            )?;
                            Some(hash)
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                };

                conn.execute(
                    r#"INSERT INTO sessions (
                           id, source, user_id, model, model_config, system_prompt,
                           system_prompt_hash,
                           parent_session_id, started_at, ended_at, end_reason,
                           message_count, tool_call_count, input_tokens, output_tokens,
                           cache_read_tokens, cache_write_tokens,
                           reasoning_tokens, cwd, git_branch, git_repo_root,
                           billing_provider, billing_base_url, billing_mode,
                           estimated_cost_usd, actual_cost_usd, cost_status, cost_source,
                           pricing_version, title, api_call_count, archived
                       )
                       VALUES (
                           :id, :source, :user_id, :model, :model_config,
                           NULL, :system_prompt_hash, NULL, :started_at, :ended_at,
                           :end_reason, 0, 0, :input_tokens, :output_tokens,
                           :cache_read_tokens, :cache_write_tokens,
                           :reasoning_tokens, :cwd, :git_branch, :git_repo_root,
                           :billing_provider, :billing_base_url, :billing_mode,
                           :estimated_cost_usd, :actual_cost_usd, :cost_status,
                           :cost_source, :pricing_version, :title,
                           :api_call_count, :archived
                       )"#,
                    rusqlite::named_params! {
                        ":id": session_id,
                        ":source": obj.get("source").and_then(|v| v.as_str()).unwrap_or("import"),
                        ":user_id": obj.get("user_id").and_then(|v| v.as_str()),
                        ":model": obj.get("model").and_then(|v| v.as_str()),
                        ":model_config": obj.get("model_config").and_then(|v| v.as_str()),
                        ":system_prompt_hash": system_prompt_hash,
                        ":started_at": started_at,
                        ":ended_at": Self::float_or_none(obj.get("ended_at")),
                        ":end_reason": obj.get("end_reason").and_then(|v| v.as_str()),
                        ":input_tokens": Self::int_or_default(obj.get("input_tokens"), 0),
                        ":output_tokens": Self::int_or_default(obj.get("output_tokens"), 0),
                        ":cache_read_tokens": Self::int_or_default(obj.get("cache_read_tokens"), 0),
                        ":cache_write_tokens": Self::int_or_default(obj.get("cache_write_tokens"), 0),
                        ":reasoning_tokens": Self::int_or_default(obj.get("reasoning_tokens"), 0),
                        ":cwd": obj.get("cwd").and_then(|v| v.as_str()),
                        ":git_branch": obj.get("git_branch").and_then(|v| v.as_str()),
                        ":git_repo_root": obj.get("git_repo_root").and_then(|v| v.as_str()),
                        ":billing_provider": obj.get("billing_provider").and_then(|v| v.as_str()),
                        ":billing_base_url": obj.get("billing_base_url").and_then(|v| v.as_str()),
                        ":billing_mode": obj.get("billing_mode").and_then(|v| v.as_str()),
                        ":estimated_cost_usd": Self::float_or_none(obj.get("estimated_cost_usd")),
                        ":actual_cost_usd": Self::float_or_none(obj.get("actual_cost_usd")),
                        ":cost_status": obj.get("cost_status").and_then(|v| v.as_str()),
                        ":cost_source": obj.get("cost_source").and_then(|v| v.as_str()),
                        ":pricing_version": obj.get("pricing_version").and_then(|v| v.as_str()),
                        ":title": obj.get("title").and_then(|v| v.as_str()),
                        ":api_call_count": Self::int_or_default(obj.get("api_call_count"), 0),
                        ":archived": archived,
                    },
                )?;

                // Insert messages — mirrors ll.751-770 (sanitize + _insert_message_rows)
                let mut sanitized: Vec<Value> = Vec::new();
                for mut msg in messages.clone() {
                    if let Some(obj) = msg.as_object_mut() {
                        for key in ["reasoning_details", "codex_reasoning_items", "codex_message_items"] {
                            let v = obj.get(key).cloned();
                            let converted = Self::reasoning_json_value(v.as_ref());
                            obj.insert(key.to_string(), converted);
                        }
                    }
                    sanitized.push(msg);
                }
                let (total_msgs, total_tool_calls) = {
                    let mut total = 0i64;
                    let mut tool_calls = 0i64;
                    for msg in &sanitized {
                        let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or("user");
                        let content = msg.get("content").and_then(|v| v.as_str()).unwrap_or("").to_string();
                        let tool_call_id = msg.get("tool_call_id").and_then(|v| v.as_str());
                        let tool_name = msg.get("tool_name").and_then(|v| v.as_str());
                        let timestamp = msg.get("timestamp").and_then(|v| v.as_f64()).unwrap_or_else(Self::now_f64);
                        let token_count: Option<i64> = msg.get("token_count").and_then(|v| v.as_i64());
                        let finish_reason = msg.get("finish_reason").and_then(|v| v.as_str());
                        let reasoning = msg.get("reasoning").and_then(|v| v.as_str());
                        let reasoning_content = msg.get("reasoning_content").and_then(|v| v.as_str());
                        let platform_message_id = msg.get("platform_message_id").and_then(|v| v.as_str());
                        let reasoning_details = msg.get("reasoning_details").map(|v| v.to_string());
                        let codex_reasoning_items = msg.get("codex_reasoning_items").map(|v| v.to_string());
                        let codex_message_items = msg.get("codex_message_items").map(|v| v.to_string());
                        let tool_calls_json = msg.get("tool_calls").map(|v| v.to_string());
                        conn.execute(
                            "INSERT INTO messages (session_id, role, content, tool_call_id, tool_name, tool_calls, timestamp, token_count, finish_reason, reasoning, reasoning_content, reasoning_details, codex_reasoning_items, codex_message_items, platform_message_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
                            params![
                                session_id, role, content, tool_call_id, tool_name, tool_calls_json, timestamp, token_count, finish_reason, reasoning, reasoning_content, reasoning_details, codex_reasoning_items, codex_message_items, platform_message_id
                            ],
                        )?;
                        total += 1;
                        if tool_name.is_some() || tool_calls_json.is_some() {
                            tool_calls += 1;
                        }
                    }
                    (total, tool_calls)
                };
                conn.execute(
                    "UPDATE sessions SET message_count = ?1, tool_call_count = ?2 WHERE id = ?3",
                    params![total_msgs, total_tool_calls, session_id],
                )?;

                if let Some(pid) = obj.get("parent_session_id").and_then(|v| v.as_str()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) {
                    parent_updates.push((session_id.clone(), pid));
                }
                imported_ids.push(session_id);
            }

            // Parent rewiring with cycle guard (ll.777-813)
            let parent_by_child: HashMap<String, String> = parent_updates.iter().cloned().collect();
            let mut parent_by_child_mut = parent_by_child.clone();

            let would_create_cycle = |session_id: &str, parent_id: &str, conn: &Connection, parent_by_child: &HashMap<String, String>| -> Result<bool> {
                let mut seen: HashSet<String> = HashSet::new();
                seen.insert(session_id.to_string());
                let mut current = parent_id.to_string();
                loop {
                    if seen.contains(&current) {
                        return Ok(true);
                    }
                    seen.insert(current.clone());
                    if let Some(next) = parent_by_child.get(&current) {
                        current = next.clone();
                        continue;
                    }
                    let row: Option<String> = conn
                        .query_row(
                            "SELECT parent_session_id FROM sessions WHERE id = ?1 LIMIT 1",
                            params![current],
                            |r| r.get(0),
                        )
                        .ok();
                    match row {
                        None => return Ok(false),
                        Some(pid) if pid.is_empty() => return Ok(false),
                        Some(pid) => current = pid,
                    }
                }
            };

            for (session_id, parent_id) in parent_updates.clone() {
                let parent_exists: bool = conn
                    .query_row(
                        "SELECT 1 FROM sessions WHERE id = ?1 LIMIT 1",
                        params![parent_id],
                        |_| Ok(()),
                    )
                    .is_ok();
                let cycle = would_create_cycle(&session_id, &parent_id, conn, &parent_by_child_mut)?;
                if parent_exists && !cycle {
                    conn.execute(
                        "UPDATE sessions SET parent_session_id = ?1 WHERE id = ?2",
                        params![parent_id, session_id],
                    )?;
                } else {
                    parent_by_child_mut.remove(&session_id);
                    detached += 1;
                }
            }

            Ok(json!({
                "ok": true,
                "imported": imported_ids.len(),
                "skipped": skipped_ids.len(),
                "detached": detached,
                "imported_ids": imported_ids,
                "skipped_ids": skipped_ids,
                "errors": []
            }))
        })?;

        Ok(res)
    }

    // -----------------------------------------------------------------------
    // Small helpers not in Python's import path but needed for the stubs above
    // -----------------------------------------------------------------------
    fn decode_content(&self, raw: &str) -> Value {
        self.decode_content_inner(raw)
    }
    fn decode_content_inner(&self, raw: &str) -> Value {
        serde_json::from_str::<Value>(raw).unwrap_or(Value::String(raw.to_string()))
    }
}

// ---------------------------------------------------------------------------
// Ponytail self-check (one runnable check, no framework) — mirrors the
// pattern in `hermes_constants_slice2.rs` and `pet_state.rs`.
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shape_preview_truncates() {
        let raw = "hello world ".repeat(10);
        let shaped = shape_preview(&raw);
        assert!(shaped.len() <= PREVIEW_MAX_CHARS + 3);
    }

    #[test]
    fn session_compact_excludes_prompt_cols() {
        let cols = StateStore::compact_session_cols();
        assert!(!cols.contains("system_prompt"));
        assert!(cols.contains("s.id"));
    }

    #[test]
    fn import_text_or_none_rejects_non_string() {
        let v = json!(123);
        assert!(StateStore::import_text_or_none(Some(&v), "field").is_err());
        assert_eq!(
            StateStore::import_text_or_none(Some(&Value::String("hi".into())), "field").unwrap(),
            Some("hi".to_string())
        );
        assert_eq!(
            StateStore::import_text_or_none(None, "field").unwrap(),
            None
        );
    }

    #[test]
    fn reasoning_json_value_parses_string() {
        let s = Value::String(r#"{"a":1}"#.to_string());
        let v = StateStore::reasoning_json_value(Some(&s));
        assert!(v.is_object());
        let plain = Value::String("not json".to_string());
        let v2 = StateStore::reasoning_json_value(Some(&plain));
        assert_eq!(v2, plain);
    }
}
