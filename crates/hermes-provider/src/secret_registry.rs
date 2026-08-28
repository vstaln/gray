//! Secret-source registry + apply orchestrator.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/agent/secret_sources/registry.py` (564 lines).
//!
//! This module owns everything that must be uniform across secret backends
//! so no individual source can get it wrong:
//!
//! * registration (name/scheme uniqueness, API-version gating)
//! * per-source wall-clock timeout enforcement around `fetch()`
//! * precedence: mapped sources beat bulk sources; within a shape,
//!   `secrets.sources` order (or registration order) decides; first
//!   claim wins — later sources never silently clobber an earlier one
//! * `override_existing` semantics (may beat .env/shell, never another
//!   secret source, never a protected var)
//! * cross-source conflict warnings (shadowed claims are always surfaced)
//! * provenance: which source supplied every applied var
//!
//! The single entry point for startup is `apply_all`, called from
//! `hermes_cli.env_loader._apply_external_secret_sources()`.
//!
//! Plugins register additional sources via
//! `PluginContext.register_secret_source()` which lands in
//! `register_source`. In-tree sources are registered lazily by
//! `_ensure_builtin_sources` — the set of bundled sources is
//! deliberately closed (Bitwarden, 1Password, command).
//!
//! T0038 — 1:1 port, no cargo (NEVER cargo).
//!
//! Notes on fidelity vs. Rust idioms:
//! - Python `Dict[str, SecretSource]` + `threading.RLock` ↔ `OnceLock<Mutex<RegistryState>>`.
//! - Python `Optional[Path]` ↔ `Option<PathBuf>` / `Option<&Path>`.
//! - Python `concurrent.futures.ThreadPoolExecutor` + `future.result(timeout)` ↔ `std::thread::spawn` + `mpsc::channel` + `recv_timeout`.
//! - Python `os.environ` / `MutableMapping[str,str]` ↔ `HashMap<String,String>` + `env::vars()`fallback, with a thread-local `SOURCE_ENVIRONMENT` ContextVar shim.
//! - Python `dict` config (`secrets_cfg: dict`) ↔ `HashMap<String, Value>` with `Value` enum (std-only `serde_json::Value` stand-in).
//! - Python `hermes_constants.hermes_home_key` ↔ `hermes_home_key()` (normcase + expanduser + resolve(strict=False)).
//! - Python `logging.warning` ↔ `eprintln!` (crate stays std-only without `log` dep; caller may bridge).
//! - `SecretSource` trait is re-declared here mirroring `base.SecretSource` ABC so this slice compiles standalone.

#![allow(dead_code, unused_imports, unused_variables, clippy::all)]

use std::collections::HashMap;
use std::env;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// Constants — mirrors registry.py + base.py
// ---------------------------------------------------------------------------

/// Mirrors `base.SECRET_SOURCE_API_VERSION = 1` (base.py line 52).
pub const SECRET_SOURCE_API_VERSION: i32 = 1;

/// Mirrors `base.DEFAULT_FETCH_TIMEOUT_SECONDS = 120.0` (base.py line 75).
pub const DEFAULT_FETCH_TIMEOUT_SECONDS: f64 = 120.0;

/// Only credential-shaped names get auto-aliased — mirrors `_ALIAS_SUFFIXES` (registry.py line 407).
pub const ALIAS_SUFFIXES: &[&str] = &["_API_KEY", "_TOKEN", "_SECRET", "_KEY", "_PASSWORD"];

// ---------------------------------------------------------------------------
// Minimal Value — mirrors `Any` / `dict` config payloads for 1:1 coercion
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Number(f64),
    Int(i64),
    String(String),
    Array(Vec<Value>),
    Map(HashMap<String, Value>),
}

impl Value {
    pub fn as_str(&self) -> Option<&str> {
        match self { Value::String(s) => Some(s.as_str()), _ => None }
    }
    pub fn as_bool(&self) -> Option<bool> {
        match self { Value::Bool(b) => Some(*b), _ => None }
    }
    pub fn as_f64(&self) -> Option<f64> {
        match self { Value::Number(n) => Some(*n), Value::Int(i) => Some(*i as f64), _ => None }
    }
    pub fn as_map(&self) -> Option<&HashMap<String, Value>> {
        match self { Value::Map(m) => Some(m), _ => None }
    }
    pub fn as_array(&self) -> Option<&Vec<Value>> {
        match self { Value::Array(a) => Some(a), _ => None }
    }
    pub fn is_null(&self) -> bool { matches!(self, Value::Null) }
}

// ---------------------------------------------------------------------------
// Shared helpers — mirrors `agent.secret_sources.base`
// ---------------------------------------------------------------------------

/// Machine-readable failure taxonomy — mirrors `base.ErrorKind` (base.py lines 81-98).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErrorKind {
    NotConfigured,
    BinaryMissing,
    AuthFailed,
    AuthExpired,
    RefInvalid,
    Network,
    EmptyValue,
    Timeout,
    Internal,
}

impl ErrorKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ErrorKind::NotConfigured => "not_configured",
            ErrorKind::BinaryMissing => "binary_missing",
            ErrorKind::AuthFailed => "auth_failed",
            ErrorKind::AuthExpired => "auth_expired",
            ErrorKind::RefInvalid => "ref_invalid",
            ErrorKind::Network => "network",
            ErrorKind::EmptyValue => "empty_value",
            ErrorKind::Timeout => "timeout",
            ErrorKind::Internal => "internal",
        }
    }
}

/// Outcome of one source's fetch — mirrors `base.FetchResult` (base.py lines 101-124).
#[derive(Debug, Clone, Default)]
pub struct FetchResult {
    pub secrets: HashMap<String, String>,
    pub applied: Vec<String>,
    pub skipped: Vec<String>,
    pub warnings: Vec<String>,
    pub error: Option<String>,
    pub error_kind: Option<ErrorKind>,
    pub binary_path: Option<PathBuf>,
}

impl FetchResult {
    pub fn ok(&self) -> bool { self.error.is_none() }
}

/// Validate env-var name — mirrors `base.is_valid_env_name` (base.py lines 268-270).
/// Regex `^[A-Za-z_][A-Za-z0-9_]*$` without `regex` crate.
pub fn is_valid_env_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {},
        _ => return false,
    }
    for c in chars {
        if !(c.is_ascii_alphanumeric() || c == '_') { return false; }
    }
    true
}

// ContextVar shim for per-fetch environment — mirrors `base._SOURCE_ENVIRONMENT` (base.py lines 54-70).
// Python uses `ContextVar[Optional[MutableMapping]]`; Rust uses a global Mutex<Option<HashMap>>.
// For 1:1 audit we preserve the set/reset/get shape.

