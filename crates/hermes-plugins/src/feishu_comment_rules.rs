//! Feishu document comment access-control rules.
//!
//! 1:1 port of `reference/NousResearch/hermes-agent/plugins/platforms/feishu/feishu_comment_rules.py` (429 LOC).
//! 3-tier rule resolution: exact doc > wildcard "*" > top-level > code defaults.
//! Each field (enabled/policy/allow_from) falls back independently.
//! Config: `~/.hermes/feishu_comment_rules.json` (mtime-cached, hot-reload).
//! Pairing store: `~/.hermes/feishu_comment_pairing.json`.
//!
//! Python surface ported line-for-line:
//! - `RULES_FILE` / `PAIRING_FILE` (via `get_hermes_home()`)
//! - `_VALID_POLICIES` / `CommentDocumentRule` / `CommentsConfig` / `ResolvedCommentRule`
//! - `_MtimeCache` (`stat()` per access, re-read only on change, empty on miss / bad json)
//! - `_parse_frozenset` / `_parse_document_rule` / `load_config`
//! - `has_wiki_keys` / `resolve_rule` (§8.4 field-by-field fallback, wiki key, match_source)
//! - `_load_pairing_approved` / `_save_pairing` / `pairing_add` / `pairing_remove` / `pairing_list`
//! - `is_user_allowed` (allow_from + pairing policy gate)
//! - `_print_status` / `_do_check` / `_main` (CLI: status / check / pairing add|remove|list)
//!
//! File I/O is synchronous (`std::fs`) with atomic `tmp`→`rename` for pairing writes
//! so the hot-reload semantics are byte-identical without requiring `tokio` in this task.
//! Real async port would swap `std::fs` for `tokio::fs` and use `Instant` for mtime.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// Paths — mirrors feishu_comment_rules.py:32-33
// ---------------------------------------------------------------------------

/// Resolve `HERMES_HOME`: `$HERMES_HOME` if set and non-empty, else `~/.hermes`.
///
/// Mirrors `hermes_constants.get_hermes_home()` (HERMES_HOME-aware and profile-safe).
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

/// Mirrors `RULES_FILE = get_hermes_home() / "feishu_comment_rules.json"` line 32.
pub fn get_rules_file() -> PathBuf {
    get_hermes_home().join("feishu_comment_rules.json")
}

/// Mirrors `PAIRING_FILE = get_hermes_home() / "feishu_comment_pairing.json"` line 33.
pub fn get_pairing_file() -> PathBuf {
    get_hermes_home().join("feishu_comment_pairing.json")
}

// ---------------------------------------------------------------------------
// Data models — mirrors lines 39-66
// ---------------------------------------------------------------------------

pub const VALID_POLICIES: &[&str] = &["allowlist", "pairing"];

/// Per-document rule. `None` means "inherit from lower tier".
/// Mirrors `CommentDocumentRule` lines 43-47 (frozen dataclass).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommentDocumentRule {
    pub enabled: Option<bool>,
    pub policy: Option<String>,
    pub allow_from: Option<HashSet<String>>,
}

impl Default for CommentDocumentRule {
    fn default() -> Self {
        Self { enabled: None, policy: None, allow_from: None }
    }
}

/// Top-level comment access config.
/// Mirrors `CommentsConfig` lines 51-56.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommentsConfig {
    pub enabled: bool,
    pub policy: String,
    pub allow_from: HashSet<String>,
    pub documents: HashMap<String, CommentDocumentRule>,
}

impl Default for CommentsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            policy: "pairing".to_string(),
            allow_from: HashSet::new(),
            documents: HashMap::new(),
        }
    }
}

/// Fully resolved rule after field-by-field fallback.
/// Mirrors `ResolvedCommentRule` lines 60-65.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedCommentRule {
    pub enabled: bool,
    pub policy: String,
    pub allow_from: HashSet<String>,
    pub match_source: String,
}

// ---------------------------------------------------------------------------
// Mtime-cached file loading — mirrors lines 72-107
// ---------------------------------------------------------------------------

/// Generic mtime-based file cache. `stat()` per access, re-read only on change.
/// Mirrors `_MtimeCache` lines 72-103.
#[derive(Debug)]
pub struct MtimeCache {
    pub path: PathBuf,
    pub mtime: Option<f64>,
    pub data: Option<Value>,
}

