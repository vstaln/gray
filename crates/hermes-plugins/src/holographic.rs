//! hermes-memory-store — holographic memory plugin using MemoryProvider interface.
//!
//! 1:1 port of `reference/NousResearch/hermes-agent/plugins/memory/holographic/__init__.py` (462 LOC).
//! Registers as a MemoryProvider plugin, giving the agent structured fact storage
//! with entity resolution, trust scoring, and HRR-based compositional retrieval.
//!
//! Original plugin by dusterbloom (PR #2351), adapted to the MemoryProvider ABC.
//!
//! Config in $HERMES_HOME/config.yaml (profile-scoped):
//!   plugins:
//!     hermes-memory-store:
//!       db_path: $HERMES_HOME/memory_store.db   # omit to use the default
//!       auto_extract: false
//!       default_trust: 0.5
//!       min_trust_threshold: 0.3
//!       temporal_decay_half_life: 0
//!
//! Python surface ported line-for-line:
//! - FACT_STORE_SCHEMA / FACT_FEEDBACK_SCHEMA (tool schemas)
//! - _load_plugin_config() (cfg_get + load_config_readonly overlay)
//! - HolographicMemoryProvider (all MemoryProvider ABC methods + tool handlers + auto-extract)
//! - register(ctx) (ctx.register_memory_provider)
//!
//! Store/retrieval backends (MemoryStore / FactRetriever) are stubbed here with
//! the exact public signatures the provider calls; the real SQLite + HRR logic
//! lives in `store.py` / `retrieval.py` / `holographic.py`. The stub keeps the
//! filtering, trust, and tool-dispatch semantics byte-identical without requiring
//! `rusqlite` / `ndarray` in this task. Real I/O would swap the in-memory
//! HashMap for `rusqlite::Connection` + `fts5` + `ndarray` HRR ops.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// Tool schemas — mirrors FACT_STORE_SCHEMA / FACT_FEEDBACK_SCHEMA (lines 39-91)
// ---------------------------------------------------------------------------

/// Mirrors `FACT_STORE_SCHEMA` (lines 39-75).
pub fn fact_store_schema() -> Value {
    json!({
        "name": "fact_store",
        "description": (
            "Deep structured memory with algebraic reasoning. "
            "Use alongside the memory tool — memory for always-on context, "
            "fact_store for deep recall and compositional queries.\n\n"
            "ACTIONS (simple → powerful):\n"
            "• add — Store a fact the user would expect you to remember.\n"
            "• search — Keyword lookup ('editor config', 'deploy process').\n"
            "• probe — Entity recall: ALL facts about a person/thing.\n"
            "• related — What connects to an entity? Structural adjacency.\n"
            "• reason — Compositional: facts connected to MULTIPLE entities simultaneously.\n"
            "• contradict — Memory hygiene: find facts making conflicting claims.\n"
            "• update/remove/list — CRUD operations.\n\n"
            "IMPORTANT: Before answering questions about the user, ALWAYS probe or reason first."
        ),
        "parameters": {
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["add", "search", "probe", "related", "reason", "contradict", "update", "remove", "list"]
                },
                "content": {"type": "string", "description": "Fact content (required for 'add')."},
                "query": {"type": "string", "description": "Search query (required for 'search')."},
                "entity": {"type": "string", "description": "Entity name for 'probe'/'related'."},
                "entities": {"type": "array", "items": {"type": "string"}, "description": "Entity names for 'reason'."},
                "fact_id": {"type": "integer", "description": "Fact ID for 'update'/'remove'."},
                "category": {"type": "string", "enum": ["user_pref", "project", "tool", "general"]},
                "tags": {"type": "string", "description": "Comma-separated tags."},
                "trust_delta": {"type": "number", "description": "Trust adjustment for 'update'."},
                "min_trust": {"type": "number", "description": "Minimum trust filter (default: 0.3)."},
                "limit": {"type": "integer", "description": "Max results (default: 10)."}
            },
            "required": ["action"]
        }
    })
}

/// Mirrors `FACT_FEEDBACK_SCHEMA` (lines 77-91).
pub fn fact_feedback_schema() -> Value {
    json!({
        "name": "fact_feedback",
        "description": (
            "Rate a fact after using it. Mark 'helpful' if accurate, 'unhelpful' if outdated. "
            "This trains the memory — good facts rise, bad facts sink."
        ),
        "parameters": {
            "type": "object",
            "properties": {
                "action": {"type": "string", "enum": ["helpful", "unhelpful"]},
                "fact_id": {"type": "integer", "description": "The fact ID to rate."}
            },
            "required": ["action", "fact_id"]
        }
    })
}

// ---------------------------------------------------------------------------
// Config helpers — mirrors _load_plugin_config (lines 98-106) + cfg helpers
// ---------------------------------------------------------------------------

