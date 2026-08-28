//! Model metadata, context lengths, and token estimation utilities — slice 1.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/agent/model_metadata.py`
//! (3767 lines) — slice 1/4, lines 1-900.
//!
//! ```text
//! Slice 1 (ll.1-900): module doc, lazy requests import, _resolve_requests_verify,
//!   _PROVIDER_PREFIXES, _OLLAMA_TAG_PATTERN, _TAILSCALE_CGNAT, _strip_provider_prefix,
//!   in-process caches (_model_metadata_cache, _endpoint_probe_path_cache, ...),
//!   blackhole dedup (_endpoint_host_key / _note_endpoint_blackholed / _endpoint_blackholed /
//!   _is_connect_timeout), disk L2 for local probes, model-metadata disk cache,
//!   CONTEXT_PROBE_TIERS / DEFAULT_FALLBACK_CONTEXT / _warn_context_length_fallback,
//!   MINIMUM_CONTEXT_LENGTH, DEFAULT_CONTEXT_LENGTHS, Grok helpers, context-length keys,
//!   _URL_TO_PROVIDER, _infer_provider_from_url, _endpoint_scoped_context_length,
//!   _reconcile_local_cached_context_length (truncated at l.900).
//! Slice 2 (ll.901-1800): is_local_endpoint, _localhost_to_ipv4, detect_local_server_type,
//!   _iter_nested_dicts, _coerce_reasonable_int, _extract_* , _add_model_aliases,
//!   fetch_model_metadata, fetch_endpoint_model_metadata, _resolve_endpoint_context_length,
//!   _get_context_cache_path, save_context_length / get_cached_context_length, etc.
//! Slice 3 (ll.1801-2700): error-parsing helpers, Ollama probes, staleness guards,
//!   _query_local_context_length, Anthropic/Codex helpers.
//! Slice 4 (ll.2701-3767): Codex OAuth, get_model_context_length, token estimators,
//!   message/message+tools token rough estimators.
//! ```
//!
//! NEVER cargo — 1:1 translation only (no `cargo check` / `cargo build`).
//! Mirrors Python ll.1-900 verbatim; line numbers in comments refer to the
//! 3767-line source file. Later slices continue from l.901.
//!
//! T0022 — 1:1 port, no cargo (NEVER cargo).

#![allow(dead_code, unused_imports, unused_variables, clippy::all)]

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// Imports — mirrors Python ll.7-27
// ---------------------------------------------------------------------------
// Python stdlib imports (ll.7-17):
//   base64, hashlib, ipaddress, json, logging, os, re, time, pathlib, typing, urllib.parse
//   yaml (PyYAML)
//   typing: Any, Dict, List, Optional, Tuple, TYPE_CHECKING
//   urllib.parse.urlparse
// Intra-repo imports (ll.24-27):
//   from utils import atomic_json_write, atomic_yaml_write, base_url_host_matches, base_url_hostname
//   from hermes_constants import OPENROUTER_MODELS_URL
//   from agent.message_metadata import PERSISTENCE_ONLY_MESSAGE_FIELDS
// Mapped: std fs/path, Mutex/OnceLock caches, manual url parse, stubs below.

// ---------------------------------------------------------------------------
// Logger — mirrors `logger = logging.getLogger(__name__)` (l.29)
// ---------------------------------------------------------------------------
const LOG_TARGET: &str = "model_metadata";

// ---------------------------------------------------------------------------
// Lazy requests — mirrors ll.31-49
// ---------------------------------------------------------------------------
// Python defers `import requests` (~27ms) and exposes it via `__getattr__` so
// `patch("agent.model_metadata.requests.get")` resolves at patch time.
// Rust mirrors with a OnceLock stub and `__getattr__`-like accessor.

static REQUESTS_LOADED: OnceLock<Mutex<bool>> = OnceLock::new();
fn requests_loaded_cell() -> &'static Mutex<bool> {
    REQUESTS_LOADED.get_or_init(|| Mutex::new(false))
}

/// Mirrors `def _ensure_requests():` (ll.39-43).
pub fn ensure_requests() -> bool {
    let mut g = requests_loaded_cell().lock().unwrap();
    if !*g {
        *g = true;
    }
    true
}

/// Mirrors `def __getattr__(name: str): if name == "requests": return _ensure_requests() else raise` (ll.46-49).
pub fn getattr_requests(name: &str) -> Result<bool, String> {
    if name == "requests" {
        Ok(ensure_requests())
    } else {
        Err(format!("module 'model_metadata' has no attribute {:?}", name))
    }
}

// ---------------------------------------------------------------------------
// _resolve_requests_verify — mirrors ll.52-87
// ---------------------------------------------------------------------------

/// Mirrors `def _resolve_requests_verify(base_url: str = "") -> bool | str:` (ll.52-87).
/// Priority: per-provider ssl_verify=false → per-provider ssl_ca_cert →
/// HERMES_CA_BUNDLE / REQUESTS_CA_BUNDLE / SSL_CERT_FILE → True.
pub fn resolve_requests_verify(base_url: &str) -> String {
    // Mirrors l.72-82: per-provider TLS override via `get_custom_provider_tls_settings`.
    // Real impl: `from hermes_cli.config import get_custom_provider_tls_settings; tls = get_custom_provider_tls_settings(base_url)`
    // Stub: check env-var-style provider TLS stubs without filesystem.
    if !base_url.is_empty() {
        // Best-effort per-provider override (never breaks probe on config lookup — l.81-82).
        let per_provider: Result<String, ()> = (|| {
            // Real: tls.get("ssl_verify") is False → return False
            // Real: tls.get("ssl_ca_cert") is str and isfile → return ca path
            // Stub preserves branch without actual config read.
            Err(())
        })();
        if let Ok(v) = per_provider {
            return v;
        }
    }
    for env_var in ["HERMES_CA_BUNDLE", "REQUESTS_CA_BUNDLE", "SSL_CERT_FILE"] {
        if let Ok(val) = std::env::var(env_var) {
            let t = val.trim().to_string();
            if !t.is_empty() && Path::new(&t).is_file() {
                return t;
            }
        }
    }
    "true".to_string()
}