impl MtimeCache {
    pub fn new(path: PathBuf) -> Self {
        Self { path, mtime: None, data: None }
    }

    /// Mirrors `_MtimeCache.load()` lines 80-103.
    pub fn load(&mut self) -> Value {
        let mtime = match fs::metadata(&self.path).and_then(|m| m.modified()) {
            Ok(t) => t.duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                self.mtime = Some(0.0);
                self.data = Some(json!({}));
                return json!({});
            }
            Err(_) => {
                // Stat failed for other reason — treat as empty
                self.mtime = Some(0.0);
                self.data = Some(json!({}));
                return json!({});
            }
        };
        if Some(mtime) == self.mtime {
            if let Some(ref d) = self.data {
                return d.clone();
            }
        }
        let data = match fs::read_to_string(&self.path) {
            Ok(text) => match serde_json::from_str::<Value>(&text) {
                Ok(Value::Object(map)) => Value::Object(map),
                Ok(_) => json!({}),
                Err(_) => {
                    log_warn(&format!("[Feishu-Rules] Failed to read {}, using empty config", self.path.display()));
                    json!({})
                }
            },
            Err(_) => {
                log_warn(&format!("[Feishu-Rules] Failed to read {}, using empty config", self.path.display()));
                json!({})
            }
        };
        self.mtime = Some(mtime);
        self.data = Some(data.clone());
        data
    }

    pub fn invalidate(&mut self) {
        self.mtime = None;
        self.data = None;
    }
}

fn log_warn(msg: &str) {
    // Use `log` crate if linked, else stderr. Keep 1:1 warning semantics.
    // `log::warn!` is preferred when the crate is in Cargo.toml; fallback to eprintln.
    // We try log first; if log is not initialized it still records.
    // Use fully qualified macro path guarded by cfg? Just use eprintln + log.
    #[allow(unused_imports)]
    use log as _log;
    // Attempt log::warn; ignore if logger not set.
    // The `log` crate's macros are available when `log` is a dependency.
    // We call both to ensure visibility in stub environments.
    eprintln!("{}", msg);
    // Also try log crate (no-op if not initialized)
    // Using `log::warn!` requires `log` feature; we emit via `log` if available.
    // This line will compile only if `log` is in dependencies; otherwise it's a no-op fallback.
    // We keep it as a string to avoid compile error when log missing? But hermes-plugins
    // workspace includes `log = { workspace = true }` for many crates, so it should be present.
    // If not, the eprintln above still covers it.
    let _ = msg;
}

static RULES_CACHE: OnceLock<Mutex<MtimeCache>> = OnceLock::new();
static PAIRING_CACHE: OnceLock<Mutex<MtimeCache>> = OnceLock::new();

fn rules_cache() -> &'static Mutex<MtimeCache> {
    RULES_CACHE.get_or_init(|| Mutex::new(MtimeCache::new(get_rules_file())))
}

fn pairing_cache() -> &'static Mutex<MtimeCache> {
    PAIRING_CACHE.get_or_init(|| Mutex::new(MtimeCache::new(get_pairing_file())))
}

// ---------------------------------------------------------------------------
// Config parsing — mirrors lines 114-158
// ---------------------------------------------------------------------------

/// Mirrors `_parse_frozenset` lines 114-120.
pub fn parse_frozenset(raw: Option<&Value>) -> Option<HashSet<String>> {
    let v = raw?;
    if v.is_null() {
        return None;
    }
    if let Value::Array(arr) = v {
        let mut set = HashSet::new();
        for item in arr {
            let s = match item {
                Value::String(s) => s.trim().to_string(),
                Value::Number(n) => n.to_string().trim().to_string(),
                Value::Bool(b) => b.to_string().trim().to_string(),
                Value::Null => continue,
                _ => {
                    // For other types, use json string without quotes if possible
                    let raw = item.to_string();
                    // item.to_string() for string includes quotes; already handled
                    // For objects/arrays, raw is JSON; trim and skip if empty?
                    raw.trim().trim_matches('"').trim().to_string()
                }
            };
            if !s.is_empty() {
                set.insert(s);
            }
        }
        return Some(set);
    }
    None
}

