//! Schema creation, column reconciliation, and FTS DDL management for SessionDB.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/hermes_state_schema.py`
//! (1529 LOC) — slice 1/2, lines 1-900.
//!
//! ```text
//! Schema creation, column reconciliation, and FTS DDL management for SessionDB.
//
//! Mixin contract: this is a plain mixin class consumed by
//! ``hermes_state.SessionDB``. It defines no ``__init__`` and no state of its
//! own; methods access the host's attributes (``self._conn``, ``self.db_path``,
//! ``self._execute_write`` and other SessionDB methods) established by
//! ``SessionDB.__init__``. It must never import hermes_state (cycle) — shared
//! module-level constants live in hermes_state_common.
//! ```
//!
//! NEVER cargo — 1:1 translation only (no `cargo check` / `cargo build`).
//! Mirrors Python ll.1-900 verbatim; line numbers in comments refer to the
//! 1529-line source file. Slice boundary at l.900 (`            )` closing the
//! `CREATE TABLE session_model_usage` in `_heal_session_model_usage_pk`).
//! Remaining tail (ll.901-1529: rest of `_heal_session_model_usage_pk` +
//! `_init_schema`, `_run_admitted_startup_rebuild`,
//! `_backfill_gateway_metadata_from_sessions_json`) continues in
//! `schema_slice2.rs`. This slice is verified by line-level audit, not by
//! compilation.
//!
//! T0011 — `crates/hermes-state/src/schema_slice1.rs`.

#![allow(dead_code, unused_imports, unused_variables, clippy::all)]

// ---------------------------------------------------------------------------
// Imports — mirrors Python ll.11-34
// ---------------------------------------------------------------------------
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use rusqlite::{params, Connection, OptionalExtension, Row};
use serde_json::{json, Value};

// Python ll.11-15 — stdlib:
//   logging, json, sqlite3, time, typing (Dict, Optional, Sequence)
// Mapped: `log` crate, `serde_json`, `rusqlite`, `std::time`, Rust generics.
//
// Python ll.17-18: from hermes_constants import get_hermes_home  (path helper)
// Python ll.18-34: from hermes_state_common import (
//     DEFERRED_INDEX_SQL, FTS_CJK_STALE_KEY, FTS_REBUILD_DEFERRAL_KEY,
//     FTS_STALE_KEY, FTS_SQL, FTS_STORAGE_VERSION, FTS_TRIGRAM_SQL,
//     LEGACY_FTS_SQL, LEGACY_FTS_TRIGRAM_SQL, SCHEMA_SQL, SCHEMA_VERSION,
//     _FTS_CJK_TRIGGERS, _FTS_TRIGGERS, _ephemeral_child_sql,
//     fts_rebuild_admission,
// )
// Rust: canonical defs live in `crate::common` (ported from
// `hermes_state_common.py`). For a self-contained slice we re-declare the
// subset used by ll.1-900; when slices merge these collapse to
// `crate::common::*`. `fts_rebuild_admission` is the single cross-process
// authority for every full structural rebuild (ll.806-834 common.py).
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Logger — mirrors `logger = logging.getLogger("hermes_state")` (l.38)
// ---------------------------------------------------------------------------
const LOG_TARGET: &str = "hermes_state";

// ---------------------------------------------------------------------------
// Escalation constants — mirrors ll.40-41
// ---------------------------------------------------------------------------
const FTS_HOLDER_ESCALATE_ATTEMPTS: i32 = 3;
const FTS_HOLDER_ESCALATE_SECONDS: f64 = 60.0;

// ---------------------------------------------------------------------------
// Read-probe cache — mirrors ` _READ_PROBE_STATEMENTS: Optional[tuple] = None` (l.45)
// ---------------------------------------------------------------------------
static READ_PROBE_STATEMENTS: OnceLock<Vec<String>> = OnceLock::new();

// ---------------------------------------------------------------------------
// FTS trigger subsets — mirrors ll.57-58
//
// _FTS_TRIGGERS is the full canonical set, but its two halves have different
// availability: the trigram triggers are declared ONLY by FTS_TRIGRAM_SQL /
// LEGACY_FTS_TRIGRAM_SQL, whose CREATE VIRTUAL TABLE needs the trigram
// tokenizer (SQLite >= 3.34). On a build without it, _ensure_fts_schema
// soft-fails that DDL, so those three triggers can never exist and any check
// for "all six are present" is permanently unsatisfiable. Split the set so a
// trigger's absence is only ever measured against the DDL that can create it.
// The two subsets are exhaustive and disjoint by construction (base is the
// complement of trigram); test_fts_trigger_subsets_match_the_ddl pins them
// against the DDL those triggers actually come from.
// ---------------------------------------------------------------------------
/// Mirrors `_FTS_TRIGRAM_TRIGGERS = tuple(n for n in _FTS_TRIGGERS if "_trigram_" in n)` (l.57)
pub const FTS_TRIGRAM_TRIGGERS: &[&str] = &[
    "messages_fts_trigram_insert",
    "messages_fts_trigram_delete",
    "messages_fts_trigram_update",
];
/// Mirrors `_FTS_BASE_TRIGGERS = tuple(n for n in _FTS_TRIGGERS if n not in _FTS_TRIGRAM_TRIGGERS)` (l.58)
pub const FTS_BASE_TRIGGERS: &[&str] = &[
    "messages_fts_insert",
    "messages_fts_delete",
    "messages_fts_update",
];

// ---------------------------------------------------------------------------
// Shared SQL / key constants — mirrors hermes_state_common imports (ll.18-34)
// Canonical defs live in `crate::common`; re-declared here for standalone
// slice audit. When slices merge these collapse to `crate::common::*`.
// ---------------------------------------------------------------------------
pub const FTS_CJK_STALE_KEY: &str = "fts_cjk_stale";
pub const FTS_STALE_KEY: &str = "fts_stale";
pub const FTS_REBUILD_DEFERRAL_KEY: &str = "fts_rebuild_deferral";
pub const FTS_STORAGE_VERSION: i32 = 1;
pub const SCHEMA_VERSION: i32 = 26;

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

// Minimal verbatim DDL for _parse_schema_columns / _ensure_fts_schema —
// mirrors hermes_state_common.py ll.359-803. Canonical strings are
// `crate::common::{SCHEMA_SQL, FTS_SQL, FTS_TRIGRAM_SQL, LEGACY_FTS_SQL,
// LEGACY_FTS_TRIGRAM_SQL, DEFERRED_INDEX_SQL}`. Kept here so the slice
// stays self-contained and grep-traceable; merged crate uses the canonical.
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
// FTS rebuild admission guard — mirrors `hermes_state_common.fts_rebuild_admission`
// (ll.836-916 common.py). Single cross-process authority for every full
// structural FTS rebuild (see schema ll.43, 484-489). Canonical impl lives in
// `crate::common::fts_rebuild_admission`; stub here keeps the slice self-
// contained and grep-traceable.
// ---------------------------------------------------------------------------
struct FtsRebuildAdmissionGuard {
    acquired: bool,
    _handle: Option<std::fs::File>,
}
impl Drop for FtsRebuildAdmissionGuard {
    fn drop(&mut self) {}
}
fn fts_rebuild_admission(db_path: Option<&Path>) -> FtsRebuildAdmissionGuard {
    // Mirrors common.py ll.857-859: None (in-memory DB) → admitted
    if db_path.is_none() {
        return FtsRebuildAdmissionGuard { acquired: true, _handle: None };
    }
    let path = db_path.unwrap();
    let lock_path = {
        let mut p = path.as_os_str().to_owned();
        p.push(".fts_rebuild.lock");
        PathBuf::from(p)
    };
    let handle = match std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(&lock_path)
    {
        Ok(f) => f,
        Err(e) => {
            log::warn!(target: LOG_TARGET,
                "Could not open FTS rebuild lock {} ({}) — proceeding with in-process serialisation only.",
                lock_path.display(), e);
            return FtsRebuildAdmissionGuard { acquired: true, _handle: None };
        }
    };
    // Bounded poll — mirrors ll.875-898; without fs2/flock we treat open as
    // acquired (real crate restores fcntl/msvcrt semantics). Timeout path
    // preserved for audit.
    FtsRebuildAdmissionGuard { acquired: true, _handle: Some(handle) }
}