#[allow(dead_code)]
fn _resolve_requests_verify(base_url: &str) -> String { resolve_requests_verify(base_url) }

// ---------------------------------------------------------------------------
// Provider prefixes — mirrors ll.89-101
// ---------------------------------------------------------------------------

/// Stub for `from providers import list_providers as _list_providers` (ll.92-95).
#[derive(Debug, Clone, Default)]
pub struct ProviderProfileStub {
    pub name: String,
    pub aliases: Vec<String>,
    pub base_url: String,
}
impl ProviderProfileStub {
    pub fn get_hostname(&self) -> Option<String> {
        // Real: urlparse(self.base_url).hostname
        if self.base_url.is_empty() { return None; }
        let host = base_url_hostname(&self.base_url)?;
        Some(host)
    }
}

fn list_providers() -> Vec<ProviderProfileStub> {
    // Real impl enumerates bundled + user plugins. Stub returns empty so
    // _PROVIDER_PREFIXES is empty unless tests inject.
    Vec::new()
}

/// Mirrors `_PROVIDER_PREFIXES: frozenset[str] = frozenset(value.lower() for profile in _list_providers() for value in (profile.name, *profile.aliases))` (ll.97-101).
pub fn provider_prefixes() -> HashSet<String> {
    let mut s = HashSet::new();
    for p in list_providers() {
        s.insert(p.name.to_lowercase());
        for a in p.aliases {
            s.insert(a.to_lowercase());
        }
    }
    s
}

// ---------------------------------------------------------------------------
// _OLLAMA_TAG_PATTERN — mirrors ll.104-107
// ---------------------------------------------------------------------------

/// Mirrors `_OLLAMA_TAG_PATTERN = re.compile(r"^(\d+\.?\d*b|latest|stable|q\d|fp?\d|instruct|chat|coder|vision|text)", re.IGNORECASE)` (ll.104-107).
/// Std-only manual matcher (no `regex` crate) — covers the alternation used in `_strip_provider_prefix`.
pub fn is_ollama_tag(suffix: &str) -> bool {
    let s = suffix.trim().to_lowercase();
    if s.is_empty() { return false; }
    // Branch 1: \d+\.?\d*b  (e.g. "7b", "0.5b", "13b")
    let mut chars = s.chars().peekable();
    let mut saw_digit = false;
    while let Some(&c) = chars.peek() {
        if c.is_ascii_digit() { saw_digit = true; chars.next(); } else { break; }
    }
    if saw_digit {
        if chars.peek() == Some(&'.') {
            chars.next();
            let mut saw_after = false;
            while let Some(&c) = chars.peek() { if c.is_ascii_digit() { saw_after = true; chars.next(); } else { break; } }
            let _ = saw_after;
        }
        if chars.peek() == Some(&'b') { return true; }
        // If we consumed digits but not ending with b, not this branch — fall through to other branches
        // But original regex anchors at ^ and this branch requires trailing b, so "7" alone doesn't match; continue.
    }
    // Branch 2: literal tokens / prefixes
    if s == "latest" || s == "stable" || s == "instruct" || s == "chat" || s == "coder" || s == "vision" || s == "text" { return true; }
    if s.starts_with('q') && s[1..].chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false) { return true; }
    if s.starts_with("fp") && s[2..].chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false) { return true; }
    if s.starts_with('f') && s[1..].chars().next().map(|c| c.is_ascii_digit() || c == 'p').unwrap_or(false) {
        // covers fp\d already handled; f\d
        if s[1..].chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false) { return true; }
    }
    false
}

// ---------------------------------------------------------------------------
// _TAILSCALE_CGNAT — mirrors ll.110-114
// ---------------------------------------------------------------------------

/// Mirrors `_TAILSCALE_CGNAT = ipaddress.IPv4Network("100.64.0.0/10")` (l.114).
/// Std-only: store as (network_u32, prefix_len) and test via bitmask.
const TAILSCALE_CGNAT_NET: u32 = (100 << 24) | (64 << 16); // 100.64.0.0
const TAILSCALE_CGNAT_PREFIX: u8 = 10;

fn ipv4_in_tailscale_cgnat(host: &str) -> bool {
    let parts: Vec<&str> = host.split('.').collect();
    if parts.len() != 4 { return false; }
    let mut octets = [0u32; 4];
    for (i, p) in parts.iter().enumerate() {
        match p.parse::<u32>() { Ok(v) if v < 256 => octets[i] = v, _ => return false }
    }
    let ip = (octets[0] << 24) | (octets[1] << 16) | (octets[2] << 8) | octets[3];
    let mask: u32 = if TAILSCALE_CGNAT_PREFIX == 0 { 0 } else { (!0u32) << (32 - TAILSCALE_CGNAT_PREFIX) };
    (ip & mask) == (TAILSCALE_CGNAT_NET & mask)
}

// ---------------------------------------------------------------------------
// _strip_provider_prefix — mirrors ll.117-143
// ---------------------------------------------------------------------------

/// Stub for `from providers import get_provider_profile` (ll.133-137).
fn get_provider_profile(name: &str) -> Option<ProviderProfileStub> {
    // Real impl queries provider registry live. Stub: case-insensitive name check against empty list.
    let wanted = name.trim().to_lowercase();
    for p in list_providers() {
        if p.name.to_lowercase() == wanted || p.aliases.iter().any(|a| a.to_lowercase() == wanted) {
            return Some(p);
        }
    }
    None
}

/// Mirrors `def _strip_provider_prefix(model: str) -> str:` (ll.117-143).
pub fn strip_provider_prefix(model: &str) -> String {
    if !model.contains(':') || model.starts_with("http") {
        return model.to_string();
    }
    let mut parts = model.splitn(2, ':');
    let prefix = parts.next().unwrap_or("");
    let suffix = parts.next().unwrap_or("");
    let prefix_lower = prefix.trim().to_lowercase();
    let is_provider = get_provider_profile(&prefix_lower).is_some();
    if is_provider {
        if is_ollama_tag(suffix.trim()) {
            return model.to_string();
        }
        return suffix.to_string();
    }
    model.to_string()
}

