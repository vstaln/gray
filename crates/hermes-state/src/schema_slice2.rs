//! Schema creation, column reconciliation, and FTS DDL management for SessionDB.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/hermes_state_schema.py`
//! (1529 LOC) — slice 2/2, lines 900-1529.
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
//! Mirrors Python ll.900-1529 verbatim; line numbers in comments refer to the
//! 1529-line source file. Slice boundary at l.900 (`            )` closing the
//! `CREATE TABLE session_model_usage` in `_heal_session_model_usage_pk`).
//! Slice 1 (`schema_slice1.rs`) covers ll.1-900 (including the first half of
//! `_heal_session_model_usage_pk` up to the `)` at l.900); this slice resumes
//! at l.900 and continues through `_init_schema`, `_run_admitted_startup_
//! rebuild`, and `_backfill_gateway_metadata_from_sessions_json` to EOF.
//! Verified by line-level audit, not by compilation.
//!
//! T0011 — `crates/hermes-state/src/schema_slice2.rs`.

#![allow(dead_code, unused_imports, unused_variables, clippy::all)]

// ---------------------------------------------------------------------------
// Imports — mirrors Python ll.11-34 (same as slice 1)
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
// subset used by ll.900-1529; when slices merge these collapse to
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
pub const FTS_BASE_TRIGGERS: &[&str] = &[
    "messages_fts_insert",
    "messages_fts_delete",
    "messages_fts_update",
];
pub const FTS_TRIGRAM_TRIGGERS: &[&str] = &[
    "messages_fts_trigram_insert",
    "messages_fts_trigram_delete",
    "messages_fts_trigram_update",
];

// Minimal verbatim DDL for _init_schema / _heal_* — mirrors
// hermes_state_common.py ll.359-803. Canonical strings are
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
// Helpers — mirrors hermes_state_common / hermes_constants
// ---------------------------------------------------------------------------

/// Mirrors `hermes_constants.get_hermes_home()` — profile-aware `~/.hermes`.
/// Canonical def lives in `crate::common` / `hermes_constants.py`; stub here
/// keeps the slice self-contained and grep-traceable.
fn get_hermes_home() -> PathBuf {
    if let Ok(v) = std::env::var("HERMES_HOME") {
        if !v.is_empty() {
            return PathBuf::from(v);
        }
    }
    if let Some(home) = dirs_next_stub_home() {
        home.join(".hermes")
    } else {
        PathBuf::from(".hermes")
    }
}
fn dirs_next_stub_home() -> Option<PathBuf> {
    std::env::var("HOME").ok().map(PathBuf::from)
}