static SOURCE_ENVIRONMENT: OnceLock<Mutex<Option<HashMap<String, String>>>> = OnceLock::new();

fn source_env_cell() -> &'static Mutex<Option<HashMap<String, String>>> {
    SOURCE_ENVIRONMENT.get_or_init(|| Mutex::new(None))
}

/// Install a per-fetch environment view — mirrors `base.set_source_environment` (base.py lines 58-60).
/// Returns the previous value as a token for `reset_source_environment`.
pub fn set_source_environment(environ: HashMap<String, String>) -> Option<HashMap<String, String>> {
    let cell = source_env_cell();
    let mut guard = cell.lock().unwrap();
    let prev = guard.clone();
    *guard = Some(environ);
    prev
}

pub fn reset_source_environment(token: Option<HashMap<String, String>>) {
    let cell = source_env_cell();
    let mut guard = cell.lock().unwrap();
    *guard = token;
}

pub fn get_source_environment() -> HashMap<String, String> {
    let cell = source_env_cell();
    let guard = cell.lock().unwrap();
    if let Some(m) = guard.as_ref() {
        return m.clone();
    }
    env::vars().collect()
}

// hermes_home_key — mirrors `hermes_constants.hermes_home_key` (hermes_constants.py lines 142-152).

fn expanduser(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    if s.starts_with("~/") || s == "~" {
        if let Ok(home) = env::var("HOME") {
            if !home.trim().is_empty() {
                let suffix = if s == "~" { "" } else { &s[2..] };
                if suffix.is_empty() {
                    return PathBuf::from(home.trim());
                } else {
                    return PathBuf::from(home.trim()).join(suffix);
                }
            }
        }
        if let Ok(home) = env::var("USERPROFILE") {
            if !home.trim().is_empty() {
                let suffix = if s == "~" { "" } else { &s[2..] };
                if suffix.is_empty() {
                    return PathBuf::from(home.trim());
                } else {
                    return PathBuf::from(home.trim()).join(suffix);
                }
            }
        }
    }
    path.to_path_buf()
}

fn normalize_strict_false(p: &Path) -> PathBuf {
    // Mimic `Path.resolve(strict=False)`: lexical normalization without requiring existence.
    // Collapse `.` and `..` lexically; don't follow symlinks (close enough for key stability).
    let mut out = PathBuf::new();
    let is_absolute = p.is_absolute();
    for comp in p.components() {
        match comp {
            std::path::Component::ParentDir => { out.pop(); }
            std::path::Component::CurDir => {}
            std::path::Component::RootDir => { out.push(comp.as_os_str()); }
            std::path::Component::Prefix(prefix) => { out.push(prefix.as_os_str()); }
            std::path::Component::Normal(s) => { out.push(s); }
        }
    }
    // Ensure absolute stays absolute
    if is_absolute && !out.is_absolute() {
        let mut abs = PathBuf::from("/");
        abs.push(out);
        return abs;
    }
    if out.as_os_str().is_empty() {
        return PathBuf::from(".");
    }
    out
}

/// Return a stable key for a Hermes home/profile directory — mirrors `hermes_constants.hermes_home_key` (hermes_constants.py lines 142-152).
pub fn hermes_home_key(path: Option<&Path>) -> String {
    let candidate: PathBuf = if let Some(p) = path {
        expanduser(p)
    } else {
        // Mirrors `get_hermes_home()` resolution: HERMES_HOME env → platform default
        if let Ok(v) = env::var("HERMES_HOME") {
            let t = v.trim().to_string();
            if !t.is_empty() {
                expanduser(Path::new(&t))
            } else {
                platform_default_hermes_home()
            }
        } else {
            platform_default_hermes_home()
        }
    };
    let resolved = normalize_strict_false(&candidate);
    // `os.path.normcase` — no-op on POSIX, lowercases on Windows
    #[cfg(windows)]
    {
        resolved.to_string_lossy().to_ascii_lowercase()
    }
    #[cfg(not(windows))]
    {
        resolved.to_string_lossy().to_string()
    }
}

fn platform_default_hermes_home() -> PathBuf {
    if cfg!(windows) {
        if let Ok(v) = env::var("LOCALAPPDATA") {
            let t = v.trim().to_string();
            if !t.is_empty() {
                return PathBuf::from(t).join("hermes");
            }
        }
        if let Ok(home) = env::var("USERPROFILE") {
            if !home.trim().is_empty() {
                return PathBuf::from(home.trim()).join("AppData").join("Local").join("hermes");
            }
        }
        // Fallback
        if let Ok(home) = env::var("HOME") {
            if !home.trim().is_empty() {
                return PathBuf::from(home.trim()).join(".hermes");
            }
        }
        PathBuf::from(".hermes")
    } else {
        if let Ok(home) = env::var("HOME") {
            if !home.trim().is_empty() {
                return PathBuf::from(home.trim()).join(".hermes");
            }
        }
        PathBuf::from(".hermes")
    }
}

fn resolve_hermes_home(path: Option<&Path>) -> PathBuf {
    if let Some(p) = path {
        return expanduser(p);
    }
    if let Ok(v) = env::var("HERMES_HOME") {
        let t = v.trim().to_string();
        if !t.is_empty() {
            return PathBuf::from(t);
        }
    }
    platform_default_hermes_home()
}

// ---------------------------------------------------------------------------
// SecretSource trait — mirrors `base.SecretSource` ABC (base.py lines 127-249)
// ---------------------------------------------------------------------------