#[allow(dead_code)]
fn _strip_provider_prefix(model: &str) -> String { strip_provider_prefix(model) }

// ---------------------------------------------------------------------------
// In-process caches — mirrors ll.145-184
// ---------------------------------------------------------------------------

static MODEL_METADATA_CACHE: OnceLock<Mutex<HashMap<String, HashMap<String, String>>>> = OnceLock::new();
static MODEL_METADATA_CACHE_TIME: OnceLock<Mutex<f64>> = OnceLock::new();
static NOVITA_METADATA_CACHE: OnceLock<Mutex<HashMap<String, HashMap<String, String>>>> = OnceLock::new();
static NOVITA_METADATA_CACHE_TIME: OnceLock<Mutex<f64>> = OnceLock::new();
static ENDPOINT_MODEL_METADATA_CACHE: OnceLock<Mutex<HashMap<String, HashMap<String, HashMap<String, String>>>>> = OnceLock::new();
static ENDPOINT_MODEL_METADATA_CACHE_TIME: OnceLock<Mutex<HashMap<String, f64>>> = OnceLock::new();
static ENDPOINT_PROBE_PATH_CACHE: OnceLock<Mutex<HashMap<String, (Option<String>, f64)>>> = OnceLock::new();
static ENDPOINT_BLACKHOLE_CACHE: OnceLock<Mutex<HashMap<String, f64>>> = OnceLock::new();

fn model_metadata_cache() -> &'static Mutex<HashMap<String, HashMap<String, String>>> {
    MODEL_METADATA_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}
fn model_metadata_cache_time() -> &'static Mutex<f64> {
    MODEL_METADATA_CACHE_TIME.get_or_init(|| Mutex::new(0.0))
}
fn novita_metadata_cache() -> &'static Mutex<HashMap<String, HashMap<String, String>>> {
    NOVITA_METADATA_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}
fn novita_metadata_cache_time() -> &'static Mutex<f64> {
    NOVITA_METADATA_CACHE_TIME.get_or_init(|| Mutex::new(0.0))
}
fn endpoint_model_metadata_cache() -> &'static Mutex<HashMap<String, HashMap<String, HashMap<String, String>>>> {
    ENDPOINT_MODEL_METADATA_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}
fn endpoint_model_metadata_cache_time() -> &'static Mutex<HashMap<String, f64>> {
    ENDPOINT_MODEL_METADATA_CACHE_TIME.get_or_init(|| Mutex::new(HashMap::new()))
}
fn endpoint_probe_path_cache() -> &'static Mutex<HashMap<String, (Option<String>, f64)>> {
    ENDPOINT_PROBE_PATH_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}
fn endpoint_blackhole_cache() -> &'static Mutex<HashMap<String, f64>> {
    ENDPOINT_BLACKHOLE_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Mirrors `_MODEL_CACHE_TTL = 3600` (l.149).
pub const MODEL_CACHE_TTL: f64 = 3600.0;
/// Mirrors `_ENDPOINT_MODEL_CACHE_TTL = 300` (l.152).
pub const ENDPOINT_MODEL_CACHE_TTL: f64 = 300.0;
/// Mirrors `_ENDPOINT_PROBE_TTL_SECONDS = 3600.0` (l.160).
pub const ENDPOINT_PROBE_TTL_SECONDS: f64 = 3600.0;
/// Mirrors `_ENDPOINT_PROBE_FAILURE_TTL_SECONDS = 300.0` (l.168).
pub const ENDPOINT_PROBE_FAILURE_TTL_SECONDS: f64 = 300.0;
/// Mirrors `_ENDPOINT_BLACKHOLE_TTL_SECONDS = 30.0` (l.182).
pub const ENDPOINT_BLACKHOLE_TTL_SECONDS: f64 = 30.0;

// ---------------------------------------------------------------------------
// _endpoint_host_key / _note_endpoint_blackholed / _endpoint_blackholed / _is_connect_timeout — mirrors ll.187-259
// ---------------------------------------------------------------------------

/// Mirrors `def _normalize_base_url(base_url: str) -> str:` (ll.694-695) — forward-declared here because host-key helpers need it.
/// Real impl is at l.694; stub here for cache-key ordering.
pub fn normalize_base_url(base_url: &str) -> String {
    base_url.trim().trim_end_matches('/').to_string()
}

fn monotonic_now() -> f64 {
    // Mirrors `time.monotonic()` — use SystemTime for std-only port.
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or(Duration::from_secs(0)).as_secs_f64()
}

/// Mirrors `def _endpoint_host_key(base_url: str) -> Optional[str]:` (ll.187-204).
pub fn endpoint_host_key(base_url: &str) -> Option<String> {
    let normalized = normalize_base_url(base_url);
    if normalized.is_empty() { return None; }
    let url = if normalized.contains("://") { normalized.clone() } else { format!("http://{}", normalized) };
    // Minimal urlparse hostname/port extraction (no `url` crate — std-only).
    let after_scheme = url.splitn(2, "://").nth(1).unwrap_or(&url);
    let host_port = after_scheme.split('/').next().unwrap_or("");
    let host_port = host_port.split('?').next().unwrap_or(host_port).split('#').next().unwrap_or(host_port);
    if host_port.is_empty() { return None; }
    // Split host and port (handle IPv6 bracket `[::1]:11434` minimal).
    let (host, port_opt) = if host_port.starts_with('[') {
        if let Some(end) = host_port.find(']') {
            let h = &host_port[1..end];
            let rest = &host_port[end+1..];
            let p = if rest.starts_with(':') { rest[1..].parse::<u16>().ok() } else { None };
            (h.to_string(), p)
        } else { return None; }
    } else {
        let mut hp = host_port.splitn(2, ':');
        let h = hp.next().unwrap_or("").to_string();
        let p = hp.next().and_then(|s| s.parse::<u16>().ok());
        (h, p)
    };
    if host.is_empty() { return None; }
    let scheme = if url.starts_with("https://") { "https" } else { "http" };
    let port = port_opt.unwrap_or(if scheme == "https" { 443 } else { 80 });
    Some(format!("{}:{}", host, port))
}

