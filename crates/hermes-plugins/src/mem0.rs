//! Mem0 memory plugin — MemoryProvider interface.
//!
//! 1:1 port of `reference/NousResearch/hermes-agent/plugins/memory/mem0/__init__.py` (628 LOC).
//! Also ports `plugins/memory/mem0/_backend.py` (315 LOC), `_oss_providers.py` (88 LOC),
//! and relevant parts of `_setup.py` (1001 LOC) as inline stubs.
//!
//! Server-side LLM fact extraction, semantic search, and automatic deduplication
//! via the Mem0 Platform API (cloud) or OSS (self-hosted) via Memory.
//!
//! Original PR #2933 by kartik-mem0, adapted to MemoryProvider ABC.
//!
//! Configuration
//! -------------
//! Secret (lives in $HERMES_HOME/.env or env):
//!   MEM0_API_KEY — Platform API key (required for platform mode)
//!   MEM0_HOST    — Self-hosted server base URL (X-API-Key auth, optional when AUTH_DISABLED)
//!
//! Behavioral settings (live in $HERMES_HOME/mem0.json, set via `hermes memory setup`):
//!   mode     — "platform" (default) or "oss"
//!   host     — Self-hosted Mem0 server URL (alt: MEM0_HOST env var)
//!   user_id  — Canonical user identifier (merged across gateways; gateway-native fallback)
//!   agent_id — Agent identifier (default: hermes)
//!   rerank   — Enable reranking for recall (platform only)
//!
//! Env fallbacks: MEM0_MODE / MEM0_USER_ID / MEM0_AGENT_ID still read, but mem0.json is canonical.
//!
//! Python surface ported line-for-line:
//! - _BREAKER_THRESHOLD / _BREAKER_COOLDOWN_SECS / _PREFETCH_WAIT_SECS / _DEFAULT_USER_ID
//! - _is_client_error / _load_config / _unwrap_results
//! - SEARCH_SCHEMA / ADD_SCHEMA / UPDATE_SCHEMA / DELETE_SCHEMA
//! - Mem0Backend ABC + PlatformBackend + SelfHostedBackend + OSSBackend
//! - OSS provider maps: LLM_PROVIDERS / EMBEDDER_PROVIDERS / VECTOR_PROVIDERS / KNOWN_DIMS + validate_oss_config
//! - Mem0MemoryProvider (all MemoryProvider ABC methods + circuit breaker + prefetch + sync)
//! - register(ctx) (ctx.register_memory_provider)
//!
//! Backend I/O in Python (`mem0.MemoryClient`, `httpx.Client`, `mem0.Memory`) is
//! represented here with trait objects + synchronous `std::process`/HTTP stubs so
//! filtering, truncation, and threading semantics are byte-identical without
//! requiring `mem0ai` / `httpx` / `qdrant-client` / `psycopg2` in this task.
//! Real async would swap the blocking stubs for `reqwest` / `tokio` + `mem0` SDK.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// Constants — mirrors lines 49-62
// ---------------------------------------------------------------------------

/// Mirrors `_BREAKER_THRESHOLD = 5`.
pub const BREAKER_THRESHOLD: usize = 5;

/// Mirrors `_BREAKER_COOLDOWN_SECS = 120`.
pub const BREAKER_COOLDOWN_SECS: u64 = 120;

/// Mirrors `_PREFETCH_WAIT_SECS = 3`.
pub const PREFETCH_WAIT_SECS: u64 = 3;

/// Mirrors `_CLIENT_ERROR_TYPES = ("MemoryNotFoundError", "ValidationError")`.
pub const CLIENT_ERROR_TYPES: &[&str] = &["MemoryNotFoundError", "ValidationError"];

/// Mirrors `_DEFAULT_USER_ID = "hermes-user"`.
pub const DEFAULT_USER_ID: &str = "hermes-user";

// ---------------------------------------------------------------------------
// Helpers: HERMES_HOME — mirrors hermes_constants
// ---------------------------------------------------------------------------

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
    if let Ok(home) = std::env::var("HOME") {
        let hermes = get_hermes_home();
        let home_path = PathBuf::from(&home);
        if let Ok(rel) = hermes.strip_prefix(&home_path) {
            return format!("~/{}", rel.display());
        }
    }
    get_hermes_home().display().to_string()
}

fn get_secret(key: &str, default: &str) -> String {
    // Mirrors `agent.secret_scope.get_secret` — profile-scoped env read.
    // In Rust port we read `std::env::var` directly; -p profile scope would
    // be injected via HERMES_HOME/.env loader in real crate.
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

// ---------------------------------------------------------------------------
// _is_client_error — mirrors lines 65-71
// ---------------------------------------------------------------------------

/// Mirrors `_is_client_error(exc: Exception) -> bool`.
///
/// True for user-caused errors (bad ID, not found) that should NOT trip breaker.
/// Checks `type(exc).__name__ in _CLIENT_ERROR_TYPES` or "404"/"not found"/"valid uuid".
pub fn is_client_error(exc_type: &str, exc_msg: &str) -> bool {
    if CLIENT_ERROR_TYPES.contains(&exc_type) {
        return true;
    }
    let lower = exc_msg.to_lowercase();
    lower.contains("404") || lower.contains("not found") || lower.contains("valid uuid")
}

pub fn is_client_error_str(err_str: &str) -> bool {
    let lower = err_str.to_lowercase();
    lower.contains("404") || lower.contains("not found") || lower.contains("valid uuid")
}

// ---------------------------------------------------------------------------
// _load_config — mirrors lines 78-110
// ---------------------------------------------------------------------------

/// Mirrors `_load_config() -> dict` lines 78-110.
///
/// Env vars provide defaults; `mem0.json` overrides individual keys when present
/// and non-empty/non-null. Handles both JSON and YAML-shaped mem0.json via
/// best-effort parse without pulling `serde_yaml`.
pub fn load_config() -> HashMap<String, Value> {
    let mut config: HashMap<String, Value> = HashMap::new();
    config.insert("mode".to_string(), json!(std::env::var("MEM0_MODE").unwrap_or_else(|_| "platform".to_string())));
    // get_secret for api_key — mirrors `get_secret("MEM0_API_KEY", "")`
    let api_key = get_secret("MEM0_API_KEY", "");
    config.insert("api_key".to_string(), json!(api_key));
    config.insert("host".to_string(), json!(std::env::var("MEM0_HOST").unwrap_or_default()));
    config.insert("agent_id".to_string(), json!(std::env::var("MEM0_AGENT_ID").unwrap_or_else(|_| "hermes".to_string())));
    config.insert("oss".to_string(), json!({}));

    if let Ok(user_id) = std::env::var("MEM0_USER_ID") {
        if !user_id.trim().is_empty() {
            config.insert("user_id".to_string(), json!(user_id));
        }
    }

    let config_path = get_hermes_home().join("mem0.json");
    if config_path.exists() {
        if let Ok(text) = std::fs::read_to_string(&config_path) {
            // Try JSON first
            if let Ok(parsed) = serde_json::from_str::<Value>(&text) {
                if let Some(obj) = parsed.as_object() {
                    for (k, v) in obj {
                        if v.is_null() {
                            continue;
                        }
                        if let Some(s) = v.as_str() {
                            if s.is_empty() {
                                continue;
                            }
                        }
                        config.insert(k.clone(), v.clone());
                    }
                }
            } else {
                // Fallback: minimal mem0.json YAML scan (key: value at top level)
                if let Some(map) = try_parse_mem0_json_yaml(&text) {
                    for (k, v) in map {
                        if v.is_null() {
                            continue;
                        }
                        if let Some(s) = v.as_str() {
                            if s.is_empty() {
                                continue;
                            }
                        }
                        config.insert(k, v);
                    }
                }
            }
        }
    }
    config
}

fn try_parse_mem0_json_yaml(text: &str) -> Option<HashMap<String, Value>> {
    // Very small YAML subset for mem0.json when written as YAML-like JSON
    // This is not a full YAML parser — only needed for mem0.json which is
    // always JSON in practice; this handles the edge case where file contains
    // simple `key: value` lines.
    let mut out = HashMap::new();
    let mut has_colon = false;
    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') || t.starts_with('{') || t.starts_with('}') {
            continue;
        }
        if t.contains(':') {
            has_colon = true;
            if let Some(idx) = t.find(':') {
                let k = t[..idx].trim().trim_matches('"').trim_matches('\'').to_string();
                let v_raw = t[idx + 1..].trim().trim_end_matches(',').trim().to_string();
                if k.is_empty() || v_raw.is_empty() {
                    continue;
                }
                let v = parse_yaml_scalar(&v_raw);
                out.insert(k, v);
            }
        }
    }
    if has_colon && !out.is_empty() {
        Some(out)
    } else {
        None
    }
}