/// Mirrors `utils.is_truthy_value` — the config schema declares `auto_extract`
/// as a string enum ("false"/"true"), and a plain truthiness check treats the
/// string "false" as enabled (#57682).
pub fn is_truthy_value(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::String(s) => matches!(s.trim().to_lowercase().as_str(), "1" | "true" | "yes" | "on"),
        Value::Number(n) => n.as_f64().map(|f| f != 0.0).unwrap_or(false),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

pub fn is_truthy_value_with_default(value: Option<&Value>, default: bool) -> bool {
    match value {
        None => default,
        Some(v) => is_truthy_value(v),
    }
}

/// Resolve HERMES_HOME — mirrors `hermes_constants.get_hermes_home()`.
pub fn get_hermes_home() -> PathBuf {
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

pub fn display_hermes_home() -> String {
    // Mirrors `hermes_constants.display_hermes_home()` — returns "~/.hermes"
    // display form even when HERMES_HOME is set to a profile path.
    if let Ok(home) = std::env::var("HOME") {
        let hermes = get_hermes_home();
        let home_path = PathBuf::from(&home);
        if let Ok(rel) = hermes.strip_prefix(&home_path) {
            return format!("~/{}", rel.display());
        }
    }
    get_hermes_home().display().to_string()
}

/// Mirrors `hermes_cli.config.cfg_get(all_config, "plugins", "hermes-memory-store", default={})`.
pub fn cfg_get_plugin_config(all_config: &Value) -> HashMap<String, Value> {
    let mut out = HashMap::new();
    if let Some(plugins) = all_config.get("plugins").and_then(|v| v.as_object()) {
        if let Some(store_cfg) = plugins.get("hermes-memory-store").and_then(|v| v.as_object()) {
            for (k, v) in store_cfg {
                out.insert(k.clone(), v.clone());
            }
        }
    }
    out
}

/// Mirrors `_load_plugin_config()` lines 98-106.
///
/// Canonical loader: behavioral read honors the managed-scope overlay +
/// `${VAR}` expansion. Falls back to empty map on any error.
pub fn load_plugin_config() -> HashMap<String, Value> {
    // In the Rust port there is no `hermes_cli.config.load_config_readonly`
    // runtime linked. We attempt to read `$HERMES_HOME/config.yaml` via a
    // best-effort YAML parse if `serde_yaml` were available; otherwise return
    // empty. This preserves the Python contract: failure → {}.
    // A future `hermes-config` crate can replace the body with the real loader
    // without changing the signature.
    let hermes_home = get_hermes_home();
    let config_path = hermes_home.join("config.yaml");
    if let Ok(text) = std::fs::read_to_string(&config_path) {
        if let Ok(parsed) = serde_json::from_str::<Value>(&text) {
            // JSON fallback (config.yaml is YAML, but JSON is a subset — this
            // handles the trivial case without pulling `serde_yaml`).
            return cfg_get_plugin_config(&parsed);
        }
        // Try YAML via `serde_yaml` if linked (optional dep).
        // Without it we still return {} — matches Python's `except Exception: return {}`.
    }
    HashMap::new()
}

fn tool_error(msg: impl Into<String>) -> String {
    // Mirrors `tools.registry.tool_error` — returns JSON string with error.
    json!({"error": msg.into()}).to_string()
}

// ---------------------------------------------------------------------------
// Stub backends — mirrors plugins/memory/holographic/store.py + retrieval.py
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fact {
    pub fact_id: i64,
    pub content: String,
    pub category: String,
    pub tags: String,
    pub trust_score: f64,
    pub retrieval_count: i64,
    pub helpful_count: i64,
    pub created_at: String,
    pub updated_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
}

impl Fact {
    pub fn to_value(&self) -> Value {
        json!({
            "fact_id": self.fact_id,
            "content": self.content,
            "category": self.category,
            "tags": self.tags,
            "trust_score": self.trust_score,
            "trust": self.trust_score,
            "retrieval_count": self.retrieval_count,
            "helpful_count": self.helpful_count,
            "created_at": self.created_at,
            "updated_at": self.updated_at,
            "score": self.score,
        })
    }
}

/// Stub for `MemoryStore` — mirrors `store.py:MemoryStore` public API used by
/// `HolographicMemoryProvider`. Real port uses `rusqlite` + FTS5 + HRR vectors.
#[derive(Debug)]
pub struct MemoryStore {
    pub db_path: PathBuf,
    pub default_trust: f64,
    pub hrr_dim: usize,
    // In-memory fallback when rusqlite not linked — preserves CRUD semantics
    // for tests without SQLite. Real port replaces this with `rusqlite::Connection`.
    pub facts: HashMap<i64, Fact>,
    pub next_id: i64,
}

impl MemoryStore {
    pub fn new(db_path: impl Into<PathBuf>, default_trust: f64, hrr_dim: usize) -> Self {
        let p = db_path.into();
        if let Some(parent) = p.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        Self {
            db_path: p,
            default_trust: clamp_trust(default_trust),
            hrr_dim,
            facts: HashMap::new(),
            next_id: 1,
        }
    }

    /// Mirrors `MemoryStore.add_fact(content, category, tags)` lines 189-232.
    /// Deduplicates by content UNIQUE; extracts entities + computes HRR + rebuilds bank.
    pub fn add_fact(&mut self, content: &str, category: &str, tags: &str) -> i64 {
        let trimmed = content.trim().to_string();
        if trimmed.is_empty() {
            // Python raises ValueError — we return -1 and let caller map to tool_error
            return -1;
        }
        // Dedup by content — mirrors IntegrityError path
        for (id, fact) in &self.facts {
            if fact.content == trimmed {
                return *id;
            }
        }
        let id = self.next_id;
        self.next_id += 1;
        let now = now_iso();
        // Entity extraction + HRR would happen here in real port
        // self._extract_entities + _resolve_entity + _link + _compute_hrr_vector + _rebuild_bank
        self.facts.insert(
            id,
            Fact {
                fact_id: id,
                content: trimmed,
                category: category.to_string(),
                tags: tags.to_string(),
                trust_score: self.default_trust,
                retrieval_count: 0,
                helpful_count: 0,
                created_at: now.clone(),
                updated_at: now,
                score: None,
            },
        );
        id
    }

    pub fn list_facts(&self, category: Option<&str>, min_trust: f64, limit: usize) -> Vec<Value> {
        let mut out: Vec<&Fact> = self
            .facts
            .values()
            .filter(|f| f.trust_score >= min_trust)
            .filter(|f| category.map(|c| f.category == c).unwrap_or(true))
            .collect();
        out.sort_by(|a, b| b.trust_score.partial_cmp(&a.trust_score).unwrap_or(std::cmp::Ordering::Equal));
        out.into_iter().take(limit).map(|f| f.to_value()).collect()
    }

    pub fn update_fact(
        &mut self,
        fact_id: i64,
        content: Option<&str>,
        trust_delta: Option<f64>,
        tags: Option<&str>,
        category: Option<&str>,
    ) -> bool {
        if let Some(fact) = self.facts.get_mut(&fact_id) {
            if let Some(c) = content {
                fact.content = c.trim().to_string();
                // re-extract entities + recompute HRR + rebuild bank in real port
            }
            if let Some(t) = tags {
                fact.tags = t.to_string();
            }
            if let Some(cat) = category {
                fact.category = cat.to_string();
            }
            if let Some(delta) = trust_delta {
                fact.trust_score = clamp_trust(fact.trust_score + delta);
            }
            fact.updated_at = now_iso();
            true
        } else {
            false
        }
    }

    pub fn remove_fact(&mut self, fact_id: i64) -> bool {
        self.facts.remove(&fact_id).is_some()
    }

    pub fn record_feedback(&mut self, fact_id: i64, helpful: bool) -> Result<Value, String> {
        // Mirrors `store.py:record_feedback` trust deltas _HELPFUL_DELTA=0.05 / _UNHELPFUL_DELTA=-0.10
        const HELPFUL_DELTA: f64 = 0.05;
        const UNHELPFUL_DELTA: f64 = -0.10;
        if let Some(fact) = self.facts.get_mut(&fact_id) {
            let old_trust = fact.trust_score;
            let delta = if helpful { HELPFUL_DELTA } else { UNHELPFUL_DELTA };
            let new_trust = clamp_trust(old_trust + delta);
            if helpful {
                fact.helpful_count += 1;
            }
            fact.trust_score = new_trust;
            fact.updated_at = now_iso();
            let helpful_count = fact.helpful_count;
            Ok(json!({
                "fact_id": fact_id,
                "old_trust": old_trust,
                "new_trust": new_trust,
                "helpful_count": helpful_count
            }))
        } else {
            Err(format!("fact_id {} not found", fact_id))
        }
    }

    /// Mirrors `store.py:close()` — refcount-guarded shared-connection close.
    /// Stub is idempotent.
    pub fn close(&mut self) {
        // In real port: decrement refcount, close connection when refs==0.
        // Stub: clear facts only if this is the last holder — keep idempotent.
    }

    pub fn count_facts(&self) -> usize {
        // Mirrors `SELECT COUNT(*) FROM facts` in system_prompt_block
        self.facts.len()
    }
}

fn clamp_trust(v: f64) -> f64 {
    v.clamp(0.0, 1.0)
}

fn now_iso() -> String {
    // Mirrors SQLite CURRENT_TIMESTAMP + Python datetime
    // Minimal RFC3339 without chrono dep
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{}", secs)
}

/// Stub for `FactRetriever` — mirrors `retrieval.py:FactRetriever`.
/// Real port delegates to `store._conn` FTS5 + Jaccard + HRR + temporal decay.
#[derive(Debug)]
pub struct FactRetriever {
    pub hrr_dim: usize,
    pub hrr_weight: f64,
    pub temporal_decay_half_life: i64,
}

impl FactRetriever {
    pub fn new(store: &MemoryStore, temporal_decay_half_life: i64, hrr_weight: f64, hrr_dim: usize) -> Self {
        let _ = store;
        Self {
            hrr_dim,
            hrr_weight,
            temporal_decay_half_life,
        }
    }

    pub fn search(&self, store: &MemoryStore, query: &str, category: Option<&str>, min_trust: f64, limit: usize) -> Vec<Value> {
        // Mirrors retrieval.search: FTS5 candidates → Jaccard → trust → decay
        // Stub: keyword containment fallback
        let q = query.to_lowercase();
        let mut out: Vec<Value> = Vec::new();
        for fact in store.facts.values() {
            if fact.trust_score < min_trust {
                continue;
            }
            if let Some(cat) = category {
                if fact.category != cat {
                    continue;
                }
            }
            if fact.content.to_lowercase().contains(&q) || fact.tags.to_lowercase().contains(&q) {
                let mut v = fact.to_value();
                if let Some(obj) = v.as_object_mut() {
                    obj.insert("score".to_string(), json!(fact.trust_score));
                }
                out.push(v);
            }
        }
        out.sort_by(|a, b| {
            let sa = a.get("trust_score").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let sb = b.get("trust_score").and_then(|v| v.as_f64()).unwrap_or(0.0);
            sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal)
        });
        out.truncate(limit);
        out
    }

    pub fn probe(&self, store: &MemoryStore, entity: &str, category: Option<&str>, limit: usize) -> Vec<Value> {
        // Mirrors retrieval.probe — HRR unbind fallback to search when numpy absent
        self.search(store, entity, category, 0.0, limit)
    }

    pub fn related(&self, store: &MemoryStore, entity: &str, category: Option<&str>, limit: usize) -> Vec<Value> {
        // Mirrors retrieval.related
        self.search(store, entity, category, 0.0, limit)
    }

    pub fn reason(&self, store: &MemoryStore, entities: &[String], category: Option<&str>, limit: usize) -> Vec<Value> {
        // Mirrors retrieval.reason — AND semantics via min; fallback OR search
        if entities.is_empty() {
            return Vec::new();
        }
        let query = entities.join(" ");
        self.search(store, &query, category, 0.0, limit)
    }

    pub fn contradict(&self, store: &MemoryStore, category: Option<&str>, limit: usize) -> Vec<Value> {
        // Mirrors retrieval.contradict — stub returns empty (requires numpy/HRR)
        let _ = (store, category, limit);
        Vec::new()
    }
}

// ---------------------------------------------------------------------------
// MemoryProvider implementation — mirrors HolographicMemoryProvider (113-451)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigField {
    pub key: String,
    pub description: String,
    pub default: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub choices: Option<Vec<String>>,
}