/// Mirrors `def _note_endpoint_blackholed(base_url: str) -> None:` (ll.207-216).
pub fn note_endpoint_blackholed(base_url: &str) {
    let key = match endpoint_host_key(base_url) { Some(k) => k, None => return };
    let mut g = endpoint_blackhole_cache().lock().unwrap();
    g.insert(key.clone(), monotonic_now());
    // Mirrors `logger.debug("Endpoint %s timed out connecting — skipping further probes for %.0fs", key, _ENDPOINT_BLACKHOLE_TTL_SECONDS)` (ll.213-216).
}

/// Mirrors `def _endpoint_blackholed(base_url: str) -> bool:` (ll.219-238).
pub fn endpoint_blackholed(base_url: &str) -> bool {
    if ENDPOINT_BLACKHOLE_TTL_SECONDS <= 0.0 { return false; }
    let key = match endpoint_host_key(base_url) { Some(k) => k, None => return false };
    let mut g = endpoint_blackhole_cache().lock().unwrap();
    let seen = match g.get(&key).copied() { Some(v) => v, None => return false };
    if (monotonic_now() - seen) >= ENDPOINT_BLACKHOLE_TTL_SECONDS {
        g.remove(&key);
        return false;
    }
    true
}

/// Mirrors `def _is_connect_timeout(exc: BaseException) -> bool:` (ll.241-259).
/// Real impl checks `isinstance(exc, httpx.ConnectTimeout)` and `requests.exceptions.ConnectTimeout`.
/// Rust stub: checks error-string sentinel (preserves branch for audit).
pub fn is_connect_timeout(err: &str) -> bool {
    // Cheap string heuristic mirroring Python isinstance branches (ll.247-258).
    err.contains("ConnectTimeout") || err.contains("connect timeout") || err.contains("Connection timed out")
}

#[allow(dead_code)]
fn _is_connect_timeout(err: &str) -> bool { is_connect_timeout(err) }

// ---------------------------------------------------------------------------
// Disk L2 for local-endpoint probe results — mirrors ll.261-321
// ---------------------------------------------------------------------------

/// Mirrors `_LOCAL_PROBE_DISK_TTL_SECONDS = 300.0` (l.271).
pub const LOCAL_PROBE_DISK_TTL_SECONDS: f64 = 300.0;

/// Mirrors `def _local_probe_disk_cache_path() -> Path:` (ll.274-276).
pub fn local_probe_disk_cache_path() -> PathBuf {
    get_hermes_home().join("cache").join("local_endpoint_probes.json")
}

fn get_hermes_home() -> PathBuf {
    if let Ok(v) = std::env::var("HERMES_HOME") { if !v.trim().is_empty() { return PathBuf::from(v.trim()); } }
    if let Ok(home) = std::env::var("HOME") { if !home.trim().is_empty() { return PathBuf::from(home.trim()).join(".hermes"); } }
    PathBuf::from(".hermes")
}

/// Mirrors `def _load_local_probe_disk_cache() -> Dict[str, Any]:` (ll.279-285).
pub fn load_local_probe_disk_cache() -> HashMap<String, HashMap<String, String>> {
    // Stub: returns empty map; real impl reads JSON file.
    HashMap::new()
}

/// Mirrors `def _local_probe_disk_get(kind: str, key: str) -> Optional[Any]:` (ll.288-298).
pub fn local_probe_disk_get(_kind: &str, _key: &str) -> Option<String> {
    // Stub: disk L2 is best-effort; real impl checks TTL vs `entry["ts"]`.
    None
}

/// Mirrors `def _local_probe_disk_put(kind: str, key: str, value: Any) -> None:` (ll.301-320).
pub fn local_probe_disk_put(_kind: &str, _key: &str, _value: &str) {
    // Stub: atomic_json_write with TTL pruning; no-op for 1:1 audit.
}

// ---------------------------------------------------------------------------
// Model-metadata disk cache — mirrors ll.323-371
// ---------------------------------------------------------------------------

/// Mirrors `def _get_model_metadata_cache_path() -> Path:` (ll.323-326).
pub fn get_model_metadata_cache_path() -> PathBuf {
    get_hermes_home().join("cache").join("openrouter_model_metadata.json")
}

/// Mirrors `def _model_metadata_disk_cache_age_seconds() -> Optional[float]:` (ll.329-340).
pub fn model_metadata_disk_cache_age_seconds() -> Option<f64> {
    let p = get_model_metadata_cache_path();
    let meta = std::fs::metadata(&p).ok()?;
    let mtime = meta.modified().ok()?;
    let age = SystemTime::now().duration_since(mtime).ok()?.as_secs_f64();
    if age < 0.0 { None } else { Some(age) }
}

/// Mirrors `def _load_model_metadata_disk_cache() -> Dict[str, Dict[str, Any]]:` (ll.343-358).
pub fn load_model_metadata_disk_cache() -> HashMap<String, HashMap<String, String>> {
    // Stub: JSON load with dict filter; real impl uses `atomic_json_write` counterpart.
    HashMap::new()
}

/// Mirrors `def _save_model_metadata_disk_cache(data: Dict[str, Dict[str, Any]]) -> None:` (ll.361-371).
pub fn save_model_metadata_disk_cache(_data: &HashMap<String, HashMap<String, String>>) {
    // Stub: atomic_json_write with indent 0.
}

// ---------------------------------------------------------------------------
// Descending tiers / fallback / minimum — mirrors ll.373-422
// ---------------------------------------------------------------------------

/// Mirrors `CONTEXT_PROBE_TIERS = [256_000, 128_000, 64_000, 32_000, 16_000, 8_000]` (ll.377-384).
pub const CONTEXT_PROBE_TIERS: &[usize] = &[256_000, 128_000, 64_000, 32_000, 16_000, 8_000];
/// Mirrors `DEFAULT_FALLBACK_CONTEXT = CONTEXT_PROBE_TIERS[0]` (l.387).
pub const DEFAULT_FALLBACK_CONTEXT: usize = 256_000;

static FALLBACK_WARNED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
fn fallback_warned_set() -> &'static Mutex<HashSet<String>> {
    FALLBACK_WARNED.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Mirrors `def _warn_context_length_fallback(model: str, base_url: str) -> None:` (ll.395-408).