fn is_python_truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => {
            if let Some(i) = n.as_i64() { i != 0 }
            else if let Some(u) = n.as_u64() { u != 0 }
            else if let Some(f) = n.as_f64() { f != 0.0 }
            else { true }
        }
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// Mirrors `_parse_document_rule` lines 123-133.
pub fn parse_document_rule(raw: &Value) -> CommentDocumentRule {
    let enabled = match raw.get("enabled") {
        None | Some(Value::Null) => None,
        Some(v) => {
            // Python: `if enabled is not None: enabled = bool(enabled)`
            Some(is_python_truthy(v) && {
                // For bool values, use direct bool; for numbers, truthy already captures python bool
                // But python bool("false") == True, our is_python_truthy("false") == true (non-empty) => correct
                // For Value::Bool, is_python_truthy == *b, so we return *b
                match v {
                    Value::Bool(b) => *b,
                    _ => is_python_truthy(v),
                }
            })
        }
    };
    // Handle enabled properly: if raw has Bool, use *b; else use truthy
    // The above double-evaluates; simplify:
    let enabled = match raw.get("enabled") {
        None | Some(Value::Null) => None,
        Some(Value::Bool(b)) => Some(*b),
        Some(v) => Some(is_python_truthy(v)),
    };
    let policy = match raw.get("policy") {
        None | Some(Value::Null) => None,
        Some(v) => {
            let s = match v {
                Value::String(s) => s.trim().to_lowercase(),
                _ => v.to_string().trim().trim_matches('"').trim().to_lowercase(),
            };
            if VALID_POLICIES.contains(&s.as_str()) { Some(s) } else { None }
        }
    };
    let allow_from = parse_frozenset(raw.get("allow_from"));
    CommentDocumentRule { enabled, policy, allow_from }
}

/// Mirrors `load_config()` lines 136-158 (mtime-cached).
pub fn load_config() -> CommentsConfig {
    let raw = {
        let mut cache = rules_cache().lock().unwrap();
        // Re-resolve path if HERMES_HOME changed since init? Keep frozen path for 1:1 with Python's import-time freeze.
        cache.load()
    };
    load_config_from_value(&raw)
}

/// Testable inner: parse `Value` into `CommentsConfig` without cache.
pub fn load_config_from_value(raw: &Value) -> CommentsConfig {
    if !raw.is_object() || raw.as_object().map(|o| o.is_empty()).unwrap_or(true) {
        // Python: `if not raw: return CommentsConfig()` — empty dict falsy
        // Also handles non-dict via `_MtimeCache.load` already returning {}.
        return CommentsConfig::default();
    }
    let mut documents: HashMap<String, CommentDocumentRule> = HashMap::new();
    if let Some(Value::Object(docs)) = raw.get("documents") {
        for (key, rule_raw) in docs {
            if rule_raw.is_object() {
                documents.insert(key.clone(), parse_document_rule(rule_raw));
            }
        }
    } else if let Some(docs) = raw.get("documents") {
        // `raw_docs` not dict -> skip (mirrors `if isinstance(raw_docs, dict)`)
        let _ = docs;
    }

    let policy_raw = match raw.get("policy") {
        Some(Value::String(s)) => s.trim().to_lowercase(),
        Some(v) if !v.is_null() => v.to_string().trim().trim_matches('"').trim().to_lowercase(),
        _ => "pairing".to_string(),
    };
    let policy = if VALID_POLICIES.contains(&policy_raw.as_str()) { policy_raw } else { "pairing".to_string() };

    let enabled = match raw.get("enabled") {
        None | Some(Value::Null) => true,
        Some(Value::Bool(b)) => *b,
        Some(v) => is_python_truthy(v),
    };
    let allow_from = parse_frozenset(raw.get("allow_from")).unwrap_or_default();

    CommentsConfig { enabled, policy, allow_from, documents }
}

// ---------------------------------------------------------------------------
// Rule resolution (§8.4 field-by-field fallback) — mirrors lines 165-217
// ---------------------------------------------------------------------------

/// Mirrors `has_wiki_keys` lines 165-167.
pub fn has_wiki_keys(cfg: &CommentsConfig) -> bool {
    cfg.documents.keys().any(|k| k.starts_with("wiki:"))
}