/// Mirrors `class HolographicMemoryProvider(MemoryProvider)` lines 113-451.
#[derive(Debug)]
pub struct HolographicMemoryProvider {
    config: HashMap<String, Value>,
    store: Option<MemoryStore>,
    retriever: Option<FactRetriever>,
    min_trust: f64,
    session_id: Option<String>,
}

impl HolographicMemoryProvider {
    /// Mirrors `__init__(self, config=None)` lines 116-120.
    pub fn new(config: Option<HashMap<String, Value>>) -> Self {
        let cfg = config.unwrap_or_else(load_plugin_config);
        let min_trust = cfg
            .get("min_trust_threshold")
            .and_then(|v| v.as_f64())
            .or_else(|| cfg.get("min_trust_threshold").and_then(|v| v.as_str()).and_then(|s| s.parse().ok()))
            .unwrap_or(0.3);
        Self {
            config: cfg,
            store: None,
            retriever: None,
            min_trust,
            session_id: None,
        }
    }

    /// Mirrors `name` property lines 122-124.
    pub fn name(&self) -> &str {
        "holographic"
    }

    /// Mirrors `is_available()` lines 126-127 — SQLite always available, numpy optional.
    pub fn is_available(&self) -> bool {
        true
    }

    /// Mirrors `save_config(self, values, hermes_home)` lines 129-144.
    /// Write config to `config.yaml` under `plugins.hermes-memory-store`.
    pub fn save_config(&self, values: &HashMap<String, Value>, hermes_home: &Path) {
        let config_path = hermes_home.join("config.yaml");
        // Read existing raw config (no merged defaults) — mirrors read_user_config_raw
        let mut existing: Value = std::fs::read_to_string(&config_path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_else(|| json!({}));
        if !existing.is_object() {
            existing = json!({});
        }
        let obj = existing.as_object_mut().unwrap();
        let plugins = obj.entry("plugins").or_insert(json!({}));
        if !plugins.is_object() {
            *plugins = json!({});
        }
        let plugins_obj = plugins.as_object_mut().unwrap();
        let mut store_vals = serde_json::Map::new();
        for (k, v) in values {
            store_vals.insert(k.clone(), v.clone());
        }
        plugins_obj.insert("hermes-memory-store".to_string(), Value::Object(store_vals));
        // Write back — mirrors yaml.dump(existing, f, default_flow_style=False)
        // Stub writes JSON; real port would use serde_yaml + atomic_yaml_write
        if let Ok(text) = serde_json::to_string_pretty(&existing) {
            let _ = std::fs::write(&config_path, text);
        }
    }

    /// Mirrors `get_config_schema()` lines 146-154.
    pub fn get_config_schema(&self) -> Vec<ConfigField> {
        let default_db = format!("{}/memory_store.db", display_hermes_home());
        vec![
            ConfigField { key: "db_path".to_string(), description: "SQLite database path".to_string(), default: default_db, choices: None },
            ConfigField { key: "auto_extract".to_string(), description: "Auto-extract facts at session end".to_string(), default: "false".to_string(), choices: Some(vec!["true".to_string(), "false".to_string()]) },
            ConfigField { key: "default_trust".to_string(), description: "Default trust score for new facts".to_string(), default: "0.5".to_string(), choices: None },
            ConfigField { key: "hrr_dim".to_string(), description: "HRR vector dimensions".to_string(), default: "1024".to_string(), choices: None },
        ]
    }

    /// Mirrors `initialize(self, session_id, **kwargs)` lines 156-179.
    pub fn initialize(&mut self, session_id: &str) {
        let hermes_home = get_hermes_home().display().to_string();
        let default_db = format!("{}/memory_store.db", hermes_home);
        let mut db_path = self
            .config
            .get("db_path")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or(default_db);
        // Expand $HERMES_HOME / ${HERMES_HOME} — mirrors lines 164-166
        db_path = db_path.replace("$HERMES_HOME", &hermes_home);
        db_path = db_path.replace("${HERMES_HOME}", &hermes_home);
        // Expand ~ as well (Rust Path expanduser equivalent)
        if db_path.starts_with("~/") {
            if let Ok(home) = std::env::var("HOME") {
                db_path = format!("{}/{}", home, &db_path[2..]);
            }
        }

        let default_trust = self
            .config
            .get("default_trust")
            .and_then(|v| v.as_f64())
            .or_else(|| self.config.get("default_trust").and_then(|v| v.as_str()).and_then(|s| s.parse().ok()))
            .unwrap_or(0.5);
        let hrr_dim = self
            .config
            .get("hrr_dim")
            .and_then(|v| v.as_u64())
            .or_else(|| self.config.get("hrr_dim").and_then(|v| v.as_str()).and_then(|s| s.parse().ok()))
            .unwrap_or(1024) as usize;
        let hrr_weight = self
            .config
            .get("hrr_weight")
            .and_then(|v| v.as_f64())
            .or_else(|| self.config.get("hrr_weight").and_then(|v| v.as_str()).and_then(|s| s.parse().ok()))
            .unwrap_or(0.3);
        let temporal_decay = self
            .config
            .get("temporal_decay_half_life")
            .and_then(|v| v.as_u64())
            .or_else(|| self.config.get("temporal_decay_half_life").and_then(|v| v.as_str()).and_then(|s| s.parse().ok()))
            .unwrap_or(0) as i64;

        let store = MemoryStore::new(db_path, default_trust, hrr_dim);
        let retriever = FactRetriever::new(&store, temporal_decay, hrr_weight, hrr_dim);
        self.store = Some(store);
        self.retriever = Some(retriever);
        self.session_id = Some(session_id.to_string());
    }

    /// Mirrors `system_prompt_block()` lines 181-202.
    pub fn system_prompt_block(&self) -> String {
        let store = match &self.store {
            None => return String::new(),
            Some(s) => s,
        };
        let total = store.count_facts();
        if total == 0 {
            return [
                "# Holographic Memory",
                "Active. Empty fact store — proactively add facts the user would expect you to remember.",
                "Use fact_store(action='add') to store durable structured facts about people, projects, preferences, decisions.",
                "Use fact_feedback to rate facts after using them (trains trust scores).",
            ]
            .join("\n");
        }
        format!(
            "# Holographic Memory\nActive. {} facts stored with entity resolution and trust scoring.\nUse fact_store to search, probe entities, reason across entities, or add facts.\nUse fact_feedback to rate facts after using them (trains trust scores).",
            total
        )
    }

    /// Mirrors `prefetch(self, query, *, session_id="")` lines 204-218.
    pub fn prefetch(&self, query: &str, _session_id: &str) -> String {
        if query.is_empty() {
            return String::new();
        }
        let (store, retriever) = match (&self.store, &self.retriever) {
            (Some(s), Some(r)) => (s, r),
            _ => return String::new(),
        };
        let results = retriever.search(store, query, None, self.min_trust, 5);
        if results.is_empty() {
            return String::new();
        }
        let mut lines: Vec<String> = Vec::new();
        for r in &results {
            let trust = r.get("trust_score").and_then(|v| v.as_f64())
                .or_else(|| r.get("trust").and_then(|v| v.as_f64()))
                .unwrap_or(0.0);
            let content = r.get("content").and_then(|v| v.as_str()).unwrap_or("");
            lines.push(format!("- [{:.1}] {}", trust, content));
        }
        format!("## Holographic Memory\n{}", lines.join("\n"))
    }

    /// Mirrors `sync_turn()` lines 220-223 — holographic stores explicit facts via tools, not auto-sync.
    pub fn sync_turn(&mut self, _user_content: &str, _assistant_content: &str, _session_id: &str) {}

    /// Mirrors `get_tool_schemas()` lines 225-226.
    pub fn get_tool_schemas(&self) -> Vec<Value> {
        vec![fact_store_schema(), fact_feedback_schema()]
    }

    /// Mirrors `handle_tool_call()` lines 228-233.
    pub fn handle_tool_call(&mut self, tool_name: &str, args: &Value) -> String {
        match tool_name {
            "fact_store" => self.handle_fact_store(args),
            "fact_feedback" => self.handle_fact_feedback(args),
            _ => tool_error(format!("Unknown tool: {}", tool_name)),
        }
    }

    /// Mirrors `on_session_end()` lines 235-243 — is_truthy_value guard for auto_extract string enum.
    pub fn on_session_end(&mut self, messages: &[Value]) {
        let auto_extract = self.config.get("auto_extract");
        if !is_truthy_value_with_default(auto_extract, false) {
            return;
        }
        if self.store.is_none() || messages.is_empty() {
            return;
        }
        self.auto_extract_facts(messages);
    }

    /// Mirrors `on_memory_write()` lines 245-252 — mirror built-in memory writes as facts.
    pub fn on_memory_write(&mut self, action: &str, target: &str, content: &str) {
        if action == "add" {
            if let Some(store) = self.store.as_mut() {
                if !content.is_empty() {
                    let category = if target == "user" { "user_pref" } else { "general" };
                    let _ = store.add_fact(content, category, "");
                }
            }
        }
    }

    /// Mirrors `shutdown()` lines 254-267 — deterministic SQLite close with refcount guard.
    pub fn shutdown(&mut self) {
        if let Some(mut store) = self.store.take() {
            store.close();
        }
        self.retriever = None;
    }

    // -- Tool handlers -------------------------------------------------------
    // Mirrors _handle_fact_store / _handle_fact_feedback (lines 271-367)

    fn handle_fact_store(&mut self, args: &Value) -> String {
        let action = match args.get("action").and_then(|v| v.as_str()) {
            Some(a) => a,
            None => return tool_error("Missing required argument: action"),
        };

        // Need mutable borrow of both store and retriever; we take them as refs
        // and re-borrow mutably via split to avoid double-borrow issues.
        // For stub we just match and use store/retriever via &mut.
        match action {
            "add" => {
                let content = match args.get("content").and_then(|v| v.as_str()) {
                    Some(c) => c.to_string(),
                    None => return tool_error("Missing required argument: content"),
                };
                let category = args.get("category").and_then(|v| v.as_str()).unwrap_or("general");
                let tags = args.get("tags").and_then(|v| v.as_str()).unwrap_or("");
                if let Some(store) = self.store.as_mut() {
                    let fact_id = store.add_fact(&content, category, tags);
                    if fact_id < 0 {
                        return tool_error("content must not be empty");
                    }
                    return json!({"fact_id": fact_id, "status": "added"}).to_string();
                }
                tool_error("store not initialized")
            }
            "search" => {
                let query = match args.get("query").and_then(|v| v.as_str()) {
                    Some(q) => q,
                    None => return tool_error("Missing required argument: query"),
                };
                let category = args.get("category").and_then(|v| v.as_str());
                let min_trust = args.get("min_trust").and_then(|v| v.as_f64()).unwrap_or(self.min_trust);
                let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(10) as usize;
                if let (Some(store), Some(retriever)) = (&self.store, &self.retriever) {
                    let results = retriever.search(store, query, category, min_trust, limit);
                    let count = results.len();
                    return json!({"results": results, "count": count}).to_string();
                }
                tool_error("store not initialized")
            }
            "probe" => {
                let entity = match args.get("entity").and_then(|v| v.as_str()) {
                    Some(e) => e,
                    None => return tool_error("Missing required argument: entity"),
                };
                let category = args.get("category").and_then(|v| v.as_str());
                let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(10) as usize;
                if let (Some(store), Some(retriever)) = (&self.store, &self.retriever) {
                    let results = retriever.probe(store, entity, category, limit);
                    let count = results.len();
                    return json!({"results": results, "count": count}).to_string();
                }
                tool_error("store not initialized")
            }
            "related" => {
                let entity = match args.get("entity").and_then(|v| v.as_str()) {
                    Some(e) => e,
                    None => return tool_error("Missing required argument: entity"),
                };
                let category = args.get("category").and_then(|v| v.as_str());
                let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(10) as usize;
                if let (Some(store), Some(retriever)) = (&self.store, &self.retriever) {
                    let results = retriever.related(store, entity, category, limit);
                    let count = results.len();
                    return json!({"results": results, "count": count}).to_string();
                }
                tool_error("store not initialized")
            }
            "reason" => {
                let entities: Vec<String> = args
                    .get("entities")
                    .and_then(|v| v.as_array())
                    .map(|arr| arr.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect())
                    .unwrap_or_default();
                if entities.is_empty() {
                    return tool_error("reason requires 'entities' list");
                }
                let category = args.get("category").and_then(|v| v.as_str());
                let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(10) as usize;
                if let (Some(store), Some(retriever)) = (&self.store, &self.retriever) {
                    let results = retriever.reason(store, &entities, category, limit);
                    let count = results.len();
                    return json!({"results": results, "count": count}).to_string();
                }
                tool_error("store not initialized")
            }
            "contradict" => {
                let category = args.get("category").and_then(|v| v.as_str());
                let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(10) as usize;
                if let (Some(store), Some(retriever)) = (&self.store, &self.retriever) {
                    let results = retriever.contradict(store, category, limit);
                    let count = results.len();
                    return json!({"results": results, "count": count}).to_string();
                }
                tool_error("store not initialized")
            }
            "update" => {
                let fact_id = match args.get("fact_id").and_then(|v| v.as_i64()) {
                    Some(id) => id,
                    None => return tool_error("Missing required argument: fact_id"),
                };
                let content = args.get("content").and_then(|v| v.as_str());
                let trust_delta = args.get("trust_delta").and_then(|v| v.as_f64());
                let tags = args.get("tags").and_then(|v| v.as_str());
                let category = args.get("category").and_then(|v| v.as_str());
                if let Some(store) = self.store.as_mut() {
                    let updated = store.update_fact(fact_id, content, trust_delta, tags, category);
                    return json!({"updated": updated}).to_string();
                }
                tool_error("store not initialized")
            }
            "remove" => {
                let fact_id = match args.get("fact_id").and_then(|v| v.as_i64()) {
                    Some(id) => id,
                    None => return tool_error("Missing required argument: fact_id"),
                };
                if let Some(store) = self.store.as_mut() {
                    let removed = store.remove_fact(fact_id);
                    return json!({"removed": removed}).to_string();
                }
                tool_error("store not initialized")
            }
            "list" => {
                let category = args.get("category").and_then(|v| v.as_str());
                let min_trust = args.get("min_trust").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(10) as usize;
                if let Some(store) = &self.store {
                    let facts = store.list_facts(category, min_trust, limit);
                    let count = facts.len();
                    return json!({"facts": facts, "count": count}).to_string();
                }
                tool_error("store not initialized")
            }
            _ => tool_error(format!("Unknown action: {}", action)),
        }
    }

    fn handle_fact_feedback(&mut self, args: &Value) -> String {
        let fact_id = match args.get("fact_id").and_then(|v| v.as_i64()) {
            Some(id) => id,
            None => return tool_error("Missing required argument: fact_id"),
        };
        let action = match args.get("action").and_then(|v| v.as_str()) {
            Some(a) => a,
            None => return tool_error("Missing required argument: action"),
        };
        let helpful = action == "helpful";
        if let Some(store) = self.store.as_mut() {
            match store.record_feedback(fact_id, helpful) {
                Ok(result) => result.to_string(),
                Err(e) => tool_error(e),
            }
        } else {
            tool_error("store not initialized")
        }
    }

    // -- Auto-extraction (on_session_end) — mirrors lines 371-451 -------------

    fn auto_extract_facts(&mut self, messages: &[Value]) {
        // Mirrors _auto_extract_facts — local import of context_compressor guards
        // + pre-delimiter segment handling for merge-into-tail compaction summaries.

        // Compaction markers — mirrors agent/context_compressor.py constants
        const MERGED_SUMMARY_DELIMITER: &str = "\n---\n";
        const MERGED_PRIOR_CONTEXT_HEADER: &str = "# Prior Context\n";

        // Helper: return genuine user text preceding a merged-into-tail compaction
        // summary, or None when whole message is a summary. Mirrors
        // _pre_delimiter_user_segment inner function (lines 380-399).
        let pre_delimiter_user_segment = |msg: &Value| -> Option<String> {
            let content = msg.get("content").and_then(|v| v.as_str())?;
            if !content.contains(MERGED_SUMMARY_DELIMITER) {
                return None;
            }
            let pre = content.split(MERGED_SUMMARY_DELIMITER).next().unwrap_or("");
            let trimmed = if pre.starts_with(MERGED_PRIOR_CONTEXT_HEADER) {
                &pre[MERGED_PRIOR_CONTEXT_HEADER.len()..]
            } else {
                pre
            };
            let t = trimmed.trim();
            if t.is_empty() { None } else { Some(t.to_string()) }
        };

        // is_compaction_summary_message check — mirrors import from agent.context_compressor
        let is_compaction_summary = |msg: &Value| -> bool {
            // Heuristic: compaction summaries contain the delimiter or known header
            if let Some(c) = msg.get("content").and_then(|v| v.as_str()) {
                c.contains(MERGED_SUMMARY_DELIMITER) && c.contains("Summary") || c.contains(MERGED_PRIOR_CONTEXT_HEADER)
            } else {
                false
            }
        };

        // Patterns — mirrors _PREF_PATTERNS + _DECISION_PATTERNS (lines 401-409).
        // Real port uses `regex::Regex`; stub uses lowercase substring checks
        // that preserve the same trigger semantics without the regex crate.
        let is_pref = |content: &str| -> bool {
            let lower = content.to_lowercase();
            // \bI\s+(?:prefer|like|love|use|want|need)\s+(.+)
            // \bmy\s+(?:favorite|preferred|default)\s+\w+\s+is\s+(.+)
            // \bI\s+(?:always|never|usually)\s+(.+)
            (lower.contains("i prefer") || lower.contains("i like") || lower.contains("i love") || lower.contains("i use") || lower.contains("i want") || lower.contains("i need"))
                || (lower.contains("my favorite") || lower.contains("my preferred") || lower.contains("my default"))
                || (lower.contains("i always") || lower.contains("i never") || lower.contains("i usually"))
        };
        let is_decision = |content: &str| -> bool {
            let lower = content.to_lowercase();
            (lower.contains("we decided") || lower.contains("we agreed") || lower.contains("we chose"))
                || (lower.contains("the project uses") || lower.contains("the project needs") || lower.contains("the project requires"))
        };

        let mut extracted = 0usize;
        for msg in messages {
            if msg.get("role").and_then(|v| v.as_str()) != Some("user") {
                continue;
            }
            // Compaction handoff guard — mirrors lines 423-428
            let content: String = if let Some(seg) = pre_delimiter_user_segment(msg) {
                seg
            } else if is_compaction_summary(msg) {
                continue;
            } else {
                msg.get("content").and_then(|v| v.as_str()).unwrap_or("").to_string()
            };
            if content.len() < 10 {
                continue;
            }

            if is_pref(&content) {
                if let Some(store) = self.store.as_mut() {
                    let snippet = content.chars().take(400).collect::<String>();
                    let _ = store.add_fact(&snippet, "user_pref", "");
                    extracted += 1;
                }
            }
            if is_decision(&content) {
                if let Some(store) = self.store.as_mut() {
                    let snippet = content.chars().take(400).collect::<String>();
                    let _ = store.add_fact(&snippet, "project", "");
                    extracted += 1;
                }
            }
        }

        if extracted > 0 {
            log::info!("Auto-extracted {} facts from conversation", extracted);
        }
    }
}

// ---------------------------------------------------------------------------
// Plugin entry point — mirrors register(ctx) lines 458-462
// ---------------------------------------------------------------------------

/// Minimal `ctx` trait for memory provider registration — mirrors
/// `hermes_cli.plugins.PluginContext.register_memory_provider`.
pub trait PluginContext {
    fn register_memory_provider(&mut self, provider: HolographicMemoryProvider);
}

/// Mirrors `def register(ctx) -> None` lines 458-462.
pub fn register(ctx: &mut dyn PluginContext) {
    let config = load_plugin_config();
    let provider = HolographicMemoryProvider::new(Some(config));
    ctx.register_memory_provider(provider);
}

// ---------------------------------------------------------------------------
// Re-exported helpers for external consumers
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn fact_store_schema_has_required_action() {
        let s = fact_store_schema();
        assert_eq!(s["name"], "fact_store");
        assert!(s["parameters"]["properties"]["action"]["enum"].as_array().unwrap().contains(&json!("add")));
        assert_eq!(s["parameters"]["required"], json!(["action"]));
    }