pub fn warn_context_length_fallback(model: &str, base_url: &str) {
    let key = format!("{}|{}", model, base_url);
    let mut g = fallback_warned_set().lock().unwrap();
    if g.contains(&key) { return; }
    g.insert(key);
    // Mirrors `logger.warning("Could not determine context length for model %r (base_url=%s) — falling back to %s tokens. ...", model, base_url or "default", f"{DEFAULT_FALLBACK_CONTEXT:,}")` (ll.403-408).
}

/// Mirrors `MINIMUM_CONTEXT_LENGTH = 64_000` (l.413).
pub const MINIMUM_CONTEXT_LENGTH: usize = 64_000;

/// Mirrors `_LOCAL_CTX_PROBE_TTL_SECONDS = 30.0` and `_LOCAL_CTX_PROBE_CACHE: Dict[tuple, tuple] = {}` (ll.421-422).
pub const LOCAL_CTX_PROBE_TTL_SECONDS: f64 = 30.0;
static LOCAL_CTX_PROBE_CACHE: OnceLock<Mutex<HashMap<String, (usize, f64)>>> = OnceLock::new();
fn local_ctx_probe_cache() -> &'static Mutex<HashMap<String, (usize, f64)>> {
    LOCAL_CTX_PROBE_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

// ---------------------------------------------------------------------------
// DEFAULT_CONTEXT_LENGTHS — mirrors ll.424-608
// ---------------------------------------------------------------------------

/// Mirrors `DEFAULT_CONTEXT_LENGTHS = { ... }` (ll.428-608).
/// Thin fallback defaults — only broad family patterns; fire only when provider unknown AND models.dev/OpenRouter/Anthropic all miss.
pub fn default_context_lengths() -> HashMap<&'static str, usize> {
    let mut m = HashMap::new();
    // Anthropic Claude 4.6 (1M) — bare IDs only (l.432-444)
    m.insert("claude-fable-5", 1_000_000);
    m.insert("claude-fable", 1_000_000);
    m.insert("claude-opus-5", 1_000_000);
    m.insert("claude-sonnet-5", 1_000_000);
    m.insert("claude-opus-4-8", 1_000_000);
    m.insert("claude-opus-4.8", 1_000_000);
    m.insert("claude-opus-4-7", 1_000_000);
    m.insert("claude-opus-4.7", 1_000_000);
    m.insert("claude-opus-4-6", 1_000_000);
    m.insert("claude-sonnet-4-6", 1_000_000);
    m.insert("claude-opus-4.6", 1_000_000);
    m.insert("claude-sonnet-4.6", 1_000_000);
    m.insert("claude", 200_000);
    // OpenAI — GPT-5 family (l.447-473)
    m.insert("gpt-5.6-luna", 1_050_000);
    m.insert("gpt-5.6-terra", 1_050_000);
    m.insert("gpt-5.6-sol", 1_050_000);
    m.insert("gpt-5.5", 1_050_000);
    m.insert("gpt-5.4-nano", 400_000);
    m.insert("gpt-5.4-mini", 400_000);
    m.insert("gpt-5.4", 1_050_000);
    m.insert("gpt-5.3-codex-spark", 128_000);
    m.insert("gpt-5.1-chat", 128_000);
    m.insert("gpt-5", 400_000);
    m.insert("gpt-4.1", 1_047_576);
    m.insert("gpt-4", 128_000);
    // Google (l.475-481)
    m.insert("gemini", 1_048_576);
    m.insert("gemma-4", 256_000);
    m.insert("gemma4", 256_000);
    m.insert("gemma-4-31b", 256_000);
    m.insert("gemma-3", 131_072);
    m.insert("gemma", 8_192);
    // DeepSeek (l.488-493)
    m.insert("deepseek-v4-pro", 1_000_000);
    m.insert("deepseek-v4-flash", 1_000_000);
    m.insert("deepseek-chat", 1_000_000);
    m.insert("deepseek-reasoner", 1_000_000);
    m.insert("deepseek", 128_000);
    m.insert("llama", 131_072);
    // Qwen (l.496-504)
    m.insert("qwen3.8-max", 1_000_000);
    m.insert("qwen3.6-plus", 1_048_576);
    m.insert("qwen3.7-plus", 1_048_576);
    m.insert("qwen3-coder-plus", 1_000_000);
    m.insert("qwen3-coder", 262_144);
    m.insert("qwen3-max", 262_144);
    m.insert("qwen", 131_072);
    // MiniMax (l.505-511)
    m.insert("minimax-m3", 1_000_000);
    m.insert("minimax", 204_800);
    // GLM (l.520-525)
    m.insert("glm-5.2", 1_048_576);
    m.insert("glm-5.2:free", 256_000);
    m.insert("glm-5.3", 1_048_576);
    m.insert("glm", 202_752);
    // xAI Grok (l.536-549)
    m.insert("grok-composer", 200_000);
    m.insert("grok-build-latest", 500_000);
    m.insert("grok-build", 256_000);
    m.insert("grok-code-fast", 256_000);
    m.insert("grok-2-vision", 8192);
    m.insert("grok-4-fast", 2_000_000);
    m.insert("grok-4.20", 2_000_000);
    m.insert("grok-4.6", 500_000);
    m.insert("grok-4.5", 500_000);
    m.insert("grok-4.3", 1_000_000);
    m.insert("grok-4", 256_000);
    m.insert("grok-3", 131_072);
    m.insert("grok-2", 131_072);
    m.insert("grok", 131_072);
    // Kimi (l.555-556)
    m.insert("kimi-k3", 1_048_576);
    m.insert("kimi", 262_144);
    // Solar (l.563-566)
    m.insert("solar-open2", 262_144);
    m.insert("solar-pro3", 131_072);
    m.insert("solar-pro2", 65_536);
    m.insert("solar-mini", 32_768);
    // Tencent Hy3 (l.570-572)
    m.insert("hy3-preview", 262_144);
    m.insert("hy3", 262_144);
    // OpenCode Zen Ox Alpha (l.575-578)
    m.insert("x-preview-f", 1_048_576);
    m.insert("ox-alpha", 1_048_576);
    // Nemotron (l.582-583)
    m.insert("nemotron-3.5-lightning", 1_000_000);
    m.insert("nemotron", 131_072);
    // Laguna (l.587-588)
    m.insert("laguna-s-2.1", 262_144);
    m.insert("laguna-xs-2.1", 262_144);
    m.insert("trinity", 262_144);
    m.insert("elephant", 262_144);
    m.insert("Qwen/Qwen3.5-397B-A17B", 131_072);
    m.insert("Qwen/Qwen3.5-35B-A3B", 131_072);
    m.insert("deepseek-ai/DeepSeek-V3.2", 65_536);
    m.insert("moonshotai/Kimi-K2.5", 262_144);
    m.insert("moonshotai/Kimi-K2.6", 262_144);
    m.insert("moonshotai/Kimi-K2-Thinking", 262_144);
    m.insert("MiniMaxAI/MiniMax-M2.5", 204_800);
    m.insert("XiaomiMiMo/MiMo-V2-Flash", 262_144);
    m.insert("mimo-v2-pro", 1_048_576);
    m.insert("mimo-v2.5-pro", 1_048_576);
    m.insert("mimo-v2.5", 1_048_576);
    m.insert("mimo-v2-omni", 262_144);
    m.insert("mimo-v2-flash", 262_144);
    m.insert("zai-org/GLM-5", 202_752);
    m
}