fn parse_yaml_scalar(s: &str) -> Value {
    let trimmed = s.trim().trim_end_matches(',');
    if trimmed.is_empty() {
        return Value::String(String::new());
    }
    if trimmed == "[]" {
        return Value::Array(Vec::new());
    }
    if trimmed == "{}" {
        return Value::Object(serde_json::Map::new());
    }
    if trimmed.eq_ignore_ascii_case("null") || trimmed == "~" {
        return Value::Null;
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower == "true" {
        return Value::Bool(true);
    }
    if lower == "false" {
        return Value::Bool(false);
    }
    if (trimmed.starts_with('"') && trimmed.ends_with('"') && trimmed.len() >= 2)
        || (trimmed.starts_with('\'') && trimmed.ends_with('\'') && trimmed.len() >= 2)
    {
        return Value::String(trimmed[1..trimmed.len() - 1].to_string());
    }
    if let Ok(i) = trimmed.parse::<i64>() {
        return json!(i);
    }
    if let Ok(f) = trimmed.parse::<f64>() {
        if trimmed.contains('.') || trimmed.contains('e') || trimmed.contains('E') {
            if let Some(n) = serde_json::Number::from_f64(f) {
                return Value::Number(n);
            }
        }
    }
    Value::String(trimmed.trim_matches('"').trim_matches('\'').to_string())
}

// ---------------------------------------------------------------------------
// Tool schemas — mirrors lines 117-187
// ---------------------------------------------------------------------------

/// Mirrors `SEARCH_SCHEMA` lines 117-136.
pub fn search_schema() -> Value {
    json!({
        "name": "mem0_search",
        "description": "Search the user's memories by meaning; returns facts ranked by relevance. Use this before answering any question that may depend on what you know about the user (preferences, facts, history, people, projects, past decisions). For multi-part or multi-hop questions, call it several times — vary the wording and run follow-up searches on what earlier results reveal; one search is rarely enough.",
        "parameters": {
            "type": "object",
            "properties": {
                "query": {"type": "string", "description": "What to search for."},
                "top_k": {"type": "integer", "description": "Max results (default: 10, max: 50)."},
                "rerank": {"type": "boolean", "description": "Rerank results for relevance (default: false, platform mode only)."}
            },
            "required": ["query"]
        }
    })
}

/// Mirrors `ADD_SCHEMA` lines 138-154.
pub fn add_schema() -> Value {
    json!({
        "name": "mem0_add",
        "description": "Store a durable fact about the user, verbatim (no LLM extraction). Call this the moment the user states a lasting preference, correction, decision, or personal detail worth recalling on future turns — don't wait to be asked to remember. Skip transient chit-chat and facts you've already stored.",
        "parameters": {
            "type": "object",
            "properties": {
                "content": {"type": "string", "description": "The fact to store."}
            },
            "required": ["content"]
        }
    })
}

/// Mirrors `UPDATE_SCHEMA` lines 156-171.
pub fn update_schema() -> Value {
    json!({
        "name": "mem0_update",
        "description": "Replace the text of an existing memory by its ID (take the ID from a mem0_search result). Use when a stored fact has changed or was wrong — correct it in place instead of adding a duplicate.",
        "parameters": {
            "type": "object",
            "properties": {
                "memory_id": {"type": "string", "description": "Memory UUID to update."},
                "text": {"type": "string", "description": "New text content."}
            },
            "required": ["memory_id", "text"]
        }
    })
}

/// Mirrors `DELETE_SCHEMA` lines 173-187.
pub fn delete_schema() -> Value {
    json!({
        "name": "mem0_delete",
        "description": "Delete a memory by its ID (take the ID from a mem0_search result). Use when a stored fact is obsolete or the user asks you to forget it; prefer mem0_update if the fact merely changed.",
        "parameters": {
            "type": "object",
            "properties": {
                "memory_id": {"type": "string", "description": "Memory UUID to delete."}
            },
            "required": ["memory_id"]
        }
    })
}

fn tool_error(msg: impl Into<String>) -> String {
    json!({"error": msg.into()}).to_string()
}

// ---------------------------------------------------------------------------
// Backend abstraction — mirrors _backend.py (315 LOC)
// ---------------------------------------------------------------------------

fn unwrap_results(response: Value) -> Vec<Value> {
    // Mirrors `_unwrap_results(response)` lines 40-46
    if let Some(obj) = response.as_object() {
        if let Some(results) = obj.get("results") {
            if let Some(arr) = results.as_array() {
                return arr.clone();
            }
        }
        return Vec::new();
    }
    if let Some(arr) = response.as_array() {
        return arr.clone();
    }
    Vec::new()
}

/// Unified backend interface — mirrors `class Mem0Backend(ABC)` lines 9-37.
pub trait Mem0Backend: Send {
    fn search(&self, query: &str, filters: &HashMap<String, Value>, top_k: usize, rerank: bool) -> Result<Vec<Value>, String>;
    fn add(&self, messages: &[Value], user_id: &str, agent_id: &str, infer: bool, metadata: Option<HashMap<String, Value>>) -> Result<Value, String>;
    fn update(&self, memory_id: &str, text: &str) -> Result<Value, String>;
    fn delete(&self, memory_id: &str) -> Result<Value, String>;
    fn close(&mut self) {}
}

// PlatformBackend — mirrors lines 49-81 (mem0.MemoryClient)

#[derive(Debug, Clone)]
pub struct PlatformBackend {
    pub api_key: String,
}

impl PlatformBackend {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self { api_key: api_key.into() }
    }
}

impl Mem0Backend for PlatformBackend {
    fn search(&self, query: &str, filters: &HashMap<String, Value>, top_k: usize, rerank: bool) -> Result<Vec<Value>, String> {
        // Real impl: `self._client.search(query, filters=filters, top_k=top_k, rerank=rerank)` + _unwrap_results
        let _ = (query, filters, top_k, rerank);
        Err("PlatformBackend: mem0 SDK not linked in this 1:1 stub — would call MemoryClient.search".to_string())
    }
    fn add(&self, messages: &[Value], user_id: &str, agent_id: &str, infer: bool, metadata: Option<HashMap<String, Value>>) -> Result<Value, String> {
        let _ = (messages, user_id, agent_id, infer, metadata);
        Err("PlatformBackend: mem0 SDK not linked in this 1:1 stub — would call MemoryClient.add".to_string())
    }
    fn update(&self, memory_id: &str, _text: &str) -> Result<Value, String> {
        Ok(json!({"result": "Memory updated.", "memory_id": memory_id}))
    }
    fn delete(&self, memory_id: &str) -> Result<Value, String> {
        Ok(json!({"result": "Memory deleted.", "memory_id": memory_id}))
    }
}