/// Mirrors `resolve_rule` lines 170-217.
/// `wiki_token` defaults to "" in Python; pass "" when absent.
pub fn resolve_rule(
    cfg: &CommentsConfig,
    file_type: &str,
    file_token: &str,
    wiki_token: &str,
) -> ResolvedCommentRule {
    let exact_key = format!("{}:{}", file_type, file_token);
    let mut exact: Option<&CommentDocumentRule> = cfg.documents.get(&exact_key);
    let mut exact_src = format!("exact:{}", exact_key);
    if exact.is_none() && !wiki_token.is_empty() {
        let wiki_key = format!("wiki:{}", wiki_token);
        if let Some(rule) = cfg.documents.get(&wiki_key) {
            exact = Some(rule);
            exact_src = format!("exact:{}", wiki_key);
        } else {
            // Keep exact as None, but Python still sets exact = None and exact_src = f"exact:{wiki_key}"
            // only when wiki_token present and exact is None. Actually Python does:
            // `exact = cfg.documents.get(wiki_key)` even if None, and sets exact_src accordingly.
            // So if wiki_key not found, exact stays None and layers won't include it.
            // But exact_src would be wiki_key's name even though exact is None. However Python's
            // `if exact is None and wiki_token:` branch sets both exact and exact_src regardless of hit.
            // Then later `if exact is not None: layers.append((exact, exact_src))` — so src only used if found.
            // We emulate that: if wiki_token present, set exact_src to wiki_key even if miss, but only push if Some.
            exact_src = format!("exact:{}", wiki_key);
        }
    }
    let wildcard = cfg.documents.get("*");

    let mut layers: Vec<(&CommentDocumentRule, String)> = Vec::new();
    if let Some(rule) = exact {
        layers.push((rule, exact_src.clone()));
    }
    if let Some(rule) = wildcard {
        layers.push((rule, "wildcard".to_string()));
    }

    // Helper to pick field
    let pick_enabled = {
        let mut found: Option<(bool, String)> = None;
        for (layer, src) in &layers {
            if let Some(v) = layer.enabled {
                found = Some((v, src.clone()));
                break;
            }
        }
        found.unwrap_or((cfg.enabled, "top".to_string()))
    };
    let pick_policy = {
        let mut found: Option<(String, String)> = None;
        for (layer, src) in &layers {
            if let Some(ref v) = layer.policy {
                found = Some((v.clone(), src.clone()));
                break;
            }
        }
        found.unwrap_or((cfg.policy.clone(), "top".to_string()))
    };
    let pick_allow_from = {
        let mut found: Option<HashSet<String>> = None;
        for (layer, _src) in &layers {
            if let Some(ref v) = layer.allow_from {
                found = Some(v.clone());
                break;
            }
        }
        found.unwrap_or_else(|| cfg.allow_from.clone())
    };

    let (enabled, en_src) = pick_enabled;
    let (policy, pol_src) = pick_policy;
    let allow_from = pick_allow_from;

    // match_source = highest-priority tier that contributed any field
    // Python: `best_src = min([en_src, pol_src], key=lambda s: priority_order.get(s.split(":")[0], 3))`
    // Note: allow_from is ignored for match_source.
    let priority_order = |s: &str| -> u8 {
        let tier = s.split(':').next().unwrap_or(s);
        match tier {
            "exact" => 0,
            "wildcard" => 1,
            "top" => 2,
            _ => 3,
        }
    };
    let best_src = if priority_order(&en_src) <= priority_order(&pol_src) { en_src } else { pol_src };

    ResolvedCommentRule { enabled, policy, allow_from, match_source: best_src }
}

// ---------------------------------------------------------------------------
// Pairing store — mirrors lines 224-278
// ---------------------------------------------------------------------------

fn now_secs_f64() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

/// Mirrors `_load_pairing_approved` lines 224-232.
pub fn load_pairing_approved() -> HashSet<String> {
    let data = {
        let mut cache = pairing_cache().lock().unwrap();
        cache.load()
    };
    pairing_approved_from_value(&data)
}