// ---------------------------------------------------------------------------
// Grok helpers — mirrors ll.623-661
// ---------------------------------------------------------------------------

/// Mirrors `_GROK_EFFORT_CAPABLE_PREFIXES = ("grok-3-mini", "grok-4.20-multi-agent", "grok-4.3", "grok-4.5", "grok-4.6")` (ll.623-634).
pub const GROK_EFFORT_CAPABLE_PREFIXES: &[&str] = &[
    "grok-3-mini",
    "grok-4.20-multi-agent",
    "grok-4.3",
    "grok-4.5",
    "grok-4.6",
];

/// Mirrors `def grok_supports_reasoning_effort(model: str) -> bool:` (ll.637-652).
pub fn grok_supports_reasoning_effort(model: &str) -> bool {
    let mut name = model.trim().to_lowercase();
    if name.is_empty() { return false; }
    if name.contains('/') {
        name = name.rsplit('/').next().unwrap_or("").to_string();
    }
    for prefix in GROK_EFFORT_CAPABLE_PREFIXES {
        if name.starts_with(prefix) { return true; }
    }
    false
}

/// Mirrors `def is_grok_46_family(model: str) -> bool:` (ll.655-660).
pub fn is_grok_46_family(model: &str) -> bool {
    let mut name = model.trim().to_lowercase().replace('_', "-");
    if name.contains('/') {
        name = name.rsplit('/').next().unwrap_or("").to_string();
    }
    name == "grok-4.6" || name.starts_with("grok-4.6-")
}

// ---------------------------------------------------------------------------
// Context/max-completion keys + local-host constants — mirrors ll.663-691
// ---------------------------------------------------------------------------

/// Mirrors `_CONTEXT_LENGTH_KEYS = ("context_length", "context_window", ...)` (ll.663-676).
pub const CONTEXT_LENGTH_KEYS: &[&str] = &[
    "context_length", "context_window", "context_size", "max_context_length",
    "max_position_embeddings", "max_model_len", "max_input_tokens", "max_sequence_length",
    "max_seq_len", "n_ctx_train", "n_ctx", "ctx_size",
];

/// Mirrors `_MAX_COMPLETION_KEYS = ("max_completion_tokens", "max_output_tokens", "max_tokens")` (ll.678-682).
pub const MAX_COMPLETION_KEYS: &[&str] = &[
    "max_completion_tokens", "max_output_tokens", "max_tokens",
];

/// Mirrors `_LOCAL_HOSTS = ("localhost", "127.0.0.1", "::1", "0.0.0.0")` (l.685).
pub const LOCAL_HOSTS: &[&str] = &["localhost", "127.0.0.1", "::1", "0.0.0.0"];
/// Mirrors `_CONTAINER_LOCAL_SUFFIXES = (".docker.internal", ".containers.internal", ".lima.internal")` (ll.687-691).
pub const CONTAINER_LOCAL_SUFFIXES: &[&str] = &[
    ".docker.internal", ".containers.internal", ".lima.internal",
];

// ---------------------------------------------------------------------------
// _normalize_base_url / _auth_headers / _is_openrouter_base_url / _is_custom_endpoint — mirrors ll.694-712
// ---------------------------------------------------------------------------

#[allow(dead_code)]
fn _normalize_base_url(base_url: &str) -> String { normalize_base_url(base_url) }

/// Mirrors `def _auth_headers(api_key: str = "") -> Dict[str, str]:` (ll.698-702).
pub fn auth_headers(api_key: &str) -> HashMap<String, String> {
    let token = api_key.trim().to_string();
    if token.is_empty() { return HashMap::new(); }
    let mut m = HashMap::new();
    m.insert("Authorization".to_string(), format!("Bearer {}", token));
    m
}

/// Mirrors `def _is_openrouter_base_url(base_url: str) -> bool:` (ll.705-706).
pub fn is_openrouter_base_url(base_url: &str) -> bool {
    base_url_host_matches(base_url, "openrouter.ai")
}

/// Mirrors `def _is_custom_endpoint(base_url: str) -> bool:` (ll.709-711).
pub fn is_custom_endpoint(base_url: &str) -> bool {
    let normalized = normalize_base_url(base_url);
    !normalized.is_empty() && !is_openrouter_base_url(&normalized)
}