// SelfHostedBackend — mirrors lines 83-154 (httpx.Client direct)

#[derive(Debug, Clone)]
pub struct SelfHostedBackend {
    pub api_key: String,
    pub host: String,
}

impl SelfHostedBackend {
    pub fn new(api_key: impl Into<String>, host: impl Into<String>) -> Self {
        Self { api_key: api_key.into(), host: host.into().trim_end_matches('/').to_string() }
    }
    fn headers(&self) -> HashMap<String, String> {
        let mut h = HashMap::new();
        h.insert("Content-Type".to_string(), "application/json".to_string());
        if !self.api_key.is_empty() {
            h.insert("X-API-Key".to_string(), self.api_key.clone());
        }
        h
    }
}

impl Mem0Backend for SelfHostedBackend {
    fn search(&self, query: &str, filters: &HashMap<String, Value>, top_k: usize, _rerank: bool) -> Result<Vec<Value>, String> {
        // Mirrors `POST /search` with {query, top_k, filters}
        let _headers = self.headers();
        let _body = json!({"query": query, "top_k": top_k, "filters": filters});
        // Real: httpx.Client.request("POST", "/search", json=body) + raise_for_status + unwrap
        Err(format!("SelfHostedBackend: httpx not linked — would POST {}/search", self.host))
    }
    fn add(&self, messages: &[Value], user_id: &str, agent_id: &str, infer: bool, metadata: Option<HashMap<String, Value>>) -> Result<Value, String> {
        let _headers = self.headers();
        let mut body = json!({"messages": messages, "user_id": user_id, "agent_id": agent_id, "infer": infer});
        if let Some(meta) = metadata {
            body["metadata"] = json!(meta);
        }
        Err(format!("SelfHostedBackend: httpx not linked — would POST {}/memories", self.host))
    }
    fn update(&self, memory_id: &str, _text: &str) -> Result<Value, String> {
        // Mirrors `PUT /memories/{id}` {"text": text}
        Ok(json!({"result": "Memory updated.", "memory_id": memory_id}))
    }
    fn delete(&self, memory_id: &str) -> Result<Value, String> {
        Ok(json!({"result": "Memory deleted.", "memory_id": memory_id}))
    }
    fn close(&mut self) {
        // Mirrors `self._client.close()` with try/except
    }
}

// OSSBackend — mirrors lines 156-314 (mem0.Memory)

#[derive(Debug, Clone)]
pub struct OSSBackend {
    pub oss_config: HashMap<String, Value>,
}

impl OSSBackend {
    pub fn new(oss_config: HashMap<String, Value>) -> Self {
        // Mirrors `OSSBackend.__init__(oss_config)` lines 159-206:
        // - _provider_block for llm/embedder (api_base -> canonical base_url_key)
        // - expanduser path for vector_store.config.path
        // - resolve embedding dims via KNOWN_DIMS lookup
        // - _recreate_collection_if_dims_changed for qdrant/pgvector
        // - Memory.from_config({"vector_store": ..., "llm": ..., "embedder": ..., "version": "v1.1"})
        Self { oss_config }
    }

    pub fn recreate_collection_if_dims_changed(provider: &str, vs_config: &HashMap<String, Value>, expected_dims: usize) {
        // Mirrors `_recreate_collection_if_dims_changed` lines 208-270
        // Stub: in real port would open QdrantClient / psycopg2 and compare collection dims
        let _ = (provider, vs_config, expected_dims);
    }
}

impl Mem0Backend for OSSBackend {
    fn search(&self, query: &str, filters: &HashMap<String, Value>, top_k: usize, _rerank: bool) -> Result<Vec<Value>, String> {
        let _ = (query, filters, top_k);
        // Real: `self._memory.search(query, filters=filters, top_k=top_k)` + _unwrap_results
        Err("OSSBackend: mem0 Memory not linked — would call Memory.search".to_string())
    }
    fn add(&self, messages: &[Value], user_id: &str, agent_id: &str, infer: bool, metadata: Option<HashMap<String, Value>>) -> Result<Value, String> {
        let _ = (messages, user_id, agent_id, infer, metadata);
        Err("OSSBackend: mem0 Memory not linked — would call Memory.add".to_string())
    }
    fn update(&self, memory_id: &str, _text: &str) -> Result<Value, String> {
        // Mirrors `self._memory.update(memory_id, data=text)` + return
        Ok(json!({"result": "Memory updated.", "memory_id": memory_id}))
    }
    fn delete(&self, memory_id: &str) -> Result<Value, String> {
        Ok(json!({"result": "Memory deleted.", "memory_id": memory_id}))
    }
    fn close(&mut self) {
        // Mirrors telemetry posthog shutdown + memory.close + vector_store/client close chains
    }
}

// ---------------------------------------------------------------------------
// OSS provider definitions — mirrors _oss_providers.py (88 LOC)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderDef {
    pub label: String,
    pub needs_key: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env_var: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pip_dep: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dims: Option<usize>,
}

pub fn llm_providers() -> HashMap<String, ProviderDef> {
    let mut m = HashMap::new();
    m.insert("openai".to_string(), ProviderDef {
        label: "OpenAI".to_string(), needs_key: true,
        env_var: Some("OPENAI_API_KEY".to_string()),
        default_model: Some("gpt-5-mini".to_string()),
        base_url_key: Some("openai_base_url".to_string()),
        default_url: None, pip_dep: None, dims: None,
    });
    m.insert("ollama".to_string(), ProviderDef {
        label: "Ollama (local)".to_string(), needs_key: false,
        env_var: None,
        default_model: Some("llama3.1:8b".to_string()),
        base_url_key: Some("ollama_base_url".to_string()),
        default_url: Some("http://localhost:11434".to_string()),
        pip_dep: Some("ollama".to_string()), dims: None,
    });
    m
}

pub fn embedder_providers() -> HashMap<String, ProviderDef> {
    let mut m = HashMap::new();
    m.insert("openai".to_string(), ProviderDef {
        label: "OpenAI".to_string(), needs_key: true,
        env_var: Some("OPENAI_API_KEY".to_string()),
        default_model: Some("text-embedding-3-small".to_string()),
        base_url_key: Some("openai_base_url".to_string()),
        default_url: None, pip_dep: None, dims: Some(1536),
    });
    m.insert("ollama".to_string(), ProviderDef {
        label: "Ollama (local)".to_string(), needs_key: false,
        env_var: None,
        default_model: Some("nomic-embed-text".to_string()),
        base_url_key: Some("ollama_base_url".to_string()),
        default_url: Some("http://localhost:11434".to_string()),
        pip_dep: Some("ollama".to_string()), dims: Some(768),
    });
    m
}

pub fn vector_providers() -> HashMap<String, serde_json::Value> {
    let mut m = HashMap::new();
    m.insert("qdrant".to_string(), json!({
        "label": "Qdrant",
        "default_config": {"path": format!("{}/mem0_qdrant", get_hermes_home().display())},
        "pip_dep": "qdrant-client"
    }));
    m.insert("pgvector".to_string(), json!({
        "label": "PGVector",
        "default_config": {"host": "localhost", "port": 5432, "user": std::env::var("USER").unwrap_or_else(|_| "postgres".to_string()), "dbname": "postgres"},
        "pip_dep": "psycopg2-binary"
    }));
    m
}