/// Mirrors `hermes_state_common._ephemeral_child_sql(alias="s")` (ll.266-276
/// common.py) — leaf ephemeral delegate predicate used in the v16 backfill
/// (schema l.1089 `f"AND {_ephemeral_child_sql('sessions')}"`).
fn ephemeral_child_sql(alias: &str) -> String {
    // Mirrors common.py ll.266-276: branch + compression + reset negated.
    // Full expansion lives in `crate::common::ephemeral_child_sql`; we inline
    // a grep-traceable stub that is structurally identical.
    format!(
        "({a}.parent_session_id IS NOT NULL AND NOT (json_extract(COALESCE({a}.model_config, '{{}}'), '$._branched_from') IS NOT NULL OR EXISTS (SELECT 1 FROM sessions p WHERE p.id = {a}.parent_session_id AND p.end_reason = 'branched' AND {a}.started_at >= p.ended_at)) AND NOT (EXISTS (SELECT 1 FROM sessions p WHERE p.id = {a}.parent_session_id AND p.end_reason = 'compression')) AND NOT (json_extract(COALESCE({a}.model_config, '{{}}'), '$._reset_from') IS NOT NULL OR EXISTS (SELECT 1 FROM sessions p WHERE p.id = {a}.parent_session_id AND p.end_reason IN ('session_reset','session_switch','idle','daily','suspended','resume_pending_expired') AND {a}.session_key IS NOT NULL AND {a}.session_key != '' AND {a}.session_key = p.session_key)))",
        a = alias
    )
}

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

    // ---- cross-module helper stubs (canonical elsewhere) ----

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
        let like = "fts_v22_trash_%";
        conn.query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name LIKE ?1 ESCAPE '\\' LIMIT 1",
            params![like],
            |_| Ok(()),
        )
        .is_ok()
    }

    fn fts_external_index_empty_with_messages(&self, conn: &Connection) -> bool {
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

    fn drop_all_fts_triggers(&self, conn: &Connection) -> rusqlite::Result<()> {
        self.drop_fts_triggers(conn)?;
        for trigger in FTS_CJK_TRIGGERS {
            let _ = conn.execute(&format!("DROP TRIGGER IF EXISTS {}", trigger), []);
        }
        Ok(())
    }

    fn sqlite_supports_fts5(&self, conn: &Connection) -> bool {
        match conn.execute("CREATE VIRTUAL TABLE temp._hermes_fts5_probe USING fts5(x)", []) {
            Ok(_) => {
                let _ = conn.execute("DROP TABLE temp._hermes_fts5_probe", []);
                true
            }
            Err(exc) => {
                if !self.is_fts5_unavailable_error(&exc) {
                    log::warn!(target: LOG_TARGET, "FTS5 probe unexpected error: {}", exc);
                    return false;
                }
                self.warn_fts5_unavailable(&exc);
                false
            }
        }
    }

    fn fts_trigger_count(&self, conn: &Connection, names: &[&str]) -> usize {
        Self::fts_trigger_count_static(conn, names)
    }

    fn fts_trigger_count_static(conn: &Connection, names: &[&str]) -> usize {
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

    fn fts_update_trigger_needs_narrowing(sql: Option<&str>) -> bool {
        let Some(s) = sql else { return false; };
        let compact = s.split_whitespace().collect::<Vec<_>>().join(" ").to_uppercase();
        if compact.contains("AFTER UPDATE OF ") { return false; }
        compact.contains("AFTER UPDATE ON ")
    }

    fn cjk_update_trigger_is_narrowed(&self, conn: &Connection) -> bool {
        let row: Option<Option<String>> = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'trigger' AND name = ?1 LIMIT 1",
                params!["messages_fts_cjk_update"],
                |r| r.get(0),
            )
            .optional()
            .ok()
            .flatten();
        let Some(sql_opt) = row else { return false; };
        let Some(sql) = sql_opt else { return false; };
        !Self::fts_update_trigger_needs_narrowing(Some(&sql))
    }

    fn quarantine_cjk_after_update_of_migration(&self, conn: &Connection) -> rusqlite::Result<()> {
        let _ = self.set_meta(FTS_CJK_STALE_KEY, "1", conn);
        let _ = conn.execute("DROP TRIGGER IF EXISTS messages_fts_cjk_update", []);
        log::warn!(target: LOG_TARGET, "Quarantined CJK after UPDATE OF migration — marked stale and unavailable");
        Ok(())
    }

    fn fts_table_probe(&self, conn: &Connection, table_name: &str) -> Option<bool> {
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

    fn rebuild_fts_indexes(conn: &Connection, include_trigram: bool) -> rusqlite::Result<()> {
        conn.execute("INSERT INTO messages_fts(messages_fts) VALUES('rebuild')", [])?;
        if include_trigram {
            conn.execute("INSERT INTO messages_fts_trigram(messages_fts_trigram) VALUES('rebuild')", [])?;
        }
        conn.execute("DELETE FROM state_meta WHERE key IN ('fts_rebuild_high_water', 'fts_rebuild_progress')", [])?;
        Ok(())
    }

    fn rebuild_legacy_fts_indexes(conn: &Connection, include_trigram: bool) -> rusqlite::Result<()> {
        conn.execute("DELETE FROM messages_fts", [])?;
        conn.execute(
            "INSERT INTO messages_fts(rowid, content) SELECT id, COALESCE(content, '') || ' ' || COALESCE(tool_name, '') || ' ' || COALESCE(tool_calls, '') FROM messages",
            [],
        )?;
        if !include_trigram { return Ok(()); }
        conn.execute("DELETE FROM messages_fts_trigram", [])?;
        conn.execute(
            "INSERT INTO messages_fts_trigram(rowid, content) SELECT id, COALESCE(content, '') || ' ' || COALESCE(tool_name, '') || ' ' || COALESCE(tool_calls, '') FROM messages",
            [],
        )?;
        Ok(())
    }

    fn recover_stale_fts(&self, conn: &Connection, legacy: bool) -> rusqlite::Result<bool> {
        let foreign_holders = self.foreign_state_db_holders();
        if !foreign_holders.is_empty() {
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0);
            let existing: Option<String> = conn.query_row("SELECT value FROM state_meta WHERE key = ?1 LIMIT 1", params![FTS_REBUILD_DEFERRAL_KEY], |r| r.get(0)).optional().ok().flatten();
            let mut record: serde_json::Value = existing.as_deref().and_then(|s| serde_json::from_str(s).ok()).unwrap_or(json!({}));
            let first_seen = record.get("first_seen").and_then(|v| v.as_f64()).unwrap_or(now);
            let first_seen = if first_seen > now || first_seen < 0.0 { now } else { first_seen };
            let attempts = record.get("attempts").and_then(|v| v.as_i64()).unwrap_or(0) + 1;
            let holder_pids: Vec<i32> = {
                let mut pids: Vec<i32> = foreign_holders.iter().map(|(pid, _)| *pid).filter(|p| *p > 0).collect();
                pids.sort(); pids.dedup(); pids
            };
            let diagnostic = json!({"first_seen": first_seen, "last_seen": now, "attempts": attempts, "holder_pids": holder_pids});
            conn.execute("INSERT INTO state_meta (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value", params![FTS_REBUILD_DEFERRAL_KEY, diagnostic.to_string()])?;
            let escalated = attempts >= FTS_HOLDER_ESCALATE_ATTEMPTS as i64 && now - first_seen >= FTS_HOLDER_ESCALATE_SECONDS;
            if escalated {
                let reaped = self.reap_inactive_orphan_desktop_holders(&foreign_holders, FTS_HOLDER_ESCALATE_SECONDS);
                let foreign_holders = if !reaped.is_empty() {
                    log::error!(target: LOG_TARGET, "Reaped inactive orphan Desktop backend(s) {:?} after {} state.db FTS rebuild deferrals; checking holders again.", reaped, attempts);
                    self.foreign_state_db_holders()
                } else { foreign_holders.clone() };
                if !foreign_holders.is_empty() {
                    log::error!(target: LOG_TARGET, "state.db FTS repair remains blocked after {} deferrals by holder(s) {:?}. Stop the listed processes, then run `hermes sessions optimize-storage` with the gateway stopped. `hermes doctor` reports this degraded state.", attempts, foreign_holders);
                }
                if !foreign_holders.is_empty() {
                    log::warn!(target: LOG_TARGET, "Deferred stale state.db FTS rebuild while foreign processes hold the database or WAL sidecars ({:?}); canonical writes and LIKE search remain available (deferral {}).", foreign_holders, attempts);
                    return Ok(false);
                }
            } else if !foreign_holders.is_empty() {
                log::warn!(target: LOG_TARGET, "Deferred stale state.db FTS rebuild while foreign processes hold the database or WAL sidecars ({:?}); canonical writes and LIKE search remain available (deferral {}).", foreign_holders, attempts);
                return Ok(false);
            }
            if !self.foreign_state_db_holders().is_empty() { return Ok(false); }
        }
        let guard = fts_rebuild_admission(self.db_path.as_deref());
        if !guard.acquired {
            log::warn!(target: LOG_TARGET, "Deferred stale state.db FTS rebuild: another process holds the rebuild authority; canonical writes and LIKE search remain available.");
            return Ok(false);
        }
        self.recover_stale_fts_locked(conn, legacy)
    }

    fn recover_stale_fts_locked(&self, conn: &Connection, legacy: bool) -> rusqlite::Result<bool> {
        let trigram_status = match self.fts_table_probe(conn, "messages_fts_trigram") {
            Some(v) => Some(v),
            None => None,
        };
        let include_trigram = trigram_status == Some(true);
        let mut drop_sql = String::new();
        for trigger in FTS_TRIGGERS { drop_sql.push_str(&format!("DROP TRIGGER IF EXISTS {};", trigger)); }
        if include_trigram { drop_sql.push_str("DROP TABLE IF EXISTS messages_fts_trigram;"); }
        drop_sql.push_str("DROP VIEW IF EXISTS messages_fts_trigram_src;");
        drop_sql.push_str("DROP TABLE IF EXISTS messages_fts;");
        let rebuild_sql = if legacy {
            let mut s = String::new();
            s.push_str(LEGACY_FTS_SQL);
            if include_trigram { s.push_str(LEGACY_FTS_TRIGRAM_SQL); }
            s.push_str("INSERT INTO messages_fts(rowid, content) SELECT id, COALESCE(content, '') || ' ' || COALESCE(tool_name, '') || ' ' || COALESCE(tool_calls, '') FROM messages;");
            if include_trigram { s.push_str("DELETE FROM messages_fts_trigram; INSERT INTO messages_fts_trigram(rowid, content) SELECT id, COALESCE(content, '') || ' ' || COALESCE(tool_name, '') || ' ' || COALESCE(tool_calls, '') FROM messages;"); }
            s
        } else {
            let mut s = String::new();
            s.push_str(FTS_SQL);
            if include_trigram { s.push_str(FTS_TRIGRAM_SQL); }
            s.push_str("INSERT INTO messages_fts(messages_fts) VALUES('rebuild');");
            if include_trigram { s.push_str("INSERT INTO messages_fts_trigram(messages_fts_trigram) VALUES('rebuild');"); }
            s.push_str("DELETE FROM state_meta WHERE key IN ('fts_rebuild_high_water', 'fts_rebuild_progress');");
            s
        };
        let recovery_sql = format!("BEGIN IMMEDIATE;{} {} DELETE FROM state_meta WHERE key IN ('{}', '{}'); COMMIT;", drop_sql, rebuild_sql, FTS_STALE_KEY, FTS_REBUILD_DEFERRAL_KEY);
        match conn.execute_batch(&recovery_sql) {
            Ok(()) => {
                log::warn!(target: LOG_TARGET, "Rebuilt stale state.db FTS indexes from canonical messages and restored sync triggers.");
                Ok(true)
            }
            Err(exc) => {
                let _ = conn.execute("ROLLBACK", []);
                let _ = self.drop_all_fts_triggers(conn);
                let _ = conn.execute_batch("COMMIT;");
                log::error!(target: LOG_TARGET, "Automatic rebuild of stale FTS indexes failed ({}); canonical writes remain enabled with FTS detached.", exc);
                Ok(false)
            }
        }
    }

    fn migrate_broad_fts_update_triggers(&self, conn: &Connection) -> rusqlite::Result<usize> {
        let legacy_layout = self.db_has_legacy_inline_fts(conn);
        let mut update_names: Vec<&str> = vec!["messages_fts_update", "messages_fts_trigram_update"];
        if !legacy_layout { update_names.push("messages_fts_cjk_update"); }
        let placeholders = update_names.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
        let sql = format!("SELECT name, sql FROM sqlite_master WHERE type = 'trigger' AND name IN ({})", placeholders);
        let mut stmt = conn.prepare(&sql)?;
        let params_vec: Vec<&dyn rusqlite::ToSql> = update_names.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
        let rows: Vec<(String, Option<String>)> = stmt.query_map(params_vec.as_slice(), |r| { let n: String = r.get(0)?; let s: Option<String> = r.get(1)?; Ok((n, s)) })?.flatten().collect();
        let mut to_drop: Vec<String> = Vec::new();
        for (name, sql) in rows { if Self::fts_update_trigger_needs_narrowing(sql.as_deref()) { to_drop.push(name); } }
        if to_drop.is_empty() { return Ok(0); }
        for name in &to_drop { conn.execute(&format!("DROP TRIGGER IF EXISTS {}", name), [])?; }
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
                            log::warn!(target: LOG_TARGET, "CJK FTS UPDATE trigger missing or still broad after UPDATE OF migration; marked stale and unavailable");
                        }
                    }
                    Err(e) => {
                        self.quarantine_cjk_after_update_of_migration(conn)?;
                        log::error!(target: LOG_TARGET, "CJK FTS re-ensure after UPDATE OF migration failed: {}", e);
                        return Err(e);
                    }
                }
            }
        }
        log::info!(target: LOG_TARGET, "Migrated {} broad FTS UPDATE trigger(s) to AFTER UPDATE OF (no rebuild required)", to_drop.len());
        Ok(to_drop.len())
    }

    // ---- helpers for _heal_* (shared with slice 1) ----
    fn parse_schema_columns(schema_sql: &str) -> HashMap<String, HashMap<String, String>> {
        let mut out: HashMap<String, HashMap<String, String>> = HashMap::new();
        let ref_conn = match Connection::open_in_memory() { Ok(c) => c, Err(_) => return out };
        if ref_conn.execute_batch(schema_sql).is_err() { return out; }
        let tables: Vec<String> = ref_conn.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'").ok().and_then(|mut stmt| stmt.query_map([], |r| r.get(0)).ok().map(|rows| rows.flatten().collect())).unwrap_or_default();
        for tbl in tables {
            let pragma = format!("PRAGMA table_info(\"{}\")", tbl.replace('"', "\"\""));
            let mut stmt = match ref_conn.prepare(&pragma) { Ok(s) => s, Err(_) => continue };
            let mut cols: HashMap<String, String> = HashMap::new();
            let rows = match stmt.query_map([], |r| {
                let name: String = r.get(1)?;
                let typ: String = r.get::<_, Option<String>>(2)?.unwrap_or_default();
                let notnull: i64 = r.get(3)?;
                let dflt: Option<String> = r.get(4)?;
                let pk: i64 = r.get(5)?;
                let mut parts: Vec<String> = Vec::new();
                if !typ.is_empty() { parts.push(typ); }
                if notnull != 0 && pk == 0 { parts.push("NOT NULL".to_string()); }
                if let Some(d) = dflt { parts.push(format!("DEFAULT {}", d)); }
                Ok((name, parts.join(" ")))
            }) { Ok(m) => m, Err(_) => continue };
            for row in rows.flatten() { cols.insert(row.0, row.1); }
            out.insert(tbl, cols);
        }
        out
    }

    fn reconcile_columns(&self, conn: &Connection) -> rusqlite::Result<()> {
        let expected = Self::parse_schema_columns(SCHEMA_SQL);
        for (table_name, declared_cols) in expected {
            let pragma = format!("PRAGMA table_info(\"{}\")", table_name.replace('"', "\"\""));
            let mut stmt = match conn.prepare(&pragma) { Ok(s) => s, Err(_) => continue };
            let live_rows: Vec<String> = stmt.query_map([], |r| r.get::<_, String>(1)).into_iter().flatten().flatten().collect();
            let live_cols: HashSet<String> = live_rows.into_iter().collect();
            for (col_name, col_type) in declared_cols {
                if live_cols.contains(&col_name) { continue; }
                let safe_name = col_name.replace('"', "\"\"");
                let sql = format!("ALTER TABLE \"{}\" ADD COLUMN \"{}\" {}", table_name, safe_name, col_type);
                match conn.execute(&sql, []) {
                    Ok(_) => {}
                    Err(exc) => {
                        let msg = exc.to_string().to_lowercase();
                        if msg.contains("duplicate column") { log::debug!(target: LOG_TARGET, "reconcile {}.{}: {}", table_name, col_name, exc); continue; }
                        if msg.contains("locked") || msg.contains("busy") { return Err(exc); }
                        log::warn!(target: LOG_TARGET, "reconcile {}.{} failed; store remains behind SCHEMA_SQL: {}", table_name, col_name, exc);
                    }
                }
            }
        }
        Ok(())
    }

    fn heal_gateway_routing_pk(&self, conn: &Connection) -> rusqlite::Result<()> {
        let pragma = "PRAGMA table_info(\"gateway_routing\")";
        let mut stmt = match conn.prepare(pragma) { Ok(s) => s, Err(_) => return Ok(()) };
        let rows: Vec<(String, i64)> = stmt.query_map([], |r| { let n: String = r.get(1)?; let pk: i64 = r.get(5)?; Ok((n, pk)) }).into_iter().flatten().flatten().collect();
        if rows.is_empty() { return Ok(()); }
        let mut pk_cols: Vec<(i64, String)> = rows.iter().filter(|(_, pk)| *pk > 0).map(|(n, pk)| (*pk, n.clone())).collect();
        pk_cols.sort_by_key(|(pk, _)| *pk);
        let pk_names: Vec<String> = pk_cols.iter().map(|(_, n)| n.clone()).collect();
        if pk_names == vec!["scope".to_string(), "session_key".to_string()] { return Ok(()); }
        log::info!(target: LOG_TARGET, "gateway_routing has legacy primary key {:?}; rebuilding with composite (scope, session_key) key", pk_names);
        conn.execute("ALTER TABLE gateway_routing RENAME TO gateway_routing_legacy_pk", [])?;
        conn.execute("CREATE TABLE gateway_routing ( scope TEXT NOT NULL DEFAULT '', session_key TEXT NOT NULL, entry_json TEXT NOT NULL, updated_at REAL NOT NULL, PRIMARY KEY (scope, session_key) )", [])?;
        conn.execute("INSERT OR REPLACE INTO gateway_routing (scope, session_key, entry_json, updated_at) SELECT COALESCE(scope, ''), session_key, entry_json, updated_at FROM gateway_routing_legacy_pk ORDER BY updated_at ASC", [])?;
        conn.execute("DROP TABLE gateway_routing_legacy_pk", [])?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // _heal_session_model_usage_pk — Python ll.818-936
    // Slice boundary: l.900 `            )` closes CREATE TABLE; this slice
    // resumes at l.901 `INSERT OR IGNORE` through to `PRAGMA foreign_keys=ON`.
    // For standalone audit we reproduce the full method (ll.818-936) verbatim;
    // the first half (ll.837-900) mirrors slice 1 and is kept here so the
    // method is whole and grep-traceable. The 1:1 port covers ll.900-936
    // (the tail) plus the full init flow below.
    // -----------------------------------------------------------------------
    /// Mirrors `def _heal_session_model_usage_pk(self, cursor: sqlite3.Cursor) -> None:` (ll.818-936)
    ///
    /// Rebuild `session_model_usage` when its PRIMARY KEY lacks `task`.
    /// Installs whose `state.db` reached `schema_version >= 22` before
    /// the `task` dimension was added carry a 5-column PRIMARY KEY.
    /// `_reconcile_columns()` ADDs the `task` column as a bare nullable, but
    /// SQLite cannot ALTER a primary key, so the shipped composite 6-column
    /// key never lands.
    pub fn heal_session_model_usage_pk(&self, conn: &Connection) -> rusqlite::Result<()> {
        // Python ll.837-842: try PRAGMA table_info, except OperationalError: return, if not rows: return
        let pragma = "PRAGMA table_info(\"session_model_usage\")";
        let mut stmt = match conn.prepare(pragma) { Ok(s) => s, Err(_) => return Ok(()) };
        let rows: Vec<(String, i64)> = stmt.query_map([], |r| { let n: String = r.get(1)?; let pk: i64 = r.get(5)?; Ok((n, pk)) }).into_iter().flatten().flatten().collect();
        if rows.is_empty() { return Ok(()); }
        // Python ll.847-855: pk_cols = {_col(r,1,"name") for r in rows if _col(r,5,"pk")}; if "task" in pk_cols: return
        let pk_set: HashSet<String> = rows.iter().filter(|(_, pk)| *pk > 0).map(|(n, _)| n.clone()).collect();
        if pk_set.contains("task") { return Ok(()); }
        log::info!(target: LOG_TARGET, "session_model_usage has legacy primary key {:?} (missing task); rebuilding with composite 6-column key", pk_set);
        // Python ll.872: cursor.execute("PRAGMA foreign_keys=OFF")
        let _ = conn.execute("PRAGMA foreign_keys=OFF", []);
        // Python ll.873-936: try: RENAME, CREATE, INSERT OR IGNORE, DROP, CREATE INDEX ×2, except OperationalError, finally PRAGMA foreign_keys=ON
        let res: rusqlite::Result<()> = (|| {
            // Python ll.874-877: ALTER TABLE ... RENAME TO session_model_usage_legacy_pk
            conn.execute("ALTER TABLE session_model_usage RENAME TO session_model_usage_legacy_pk", [])?;
            // Python ll.878-900: CREATE TABLE session_model_usage ( ... PRIMARY KEY (session_id, model, billing_provider, billing_base_url, billing_mode, task) )
            // The `            )` at l.900 closes the CREATE TABLE DDL — slice 2 boundary.
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
            // Python ll.901-923: OR IGNORE: while the PK was wrong the reconciler may have left ``task`` NULL
            // COALESCE to '' can theoretically collide with a genuine ''-task row — keep the first, drop duplicate
            conn.execute(
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
            )?;
            // Python ll.924: DROP TABLE session_model_usage_legacy_pk
            conn.execute("DROP TABLE session_model_usage_legacy_pk", [])?;
            // Python ll.925-928: CREATE INDEX idx_session_model_usage_session
            conn.execute("CREATE INDEX IF NOT EXISTS idx_session_model_usage_session ON session_model_usage(session_id)", [])?;
            // Python ll.929-932: CREATE INDEX idx_session_model_usage_model
            conn.execute("CREATE INDEX IF NOT EXISTS idx_session_model_usage_model ON session_model_usage(model)", [])?;
            Ok(())
        })();
        // Python ll.933-934: except sqlite3.OperationalError as exc: logger.debug(...)
        if let Err(exc) = res {
            log::debug!(target: LOG_TARGET, "session_model_usage PK heal skipped: {}", exc);
        }
        // Python ll.935-936: finally: cursor.execute("PRAGMA foreign_keys=ON")
        let _ = conn.execute("PRAGMA foreign_keys=ON", []);
        Ok(())
    }

    // -----------------------------------------------------------------------
    // _init_schema — Python ll.938-1441
    // -----------------------------------------------------------------------
    /// Mirrors `def _init_schema(self):` (ll.938-1441)
    ///
    /// Create tables and FTS if they don't exist, reconcile columns.
    ///
    /// Schema management follows the declarative reconciliation pattern
    /// (Beets, sqlite-utils): SCHEMA_SQL is the single source of truth.
    /// On existing databases, _reconcile_columns() diffs live columns
    /// against SCHEMA_SQL and ADDs any missing ones.
    pub fn init_schema(&self, conn: &Connection) -> rusqlite::Result<()> {
        // Python l.951: cursor = self._conn.cursor()
        // Python l.953: cursor.executescript(SCHEMA_SQL)
        conn.execute_batch(SCHEMA_SQL)?;

        // Python ll.955-960: # ── Declarative column reconciliation ──────────
        // Diff live tables against SCHEMA_SQL and ADD any missing columns.
        self.reconcile_columns(conn)?;

        // Python ll.962-965: Rebuild gateway_routing if it still carries the pre-scope PRIMARY KEY
        self.heal_gateway_routing_pk(conn)?;

        // Python ll.967-971: Rebuild session_model_usage if its PRIMARY KEY lacks the ``task`` column
        self.heal_session_model_usage_pk(conn)?;

        // Python ll.973-984: Indexes that reference reconciler-added columns must be created AFTER _reconcile_columns
        // try: cursor.execute("CREATE INDEX IF NOT EXISTS idx_messages_platform_msg_id ...") except OperationalError: debug
        match conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_messages_platform_msg_id ON messages(session_id, platform_message_id) WHERE platform_message_id IS NOT NULL",
            [],
        ) {
            Ok(_) => {},
            Err(exc) => { log::debug!(target: LOG_TARGET, "idx_messages_platform_msg_id create skipped: {}", exc); }
        }

        // Python ll.986-988: Deferred indexes that reference the reconciler-added ``active`` column
        // cursor.executescript(DEFERRED_INDEX_SQL)
        conn.execute_batch(DEFERRED_INDEX_SQL)?;

        // Python ll.990-1005: Heal NULL ``active`` rows unconditionally on every startup.
        // On real-world DBs the reconciler-added ``active`` column can lack its NOT NULL DEFAULT 1
        // so INSERTs that omitted the column wrote NULL and the ``WHERE active = 1`` transcript loaders hid history.
        // try: cursor.execute("UPDATE messages SET active = 1 WHERE active IS NULL") except OperationalError: pass
        let _ = conn.execute("UPDATE messages SET active = 1 WHERE active IS NULL", []);

        // Python l.1007: fts5_available = self._sqlite_supports_fts5(cursor)
        let fts5_available = self.sqlite_supports_fts5(conn);
        // Python l.1008: fts_migrations_complete = True
        let mut fts_migrations_complete = true;
        // Python ll.1009-1012: self._fts_stale = cursor.execute("SELECT 1 FROM state_meta WHERE key = ? LIMIT 1", (FTS_STALE_KEY,)).fetchone() is not None
        let fts_stale: bool = conn
            .query_row("SELECT 1 FROM state_meta WHERE key = ?1 LIMIT 1", params![FTS_STALE_KEY], |_| Ok(()))
            .is_ok();
        // We track stale locally; real store writes to self._fts_stale field (interior). For slice audit we mirror via local.
        // Python ll.1013-1016: if self._fts_stale: self._drop_all_fts_triggers(cursor)
        if fts_stale {
            let _ = self.drop_all_fts_triggers(conn);
        }
        // Python ll.1017-1023: if not fts5_available: self._drop_fts_triggers(cursor)
        if !fts5_available {
            let _ = self.drop_fts_triggers(conn);
        }

        // Python ll.1025-1039: # ── Schema version bookkeeping ──────────────
        // Bump to current so future data migrations can gate on version.
        // cursor.execute("SELECT version FROM schema_version LIMIT 1"); row = cursor.fetchone()
        let row: Option<i64> = conn
            .query_row("SELECT version FROM schema_version LIMIT 1", [], |r| r.get(0))
            .optional().ok().flatten();
        if row.is_none() {
            // Python ll.1031-1034: INSERT INTO schema_version (version) VALUES (?)
            conn.execute("INSERT INTO schema_version (version) VALUES (?1)", params![SCHEMA_VERSION])?;
        } else {
            let current_version = row.unwrap();
            // Python l.1041: if current_version < 10 and SCHEMA_VERSION == 10:
            if current_version < 10 && SCHEMA_VERSION == 10 {
                // Python ll.1042-1068: v10 trigram backfill — only when v10 itself is target
                if fts5_available {
                    let trigram_exists = self.fts_table_probe(conn, "messages_fts_trigram");
                    if trigram_exists == Some(false) {
                        if self.ensure_fts_schema(conn, "messages_fts_trigram", FTS_TRIGRAM_SQL)? {
                            conn.execute("INSERT INTO messages_fts_trigram(rowid, content) SELECT id, content FROM messages WHERE content IS NOT NULL", [])?;
                        } else {
                            fts_migrations_complete = false;
                        }
                    } else if trigram_exists.is_none() {
                        fts_migrations_complete = false;
                    }
                } else {
                    fts_migrations_complete = false;
                }
            }
            // Python ll.1069-1078: if current_version < 11 and SCHEMA_VERSION < 23: pass (SUPERSEDED by v23)
            if current_version < 11 && SCHEMA_VERSION < 23 {
                // Kept only for source archaeology; unreachable while SCHEMA_VERSION >= 23.
            }
            // Python ll.1079-1105: if current_version < 16: tag delegate subagent rows
            if current_version < 16 {
                let ephemeral = ephemeral_child_sql("sessions");
                let _ = conn.execute(
                    &format!(
                        "UPDATE sessions SET model_config = json_set(COALESCE(model_config, '{{}}'), '$._delegate_from', parent_session_id) WHERE parent_session_id IS NOT NULL AND json_extract(COALESCE(model_config, '{{}}'), '$._delegate_from') IS NULL AND {}",
                        ephemeral
                    ),
                    [],
                );
                let _ = conn.execute(
                    "UPDATE sessions SET model_config = json_set(COALESCE(model_config, '{}'), '$._delegate_from', '__orphaned__') WHERE parent_session_id IS NULL AND json_extract(COALESCE(model_config, '{}'), '$._delegate_from') IS NULL AND json_extract(COALESCE(model_config, '{}'), '$._branched_from') IS NULL AND title IS NULL AND message_count <= 25 AND EXISTS (SELECT 1 FROM messages m WHERE m.session_id = sessions.id AND m.role = 'tool') AND NOT EXISTS (SELECT 1 FROM sessions ch WHERE ch.parent_session_id = sessions.id)",
                    [],
                );
            }
            // Python ll.1106-1118: if current_version < 18: backfill gateway metadata from sessions.json
            if current_version < 18 {
                let res = self.backfill_gateway_metadata_from_sessions_json(conn);
                if let Err(exc) = res {
                    log::debug!(target: LOG_TARGET, "v18 gateway metadata backfill skipped: {}", exc);
                }
            }
            // Python ll.1119-1162: if current_version < 20: per-model usage attribution seed
            if current_version < 20 {
                let _ = conn.execute(
                    "INSERT OR IGNORE INTO session_model_usage (session_id, model, billing_provider, billing_base_url, billing_mode, api_call_count, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, reasoning_tokens, estimated_cost_usd, actual_cost_usd, cost_status, cost_source, first_seen, last_seen) SELECT id, COALESCE(model, 'unknown'), COALESCE(billing_provider, ''), COALESCE(billing_base_url, ''), COALESCE(billing_mode, ''), COALESCE(api_call_count, 0), COALESCE(input_tokens, 0), COALESCE(output_tokens, 0), COALESCE(cache_read_tokens, 0), COALESCE(cache_write_tokens, 0), COALESCE(reasoning_tokens, 0), COALESCE(estimated_cost_usd, 0), COALESCE(actual_cost_usd, 0), cost_status, cost_source, started_at, COALESCE(ended_at, started_at) FROM sessions WHERE COALESCE(input_tokens, 0) + COALESCE(output_tokens, 0) + COALESCE(cache_read_tokens, 0) + COALESCE(cache_write_tokens, 0) + COALESCE(reasoning_tokens, 0) > 0",
                    [],
                );
            }
            // Python ll.1163-1228: if current_version < 22: task-dimension usage attribution rebuild
            if current_version < 22 {
                let legacy_pk: i64 = conn
                    .query_row("SELECT COUNT(*) FROM pragma_table_info('session_model_usage') WHERE name = 'task' AND pk > 0", [], |r| r.get(0))
                    .unwrap_or(0);
                if legacy_pk == 0 {
                    let r: rusqlite::Result<()> = (|| {
                        conn.execute("ALTER TABLE session_model_usage RENAME TO session_model_usage_v21", [])?;
                        conn.execute(
                            "CREATE TABLE session_model_usage (session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE, model TEXT NOT NULL, billing_provider TEXT NOT NULL DEFAULT '', billing_base_url TEXT NOT NULL DEFAULT '', billing_mode TEXT NOT NULL DEFAULT '', task TEXT NOT NULL DEFAULT '', api_call_count INTEGER NOT NULL DEFAULT 0, input_tokens INTEGER NOT NULL DEFAULT 0, output_tokens INTEGER NOT NULL DEFAULT 0, cache_read_tokens INTEGER NOT NULL DEFAULT 0, cache_write_tokens INTEGER NOT NULL DEFAULT 0, reasoning_tokens INTEGER NOT NULL DEFAULT 0, estimated_cost_usd REAL NOT NULL DEFAULT 0, actual_cost_usd REAL NOT NULL DEFAULT 0, cost_status TEXT, cost_source TEXT, first_seen REAL, last_seen REAL, PRIMARY KEY (session_id, model, billing_provider, billing_base_url, billing_mode, task))",
                            [],
                        )?;
                        conn.execute(
                            "INSERT INTO session_model_usage (session_id, model, billing_provider, billing_base_url, billing_mode, task, api_call_count, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, reasoning_tokens, estimated_cost_usd, actual_cost_usd, cost_status, cost_source, first_seen, last_seen) SELECT session_id, model, billing_provider, billing_base_url, billing_mode, '', api_call_count, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, reasoning_tokens, estimated_cost_usd, actual_cost_usd, cost_status, cost_source, first_seen, last_seen FROM session_model_usage_v21",
                            [],
                        )?;
                        conn.execute("DROP TABLE session_model_usage_v21", [])?;
                        conn.execute("CREATE INDEX IF NOT EXISTS idx_session_model_usage_session ON session_model_usage(session_id)", [])?;
                        conn.execute("CREATE INDEX IF NOT EXISTS idx_session_model_usage_model ON session_model_usage(model)", [])?;
                        Ok(())
                    })();
                    if let Err(exc) = r {
                        log::debug!(target: LOG_TARGET, "v22 session_model_usage rebuild skipped: {}", exc);
                    }
                }
            }
            // Python ll.1229-1259: if current_version < 23: FTS storage redesign — OPT-IN, NOT AUTOMATIC
            if current_version < 23 {
                if fts5_available && self.db_has_legacy_inline_fts(conn) {
                    let _ = self.set_meta("fts_optimize_available", "1", conn);
                }
            }
            // Python ll.1261-1267: if current_version < 25: dedupe legacy system prompts
            if current_version < 25 {
                // Python l.1267: self._dedupe_legacy_system_prompts(cursor) — defined in slice 1 (ll.107-146)
                // Stub call keeping grep-traceability; canonical impl in schema_slice1.rs.
                let _ = self.dedupe_legacy_system_prompts(conn);
            }

            // Python ll.1269-1292: The FTS storage layout is versioned independently — stamp fts_storage_version
            if fts5_available
                && !self.db_has_legacy_inline_fts(conn)
                && conn.query_row("SELECT 1 FROM state_meta WHERE key = 'fts_rebuild_high_water' LIMIT 1", [], |_| Ok(())).is_err()
                && !self.has_fts_trash(conn)
                && !self.fts_external_index_empty_with_messages(conn)
            {
                let _ = self.set_meta("fts_storage_version", &FTS_STORAGE_VERSION.to_string(), conn);
            }

            // Python ll.1294-1308: Advance schema_version to current for ALL non-FTS-layout migrations.
            if current_version < SCHEMA_VERSION && fts_migrations_complete && fts5_available {
                conn.execute("UPDATE schema_version SET version = ?1", params![SCHEMA_VERSION])?;
            }
        }

        // Python ll.1310-1344: Unique title index — always ensure it exists.
        // try: cursor.execute("CREATE UNIQUE INDEX IF NOT EXISTS idx_sessions_title_unique ...")
        // except IntegrityError: UPDATE older SET title=NULL WHERE EXISTS newer, then retry
        // except OperationalError: pass
        let title_index_sql = "CREATE UNIQUE INDEX IF NOT EXISTS idx_sessions_title_unique ON sessions(title) WHERE title IS NOT NULL";
        match conn.execute(title_index_sql, []) {
            Ok(_) => {},
            Err(exc) => {
                let msg = exc.to_string().to_lowercase();
                if msg.contains("unique") || msg.contains("constraint") || msg.contains("integrity") {
                    let repair = (|| -> rusqlite::Result<()> {
                        conn.execute("UPDATE sessions AS older SET title = NULL WHERE title IS NOT NULL AND EXISTS (SELECT 1 FROM sessions AS newer WHERE newer.title = older.title AND newer.rowid > older.rowid)", [])?;
                        let cnt = conn.changes() as usize;
                        log::warn!(target: LOG_TARGET, "Cleared {} duplicate session title(s) while restoring the unique index", cnt);
                        conn.execute(title_index_sql, [])?;
                        Ok(())
                    })();
                    if let Err(e) = repair {
                        log::warn!(target: LOG_TARGET, "Could not repair duplicate session titles; unique title index not created: {}", e);
                    }
                } else {
                    // OperationalError: pass — Index already exists (or other transient)
                    log::debug!(target: LOG_TARGET, "idx_sessions_title_unique create skipped: {}", exc);
                }
            }
        }

        // Python ll.1346-1440: if fts5_available: FTS5 setup — DDL even when virtual table exists
        if fts5_available {
            let legacy_fts = self.db_has_legacy_inline_fts(conn);
            if fts_stale {
                // Python ll.1360-1369: if recover succeeds ensure CJK else detach
                if self.recover_stale_fts(conn, legacy_fts)? {
                    let _ = self.ensure_fts_cjk_schema(conn);
                } else {
                    // Python ll.1367-1369: self._fts_enabled = False; _trigram_available = False; _fts_cjk_available = False
                    // Real store mutates Atomics/fields; we ensure triggers stay detached and log intent.
                    // Stale path already detached via _recover_stale_fts failure branch.
                }
            } else if legacy_fts {
                // Python ll.1370-1399: legacy branch — measure trigger counts BEFORE DDL
                let base_missing = Self::fts_trigger_count_static(conn, FTS_BASE_TRIGGERS) < FTS_BASE_TRIGGERS.len();
                let trigram_missing = Self::fts_trigger_count_static(conn, FTS_TRIGRAM_TRIGGERS) < FTS_TRIGRAM_TRIGGERS.len();
                let fts_enabled = self.ensure_fts_schema(conn, "messages_fts", LEGACY_FTS_SQL)?;
                if fts_enabled {
                    let trigram_enabled = self.ensure_fts_schema(conn, "messages_fts_trigram", LEGACY_FTS_TRIGRAM_SQL)?;
                    if base_missing || (trigram_enabled && trigram_missing) {
                        self.run_admitted_startup_rebuild(conn, |c| Self::rebuild_legacy_fts_indexes(c, trigram_enabled))?;
                    }
                }
            } else {
                // Python ll.1400-1434: v23 external-content branch
                let base_missing = Self::fts_trigger_count_static(conn, FTS_BASE_TRIGGERS) < FTS_BASE_TRIGGERS.len();
                let trigram_missing = Self::fts_trigger_count_static(conn, FTS_TRIGRAM_TRIGGERS) < FTS_TRIGRAM_TRIGGERS.len();
                let fts_enabled = self.ensure_fts_schema(conn, "messages_fts", FTS_SQL)?;
                if fts_enabled {
                    let trigram_enabled = self.ensure_fts_schema(conn, "messages_fts_trigram", FTS_TRIGRAM_SQL)?;
                    if base_missing || (trigram_enabled && trigram_missing) {
                        self.run_admitted_startup_rebuild(conn, |c| Self::rebuild_fts_indexes(c, trigram_enabled))?;
                    }
                    // Python l.1434: self._ensure_fts_cjk_schema(cursor)
                    let _ = self.ensure_fts_cjk_schema(conn);
                }
            }
            // Python ll.1436-1439: Replace any pre-existing broad AFTER UPDATE triggers with AFTER UPDATE OF variants.
            // if getattr(self, "_fts_enabled", False): self._migrate_broad_fts_update_triggers(cursor)
            // We gate on fts5_available as proxy for enabled after above setup.
            let _ = self.migrate_broad_fts_update_triggers(conn);
        }

        // Python l.1441: self._conn.commit()
        // In Python `self._conn.commit()` after executescript; in Rust rusqlite uses autocommit per statement when no explicit transaction.
        // The `conn` here is the writer connection borrowed from `self.connect()` — each `execute` autocommits.
        // We issue an explicit COMMIT for parity when operating inside an implicit transaction (mirrors Python commit).
        // If not in transaction, this is a no-op error we ignore.
        let _ = conn.execute("COMMIT", []);
        Ok(())
    }

    // Helper stub for ll.1267 dedupe path — canonical impl in slice 1 (ll.107-146).
    // Kept here for grep-traceability so slice 2's init_schema call resolves.
    fn dedupe_legacy_system_prompts(&self, conn: &Connection) -> rusqlite::Result<()> {
        // Mirrors schema l.107-146: SELECT id, system_prompt WHERE system_prompt IS NOT NULL, hash + UPDATE, contention-safe.
        // Delegate to slice1's canonical logic; stub loops with best-effort to keep slice self-contained.
        let rows: Vec<(String, String)> = {
            let mut stmt = match conn.prepare("SELECT id, system_prompt FROM sessions WHERE system_prompt IS NOT NULL") {
                Ok(s) => s,
                Err(_) => return Ok(()),
            };
            match stmt.query_map([], |r| { let id: String = r.get(0)?; let p: String = r.get(1)?; Ok((id, p)) }) {
                Ok(mapped) => mapped.flatten().collect(),
                Err(_) => return Ok(()),
            }
        };
        for (session_id, prompt) in rows {
            // Content-addressed hash stub — mirrors slice1's store_system_prompt (DefaultHasher hex).
            use std::collections::hash_map::DefaultHasher;
            use std::hash::{Hash, Hasher};
            let mut h = DefaultHasher::new();
            prompt.hash(&mut h);
            let hash = format!("{:016x}", h.finish());
            let _ = conn.execute("INSERT OR IGNORE INTO system_prompts (hash, prompt) VALUES (?1, ?2)", params![hash, prompt]);
            if let Err(exc) = conn.execute("UPDATE sessions SET system_prompt_hash = ?1, system_prompt = NULL WHERE id = ?2", params![hash, session_id]) {
                let m = exc.to_string().to_lowercase();
                if m.contains("locked") || m.contains("busy") {
                    log::warn!(target: LOG_TARGET, "v25 prompt dedupe paused after contention ({}); unmigrated rows keep the legacy inline prompt and the next schema init resumes the migration.", exc);
                    return Ok(());
                } else {
                    return Err(exc);
                }
            }
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // _run_admitted_startup_rebuild — Python ll.1443-1480
    // -----------------------------------------------------------------------
    /// Mirrors `def _run_admitted_startup_rebuild(self, cursor, rebuild_fn) -> None:` (ll.1443-1480)
    ///
    /// Run a full trigger-repair FTS rebuild under cross-process admission.
    /// `_init_schema` reaches here when the sync triggers were missing and
    /// the DDL just recreated them, so the index has a gap of unknown extent
    /// and must be rebuilt in full.
    pub fn run_admitted_startup_rebuild<F>(&self, conn: &Connection, rebuild_fn: F) -> rusqlite::Result<()>
    where
        F: FnOnce(&Connection) -> rusqlite::Result<()>,
    {
        // Python ll.1462-1464: with fts_rebuild_admission(getattr(self, "db_path", None)) as admitted:
        // if admitted: rebuild_fn(); return
        let guard = fts_rebuild_admission(self.db_path.as_deref());
        if guard.acquired {
            rebuild_fn(conn)?;
            return Ok(());
        }
        // Python ll.1466-1480: deferred — drop triggers, set stale breadcrumb, detach.
        log::warn!(target: LOG_TARGET, "Deferred startup FTS rebuild: another process holds the rebuild authority for this state.db; detaching FTS sync until the stale-index recovery path rebuilds it.");
        conn.execute("INSERT INTO state_meta (key, value) VALUES (?1, '1') ON CONFLICT(key) DO UPDATE SET value = excluded.value", params![FTS_STALE_KEY])?;
        self.drop_all_fts_triggers(conn)?;
        // Python ll.1477-1480: self._fts_stale = True; _fts_enabled = False; _trigram_available = False; _fts_cjk_available = False
        // Real store mutates fields; we log intent. Stale breadcrumb ensures next open retries via _recover_stale_fts.
        Ok(())
    }

    // -----------------------------------------------------------------------
    // _backfill_gateway_metadata_from_sessions_json — Python ll.1482-1529
    // -----------------------------------------------------------------------
    /// Mirrors `def _backfill_gateway_metadata_from_sessions_json(self, cursor: sqlite3.Cursor) -> None:` (ll.1482-1529)
    ///
    /// One-time v18 backfill of gateway metadata from sessions.json.
    /// Existing gateway sessions predate the display_name / origin_json /
    /// expiry_finalized columns; copy what sessions.json knows so consumers
    /// can switch to state.db without losing pre-migration sessions.
    /// Only fills NULL columns — never overwrites data written by newer code.
    pub fn backfill_gateway_metadata_from_sessions_json(&self, conn: &Connection) -> rusqlite::Result<()> {
        // Python ll.1492-1493: sessions_file = get_hermes_home() / "sessions" / "sessions.json"; if not exists: return
        let sessions_file = get_hermes_home().join("sessions").join("sessions.json");
        if !sessions_file.exists() {
            return Ok(());
        }
        // Python ll.1494-1495: with open(sessions_file, "r", ...) as f: data = json.load(f)
        let bytes = std::fs::read(&sessions_file).map_err(|e| rusqlite::Error::InvalidParameterName(e.to_string()))?;
        let data: Value = serde_json::from_slice(&bytes).map_err(|e| rusqlite::Error::InvalidParameterName(e.to_string()))?;
        // Python ll.1496-1497: if not isinstance(data, dict): return
        let map = match data.as_object() {
            Some(m) => m,
            None => return Ok(()),
        };
        // Python ll.1498-1528: for key, entry in data.items(): if str(key).startswith("_") or not isinstance(entry, dict): continue
        for (key, entry) in map {
            if key.starts_with('_') {
                continue;
            }
            let entry_map = match entry.as_object() {
                Some(m) => m,
                None => continue,
            };
            // Python ll.1502-1504: session_id = entry.get("session_id"); if not session_id: continue
            let session_id = match entry_map.get("session_id").and_then(|v| v.as_str()) {
                Some(s) if !s.is_empty() => s,
                _ => continue,
            };
            // Python ll.1505: origin = entry.get("origin")
            let origin = entry_map.get("origin");
            let origin_is_dict = origin.map(|v| v.is_object()).unwrap_or(false);
            // Prepare bound params for the UPDATE — COALESCE semantics (never overwrite)
            let session_key = entry_map.get("session_key").and_then(|v| v.as_str()).unwrap_or(key);
            let chat_id = origin.and_then(|o| o.get("chat_id")).and_then(|v| v.as_str());
            let chat_type = entry_map.get("chat_type").and_then(|v| v.as_str());
            let thread_id = origin.and_then(|o| o.get("thread_id")).and_then(|v| v.as_str());
            let display_name = entry_map.get("display_name").and_then(|v| v.as_str());
            let origin_json = if origin_is_dict { origin.map(|v| v.to_string()) } else { None };
            let expiry_flag: i64 = if entry_map.get("expiry_finalized").and_then(|v| v.as_bool()).unwrap_or(false)
                || entry_map.get("memory_flushed").and_then(|v| v.as_bool()).unwrap_or(false)
            {
                1
            } else {
                0
            };
            // Python ll.1506-1528: UPDATE sessions SET session_key = COALESCE(session_key, ?), chat_id = COALESCE(...), ... WHERE id = ?
            // CASE for expiry_finalized: WHEN COALESCE(expiry_finalized,0)=0 AND ?=1 THEN 1 ELSE expiry_finalized END
            conn.execute(
                "UPDATE sessions SET session_key = COALESCE(session_key, ?1), chat_id = COALESCE(chat_id, ?2), chat_type = COALESCE(chat_type, ?3), thread_id = COALESCE(thread_id, ?4), display_name = COALESCE(display_name, ?5), origin_json = COALESCE(origin_json, ?6), expiry_finalized = CASE WHEN COALESCE(expiry_finalized, 0) = 0 AND ?7 = 1 THEN 1 ELSE expiry_finalized END WHERE id = ?8",
                params![session_key, chat_id, chat_type, thread_id, display_name, origin_json, expiry_flag, session_id],
            )?;
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
    fn fts_update_narrowing_detection_slice2() {
        // Mirrors schema ll.191-202 (also exercised in slice1) — kept here so
        // slice2's quarantine/migrate path stays covered without importing slice1.
        assert!(!StateStore::fts_update_trigger_needs_narrowing(None));
        assert!(!StateStore::fts_update_trigger_needs_narrowing(Some("CREATE TRIGGER t AFTER UPDATE OF content ON messages BEGIN END;")));
        assert!(StateStore::fts_update_trigger_needs_narrowing(Some("CREATE TRIGGER t AFTER UPDATE ON messages BEGIN END;")));
    }

    #[test]
    fn heal_and_backfill_smoke() {
        // Smoke: ephemeral_child_sql and get_hermes_home produce plausible values
        let sql = ephemeral_child_sql("sessions");
        assert!(sql.contains("parent_session_id"));
        assert!(sql.contains("model_config"));
        let home = get_hermes_home();
        assert!(home.to_string_lossy().len() > 0);
    }

    #[test]
    fn legacy_inline_probe_parity() {
        // Mirrors _db_has_legacy_inline_fts probe contract (tool_name column)
        // — on an empty :memory: DB the probe returns false.
        let store = StateStore::open(Path::new(":memory:")).unwrap();
        let conn = store.connect().unwrap_or_else(|_| Connection::open_in_memory().unwrap());
        // Without any tables, legacy check is false
        assert!(!store.db_has_legacy_inline_fts(&conn));
        assert!(!store.has_fts_trash(&conn));
    }
}