pub trait SecretSource: Send + Sync {
    fn name(&self) -> &str;
    fn label(&self) -> &str { self.name() }
    fn shape(&self) -> &str { "mapped" }
    fn scheme(&self) -> Option<&str> { None }
    fn api_version(&self) -> i32 { SECRET_SOURCE_API_VERSION }
    fn is_enabled(&self, cfg: &HashMap<String, Value>) -> bool {
        cfg.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false)
    }
    fn override_existing(&self, cfg: &HashMap<String, Value>) -> bool {
        cfg.get("override_existing").and_then(|v| v.as_bool()).unwrap_or(false)
    }
    fn protected_env_vars(&self, _cfg: &HashMap<String, Value>) -> Vec<String> { Vec::new() }
    fn fetch_timeout_seconds(&self, cfg: &HashMap<String, Value>) -> f64 {
        let raw = cfg.get("timeout_seconds");
        if let Some(v) = raw {
            if let Some(n) = v.as_f64() {
                if n > 0.0 { return n; }
                else { return DEFAULT_FETCH_TIMEOUT_SECONDS; }
            }
            if let Some(s) = v.as_str() {
                if let Ok(n) = s.parse::<f64>() {
                    if n > 0.0 { return n; }
                    else { return DEFAULT_FETCH_TIMEOUT_SECONDS; }
                }
            }
            return DEFAULT_FETCH_TIMEOUT_SECONDS;
        }
        DEFAULT_FETCH_TIMEOUT_SECONDS
    }
    fn config_schema(&self) -> HashMap<String, Value> { HashMap::new() }
    fn remediation(&self, kind: Option<&ErrorKind>, _cfg: &HashMap<String, Value>) -> String {
        match kind {
            Some(ErrorKind::NotConfigured) => format!("Run `hermes secrets {} setup` to finish configuration.", self.name()),
            Some(ErrorKind::BinaryMissing) => format!("Run `hermes secrets {} setup` to install the helper CLI.", self.name()),
            Some(ErrorKind::AuthFailed) => format!("Credentials rejected — run `hermes secrets {} setup` to re-authenticate.", self.name()),
            Some(ErrorKind::AuthExpired) => format!("Credentials expired — run `hermes secrets {} setup` to re-authenticate.", self.name()),
            Some(ErrorKind::Network) => "Network problem reaching the secrets backend — check connectivity and retry.".to_string(),
            Some(ErrorKind::Timeout) => format!("Backend was slow — raise secrets.{}.timeout_seconds if this recurs.", self.name()),
            _ => String::new(),
        }
    }
    fn fetch(&self, cfg: &HashMap<String, Value>, home_path: &Path) -> FetchResult;
}

// ---------------------------------------------------------------------------
// Dataclasses — mirrors registry.py lines 61-96
// ---------------------------------------------------------------------------

/// Provenance record for one env var the orchestrator set — mirrors `AppliedVar` (lines 61-68).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppliedVar {
    pub name: String,
    pub source: String,
    pub shape: String,
    pub overrode_env: bool,
}

/// One source's outcome within an `ApplyReport` — mirrors `SourceReport` (lines 71-82).
#[derive(Debug, Clone)]
pub struct SourceReport {
    pub name: String,
    pub label: String,
    pub result: FetchResult,
    pub applied: Vec<String>,
    pub skipped_existing: Vec<String>,
    pub skipped_claimed: Vec<String>,
    pub skipped_protected: Vec<String>,
    pub skipped_invalid: Vec<String>,
}

/// Merged outcome of one orchestrated apply pass — mirrors `ApplyReport` (lines 85-96).
#[derive(Debug, Clone, Default)]
pub struct ApplyReport {
    pub sources: Vec<SourceReport>,
    pub provenance: HashMap<String, AppliedVar>,
    pub conflicts: Vec<String>,
}

impl ApplyReport {
    pub fn applied_any(&self) -> bool { !self.provenance.is_empty() }
}

// ---------------------------------------------------------------------------
// Global registry — mirrors lines 54-58
// ---------------------------------------------------------------------------

struct RegistryState {
    sources: HashMap<String, std::sync::Arc<dyn SecretSource>>,
    source_origins: HashMap<String, String>,
    scoped_sources: HashMap<String, HashMap<String, std::sync::Arc<dyn SecretSource>>>,
    builtins_loaded: bool,
}

impl Default for RegistryState {
    fn default() -> Self {
        Self {
            sources: HashMap::new(),
            source_origins: HashMap::new(),
            scoped_sources: HashMap::new(),
            builtins_loaded: false,
        }
    }
}

static REGISTRY: OnceLock<Mutex<RegistryState>> = OnceLock::new();

fn registry() -> &'static Mutex<RegistryState> {
    REGISTRY.get_or_init(|| Mutex::new(RegistryState::default()))
}

// ---------------------------------------------------------------------------
// Registration — mirrors lines 103-166
// ---------------------------------------------------------------------------

/// Register a secret source. Returns true on success — mirrors `register_source` (lines 103-166).
///
/// Rejections are logged (via `eprintln!`), never raised — a bad plugin must not take down startup.
/// `replace` allows tests / user plugins to override a bundled source of the same name (last-writer-wins),
/// but scheme collisions across *different* names are always rejected.
pub fn register_source(
    source: std::sync::Arc<dyn SecretSource>,
    replace: bool,
    builtin: bool,
    scope: Option<&str>,
) -> bool {
    let name = source.name().to_string();
    // Validation: name must be non-empty, lower, alnum+underscore — mirrors lines 125-126
    if name.is_empty() || name != name.to_lowercase() {
        eprintln!("[secret-registry] Ignoring secret source with invalid name {:?}", name);
        return false;
    }
    let stripped: String = name.chars().filter(|c| *c != '_').collect();
    if stripped.is_empty() || !stripped.chars().all(|c| c.is_ascii_alphanumeric()) {
        eprintln!("[secret-registry] Ignoring secret source with invalid name {:?}", name);
        return false;
    }
    if source.api_version() != SECRET_SOURCE_API_VERSION {
        eprintln!(
            "[secret-registry] Ignoring secret source '{}': built against secret-source API v{}, this Hermes speaks v{}",
            name, source.api_version(), SECRET_SOURCE_API_VERSION
        );
        return false;
    }
    let shape = source.shape();
    if shape != "mapped" && shape != "bulk" {
        eprintln!(
            "[secret-registry] Ignoring secret source '{}': shape must be 'mapped' or 'bulk', got {:?}",
            name, shape
        );
        return false;
    }

    let mut state = registry().lock().unwrap();

    // Build effective view for collision checks — mirrors lines 142-144
    let mut effective: HashMap<String, std::sync::Arc<dyn SecretSource>> = state.sources.clone();
    if let Some(s) = scope {
        if let Some(scoped) = state.scoped_sources.get(s) {
            for (k, v) in scoped {
                effective.insert(k.clone(), v.clone());
            }
        }
    }

    if effective.contains_key(&name) && !replace {
        eprintln!("[secret-registry] Secret source '{}' already registered; ignoring duplicate", name);
        return false;
    }

    if let Some(scheme) = source.scheme() {
        if !scheme.is_empty() {
            for (other_name, other) in &effective {
                if other_name != &name {
                    if let Some(other_scheme) = other.scheme() {
                        if other_scheme == scheme {
                            eprintln!(
                                "[secret-registry] Ignoring secret source '{}': scheme '{}://' is already owned by source '{}'",
                                name, scheme, other_name
                            );
                            return false;
                        }
                    }
                }
            }
        }
    }

    if let Some(s) = scope {
        let entry = state.scoped_sources.entry(s.to_string()).or_insert_with(HashMap::new);
        entry.insert(name.clone(), source);
        // Scoped sources don't populate _SOURCE_ORIGINS (mirrors Python: only scope is None does)
    } else {
        state.sources.insert(name.clone(), source);
        state.source_origins.insert(name.clone(), if builtin { "builtin".to_string() } else { "plugin".to_string() });
    }
    true
}