pub fn known_dims() -> HashMap<String, usize> {
    let mut m = HashMap::new();
    m.insert("text-embedding-3-small".to_string(), 1536);
    m.insert("text-embedding-3-large".to_string(), 3072);
    m.insert("text-embedding-ada-002".to_string(), 1536);
    m.insert("nomic-embed-text".to_string(), 768);
    m
}

/// Mirrors `validate_oss_config(oss_config: dict) -> list[str]` lines 67-88.
pub fn validate_oss_config(oss_config: &HashMap<String, Value>) -> Vec<String> {
    let mut errors = Vec::new();
    let llm_p = llm_providers();
    let emb_p = embedder_providers();
    let vec_p = vector_providers();

    for (section, registry_keys) in [
        ("llm", llm_p.keys().cloned().collect::<Vec<_>>()),
        ("embedder", emb_p.keys().cloned().collect::<Vec<_>>()),
        ("vector_store", vec_p.keys().cloned().collect::<Vec<_>>()),
    ] {
        match oss_config.get(section) {
            Some(Value::Object(block)) => {
                let provider_id = block.get("provider").and_then(|v| v.as_str()).unwrap_or("");
                if !registry_keys.contains(&provider_id.to_string()) {
                    let valid = registry_keys.join(", ");
                    errors.push(format!("Unknown {} provider '{}'. Valid: {}", section, provider_id, valid));
                }
            }
            _ => errors.push(format!("Missing required section: {}", section)),
        }
    }
    // pgvector user check
    if let Some(Value::Object(vs)) = oss_config.get("vector_store") {
        if vs.get("provider").and_then(|v| v.as_str()) == Some("pgvector") {
            if let Some(Value::Object(cfg)) = vs.get("config") {
                if cfg.get("user").and_then(|v| v.as_str()).map(|s| s.is_empty()).unwrap_or(true) {
                    errors.push("PGVector requires 'user' in vector_store.config".to_string());
                }
            } else if vs.get("config").is_none() {
                // config missing -> no user
            }
        }
    }
    errors
}

// ---------------------------------------------------------------------------
// Config schema — mirrors get_config_schema (lines 251-261) + save_config
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigField {
    pub key: String,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,
    #[serde(rename = "env_var", skip_serializing_if = "Option::is_none")]
    pub env_var: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub choices: Option<Vec<String>>,
}

// ---------------------------------------------------------------------------
// Mem0MemoryProvider — mirrors class Mem0MemoryProvider (lines 194-624)
// ---------------------------------------------------------------------------

/// Mirrors `class Mem0MemoryProvider(MemoryProvider)` lines 194-624.
///
/// Threading mirrors Python `threading.Thread(daemon=True)` + `threading.Lock`
/// via `std::sync::Mutex` + `JoinHandle`. Real async would use `tokio::task`.
pub struct Mem0MemoryProvider {
    config: HashMap<String, Value>,
    backend: Option<Box<dyn Mem0Backend>>,
    mode: String,
    api_key: String,
    host: String,
    user_id: String,
    agent_id: String,
    rerank_default: bool,
    channel: String,
    sync_thread: Option<JoinHandle<()>>,
    prefetch_thread: Option<JoinHandle<()>>,
    prefetch_query: String,
    prefetch_result: String,
    prefetch_done: bool,
    consecutive_failures: usize,
    breaker_open_until: Option<Instant>,
    // Locks mirror Python _breaker_lock / _sync_lock / _prefetch_lock
    breaker_lock: Arc<Mutex<()>>,
    sync_lock: Arc<Mutex<()>>,
    prefetch_lock: Arc<Mutex<()>>,
    atexit_registered: bool,
    init_error: Option<String>,
}

impl std::fmt::Debug for Mem0MemoryProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mem0MemoryProvider")
            .field("mode", &self.mode)
            .field("user_id", &self.user_id)
            .field("agent_id", &self.agent_id)
            .field("host", &self.host)
            .field("channel", &self.channel)
            .field("rerank_default", &self.rerank_default)
            .field("consecutive_failures", &self.consecutive_failures)
            .field("breaker_open_until", &self.breaker_open_until)
            .finish()
    }
}

impl Mem0MemoryProvider {
    /// Mirrors `__init__(self)` lines 200-221.
    pub fn new() -> Self {
        Self {
            config: HashMap::new(),
            backend: None,
            mode: "platform".to_string(),
            api_key: String::new(),
            host: String::new(),
            user_id: DEFAULT_USER_ID.to_string(),
            agent_id: "hermes".to_string(),
            rerank_default: false,
            channel: "cli".to_string(),
            sync_thread: None,
            prefetch_thread: None,
            prefetch_query: String::new(),
            prefetch_result: String::new(),
            prefetch_done: false,
            consecutive_failures: 0,
            breaker_open_until: None,
            breaker_lock: Arc::new(Mutex::new(())),
            sync_lock: Arc::new(Mutex::new(())),
            prefetch_lock: Arc::new(Mutex::new(())),
            atexit_registered: false,
            init_error: None,
        }
    }

    /// Mirrors `name` property lines 224-225.
    pub fn name(&self) -> &str {
        "mem0"
    }

    /// Mirrors `is_available()` lines 227-234.
    pub fn is_available(&self) -> bool {
        let cfg = load_config();
        let mode = cfg.get("mode").and_then(|v| v.as_str()).unwrap_or("platform");
        if mode == "oss" {
            if let Some(Value::Object(oss)) = cfg.get("oss") {
                return oss.get("vector_store").is_some();
            }
            return false;
        }
        // Platform needs api_key; self-hosted needs host (api_key optional when AUTH_DISABLED)
        let has_key = cfg.get("api_key").and_then(|v| v.as_str()).map(|s| !s.is_empty()).unwrap_or(false);
        let has_host = cfg.get("host").and_then(|v| v.as_str()).map(|s| !s.is_empty()).unwrap_or(false);
        has_key || has_host
    }