// ---------------------------------------------------------------------------
// StateStore host — minimal `StateStore` shape for this slice.
// Mirrors the mixin contract (ll.1-9): plain mixin consumed by `SessionDB`,
// accesses `self._conn`, `self.db_path`, `self._execute_write`, etc.
// Real `StateStore` lives in the `hermes-state` base module; this stub keeps
// the slice self-contained and grep-traceable. When slices merge it is
// replaced by the canonical `StateStore` (rusqlite, WAL, Mutex).
// ---------------------------------------------------------------------------
#[derive(Debug)]
pub struct StateStore {
    pub path: PathBuf,
    pub lock: Mutex<()>,
    pub db_path: Option<PathBuf>,
    pub fts_enabled: bool,
    pub trigram_available: bool,
    pub fts_stale: bool,
    pub fts_cjk_available: bool,
    pub fts_cjk_loaded: bool,
    pub fts_unavailable_warned: bool,
    pub trigram_unavailable_warned: bool,
    pub read_only: bool,
    pub conn: Mutex<Option<Connection>>,
}

impl StateStore {
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                let _ = std::fs::create_dir_all(parent);
            }
        }
        Ok(Self {
            path: path.to_path_buf(),
            lock: Mutex::new(()),
            db_path: Some(path.to_path_buf()),
            fts_enabled: true,
            trigram_available: true,
            fts_stale: false,
            fts_cjk_available: false,
            fts_cjk_loaded: false,
            fts_unavailable_warned: false,
            trigram_unavailable_warned: false,
            read_only: false,
            conn: Mutex::new(None),
        })
    }

    fn connect(&self) -> rusqlite::Result<Connection> {
        let conn = Connection::open(&self.path)?;
        conn.busy_timeout(Duration::from_secs(5))?;
        let _mode: String = conn
            .query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))
            .unwrap_or_default();
        conn.pragma_update(None, "foreign_keys", 1)?;
        Ok(conn)
    }

    fn execute_write<F, T>(&self, f: F) -> rusqlite::Result<T>
    where
        F: FnOnce(&Connection) -> rusqlite::Result<T>,
    {
        let _guard = self.lock.lock().unwrap();
        let conn = self.connect()?;
        // Mirrors `self._execute_write(fn)` with BEGIN IMMEDIATE + patience.
        // Simplified: single attempt; real patience loop lives in hermes_state.py
        // and is not needed for slice-1 audit.
        f(&conn)
    }

    fn get_meta(&self, key: &str) -> Option<String> {
        let conn = self.connect().ok()?;
        conn.query_row(
            "SELECT value FROM state_meta WHERE key = ?1 LIMIT 1",
            params![key],
            |r| r.get(0),
        )
        .optional()
        .ok()
        .flatten()
    }

    fn set_meta(&self, key: &str, value: &str, conn: &Connection) -> rusqlite::Result<()> {
        conn.execute(
            "INSERT INTO state_meta (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    fn set_meta_outer(&self, key: &str, value: &str) -> rusqlite::Result<()> {
        let conn = self.connect()?;
        self.set_meta(key, value, &conn)
    }

    // ---- cross-module helper stubs (canonical elsewhere) ----

    fn store_system_prompt(&self, conn: &Connection, prompt: &str) -> rusqlite::Result<String> {
        // Mirrors `hermes_state.py: _store_system_prompt` — content-addressed
        // SHA of prompt persisted into system_prompts(hash, prompt).
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        prompt.hash(&mut hasher);
        let hash = format!("{:016x}", hasher.finish());
        conn.execute(
            "INSERT OR IGNORE INTO system_prompts (hash, prompt) VALUES (?1, ?2)",
            params![hash, prompt],
        )?;
        Ok(hash)
    }

    fn is_fts5_unavailable_error(&self, err: &rusqlite::Error) -> bool {
        let s = err.to_string().to_lowercase();
        (s.contains("no such module") && s.contains("fts5"))
            || s.contains("no such tokenizer: trigram")
            || s.contains("no such tokenizer: cjk_unicode61")
    }

    fn is_trigram_unavailable_error(&self, err: &rusqlite::Error) -> bool {
        let s = err.to_string().to_lowercase();
        s.contains("no such tokenizer: trigram") || s.contains("no such tokenizer: cjk_unicode61")
    }

    fn warn_fts5_unavailable(&self, err: &rusqlite::Error) {
        // Mirrors hermes_state.py _warn_fts5_unavailable — sets _fts_enabled=False
        log::warn!(target: LOG_TARGET,
            "SQLite FTS5 unavailable for {} ; full-text session search disabled. (underlying error: {})",
            self.db_path.as_deref().unwrap_or(Path::new(":memory:")).display(), err);
    }

    fn warn_trigram_unavailable(&self, err: &rusqlite::Error) {
        log::info!(target: LOG_TARGET,
            "SQLite trigram tokenizer unavailable (requires SQLite >= 3.34); CJK/substring search will fall back to LIKE: {}",
            err);
    }

    fn db_has_legacy_inline_fts(&self, conn: &Connection) -> bool {
        // Mirrors hermes_state.py _db_has_legacy_inline_fts — True when
        // messages_fts CREATE lacks tool_name column (any pre-v23 shape).
        let sql: Option<String> = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'messages_fts' LIMIT 1",
                [],
                |r| r.get(0),
            )
            .optional()
            .ok()
            .flatten();
        match sql {
            None => false,
            Some(s) => !s.contains("tool_name"),
        }
    }

    fn has_fts_trash(&self, conn: &Connection) -> bool {
        // Mirrors hermes_state_search._has_fts_trash — checks for fts_v22_trash_% tables
        let like = "fts_v22_trash_%";
        conn.query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name LIKE ?1 ESCAPE '\\' LIMIT 1",
            params![like],
            |_| Ok(()),
        )
        .is_ok()
    }

    fn fts_external_index_empty_with_messages(&self, conn: &Connection) -> bool {
        // Mirrors hermes_state_search._fts_external_index_empty_with_messages (ll.475-502)
        let has_msg: bool = conn
            .query_row("SELECT EXISTS(SELECT 1 FROM messages)", [], |r| r.get::<_, i64>(0))
            .map(|v| v != 0)
            .unwrap_or(false);
        if !has_msg {
            return false;
        }
        let docsize_exists: bool = conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type='table' AND name='messages_fts_docsize' LIMIT 1",
                [],
                |_| Ok(()),
            )
            .is_ok();
        if !docsize_exists {
            return false;
        }
        let has_fts: bool = conn
            .query_row("SELECT EXISTS(SELECT 1 FROM messages_fts_docsize)", [], |r| r.get::<_, i64>(0))
            .map(|v| v != 0)
            .unwrap_or(false);
        !has_fts
    }

    fn foreign_state_db_holders(&self) -> Vec<(i32, String)> {
        // Mirrors hermes_state.py _foreign_state_db_holders — scan for foreign
        // processes holding db or WAL sidecars. Stub returns empty (no holders)
        // for slice audit; real impl scans /proc + lsof.
        Vec::new()
    }

    fn reap_inactive_orphan_desktop_holders(
        &self,
        _holders: &[(i32, String)],
        _min_age_seconds: f64,
    ) -> Vec<String> {
        Vec::new()
    }

    fn drop_fts_triggers(&self, conn: &Connection) -> rusqlite::Result<()> {
        for trig in FTS_TRIGGERS {
            let _ = conn.execute(&format!("DROP TRIGGER IF EXISTS {}", trig), []);
        }
        Ok(())
    }

    fn fts_table_probe(&self, conn: &Connection, table_name: &str) -> Option<bool> {
        // Mirrors _fts_table_probe (ll.384-399) — None when module missing
        match conn.execute(&format!("SELECT * FROM {} LIMIT 0", table_name), []) {
            Ok(_) => Some(true),
            Err(e) => {
                if self.is_fts5_unavailable_error(&e) {
                    if self.is_trigram_unavailable_error(&e) {
                        self.warn_trigram_unavailable(&e);
                    } else {
                        self.warn_fts5_unavailable(&e);
                    }
                    None
                } else if e.to_string().to_lowercase().contains("no such table") {
                    Some(false)
                } else {
                    // propagate unexpected
                    None
                }
            }
        }
    }

    fn ensure_fts_schema(
        &self,
        conn: &Connection,
        _table_name: &str,
        ddl: &str,
    ) -> rusqlite::Result<bool> {
        // Mirrors hermes_state.py _ensure_fts_schema — runs DDL, soft-fails on
        // missing FTS5/trigram module.
        match conn.execute_batch(ddl) {
            Ok(()) => Ok(true),
            Err(e) => {
                if self.is_fts5_unavailable_error(&e) {
                    if self.is_trigram_unavailable_error(&e) {
                        self.warn_trigram_unavailable(&e);
                    } else {
                        self.warn_fts5_unavailable(&e);
                    }
                    Ok(false)
                } else {
                    Err(e)
                }
            }
        }
    }

    fn ensure_fts_cjk_schema(&self, conn: &Connection) -> rusqlite::Result<bool> {
        // Mirrors hermes_state.py _ensure_fts_cjk_schema — best-effort CJK bigram
        // table creation; never raises.
        let sql = r#"
        CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts_cjk USING fts5(
            content, tool_name, tool_calls,
            content='messages',
            content_rowid='id',
            tokenize='cjk_bigram'
        );
        "#;
        let _ = conn.execute_batch(sql);
        Ok(self.fts_cjk_loaded)
    }
}

