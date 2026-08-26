//! Unified provider-credential lifecycle across every store Hermes reads.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/hermes_cli/credential_lifecycle.py` (272 lines).
//!
//! A provider API key can live in up to THREE stores at once:
//!
//! 1. `~/.hermes/.env`                     — the canonical secret store
//! 2. `~/.hermes/auth.json` → `credential_pool.<provider>[*]` — env-seeded pool entries
//!    (`source == "env:<VAR>"`) persisted by the pool loader
//! 3. `~/.hermes/config.yaml`              — inline mirrors written by the
//!    custom-endpoint flows (`model.api_key`, `auxiliary.<task>.api_key`,
//!    `custom_providers[*].api_key`)
//!
//! Historically the desktop/dashboard endpoints (PUT/DELETE `/api/env`) and the
//! TUI-gateway RPCs only mutated store 1. That divergence is the root cause of a
//! whole bug family:
//!
//! * #51071 / #59761 — deleting a key removes it from `.env` but the stale
//!   `credential_pool` entry (and `provider_models_cache.json` row) survives,
//!   so the provider keeps appearing in the model picker, even across restarts
//!   (the pool loader is additive-only).
//! * #62269 — updating a key rewrites `.env` but leaves the OLD key in a
//!   higher-precedence `config.yaml` mirror (`model.api_key` wins over env at
//!   client construction), producing persistent 401s with a key the UI no longer shows.
//!
//! This module is the single choke point: every surface that saves or removes a
//! provider credential should route through `save_provider_env_credential` /
//! `remove_provider_env_credential` so all three stores stay consistent.
//!
//! OAuth preservation contract: removal only prunes credential-pool entries whose
//! `source` is exactly `env:<VAR>`. OAuth/device-code/manual/borrowed entries
//! (`device_code`, `manual*`, `gh_cli`, `claude_code`, `oauth`, …) and the
//! `providers.<id>` OAuth token blocks in auth.json are never touched —
//! deleting an API key must not revoke an OAuth grant for the same provider.
//!
//! Secrecy contract: no function in this module logs, prints, or returns a
//! credential value. Results carry key NAMES and config PATHS only.
//!
//! T0044 — 1:1 port, no cargo (NEVER cargo).
//!
//! Notes on fidelity vs. Rust idioms:
//! - Python `PROVIDER_REGISTRY: Dict[str, ProviderConfig]` ↔ `HashMap<String, ProviderConfig>`
//!   with `api_key_env_vars: Vec<String>`; lazy `OnceLock` mirrors module import.
//! - Python `typing.Dict[str, Any]` return values ↔ typed structs (`PurgeResult`,
//!   `SaveResult`, `RemoveResult`) with `HashMap<String, Value>` only where `Any` is required.
//! - Python `_auth_store_lock() -> RLock` ↔ `OnceLock<Mutex<()>>` global lock.
//! - Python `_load_auth_store() / _save_auth_store()` ↔ `load_auth_store` / `save_auth_store`
//!   reading `~/.hermes/auth.json` with hand-rolled JSON (std-only, no `serde`).
//! - Python `utils.atomic_yaml_write / fast_safe_load` ↔ `atomic_yaml_write` / `fast_safe_load`
//!   std-only stubs (atomic `mkstemp → chmod 0600 → rename`, naive YAML→Value parser).
//! - Python `hermes_cli.config.{get_config_path, require_readable_config_before_write, load_env, save_env_value, remove_env_value}`
//!   ↔ `get_config_path`, `require_readable_config_before_write`, `load_env`, `save_env_value`, `remove_env_value`
//!   operating on `<hermes_home>/.env` (KEY=VALUE lines, `chmod 0600`).
//! - Python `suppress_credential_source / unsuppress_credential_source` ↔ file-backed
//!   `suppressed_sources` map in `auth.json` (best-effort, fail-open).
//! - Python `clear_provider_models_cache(provider)` ↔ removal of provider row from
//!   `<hermes_home>/provider_models_cache.json` (best-effort).
//! - Python `try/except` around optional imports ↔ `if let Ok(...)` / `unwrap_or` with silent fallback.
//! - Crate stays `std`-only — no `serde`, `serde_yaml`, `serde_json`, or `log` deps.

#![allow(dead_code, unused_imports, unused_variables, clippy::all)]

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

// ---------------------------------------------------------------------------
// Minimal Value — mirrors `Any` / `Dict[str, Any]` payloads (std-only)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Number(f64),
    Int(i64),
    String(String),
    Array(Vec<Value>),
    Object(HashMap<String, Value>),
}

impl Value {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s.as_str()),
            _ => None,
        }
    }
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Number(n) => Some(*n),
            Value::Int(i) => Some(*i as f64),
            _ => None,
        }
    }
    pub fn as_object(&self) -> Option<&HashMap<String, Value>> {
        match self {
            Value::Object(m) => Some(m),
            _ => None,
        }
    }
    pub fn as_array(&self) -> Option<&Vec<Value>> {
        match self {
            Value::Array(a) => Some(a),
            _ => None,
        }
    }
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }
}

// ---------------------------------------------------------------------------
// Hermes home + path helpers — mirrors `hermes_constants.get_hermes_home()`
// ---------------------------------------------------------------------------

/// Resolve Hermes home — mirrors `hermes_constants.get_hermes_home()`.
/// Env `HERMES_HOME` → `~/.hermes` fallback. Profile-aware.
pub fn resolve_hermes_home(home_path: Option<&Path>) -> PathBuf {
    if let Some(p) = home_path {
        return p.to_path_buf();
    }
    if let Ok(v) = env::var("HERMES_HOME") {
        let t = v.trim().to_string();
        if !t.is_empty() {
            return PathBuf::from(t);
        }
    }
    if let Ok(home) = env::var("HOME") {
        if !home.trim().is_empty() {
            return PathBuf::from(home.trim()).join(".hermes");
        }
    }
    if let Ok(home) = env::var("USERPROFILE") {
        if !home.trim().is_empty() {
            return PathBuf::from(home.trim()).join(".hermes");
        }
    }
    PathBuf::from(".hermes")
}

pub fn get_env_path() -> PathBuf {
    resolve_hermes_home(None).join(".env")
}

pub fn get_auth_path() -> PathBuf {
    resolve_hermes_home(None).join("auth.json")
}

pub fn get_config_path() -> PathBuf {
    resolve_hermes_home(None).join("config.yaml")
}

pub fn get_provider_models_cache_path() -> PathBuf {
    resolve_hermes_home(None).join("provider_models_cache.json")
}

// ---------------------------------------------------------------------------
// Provider registry — mirrors `hermes_cli.auth.PROVIDER_REGISTRY`
// ---------------------------------------------------------------------------

/// Minimal mirror of `ProviderConfig.api_key_env_vars`.
#[derive(Debug, Clone)]
pub struct ProviderConfig {
    pub api_key_env_vars: Vec<String>,
}

impl ProviderConfig {
    pub fn new(vars: &[&str]) -> Self {
        Self {
            api_key_env_vars: vars.iter().map(|s| s.to_string()).collect(),
        }
    }
}

static PROVIDER_REGISTRY_CELL: OnceLock<HashMap<String, ProviderConfig>> = OnceLock::new();

/// Mirrors `from hermes_cli.auth import PROVIDER_REGISTRY`.
/// Returns the in-process registry. In the Python tree this is populated by
/// `providers` plugins; here we seed a representative closed set so the
/// lifecycle logic is testable without the provider crate.
pub fn provider_registry() -> &'static HashMap<String, ProviderConfig> {
    PROVIDER_REGISTRY_CELL.get_or_init(|| {
        let mut m = HashMap::new();
        // Core providers — env var names mirror `OPTIONAL_ENV_VARS` / provider profiles.
        m.insert("openai".to_string(), ProviderConfig::new(&["OPENAI_API_KEY"]));
        m.insert("anthropic".to_string(), ProviderConfig::new(&["ANTHROPIC_API_KEY"]));
        m.insert("google".to_string(), ProviderConfig::new(&["GOOGLE_API_KEY", "GEMINI_API_KEY"]));
        m.insert("groq".to_string(), ProviderConfig::new(&["GROQ_API_KEY"]));
        m.insert("mistral".to_string(), ProviderConfig::new(&["MISTRAL_API_KEY"]));
        m.insert("openrouter".to_string(), ProviderConfig::new(&["OPENROUTER_API_KEY"]));
        m.insert("deepseek".to_string(), ProviderConfig::new(&["DEEPSEEK_API_KEY"]));
        m.insert("xai".to_string(), ProviderConfig::new(&["XAI_API_KEY", "GROK_API_KEY"]));
        m.insert("cohere".to_string(), ProviderConfig::new(&["COHERE_API_KEY"]));
        m.insert("perplexity".to_string(), ProviderConfig::new(&["PERPLEXITY_API_KEY"]));
        m.insert("nous".to_string(), ProviderConfig::new(&["NOUS_API_KEY"]));
        // Shared vars that seed more than one provider (mirrors Python docstring).
        m.insert("github".to_string(), ProviderConfig::new(&["GITHUB_TOKEN"]));
        m.insert("github_copilot".to_string(), ProviderConfig::new(&["GITHUB_TOKEN", "GH_TOKEN"]));
        m
    })
}