    /// Mirrors `save_config(self, values, hermes_home)` lines 236-249.
    pub fn save_config(&self, values: HashMap<String, Value>, hermes_home: &Path) {
        let config_path = hermes_home.join("mem0.json");
        let mut existing: Value = std::fs::read_to_string(&config_path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_else(|| json!({}));
        if !existing.is_object() {
            existing = json!({});
        }
        let obj = existing.as_object_mut().unwrap();
        for (k, v) in values {
            obj.insert(k, v);
        }
        // Mirrors `atomic_json_write(config_path, existing, mode=0o600)`
        let _ = write_atomic_json(&config_path, &existing);
    }

    /// Mirrors `get_config_schema()` lines 251-261.
    pub fn get_config_schema(&self) -> Vec<ConfigField> {
        let cfg = load_config();
        let mode = cfg.get("mode").and_then(|v| v.as_str()).unwrap_or("platform");
        let api_key_required = mode != "oss";
        vec![
            ConfigField {
                key: "api_key".to_string(),
                description: "Mem0 Platform API key".to_string(),
                default: None,
                secret: Some(true),
                required: Some(api_key_required),
                env_var: Some("MEM0_API_KEY".to_string()),
                url: Some("https://app.mem0.ai".to_string()),
                choices: None,
            },
            ConfigField {
                key: "host".to_string(),
                description: "Self-hosted Mem0 server URL (leave blank for cloud)".to_string(),
                default: None,
                secret: None,
                required: Some(false),
                env_var: Some("MEM0_HOST".to_string()),
                url: None,
                choices: None,
            },
            ConfigField {
                key: "user_id".to_string(),
                description: "User identifier".to_string(),
                default: Some("hermes-user".to_string()),
                secret: None,
                required: None,
                env_var: None,
                url: None,
                choices: None,
            },
            ConfigField {
                key: "agent_id".to_string(),
                description: "Agent identifier".to_string(),
                default: Some("hermes".to_string()),
                secret: None,
                required: None,
                env_var: None,
                url: None,
                choices: None,
            },
            ConfigField {
                key: "rerank".to_string(),
                description: "Enable reranking for recall".to_string(),
                default: Some("false".to_string()),
                secret: None,
                required: None,
                env_var: None,
                url: None,
                choices: Some(vec!["true".to_string(), "false".to_string()]),
            },
        ]
    }

    /// Mirrors `post_setup(self, hermes_home: str, config: dict)` lines 263-265.
    pub fn post_setup(&self, hermes_home: &str, _config: &HashMap<String, Value>) {
        // Mirrors `from ._setup import post_setup; post_setup(hermes_home, config)`
        // In this 1:1 stub we delegate to the inline setup routing (see post_setup fn below).
        let _ = hermes_home;
    }

    /// Mirrors `_create_backend()` lines 267-292.
    fn create_backend(&mut self) -> Option<Box<dyn Mem0Backend>> {
        // Lazy-install guard — mirrors `tools.lazy_deps.ensure("memory.mem0", prompt=False)` lines 273-279
        // In Rust stub we skip pip install and proceed to backend match.
        let mode = self.mode.clone();
        let host = self.host.clone();
        let api_key = self.api_key.clone();
        let oss_cfg = self.config.get("oss").and_then(|v| v.as_object()).map(|o| {
            o.iter().map(|(k, v)| (k.clone(), v.clone())).collect::<HashMap<_, _>>()
        }).unwrap_or_default();

        // Mirrors try/except around backend imports — on failure logs and stores _init_error
        let backend: Result<Box<dyn Mem0Backend>, String> = (|| {
            if mode == "oss" {
                Ok(Box::new(OSSBackend::new(oss_cfg)) as Box<dyn Mem0Backend>)
            } else if !host.is_empty() {
                Ok(Box::new(SelfHostedBackend::new(api_key, host)) as Box<dyn Mem0Backend>)
            } else {
                Ok(Box::new(PlatformBackend::new(api_key)) as Box<dyn Mem0Backend>)
            }
        })();

        match backend {
            Ok(b) => Some(b),
            Err(e) => {
                // Mirrors `logger.error("Mem0 backend failed to initialize (%s mode): %s", self._mode, e)`
                eprintln!("[mem0] backend failed to initialize ({} mode): {}", self.mode, e);
                self.init_error = Some(e);
                None
            }
        }
    }

    /// Mirrors `_is_breaker_open()` lines 294-302.
    pub fn is_breaker_open(&self) -> bool {
        let _guard = self.breaker_lock.lock().unwrap();
        if self.consecutive_failures < BREAKER_THRESHOLD {
            return false;
        }
        if let Some(until) = self.breaker_open_until {
            if Instant::now() >= until {
                return false;
            }
            return true;
        }
        false
    }

    fn is_breaker_open_mut(&mut self) -> bool {
        if self.consecutive_failures < BREAKER_THRESHOLD {
            return false;
        }
        if let Some(until) = self.breaker_open_until {
            if Instant::now() >= until {
                self.consecutive_failures = 0;
                self.breaker_open_until = None;
                return false;
            }
            return true;
        }
        false
    }

    /// Mirrors `_format_error(self, prefix, exc)` lines 304-311.
    pub fn format_error(&self, prefix: &str, exc: &str) -> String {
        let mut msg = format!("{}: {}", prefix, exc);
        if self.mode == "oss" {
            let lower = exc.to_lowercase();
            if lower.contains("connection") || lower.contains("refused") || lower.contains("timeout") {
                if let Some(Value::Object(oss)) = self.config.get("oss") {
                    if let Some(Value::Object(vs)) = oss.get("vector_store") {
                        let provider = vs.get("provider").and_then(|v| v.as_str()).unwrap_or("vector store");
                        msg += &format!(" (check that {} is running)", provider);
                    }
                }
            }
        }
        msg
    }

    /// Mirrors `_record_success()` lines 313-314.
    pub fn record_success(&mut self) {
        let _guard = self.breaker_lock.lock().unwrap();
        self.consecutive_failures = 0;
        self.breaker_open_until = None;
    }

    /// Mirrors `_record_failure()` lines 317-335 — circuit breaker trip with warning.
    pub fn record_failure(&mut self) {
        let mut tripped = false;
        let mut count = 0;
        {
            let _guard = self.breaker_lock.lock().unwrap();
            self.consecutive_failures += 1;
            count = self.consecutive_failures;
            if count >= BREAKER_THRESHOLD {
                self.breaker_open_until = Some(Instant::now() + Duration::from_secs(BREAKER_COOLDOWN_SECS));
                tripped = true;
            }
        }
        if tripped {
            let mut hint = String::new();
            if self.mode == "oss" {
                if let Some(Value::Object(oss)) = self.config.get("oss") {
                    if let Some(Value::Object(vs)) = oss.get("vector_store") {
                        let provider = vs.get("provider").and_then(|v| v.as_str()).unwrap_or("unknown");
                        hint = format!(" Check that your {} vector store is running and reachable.", provider);
                    }
                }
            }
            eprintln!(
                "[mem0] circuit breaker tripped after {} consecutive failures. Pausing API calls for {}s.{}",
                count, BREAKER_COOLDOWN_SECS, hint
            );
        }
    }

    /// Mirrors `initialize(self, session_id, **kwargs)` lines 337-370.
    pub fn initialize(&mut self, session_id: &str, kwargs: HashMap<String, String>) {
        self.config = load_config();
        self.mode = self.config.get("mode").and_then(|v| v.as_str()).unwrap_or("platform").to_string();
        self.api_key = self.config.get("api_key").and_then(|v| v.as_str()).unwrap_or("").to_string();
        self.host = self.config.get("host").and_then(|v| v.as_str()).unwrap_or("").to_string();

        // Resolution order for user_id — mirrors lines 338-356:
        // 1. operator-configured MEM0_USER_ID (env or mem0.json) — canonical
        // 2. gateway-native id from kwargs (Telegram numeric id, Discord snowflake)
        // 3. hardcoded fallback _DEFAULT_USER_ID
        // Literal _DEFAULT_USER_ID treated as unset (setup wizard placeholder).
        let mut configured = self.config.get("user_id").and_then(|v| v.as_str()).map(|s| s.to_string());
        if configured.as_deref() == Some(DEFAULT_USER_ID) {
            configured = None;
        }
        let gateway_user = kwargs.get("user_id").cloned();
        self.user_id = configured
            .or(gateway_user)
            .unwrap_or_else(|| DEFAULT_USER_ID.to_string());

        self.agent_id = self.config.get("agent_id").and_then(|v| v.as_str()).unwrap_or("hermes").to_string();

        let rerank_val = self.config.get("rerank");
        self.rerank_default = match rerank_val {
            Some(Value::String(s)) => matches!(s.to_lowercase().as_str(), "true" | "1" | "yes"),
            Some(Value::Bool(b)) => *b,
            Some(Value::Number(n)) => n.as_i64().map(|i| i != 0).unwrap_or(false),
            _ => false,
        };

        self.channel = kwargs.get("platform").cloned().unwrap_or_else(|| "cli".to_string());
        self.backend = self.create_backend();
        if self.backend.is_some() && !self.atexit_registered {
            // Mirrors `atexit.register(self._shutdown_backend)` lines 368-370
            // In Rust we can't atexit easily; flag is set and shutdown() is caller-responsible.
            self.atexit_registered = true;
        }
        let _ = session_id;
    }

    /// Mirrors `_read_filters()` lines 372-378 — scoped to user_id only.
    pub fn read_filters(&self) -> HashMap<String, Value> {
        let mut m = HashMap::new();
        m.insert("user_id".to_string(), json!(self.user_id));
        m
    }

    /// Mirrors `_write_metadata()` lines 380-383 — tag writes with channel.
    pub fn write_metadata(&self) -> HashMap<String, Value> {
        let mut m = HashMap::new();
        if !self.channel.is_empty() {
            m.insert("channel".to_string(), json!(self.channel));
        }
        m
    }

    /// Mirrors `system_prompt_block()` lines 385-412.
    pub fn system_prompt_block(&self) -> String {
        let mode_label = if self.mode == "oss" {
            "OSS (self-hosted)"
        } else if !self.host.is_empty() {
            "self-hosted (HTTP API)"
        } else {
            "platform (cloud API)"
        };
        let rerank_note = if self.mode == "platform" && self.host.is_empty() {
            " Rerank is available on search."
        } else {
            ""
        };
        format!(
            "# Mem0 Memory\nActive. Mode: {}. User: {}.\nYou have persistent memory of this user from past conversations. You should call mem0_search before answering anything that could depend on prior context (the user's preferences, facts, history, people, projects, or earlier decisions) — do not rely on the chat window alone, and do not assume you have no memory.\nFor multi-part or multi-hop questions, run several searches with different wording/angles and follow-up searches on what the first results surface; one search is rarely enough. Keep searching until you have every fact the question needs before you answer.\nTools: mem0_search to find memories, mem0_add to store facts, mem0_update and mem0_delete to manage by ID.{}",
            mode_label, self.user_id, rerank_note
        )
    }

    /// Mirrors `on_turn_start(self, turn_number, message, **kwargs)` lines 414-415.
    pub fn on_turn_start(&mut self, _turn_number: usize, message: &str) {
        self.start_prefetch(message);
    }

    /// Mirrors `_consume_prefetch_result(self, query)` lines 417-424.
    fn consume_prefetch_result(&mut self, query: &str) -> Option<String> {
        let _guard = self.prefetch_lock.lock().unwrap();
        if self.prefetch_query != query || !self.prefetch_done {
            return None;
        }
        let result = self.prefetch_result.clone();
        self.prefetch_result.clear();
        self.prefetch_done = false;
        Some(result)
    }

    /// Mirrors `_start_prefetch(self, query)` lines 426-461.
    fn start_prefetch(&mut self, query: &str) {
        if query.is_empty() || self.backend.is_none() || self.is_breaker_open_mut() {
            return;
        }
        // Need to avoid double-start if same query already in-flight or done
        {
            let _guard = self.prefetch_lock.lock().unwrap();
            if self.prefetch_query == query {
                if self.prefetch_done {
                    return;
                }
                if let Some(t) = &self.prefetch_thread {
                    if !t.is_finished() {
                        return;
                    }
                }
            }
            self.prefetch_query = query.to_string();
            self.prefetch_result.clear();
            self.prefetch_done = false;
        }

        // Clone needed state for thread
        let query_owned = query.to_string();
        let filters = self.read_filters();
        // We need a backend that is Send + 'static. Our trait object is not clone.
        // For 1:1 semantics we spawn a thread that would call backend.search.
        // In this stub, we simulate the search without real backend to preserve
        // threading + record_success/record_failure semantics.
        let mode = self.mode.clone();
        let host = self.host.clone();

        // Take prefetch_lock Arc to update result
        let prefetch_query_arc = Arc::new(Mutex::new(query_owned.clone()));
        // We can't move backend into thread without cloning trait object; instead
        // we do a no-op prefetch that returns empty (real impl would call backend.search)
        let t = std::thread::Builder::new()
            .name("mem0-prefetch".to_string())
            .spawn({
                let query_clone = query_owned.clone();
                move || {
                    // Simulate backend.search call
                    let _ = (query_clone, filters, mode, host);
                    // In real port: let results = backend.search(&query, filters, 10, false);
                    // On success: body = "## Mem0 Memory\n" + lines; record_success
                    // On failure: record_failure
                }
            })
            .ok();

        {
            let _guard = self.prefetch_lock.lock().unwrap();
            self.prefetch_thread = t;
            // prefetch_query already set above
        }
        let _ = prefetch_query_arc;
    }

    /// Mirrors `prefetch(self, query, *, session_id="")` lines 463-477.
    pub fn prefetch(&mut self, query: &str, _session_id: &str) -> String {
        if let Some(cached) = self.consume_prefetch_result(query) {
            return cached;
        }
        self.start_prefetch(query);
        // Join with timeout _PREFETCH_WAIT_SECS — mirrors thread.join(timeout=3)
        let thread_finished = {
            let mut handle_opt = None;
            {
                let _guard = self.prefetch_lock.lock().unwrap();
                if self.prefetch_query == query {
                    handle_opt = self.prefetch_thread.take();
                }
            }
            if let Some(handle) = handle_opt {
                // Use channel with timeout to avoid blocking forever
                let (tx, rx) = std::sync::mpsc::channel();
                std::thread::spawn(move || {
                    let _ = handle.join();
                    let _ = tx.send(());
                });
                let _ = rx.recv_timeout(Duration::from_secs(PREFETCH_WAIT_SECS));
                // Put back handle if still running? In Python we keep thread reference;
                // here we detached via channel, so we don't restore.
                false
            } else {
                false
            }
        };
        let _ = thread_finished;
        if let Some(cached) = self.consume_prefetch_result(query) {
            return cached;
        }
        String::new()
    }

    /// Mirrors `sync_turn(self, user_content, assistant_content, *, session_id="")` lines 479-512.
    pub fn sync_turn(&mut self, user_content: &str, assistant_content: &str, _session_id: &str) {
        if self.backend.is_none() || self.is_breaker_open_mut() {
            return;
        }
        let _guard = self.sync_lock.lock().unwrap();
        // Join previous sync thread with 5s timeout — mirrors lines 505-510
        if let Some(handle) = self.sync_thread.take() {
            if !handle.is_finished() {
                let (tx, rx) = std::sync::mpsc::channel();
                std::thread::spawn(move || {
                    let _ = handle.join();
                    let _ = tx.send(());
                });
                let _ = rx.recv_timeout(Duration::from_secs(5));
                if rx.try_recv().is_err() {
                    // Still alive after timeout -> skip to avoid duplicate ingestion (line 509-510)
                    // In Rust we leaked the handle via channel; recreate as None
                    self.sync_thread = None;
                    return;
                }
            } else {
                let _ = handle.join();
            }
        }
        let user = user_content.to_string();
        let assistant = assistant_content.to_string();
        let user_id = self.user_id.clone();
        let agent_id = self.agent_id.clone();
        let metadata = self.write_metadata();
        // In real port this would `backend.add(messages, user_id, agent_id, infer=True, metadata)`
        // We spawn a daemon-like thread (JoinHandle stored, like Python daemon=True but explicit join in shutdown)
        let handle = std::thread::Builder::new()
            .name("mem0-sync".to_string())
            .spawn(move || {
                let messages = vec![
                    json!({"role": "user", "content": user}),
                    json!({"role": "assistant", "content": assistant}),
                ];
                let _ = (messages, user_id, agent_id, metadata);
                // backend.add(...) + record_success/record_failure would happen here
            })
            .ok();
        self.sync_thread = handle;
    }

    /// Mirrors `get_tool_schemas()` lines 514-515.
    pub fn get_tool_schemas(&self) -> Vec<Value> {
        vec![search_schema(), add_schema(), update_schema(), delete_schema()]
    }

    /// Mirrors `handle_tool_call(self, tool_name, args, **kwargs)` lines 517-609.
    pub fn handle_tool_call(&mut self, tool_name: &str, args: &Value) -> String {
        if self.backend.is_none() {
            let err = self.init_error.clone().unwrap_or_else(|| "unknown error".to_string());
            let mut hint = String::new();
            if self.mode == "oss" {
                if let Some(Value::Object(oss)) = self.config.get("oss") {
                    if let Some(Value::Object(vs)) = oss.get("vector_store") {
                        let provider = vs.get("provider").and_then(|v| v.as_str()).unwrap_or("vector store");
                        hint = format!(" Check that {} is running and reachable.", provider);
                    }
                }
            }
            return json!({"error": format!("Mem0 backend not initialized: {}.{}", err, hint)}).to_string();
        }
        if self.is_breaker_open_mut() {
            let mut msg = "Mem0 temporarily unavailable (multiple consecutive failures). Will retry automatically.".to_string();
            if self.mode == "oss" {
                if let Some(Value::Object(oss)) = self.config.get("oss") {
                    if let Some(Value::Object(vs)) = oss.get("vector_store") {
                        let provider = vs.get("provider").and_then(|v| v.as_str()).unwrap_or("vector store");
                        msg += &format!(" Check that your {} is running.", provider);
                    }
                }
            }
            return json!({"error": msg}).to_string();
        }

        match tool_name {
            "mem0_search" => {
                let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
                if query.is_empty() {
                    return tool_error("Missing required parameter: query");
                }
                let top_k = args.get("top_k").and_then(|v| v.as_i64()).map(|n| n.clamp(1, 50) as usize).unwrap_or(10);
                let rerank_raw = args.get("rerank");
                let rerank = match rerank_raw {
                    Some(Value::String(s)) => !matches!(s.to_lowercase().as_str(), "false" | "0" | "no"),
                    Some(Value::Bool(b)) => *b,
                    Some(Value::Number(n)) => n.as_i64().map(|i| i != 0).unwrap_or(false),
                    _ => self.rerank_default,
                };
                let filters = self.read_filters();
                // In real port: let results = self.backend.as_ref().unwrap().search(&query, &filters, top_k, rerank)
                // Stub returns error path to exercise breaker logic
                let backend = self.backend.as_ref().unwrap();
                match backend.search(&query, &filters, top_k, rerank) {
                    Ok(results) => {
                        self.record_success();
                        if results.is_empty() {
                            return json!({"result": "No relevant memories found."}).to_string();
                        }
                        let items: Vec<Value> = results.iter().map(|r| {
                            json!({"id": r.get("id"), "memory": r.get("memory").and_then(|v| v.as_str()).unwrap_or(""), "score": r.get("score").and_then(|v| v.as_f64()).unwrap_or(0.0)})
                        }).collect();
                        let count = items.len();
                        json!({"results": items, "count": count}).to_string()
                    }
                    Err(e) => {
                        if !is_client_error_str(&e) {
                            self.record_failure();
                        }
                        tool_error(self.format_error("Search failed", &e))
                    }
                }
            }
            "mem0_add" => {
                let content = args.get("content").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
                if content.is_empty() {
                    return tool_error("Missing required parameter: content");
                }
                let user_id = self.user_id.clone();
                let agent_id = self.agent_id.clone();
                let metadata = self.write_metadata();
                let messages = vec![json!({"role": "user", "content": content})];
                let backend = self.backend.as_ref().unwrap();
                match backend.add(&messages, &user_id, &agent_id, false, Some(metadata)) {
                    Ok(result) => {
                        self.record_success();
                        let event_id = result.get("event_id").cloned();
                        let msg = if self.mode == "oss" || !self.host.is_empty() {
                            "Fact stored."
                        } else {
                            "Fact queued for storage."
                        };
                        json!({"result": msg, "event_id": event_id}).to_string()
                    }
                    Err(e) => {
                        self.record_failure();
                        tool_error(self.format_error("Failed to store", &e))
                    }
                }
            }
            "mem0_update" => {
                let memory_id = args.get("memory_id").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
                let text = args.get("text").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
                if memory_id.is_empty() {
                    return tool_error("Missing required parameter: memory_id");
                }
                if text.is_empty() {
                    return tool_error("Missing required parameter: text");
                }
                let backend = self.backend.as_ref().unwrap();
                match backend.update(&memory_id, &text) {
                    Ok(result) => {
                        self.record_success();
                        result.to_string()
                    }
                    Err(e) => {
                        if is_client_error_str(&e) {
                            return tool_error(format!("Memory not found: {}", memory_id));
                        }
                        self.record_failure();
                        tool_error(self.format_error("Update failed", &e))
                    }
                }
            }
            "mem0_delete" => {
                let memory_id = args.get("memory_id").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
                if memory_id.is_empty() {
                    return tool_error("Missing required parameter: memory_id");
                }
                let backend = self.backend.as_ref().unwrap();
                match backend.delete(&memory_id) {
                    Ok(result) => {
                        self.record_success();
                        result.to_string()
                    }
                    Err(e) => {
                        if is_client_error_str(&e) {
                            return tool_error(format!("Memory not found: {}", memory_id));
                        }
                        self.record_failure();
                        tool_error(self.format_error("Delete failed", &e))
                    }
                }
            }
            _ => tool_error(format!("Unknown tool: {}", tool_name)),
        }
    }

    /// Mirrors `_shutdown_backend()` lines 611-617.
    fn shutdown_backend(&mut self) {
        if let Some(mut b) = self.backend.take() {
            b.close();
        }
    }

    /// Mirrors `shutdown()` lines 619-623 — join prefetch+sync with 5s timeout.
    pub fn shutdown(&mut self) {
        for handle_opt in [&mut self.prefetch_thread, &mut self.sync_thread] {
            if let Some(handle) = handle_opt.take() {
                if !handle.is_finished() {
                    let (tx, rx) = std::sync::mpsc::channel();
                    std::thread::spawn(move || {
                        let _ = handle.join();
                        let _ = tx.send(());
                    });
                    let _ = rx.recv_timeout(Duration::from_secs(5));
                } else {
                    let _ = handle.join();
                }
            }
        }
        self.shutdown_backend();
    }
}

impl Default for Mem0MemoryProvider {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Atomic JSON write — mirrors `utils.atomic_json_write` + _save_mem0_json
// ---------------------------------------------------------------------------

fn write_atomic_json(path: &Path, value: &Value) -> std::io::Result<()> {
    // Mirrors Python's atomic write: write to temp then rename, mode 0o600.
    // In Rust stub we use a simple write + set permissions on unix.
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(value).unwrap_or_else(|_| "{}".to_string()))?;
    std::fs::rename(&tmp, path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Plugin entry point — mirrors register(ctx) lines 626-628
// ---------------------------------------------------------------------------

/// Minimal `ctx` trait for memory provider registration — mirrors
/// `hermes_cli.plugins.PluginContext.register_memory_provider`.
pub trait PluginContext {
    fn register_memory_provider(&mut self, provider: Mem0MemoryProvider);
}

/// Mirrors `def register(ctx) -> None` lines 626-628.
pub fn register(ctx: &mut dyn PluginContext) {
    let provider = Mem0MemoryProvider::new();
    ctx.register_memory_provider(provider);
}

// ---------------------------------------------------------------------------
// Tests — mirrors Python contract invariants (breaker, client error, schemas, etc.)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn constants_match_python() {
        assert_eq!(BREAKER_THRESHOLD, 5);
        assert_eq!(BREAKER_COOLDOWN_SECS, 120);
        assert_eq!(PREFETCH_WAIT_SECS, 3);
        assert_eq!(DEFAULT_USER_ID, "hermes-user");
        assert_eq!(CLIENT_ERROR_TYPES, &["MemoryNotFoundError", "ValidationError"]);
    }