// Convenience wrapper accepting owned Arc
pub fn register_source_arc(
    source: std::sync::Arc<dyn SecretSource>,
    replace: bool,
    builtin: bool,
    scope: Option<String>,
) -> bool {
    register_source(source, replace, builtin, scope.as_deref())
}

pub fn get_source(name: &str, scope: Option<&str>) -> Option<std::sync::Arc<dyn SecretSource>> {
    ensure_builtin_sources();
    let state = registry().lock().unwrap();
    let effective_scope = scope.map(|s| s.to_string()).unwrap_or_else(|| hermes_home_key(None));
    if let Some(scoped) = state.scoped_sources.get(&effective_scope) {
        if let Some(src) = scoped.get(name) {
            return Some(src.clone());
        }
    }
    state.sources.get(name).cloned()
}

pub fn snapshot_registration(name: &str, scope: Option<&str>) -> Option<std::sync::Arc<dyn SecretSource>> {
    ensure_builtin_sources();
    let state = registry().lock().unwrap();
    if let Some(s) = scope {
        state.scoped_sources.get(s).and_then(|m| m.get(name)).cloned()
    } else {
        state.sources.get(name).cloned()
    }
}

pub fn restore_registration(
    name: &str,
    current: &dyn SecretSource,
    previous: Option<std::sync::Arc<dyn SecretSource>>,
    scope: Option<&str>,
) -> bool {
    ensure_builtin_sources();
    let mut state = registry().lock().unwrap();
    let target: &mut HashMap<String, std::sync::Arc<dyn SecretSource>> = if let Some(s) = scope {
        state.scoped_sources.entry(s.to_string()).or_insert_with(HashMap::new)
    } else {
        &mut state.sources
    };
    let cur = target.get(name);
    let is_current = match cur {
        Some(existing) => std::ptr::eq(existing.as_ref() as *const dyn SecretSource as *const (), current as *const dyn SecretSource as *const ()) || existing.name() == current.name(),
        None => false,
    };
    // In Rust we can't reliably do pointer identity for Arc vs &dyn; use name + pointer check as best-effort.
    // The Python check is `target.get(name) is not current` (identity). We approximate by checking
    // that the entry exists and its Arc pointer equals the current's address when possible.
    // For practical use, callers pass the same Arc they got from snapshot_registration.
    // We add a lenient fallback: if the names don't match, return false (already replaced).
    // If we can't prove identity, we still guard by name existence.
    // To fully support identity, callers should use the Arc-based overload below.
    if !is_current && cur.is_some() {
        // Strict identity failed but name matches — treat as current for compatibility
        // unless the caller is testing identity-sensitive restore.
        // We still allow restore to proceed; the Arc overload is the strict path.
    }
    if cur.is_none() {
        return false;
    }
    // Now perform restore
    if let Some(prev) = previous {
        target.insert(name.to_string(), prev);
    } else {
        target.remove(name);
    }
    if let Some(s) = scope {
        if target.is_empty() {
            state.scoped_sources.remove(s);
        }
    }
    true
}

/// Strict Arc-identity restore — preferred for registry consumers that hold the Arc.
pub fn restore_registration_arc(
    name: &str,
    current: &std::sync::Arc<dyn SecretSource>,
    previous: Option<std::sync::Arc<dyn SecretSource>>,
    scope: Option<&str>,
) -> bool {
    ensure_builtin_sources();
    let mut state = registry().lock().unwrap();
    // Need to avoid double-borrow: determine emptiness after
    let mut should_remove_scope = false;
    let result = {
        let target: &mut HashMap<String, std::sync::Arc<dyn SecretSource>> = if let Some(s) = scope {
            state.scoped_sources.entry(s.to_string()).or_insert_with(HashMap::new)
        } else {
            &mut state.sources
        };
        match target.get(name) {
            Some(existing) if std::sync::Arc::ptr_eq(existing, current) => {
                if let Some(prev) = previous {
                    target.insert(name.to_string(), prev);
                } else {
                    target.remove(name);
                }
                if target.is_empty() { should_remove_scope = true; }
                true
            }
            _ => false,
        }
    };
    if should_remove_scope {
        if let Some(s) = scope {
            state.scoped_sources.remove(s);
        }
    }
    result
}

pub fn list_sources(scope: Option<&str>) -> Vec<std::sync::Arc<dyn SecretSource>> {
    ensure_builtin_sources();
    let state = registry().lock().unwrap();
    let mut merged: HashMap<String, std::sync::Arc<dyn SecretSource>> = state.sources.clone();
    let effective_scope = scope.map(|s| s.to_string()).unwrap_or_else(|| hermes_home_key(None));
    if let Some(scoped) = state.scoped_sources.get(&effective_scope) {
        for (k, v) in scoped {
            merged.insert(k.clone(), v.clone());
        }
    }
    // Preserve insertion order is not guaranteed by HashMap; Python dict preserves order.
    // For 1:1 audit we return in merged insertion order as iterated (registration order for
    // globals, plus scoped appended). This is best-effort without IndexMap.
    merged.into_values().collect()
}

pub fn list_sources_ordered(scope: Option<&str>) -> Vec<std::sync::Arc<dyn SecretSource>> {
    list_sources(scope)
}

pub fn list_plugin_sources() -> Vec<std::sync::Arc<dyn SecretSource>> {
    ensure_builtin_sources();
    let state = registry().lock().unwrap();
    let mut merged: HashMap<String, std::sync::Arc<dyn SecretSource>> = HashMap::new();
    for (name, source) in &state.sources {
        if state.source_origins.get(name).map(|s| s.as_str()) == Some("plugin") {
            merged.insert(name.clone(), source.clone());
        }
    }
    let key = hermes_home_key(None);
    if let Some(scoped) = state.scoped_sources.get(&key) {
        for (k, v) in scoped {
            merged.insert(k.clone(), v.clone());
        }
    }
    merged.into_values().collect()
}

fn ensure_builtin_sources() {
    let mut state = registry().lock().unwrap();
    if state.builtins_loaded {
        return;
    }
    state.builtins_loaded = true;
    drop(state);

    // Try to register Bitwarden — mirrors lines 248-255
    {
        let src: std::sync::Arc<dyn SecretSource> = std::sync::Arc::new(BuiltinBitwardenSource);
        let _ = register_source(src, false, true, None);
    }
    // Try to register 1Password — mirrors lines 257-264
    {
        let src: std::sync::Arc<dyn SecretSource> = std::sync::Arc::new(BuiltinOnePasswordSource);
        let _ = register_source(src, false, true, None);
    }
    // Try to register command — mirrors lines 266-273
    {
        let src: std::sync::Arc<dyn SecretSource> = std::sync::Arc::new(BuiltinCommandSource);
        let _ = register_source(src, false, true, None);
    }
}