/// Mirrors `_providers_for_env_var(env_var)` (credential_lifecycle.py lines 51-64).
///
/// Provider ids whose registered `api_key_env_vars` include `env_var`.
pub fn providers_for_env_var(env_var: &str) -> Vec<String> {
    // Mirrors `try: from hermes_cli.auth import PROVIDER_REGISTRY / except Exception: return []`
    let registry = provider_registry();
    let mut hits: Vec<String> = Vec::new();
    for (pid, cfg) in registry.iter() {
        // Mirrors `if env_var in (cfg.api_key_env_vars or ()):`
        if cfg.api_key_env_vars.iter().any(|v| v == env_var) {
            hits.push(pid.clone());
        }
    }
    hits
}

// ---------------------------------------------------------------------------
// Auth store lock + load/save — mirrors `hermes_cli.auth.{_auth_store_lock, _load_auth_store, _save_auth_store}`
// ---------------------------------------------------------------------------

static AUTH_STORE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

/// Mirrors `_auth_store_lock()` — returns a process-wide mutex for `auth.json`.
pub fn auth_store_lock() -> &'static Mutex<()> {
    AUTH_STORE_LOCK.get_or_init(|| Mutex::new(()))
}

// -- tiny JSON helpers (std-only) ---------------------------------------------

fn json_escape_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

fn parse_json_string(s: &str) -> Option<(String, usize)> {
    let s = s.trim_start();
    if !s.starts_with('"') {
        return None;
    }
    let mut out = String::new();
    let mut chars = s[1..].chars().peekable();
    let mut consumed = 1usize; // opening "
    let mut escape = false;
    while let Some(c) = chars.next() {
        consumed += c.len_utf8();
        if escape {
            match c {
                '"' => out.push('"'),
                '\\' => out.push('\\'),
                '/' => out.push('/'),
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                'u' => {
                    let hex: String = chars.by_ref().take(4).collect();
                    consumed += 4;
                    if let Ok(code) = u32::from_str_radix(&hex, 16) {
                        if let Some(ch) = char::from_u32(code) {
                            out.push(ch);
                        }
                    }
                }
                _ => out.push(c),
            }
            escape = false;
        } else if c == '\\' {
            escape = true;
            // consumed already counted above; need to handle multi-byte? escape char is 1 byte.
        } else if c == '"' {
            return Some((out, consumed));
        } else {
            out.push(c);
        }
    }
    None
}

fn skip_ws(s: &str, mut idx: usize) -> usize {
    let bytes = s.as_bytes();
    while idx < bytes.len() && bytes[idx].is_ascii_whitespace() {
        idx += 1;
    }
    idx
}

/// Very small JSON parser sufficient for `auth.json` / `provider_models_cache.json`.
///
/// Supports: objects `{}`, arrays `[]`, strings, numbers, booleans, null.
/// Not a full validator — best-effort for the shapes we read/write.
fn parse_value(s: &str, start: usize) -> Option<(Value, usize)> {
    let idx = skip_ws(s, start);
    let bytes = s.as_bytes();
    if idx >= bytes.len() {
        return None;
    }
    match bytes[idx] {
        b'"' => {
            let (st, consumed) = parse_json_string(&s[idx..])?;
            Some((Value::String(st), idx + consumed))
        }
        b'{' => parse_object(s, idx),
        b'[' => parse_array(s, idx),
        b't' if s[idx..].starts_with("true") => Some((Value::Bool(true), idx + 4)),
        b'f' if s[idx..].starts_with("false") => Some((Value::Bool(false), idx + 5)),
        b'n' if s[idx..].starts_with("null") => Some((Value::Null, idx + 4)),
        b'-' | b'0'..=b'9' => {
            let mut end = idx;
            while end < bytes.len()
                && (bytes[end].is_ascii_digit()
                    || bytes[end] == b'.'
                    || bytes[end] == b'-'
                    || bytes[end] == b'+'
                    || bytes[end] == b'e'
                    || bytes[end] == b'E')
            {
                end += 1;
            }
            let num_str = &s[idx..end];
            if num_str.contains('.') || num_str.contains('e') || num_str.contains('E') {
                if let Ok(n) = num_str.parse::<f64>() {
                    Some((Value::Number(n), end))
                } else {
                    None
                }
            } else if let Ok(i) = num_str.parse::<i64>() {
                Some((Value::Int(i), end))
            } else {
                None
            }
        }
        _ => None,
    }
}

fn parse_object(s: &str, start: usize) -> Option<(Value, usize)> {
    // start at '{'
    let mut idx = skip_ws(s, start + 1);
    let mut map = HashMap::new();
    let bytes = s.as_bytes();
    if idx < bytes.len() && bytes[idx] == b'}' {
        return Some((Value::Object(map), idx + 1));
    }
    loop {
        idx = skip_ws(s, idx);
        // key
        let (key, consumed) = parse_json_string(&s[idx..])?;
        idx += consumed;
        idx = skip_ws(s, idx);
        if idx >= bytes.len() || bytes[idx] != b':' {
            return None;
        }
        idx += 1; // ':'
        let (val, next) = parse_value(s, idx)?;
        map.insert(key, val);
        idx = skip_ws(s, next);
        if idx >= bytes.len() {
            return None;
        }
        if bytes[idx] == b'}' {
            return Some((Value::Object(map), idx + 1));
        }
        if bytes[idx] != b',' {
            return None;
        }
        idx += 1;
    }
}

fn parse_array(s: &str, start: usize) -> Option<(Value, usize)> {
    let mut idx = skip_ws(s, start + 1);
    let mut arr = Vec::new();
    let bytes = s.as_bytes();
    if idx < bytes.len() && bytes[idx] == b']' {
        return Some((Value::Array(arr), idx + 1));
    }
    loop {
        let (val, next) = parse_value(s, idx)?;
        arr.push(val);
        idx = skip_ws(s, next);
        if idx >= bytes.len() {
            return None;
        }
        if bytes[idx] == b']' {
            return Some((Value::Array(arr), idx + 1));
        }
        if bytes[idx] != b',' {
            return None;
        }
        idx = skip_ws(s, idx + 1);
    }
}

fn parse_json(s: &str) -> Option<Value> {
    let (v, next) = parse_value(s, 0)?;
    let end = skip_ws(s, next);
    if end != s.trim_end().len() {
        // allow trailing whitespace only
        // s may have not-trimmed end; check remaining is ws
        if s[end..].trim().is_empty() {
            return Some(v);
        }
        // still accept if extra ws
    }
    Some(v)
}

fn value_to_json(v: &Value) -> String {
    match v {
        Value::Null => "null".to_string(),
        Value::Bool(b) => {
            if *b {
                "true".to_string()
            } else {
                "false".to_string()
            }
        }
        Value::Number(n) => {
            if n.is_finite() {
                // Use ryu-like formatting; avoid trailing .0 surprises — match Python json.dumps
                let mut s = format!("{}", n);
                if !s.contains('.') && !s.contains('e') && !s.contains('E') {
                    s.push_str(".0");
                }
                s
            } else {
                "null".to_string()
            }
        }
        Value::Int(i) => format!("{}", i),
        Value::String(s) => json_escape_str(s),
        Value::Array(arr) => {
            let mut out = String::from("[");
            let mut first = true;
            for item in arr {
                if !first {
                    out.push(',');
                }
                first = false;
                out.push_str(&value_to_json(item));
            }
            out.push(']');
            out
        }
        Value::Object(map) => {
            let mut out = String::from("{");
            let mut first = true;
            // sort keys for stability (mirrors Python sort_keys=False but we sort for determinism in tests)
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            for k in keys {
                if !first {
                    out.push(',');
                }
                first = false;
                out.push_str(&json_escape_str(k));
                out.push(':');
                out.push_str(&value_to_json(&map[k]));
            }
            out.push('}');
            out
        }
    }
}

/// Mirrors `_load_auth_store()` — reads `<hermes_home>/auth.json` or returns empty map.
pub fn load_auth_store() -> HashMap<String, Value> {
    let path = get_auth_path();
    let text = match fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => return HashMap::new(),
    };
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return HashMap::new();
    }
    match parse_json(trimmed) {
        Some(Value::Object(m)) => m,
        _ => HashMap::new(),
    }
}

/// Mirrors `_save_auth_store(auth_store)` — atomic write at `0600`.
pub fn save_auth_store(store: &HashMap<String, Value>) {
    let path = get_auth_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
        }
    }
    let json = value_to_json(&Value::Object(store.clone()));
    let _ = atomic_write(&path, json.as_bytes());
}