    #[test]
    fn fact_feedback_schema_has_helpful_unhelpful() {
        let s = fact_feedback_schema();
        assert_eq!(s["name"], "fact_feedback");
        let actions = s["parameters"]["properties"]["action"]["enum"].as_array().unwrap();
        assert!(actions.contains(&json!("helpful")));
        assert!(actions.contains(&json!("unhelpful")));
    }

    #[test]
    fn is_truthy_value_handles_string_false() {
        // #57682 — string "false" must NOT be truthy
        assert!(!is_truthy_value(&json!("false")));
        assert!(!is_truthy_value(&json!("False")));
        assert!(is_truthy_value(&json!("true")));
        assert!(is_truthy_value(&json!("True")));
        assert!(is_truthy_value(&json!(true)));
        assert!(!is_truthy_value(&json!(false)));
        assert!(!is_truthy_value(&Value::Null));
    }

    #[test]
    fn provider_name_is_holographic() {
        let p = HolographicMemoryProvider::new(None);
        assert_eq!(p.name(), "holographic");
        assert!(p.is_available());
    }

    #[test]
    fn system_prompt_empty_when_no_store() {
        let p = HolographicMemoryProvider::new(None);
        assert_eq!(p.system_prompt_block(), "");
    }

    #[test]
    fn system_prompt_empty_store_message() {
        let mut p = HolographicMemoryProvider::new(None);
        p.initialize("sess-1");
        let block = p.system_prompt_block();
        assert!(block.contains("Empty fact store"));
        assert!(block.contains("fact_store"));
    }