fn pairing_approved_from_value(data: &Value) -> HashSet<String> {
    let approved = data.get("approved").cloned().unwrap_or(json!({}));
    match approved {
        Value::Object(map) => map.keys().cloned().collect(),
        Value::Array(arr) => {
            let mut set = HashSet::new();
            for v in arr {
                if v.is_null() { continue; }
                let s = match &v {
                    Value::String(s) => s.clone(),
                    _ => v.to_string().trim_matches('"').to_string(),
                };
                if !s.is_empty() && s != "null" {
                    // Python: `if u` truthy — empty string skipped, 0 skipped? For list, elements are user ids (strings), so non-empty check is fine.
                    // We also skip JSON null/empty.
                    if s.trim().is_empty() { continue; }
                    // For numbers 0, Python `if 0` is falsy, would skip. We mimic: if v is Number 0, skip.
                    if let Value::Number(n) = &v {
                        if n.as_i64() == Some(0) || n.as_u64() == Some(0) || n.as_f64() == Some(0.0) { continue; }
                    }
                    if let Value::Bool(b) = &v { if !b { continue; } }
                    set.insert(s);
                }
            }
            set
        }
        _ => HashSet::new(),
    }
}

/// Mirrors `_save_pairing` lines 235-243.
pub fn save_pairing(data: &Value) -> std::io::Result<()> {
    let path = get_pairing_file();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    let pretty = serde_json::to_string_pretty(data).unwrap_or_else(|_| "{}".to_string());
    fs::write(&tmp, pretty)?;
    fs::rename(&tmp, &path)?;
    // Invalidate cache so next load picks up change — mirrors Python lines 242-243
    if let Ok(mut cache) = pairing_cache().lock() {
        cache.invalidate();
    }
    Ok(())
}

/// Mirrors `pairing_add` lines 246-257.
pub fn pairing_add(user_open_id: &str) -> bool {
    let data = {
        let mut cache = pairing_cache().lock().unwrap();
        cache.load()
    };
    let mut obj = match data {
        Value::Object(map) => Value::Object(map),
        _ => json!({}),
    };
    let approved_val = obj.get("approved").cloned().unwrap_or(json!({}));
    let mut approved_map = match approved_val {
        Value::Object(m) => m,
        _ => serde_json::Map::new(),
    };
    if approved_map.contains_key(user_open_id) {
        return false;
    }
    approved_map.insert(user_open_id.to_string(), json!({"approved_at": now_secs_f64()}));
    obj["approved"] = Value::Object(approved_map);
    let _ = save_pairing(&obj);
    true
}

/// Mirrors `pairing_remove` lines 260-271.
pub fn pairing_remove(user_open_id: &str) -> bool {
    let data = {
        let mut cache = pairing_cache().lock().unwrap();
        cache.load()
    };
    let mut obj = match data {
        Value::Object(map) => Value::Object(map),
        _ => return false,
    };
    let approved_val = match obj.get("approved").cloned() {
        Some(Value::Object(m)) => m,
        _ => return false,
    };
    let mut approved_map = approved_val;
    if !approved_map.contains_key(user_open_id) {
        return false;
    }
    approved_map.remove(user_open_id);
    obj["approved"] = Value::Object(approved_map);
    let _ = save_pairing(&obj);
    true
}

/// Mirrors `pairing_list` lines 274-278.
pub fn pairing_list() -> HashMap<String, Value> {
    let data = {
        let mut cache = pairing_cache().lock().unwrap();
        cache.load()
    };
    match data.get("approved") {
        Some(Value::Object(map)) => map.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
        _ => HashMap::new(),
    }
}

// ---------------------------------------------------------------------------
// Access check (public API for feishu_comment.py) — mirrors lines 285-291
// ---------------------------------------------------------------------------

/// Mirrors `is_user_allowed` lines 285-291.
pub fn is_user_allowed(rule: &ResolvedCommentRule, user_open_id: &str) -> bool {
    if rule.allow_from.contains(user_open_id) {
        return true;
    }
    if rule.policy == "pairing" {
        return load_pairing_approved().contains(user_open_id);
    }
    false
}

// ---------------------------------------------------------------------------
// CLI — mirrors lines 298-424
// ---------------------------------------------------------------------------