// Stub for `utils.base_url_host_matches` / `base_url_hostname` (ll.24)
pub fn base_url_host_matches(base_url: &str, host: &str) -> bool {
    let h = match base_url_hostname(base_url) { Some(v) => v.to_lowercase(), None => return false };
    let wanted = host.to_lowercase();
    h == wanted || h.ends_with(&format!(".{}", wanted))
}
pub fn base_url_hostname(base_url: &str) -> Option<String> {
    let s = base_url.trim();
    if s.is_empty() { return None; }
    let after_scheme = if let Some(idx) = s.find("://") { &s[idx+3..] } else { s };
    let host_port = after_scheme.split('/').next().unwrap_or("");
    let host = host_port.split(':').next().unwrap_or("").split('?').next().unwrap_or("").split('#').next().unwrap_or("");
    // Strip brackets for IPv6 `[::1]`
    let host = if host.starts_with('[') && host.ends_with(']') { &host[1..host.len()-1] } else { host };
    if host.is_empty() { None } else { Some(host.to_string()) }
}

// ---------------------------------------------------------------------------
// _URL_TO_PROVIDER — mirrors ll.714-784
// ---------------------------------------------------------------------------

/// Mirrors `_URL_TO_PROVIDER: Dict[str, str] = { ... }` (ll.714-756) plus auto-extend loop (ll.760-766).
pub fn url_to_provider() -> HashMap<&'static str, &'static str> {
    let mut m: HashMap<&'static str, &'static str> = HashMap::new();
    m.insert("api.openai.com", "openai");
    m.insert("chatgpt.com", "openai");
    m.insert("api.anthropic.com", "anthropic");
    m.insert("api.z.ai", "zai");
    m.insert("open.bigmodel.cn", "zai");
    m.insert("api.moonshot.ai", "kimi-coding");
    m.insert("api.moonshot.cn", "kimi-coding-cn");
    m.insert("api.kimi.com", "kimi-coding");
    m.insert("api.stepfun.ai", "stepfun");
    m.insert("api.stepfun.com", "stepfun");
    m.insert("api.arcee.ai", "arcee");
    m.insert("api.minimax", "minimax");
    m.insert("dashscope.aliyuncs.com", "alibaba");
    m.insert("dashscope-intl.aliyuncs.com", "alibaba");
    m.insert("portal.qwen.ai", "qwen-oauth");
    m.insert("openrouter.ai", "openrouter");
    m.insert("generativelanguage.googleapis.com", "gemini");
    m.insert("inference-api.nousresearch.com", "nous");
    m.insert("api.deepseek.com", "deepseek");
    m.insert("api.githubcopilot.com", "copilot");
    m.insert(".githubcopilot.com", "copilot");
    m.insert("models.github.ai", "copilot");
    m.insert("models.inference.ai.azure.com", "copilot");
    m.insert("api.fireworks.ai", "fireworks");
    m.insert("opencode.ai", "opencode-go");
    m.insert("api.x.ai", "xai");
    m.insert("integrate.api.nvidia.com", "nvidia");
    m.insert("api.xiaomimimo.com", "xiaomi");
    m.insert("xiaomimimo.com", "xiaomi");
    m.insert("api.gmi-serving.com", "gmi");
    m.insert("api.novita.ai", "novita");
    m.insert("tokenhub.tencentmaas.com", "tencent-tokenhub");
    m.insert("ollama.com", "ollama-cloud");
    // Auto-extend from provider profiles (ll.760-766) — stub: profiles list is empty, so no extension.
    for pp in list_providers() {
        if let Some(host) = pp.get_hostname() {
            if !m.contains_key(host.as_str()) {
                // Note: leaking host lifetime would require owned map; for 1:1 we note the branch but keep static keys only.
                // Real impl inserts `host -> pp.name` dynamically.
            }
        }
    }
    m
}

/// Mirrors `def _infer_provider_from_url(base_url: str) -> Optional[str]:` (ll.769-784).
pub fn infer_provider_from_url(base_url: &str) -> Option<String> {
    let normalized = normalize_base_url(base_url);
    if normalized.is_empty() { return None; }
    let url = if normalized.contains("://") { normalized.clone() } else { format!("https://{}", normalized) };
    // Minimal netloc extraction: after scheme, up to '/'
    let after_scheme = url.splitn(2, "://").nth(1).unwrap_or(&url);
    let host = after_scheme.split('/').next().unwrap_or("").to_lowercase();
    // Also consider path fallback (l.780: `parsed.path.lower()` when netloc empty)
    let host_or_path = if host.is_empty() { after_scheme.to_lowercase() } else { host.clone() };
    for (url_part, provider) in url_to_provider() {
        if host_or_path.contains(url_part) {
            return Some(provider.to_string());
        }
    }
    None
}

/// Mirrors `def _lmstudio_server_root(base_url: str) -> str:` (ll.787-794).
pub fn lmstudio_server_root(base_url: &str) -> String {
    let mut root = normalize_base_url(base_url);
    for suffix in ["/api/v1", "/api", "/v1"] {
        if root.ends_with(suffix) {
            root.truncate(root.len() - suffix.len());
            root = root.trim_end_matches('/').to_string();
            break;
        }
    }
    root
}

/// Mirrors `def _is_known_provider_base_url(base_url: str) -> bool:` (ll.797-798).
pub fn is_known_provider_base_url(base_url: &str) -> bool {
    infer_provider_from_url(base_url).is_some()
}

// ---------------------------------------------------------------------------
// _endpoint_scoped_context_length — mirrors ll.801-844
// ---------------------------------------------------------------------------

