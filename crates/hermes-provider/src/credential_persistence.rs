//! Credential-pool disk-boundary sanitization helpers.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/agent/credential_persistence.py` (174 lines).
//!
//! These helpers define which credential-pool entries are references to borrowed
//! runtime secrets and strip raw values before those entries are written to
//! ``auth.json``.  They intentionally have no dependency on ``hermes_cli.auth`` so
//! both the pool model and the final auth-store write boundary can share the same
//! policy without import cycles.
//!
//! T0048 — 1:1 port, no cargo (NEVER cargo).
//!
//! Notes on fidelity vs. Rust idioms:
//! - Python `frozenset({("anthropic","hermes_pkce"), ...})` ↔ `&[(&str,&str)]` slice + linear scan (std-only, tiny set).
//! - Python `re.compile(r"(?<=[a-z0-9])(?=[A-Z])")` ↔ manual scan (`is_ascii_lowercase`/`is_ascii_digit` → `is_ascii_uppercase`).
//! - Python `hashlib.sha256(text.encode("utf-8", errors="surrogatepass")).hexdigest()[:16]` ↔ pure-Rust SHA-256 (FIPS 180-4, no `sha2` crate).
//! - Python `str(key or "").strip()` ↔ `value_to_python_str_or_empty` handling `None`/`Null`/falsy `0`/`False`/`""`→`""`.
//! - Python `Mapping[str, Any]` / `Dict[str, Any]` ↔ `HashMap<String, Value>` with `Value` enum (std-only `Any` stand-in).
//! - Python `payload.get("source")` / `provider_id` as `Any` ↔ `Option<&Value>` + `Option<&str>` overloads.
//! - Crate stays `std`-only — no `serde`, `regex`, `sha2` deps.

#![allow(dead_code, unused_imports, unused_variables, clippy::all)]

use std::collections::{HashMap, HashSet};

// ---------------------------------------------------------------------------
// Minimal Value — mirrors `Any` / `Dict[str, Any]` (std-only)
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
// Constants — mirrors credential_persistence.py lines 20-92
// ---------------------------------------------------------------------------

/// Sources Hermes owns and can intentionally persist in auth.json. Everything
/// else with a non-empty source is treated as borrowed/reference-only by default.
/// Mirrors `_PERSISTABLE_PROVIDER_SOURCES` (lines 20-26).
const PERSISTABLE_PROVIDER_SOURCES: &[(&str, &str)] = &[
    ("anthropic", "hermes_pkce"),
    ("minimax-oauth", "oauth"),
    ("nous", "device_code"),
    ("openai-codex", "device_code"),
    ("xai-oauth", "device_code"),
];

/// Mirrors `_SAFE_SECRETISH_METADATA_KEYS` (lines 28-49).
const SAFE_SECRETISH_METADATA_KEYS: &[&str] = &[
    "secret_fingerprint",
    "secret_source",
    "token_type",
    "scope",
    "client_id",
    "agent_key_id",
    "agent_key_expires_at",
    "agent_key_expires_in",
    "agent_key_reused",
    "agent_key_obtained_at",
    "expires_at",
    "expires_at_ms",
    "expires_in",
    "last_refresh",
    "last_status",
    "last_status_at",
    "last_error_code",
    "last_error_reason",
    "last_error_message",
    "last_error_reset_at",
];

/// Mirrors `_SECRET_VALUE_KEYS` (lines 51-73).
const SECRET_VALUE_KEYS: &[&str] = &[
    "access_token",
    "refresh_token",
    "agent_key",
    "api_key",
    "apikey",
    "api_token",
    "auth_token",
    "authorization",
    "bearer_token",
    "client_secret",
    "credential",
    "credentials",
    "id_token",
    "oauth_token",
    "private_key",
    "secret_key",
    "session_token",
    "password",
    "secret",
    "token",
    "tokens",
];

/// Mirrors `_SECRET_VALUE_SUFFIXES` (lines 75-92).
const SECRET_VALUE_SUFFIXES: &[&str] = &[
    "_api_key",
    "_api_token",
    "_access_token",
    "_auth_token",
    "_refresh_token",
    "_bearer_token",
    "_client_secret",
    "_id_token",
    "_oauth_token",
    "_private_key",
    "_session_token",
    "_secret_key",
    "_password",
    "_secret",
    "_token",
    "_key",
];

// ---------------------------------------------------------------------------
// Helpers: python str / truthiness — mirrors `str(x or "")`
// ---------------------------------------------------------------------------