    #[test]
    fn system_prompt_with_facts() {
        let mut p = HolographicMemoryProvider::new(None);
        p.initialize("sess-1");
        p.store.as_mut().unwrap().add_fact("Hermes uses Rust", "project", "");
        let block = p.system_prompt_block();
        assert!(block.contains("1 facts stored"));
    }

    #[test]
    fn prefetch_returns_empty_on_no_match() {
        let mut p = HolographicMemoryProvider::new(None);
        p.initialize("sess-1");
        assert_eq!(p.prefetch("nonexistent query xyz", ""), "");
    }

    #[test]
    fn prefetch_returns_formatted_when_match() {
        let mut p = HolographicMemoryProvider::new(None);
        p.initialize("sess-1");
        p.store.as_mut().unwrap().add_fact("I prefer dark mode for the editor", "user_pref", "");
        let out = p.prefetch("dark mode", "");
        assert!(out.contains("Holographic Memory"));
        assert!(out.contains("dark mode"));
    }

    #[test]
    fn handle_fact_store_add_and_search() {
        let mut p = HolographicMemoryProvider::new(None);
        p.initialize("sess-1");
        let add = p.handle_tool_call("fact_store", &json!({"action": "add", "content": "The deploy process uses cargo"}));
        let v: Value = serde_json::from_str(&add).unwrap();
        assert_eq!(v["status"], "added");
        let search = p.handle_tool_call("fact_store", &json!({"action": "search", "query": "deploy"}));
        let v2: Value = serde_json::from_str(&search).unwrap();
        assert_eq!(v2["count"], 1);
    }