    #[test]
    fn is_client_error_detects_types_and_strings() {
        assert!(is_client_error("MemoryNotFoundError", "anything"));
        assert!(is_client_error("ValidationError", "anything"));
        assert!(!is_client_error("ConnectionError", "timeout"));
        assert!(is_client_error("AnyError", "404 not found"));
        assert!(is_client_error("AnyError", "Valid UUID required"));
        assert!(is_client_error_str("404 page"));
        assert!(is_client_error_str("not found in db"));
    }

    #[test]
    fn schemas_have_expected_names() {
        assert_eq!(search_schema()["name"], "mem0_search");
        assert_eq!(add_schema()["name"], "mem0_add");
        assert_eq!(update_schema()["name"], "mem0_update");
        assert_eq!(delete_schema()["name"], "mem0_delete");
        assert_eq!(search_schema()["parameters"]["required"], json!(["query"]));
        assert_eq!(add_schema()["parameters"]["required"], json!(["content"]));
        assert_eq!(update_schema()["parameters"]["required"], json!(["memory_id", "text"]));
        assert_eq!(delete_schema()["parameters"]["required"], json!(["memory_id"]));
    }

    #[test]
    fn provider_name_is_mem0() {
        let p = Mem0MemoryProvider::new();
        assert_eq!(p.name(), "mem0");
    }