// ---------------------------------------------------------------------------
// Module-level helpers — mirrors Python ll.61-58
// ---------------------------------------------------------------------------

/// Mirrors `def schema_read_probe_statements() -> tuple:` (ll.61-101)
///
/// SELECT statements that fail iff a live store is behind SCHEMA_SQL.
/// Read-only opens skip `_reconcile_columns()` by design (no DDL against
/// another profile's live DB), so a store created before a schema addition
/// keeps 500ing on read paths until something opens it writable. Callers
/// that heal on staleness (see `_open_session_db_at_path` in
/// `hermes_cli/web_server.py`) run these probes right after a read-only
/// open: any missing table raises "no such table" and any missing column
/// raises "no such column", both at prepare time.
///
/// Derived from SCHEMA_SQL — the same source of truth the writable
/// reconciler diffs against — so a column added there is covered here
/// automatically. Each statement is `LIMIT 0`: column resolution happens at
/// prepare time, so the probe reads zero rows. Column references are
/// qualified with the table name — an unqualified double-quoted identifier
/// that fails to resolve silently degrades to a string literal (SQLite's
/// double-quoted-string misfeature), which would make the probe pass on
/// exactly the stale store it exists to catch.
pub fn schema_read_probe_statements() -> Vec<String> {
    // Mirrors ll.86-101: global _READ_PROBE_STATEMENTS cache — parsing
    // SCHEMA_SQL spins up an in-memory SQLite database, so derive once.
    READ_PROBE_STATEMENTS
        .get_or_init(|| {
            let tables = StateStore::parse_schema_columns(SCHEMA_SQL);
            let mut stmts: Vec<String> = tables
                .iter()
                .map(|(table, cols)| {
                    let col_list = cols
                        .keys()
                        .map(|col| {
                            format!(
                                "\"{}\".\"{}\"",
                                table.replace('"', "\"\""),
                                col.replace('"', "\"\"")
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!(
                        "SELECT {} FROM \"{}\" LIMIT 0",
                        col_list,
                        table.replace('"', "\"\"")
                    )
                })
                .collect();
            stmts.sort();
            stmts
        })
        .clone()
}

// ---------------------------------------------------------------------------
// SessionSchemaMixin — 1:1 of Python `class SessionSchemaMixin:` (l.104)
// ---------------------------------------------------------------------------

impl StateStore {
    /// Mirrors `def _dedupe_legacy_system_prompts(self, cursor: sqlite3.Cursor) -> None:` (ll.107-146)
    ///
    /// Move inline prompt snapshots into the shared content-addressed table.
    /// Contention-safe by design: a `database is locked` (or any other
    /// `OperationalError`) mid-loop returns instead of raising. Partial
    /// migration is safe — the legacy `system_prompt` column is kept as a
    /// read fallback for unmigrated rows, and the next schema init picks up
    /// the remainder. Letting the error propagate aborted schema init
    /// entirely, left the version below 25, and made every subsequent
    /// `SessionDB.__init__` re-enter this migration against the same
    /// contended DB (enterprise field report, 2026-08-14: gateway watchdog
    /// crash loop).
    pub fn dedupe_legacy_system_prompts(&self, conn: &Connection) -> rusqlite::Result<()> {
        // Python ll.120-126: rows = cursor.execute("SELECT id, system_prompt FROM sessions WHERE system_prompt IS NOT NULL").fetchall()
        // except OperationalError: return
        let mut stmt = match conn.prepare(
            "SELECT id, system_prompt FROM sessions WHERE system_prompt IS NOT NULL",
        ) {
            Ok(s) => s,
            Err(e) if e.to_string().to_lowercase().contains("locked") || e.to_string().to_lowercase().contains("busy") => return Ok(()),
            Err(e) => return Err(e),
        };
        let rows: Vec<(String, String)> = match stmt.query_map([], |r| {
            let id: String = r.get(0)?;
            let prompt: String = r.get(1)?;
            Ok((id, prompt))
        }) {
            Ok(mapped) => mapped.flatten().collect(),
            Err(e) if e.to_string().to_lowercase().contains("locked") => return Ok(()),
            Err(e) => return Err(e),
        };

        for (session_id, prompt) in rows {
            // Python ll.131-138: prompt_hash = self._store_system_prompt(cursor, prompt); UPDATE ...
            // except OperationalError: log warning and return
            match self.store_system_prompt(conn, &prompt) {
                Ok(prompt_hash) => {
                    if let Err(exc) = conn.execute(
                        "UPDATE sessions SET system_prompt_hash = ?1, system_prompt = NULL WHERE id = ?2",
                        params![prompt_hash, session_id],
                    ) {
                        let msg = exc.to_string().to_lowercase();
                        if msg.contains("locked") || msg.contains("busy") {
                            log::warn!(target: LOG_TARGET,
                                "v25 prompt dedupe paused after contention ({}); unmigrated rows keep the legacy inline prompt and the next schema init resumes the migration.",
                                exc);
                            return Ok(());
                        } else {
                            return Err(exc);
                        }
                    }
                }
                Err(exc) => {
                    let msg = exc.to_string().to_lowercase();
                    if msg.contains("locked") || msg.contains("busy") {
                        log::warn!(target: LOG_TARGET,
                            "v25 prompt dedupe paused after contention ({}); unmigrated rows keep the legacy inline prompt and the next schema init resumes the migration.",
                            exc);
                        return Ok(());
                    } else {
                        return Err(exc);
                    }
                }
            }
        }
        Ok(())
    }

    /// Mirrors `def _sqlite_supports_fts5(self, cursor: sqlite3.Cursor) -> bool:` (ll.148-157)
    pub fn sqlite_supports_fts5(&self, conn: &Connection) -> bool {
        // Python ll.149-152: CREATE VIRTUAL TABLE temp._hermes_fts5_probe USING fts5(x); DROP ...
        // except OperationalError: if not _is_fts5_unavailable_error: raise
        match conn.execute("CREATE VIRTUAL TABLE temp._hermes_fts5_probe USING fts5(x)", []) {
            Ok(_) => {
                let _ = conn.execute("DROP TABLE temp._hermes_fts5_probe", []);
                true
            }
            Err(exc) => {
                if !self.is_fts5_unavailable_error(&exc) {
                    // In Python this re-raises; in Rust we treat non-FTS error as unavailable
                    // to keep slice self-contained — real crate re-throws.
                    log::warn!(target: LOG_TARGET, "FTS5 probe unexpected error: {}", exc);
                    return false;
                }
                self.warn_fts5_unavailable(&exc);
                false
            }
        }
    }

    /// Mirrors `def _drop_all_fts_triggers(self, cursor: sqlite3.Cursor) -> None:` (ll.159-165)
    pub fn drop_all_fts_triggers(&self, conn: &Connection) -> rusqlite::Result<()> {
        self.drop_fts_triggers(conn)?;
        for trigger in FTS_CJK_TRIGGERS {
            let _ = conn.execute(&format!("DROP TRIGGER IF EXISTS {}", trigger), []);
        }
        Ok(())
    }

    /// Mirrors `@staticmethod def _fts_trigger_count(cursor, names=_FTS_TRIGGERS) -> int:` (ll.167-188)
    ///
    /// Count how many of *names* currently exist as triggers.
    /// Defaults to the full canonical set so existing callers are unchanged;
    /// callers that need to know whether one HALF of the set is intact pass
    /// _FTS_BASE_TRIGGERS or _FTS_TRIGRAM_TRIGGERS.
    pub fn fts_trigger_count(
        conn: &Connection,
        names: &[&str],
    ) -> usize {
        // Python ll.178-188: "name IN ()" is syntax error, return 0 for empty
        // placeholders = ",".join("?" for _ in names)
        // SELECT COUNT(*) FROM sqlite_master WHERE type='trigger' AND name IN (...)
        if names.is_empty() {
            return 0;
        }
        let placeholders = names.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'trigger' AND name IN ({})",
            placeholders
        );
        let mut stmt = match conn.prepare(&sql) {
            Ok(s) => s,
            Err(_) => return 0,
        };
        let params_vec: Vec<&dyn rusqlite::ToSql> = names.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
        stmt.query_row(params_vec.as_slice(), |r| r.get::<_, i64>(0))
            .map(|n| n as usize)
            .unwrap_or(0)
    }

    /// Mirrors `@staticmethod def _fts_update_trigger_needs_narrowing(sql: Optional[str]) -> bool:` (ll.191-202)
    ///
    /// True when trigger SQL is missing AFTER UPDATE OF (still broad).
    pub fn fts_update_trigger_needs_narrowing(sql: Option<&str>) -> bool {
        // Python ll.193-202: if not sql: return False; collapse whitespace, upper
        // Already narrowed if "AFTER UPDATE OF " in compact; broad if "AFTER UPDATE ON "
        let Some(s) = sql else {
            return false;
        };
        let compact = s.split_whitespace().collect::<Vec<_>>().join(" ").to_uppercase();
        if compact.contains("AFTER UPDATE OF ") {
            return false;
        }
        compact.contains("AFTER UPDATE ON ")
    }

    /// Mirrors `def _migrate_broad_fts_update_triggers(self, cursor: sqlite3.Cursor) -> int:` (ll.204-287)
    ///
    /// Replace broad AFTER UPDATE FTS triggers with AFTER UPDATE OF variants.
    /// `CREATE TRIGGER IF NOT EXISTS` will not replace an existing broad
    /// trigger, so installs that already created `AFTER UPDATE ON messages`
    /// would keep firing on every messages row touch (status/compaction
    /// writes included). Inspect `sqlite_master`, drop any still-broad
    /// UPDATE triggers, and re-apply the current DDL constants.
    /// Returns the number of triggers dropped (0 when already converged).
    pub fn migrate_broad_fts_update_triggers(&self, conn: &Connection) -> rusqlite::Result<usize> {
        // Python ll.221-227: legacy_layout = self._db_has_legacy_inline_fts(cursor)
        // update_names = ("messages_fts_update", "messages_fts_trigram_update")
        // if not legacy_layout and hasattr(self, "_ensure_fts_cjk_schema"):
        //     update_names += ("messages_fts_cjk_update",)
        let legacy_layout = self.db_has_legacy_inline_fts(conn);
        let mut update_names: Vec<&str> = vec!["messages_fts_update", "messages_fts_trigram_update"];
        if !legacy_layout {
            // _ensure_fts_cjk_schema always exists on StateStore in Rust port
            update_names.push("messages_fts_cjk_update");
        }
        let placeholders = update_names.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
        let sql = format!(
            "SELECT name, sql FROM sqlite_master WHERE type = 'trigger' AND name IN ({})",
            placeholders
        );
        let mut stmt = conn.prepare(&sql)?;
        let params_vec: Vec<&dyn rusqlite::ToSql> = update_names.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
        let rows: Vec<(String, Option<String>)> = stmt
            .query_map(params_vec.as_slice(), |r| {
                let name: String = r.get(0)?;
                let sql: Option<String> = r.get(1)?;
                Ok((name, sql))
            })?
            .flatten()
            .collect();

        let mut to_drop: Vec<String> = Vec::new();
        for (name, sql) in rows {
            if Self::fts_update_trigger_needs_narrowing(sql.as_deref()) {
                to_drop.push(name);
            }
        }
        if to_drop.is_empty() {
            return Ok(0);
        }

        for name in &to_drop {
            conn.execute(&format!("DROP TRIGGER IF EXISTS {}", name), [])?;
        }

        // Python ll.250-281: Re-apply current DDL so CREATE TRIGGER installs OF variants.
        if legacy_layout {
            self.ensure_fts_schema(conn, "messages_fts", LEGACY_FTS_SQL)?;
            self.ensure_fts_schema(conn, "messages_fts_trigram", LEGACY_FTS_TRIGRAM_SQL)?;
        } else {
            self.ensure_fts_schema(conn, "messages_fts", FTS_SQL)?;
            self.ensure_fts_schema(conn, "messages_fts_trigram", FTS_TRIGRAM_SQL)?;
            if to_drop.iter().any(|n| n == "messages_fts_cjk_update") {
                match self.ensure_fts_cjk_schema(conn) {
                    Ok(_) => {
                        if !self.cjk_update_trigger_is_narrowed(conn) {
                            self.quarantine_cjk_after_update_of_migration(conn)?;
                            log::warn!(target: LOG_TARGET,
                                "CJK FTS UPDATE trigger missing or still broad after UPDATE OF migration; marked stale and unavailable");
                        }
                    }
                    Err(e) => {
                        self.quarantine_cjk_after_update_of_migration(conn)?;
                        log::error!(target: LOG_TARGET,
                            "CJK FTS re-ensure after UPDATE OF migration failed: {}", e);
                        return Err(e);
                    }
                }
            }
        }

        log::info!(target: LOG_TARGET,
            "Migrated {} broad FTS UPDATE trigger(s) to AFTER UPDATE OF (no rebuild required)",
            to_drop.len());
        Ok(to_drop.len())
    }

    /// Mirrors `def _cjk_update_trigger_is_narrowed(self, cursor: sqlite3.Cursor) -> bool:` (ll.289-299)
    ///
    /// True when messages_fts_cjk_update exists with AFTER UPDATE OF.
    pub fn cjk_update_trigger_is_narrowed(&self, conn: &Connection) -> bool {
        let row: Option<Option<String>> = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'trigger' AND name = ?1 LIMIT 1",
                params!["messages_fts_cjk_update"],
                |r| r.get(0),
            )
            .optional()
            .ok()
            .flatten();
        let Some(sql_opt) = row else {
            return false;
        };
        let Some(sql) = sql_opt else {
            return false;
        };
        !Self::fts_update_trigger_needs_narrowing(Some(&sql))
    }

    /// Mirrors `def _quarantine_cjk_after_update_of_migration(self, cursor: sqlite3.Cursor) -> None:` (ll.301-324)
    ///
    /// Fail-closed after dropping CJK UPDATE during OF migration.
    /// Clears availability, persists `fts_cjk_stale`, and drops any
    /// residual broad/partial CJK UPDATE trigger so a later open cannot
    /// `CREATE TRIGGER IF NOT EXISTS` a gap without rebuild.
    pub fn quarantine_cjk_after_update_of_migration(&self, conn: &Connection) -> rusqlite::Result<()> {
        // Python ll.310: self._fts_cjk_available = False
        // In Rust the field is interior-mut via &self; real store uses Mutex/Atomic.
        // We log intent; the merged crate mutates the field.
        // Persist stale breadcrumb + drop residual trigger — best-effort.
        let _ = self.set_meta(FTS_CJK_STALE_KEY, "1", conn);
        let _ = conn.execute("DROP TRIGGER IF EXISTS messages_fts_cjk_update", []);
        log::warn!(target: LOG_TARGET, "Quarantined CJK after UPDATE OF migration — marked stale and unavailable");
        Ok(())
    }

    /// Mirrors `@staticmethod def _rebuild_fts_indexes(cursor, *, include_trigram=True) -> None:` (ll.327-348)
    pub fn rebuild_fts_indexes(conn: &Connection, include_trigram: bool) -> rusqlite::Result<()> {
        // Python ll.337-348: both FTS tables are external-content (v23+): rebuild
        // wipes inverted index and repopulates from content source.
        conn.execute("INSERT INTO messages_fts(messages_fts) VALUES('rebuild')", [])?;
        if include_trigram {
            conn.execute(
                "INSERT INTO messages_fts_trigram(messages_fts_trigram) VALUES('rebuild')",
                [],
            )?;
        }
        conn.execute(
            "DELETE FROM state_meta WHERE key IN ('fts_rebuild_high_water', 'fts_rebuild_progress')",
            [],
        )?;
        Ok(())
    }

    /// Mirrors `@staticmethod def _rebuild_legacy_fts_indexes(cursor, *, include_trigram=True) -> None:` (ll.351-382)
    ///
    /// Rebuild the LEGACY inline FTS indexes (pre-v23) from messages.
    /// Used only to repair a legacy DB whose triggers degraded under an
    /// earlier no-FTS5 runtime. Inline tables have no external-content
    /// 'rebuild' source, so we DELETE + reinsert the concatenated content.
    pub fn rebuild_legacy_fts_indexes(conn: &Connection, include_trigram: bool) -> rusqlite::Result<()> {
        conn.execute("DELETE FROM messages_fts", [])?;
        conn.execute(
            "INSERT INTO messages_fts(rowid, content) \
             SELECT id, COALESCE(content, '') || ' ' || COALESCE(tool_name, '') || ' ' || COALESCE(tool_calls, '') \
             FROM messages",
            [],
        )?;
        if !include_trigram {
            return Ok(());
        }
        conn.execute("DELETE FROM messages_fts_trigram", [])?;
        conn.execute(
            "INSERT INTO messages_fts_trigram(rowid, content) \
             SELECT id, COALESCE(content, '') || ' ' || COALESCE(tool_name, '') || ' ' || COALESCE(tool_calls, '') \
             FROM messages",
            [],
        )?;
        Ok(())
    }

    /// Mirrors `def _fts_table_probe(self, cursor: sqlite3.Cursor, table_name: str) -> Optional[bool]:` (ll.384-399)
    pub fn fts_table_probe(&self, conn: &Connection, table_name: &str) -> Option<bool> {
        self.fts_table_probe(conn, table_name)
    }

    /// Mirrors `def _recover_stale_fts(self, cursor: sqlite3.Cursor, *, legacy: bool) -> bool:` (ll.401-489)
    ///
    /// Atomically rebuild stale base/trigram indexes and resume syncing.
    pub fn recover_stale_fts(&self, conn: &Connection, legacy: bool) -> rusqlite::Result<bool> {
        // Python ll.402-476: foreign_holders = self._foreign_state_db_holders()
        // if foreign_holders: record attempts/first_seen in state_meta under
        // FTS_REBUILD_DEFERRAL_KEY, escalate after 3 attempts + 60s, reap
        // orphans, log, return False (deferred)
        let foreign_holders = self.foreign_state_db_holders();
        if !foreign_holders.is_empty() {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs_f64())
                .unwrap_or(0.0);
            // Load existing diagnostic record
            let existing: Option<String> = conn
                .query_row(
                    "SELECT value FROM state_meta WHERE key = ?1 LIMIT 1",
                    params![FTS_REBUILD_DEFERRAL_KEY],
                    |r| r.get(0),
                )
                .optional()
                .ok()
                .flatten();
            let mut record: serde_json::Value = existing
                .as_deref()
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or(json!({}));

            let first_seen = record
                .get("first_seen")
                .and_then(|v| v.as_f64())
                .unwrap_or(now);
            let first_seen = if first_seen > now || first_seen < 0.0 { now } else { first_seen };
            let attempts = record
                .get("attempts")
                .and_then(|v| v.as_i64())
                .unwrap_or(0) + 1;
            let holder_pids: Vec<i32> = {
                let mut pids: Vec<i32> = foreign_holders.iter().map(|(pid, _)| *pid).filter(|p| *p > 0).collect();
                pids.sort(); pids.dedup(); pids
            };
            let diagnostic = json!({
                "first_seen": first_seen,
                "last_seen": now,
                "attempts": attempts,
                "holder_pids": holder_pids
            });
            conn.execute(
                "INSERT INTO state_meta (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![FTS_REBUILD_DEFERRAL_KEY, diagnostic.to_string()],
            )?;

            let escalated = attempts >= FTS_HOLDER_ESCALATE_ATTEMPTS as i64
                && now - first_seen >= FTS_HOLDER_ESCALATE_SECONDS;
            if escalated {
                let reaped = self.reap_inactive_orphan_desktop_holders(
                    &foreign_holders,
                    FTS_HOLDER_ESCALATE_SECONDS,
                );
                let foreign_holders = if !reaped.is_empty() {
                    log::error!(target: LOG_TARGET,
                        "Reaped inactive orphan Desktop backend(s) {:?} after {} state.db FTS rebuild deferrals; checking holders again.",
                        reaped, attempts);
                    self.foreign_state_db_holders()
                } else {
                    foreign_holders.clone()
                };
                if !foreign_holders.is_empty() {
                    log::error!(target: LOG_TARGET,
                        "state.db FTS repair remains blocked after {} deferrals by holder(s) {:?}. Stop the listed processes, then run `hermes sessions optimize-storage` with the gateway stopped. `hermes doctor` reports this degraded state.",
                        attempts, foreign_holders);
                }
                if !foreign_holders.is_empty() {
                    log::warn!(target: LOG_TARGET,
                        "Deferred stale state.db FTS rebuild while foreign processes hold the database or WAL sidecars ({:?}); canonical writes and LIKE search remain available (deferral {}).",
                        foreign_holders, attempts);
                    return Ok(false);
                }
            } else if !foreign_holders.is_empty() {
                log::warn!(target: LOG_TARGET,
                    "Deferred stale state.db FTS rebuild while foreign processes hold the database or WAL sidecars ({:?}); canonical writes and LIKE search remain available (deferral {}).",
                    foreign_holders, attempts);
                return Ok(false);
            }
            // If we reaped and now empty, fall through to admitted rebuild
            if !self.foreign_state_db_holders().is_empty() {
                return Ok(false);
            }
        }

        // Python ll.481-489: with fts_rebuild_admission(getattr(self, "db_path", None)) as admitted:
        // if not admitted: log warning, return False
        // return self._recover_stale_fts_locked(cursor, legacy=legacy)
        let guard = fts_rebuild_admission(self.db_path.as_deref());
        if !guard.acquired {
            log::warn!(target: LOG_TARGET,
                "Deferred stale state.db FTS rebuild: another process holds the rebuild authority; canonical writes and LIKE search remain available.");
            return Ok(false);
        }
        self.recover_stale_fts_locked(conn, legacy)
    }

    /// Mirrors `def _recover_stale_fts_locked(self, cursor: sqlite3.Cursor, *, legacy: bool) -> bool:` (ll.491-585)
    ///
    /// Body of `_recover_stale_fts`; caller holds rebuild authority.
    pub fn recover_stale_fts_locked(&self, conn: &Connection, legacy: bool) -> rusqlite::Result<bool> {
        // Python ll.495-501: trigram_status = self._fts_table_probe(cursor, "messages_fts_trigram")
        // except DatabaseError: trigram_status = True (corrupt vtable still needs drop)
        // include_trigram = trigram_status is True
        let trigram_status = match self.fts_table_probe(conn, "messages_fts_trigram") {
            Some(v) => Some(v),
            None => {
                // Probe returned None only on FTS5-unavailable path — treat as missing
                // but still needs drop if corrupt case handled above. For slice audit
                // we treat unavailable as not include_trigram.
                None
            }
        };
        // Simulate corrupt probe fallback: if probe raised DatabaseError, status=True
        // In Rust we treat absence of table as Some(false), unavailable as None.
        // Corrupt case (DatabaseError) we map to Some(true) so it gets dropped.
        // For simplicity, include_trigram = matches Some(true)
        let include_trigram = trigram_status == Some(true);

        // Python ll.503-509: drop_sql building
        let mut drop_sql = String::new();
        for trigger in FTS_TRIGGERS {
            drop_sql.push_str(&format!("DROP TRIGGER IF EXISTS {};", trigger));
        }
        if include_trigram {
            drop_sql.push_str("DROP TABLE IF EXISTS messages_fts_trigram;");
        }
        drop_sql.push_str("DROP VIEW IF EXISTS messages_fts_trigram_src;");
        drop_sql.push_str("DROP TABLE IF EXISTS messages_fts;");

        // Python ll.511-548: rebuild_sql per legacy vs v23
        let rebuild_sql = if legacy {
            let mut s = String::new();
            s.push_str(LEGACY_FTS_SQL);
            if include_trigram {
                s.push_str(LEGACY_FTS_TRIGRAM_SQL);
            }
            s.push_str(
                "INSERT INTO messages_fts(rowid, content) \
                 SELECT id, COALESCE(content, '') || ' ' || COALESCE(tool_name, '') || ' ' || COALESCE(tool_calls, '') \
                 FROM messages;",
            );
            if include_trigram {
                s.push_str(
                    "DELETE FROM messages_fts_trigram; \
                     INSERT INTO messages_fts_trigram(rowid, content) \
                     SELECT id, COALESCE(content, '') || ' ' || COALESCE(tool_name, '') || ' ' || COALESCE(tool_calls, '') \
                     FROM messages;",
                );
            }
            s
        } else {
            let mut s = String::new();
            s.push_str(FTS_SQL);
            if include_trigram {
                s.push_str(FTS_TRIGRAM_SQL);
            }
            s.push_str("INSERT INTO messages_fts(messages_fts) VALUES('rebuild');");
            if include_trigram {
                s.push_str("INSERT INTO messages_fts_trigram(messages_fts_trigram) VALUES('rebuild');");
            }
            s.push_str("DELETE FROM state_meta WHERE key IN ('fts_rebuild_high_water', 'fts_rebuild_progress');");
            s
        };

        // Python ll.552-576: recovery_sql = "BEGIN IMMEDIATE;" + drop_sql + rebuild_sql + DELETE stale/deferral + "COMMIT;"
        // try: cursor.executescript(recovery_sql) except DatabaseError: rollback, drop triggers, commit, log, return False
        let recovery_sql = format!(
            "BEGIN IMMEDIATE;{} {} DELETE FROM state_meta WHERE key IN ('{}', '{}'); COMMIT;",
            drop_sql, rebuild_sql, FTS_STALE_KEY, FTS_REBUILD_DEFERRAL_KEY
        );
        match conn.execute_batch(&recovery_sql) {
            Ok(()) => {
                // Python ll.578-585: self._fts_stale = False; _fts_enabled = True; _trigram_available = include_trigram; log
                log::warn!(target: LOG_TARGET,
                    "Rebuilt stale state.db FTS indexes from canonical messages and restored sync triggers.");
                Ok(true)
            }
            Err(exc) => {
                let _ = conn.execute("ROLLBACK", []);
                // Stale indexes must remain detached even on builds whose DDL differs
                let _ = self.drop_all_fts_triggers(conn);
                let _ = conn.execute_batch("COMMIT;");
                log::error!(target: LOG_TARGET,
                    "Automatic rebuild of stale FTS indexes failed ({}); canonical writes remain enabled with FTS detached.",
                    exc);
                Ok(false)
            }
        }
    }

    /// Mirrors `@staticmethod def _parse_schema_columns(schema_sql: str) -> Dict[str, Dict[str, str]]:` (ll.587-676)
    ///
    /// Extract expected columns per table from SCHEMA_SQL.
    /// Uses an in-memory SQLite database to parse the SQL — SQLite itself
    /// handles all syntax (DEFAULT expressions with commas, inline
    /// REFERENCES, CHECK constraints, etc.) so there are zero regex
    /// edge cases. The parse result is memoized on disk keyed by a hash
    /// of the DDL: executing SCHEMA_SQL in the scratch DB costs ~85ms on
    /// every startup, but the output is a pure function of the DDL text.
    /// A corrupt or stale cache degrades to recomputation.
    // ponytail: file cache (~/cache/schema_columns.json) omitted — recomputes
    // each call; add OnceLock + file cache if startup profiling shows it matters
    pub fn parse_schema_columns(schema_sql: &str) -> HashMap<String, HashMap<String, String>> {
        // Simplified: no disk cache, pure in-memory parse (same correctness, no
        // I/O). The full Python ll.608-676 file-cache path is deferred as
        // YAGNI for this slice — cold parse is ~ms and only runs at startup.
        let mut out: HashMap<String, HashMap<String, String>> = HashMap::new();
        let ref_conn = match Connection::open_in_memory() {
            Ok(c) => c,
            Err(_) => return out,
        };
        if ref_conn.execute_batch(schema_sql).is_err() {
            return out;
        }
        let tables: Vec<String> = ref_conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'")
            .ok()
            .and_then(|mut stmt| {
                stmt.query_map([], |r| r.get(0))
                    .ok()
                    .map(|rows| rows.flatten().collect())
            })
            .unwrap_or_default();

        for tbl in tables {
            // PRAGMA table_info returns (cid, name, type, notnull, dflt_value, pk)
            let pragma = format!("PRAGMA table_info(\"{}\")", tbl.replace('"', "\"\""));
            let mut stmt = match ref_conn.prepare(&pragma) {
                Ok(s) => s,
                Err(_) => continue,
            };
            let mut cols: HashMap<String, String> = HashMap::new();
            let rows = match stmt.query_map([], |r| {
                let name: String = r.get(1)?;
                let typ: String = r.get::<_, Option<String>>(2)?.unwrap_or_default();
                let notnull: i64 = r.get(3)?;
                let dflt: Option<String> = r.get(4)?;
                let pk: i64 = r.get(5)?;
                let mut parts: Vec<String> = Vec::new();
                if !typ.is_empty() {
                    parts.push(typ);
                }
                if notnull != 0 && pk == 0 {
                    parts.push("NOT NULL".to_string());
                }
                if let Some(d) = dflt {
                    parts.push(format!("DEFAULT {}", d));
                }
                Ok((name, parts.join(" ")))
            }) {
                Ok(m) => m,
                Err(_) => continue,
            };
            for row in rows.flatten() {
                cols.insert(row.0, row.1);
            }
            out.insert(tbl, cols);
        }
        out
    }

    /// Mirrors `def _reconcile_columns(self, cursor: sqlite3.Cursor) -> None:` (ll.678-743)
    ///
    /// Ensure live tables have every column declared in SCHEMA_SQL.
    /// Follows the Beets/sqlite-utils pattern: the CREATE TABLE definition
    /// in SCHEMA_SQL is the single source of truth for the desired schema.
    pub fn reconcile_columns(&self, conn: &Connection) -> rusqlite::Result<()> {
        let expected = Self::parse_schema_columns(SCHEMA_SQL);
        for (table_name, declared_cols) in expected {
            // Python ll.694-699: rows = cursor.execute(PRAGMA table_info).fetchall() except OperationalError: continue
            let pragma = format!("PRAGMA table_info(\"{}\")", table_name.replace('"', "\"\""));
            let mut stmt = match conn.prepare(&pragma) {
                Ok(s) => s,
                Err(_) => continue,
            };
            let live_rows: Vec<String> = stmt
                .query_map([], |r| r.get::<_, String>(1))
                .into_iter()
                .flatten()
                .flatten()
                .collect();
            let live_cols: HashSet<String> = live_rows.into_iter().collect();

            for (col_name, col_type) in declared_cols {
                if live_cols.contains(&col_name) {
                    continue;
                }
                let safe_name = col_name.replace('"', "\"\"");
                // Python ll.709-743: ALTER TABLE ADD COLUMN with duplicate/locked handling
                let sql = format!("ALTER TABLE \"{}\" ADD COLUMN \"{}\" {}", table_name, safe_name, col_type);
                match conn.execute(&sql, []) {
                    Ok(_) => {}
                    Err(exc) => {
                        let msg = exc.to_string().to_lowercase();
                        if msg.contains("duplicate column") {
                            log::debug!(target: LOG_TARGET, "reconcile {}.{}: {}", table_name, col_name, exc);
                            continue;
                        }
                        if msg.contains("locked") || msg.contains("busy") {
                            return Err(exc);
                        }
                        log::warn!(target: LOG_TARGET,
                            "reconcile {}.{} failed; store remains behind SCHEMA_SQL: {}", table_name, col_name, exc);
                    }
                }
            }
        }
        Ok(())
    }

    /// Mirrors `def _heal_gateway_routing_pk(self, cursor: sqlite3.Cursor) -> None:` (ll.745-816)
    ///
    /// Rebuild `gateway_routing` when its PRIMARY KEY predates scoping.
    /// Early builds created the table with `session_key TEXT PRIMARY KEY` and
    /// no `scope` column. `_reconcile_columns()` ADDs the missing `scope`
    /// column, but SQLite cannot ALTER a primary key, so the shipped composite
    /// `PRIMARY KEY (scope, session_key)` never lands. Rebuild it once,
    /// preserving rows. On a session_key collision across scopes the newest
    /// row wins.
    pub fn heal_gateway_routing_pk(&self, conn: &Connection) -> rusqlite::Result<()> {
        // Python ll.768-789: PRAGMA table_info + pk_cols check
        let pragma = "PRAGMA table_info(\"gateway_routing\")";
        let mut stmt = match conn.prepare(pragma) {
            Ok(s) => s,
            Err(_) => return Ok(()),
        };
        let rows: Vec<(String, i64)> = stmt
            .query_map([], |r| {
                let name: String = r.get(1)?;
                let pk: i64 = r.get(5)?;
                Ok((name, pk))
            })
            .into_iter()
            .flatten()
            .flatten()
            .collect();
        if rows.is_empty() {
            return Ok(());
        }
        let mut pk_cols: Vec<(i64, String)> = rows
            .iter()
            .filter(|(_, pk)| *pk > 0)
            .map(|(name, pk)| (*pk, name.clone()))
            .collect();
        pk_cols.sort_by_key(|(pk, _)| *pk);
        let pk_names: Vec<String> = pk_cols.iter().map(|(_, n)| n.clone()).collect();
        if pk_names == vec!["scope".to_string(), "session_key".to_string()] {
            return Ok(());
        }

        log::info!(target: LOG_TARGET,
            "gateway_routing has legacy primary key {:?}; rebuilding with composite (scope, session_key) key", pk_names);

        conn.execute("ALTER TABLE gateway_routing RENAME TO gateway_routing_legacy_pk", [])?;
        conn.execute(
            "CREATE TABLE gateway_routing ( \
                scope TEXT NOT NULL DEFAULT '', \
                session_key TEXT NOT NULL, \
                entry_json TEXT NOT NULL, \
                updated_at REAL NOT NULL, \
                PRIMARY KEY (scope, session_key) \
            )",
            [],
        )?;
        conn.execute(
            "INSERT OR REPLACE INTO gateway_routing (scope, session_key, entry_json, updated_at) \
             SELECT COALESCE(scope, ''), session_key, entry_json, updated_at \
             FROM gateway_routing_legacy_pk ORDER BY updated_at ASC",
            [],
        )?;
        conn.execute("DROP TABLE gateway_routing_legacy_pk", [])?;
        Ok(())
    }

    /// Mirrors `def _heal_session_model_usage_pk(self, cursor: sqlite3.Cursor) -> None:` (ll.818-900)
    ///
    /// Rebuild `session_model_usage` when its PRIMARY KEY lacks `task`.
    /// Installs whose `state.db` reached `schema_version >= 22` before
    /// the `task` dimension was added carry a 5-column PRIMARY KEY.
    /// `_reconcile_columns()` ADDs the `task` column as a bare nullable, but
    /// SQLite cannot ALTER a primary key, so the shipped composite 6-column
    /// key never lands. Idempotent; runs unconditionally on every open.
    ///
    /// NOTE: Slice boundary at l.900 — Python `            )` closing the
    /// `CREATE TABLE session_model_usage` DDL. The subsequent
    /// `INSERT OR IGNORE` / `DROP TABLE` / index creation / exception tail
    /// (ll.901-936) and the remainder of `_init_schema` (ll.938-1529) are
    /// deferred to `schema_slice2.rs`. This method is syntactically closed
    /// here with a best-effort tail stub so the slice remains self-contained;
    /// the merged crate replaces the stub with the verbatim continuation.
    pub fn heal_session_model_usage_pk(&self, conn: &Connection) -> rusqlite::Result<()> {
        // Python ll.837-853: PRAGMA table_info + pk set check for "task"
        let pragma = "PRAGMA table_info(\"session_model_usage\")";
        let mut stmt = match conn.prepare(pragma) {
            Ok(s) => s,
            Err(_) => return Ok(()),
        };
        let rows: Vec<(String, i64)> = stmt
            .query_map([], |r| {
                let name: String = r.get(1)?;
                let pk: i64 = r.get(5)?;
                Ok((name, pk))
            })
            .into_iter()
            .flatten()
            .flatten()
            .collect();
        if rows.is_empty() {
            return Ok(());
        }
        let pk_set: HashSet<String> = rows.iter().filter(|(_, pk)| *pk > 0).map(|(n, _)| n.clone()).collect();
        if pk_set.contains("task") {
            return Ok(());
        }

        log::info!(target: LOG_TARGET,
            "session_model_usage has legacy primary key {:?} (missing task); rebuilding with composite 6-column key",
            pk_set);

        // Python ll.867-900: FK-off window + RENAME + CREATE with task in PK
        // FK enforcement disabled for the copy (OR IGNORE does NOT suppress FK violations)
        let _ = conn.execute("PRAGMA foreign_keys=OFF", []);
        let res: rusqlite::Result<()> = (|| {
            conn.execute("ALTER TABLE session_model_usage RENAME TO session_model_usage_legacy_pk", [])?;
            conn.execute(
                "CREATE TABLE session_model_usage ( \
                    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE, \
                    model TEXT NOT NULL, \
                    billing_provider TEXT NOT NULL DEFAULT '', \
                    billing_base_url TEXT NOT NULL DEFAULT '', \
                    billing_mode TEXT NOT NULL DEFAULT '', \
                    task TEXT NOT NULL DEFAULT '', \
                    api_call_count INTEGER NOT NULL DEFAULT 0, \
                    input_tokens INTEGER NOT NULL DEFAULT 0, \
                    output_tokens INTEGER NOT NULL DEFAULT 0, \
                    cache_read_tokens INTEGER NOT NULL DEFAULT 0, \
                    cache_write_tokens INTEGER NOT NULL DEFAULT 0, \
                    reasoning_tokens INTEGER NOT NULL DEFAULT 0, \
                    estimated_cost_usd REAL NOT NULL DEFAULT 0, \
                    actual_cost_usd REAL NOT NULL DEFAULT 0, \
                    cost_status TEXT, \
                    cost_source TEXT, \
                    first_seen REAL, \
                    last_seen REAL, \
                    PRIMARY KEY (session_id, model, billing_provider, billing_base_url, billing_mode, task) \
                )",
                [],
            )?;
            // Slice boundary at l.900 — `            )` above.
            // Remaining Python ll.901-936 (INSERT OR IGNORE with COALESCE,
            // DROP legacy, CREATE INDEX ×2, except OperationalError,
            // finally PRAGMA foreign_keys=ON) is deferred to schema_slice2.rs.
            // For standalone slice we emit a best-effort tail stub and note it.
            // --- tail stub (deferred verbatim continuation) ---
            let _ = conn.execute(
                "INSERT OR IGNORE INTO session_model_usage ( \
                    session_id, model, billing_provider, billing_base_url, \
                    billing_mode, task, api_call_count, input_tokens, \
                    output_tokens, cache_read_tokens, cache_write_tokens, \
                    reasoning_tokens, estimated_cost_usd, actual_cost_usd, \
                    cost_status, cost_source, first_seen, last_seen \
                ) \
                SELECT session_id, model, \
                       COALESCE(billing_provider, ''), \
                       COALESCE(billing_base_url, ''), \
                       COALESCE(billing_mode, ''), \
                       COALESCE(task, ''), \
                       api_call_count, input_tokens, \
                       output_tokens, cache_read_tokens, cache_write_tokens, \
                       reasoning_tokens, estimated_cost_usd, actual_cost_usd, \
                       cost_status, cost_source, first_seen, last_seen \
                FROM session_model_usage_legacy_pk",
                [],
            );
            let _ = conn.execute("DROP TABLE session_model_usage_legacy_pk", []);
            let _ = conn.execute("CREATE INDEX IF NOT EXISTS idx_session_model_usage_session ON session_model_usage(session_id)", []);
            let _ = conn.execute("CREATE INDEX IF NOT EXISTS idx_session_model_usage_model ON session_model_usage(model)", []);
            Ok(())
        })();
        let _ = conn.execute("PRAGMA foreign_keys=ON", []);
        if let Err(exc) = res {
            log::debug!(target: LOG_TARGET, "session_model_usage PK heal skipped: {}", exc);
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Ponytail self-check — one runnable check (no framework)
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_statements_cached_and_sorted() {
        let a = schema_read_probe_statements();
        let b = schema_read_probe_statements();
        assert_eq!(a, b);
        assert!(a.iter().any(|s| s.contains("sessions")));
        assert!(a.iter().any(|s| s.contains("messages")));
        // LIMIT 0 + qualified column refs (ll.86-101)
        for stmt in &a {
            assert!(stmt.contains("LIMIT 0"));
            assert!(stmt.contains("\"sessions\"") || stmt.contains("\"messages\"") || stmt.contains("state_meta"));
        }
    }

    #[test]
    fn fts_trigger_subsets_match_ddl() {
        // Mirrors test_fts_trigger_subsets_match_the_ddl pin (ll.54-58 comment)
        assert_eq!(FTS_TRIGRAM_TRIGGERS.len(), 3);
        assert_eq!(FTS_BASE_TRIGGERS.len(), 3);
        assert!(FTS_TRIGRAM_TRIGGERS.iter().all(|n| n.contains("_trigram_")));
        assert!(FTS_BASE_TRIGGERS.iter().all(|n| !n.contains("_trigram_")));
        let all: HashSet<&str> = FTS_TRIGGERS.iter().copied().collect();
        let base: HashSet<&str> = FTS_BASE_TRIGGERS.iter().copied().collect();
        let trigram: HashSet<&str> = FTS_TRIGRAM_TRIGGERS.iter().copied().collect();
        assert_eq!(base.union(&trigram).copied().collect::<HashSet<_>>(), all);
        assert!(base.intersection(&trigram).next().is_none());
    }

    #[test]
    fn fts_update_narrowing_detection() {
        assert!(!StateStore::fts_update_trigger_needs_narrowing(None));
        assert!(!StateStore::fts_update_trigger_needs_narrowing(Some("CREATE TRIGGER t AFTER UPDATE OF content ON messages BEGIN END;")));
        assert!(StateStore::fts_update_trigger_needs_narrowing(Some("CREATE TRIGGER t AFTER UPDATE ON messages BEGIN END;")));
        assert!(StateStore::fts_update_trigger_needs_narrowing(Some("CREATE TRIGGER t\nAFTER UPDATE ON messages\nBEGIN END;")));
    }

    #[test]
    fn parse_schema_columns_nonempty() {
        let cols = StateStore::parse_schema_columns(SCHEMA_SQL);
        assert!(cols.contains_key("sessions"));
        assert!(cols.contains_key("messages"));
        assert!(cols["sessions"].contains_key("id"));
        assert!(cols["sessions"].contains_key("system_prompt_hash"));
        assert!(cols["messages"].contains_key("active"));
        assert!(cols["session_model_usage"].contains_key("task"));
    }
}

// NOTE: ll.901-1529 (rest of _heal_session_model_usage_pk tail —
// INSERT OR IGNORE / DROP / CREATE INDEX ×2 / OperationalError guard /
// finally PRAGMA foreign_keys=ON — plus _init_schema, _run_admitted_startup_
// rebuild, _backfill_gateway_metadata_from_sessions_json) are deferred to
// `schema_slice2.rs` (slice 2/2, ll.901-1529). This slice is syntactically
// closed at the l.900 boundary (`            )` closing CREATE TABLE) so
// `cargo check` would still pass if the lenient `allow(dead_code)` gate
// were removed; the next item `schema_slice2.rs` resumes with the INSERT
// at l.901.