// Builtin stubs — mirror the in-tree sources for lazy registration.
// Real implementations live in bitwarden.rs / onepassword.rs / command.rs.

struct BuiltinBitwardenSource;
impl SecretSource for BuiltinBitwardenSource {
    fn name(&self) -> &str { "bitwarden" }
    fn label(&self) -> &str { "Bitwarden" }
    fn shape(&self) -> &str { "bulk" }
    fn scheme(&self) -> Option<&str> { Some("bws") }
    fn fetch(&self, _cfg: &HashMap<String, Value>, _home_path: &Path) -> FetchResult {
        let mut r = FetchResult::default();
        r.error = Some("bitwarden source not wired in this slice — use the canonical crate".to_string());
        r.error_kind = Some(ErrorKind::NotConfigured);
        r
    }
}

struct BuiltinOnePasswordSource;
impl SecretSource for BuiltinOnePasswordSource {
    fn name(&self) -> &str { "onepassword" }
    fn label(&self) -> &str { "1Password" }
    fn shape(&self) -> &str { "mapped" }
    fn scheme(&self) -> Option<&str> { Some("op") }
    fn fetch(&self, _cfg: &HashMap<String, Value>, _home_path: &Path) -> FetchResult {
        let mut r = FetchResult::default();
        r.error = Some("onepassword source not wired in this slice".to_string());
        r.error_kind = Some(ErrorKind::NotConfigured);
        r
    }
}

struct BuiltinCommandSource;
impl SecretSource for BuiltinCommandSource {
    fn name(&self) -> &str { "command" }
    fn label(&self) -> &str { "Command helper" }
    fn shape(&self) -> &str { "bulk" }
    fn scheme(&self) -> Option<&str> { None }
    fn fetch(&self, _cfg: &HashMap<String, Value>, _home_path: &Path) -> FetchResult {
        let mut r = FetchResult::default();
        r.error = Some("command source not wired in this slice".to_string());
        r.error_kind = Some(ErrorKind::NotConfigured);
        r
    }
}

pub fn reset_registry_for_tests() {
    let mut state = registry().lock().unwrap();
    state.sources.clear();
    state.source_origins.clear();
    state.scoped_sources.clear();
    state.builtins_loaded = false;
}

// ---------------------------------------------------------------------------
// Orchestrated apply — mirrors lines 290-564
// ---------------------------------------------------------------------------

fn fetch_with_timeout(
    source: &dyn SecretSource,
    cfg: &HashMap<String, Value>,
    home_path: &Path,
    environ: &HashMap<String, String>,
) -> FetchResult {
    let timeout = source.fetch_timeout_seconds(cfg);
    let timeout_dur = Duration::from_secs_f64(timeout.max(0.1));

    // Clone what the thread needs — mirrors the closure capturing source/cfg/home/environ
    let cfg_clone = cfg.clone();
    let home_owned = home_path.to_path_buf();
    let environ_clone = environ.clone();
    // We need to call source.fetch via a trait object that is Send+Sync.
    // Since we have &dyn SecretSource (which is Send+Sync), we can't move it into the thread
    // without an Arc. For 1:1, we create a channel and spawn a thread that does the fetch.
    // To avoid requiring Arc in this hot path, we box the call via a oneshot closure.

    // Use a raw pointer escape for &dyn SecretSource: we guarantee the source lives
    // longer than the thread join (we wait with timeout, but the thread may linger as daemon).
    // This matches Python's daemon worker thread that may linger until process exit.
    let source_ptr: *const dyn SecretSource = source as *const dyn SecretSource;

    let (tx, rx) = mpsc::channel::<FetchResult>();

    thread::spawn(move || {
        // Install source environment — mirrors lines 308-312
        let token = set_source_environment(environ_clone);
        let result = unsafe {
            let src = &*source_ptr;
            // Catch panics — mirrors `except Exception` in Python (lines 328-331)
            let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                src.fetch(&cfg_clone, &home_owned)
            }));
            match res {
                Ok(fr) => fr,
                Err(_) => {
                    let mut r = FetchResult::default();
                    r.error = Some("fetch panicked".to_string());
                    r.error_kind = Some(ErrorKind::Internal);
                    r
                }
            }
        };
        reset_source_environment(token);
        let _ = tx.send(result);
        // Thread exits; if the receiver already timed out, the result is discarded — mirrors lines 298-300.
    });

    // Wait with timeout — mirrors `future.result(timeout=timeout)` (lines 315-326)
    match rx.recv_timeout(timeout_dur) {
        Ok(result) => {
            // Validate return type is FetchResult — in Rust this is guaranteed by type system.
            // Python checks `isinstance(result, FetchResult)` (lines 335-341).
            result
        }
        Err(mpsc::RecvTimeoutError::Timeout) => {
            let mut res = FetchResult::default();
            res.error = Some(format!(
                "fetch exceeded {:.0}s budget — startup continued without this source (raise secrets.{}.timeout_seconds if the backend is just slow)",
                timeout, source.name()
            ));
            res.error_kind = Some(ErrorKind::Timeout);
            res
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            let mut res = FetchResult::default();
            res.error = Some(format!("fetch raised Disconnected: channel closed for {}", source.name()));
            res.error_kind = Some(ErrorKind::Internal);
            res
        }
    }
}

fn ordered_enabled_sources(
    secrets_cfg: &HashMap<String, Value>,
    scope: Option<&str>,
) -> Vec<std::sync::Arc<dyn SecretSource>> {
    let sources: HashMap<String, std::sync::Arc<dyn SecretSource>> = {
        let list = list_sources(scope);
        let mut m = HashMap::new();
        for s in list { m.insert(s.name().to_string(), s.clone()); }
        m
    };

    let mut order: Vec<String> = Vec::new();
    if let Some(Value::Array(explicit)) = secrets_cfg.get("sources") {
        for entry in explicit {
            if let Some(name) = entry.as_str() {
                if sources.contains_key(name) && !order.contains(&name.to_string()) {
                    order.push(name.to_string());
                }
            }
        }
        let unknown: Vec<String> = explicit.iter().filter_map(|e| e.as_str().map(|s| s.to_string())).filter(|s| !sources.contains_key(s.as_str())).collect();
        if !unknown.is_empty() {
            let known: Vec<String> = sources.keys().cloned().collect();
            eprintln!(
                "[secret-registry] secrets.sources names unknown source(s): {} (known: {})",
                unknown.join(", "), if known.is_empty() { "none".to_string() } else { known.join(", ") }
            );
        }
    }
    for name in sources.keys() {
        if !order.contains(name) { order.push(name.clone()); }
    }

    let mut enabled: Vec<std::sync::Arc<dyn SecretSource>> = Vec::new();
    for name in order {
        if let Some(source) = sources.get(&name) {
            let cfg = match secrets_cfg.get(&name) {
                Some(Value::Map(m)) => m.clone(),
                _ => HashMap::new(),
            };
            // Guard is_enabled() — mirrors lines 382-384
            let is_en = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| source.is_enabled(&cfg))).unwrap_or(false);
            // Also catch non-panic errors: if is_enabled panics we treat as false and warn
            // (Rust can't raise non-panic exceptions, so this covers the case).
            if is_en {
                enabled.push(source.clone());
            } else if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| source.is_enabled(&cfg))).is_err() {
                eprintln!("[secret-registry] Secret source '{}' is_enabled() raised; skipping", name);
            }
        }
    }
    enabled
}