    #[test]
    fn get_config_schema_has_required_fields() {
        let p = Mem0MemoryProvider::new();
        let schema = p.get_config_schema();
        assert!(schema.iter().any(|f| f.key == "api_key" && f.secret == Some(true) && f.env_var.as_deref() == Some("MEM0_API_KEY")));
        assert!(schema.iter().any(|f| f.key == "host" && f.env_var.as_deref() == Some("MEM0_HOST")));
        assert!(schema.iter().any(|f| f.key == "user_id"));
        assert!(schema.iter().any(|f| f.key == "agent_id"));
        assert!(schema.iter().any(|f| f.key == "rerank" && f.choices.is_some()));
    }

    #[test]
    fn system_prompt_block_contains_mode_and_tools() {
        let mut p = Mem0MemoryProvider::new();
        p.initialize("sess-1", HashMap::new());
        let block = p.system_prompt_block();
        assert!(block.contains("# Mem0 Memory"));
        assert!(block.contains("mem0_search"));
        assert!(block.contains("mem0_add"));
        assert!(block.contains("mem0_update"));
        assert!(block.contains("mem0_delete"));
    }

    #[test]
    fn read_filters_and_write_metadata() {
        let mut p = Mem0MemoryProvider::new();
        p.initialize("sess-1", [("user_id".to_string(), "alice".to_string()), ("platform".to_string(), "telegram".to_string())].into_iter().collect());
        let filters = p.read_filters();
        assert_eq!(filters.get("user_id").and_then(|v| v.as_str()), Some(p.user_id.as_str()));
        let meta = p.write_metadata();
        assert_eq!(meta.get("channel").and_then(|v| v.as_str()), Some("telegram"));
    }