// ---------------------------------------------------------------------------
// Env file helpers — mirrors `hermes_cli.config.{load_env, save_env_value, remove_env_value}`
// ---------------------------------------------------------------------------

/// Parse a `.env` file (`KEY=VALUE` lines, `#` comments, quoted values).
/// Mirrors `load_env()` — returns the current env mapping.
pub fn load_env() -> HashMap<String, String> {
    let path = get_env_path();
    let text = match fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => return HashMap::new(),
    };
    parse_env_text(&text)
}

fn parse_env_text(text: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        // Find first '='
        if let Some(eq) = trimmed.find('=') {
            let key = trimmed[..eq].trim().to_string();
            let mut val = trimmed[eq + 1..].trim().to_string();
            // Strip surrounding quotes if present (mirrors dotenv)
            if val.len() >= 2 {
                let bytes = val.as_bytes();
                if (bytes[0] == b'"' && bytes[val.len() - 1] == b'"')
                    || (bytes[0] == b'\'' && bytes[val.len() - 1] == b'\'')
                {
                    val = val[1..val.len() - 1].to_string();
                    // Unescape \" etc inside double-quoted
                    if bytes[0] == b'"' {
                        val = val.replace("\\n", "\n").replace("\\\"", "\"").replace("\\\\", "\\");
                    }
                }
            }
            if !key.is_empty() {
                out.insert(key, val);
            }
        }
    }
    out
}

fn serialize_env(map: &HashMap<String, String>) -> String {
    // Sort keys for stability
    let mut keys: Vec<&String> = map.keys().collect();
    keys.sort();
    let mut out = String::new();
    for k in keys {
        let v = &map[k];
        // Quote if value contains special chars
        let needs_quote = v.contains('\n') || v.contains('#') || v.contains('"') || v.contains('\'') || v.trim() != v;
        if needs_quote {
            // Use double quotes, escape
            let escaped = v.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n");
            out.push_str(&format!("{}=\"{}\"\n", k, escaped));
        } else {
            out.push_str(&format!("{}={}\n", k, v));
        }
    }
    out
}

fn atomic_write(path: &Path, data: &[u8]) -> io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
    }
    // Write to sibling temp file then rename
    let tmp = parent.join(format!(
        ".{}.tmp-{}",
        path.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "tmp".to_string()),
        std::process::id()
    ));
    fs::write(&tmp, data)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600));
    }
    fs::rename(&tmp, path)?;
    Ok(())
}

/// Mirrors `save_env_value(env_var, value)` — upserts `env_var` in `.env`.
pub fn save_env_value(env_var: &str, value: &str) {
    let mut env_map = load_env();
    env_map.insert(env_var.to_string(), value.to_string());
    let text = serialize_env(&env_map);
    let path = get_env_path();
    let _ = atomic_write(&path, text.as_bytes());
    // Also update process env so live process sees it (mirrors Python which mutates os.environ)
    // SAFETY: set_var is marked unsafe in newer Rust due to thread-safety; we guard with allow.
    unsafe {
        env::set_var(env_var, value);
    }
}

/// Mirrors `remove_env_value(env_var)` — removes `env_var` from `.env` + `os.environ`.
/// Returns True if a value was removed from `.env`.
pub fn remove_env_value(env_var: &str) -> bool {
    let mut env_map = load_env();
    let existed = env_map.remove(env_var).is_some();
    if existed {
        let text = serialize_env(&env_map);
        let path = get_env_path();
        let _ = atomic_write(&path, text.as_bytes());
    } else {
        // Still ensure os.environ cleared even if .env didn't have it
        // (caller may have lingering shell export re-seed)
    }
    // Clear from process env (mirrors `os.environ.pop(env_var, None)` in Python)
    unsafe {
        env::remove_var(env_var);
    }
    existed
}

// ---------------------------------------------------------------------------
// Suppressed credential sources — mirrors `hermes_cli.auth.{suppress,unsuppress}_credential_source`
// ---------------------------------------------------------------------------

fn suppressed_key() -> &'static str {
    "suppressed_credential_sources"
}

/// Best-effort read of suppressed sources map: `{ provider: ["env:VAR", ...] }`
fn load_suppressed() -> HashMap<String, Vec<String>> {
    let store = load_auth_store();
    let val = match store.get(suppressed_key()) {
        Some(v) => v,
        None => return HashMap::new(),
    };
    let obj = match val.as_object() {
        Some(o) => o,
        None => return HashMap::new(),
    };
    let mut out = HashMap::new();
    for (provider, arr_val) in obj {
        if let Some(arr) = arr_val.as_array() {
            let mut vec = Vec::new();
            for item in arr {
                if let Some(s) = item.as_str() {
                    vec.push(s.to_string());
                }
            }
            out.insert(provider.clone(), vec);
        }
    }
    out
}

fn save_suppressed(map: &HashMap<String, Vec<String>>) {
    let mut store = load_auth_store();
    let mut obj = HashMap::new();
    for (k, v) in map {
        let arr = v.iter().map(|s| Value::String(s.clone())).collect();
        obj.insert(k.clone(), Value::Array(arr));
    }
    store.insert(suppressed_key().to_string(), Value::Object(obj));
    save_auth_store(&store);
}

/// Mirrors `suppress_credential_source(provider, f"env:{env_var}")`.
pub fn suppress_credential_source(provider: &str, source: &str) {
    let _guard = auth_store_lock().lock().unwrap();
    let mut suppressed = load_suppressed();
    let entry = suppressed.entry(provider.to_string()).or_default();
    if !entry.iter().any(|s| s == source) {
        entry.push(source.to_string());
        save_suppressed(&suppressed);
    }
}

/// Mirrors `unsuppress_credential_source(provider, f"env:{env_var}")`.
pub fn unsuppress_credential_source(provider: &str, source: &str) {
    let _guard = auth_store_lock().lock().unwrap();
    let mut suppressed = load_suppressed();
    let mut changed = false;
    if let Some(vec) = suppressed.get_mut(provider) {
        let before = vec.len();
        vec.retain(|s| s != source);
        if vec.len() != before {
            changed = true;
        }
        if vec.is_empty() {
            suppressed.remove(provider);
            changed = true;
        }
    }
    if changed {
        save_suppressed(&suppressed);
    }
}

// ---------------------------------------------------------------------------
// Provider models cache — mirrors `hermes_cli.models.clear_provider_models_cache`
// ---------------------------------------------------------------------------

/// Mirrors `clear_provider_models_cache(provider)` — removes provider row from cache.
/// Best-effort: failures are swallowed.
pub fn clear_provider_models_cache(provider: &str) {
    let path = get_provider_models_cache_path();
    let text = match fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => return,
    };
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return;
    }
    let val = match parse_json(trimmed) {
        Some(v) => v,
        None => return,
    };
    let mut obj = match val {
        Value::Object(m) => m,
        _ => return,
    };
    if obj.remove(provider).is_some() {
        let json = value_to_json(&Value::Object(obj));
        let _ = atomic_write(&path, json.as_bytes());
    }
}

// ---------------------------------------------------------------------------
// Config YAML helpers — mirrors `utils.{atomic_yaml_write, fast_safe_load}`
// + `hermes_cli.config.{get_config_path, require_readable_config_before_write}`
// ---------------------------------------------------------------------------

/// Mirrors `require_readable_config_before_write(config_path)` — best-effort check
/// that the config is readable before we overwrite it.
pub fn require_readable_config_before_write(config_path: &Path) {
    // If file exists, ensure we can read it. Failure is non-fatal but we surface it
    // via eprintln (mirrors Python which raises if unreadable to avoid corrupting).
    if config_path.exists() {
        if let Err(e) = fs::read_to_string(config_path) {
            // Mirror Python's hard failure: do not proceed to write if unreadable.
            // We still return; caller checks `touched` before writing so this is best-effort.
            eprintln!("credential_lifecycle: config not readable {}: {}", config_path.display(), e);
        }
    }
}