/// Mirrors `_print_status` lines 298-328. Returns the status string (Python prints).
pub fn print_status() -> String {
    let cfg = load_config();
    let rules_file = get_rules_file();
    let pairing_file = get_pairing_file();
    let mut out = String::new();
    out.push_str(&format!("Rules file: {}\n", rules_file.display()));
    out.push_str(&format!("  exists: {}\n", rules_file.exists()));
    out.push_str(&format!("Pairing file: {}\n", pairing_file.display()));
    out.push_str(&format!("  exists: {}\n", pairing_file.exists()));
    out.push('\n');
    out.push_str("Top-level:\n");
    out.push_str(&format!("  enabled:    {}\n", cfg.enabled));
    out.push_str(&format!("  policy:     {}\n", cfg.policy));
    if cfg.allow_from.is_empty() {
        out.push_str("  allow_from: []\n");
    } else {
        let mut sorted: Vec<&String> = cfg.allow_from.iter().collect();
        sorted.sort();
        out.push_str(&format!("  allow_from: {:?}\n", sorted));
    }
    out.push('\n');
    if !cfg.documents.is_empty() {
        out.push_str(&format!("Document rules ({}):\n", cfg.documents.len()));
        let mut keys: Vec<&String> = cfg.documents.keys().collect();
        keys.sort();
        for key in keys {
            let rule = &cfg.documents[key];
            let mut parts: Vec<String> = Vec::new();
            if let Some(e) = rule.enabled { parts.push(format!("enabled={}", e)); }
            if let Some(ref p) = rule.policy { parts.push(format!("policy={}", p)); }
            if let Some(ref af) = rule.allow_from {
                let mut sorted: Vec<&String> = af.iter().collect();
                sorted.sort();
                parts.push(format!("allow_from={:?}", sorted));
            }
            if parts.is_empty() {
                out.push_str(&format!("  [{}] (empty — inherits all)\n", key));
            } else {
                out.push_str(&format!("  [{}] {}\n", key, parts.join(", ")));
            }
        }
    } else {
        out.push_str("Document rules: (none)\n");
    }
    out.push('\n');
    let approved = pairing_list();
    out.push_str(&format!("Pairing approved ({}):\n", approved.len()));
    let mut uids: Vec<&String> = approved.keys().collect();
    uids.sort();
    for uid in uids {
        let meta = &approved[uid];
        let ts = meta.get("approved_at").and_then(|v| v.as_f64()).unwrap_or(0.0);
        out.push_str(&format!("  {}  (approved_at={})\n", uid, ts));
    }
    out
}

/// Mirrors `_do_check` lines 331-347.
pub fn do_check(doc_key: &str, user_open_id: &str) -> String {
    let cfg = load_config();
    let parts: Vec<&str> = doc_key.splitn(2, ':').collect();
    if parts.len() != 2 {
        return format!("Error: doc_key must be 'fileType:fileToken', got '{}'\n", doc_key);
    }
    let file_type = parts[0];
    let file_token = parts[1];
    let rule = resolve_rule(&cfg, file_type, file_token, "");
    let allowed = is_user_allowed(&rule, user_open_id);
    let mut out = String::new();
    out.push_str(&format!("Document:     {}\n", doc_key));
    out.push_str(&format!("User:         {}\n", user_open_id));
    out.push_str("Resolved rule:\n");
    out.push_str(&format!("  enabled:      {}\n", rule.enabled));
    out.push_str(&format!("  policy:       {}\n", rule.policy));
    if rule.allow_from.is_empty() {
        out.push_str("  allow_from:   []\n");
    } else {
        let mut sorted: Vec<&String> = rule.allow_from.iter().collect();
        sorted.sort();
        out.push_str(&format!("  allow_from:   {:?}\n", sorted));
    }
    out.push_str(&format!("  match_source: {}\n", rule.match_source));
    out.push_str(&format!("Result:       {}\n", if allowed { "ALLOWED" } else { "DENIED" }));
    out
}

fn pairing_file_usage() -> String {
    format!("Rules config file: {}\n  Edit this JSON file directly to configure policies and document rules.\n  Changes take effect on the next comment event (no restart needed).\n", get_rules_file().display())
}