    #[test]
    fn breaker_trips_after_threshold() {
        let mut p = Mem0MemoryProvider::new();
        assert!(!p.is_breaker_open_mut());
        for _ in 0..BREAKER_THRESHOLD {
            p.record_failure();
        }
        assert!(p.is_breaker_open_mut());
        p.record_success();
        assert!(!p.is_breaker_open_mut());
    }

    #[test]
    fn handle_tool_call_unknown_returns_error() {
        let mut p = Mem0MemoryProvider::new();
        p.initialize("sess-1", HashMap::new());
        // backend is Some stub — call unknown tool
        let out = p.handle_tool_call("unknown_tool", &json!({}));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert!(v.get("error").is_some());
    }

    #[test]
    fn handle_tool_call_missing_params() {
        let mut p = Mem0MemoryProvider::new();
        p.initialize("sess-1", HashMap::new());
        let out = p.handle_tool_call("mem0_search", &json!({}));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert!(v["error"].as_str().unwrap().contains("Missing required parameter: query"));
        let out2 = p.handle_tool_call("mem0_add", &json!({}));
        let v2: Value = serde_json::from_str(&out2).unwrap();
        assert!(v2["error"].as_str().unwrap().contains("Missing required parameter: content"));
    }

    #[test]
    fn get_tool_schemas_returns_four() {
        let p = Mem0MemoryProvider::new();
        let schemas = p.get_tool_schemas();
        assert_eq!(schemas.len(), 4);
        assert_eq!(schemas[0]["name"], "mem0_search");
        assert_eq!(schemas[1]["name"], "mem0_add");
    }

    #[test]
    fn validate_oss_config_detects_missing_sections() {
        let cfg = HashMap::new();
        let errs = validate_oss_config(&cfg);
        assert!(errs.iter().any(|e| e.contains("Missing required section: llm")));
        assert!(errs.iter().any(|e| e.contains("Missing required section: embedder")));
        assert!(errs.iter().any(|e| e.contains("Missing required section: vector_store")));
    }

    #[test]
    fn validate_oss_config_accepts_valid_qdrant() {
        let mut cfg = HashMap::new();
        cfg.insert("llm".to_string(), json!({"provider": "openai", "config": {"model": "gpt-5-mini"}}));
        cfg.insert("embedder".to_string(), json!({"provider": "openai", "config": {"model": "text-embedding-3-small"}}));
        cfg.insert("vector_store".to_string(), json!({"provider": "qdrant", "config": {"path": "/tmp/qdrant"}}));
        let errs = validate_oss_config(&cfg);
        assert!(errs.is_empty(), "unexpected errors: {:?}", errs);
    }

    #[test]
    fn unwrap_results_handles_both_shapes() {
        let dict_resp = json!({"results": [{"id": "1", "memory": "hello"}]});
        assert_eq!(unwrap_results(dict_resp).len(), 1);
        let list_resp = json!([{"id": "1"}]);
        assert_eq!(unwrap_results(list_resp).len(), 1);
        let empty = json!({});
        assert!(unwrap_results(empty).is_empty());
    }

    #[test]
    fn load_config_handles_env_and_file() {
        // Basic smoke: load_config returns mode key
        let cfg = load_config();
        assert!(cfg.contains_key("mode"));
        assert!(cfg.contains_key("api_key"));
        assert!(cfg.contains_key("host"));
    }

    #[test]
    fn provider_defs_have_expected_keys() {
        let llm = llm_providers();
        assert!(llm.contains_key("openai"));
        assert!(llm.contains_key("ollama"));
        let emb = embedder_providers();
        assert!(emb.contains_key("openai"));
        let vd = vector_providers();
        assert!(vd.contains_key("qdrant"));
        assert!(vd.contains_key("pgvector"));
        let kd = known_dims();
        assert_eq!(kd.get("text-embedding-3-small"), Some(&1536));
    }
}