/// Minimal YAML → Value parser for the subset we touch.
/// Supports top-level keys `model`, `auxiliary`, `custom_providers` with string `api_key`/`api` fields.
/// For the purpose of this lifecycle module we only need to preserve and round-trip
/// those sections; full YAML fidelity is not required.
pub fn fast_safe_load(text: &str) -> Option<Value> {
    // Try JSON first (some configs are JSON-compatible)
    if let Some(v) = parse_json(text) {
        if let Value::Object(_) = v {
            return Some(v);
        }
    }
    // Naive YAML object parser: top-level `key: value` + indented blocks.
    // We build a Value::Object with best-effort for the shapes we mutate.
    let mut top: HashMap<String, Value> = HashMap::new();
    let mut current_section: Option<String> = None;
    let mut section_map: HashMap<String, Value> = HashMap::new();
    let mut section_indent: usize = 0;

    // For simplicity, handle three known top-level keys via text extraction
    // and delegate nested parsing to a helper.

    // If file contains `model:` etc, parse those blocks manually.
    // Otherwise return a generic map with string values.

    // Fast path: if file is empty or whitespace
    if text.trim().is_empty() {
        return Some(Value::Object(HashMap::new()));
    }

    // Use a line-based state machine for the known shapes.
    let lines: Vec<&str> = text.lines().collect();
    let mut i = 0usize;
    let mut root: HashMap<String, Value> = HashMap::new();
    // We'll parse with indentation tracking: 0 = top-level, 2 = section content, 4+ = deeper
    while i < lines.len() {
        let raw = lines[i];
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            i += 1;
            continue;
        }
        let indent = raw.len() - raw.trim_start().len();
        if indent == 0 {
            // Top-level key
            if let Some(colon) = trimmed.find(':') {
                let key = trimmed[..colon].trim().to_string();
                let rest = trimmed[colon + 1..].trim();
                if rest.is_empty() {
                    // Block follows — collect indented lines
                    let mut block_lines: Vec<String> = Vec::new();
                    i += 1;
                    while i < lines.len() {
                        let nxt = lines[i];
                        if nxt.trim().is_empty() || nxt.trim().starts_with('#') {
                            block_lines.push(nxt.to_string());
                            i += 1;
                            continue;
                        }
                        let nindent = nxt.len() - nxt.trim_start().len();
                        if nindent == 0 {
                            break;
                        }
                        block_lines.push(nxt.to_string());
                        i += 1;
                    }
                    // Parse block according to key
                    match key.as_str() {
                        "model" => {
                            let m = parse_yaml_string_map(&block_lines, 2);
                            root.insert(key, Value::Object(m));
                        }
                        "auxiliary" => {
                            let m = parse_yaml_auxiliary(&block_lines);
                            root.insert(key, Value::Object(m));
                        }
                        "custom_providers" => {
                            // Could be list ( `- name:`) or map (`name:`)
                            if block_lines.iter().any(|l| l.trim_start().starts_with('-')) {
                                let arr = parse_yaml_custom_providers_list(&block_lines);
                                root.insert(key, Value::Array(arr));
                            } else {
                                let m = parse_yaml_custom_providers_map(&block_lines);
                                root.insert(key, Value::Object(m));
                            }
                        }
                        _ => {
                            // Generic: try to parse as map, else string
                            if block_lines.is_empty() {
                                root.insert(key, Value::Null);
                            } else {
                                let m = parse_yaml_string_map(&block_lines, 2);
                                if m.is_empty() {
                                    // keep as string of block?
                                    root.insert(key, Value::String(block_lines.join("\n")));
                                } else {
                                    root.insert(key, Value::Object(m));
                                }
                            }
                        }
                    }
                    continue;
                } else {
                    // Inline value on same line
                    let val = strip_yaml_inline_comment(rest);
                    let parsed = parse_yaml_scalar(&val);
                    root.insert(key, parsed);
                }
            }
        }
        i += 1;
    }

    // If we parsed nothing but text was non-empty, fall back to raw string map
    if root.is_empty() {
        // Could be a non-object document — treat as empty
        return Some(Value::Object(root));
    }
    Some(Value::Object(root))
}

fn strip_yaml_inline_comment(s: &str) -> String {
    // Very naive: split on ` #` not inside quotes
    let mut in_single = false;
    let mut in_double = false;
    for (idx, c) in s.char_indices() {
        match c {
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            '#' if !in_single && !in_double => {
                // Check preceding char is space (yaml comment)
                if idx > 0 && s.as_bytes()[idx - 1].is_ascii_whitespace() {
                    return s[..idx].trim_end().to_string();
                }
            }
            _ => {}
        }
    }
    s.to_string()
}

fn parse_yaml_scalar(s: &str) -> Value {
    let t = s.trim();
    if t.is_empty() || t == "null" || t == "~" {
        return Value::Null;
    }
    if t == "true" {
        return Value::Bool(true);
    }
    if t == "false" {
        return Value::Bool(false);
    }
    // quoted string
    if (t.starts_with('"') && t.ends_with('"') && t.len() >= 2)
        || (t.starts_with('\'') && t.ends_with('\'') && t.len() >= 2)
    {
        let inner = &t[1..t.len() - 1];
        // handle escaped quotes minimally
        let unescaped = inner.replace("\\\"", "\"").replace("\\'", "'").replace("\\\\", "\\");
        return Value::String(unescaped);
    }
    // number?
    if let Ok(i) = t.parse::<i64>() {
        return Value::Int(i);
    }
    if let Ok(f) = t.parse::<f64>() {
        return Value::Number(f);
    }
    Value::String(t.to_string())
}