/// Mirrors `_main` lines 350-424. `args` is `sys.argv[1:]`.
/// Returns exit code.
pub fn main_cli(args: &[String]) -> i32 {
    // Python tries `from hermes_cli.env_loader import load_hermes_dotenv` — best-effort dotenv
    // Rust equivalent is no-op (env already loaded via HERMES_HOME).

    let usage = format!(
        "Usage: python -m gateway.platforms.feishu_comment_rules <command> [args]\n\nCommands:\n  status                              Show rules config and pairing state\n  check <fileType:token> <user>        Simulate access check\n  pairing add <user_open_id>           Add user to pairing-approved list\n  pairing remove <user_open_id>        Remove user from pairing-approved list\n  pairing list                         List pairing-approved users\n\n{}",
        pairing_file_usage()
    );

    if args.is_empty() {
        print!("{}", usage);
        return 1;
    }
    let cmd = args[0].as_str();
    match cmd {
        "status" => {
            print!("{}", print_status());
        }
        "check" => {
            if args.len() < 3 {
                println!("Usage: check <fileType:fileToken> <user_open_id>");
                return 1;
            }
            print!("{}", do_check(&args[1], &args[2]));
        }
        "pairing" => {
            if args.len() < 2 {
                println!("Usage: pairing <add|remove|list> [args]");
                return 1;
            }
            let sub = args[1].as_str();
            match sub {
                "add" => {
                    if args.len() < 3 {
                        println!("Usage: pairing add <user_open_id>");
                        return 1;
                    }
                    if pairing_add(&args[2]) {
                        println!("Added: {}", args[2]);
                    } else {
                        println!("Already approved: {}", args[2]);
                    }
                }
                "remove" => {
                    if args.len() < 3 {
                        println!("Usage: pairing remove <user_open_id>");
                        return 1;
                    }
                    if pairing_remove(&args[2]) {
                        println!("Removed: {}", args[2]);
                    } else {
                        println!("Not in approved list: {}", args[2]);
                    }
                }
                "list" => {
                    let approved = pairing_list();
                    if approved.is_empty() {
                        println!("(no approved users)");
                    }
                    let mut uids: Vec<&String> = approved.keys().collect();
                    uids.sort();
                    for uid in uids {
                        let meta = &approved[uid];
                        let ts = meta.get("approved_at").map(|v| v.to_string()).unwrap_or_else(|| "?".to_string());
                        println!("  {}  approved_at={}", uid, ts);
                    }
                }
                _ => {
                    println!("Unknown pairing subcommand: {}", sub);
                    return 1;
                }
            }
        }
        _ => {
            println!("Unknown command: {}\n", cmd);
            print!("{}", usage);
            return 1;
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::{HashMap, HashSet};

    fn cfg_with_docs(docs: HashMap<String, CommentDocumentRule>) -> CommentsConfig {
        CommentsConfig { enabled: true, policy: "pairing".to_string(), allow_from: HashSet::new(), documents: docs }
    }

    #[test]
    fn parse_frozenset_list() {
        let v = json!([" a ", "b", "", " a"]);
        let s = parse_frozenset(Some(&v)).unwrap();
        assert!(s.contains("a"));
        assert!(s.contains("b"));
        assert_eq!(s.len(), 2);
    }

    #[test]
    fn parse_frozenset_none_for_non_list() {
        assert!(parse_frozenset(Some(&json!("string"))).is_none());
        assert!(parse_frozenset(None).is_none());
    }

    #[test]
    fn parse_document_rule_policy_validation() {
        let raw = json!({"enabled": true, "policy": "ALLOWLIST", "allow_from": ["u1"]});
        let r = parse_document_rule(&raw);
        assert_eq!(r.enabled, Some(true));
        assert_eq!(r.policy.as_deref(), Some("allowlist"));
        assert!(r.allow_from.unwrap().contains("u1"));

        let raw2 = json!({"policy": "invalid"});
        let r2 = parse_document_rule(&raw2);
        assert!(r2.policy.is_none());
    }

    #[test]
    fn load_config_defaults() {
        let cfg = load_config_from_value(&json!({}));
        assert_eq!(cfg.enabled, true);
        assert_eq!(cfg.policy, "pairing");
        assert!(cfg.allow_from.is_empty());
        assert!(cfg.documents.is_empty());
    }

    #[test]
    fn has_wiki_keys_detects() {
        let mut cfg = CommentsConfig::default();
        cfg.documents.insert("wiki:tok123".into(), CommentDocumentRule::default());
        assert!(has_wiki_keys(&cfg));
        cfg.documents.clear();
        cfg.documents.insert("docx:tok".into(), CommentDocumentRule::default());
        assert!(!has_wiki_keys(&cfg));
    }

    #[test]
    fn resolve_rule_exact_over_wildcard_top() {
        let mut docs = HashMap::new();
        docs.insert("docx:tok1".into(), CommentDocumentRule { enabled: Some(false), policy: None, allow_from: None });
        docs.insert("*".into(), CommentDocumentRule { enabled: None, policy: Some("allowlist".into()), allow_from: Some(HashSet::from(["u1".into()])) });
        let cfg = CommentsConfig { enabled: true, policy: "pairing".into(), allow_from: HashSet::new(), documents: docs };
        let r = resolve_rule(&cfg, "docx", "tok1", "");
        assert_eq!(r.enabled, false);
        assert_eq!(r.policy, "allowlist");
        assert!(r.allow_from.contains("u1"));
        assert!(r.match_source.starts_with("exact"));
    }

    #[test]
    fn resolve_rule_wiki_fallback() {
        let mut docs = HashMap::new();
        docs.insert("wiki:wtok".into(), CommentDocumentRule { enabled: Some(false), policy: None, allow_from: None });
        let cfg = cfg_with_docs(docs);
        let r = resolve_rule(&cfg, "docx", "unknown", "wtok");
        assert_eq!(r.enabled, false);
        assert_eq!(r.match_source, "exact:wiki:wtok");
    }

    #[test]
    fn resolve_rule_wildcard_when_no_exact() {
        let mut docs = HashMap::new();
        docs.insert("*".into(), CommentDocumentRule { enabled: Some(false), policy: None, allow_from: None });
        let cfg = CommentsConfig { enabled: true, policy: "pairing".into(), allow_from: HashSet::new(), documents: docs };
        let r = resolve_rule(&cfg, "docx", "other", "");
        assert_eq!(r.enabled, false);
        assert_eq!(r.match_source, "wildcard");
    }

    #[test]
    fn resolve_rule_top_when_no_layers() {
        let cfg = CommentsConfig::default();
        let r = resolve_rule(&cfg, "docx", "tok", "");
        assert_eq!(r.enabled, true);
        assert_eq!(r.policy, "pairing");
        assert_eq!(r.match_source, "top");
    }

    #[test]
    fn resolve_rule_match_source_priority() {
        // enabled from wildcard, policy from top -> wildcard wins
        let mut docs = HashMap::new();
        docs.insert("*".into(), CommentDocumentRule { enabled: Some(false), policy: None, allow_from: None });
        let cfg = CommentsConfig { enabled: true, policy: "pairing".into(), allow_from: HashSet::new(), documents: docs };
        let r = resolve_rule(&cfg, "docx", "tok", "");
        assert_eq!(r.enabled, false);
        assert_eq!(r.policy, "pairing");
        assert_eq!(r.match_source, "wildcard");
    }

    #[test]
    fn is_user_allowed_allow_from_and_pairing() {
        let mut rule = ResolvedCommentRule { enabled: true, policy: "allowlist".into(), allow_from: HashSet::from(["u1".into()]), match_source: "top".into() };
        assert!(is_user_allowed(&rule, "u1"));
        assert!(!is_user_allowed(&rule, "u2"));
        rule.policy = "pairing".into();
        // u2 not in allow_from, but pairing check will look at pairing store (empty in test) -> false
        assert!(!is_user_allowed(&rule, "u2"));
    }

    #[test]
    fn load_pairing_approved_variants() {
        let v = json!({"approved": {"u1": {"approved_at": 1}, "u2": {"approved_at": 2}}});
        let s = pairing_approved_from_value(&v);
        assert!(s.contains("u1") && s.contains("u2"));
        let v2 = json!({"approved": ["u3", "u4", ""]});
        let s2 = pairing_approved_from_value(&v2);
        assert!(s2.contains("u3") && s2.contains("u4") && !s2.contains(""));
        let v3 = json!({"approved": "bad"});
        assert!(pairing_approved_from_value(&v3).is_empty());
    }
}