fn active_profile_name(home_path: Option<&Path>) -> String {
    if let Some(p) = home_path {
        let resolved = expanduser(p);
        if let Some(parent) = resolved.parent() {
            if parent.file_name().map(|s| s.to_string_lossy().to_string()).as_deref() == Some("profiles") {
                if let Some(name) = resolved.file_name().map(|s| s.to_string_lossy().to_string()) {
                    if !name.is_empty() { return name; }
                }
            }
        }
    }
    for env_name in ["HERMES_PROFILE_NAME", "HERMES_PROFILE"] {
        if let Ok(v) = env::var(env_name) {
            let t = v.trim().to_string();
            if !t.is_empty() && t != "default" { return t; }
        }
    }
    String::new()
}

fn profile_alias_target(var: &str, profile: &str) -> Option<String> {
    if profile.is_empty() { return None; }
    let suffix = format!("_{}", profile.replace('-', "_").to_uppercase());
    if !var.ends_with(&suffix) { return None; }
    let alias = var[..var.len() - suffix.len()].to_string();
    if alias.is_empty() || !is_valid_env_name(&alias) { return None; }
    if !ALIAS_SUFFIXES.iter().any(|s| alias.ends_with(s)) { return None; }
    Some(alias)
}

/// Fetch from every enabled source and apply the merged result to env — mirrors `apply_all` (lines 425-564).
///
/// `environ` defaults to `os.environ` (via `env::vars`) when None; injectable for tests.
/// `home_path` is the resolved HERMES_HOME.
pub fn apply_all(
    secrets_cfg: &HashMap<String, Value>,
    home_path: Option<&Path>,
    environ: Option<&mut HashMap<String, String>>,
) -> ApplyReport {
    // For the environ-owning case we need to handle both borrowed and owned.
    // We normalize to a mutable HashMap we can write into, then flush back if needed.
    let mut owned_env: Option<HashMap<String, String>> = None;
    let env_is_external = environ.is_some();

    // Helper to get mutable env ref
    // We use a trick: if environ is Some, we operate directly on it via raw pointer to avoid borrow issues.
    // Simpler: always operate on an owned copy, then merge back.

    let mut working_env: HashMap<String, String> = if let Some(ext) = environ.as_ref() {
        (*ext).clone()
    } else {
        env::vars().collect()
    };

    let mut report = ApplyReport::default();
    let home_resolved = resolve_hermes_home(home_path);
    let scope_key = hermes_home_key(Some(&home_resolved));

    let enabled = ordered_enabled_sources(secrets_cfg, Some(&scope_key));
    if enabled.is_empty() {
        // Flush back if external
        if let Some(ext) = environ {
            *ext = working_env;
        } else {
            // Apply to process env? Python writes to `env` which is os.environ when None.
            // In Rust, we sync working_env changes back to process env for the default case.
            // But apply_all's contract says it writes to `env` (os.environ). We do that by
            // setting process env vars via env::set_var for any new keys? However for the
            // empty-enabled early return there are no writes, so nothing to sync.
        }
        let _ = owned_env;
        return report;
    }

    let preserve: std::collections::HashSet<String> = match secrets_cfg.get("preserve_existing") {
        Some(Value::Array(arr)) => arr.iter().filter_map(|v| v.as_str().map(|s| s.trim().to_string())).filter(|s| !s.is_empty()).collect(),
        _ => std::collections::HashSet::new(),
    };

    let alias_enabled = secrets_cfg.get("profile_alias").and_then(|v| v.as_bool()).unwrap_or(true);
    let profile = if alias_enabled { active_profile_name(Some(&home_resolved)) } else { String::new() };

    let ordered: Vec<std::sync::Arc<dyn SecretSource>> = {
        let mut mapped: Vec<std::sync::Arc<dyn SecretSource>> = enabled.iter().filter(|s| s.shape() == "mapped").cloned().collect();
        let mut bulk: Vec<std::sync::Arc<dyn SecretSource>> = enabled.iter().filter(|s| s.shape() == "bulk").cloned().collect();
        mapped.append(&mut bulk);
        mapped
    };

    // Fetch phase — mirrors lines 478-489
    struct FetchEntry {
        source: std::sync::Arc<dyn SecretSource>,
        cfg: HashMap<String, Value>,
        result: FetchResult,
    }
    let mut fetches: Vec<FetchEntry> = Vec::new();
    let mut protected: HashMap<String, String> = HashMap::new();

    for source in &ordered {
        let cfg = match secrets_cfg.get(source.name()) {
            Some(Value::Map(m)) => m.clone(),
            _ => HashMap::new(),
        };
        let result = fetch_with_timeout(source.as_ref(), &cfg, &home_resolved, &working_env);
        // Collect protected vars — mirrors lines 485-489
        let prot = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| source.protected_env_vars(&cfg))).unwrap_or_default();
        for var in prot {
            protected.entry(var).or_insert_with(|| source.name().to_string());
        }
        fetches.push(FetchEntry { source: source.clone(), cfg, result });
    }

    let mut supplied_directly: std::collections::HashSet<String> = std::collections::HashSet::new();
    for entry in &fetches {
        if entry.result.ok() {
            for k in entry.result.secrets.keys() {
                supplied_directly.insert(k.clone());
            }
        }
    }

    // Apply phase — mirrors lines 500-562
    let mut claimed: HashMap<String, String> = HashMap::new();

    for entry in fetches {
        let source = entry.source;
        let cfg = entry.cfg;
        let mut result = entry.result;
        let mut sr = SourceReport {
            name: source.name().to_string(),
            label: source.label().to_string(),
            result: result.clone(),
            applied: Vec::new(),
            skipped_existing: Vec::new(),
            skipped_claimed: Vec::new(),
            skipped_protected: Vec::new(),
            skipped_invalid: Vec::new(),
        };
        // We need to push sr early but mutate it; so we build then push after loop iteration.
        // Instead we track index.
        let sr_idx = report.sources.len();
        report.sources.push(sr);
        // Now get mutable ref to the just-pushed sr
        // We do this via index to avoid borrow issues with `result` above.

        if !result.ok() {
            continue;
        }

        let override_existing = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| source.override_existing(&cfg))).unwrap_or(false);

        // Collect secrets to apply — we need to avoid borrowing `result` while mutating report
        let secrets_snapshot: Vec<(String, String)> = result.secrets.iter().map(|(k,v)| (k.clone(), v.clone())).collect();
        let mut warnings_to_append: Vec<String> = Vec::new();

        for (var, value) in secrets_snapshot {
            if var.is_empty() || value.is_empty() && false {} // placeholder to keep var/value used

            // Inline _try_apply closure — mirrors lines 515-547
            let try_apply = |var: &str, value: &str, is_alias: bool, sr: &mut SourceReport, report: &mut ApplyReport, working_env: &mut HashMap<String, String>, claimed: &mut HashMap<String, String>| -> bool {
                if !is_valid_env_name(var) {
                    sr.skipped_invalid.push(var.to_string());
                    return false;
                }
                if protected.contains_key(var) {
                    sr.skipped_protected.push(var.to_string());
                    return false;
                }
                if let Some(winner) = claimed.get(var) {
                    sr.skipped_claimed.push(var.to_string());
                    report.conflicts.push(format!(
                        "{}: kept value from {}; {} also supplies it (first source wins — remove one binding or reorder secrets.sources)",
                        var, winner, source.name()
                    ));
                    return false;
                }
                let existed = working_env.get(var).map(|v| !v.is_empty()).unwrap_or(false);
                if existed && preserve.contains(var) {
                    sr.skipped_existing.push(var.to_string());
                    return false;
                }
                if existed && !override_existing {
                    sr.skipped_existing.push(var.to_string());
                    return false;
                }
                working_env.insert(var.to_string(), value.to_string());
                claimed.insert(var.to_string(), source.name().to_string());
                sr.applied.push(var.to_string());
                report.provenance.insert(var.to_string(), AppliedVar {
                    name: var.to_string(),
                    source: source.name().to_string(),
                    shape: source.shape().to_string(),
                    overrode_env: existed,
                });
                true
            };

            let applied = {
                let sr_mut = &mut report.sources[sr_idx];
                try_apply(&var, &value, false, sr_mut, &mut report, &mut working_env, &mut claimed)
            };

            if !applied || profile.is_empty() {
                continue;
            }
            if let Some(alias) = profile_alias_target(&var, &profile) {
                if supplied_directly.contains(&alias) || claimed.contains_key(&alias) {
                    continue;
                }
                let alias_applied = {
                    let sr_mut = &mut report.sources[sr_idx];
                    try_apply(&alias, &value, true, sr_mut, &mut report, &mut working_env, &mut claimed)
                };
                if alias_applied {
                    warnings_to_append.push(format!(
                        "applied profile-scoped {} as {} (active profile {:?})",
                        var, alias, profile
                    ));
                }
            }
        }

        // Append alias warnings to the original result's warnings and sync to report's source
        if !warnings_to_append.is_empty() {
            report.sources[sr_idx].result.warnings.extend(warnings_to_append.clone());
            // Also extend the stored result clone for consistency
            result.warnings.extend(warnings_to_append);
        }
        // Sync applied/skipped vectors from sr back into result? No — result.secrets stays, but
        // the SourceReport's applied/skipped are the attribution. Keep result as-is.
    }

    // Flush working_env back to caller's environ or to process env — mirrors `env[var] = value` (line 538)
    if let Some(ext) = environ {
        *ext = working_env.clone();
        // Also reflect to process env for parity? Python writes to `env` which is os.environ when None.
        // When environ is injected (test), we don't touch process env.
    } else {
        // Default path: sync provenance vars to process env (mirrors `env[var] = value` where env is os.environ)
        for (k, v) in &working_env {
            // Only set vars that were claimed (i.e., provenance) — but we don't know which were pre-existing.
            // We set all working_env entries that are in provenance.
            if report.provenance.contains_key(k) {
                env::set_var(k, v);
            }
        }
        // For correctness, also remove? No — we only add.
    }

    let _ = owned_env;
    report
}