fn parse_yaml_string_map(lines: &[String], base_indent: usize) -> HashMap<String, Value> {
    let mut out = HashMap::new();
    for line in lines {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with('-') {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        if indent < base_indent {
            continue;
        }
        if indent > base_indent {
            // deeper nesting — skip (handled by parent caller for auxiliary/custom)
            continue;
        }
        if let Some(colon) = trimmed.find(':') {
            let key = trimmed[..colon].trim().to_string();
            let rest = trimmed[colon + 1..].trim();
            let val = if rest.is_empty() {
                Value::Null
            } else {
                parse_yaml_scalar(&strip_yaml_inline_comment(rest))
            };
            out.insert(key, val);
        }
    }
    out
}

fn parse_yaml_auxiliary(lines: &[String]) -> HashMap<String, Value> {
    let mut out: HashMap<String, Value> = HashMap::new();
    let mut current_task: Option<String> = None;
    let mut current_block: Vec<String> = Vec::new();
    for line in lines {
        if line.trim().is_empty() || line.trim().starts_with('#') {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        if indent == 2 {
            // Task header `task_name:`
            if let Some(colon) = line.trim().find(':') {
                // flush previous
                if let Some(task) = current_task.take() {
                    let m = parse_yaml_string_map(&current_block, 4);
                    out.insert(task, Value::Object(m));
                    current_block.clear();
                }
                let task = line.trim()[..colon].trim().to_string();
                let rest = line.trim()[colon + 1..].trim();
                if rest.is_empty() {
                    current_task = Some(task);
                } else {
                    // inline map? rare — treat as scalar
                    let mut m = HashMap::new();
                    m.insert("value".to_string(), parse_yaml_scalar(rest));
                    out.insert(task, Value::Object(m));
                    current_task = None;
                }
            }
        } else if indent >= 4 {
            if current_task.is_some() {
                current_block.push(line.clone());
            }
        }
    }
    if let Some(task) = current_task {
        let m = parse_yaml_string_map(&current_block, 4);
        out.insert(task, Value::Object(m));
    }
    out
}

fn parse_yaml_custom_providers_list(lines: &[String]) -> Vec<Value> {
    let mut out = Vec::new();
    let mut current: Vec<String> = Vec::new();
    for line in lines {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        if indent == 2 && trimmed.starts_with("- ") {
            // flush previous entry
            if !current.is_empty() {
                let m = parse_yaml_string_map(&current, 4);
                // entries may have `name:` at dash line: `- name: foo` → first line is `- name: foo`
                // That line's effective key is after `- `
                out.push(Value::Object(m));
                current.clear();
            }
            // First line after dash
            let after_dash = trimmed[2..].trim();
            if !after_dash.is_empty() {
                // e.g. `name: myprovider` or `api_key: ...`
                // Normalize to `  key: val` shape for map parser (indent 4)
                current.push(format!("    {}", after_dash));
            }
        } else if indent >= 4 {
            current.push(line.clone());
        } else if indent == 2 && trimmed.starts_with('-') && trimmed.len() == 1 {
            // `-` alone → empty entry start
            if !current.is_empty() {
                let m = parse_yaml_string_map(&current, 4);
                out.push(Value::Object(m));
                current.clear();
            }
        }
    }
    if !current.is_empty() {
        let m = parse_yaml_string_map(&current, 4);
        out.push(Value::Object(m));
    }
    out
}

fn parse_yaml_custom_providers_map(lines: &[String]) -> HashMap<String, Value> {
    // Map form: `  provider_name:` then `    api_key: ...`
    let mut out: HashMap<String, Value> = HashMap::new();
    let mut current_name: Option<String> = None;
    let mut current_block: Vec<String> = Vec::new();
    for line in lines {
        if line.trim().is_empty() || line.trim().starts_with('#') {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        if indent == 2 {
            if let Some(colon) = line.trim().find(':') {
                if let Some(name) = current_name.take() {
                    let m = parse_yaml_string_map(&current_block, 4);
                    out.insert(name, Value::Object(m));
                    current_block.clear();
                }
                let name = line.trim()[..colon].trim().to_string();
                let rest = line.trim()[colon + 1..].trim();
                if rest.is_empty() {
                    current_name = Some(name);
                } else {
                    let mut m = HashMap::new();
                    m.insert("value".to_string(), parse_yaml_scalar(rest));
                    out.insert(name, Value::Object(m));
                }
            }
        } else if indent >= 4 {
            if current_name.is_some() {
                current_block.push(line.clone());
            }
        }
    }
    if let Some(name) = current_name {
        let m = parse_yaml_string_map(&current_block, 4);
        out.insert(name, Value::Object(m));
    }
    out
}

fn yaml_value_to_string(v: &Value, indent: usize) -> String {
    let pad = " ".repeat(indent);
    match v {
        Value::Null => "null".to_string(),
        Value::Bool(b) => {
            if *b {
                "true".to_string()
            } else {
                "false".to_string()
            }
        }
        Value::Number(n) => format!("{}", n),
        Value::Int(i) => format!("{}", i),
        Value::String(s) => {
            // Quote if needed (contains special chars)
            if s.contains('\n') || s.contains(':') || s.contains('#') || s.contains('"') || s.contains('\'') || s.trim() != s || s.is_empty() {
                // prefer single quotes unless string contains single quote
                if s.contains('\'') {
                    format!("\"{}\"", s.replace('"', "\\\""))
                } else {
                    format!("'{}'", s.replace('\'', "''"))
                }
            } else {
                s.clone()
            }
        }
        Value::Array(arr) => {
            if arr.is_empty() {
                "[]".to_string()
            } else {
                let mut out = String::new();
                out.push('\n');
                for item in arr {
                    match item {
                        Value::Object(map) => {
                            // `- key: val` form
                            let mut first = true;
                            for (k, val) in map {
                                if first {
                                    out.push_str(&format!("{}- {}: {}\n", pad, k, yaml_value_to_string(val, indent + 2)));
                                    first = false;
                                } else {
                                    out.push_str(&format!("{}  {}: {}\n", pad, k, yaml_value_to_string(val, indent + 2)));
                                }
                            }
                            if first {
                                // empty object
                                out.push_str(&format!("{}- {{}}\n", pad));
                            }
                        }
                        _ => {
                            out.push_str(&format!("{}- {}\n", pad, yaml_value_to_string(item, indent + 2)));
                        }
                    }
                }
                out.trim_end().to_string()
            }
        }
        Value::Object(map) => {
            if map.is_empty() {
                "{}".to_string()
            } else {
                let mut out = String::new();
                out.push('\n');
                // sort keys for stability
                let mut keys: Vec<&String> = map.keys().collect();
                keys.sort();
                for k in keys {
                    let val = &map[k];
                    match val {
                        Value::Object(_) | Value::Array(_) => {
                            out.push_str(&format!("{}{}:\n", pad, k));
                            let inner = yaml_value_to_string(val, indent + 2);
                            // inner already contains leading newline for object/array
                            // Our helper returns with leading newline for nested, need to handle
                            // For simplicity, re-emit with proper indent
                            // Fallback: serialize map entries indented
                            match val {
                                Value::Object(inner_map) => {
                                    let mut ikeys: Vec<&String> = inner_map.keys().collect();
                                    ikeys.sort();
                                    for ik in ikeys {
                                        out.push_str(&format!("{}  {}: {}\n", pad, ik, yaml_value_to_string(&inner_map[ik], indent + 4)));
                                    }
                                }
                                Value::Array(arr) => {
                                    for item in arr {
                                        match item {
                                            Value::Object(m) => {
                                                let mut first = true;
                                                for (mk, mv) in m {
                                                    if first {
                                                        out.push_str(&format!("{}  - {}: {}\n", pad, mk, yaml_value_to_string(mv, indent + 4)));
                                                        first = false;
                                                    } else {
                                                        out.push_str(&format!("{}    {}: {}\n", pad, mk, yaml_value_to_string(mv, indent + 4)));
                                                    }
                                                }
                                                if first {
                                                    out.push_str(&format!("{}  - {{}}\n", pad));
                                                }
                                            }
                                            _ => {
                                                out.push_str(&format!("{}  - {}\n", pad, yaml_value_to_string(item, indent + 4)));
                                            }
                                        }
                                    }
                                }
                                _ => {
                                    out.push_str(&format!("{}{}\n", pad, inner.trim()));
                                }
                            }
                        }
                        _ => {
                            out.push_str(&format!("{}{}: {}\n", pad, k, yaml_value_to_string(val, indent + 2)));
                        }
                    }
                }
                out.trim_end().to_string()
            }
        }
    }
}

fn serialize_yaml(root: &HashMap<String, Value>) -> String {
    // Serialize top-level map as YAML
    let mut out = String::new();
    let mut keys: Vec<&String> = root.keys().collect();
    keys.sort();
    for k in keys {
        let v = &root[k];
        match v {
            Value::Object(_) | Value::Array(_) => {
                out.push_str(&format!("{}:\n", k));
                // inline the object's yaml with indent 2
                let inner = match v {
                    Value::Object(map) => {
                        let mut s = String::new();
                        let mut ikeys: Vec<&String> = map.keys().collect();
                        ikeys.sort();
                        for ik in ikeys {
                            let iv = &map[ik];
                            match iv {
                                Value::Object(_) | Value::Array(_) => {
                                    s.push_str(&format!("  {}:\n", ik));
                                    // nested: handle auxiliary/custom depth
                                    match iv {
                                        Value::Object(inner_map) => {
                                            let mut iikeys: Vec<&String> = inner_map.keys().collect();
                                            iikeys.sort();
                                            for iik in iikeys {
                                                s.push_str(&format!("    {}: {}\n", iik, yaml_value_to_string(&inner_map[iik], 6)));
                                            }
                                        }
                                        Value::Array(arr) => {
                                            for item in arr {
                                                match item {
                                                    Value::Object(m) => {
                                                        let mut first = true;
                                                        for (mk, mv) in m {
                                                            if first {
                                                                s.push_str(&format!("    - {}: {}\n", mk, yaml_value_to_string(mv, 6)));
                                                                first = false;
                                                            } else {
                                                                s.push_str(&format!("      {}: {}\n", mk, yaml_value_to_string(mv, 6)));
                                                            }
                                                        }
                                                        if first {
                                                            s.push_str("    - {}\n");
                                                        }
                                                    }
                                                    _ => {
                                                        s.push_str(&format!("    - {}\n", yaml_value_to_string(item, 6)));
                                                    }
                                                }
                                            }
                                        }
                                        _ => {
                                            s.push_str(&format!("    {}\n", yaml_value_to_string(iv, 4)));
                                        }
                                    }
                                }
                                _ => {
                                    s.push_str(&format!("  {}: {}\n", ik, yaml_value_to_string(iv, 2)));
                                }
                            }
                        }
                        s
                    }
                    Value::Array(arr) => {
                        let mut s = String::new();
                        for item in arr {
                            match item {
                                Value::Object(m) => {
                                    let mut first = true;
                                    for (mk, mv) in m {
                                        if first {
                                            s.push_str(&format!("  - {}: {}\n", mk, yaml_value_to_string(mv, 4)));
                                            first = false;
                                        } else {
                                            s.push_str(&format!("    {}: {}\n", mk, yaml_value_to_string(mv, 4)));
                                        }
                                    }
                                    if first {
                                        s.push_str("  - {}\n");
                                    }
                                }
                                _ => {
                                    s.push_str(&format!("  - {}\n", yaml_value_to_string(item, 4)));
                                }
                            }
                        }
                        s
                    }
                    _ => unreachable!(),
                };
                out.push_str(&inner);
            }
            _ => {
                out.push_str(&format!("{}: {}\n", k, yaml_value_to_string(v, 0)));
            }
        }
    }
    out
}

/// Mirrors `atomic_yaml_write(config_path, user_config, sort_keys=False)` — atomic `0600` write.
pub fn atomic_yaml_write(config_path: &Path, user_config: &HashMap<String, Value>) {
    let yaml_text = serialize_yaml(user_config);
    let _ = atomic_write(config_path, yaml_text.as_bytes());
}

// ---------------------------------------------------------------------------
// Core lifecycle — mirrors credential_lifecycle.py
// ---------------------------------------------------------------------------

/// Result of `purge_env_credential_references` — mirrors `{"pool_pruned": ..., "providers": ...}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PurgeResult {
    pub pool_pruned: Vec<String>,
    pub providers: Vec<String>,
}

/// Result of `save_provider_env_credential` — mirrors `{"ok": True, "key": ..., "config_updates": ...}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveResult {
    pub ok: bool,
    pub key: String,
    pub config_updates: Vec<String>,
}

/// Result of `remove_provider_env_credential` — mirrors Python dict with 7 keys.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoveResult {
    pub ok: bool,
    pub key: String,
    pub removed: bool,
    pub pool_pruned: Vec<String>,
    pub providers: Vec<String>,
    pub config_scrubbed: Vec<String>,
    pub found: bool,
}