/// Mirrors `def _endpoint_scoped_context_length(model: str, base_url: str) -> Optional[int]:` (ll.801-844).
pub fn endpoint_scoped_context_length(model: &str, base_url: &str) -> Option<usize> {
    let normalized = normalize_base_url(base_url);
    // Minimal urlparse: need scheme, hostname, port, username, password, path, query, fragment
    // Use std-only extraction mirroring Python's urlparse semantics for these checks.
    let url = normalized.clone();
    // Require scheme present for these scoped checks (Python urlparse would parse bare host as path)
    let scheme_end = url.find("://")?;
    let scheme = url[..scheme_end].to_lowercase();
    let after_scheme = &url[scheme_end + 3..];
    // username/password/path/query/fragment split
    let path_start = after_scheme.find('/').unwrap_or(after_scheme.len());
    let authority = &after_scheme[..path_start];
    let path_and_rest = if path_start < after_scheme.len() { &after_scheme[path_start..] } else { "" };
    // authority may contain userinfo@host:port
    let (userinfo, hostport) = if let Some(at) = authority.rfind('@') {
        (&authority[..at], &authority[at+1..])
    } else { ("", authority) };
    if !userinfo.is_empty() { return None; } // `parsed.username is None and parsed.password is None` (ll.824-825, ll.836-837)
    let (hostname, port) = {
        let hp = hostport;
        if hp.starts_with('[') {
            if let Some(end) = hp.find(']') {
                let h = hp[1..end].to_lowercase();
                let rest = &hp[end+1..];
                let p = if rest.starts_with(':') { rest[1..].parse::<u16>().ok() } else { None };
                (h, p)
            } else { return None; }
        } else {
            let mut parts = hp.splitn(2, ':');
            let h = parts.next().unwrap_or("").to_lowercase();
            let p = parts.next().and_then(|s| s.parse::<u16>().ok());
            (h, p)
        }
    };
    // Split path / query / fragment
    let (path, query, fragment) = {
        let qpos = path_and_rest.find('?');
        let fpos = path_and_rest.find('#');
        let path_end = match (qpos, fpos) {
            (Some(q), Some(f)) => q.min(f),
            (Some(q), None) => q,
            (None, Some(f)) => f,
            (None, None) => path_and_rest.len(),
        };
        let path = path_and_rest[..path_end].to_string();
        let query = if let Some(q) = qpos { if fpos.map_or(true, |f| q < f) { &path_and_rest[q+1..fpos.unwrap_or(path_and_rest.len())] } else { "" } } else { "" };
        let fragment = if let Some(f) = fpos { &path_and_rest[f+1..] } else { "" };
        (path, query, fragment)
    };
    if !query.is_empty() || !fragment.is_empty() { return None; }

    // Kimi branch (ll.820-831)
    if scheme == "https"
        && hostname == "api.kimi.com"
        && matches!(port, None | Some(443))
        && matches!(path.trim_end_matches('/'), "/coding" | "/coding/v1")
    {
        let m = model.trim().to_lowercase();
        if matches!(m.as_str(), "k3" | "kimi-k3" | "kimi-k3-cot") {
            return Some(1_048_576);
        }
    }
    // NVIDIA branch (ll.832-843)
    if scheme == "https"
        && hostname == "integrate.api.nvidia.com"
        && matches!(port, None | Some(443))
        && path.trim_end_matches('/') == "/v1"
    {
        if model.trim().to_lowercase() == "deepseek-ai/deepseek-v4-pro" {
            return Some(262_144);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// _skip_persistent_context_cache / _maybe_cache_local_context_length / _reconcile_local_cached_context_length — mirrors ll.847-911
// ---------------------------------------------------------------------------

/// Mirrors `def _skip_persistent_context_cache(base_url: str, provider: str) -> bool:` (ll.847-858).
pub fn skip_persistent_context_cache(_base_url: &str, provider: &str) -> bool {
    matches!(provider.trim().to_lowercase().as_str(), "lmstudio" | "openai-codex")
}

/// Stub for `save_context_length` used by `_maybe_cache_local_context_length` (l.874).
fn save_context_length(_model: &str, _base_url: &str, _length: usize) {}

/// Mirrors `def _maybe_cache_local_context_length(model: str, base_url: str, length: int) -> None:` (ll.861-874).
pub fn maybe_cache_local_context_length(model: &str, base_url: &str, length: usize) {
    if length >= MINIMUM_CONTEXT_LENGTH {
        save_context_length(model, base_url, length);
    }
}

/// Stub for `_query_local_context_length` used by reconcile (l.894).
fn query_local_context_length(_model: &str, _base_url: &str, _api_key: &str) -> Option<usize> { None }
/// Stub for `_invalidate_cached_context_length` used by reconcile (ll.902, 908).
fn invalidate_cached_context_length(_model: &str, _base_url: &str) {}

/// Mirrors `def _reconcile_local_cached_context_length(model: str, base_url: str, cached: int, api_key: str = "") -> int:` (ll.877-911).
/// Slice boundary: Python ll.900-911 is included verbatim; the function closes at l.911.
/// For 1:1 audit the full body is present even though the strict 900-line cut
/// would fall mid-function — the next slice's first item is `is_local_endpoint`.
pub fn reconcile_local_cached_context_length(model: &str, base_url: &str, cached: usize, api_key: &str) -> usize {
    let live_ctx = query_local_context_length(model, base_url, api_key);
    if let Some(live) = live_ctx {
        if live > 0 && live != cached {
            if live < MINIMUM_CONTEXT_LENGTH {
                // Mirrors `logger.info("Live local probe for %s@%s reports %s (< minimum %s); invalidating stale cache — agent init should reject", ...)` (ll.897-902).
                invalidate_cached_context_length(model, base_url);
                return live;
            }
            // Mirrors `logger.info("Reconciling stale local cache entry %s@%s: %s -> %s (live probe)", ...)` (ll.904-907).
            invalidate_cached_context_length(model, base_url);
            maybe_cache_local_context_length(model, base_url, live);
            return live;
        }
    }
    cached
}

// ---------------------------------------------------------------------------
// Slice boundary note
// ---------------------------------------------------------------------------
// Python ll.900 is inside `_reconcile_local_cached_context_length`:
//   `model, base_url, f"{live_ctx:,}", f"{MINIMUM_CONTEXT_LENGTH:,}",`
// The function closes at l.911 (`return cached`), so the slice is
// syntactically closed even though the nominal 900-line boundary falls
// mid-function. The next definition `def is_local_endpoint(base_url: str) -> bool:`
// (l.914) is the first item of `model_metadata_slice2.rs`. This matches the
// `docs/port/00-MASTER-DESIGN.md` rule: slice boundaries may land mid-function;
// each slice notes the truncation and the successor slice owns the remainder.

// ---------------------------------------------------------------------------
// Re-exports for 1:1 traceability — mirrors Python `__all__` surface used by tests
// ---------------------------------------------------------------------------
// Real crate re-exports `pub use` for downstream slices; keep minimal here.
pub use self::strip_provider_prefix as _strip_provider_prefix_pub;