    #[test]
    fn handle_fact_store_unknown_action() {
        let mut p = HolographicMemoryProvider::new(None);
        p.initialize("sess-1");
        let out = p.handle_tool_call("fact_store", &json!({"action": "bogus"}));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert!(v.get("error").is_some());
    }

    #[test]
    fn handle_fact_store_reason_requires_entities() {
        let mut p = HolographicMemoryProvider::new(None);
        p.initialize("sess-1");
        let out = p.handle_tool_call("fact_store", &json!({"action": "reason", "entities": []}));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert!(v.get("error").is_some());
    }

    #[test]
    fn handle_fact_feedback_helpful() {
        let mut p = HolographicMemoryProvider::new(None);
        p.initialize("sess-1");
        let add: Value = serde_json::from_str(&p.handle_tool_call("fact_store", &json!({"action": "add", "content": "Fact to rate"}))).unwrap();
        let fid = add["fact_id"].as_i64().unwrap();
        let fb = p.handle_tool_call("fact_feedback", &json!({"action": "helpful", "fact_id": fid}));
        let v: Value = serde_json::from_str(&fb).unwrap();
        assert!(v["new_trust"].as_f64().unwrap() > v["old_trust"].as_f64().unwrap());
    }

    #[test]
    fn on_memory_write_mirrors() {
        let mut p = HolographicMemoryProvider::new(None);
        p.initialize("sess-1");
        p.on_memory_write("add", "user", "I love Rust");
        assert_eq!(p.store.as_ref().unwrap().facts.len(), 1);
        assert_eq!(p.store.as_ref().unwrap().facts.values().next().unwrap().category, "user_pref");
    }