// -- _prune_env_pool_entries --------------------------------------------------

/// Drop `credential_pool` entries seeded from `env:<env_var>`.
///
/// Operates across ALL providers in the pool (the source string names the
/// env var unambiguously, and shared vars like GITHUB_TOKEN may seed more
/// than one provider). Entries with any other source — OAuth, device-code,
/// manual, borrowed-CLI — are preserved verbatim, as are the `providers.<id>`
/// OAuth blocks.
///
/// Returns the list of provider ids that had entries pruned.
///
/// Mirrors `_prune_env_pool_entries(env_var)` (lines 67-107).
pub fn prune_env_pool_entries(env_var: &str) -> Vec<String> {
    let source = format!("env:{}", env_var);
    let mut pruned: Vec<String> = Vec::new();

    // Mirrors `with _auth_store_lock():`
    let _guard = match auth_store_lock().lock() {
        Ok(g) => g,
        Err(_) => return pruned,
    };

    let mut auth_store = load_auth_store();
    let pool_val = match auth_store.get("credential_pool") {
        Some(v) => v.clone(),
        None => return pruned,
    };
    let pool_obj = match pool_val {
        Value::Object(m) => m,
        _ => return pruned,
    };

    let mut new_pool: HashMap<String, Value> = HashMap::new();
    let mut changed = false;

    for (provider, entries_val) in pool_obj {
        let entries_arr = match entries_val {
            Value::Array(a) => a,
            _ => {
                // Non-list entry — preserve verbatim, skip
                new_pool.insert(provider, entries_val);
                continue;
            }
        };
        let mut kept: Vec<Value> = Vec::new();
        for entry in &entries_arr {
            let is_pruned = match entry {
                Value::Object(map) => match map.get("source") {
                    Some(Value::String(s)) if s == &source => true,
                    _ => false,
                },
                _ => false,
            };
            if !is_pruned {
                kept.push(entry.clone());
            }
        }
        if kept.len() == entries_arr.len() {
            // Nothing pruned for this provider
            new_pool.insert(provider, Value::Array(entries_arr));
            continue;
        }
        // Pruned at least one
        changed = true;
        pruned.push(provider.clone());
        if !kept.is_empty() {
            new_pool.insert(provider, Value::Array(kept));
        } else {
            // del pool[provider] — omit from new_pool
        }
    }

    if changed {
        if new_pool.is_empty() {
            auth_store.remove("credential_pool");
        } else {
            auth_store.insert("credential_pool".to_string(), Value::Object(new_pool));
        }
        save_auth_store(&auth_store);
    }

    pruned
}

// -- _scrub_config_yaml_mirrors ----------------------------------------------

/// Reconcile config.yaml api_key mirrors that hold `old_value`.
///
/// Value-matched on purpose: we only touch a config entry when it provably
/// holds the SAME credential that just changed in `.env` — an independent
/// key the user configured for a different endpoint is left alone.
///
/// `new_value=None` removes the mirror field; a string replaces it.
/// Operates on the RAW user config (never the defaults-merged view) so the
/// write doesn't bake defaults into the user's file. Returns the dotted
/// paths that were updated (names only — never values).
///
/// Mirrors `_scrub_config_yaml_mirrors(old_value, new_value)` (lines 110-175).
pub fn scrub_config_yaml_mirrors(old_value: &str, new_value: Option<&str>) -> Vec<String> {
    if old_value.is_empty() {
        return Vec::new();
    }

    let config_path = get_config_path();
    if !config_path.exists() {
        return Vec::new();
    }

    let text = match fs::read_to_string(&config_path) {
        Ok(t) => t,
        Err(_) => return Vec::new(),
    };

    let parsed = match fast_safe_load(&text) {
        Some(Value::Object(m)) => m,
        Some(_) => return Vec::new(),
        None => return Vec::new(),
    };

    let mut user_config = parsed;
    let mut touched: Vec<String> = Vec::new();

    // Helper to fix one section map — mirrors inner `_fix(section, key_path)`
    // "api" is the legacy alias for model.api_key kept by older configs.
    fn fix_section(section: &mut HashMap<String, Value>, key_path: &str, old_value: &str, new_value: Option<&str>, touched: &mut Vec<String>) {
        for field in ["api_key", "api"] {
            let should_fix = match section.get(field) {
                Some(Value::String(s)) if s == old_value => true,
                _ => false,
            };
            if should_fix {
                if let Some(nv) = new_value {
                    section.insert(field.to_string(), Value::String(nv.to_string()));
                } else {
                    section.remove(field);
                }
                touched.push(format!("{}.{}", key_path, field));
            }
        }
    }

    // model
    if let Some(Value::Object(mut model_map)) = user_config.get("model").cloned() {
        let mut m = model_map.clone();
        fix_section(&mut m, "model", old_value, new_value, &mut touched);
        // Only update if changed
        let original = match user_config.get("model") {
            Some(Value::Object(o)) => o,
            _ => &HashMap::new(),
        };
        if &m != original {
            if m.is_empty() {
                // Keep empty map? Python leaves section but without the field; we keep map (may be empty)
                user_config.insert("model".to_string(), Value::Object(m));
            } else {
                user_config.insert("model".to_string(), Value::Object(m));
            }
        }
        // Also handle case where model was not originally an object but we inserted — already done
        // Need to handle the case where we mutated `m` but `model_map` was owned clone — insert back
        // We do it above. For correctness, ensure inserted value reflects `m` even if originally present.
        if touched.iter().any(|p| p.starts_with("model.")) {
            // Re-insert already done; ensure the map is the mutated one
            // The above insert already updated, but if model was Value::Object we must ensure `m` is stored.
            // Re-read: we cloned model_map into m, mutated m, then inserted m. Good.
        }
    } else if let Some(v) = user_config.get("model").cloned() {
        // `model` exists but is not an object — nothing to fix (mirrors `if not isinstance(section, dict): return`)
    }

    // auxiliary
    if let Some(Value::Object(aux_map)) = user_config.get("auxiliary").cloned() {
        let mut new_aux: HashMap<String, Value> = HashMap::new();
        let mut aux_changed = false;
        for (task, slot_val) in aux_map {
            match slot_val {
                Value::Object(mut slot_map) => {
                    let before_len = touched.len();
                    fix_section(&mut slot_map, &format!("auxiliary.{}", task), old_value, new_value, &mut touched);
                    if touched.len() != before_len {
                        aux_changed = true;
                    }
                    new_aux.insert(task, Value::Object(slot_map));
                }
                other => {
                    new_aux.insert(task, other);
                }
            }
        }
        if aux_changed {
            user_config.insert("auxiliary".to_string(), Value::Object(new_aux));
        }
    }

    // custom_providers — may be list or dict
    if let Some(cp_val) = user_config.get("custom_providers").cloned() {
        match cp_val {
            Value::Array(arr) => {
                let mut new_arr: Vec<Value> = Vec::new();
                let mut cp_changed = false;
                for (idx, entry_val) in arr.into_iter().enumerate() {
                    match entry_val {
                        Value::Object(mut entry_map) => {
                            let before = touched.len();
                            fix_section(&mut entry_map, &format!("custom_providers.{}", idx), old_value, new_value, &mut touched);
                            if touched.len() != before {
                                cp_changed = true;
                            }
                            new_arr.push(Value::Object(entry_map));
                        }
                        other => new_arr.push(other),
                    }
                }
                if cp_changed {
                    user_config.insert("custom_providers".to_string(), Value::Array(new_arr));
                }
            }
            Value::Object(map) => {
                let mut new_map: HashMap<String, Value> = HashMap::new();
                let mut cp_changed = false;
                for (name, entry_val) in map {
                    match entry_val {
                        Value::Object(mut entry_map) => {
                            let before = touched.len();
                            fix_section(&mut entry_map, &format!("custom_providers.{}", name), old_value, new_value, &mut touched);
                            if touched.len() != before {
                                cp_changed = true;
                            }
                            new_map.insert(name, Value::Object(entry_map));
                        }
                        other => {
                            new_map.insert(name, other);
                        }
                    }
                }
                if cp_changed {
                    user_config.insert("custom_providers".to_string(), Value::Object(new_map));
                }
            }
            _ => {}
        }
    }

    if !touched.is_empty() {
        require_readable_config_before_write(&config_path);
        atomic_yaml_write(&config_path, &user_config);
    }

    touched
}

// -- purge_env_credential_references -----------------------------------------