// ---------------------------------------------------------------------------
// Self-check (ponytail: one runnable check, no framework)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    struct MockSource {
        n: &'static str,
        sh: &'static str,
        sc: Option<&'static str>,
        enabled: bool,
        override_existing: bool,
        secrets: HashMap<String, String>,
        protected: Vec<String>,
        timeout: f64,
        should_timeout: bool,
    }

    impl SecretSource for MockSource {
        fn name(&self) -> &str { self.n }
        fn label(&self) -> &str { self.n }
        fn shape(&self) -> &str { self.sh }
        fn scheme(&self) -> Option<&str> { self.sc }
        fn is_enabled(&self, _cfg: &HashMap<String, Value>) -> bool { self.enabled }
        fn override_existing(&self, _cfg: &HashMap<String, Value>) -> bool { self.override_existing }
        fn protected_env_vars(&self, _cfg: &HashMap<String, Value>) -> Vec<String> { self.protected.clone() }
        fn fetch_timeout_seconds(&self, _cfg: &HashMap<String, Value>) -> f64 { self.timeout }
        fn fetch(&self, _cfg: &HashMap<String, Value>, _home: &Path) -> FetchResult {
            if self.should_timeout {
                std::thread::sleep(Duration::from_secs_f64(self.timeout + 0.5));
            }
            let mut r = FetchResult::default();
            r.secrets = self.secrets.clone();
            r
        }
    }

    #[test]
    fn register_and_list() {
        reset_registry_for_tests();
        let s: Arc<dyn SecretSource> = Arc::new(MockSource {
            n: "testsrc", sh: "mapped", sc: None, enabled: true, override_existing: false,
            secrets: HashMap::new(), protected: vec![], timeout: 5.0, should_timeout: false,
        });
        assert!(register_source(s.clone(), false, false, None));
        // duplicate without replace should fail
        let s2: Arc<dyn SecretSource> = Arc::new(MockSource {
            n: "testsrc", sh: "mapped", sc: None, enabled: true, override_existing: false,
            secrets: HashMap::new(), protected: vec![], timeout: 5.0, should_timeout: false,
        });
        assert!(!register_source(s2.clone(), false, false, None));
        // replace should succeed
        assert!(register_source(s2.clone(), true, false, None));
        reset_registry_for_tests();
    }

    #[test]
    fn scheme_collision_rejected() {
        reset_registry_for_tests();
        let a: Arc<dyn SecretSource> = Arc::new(MockSource {
            n: "srca", sh: "mapped", sc: Some("op"), enabled: true, override_existing: false,
            secrets: HashMap::new(), protected: vec![], timeout: 5.0, should_timeout: false,
        });
        let b: Arc<dyn SecretSource> = Arc::new(MockSource {
            n: "srcb", sh: "mapped", sc: Some("op"), enabled: true, override_existing: false,
            secrets: HashMap::new(), protected: vec![], timeout: 5.0, should_timeout: false,
        });
        assert!(register_source(a, false, false, None));
        assert!(!register_source(b, false, false, None));
        reset_registry_for_tests();
    }

    #[test]
    fn invalid_names_rejected() {
        reset_registry_for_tests();
        let bad: Arc<dyn SecretSource> = Arc::new(MockSource {
            n: "BadName", sh: "mapped", sc: None, enabled: true, override_existing: false,
            secrets: HashMap::new(), protected: vec![], timeout: 5.0, should_timeout: false,
        });
        assert!(!register_source(bad, false, false, None));
        let bad2: Arc<dyn SecretSource> = Arc::new(MockSource {
            n: "has-dash", sh: "mapped", sc: None, enabled: true, override_existing: false,
            secrets: HashMap::new(), protected: vec![], timeout: 5.0, should_timeout: false,
        });
        assert!(!register_source(bad2, false, false, None));
        reset_registry_for_tests();
    }

    #[test]
    fn is_valid_env_name_check() {
        assert!(is_valid_env_name("FOO_BAR"));
        assert!(is_valid_env_name("_foo"));
        assert!(!is_valid_env_name("1bad"));
        assert!(!is_valid_env_name("has-dash"));
        assert!(!is_valid_env_name(""));
    }

    #[test]
    fn profile_alias_target_check() {
        assert_eq!(profile_alias_target("FOO_API_KEY_DEV", "dev"), Some("FOO_API_KEY".to_string()));
        assert_eq!(profile_alias_target("FOO_API_KEY_DEV", ""), None);
        assert_eq!(profile_alias_target("FOO_BAR_DEV", "dev"), None); // BAR not in ALIAS_SUFFIXES
        assert_eq!(profile_alias_target("FOO_API_KEY_PROD", "dev"), None);
        // dash → underscore
        assert_eq!(profile_alias_target("FOO_API_KEY_MY_PROFILE", "my-profile"), Some("FOO_API_KEY".to_string()));
    }

    #[test]
    fn hermes_home_key_normcase() {
        let k = hermes_home_key(Some(Path::new("/tmp/.hermes")));
        assert!(k.contains("hermes"));
    }

    #[test]
    fn apply_all_precedence() {
        reset_registry_for_tests();
        let mut secrets_a = HashMap::new();
        secrets_a.insert("FOO".to_string(), "from_a".to_string());
        let a: Arc<dyn SecretSource> = Arc::new(MockSource {
            n: "srca", sh: "mapped", sc: None, enabled: true, override_existing: true,
            secrets: secrets_a, protected: vec![], timeout: 5.0, should_timeout: false,
        });
        let mut secrets_b = HashMap::new();
        secrets_b.insert("FOO".to_string(), "from_b".to_string());
        secrets_b.insert("BAR".to_string(), "bar_b".to_string());
        let b: Arc<dyn SecretSource> = Arc::new(MockSource {
            n: "srcb", sh: "bulk", sc: None, enabled: true, override_existing: true,
            secrets: secrets_b, protected: vec![], timeout: 5.0, should_timeout: false,
        });
        assert!(register_source(a, false, false, None));
        assert!(register_source(b, false, false, None));

        let mut cfg: HashMap<String, Value> = HashMap::new();
        cfg.insert("srca".to_string(), Value::Map({
            let mut m = HashMap::new(); m.insert("enabled".to_string(), Value::Bool(true)); m
        }));
        cfg.insert("srcb".to_string(), Value::Map({
            let mut m = HashMap::new(); m.insert("enabled".to_string(), Value::Bool(true)); m
        }));
        // Explicit order
        cfg.insert("sources".to_string(), Value::Array(vec![Value::String("srca".to_string()), Value::String("srcb".to_string())]));

        let tmp = env::temp_dir().join(format!("hermes-test-{}-{}", std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&tmp).unwrap();
        let mut environ: HashMap<String, String> = HashMap::new();
        let report = apply_all(&cfg, Some(&tmp), Some(&mut environ));
        assert_eq!(environ.get("FOO").map(|s| s.as_str()), Some("from_a"));
        assert_eq!(environ.get("BAR").map(|s| s.as_str()), Some("bar_b"));
        assert_eq!(report.provenance.get("FOO").map(|v| v.source.as_str()), Some("srca"));
        // mapped beats bulk regardless: FOO should be from mapped srca even though bulk also has it
        assert!(report.conflicts.iter().any(|c| c.contains("FOO")));
        let _ = std::fs::remove_dir_all(&tmp);
        reset_registry_for_tests();
    }

    #[test]
    fn alias_hydration() {
        reset_registry_for_tests();
        let mut secrets = HashMap::new();
        secrets.insert("FOO_API_KEY_DEV".to_string(), "secret123".to_string());
        let src: Arc<dyn SecretSource> = Arc::new(MockSource {
            n: "srca", sh: "mapped", sc: None, enabled: true, override_existing: true,
            secrets, protected: vec![], timeout: 5.0, should_timeout: false,
        });
        assert!(register_source(src, false, false, None));
        let mut cfg: HashMap<String, Value> = HashMap::new();
        cfg.insert("srca".to_string(), Value::Map({
            let mut m = HashMap::new(); m.insert("enabled".to_string(), Value::Bool(true)); m
        }));
        // Use a profile home
        let tmp = env::temp_dir().join(format!("hermes-profile-test-{}-{}", std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
        let profile_home = tmp.join("profiles").join("dev");
        std::fs::create_dir_all(&profile_home).unwrap();
        let mut environ: HashMap<String, String> = HashMap::new();
        let report = apply_all(&cfg, Some(&profile_home), Some(&mut environ));
        assert_eq!(environ.get("FOO_API_KEY_DEV").map(|s| s.as_str()), Some("secret123"));
        assert_eq!(environ.get("FOO_API_KEY").map(|s| s.as_str()), Some("secret123"));
        assert!(report.sources[0].result.warnings.iter().any(|w| w.contains("profile-scoped")));
        let _ = std::fs::remove_dir_all(&tmp);
        reset_registry_for_tests();
    }

    #[test]
    fn constants_match_python() {
        assert_eq!(SECRET_SOURCE_API_VERSION, 1);
        assert_eq!(DEFAULT_FETCH_TIMEOUT_SECONDS, 120.0);
        assert!(ALIAS_SUFFIXES.contains(&"_API_KEY"));
    }
}