fn value_to_py_string(v: &Value) -> String {
    match v {
        Value::Null => "None".to_string(),
        Value::Bool(b) => {
            if *b {
                "True".to_string()
            } else {
                "False".to_string()
            }
        }
        Value::Number(n) => {
            if n.is_finite() {
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
        Value::String(s) => s.clone(),
        Value::Array(a) => {
            let inner: Vec<String> = a.iter().map(value_to_py_string).collect();
            format!("[{}]", inner.join(", "))
        }
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            let parts: Vec<String> = keys
                .iter()
                .map(|k| format!("'{}': {}", k, value_to_py_string(&m[*k])))
                .collect();
            format!("{{{}}}", parts.join(", "))
        }
    }
}

fn is_value_falsy(v: &Value) -> bool {
    match v {
        Value::Null => true,
        Value::Bool(b) => !*b,
        Value::Int(i) => *i == 0,
        Value::Number(n) => *n == 0.0,
        Value::String(s) => s.is_empty(),
        Value::Array(a) => a.is_empty(),
        Value::Object(m) => m.is_empty(),
    }
}

/// Mirrors Python `str(x or "")` for `Option<&Value>`: falsy → `""`, else `str(x)`.
fn python_str_or_empty(v: Option<&Value>) -> String {
    match v {
        None => String::new(),
        Some(val) => {
            if is_value_falsy(val) {
                String::new()
            } else {
                value_to_py_string(val)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// _normalize_key — mirrors lines 97-100
// ---------------------------------------------------------------------------

/// Mirrors `_normalize_key(key: Any) -> str` (lines 97-100).
///
/// `raw = str(key or "").strip()` → camelCase boundary inserts `"_"` → lower → `"-"`/`"."`→`"_"`.
pub fn normalize_key(key: &str) -> String {
    let raw = key.trim();
    if raw.is_empty() {
        return String::new();
    }
    // Insert "_" at camelCase boundary (?<=[a-z0-9])(?=[A-Z])
    let chars: Vec<char> = raw.chars().collect();
    let mut out = String::with_capacity(raw.len() * 2);
    for i in 0..chars.len() {
        let c = chars[i];
        if i > 0 {
            let prev = chars[i - 1];
            let is_prev_lower_or_digit = prev.is_ascii_lowercase() || prev.is_ascii_digit();
            let is_curr_upper = c.is_ascii_uppercase();
            if is_prev_lower_or_digit && is_curr_upper {
                out.push('_');
            }
        }
        out.push(c);
    }
    out.to_ascii_lowercase().replace('-', "_").replace('.', "_")
}

/// Value-aware variant — `str(key or "")` handling.
pub fn normalize_key_value(key: Option<&Value>) -> String {
    let s = python_str_or_empty(key);
    normalize_key(&s)
}

// ---------------------------------------------------------------------------
// is_borrowed_credential_source — mirrors lines 103-111
// ---------------------------------------------------------------------------

/// Return true when `source` points at a borrowed/reference-only secret.
///
/// Mirrors `is_borrowed_credential_source(source: Any, provider_id: Any = None) -> bool` (lines 103-111).
pub fn is_borrowed_credential_source(source: Option<&str>, provider_id: Option<&str>) -> bool {
    let normalized_source = source.unwrap_or("").trim().to_ascii_lowercase();
    if normalized_source.is_empty() {
        return false;
    }
    if normalized_source == "manual" || normalized_source.starts_with("manual:") {
        return false;
    }
    let normalized_provider = provider_id.unwrap_or("").trim().to_ascii_lowercase();
    for (p, s) in PERSISTABLE_PROVIDER_SOURCES {
        if *p == normalized_provider && *s == normalized_source {
            return false;
        }
    }
    true
}

/// Value-aware variant for `Mapping` lookups (`payload.get("source")` → `Value`).
pub fn is_borrowed_credential_source_value(
    source: Option<&Value>,
    provider_id: Option<&Value>,
) -> bool {
    let src = python_str_or_empty(source);
    let prov = python_str_or_empty(provider_id);
    let src_opt = if src.trim().is_empty() {
        None
    } else {
        Some(src.trim().to_string())
    };
    let prov_opt = if prov.trim().is_empty() {
        None
    } else {
        Some(prov.trim().to_string())
    };
    is_borrowed_credential_source(src_opt.as_deref(), prov_opt.as_deref())
}

// ---------------------------------------------------------------------------
// _is_secret_payload_key — mirrors lines 114-120
// ---------------------------------------------------------------------------

/// Mirrors `_is_secret_payload_key(key: Any) -> bool` (lines 114-120).
pub fn is_secret_payload_key(key: &str) -> bool {
    let normalized = normalize_key(key);
    if normalized.is_empty() || SAFE_SECRETISH_METADATA_KEYS.contains(&normalized.as_str()) {
        return false;
    }
    if SECRET_VALUE_KEYS.contains(&normalized.as_str()) {
        return true;
    }
    for suffix in SECRET_VALUE_SUFFIXES {
        if normalized.ends_with(suffix) {
            return true;
        }
    }
    false
}

/// Value-aware variant.
pub fn is_secret_payload_key_value(key: Option<&Value>) -> bool {
    let s = python_str_or_empty(key);
    if s.trim().is_empty() {
        // Direct empty check mirrors normalize_key("") → "" → false, but early return preserves Python's
        // `str(key or "")` → "" path without extra allocation.
        return false;
    }
    is_secret_payload_key(&s)
}

// ---------------------------------------------------------------------------
// Pure SHA-256 — mirrors `hashlib.sha256(...).hexdigest()` (lines 123-130)
// ---------------------------------------------------------------------------

const SHA256_K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

const SHA256_H0: [u32; 8] = [
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
];

#[inline(always)]
fn rotr(x: u32, n: u32) -> u32 {
    (x >> n) | (x << (32 - n))
}

fn sha256_bytes(data: &[u8]) -> [u8; 32] {
    let mut h = SHA256_H0;
    // Pre-processing: padding
    let bit_len = (data.len() as u64) * 8;
    let mut padded = Vec::with_capacity(((data.len() + 9 + 63) / 64) * 64);
    padded.extend_from_slice(data);
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0x00);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());

    // Process each 512-bit chunk
    for chunk in padded.chunks(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([chunk[i * 4], chunk[i * 4 + 1], chunk[i * 4 + 2], chunk[i * 4 + 3]]);
        }
        for i in 16..64 {
            let s0 = rotr(w[i - 15], 7) ^ rotr(w[i - 15], 18) ^ (w[i - 15] >> 3);
            let s1 = rotr(w[i - 2], 17) ^ rotr(w[i - 2], 19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let mut a = h[0];
        let mut b = h[1];
        let mut c = h[2];
        let mut d = h[3];
        let mut e = h[4];
        let mut f = h[5];
        let mut g = h[6];
        let mut hh = h[7];
        for i in 0..64 {
            let s1 = rotr(e, 6) ^ rotr(e, 11) ^ rotr(e, 25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(SHA256_K[i])
                .wrapping_add(w[i]);
            let s0 = rotr(a, 2) ^ rotr(a, 13) ^ rotr(a, 22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }
    let mut out = [0u8; 32];
    for (i, val) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&val.to_be_bytes());
    }
    out
}

fn sha256_hex(text: &str) -> String {
    let digest = sha256_bytes(text.as_bytes());
    let mut s = String::with_capacity(64);
    for b in digest {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

// ---------------------------------------------------------------------------
// _fingerprint_value — mirrors lines 123-130
// ---------------------------------------------------------------------------

/// Mirrors `_fingerprint_value(value: Any) -> str | None` (lines 123-130).
///
/// `None`/`Null`/empty-string → `None`, else `sha256: + first 16 hex`.
pub fn fingerprint_value(value: Option<&Value>) -> Option<String> {
    let v = value?;
    if matches!(v, Value::Null) {
        return None;
    }
    let text = value_to_py_string(v);
    if text.is_empty() {
        return None;
    }
    let digest = sha256_hex(&text);
    Some(format!("sha256:{}", &digest[..16]))
}

/// Convenience for string values (common path for token strings).
pub fn fingerprint_value_str(text: &str) -> Option<String> {
    if text.is_empty() {
        return None;
    }
    let digest = sha256_hex(text);
    Some(format!("sha256:{}", &digest[..16]))
}

// Back-compat alias mirroring private name.
#[allow(dead_code)]
fn _fingerprint_value(value: Option<&Value>) -> Option<String> {
    fingerprint_value(value)
}

// ---------------------------------------------------------------------------
// _credential_secret_fingerprint — mirrors lines 133-148
// ---------------------------------------------------------------------------

/// Mirrors `_credential_secret_fingerprint(payload: Mapping[str, Any]) -> str | None` (lines 133-148).
pub fn credential_secret_fingerprint(payload: &HashMap<String, Value>) -> Option<String> {
    const PRIORITY_KEYS: &[&str] = &["agent_key", "access_token", "refresh_token", "api_key", "token", "secret"];
    for key in PRIORITY_KEYS {
        if let Some(v) = payload.get(*key) {
            if let Some(fp) = fingerprint_value(Some(v)) {
                return Some(fp);
            }
        }
    }
    // Any secret payload key — iterate sorted for deterministic cross-run result.
    // Python uses insertion order; Rust HashMap is unordered, so sorted is the
    // faithful deterministic approximation without requiring an ordered map.
    let mut keys: Vec<&String> = payload.keys().collect();
    keys.sort();
    for key in keys {
        if is_secret_payload_key(key) {
            if let Some(v) = payload.get(key) {
                if let Some(fp) = fingerprint_value(Some(v)) {
                    return Some(fp);
                }
            }
        }
    }
    if let Some(Value::String(s)) = payload.get("secret_fingerprint") {
        if s.starts_with("sha256:") {
            return Some(s.clone());
        }
    }
    None
}

#[allow(dead_code)]
fn _credential_secret_fingerprint(payload: &HashMap<String, Value>) -> Option<String> {
    credential_secret_fingerprint(payload)
}

// ---------------------------------------------------------------------------
// sanitize_borrowed_credential_payload — mirrors lines 151-174
// ---------------------------------------------------------------------------

/// Return a disk-safe credential-pool payload.
///
/// Owned sources (manual entries and Hermes-owned OAuth/device-code state)
/// pass through unchanged. Borrowed/reference-only sources keep labels,
/// source refs, status/cooldown metadata, counters, and a non-reversible
/// fingerprint, but raw secret value fields are removed.
///
/// Mirrors `sanitize_borrowed_credential_payload(payload: Mapping[str, Any], provider_id: Any = None) -> Dict[str, Any]` (lines 151-174).
pub fn sanitize_borrowed_credential_payload(
    payload: &HashMap<String, Value>,
    provider_id: Option<&str>,
) -> HashMap<String, Value> {
    let source_str = payload
        .get("source")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let is_borrowed = is_borrowed_credential_source(source_str.as_deref(), provider_id);
    // For non-string source values, fall back to Value-aware check if string path missed.
    let is_borrowed = if payload.get("source").is_some() && source_str.is_none() {
        is_borrowed_credential_source_value(payload.get("source"), provider_id.map(Value::String).as_ref().map(|v| v as &Value))
            || is_borrowed
    } else {
        is_borrowed
    };
    if !is_borrowed {
        return payload.clone();
    }
    let fingerprint = credential_secret_fingerprint(payload);
    let mut sanitized = HashMap::new();
    for (k, v) in payload {
        if !is_secret_payload_key(k) {
            sanitized.insert(k.clone(), v.clone());
        }
    }
    if let Some(fp) = fingerprint {
        sanitized.insert("secret_fingerprint".to_string(), Value::String(fp));
    }
    sanitized
}

/// Value-aware overload where `provider_id` is an `Any`/`Value` (mirrors Python's `Any` param).
pub fn sanitize_borrowed_credential_payload_value(
    payload: &HashMap<String, Value>,
    provider_id: Option<&Value>,
) -> HashMap<String, Value> {
    let provider_str = provider_id.and_then(|v| {
        let s = python_str_or_empty(Some(v));
        let t = s.trim().to_string();
        if t.is_empty() {
            None
        } else {
            Some(t)
        }
    });
    sanitize_borrowed_credential_payload(payload, provider_str.as_deref())
}

// Back-compat private alias
#[allow(dead_code)]
fn _sanitize_borrowed_credential_payload(
    payload: &HashMap<String, Value>,
    provider_id: Option<&str>,
) -> HashMap<String, Value> {
    sanitize_borrowed_credential_payload(payload, provider_id)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn v_str(s: &str) -> Value {
        Value::String(s.to_string())
    }
    fn v_null() -> Value {
        Value::Null
    }

    #[test]
    fn normalize_key_camel_and_separators() {
        assert_eq!(normalize_key("apiKey"), "api_key");
        assert_eq!(normalize_key("secretSource"), "secret_source");
        assert_eq!(normalize_key("accessToken"), "access_token");
        assert_eq!(normalize_key("bearerToken"), "bearer_token");
        assert_eq!(normalize_key("APIKey"), "apikey");
        assert_eq!(normalize_key("api-key"), "api_key");
        assert_eq!(normalize_key("api.key"), "api_key");
        assert_eq!(normalize_key("api_key"), "api_key");
        assert_eq!(normalize_key(""), "");
        assert_eq!(normalize_key("   "), "");
        assert_eq!(normalize_key("myAPIKey"), "my_apikey"); // only y->A splits, A->P/I/K are upper->upper no split
    }

    #[test]
    fn is_borrowed_empty_and_manual() {
        assert!(!is_borrowed_credential_source(None, None));
        assert!(!is_borrowed_credential_source(Some(""), Some("openai")));
        assert!(!is_borrowed_credential_source(Some("manual"), None));
        assert!(!is_borrowed_credential_source(Some("manual:device_code"), None));
        assert!(!is_borrowed_credential_source(Some("manual:foo"), Some("anthropic")));
        assert!(!is_borrowed_credential_source(Some("MANUAL"), None));
        assert!(!is_borrowed_credential_source(Some("Manual:Device_Code"), None));
    }

    #[test]
    fn is_borrowed_persistable_sources_are_owned() {
        // Exact persistable tuples must NOT be borrowed
        assert!(!is_borrowed_credential_source(Some("hermes_pkce"), Some("anthropic")));
        assert!(!is_borrowed_credential_source(Some("oauth"), Some("minimax-oauth")));
        assert!(!is_borrowed_credential_source(Some("device_code"), Some("nous")));
        assert!(!is_borrowed_credential_source(Some("device_code"), Some("openai-codex")));
        assert!(!is_borrowed_credential_source(Some("device_code"), Some("xai-oauth")));
        // Same sources with wrong provider are borrowed (fail closed)
        assert!(is_borrowed_credential_source(Some("oauth"), Some("anthropic")));
        assert!(is_borrowed_credential_source(Some("hermes_pkce"), Some("nous")));
        assert!(is_borrowed_credential_source(Some("device_code"), Some("anthropic")));
        // Case-insensitive
        assert!(!is_borrowed_credential_source(Some("Device_Code"), Some("NOUS")));
        assert!(!is_borrowed_credential_source(Some("Hermes_PKCE"), Some("Anthropic")));
    }

    #[test]
    fn is_borrowed_borrowed_by_default() {
        assert!(is_borrowed_credential_source(Some("gh_cli"), Some("openai-codex")));
        assert!(is_borrowed_credential_source(Some("claude_code"), None));
        assert!(is_borrowed_credential_source(Some("some_external_vault"), Some("openai")));
        assert!(is_borrowed_credential_source(Some("env:OPENAI_API_KEY"), Some("openai")));
    }

    #[test]
    fn is_secret_payload_key_exact_and_suffix() {
        assert!(is_secret_payload_key("access_token"));
        assert!(is_secret_payload_key("api_key"));
        assert!(is_secret_payload_key("secret"));
        assert!(is_secret_payload_key("token"));
        assert!(is_secret_payload_key("apikey"));
        assert!(is_secret_payload_key("authorization"));
        // camelCase normalizes
        assert!(is_secret_payload_key("accessToken"));
        assert!(is_secret_payload_key("apiKey"));
        // suffix
        assert!(is_secret_payload_key("my_api_key"));
        assert!(is_secret_payload_key("custom_secret"));
        assert!(is_secret_payload_key("service_token"));
        assert!(is_secret_payload_key("openai_api_key"));
        assert!(is_secret_payload_key("my_key"));
        assert!(is_secret_payload_key("my.password"));
        assert!(is_secret_payload_key("my-password"));
        // metadata not secret
        assert!(!is_secret_payload_key("secret_fingerprint"));
        assert!(!is_secret_payload_key("expires_at"));
        assert!(!is_secret_payload_key("client_id"));
        assert!(!is_secret_payload_key("token_type"));
        assert!(!is_secret_payload_key("last_status"));
        assert!(!is_secret_payload_key("scope"));
        // empty
        assert!(!is_secret_payload_key(""));
        // non-secret regular keys
        assert!(!is_secret_payload_key("label"));
        assert!(!is_secret_payload_key("source"));
        assert!(!is_secret_payload_key("provider"));
    }

    #[test]
    fn fingerprint_value_known_sha() {
        // sha256("hello") = 2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824
        let fp = fingerprint_value(Some(&v_str("hello"))).unwrap();
        assert_eq!(fp, "sha256:2cf24dba5fb0a30e");
        // empty string → None (str(value) empty check)
        assert!(fingerprint_value(Some(&v_str(""))).is_none());
        // Null → None
        assert!(fingerprint_value(Some(&v_null())).is_none());
        // None option → None
        assert!(fingerprint_value(None).is_none());
        // sha256("") would be e3b0c44... but we return None per python
        let empty_fp = fingerprint_value_str("");
        assert!(empty_fp.is_none());
    }

    #[test]
    fn credential_secret_fingerprint_priority() {
        let mut payload = HashMap::new();
        payload.insert("agent_key".to_string(), v_str("agent-secret"));
        payload.insert("api_key".to_string(), v_str("api-secret"));
        // agent_key has priority over api_key
        let fp = credential_secret_fingerprint(&payload).unwrap();
        let expected = fingerprint_value(Some(&v_str("agent-secret"))).unwrap();
        assert_eq!(fp, expected);

        // priority fallback: no priority keys, but suffix match
        let mut payload2 = HashMap::new();
        payload2.insert("custom_api_key".to_string(), v_str("mykey123"));
        payload2.insert("label".to_string(), v_str("test"));
        let fp2 = credential_secret_fingerprint(&payload2).unwrap();
        assert_eq!(fp2, fingerprint_value(Some(&v_str("mykey123"))).unwrap());

        // existing fingerprint passthrough when no secret keys
        let mut payload3 = HashMap::new();
        payload3.insert("secret_fingerprint".to_string(), v_str("sha256:deadbeefdeadbeef"));
        payload3.insert("label".to_string(), v_str("x"));
        assert_eq!(credential_secret_fingerprint(&payload3).unwrap(), "sha256:deadbeefdeadbeef");

        // non-sha256 existing fingerprint ignored
        let mut payload4 = HashMap::new();
        payload4.insert("secret_fingerprint".to_string(), v_str("not-a-hash"));
        assert!(credential_secret_fingerprint(&payload4).is_none());
    }

    #[test]
    fn sanitize_borrowed_strips_secret_and_adds_fingerprint() {
        let mut payload = HashMap::new();
        payload.insert("source".to_string(), v_str("gh_cli"));
        payload.insert("api_key".to_string(), v_str("sk-secret123"));
        payload.insert("label".to_string(), v_str("my label"));
        payload.insert("expires_at".to_string(), v_str("2026-01-01"));

        let sanitized = sanitize_borrowed_credential_payload(&payload, Some("openai-codex"));
        // secret stripped
        assert!(!sanitized.contains_key("api_key"));
        // metadata kept
        assert_eq!(sanitized.get("label").unwrap(), &v_str("my label"));
        assert_eq!(sanitized.get("expires_at").unwrap(), &v_str("2026-01-01"));
        assert_eq!(sanitized.get("source").unwrap(), &v_str("gh_cli"));
        // fingerprint added
        let fp = fingerprint_value(Some(&v_str("sk-secret123"))).unwrap();
        assert_eq!(sanitized.get("secret_fingerprint").unwrap(), &v_str(&fp));
    }

    #[test]
    fn sanitize_owned_passes_through() {
        let mut payload = HashMap::new();
        payload.insert("source".to_string(), v_str("device_code"));
        payload.insert("api_key".to_string(), v_str("sk-keep"));
        payload.insert("label".to_string(), v_str("x"));

        let sanitized = sanitize_borrowed_credential_payload(&payload, Some("nous"));
        // owned → unchanged (includes api_key)
        assert_eq!(sanitized.get("api_key").unwrap(), &v_str("sk-keep"));
        assert_eq!(sanitized.len(), payload.len());
    }

    #[test]
    fn sanitize_manual_passes_through() {
        let mut payload = HashMap::new();
        payload.insert("source".to_string(), v_str("manual"));
        payload.insert("secret".to_string(), v_str("keep-me"));
        let sanitized = sanitize_borrowed_credential_payload(&payload, Some("openai"));
        assert_eq!(sanitized.get("secret").unwrap(), &v_str("keep-me"));
    }

    #[test]
    fn sanitize_empty_source_passes_through() {
        let mut payload = HashMap::new();
        payload.insert("source".to_string(), v_str(""));
        payload.insert("api_key".to_string(), v_str("keep"));
        let sanitized = sanitize_borrowed_credential_payload(&payload, Some("openai"));
        assert!(sanitized.contains_key("api_key"));
    }

    #[test]
    fn sha256_vectors() {
        // Verify pure SHA-256 against known vectors
        assert_eq!(sha256_hex(""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        assert_eq!(sha256_hex("hello"), "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824");
        assert_eq!(sha256_hex("abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }
}