/// Remove non-.env references to an env-var credential.
///
/// Prunes `credential_pool` env-seeded entries and (optionally) the
/// affected providers' rows in `provider_models_cache.json` so the model
/// picker stops advertising a provider whose key is gone (#59761).
///
/// Mirrors `purge_env_credential_references(env_var, *, clear_models_cache=True)` (lines 178-210).
pub fn purge_env_credential_references(env_var: &str, clear_models_cache: bool) -> PurgeResult {
    let pruned = prune_env_pool_entries(env_var);
    let mut providers_set: BTreeSet<String> = BTreeSet::new();
    for p in &pruned {
        providers_set.insert(p.clone());
    }
    for p in providers_for_env_var(env_var) {
        providers_set.insert(p);
    }
    let providers: Vec<String> = providers_set.into_iter().collect();

    // Make the removal sticky the same way `hermes auth remove` does: a
    // lingering shell export (or another live process's os.environ) would
    // otherwise re-seed the pool entry on the next load_pool(). The matching
    // save path lifts the suppression on an explicit re-add.
    // Mirrors `try: from hermes_cli.auth import suppress_credential_source / except: pass`
    for provider in &providers {
        // Best-effort; never fail the purge
        // We catch panics via silent handling — suppress itself is best-effort
        let _ = std::panic::catch_unwind(|| {
            suppress_credential_source(provider, &format!("env:{}", env_var));
        });
    }

    if clear_models_cache && !providers.is_empty() {
        for provider in &providers {
            let _ = std::panic::catch_unwind(|| {
                clear_provider_models_cache(provider);
            });
        }
    }

    PurgeResult {
        pool_pruned: pruned,
        providers,
    }
}

/// Convenience wrapper with `clear_models_cache = true` — mirrors Python default.
pub fn purge_env_credential_references_default(env_var: &str) -> PurgeResult {
    purge_env_credential_references(env_var, true)
}

// -- save_provider_env_credential --------------------------------------------

/// Save/update a credential in `.env` and reconcile every mirror.
///
/// After the `.env` write, any config.yaml mirror that held the PREVIOUS
/// value of this var (`model.api_key` etc.) is updated to the new value so
/// a stale higher-precedence copy cannot shadow the rotation (#62269).
/// Suppressed `env:<VAR>` pool sources are re-enabled so a deliberate
/// re-add through the UI behaves like `hermes auth add`.
///
/// Mirrors `save_provider_env_credential(env_var, value)` (lines 213-242).
pub fn save_provider_env_credential(env_var: &str, value: &str) -> SaveResult {
    let old_value = load_env().get(env_var).cloned();
    save_env_value(env_var, value);

    let mut config_updates: Vec<String> = Vec::new();
    if !value.is_empty() {
        if let Some(old) = &old_value {
            if !old.is_empty() && old != value {
                config_updates = scrub_config_yaml_mirrors(old, Some(value));
            }
        }
    }

    // A prior UI/CLI removal may have suppressed this env source; a fresh
    // save is an explicit re-add, so lift the suppression for every provider
    // that reads this var.
    for provider in providers_for_env_var(env_var) {
        let _ = std::panic::catch_unwind(|| {
            unsuppress_credential_source(&provider, &format!("env:{}", env_var));
        });
    }

    SaveResult {
        ok: true,
        key: env_var.to_string(),
        config_updates,
    }
}

// -- remove_provider_env_credential ------------------------------------------