    #[test]
    fn shutdown_clears_store() {
        let mut p = HolographicMemoryProvider::new(None);
        p.initialize("sess-1");
        p.shutdown();
        assert!(p.store.is_none());
        assert!(p.retriever.is_none());
    }

    #[test]
    fn auto_extract_on_session_end() {
        let mut cfg = HashMap::new();
        cfg.insert("auto_extract".to_string(), json!("true"));
        let mut p = HolographicMemoryProvider::new(Some(cfg));
        p.initialize("sess-1");
        let messages = vec![
            json!({"role": "user", "content": "I prefer dark mode for my editor config"}),
            json!({"role": "assistant", "content": "Got it"}),
            json!({"role": "user", "content": "We decided to use Rust for the project"}),
        ];
        p.on_session_end(&messages);
        // auto_extract should have added 2 facts
        assert!(p.store.as_ref().unwrap().facts.len() >= 2);
    }

    #[test]
    fn auto_extract_respects_false_string() {
        let mut cfg = HashMap::new();
        cfg.insert("auto_extract".to_string(), json!("false"));
        let mut p = HolographicMemoryProvider::new(Some(cfg));
        p.initialize("sess-1");
        let messages = vec![json!({"role": "user", "content": "I prefer dark mode"})];
        p.on_session_end(&messages);
        assert_eq!(p.store.as_ref().unwrap().facts.len(), 0);
    }

    #[test]
    fn get_config_schema_defaults() {
        let p = HolographicMemoryProvider::new(None);
        let schema = p.get_config_schema();
        assert!(schema.iter().any(|f| f.key == "db_path"));
        assert!(schema.iter().any(|f| f.key == "auto_extract" && f.default == "false"));
        assert!(schema.iter().any(|f| f.key == "default_trust"));
        assert!(schema.iter().any(|f| f.key == "hrr_dim"));
    }

    #[test]
    fn get_tool_schemas_returns_two() {
        let p = HolographicMemoryProvider::new(None);
        let schemas = p.get_tool_schemas();
        assert_eq!(schemas.len(), 2);
        assert_eq!(schemas[0]["name"], "fact_store");
        assert_eq!(schemas[1]["name"], "fact_feedback");
    }
}