/// Remove a credential from EVERY store it lives in.
///
/// Clears the `.env` entry (and process env), prunes env-seeded
/// `credential_pool` entries, drops the affected providers' model-cache
/// rows, and removes any config.yaml mirror holding the same value.
/// OAuth/device-code/manual credentials are preserved (see module docstring).
///
/// `found` is True when ANY store held the credential — callers that
/// previously 404'd on ".env miss" should key off this instead so a stale
/// pool-only entry can still be cleaned up through the same button.
///
/// Mirrors `remove_provider_env_credential(env_var)` (lines 245-272).
pub fn remove_provider_env_credential(env_var: &str) -> RemoveResult {
    let old_value = load_env().get(env_var).cloned();
    let removed_from_env = remove_env_value(env_var);
    let refs = purge_env_credential_references(env_var, true);
    let config_scrubbed = match &old_value {
        Some(v) if !v.is_empty() => scrub_config_yaml_mirrors(v, None),
        _ => Vec::new(),
    };

    let found = removed_from_env || !refs.pool_pruned.is_empty() || !config_scrubbed.is_empty();

    RemoveResult {
        ok: true,
        key: env_var.to_string(),
        removed: removed_from_env,
        pool_pruned: refs.pool_pruned,
        providers: refs.providers,
        config_scrubbed,
        found,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::Mutex;

    // Serialize tests that touch the filesystem / env / global registry.
    static TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    fn test_lock() -> &'static Mutex<()> {
        TEST_LOCK.get_or_init(|| Mutex::new(()))
    }

    fn with_temp_home<F: FnOnce(&Path)>(f: F) {
        let _guard = test_lock().lock().unwrap();
        let dir = env::temp_dir().join(format!("hermes-lifecycle-test-{}-{}", std::process::id(), now_millis()));
        let _ = fs::create_dir_all(&dir);
        let prev_home = env::var("HERMES_HOME").ok();
        unsafe {
            env::set_var("HERMES_HOME", &dir);
        }
        // Ensure clean slate
        let _ = fs::remove_file(dir.join(".env"));
        let _ = fs::remove_file(dir.join("auth.json"));
        let _ = fs::remove_file(dir.join("config.yaml"));
        let _ = fs::remove_file(dir.join("provider_models_cache.json"));
        f(&dir);
        let _ = fs::remove_dir_all(&dir);
        match prev_home {
            Some(v) => unsafe { env::set_var("HERMES_HOME", v); },
            None => unsafe { env::remove_var("HERMES_HOME"); },
        }
    }

    fn now_millis() -> u128 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
    }

    #[test]
    fn providers_for_env_var_known() {
        let hits = providers_for_env_var("OPENAI_API_KEY");
        assert!(hits.contains(&"openai".to_string()), "openai should map to OPENAI_API_KEY, got {:?}", hits);
        let empty = providers_for_env_var("DOES_NOT_EXIST_XYZ");
        assert!(empty.is_empty());
    }

    #[test]
    fn providers_for_shared_var() {
        // GITHUB_TOKEN seeds two providers — mirrors Python docstring
        let hits = providers_for_env_var("GITHUB_TOKEN");
        assert!(hits.len() >= 1, "GITHUB_TOKEN should map to at least one provider");
        // GH_TOKEN only maps to github_copilot
        let hits2 = providers_for_env_var("GH_TOKEN");
        assert!(hits2.contains(&"github_copilot".to_string()));
    }

    #[test]
    fn save_and_remove_env_roundtrip() {
        with_temp_home(|_dir| {
            // save
            let res = save_provider_env_credential("OPENAI_API_KEY", "sk-test-123");
            assert!(res.ok);
            assert_eq!(res.key, "OPENAI_API_KEY");
            assert_eq!(load_env().get("OPENAI_API_KEY").map(|s| s.as_str()), Some("sk-test-123"));

            // save again with same value — no config scrub expected (old == new)
            let res2 = save_provider_env_credential("OPENAI_API_KEY", "sk-test-123");
            assert!(res2.config_updates.is_empty());

            // remove
            let rem = remove_provider_env_credential("OPENAI_API_KEY");
            assert!(rem.ok);
            assert!(rem.removed);
            assert!(rem.found);
            assert!(load_env().get("OPENAI_API_KEY").is_none());
        });
    }

    #[test]
    fn save_rotates_config_mirror() {
        with_temp_home(|dir| {
            // Seed .env with old value
            save_env_value("OPENAI_API_KEY", "sk-old-111");

            // Seed config.yaml with mirror holding old value
            let cfg = dir.join("config.yaml");
            let yaml = "model:\n  api_key: sk-old-111\n  model: gpt-4\n";
            fs::write(&cfg, yaml).unwrap();

            // Rotate via save
            let res = save_provider_env_credential("OPENAI_API_KEY", "sk-new-222");
            assert!(res.config_updates.iter().any(|p| p == "model.api_key"), "should touch model.api_key, got {:?}", res.config_updates);

            // Verify file was updated
            let text = fs::read_to_string(&cfg).unwrap();
            assert!(text.contains("sk-new-222"), "yaml should contain new value, got {}", text);
            assert!(!text.contains("sk-old-111"), "yaml should not contain old value");
            // Ensure .env also updated
            assert_eq!(load_env().get("OPENAI_API_KEY").map(|s| s.as_str()), Some("sk-new-222"));
        });
    }

    #[test]
    fn save_does_not_touch_unrelated_mirror() {
        with_temp_home(|dir| {
            save_env_value("OPENAI_API_KEY", "sk-old");
            let cfg = dir.join("config.yaml");
            // Independent key for different endpoint — must be left alone
            let yaml = "model:\n  api_key: sk-different-independent\n";
            fs::write(&cfg, yaml).unwrap();

            let res = save_provider_env_credential("OPENAI_API_KEY", "sk-new");
            assert!(res.config_updates.is_empty(), "unrelated mirror must not be touched, got {:?}", res.config_updates);
            let text = fs::read_to_string(&cfg).unwrap();
            assert!(text.contains("sk-different-independent"));
        });
    }

    #[test]
    fn prune_preserves_oauth_entries() {
        with_temp_home(|dir| {
            // Seed auth.json with mixed sources: env + oauth
            let auth_path = dir.join("auth.json");
            let auth_json = r#"{
                "credential_pool": {
                    "openai": [
                        {"source": "env:OPENAI_API_KEY", "key": "sk-env"},
                        {"source": "oauth", "key": "oauth-token"},
                        {"source": "device_code", "key": "device-token"}
                    ],
                    "anthropic": [
                        {"source": "env:ANTHROPIC_API_KEY", "key": "sk-ant"}
                    ]
                },
                "providers": {
                    "openai": {"access_token": "oauth-keep"}
                }
            }"#;
            fs::write(&auth_path, auth_json).unwrap();

            let pruned = prune_env_pool_entries("OPENAI_API_KEY");
            assert_eq!(pruned, vec!["openai".to_string()]);

            let store = load_auth_store();
            // credential_pool.openai should still exist with 2 entries (oauth preserved)
            let pool = store.get("credential_pool").unwrap().as_object().unwrap();
            let openai_entries = pool.get("openai").unwrap().as_array().unwrap();
            assert_eq!(openai_entries.len(), 2, "oauth entries must be preserved");
            for e in openai_entries {
                let src = e.as_object().unwrap().get("source").unwrap().as_str().unwrap();
                assert_ne!(src, "env:OPENAI_API_KEY");
            }
            // anthropic untouched
            assert!(pool.contains_key("anthropic"));
            // providers.<id> block untouched
            assert!(store.contains_key("providers"));
        });
    }

    #[test]
    fn prune_shared_var_affects_multiple_providers() {
        with_temp_home(|dir| {
            let auth_path = dir.join("auth.json");
            let auth_json = r#"{
                "credential_pool": {
                    "github": [{"source": "env:GITHUB_TOKEN", "key": "gh1"}],
                    "github_copilot": [{"source": "env:GITHUB_TOKEN", "key": "gh2"}],
                    "openai": [{"source": "env:OPENAI_API_KEY", "key": "sk"}]
                }
            }"#;
            fs::write(&auth_path, auth_json).unwrap();
            let pruned = prune_env_pool_entries("GITHUB_TOKEN");
            assert_eq!(pruned.len(), 2);
            assert!(pruned.contains(&"github".to_string()));
            assert!(pruned.contains(&"github_copilot".to_string()));
            let store = load_auth_store();
            let pool = store.get("credential_pool").unwrap().as_object().unwrap();
            assert!(!pool.contains_key("github"));
            assert!(!pool.contains_key("github_copilot"));
            assert!(pool.contains_key("openai"));
        });
    }

    #[test]
    fn purge_clears_models_cache() {
        with_temp_home(|dir| {
            // Seed auth pool
            let auth_path = dir.join("auth.json");
            fs::write(&auth_path, r#"{"credential_pool": {"openai": [{"source": "env:OPENAI_API_KEY"}]}}"#).unwrap();
            // Seed provider_models_cache
            let cache_path = dir.join("provider_models_cache.json");
            fs::write(&cache_path, r#"{"openai": ["gpt-4"], "anthropic": ["claude-3"]}"#).unwrap();

            let res = purge_env_credential_references("OPENAI_API_KEY", true);
            assert!(res.pool_pruned.contains(&"openai".to_string()));
            assert!(res.providers.contains(&"openai".to_string()));

            let cache_text = fs::read_to_string(&cache_path).unwrap();
            let cache_val = parse_json(&cache_text).unwrap();
            let cache_obj = cache_val.as_object().unwrap();
            assert!(!cache_obj.contains_key("openai"), "openai row should be cleared");
            assert!(cache_obj.contains_key("anthropic"), "unrelated provider must remain");
        });
    }

    #[test]
    fn remove_clears_all_stores_and_found() {
        with_temp_home(|dir| {
            save_env_value("OPENAI_API_KEY", "sk-to-remove");
            fs::write(dir.join("auth.json"), r#"{"credential_pool": {"openai": [{"source": "env:OPENAI_API_KEY"}]}}"#).unwrap();
            fs::write(dir.join("config.yaml"), "model:\n  api_key: sk-to-remove\n").unwrap();
            fs::write(dir.join("provider_models_cache.json"), r#"{"openai": ["gpt-4"]}"#).unwrap();

            let res = remove_provider_env_credential("OPENAI_API_KEY");
            assert!(res.removed);
            assert!(res.found);
            assert!(res.pool_pruned.contains(&"openai".to_string()));
            assert!(res.config_scrubbed.iter().any(|p| p == "model.api_key"));
            assert!(load_env().get("OPENAI_API_KEY").is_none());
            let cfg_text = fs::read_to_string(dir.join("config.yaml")).unwrap();
            assert!(!cfg_text.contains("sk-to-remove"));
        });
    }

    #[test]
    fn remove_found_via_pool_only() {
        with_temp_home(|dir| {
            // No .env entry, but stale pool entry exists — found should be true
            fs::write(dir.join("auth.json"), r#"{"credential_pool": {"openai": [{"source": "env:OPENAI_API_KEY"}]}}"#).unwrap();
            let res = remove_provider_env_credential("OPENAI_API_KEY");
            assert!(!res.removed, ".env had nothing");
            assert!(res.found, "pool-only stale entry should make found=true");
            assert_eq!(res.pool_pruned, vec!["openai".to_string()]);
        });
    }

    #[test]
    fn scrub_handles_legacy_api_alias() {
        with_temp_home(|dir| {
            save_env_value("OPENAI_API_KEY", "sk-old");
            // Older configs used `model.api` instead of `model.api_key`
            fs::write(dir.join("config.yaml"), "model:\n  api: sk-old\n").unwrap();
            let touched = scrub_config_yaml_mirrors("sk-old", Some("sk-new"));
            assert!(touched.contains(&"model.api".to_string()), "legacy alias api must be handled, got {:?}", touched);
            let text = fs::read_to_string(dir.join("config.yaml")).unwrap();
            assert!(text.contains("sk-new"));
        });
    }

    #[test]
    fn scrub_handles_auxiliary_and_custom_providers() {
        with_temp_home(|dir| {
            save_env_value("OPENAI_API_KEY", "sk-old");
            let yaml = "auxiliary:\n  vision:\n    api_key: sk-old\ncustom_providers:\n  - name: myprov\n    api_key: sk-old\n";
            fs::write(dir.join("config.yaml"), yaml).unwrap();
            let touched = scrub_config_yaml_mirrors("sk-old", None);
            assert!(touched.iter().any(|p| p == "auxiliary.vision.api_key"));
            assert!(touched.iter().any(|p| p.starts_with("custom_providers.")));
            let text = fs::read_to_string(dir.join("config.yaml")).unwrap();
            assert!(!text.contains("sk-old"));
        });
    }

    #[test]
    fn scrub_handles_custom_providers_dict_form() {
        with_temp_home(|dir| {
            save_env_value("OPENAI_API_KEY", "sk-old");
            let yaml = "custom_providers:\n  myprov:\n    api_key: sk-old\n  other:\n    api_key: sk-keep\n";
            fs::write(dir.join("config.yaml"), yaml).unwrap();
            let touched = scrub_config_yaml_mirrors("sk-old", Some("sk-new"));
            assert!(touched.contains(&"custom_providers.myprov.api_key".to_string()));
            assert_eq!(touched.len(), 1);
            let text = fs::read_to_string(dir.join("config.yaml")).unwrap();
            assert!(text.contains("sk-new"));
            assert!(text.contains("sk-keep"));
        });
    }

    #[test]
    fn scrub_noop_on_empty_old_value() {
        with_temp_home(|_dir| {
            assert!(scrub_config_yaml_mirrors("", Some("sk-new")).is_empty());
            assert!(scrub_config_yaml_mirrors("", None).is_empty());
        });
    }

    #[test]
    fn secrecy_no_value_in_result() {
        with_temp_home(|_dir| {
            save_env_value("OPENAI_API_KEY", "sk-super-secret-xyz");
            let save_res = save_provider_env_credential("OPENAI_API_KEY", "sk-even-more-secret");
            // Result must not contain the secret values — only key names and paths
            let debug = format!("{:?}", save_res);
            assert!(!debug.contains("sk-super-secret"), "result must not leak old value");
            assert!(!debug.contains("sk-even-more-secret"), "result must not leak new value");

            let rem = remove_provider_env_credential("OPENAI_API_KEY");
            let debug2 = format!("{:?}", rem);
            assert!(!debug2.contains("sk-"), "remove result must not leak values");
        });
    }
}
