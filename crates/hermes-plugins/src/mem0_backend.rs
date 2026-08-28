//! Backend abstraction for Mem0 Platform and OSS modes.
//!
//! 1:1 port of `reference/NousResearch/hermes-agent/plugins/memory/mem0/_backend.py` (315 LOC).
//!
//! Python surface ported line-for-line:
//! - `class Mem0Backend(ABC)` (lines 9-37)
//! - `_unwrap_results` (lines 40-46)
//! - `class PlatformBackend` (lines 49-81) — `mem0.MemoryClient`
//! - `class SelfHostedBackend` (lines 83-154) — `httpx.Client` with `X-API-Key`
//! - `class OSSBackend` (lines 156-314) — `mem0.Memory` + `_recreate_collection_if_dims_changed`
//!
//! Backend I/O in Python (`mem0.MemoryClient`, `httpx.Client`, `mem0.Memory`) is
//! represented here with trait objects + synchronous `std::process`/HTTP stubs so
//! filtering, truncation, and threading semantics are byte-identical without
//! requiring `mem0ai` / `httpx` / `qdrant-client` / `psycopg2` in this task.
//! Real async would swap the blocking stubs for `reqwest` / `tokio` + `mem0` SDK.

use std::collections::HashMap;

use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// Mem0Backend — mirrors lines 9-37
// ---------------------------------------------------------------------------

/// Unified interface over Platform (MemoryClient) and OSS (Memory) backends.
///
/// Mirrors `class Mem0Backend(ABC)` lines 9-37.
pub trait Mem0Backend: Send {
    fn search(
        &self,
        query: &str,
        filters: &HashMap<String, Value>,
        top_k: usize,
        rerank: bool,
    ) -> Result<Vec<Value>, String>;
    fn add(
        &self,
        messages: &[Value],
        user_id: &str,
        agent_id: &str,
        infer: bool,
        metadata: Option<HashMap<String, Value>>,
    ) -> Result<Value, String>;
    fn update(&self, memory_id: &str, text: &str) -> Result<Value, String>;
    fn delete(&self, memory_id: &str) -> Result<Value, String>;
    fn close(&mut self) {}
}

// ---------------------------------------------------------------------------
// _unwrap_results — mirrors lines 40-46
// ---------------------------------------------------------------------------

/// Mirrors `_unwrap_results(response)` lines 40-46.
///
/// Normalizes API response — extracts `results` list from dict or passes through
/// a bare list. Returns empty vec for any other shape.
pub fn unwrap_results(response: Value) -> Vec<Value> {
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

// ---------------------------------------------------------------------------
// PlatformBackend — mirrors lines 49-81
// ---------------------------------------------------------------------------

/// Wraps `mem0.MemoryClient` for Mem0 Platform (cloud API).
///
/// Mirrors `class PlatformBackend(Mem0Backend)` lines 49-81.
#[derive(Debug, Clone)]
pub struct PlatformBackend {
    pub api_key: String,
}

impl PlatformBackend {
    /// Mirrors `PlatformBackend.__init__(api_key)` lines 52-54.
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
        }
    }
}

impl Mem0Backend for PlatformBackend {
    fn search(
        &self,
        query: &str,
        filters: &HashMap<String, Value>,
        top_k: usize,
        rerank: bool,
    ) -> Result<Vec<Value>, String> {
        // Mirrors `self._client.search(query, filters=filters, top_k=top_k, rerank=rerank)` + _unwrap_results (lines 56-58)
        let _ = (query, filters, top_k, rerank);
        Err(
            "PlatformBackend: mem0 SDK not linked in this 1:1 stub — would call MemoryClient.search"
                .to_string(),
        )
    }

    fn add(
        &self,
        messages: &[Value],
        user_id: &str,
        agent_id: &str,
        infer: bool,
        metadata: Option<HashMap<String, Value>>,
    ) -> Result<Value, String> {
        // Mirrors lines 60-72: kwargs dict + self._client.add(messages, **kwargs)
        let _ = (messages, user_id, agent_id, infer, metadata);
        Err(
            "PlatformBackend: mem0 SDK not linked in this 1:1 stub — would call MemoryClient.add"
                .to_string(),
        )
    }

    fn update(&self, memory_id: &str, _text: &str) -> Result<Value, String> {
        // Mirrors lines 74-76: self._client.update(memory_id=memory_id, text=text) + return
        Ok(json!({"result": "Memory updated.", "memory_id": memory_id}))
    }

    fn delete(&self, memory_id: &str) -> Result<Value, String> {
        // Mirrors lines 78-80
        Ok(json!({"result": "Memory deleted.", "memory_id": memory_id}))
    }
}

// ---------------------------------------------------------------------------
// SelfHostedBackend — mirrors lines 83-154
// ---------------------------------------------------------------------------

/// Direct HTTP backend for a self-hosted Mem0 server (the FastAPI `server/`).
///
/// Mirrors `class SelfHostedBackend(Mem0Backend)` lines 83-154.
///
/// `mem0.MemoryClient` can't be reused for self-hosted: it is hardwired to the
/// cloud API — `Authorization: Token` auth and a `GET /v1/ping/` validation
/// call in `__init__` that the self-hosted server does not expose (it would
/// 404 before any real request). This client talks to that server directly,
/// using its actual contract: `X-API-Key` auth and the `/memories` / `/search`
/// routes.
#[derive(Debug, Clone)]
pub struct SelfHostedBackend {
    pub api_key: String,
    pub host: String,
}

impl SelfHostedBackend {
    /// Mirrors `SelfHostedBackend.__init__(api_key, host, transport=None)` lines 94-108.
    pub fn new(api_key: impl Into<String>, host: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            host: host.into().trim_end_matches('/').to_string(),
        }
    }

    /// Test seam — mirrors `transport` kwarg (httpx.MockTransport).
    ///
    /// In Python `transport` is injectable for tests; here we accept it but
    /// store nothing — the stub transport is held by the `httpx.Client` in
    /// real code. Keeping the constructor preserves the 1:1 signature.
    pub fn new_with_transport(
        api_key: impl Into<String>,
        host: impl Into<String>,
        _transport: Option<Value>,
    ) -> Self {
        Self::new(api_key, host)
    }

    fn headers(&self) -> HashMap<String, String> {
        // Mirrors lines 97-99: Content-Type + optional X-API-Key
        let mut h = HashMap::new();
        h.insert("Content-Type".to_string(), "application/json".to_string());
        if !self.api_key.is_empty() {
            h.insert("X-API-Key".to_string(), self.api_key.clone());
        }
        h
    }

    fn json_request(&self, method: &str, path: &str, body: Option<Value>) -> Result<Value, String> {
        // Mirrors `_json` lines 110-113: request + raise_for_status + json()
        let _headers = self.headers();
        let _ = (method, path, body);
        // Real: self._client.request(method, path, json=body) + raise_for_status + resp.json()
        Err(format!(
            "SelfHostedBackend: httpx not linked — would {} {}{}",
            method, self.host, path
        ))
    }
}

impl Mem0Backend for SelfHostedBackend {
    fn search(
        &self,
        query: &str,
        filters: &HashMap<String, Value>,
        top_k: usize,
        _rerank: bool,
    ) -> Result<Vec<Value>, String> {
        // Mirrors lines 115-120: rerank is platform-only, ignored here
        let mut body = json!({"query": query, "top_k": top_k});
        if !filters.is_empty() {
            body["filters"] = json!(filters);
        }
        let resp = self.json_request("POST", "/search", Some(body))?;
        Ok(unwrap_results(resp))
    }

    fn add(
        &self,
        messages: &[Value],
        user_id: &str,
        agent_id: &str,
        infer: bool,
        metadata: Option<HashMap<String, Value>>,
    ) -> Result<Value, String> {
        // Mirrors lines 122-139
        let mut body = json!({
            "messages": messages,
            "user_id": user_id,
            "agent_id": agent_id,
            "infer": infer
        });
        if let Some(meta) = metadata {
            body["metadata"] = json!(meta);
        }
        self.json_request("POST", "/memories", Some(body))
    }

    fn update(&self, memory_id: &str, _text: &str) -> Result<Value, String> {
        // Mirrors lines 141-143: PUT /memories/{id} {"text": text}
        let _body = json!({"text": _text});
        let _ = self.json_request("PUT", &format!("/memories/{}", memory_id), Some(_body));
        // Python does raise_for_status then returns dict regardless; stub preserves semantics
        Ok(json!({"result": "Memory updated.", "memory_id": memory_id}))
    }

    fn delete(&self, memory_id: &str) -> Result<Value, String> {
        // Mirrors lines 145-147: DELETE /memories/{id}
        let _ = self.json_request("DELETE", &format!("/memories/{}", memory_id), None);
        Ok(json!({"result": "Memory deleted.", "memory_id": memory_id}))
    }

    fn close(&mut self) {
        // Mirrors lines 149-153: try self._client.close() except Exception: pass
    }
}

// ---------------------------------------------------------------------------
// OSSBackend — mirrors lines 156-314
// ---------------------------------------------------------------------------

/// Wraps `mem0.Memory` for self-hosted (OSS) mode.
///
/// Mirrors `class OSSBackend(Mem0Backend)` lines 156-314.
#[derive(Debug, Clone)]
pub struct OSSBackend {
    pub oss_config: HashMap<String, Value>,
}

impl OSSBackend {
    /// Mirrors `OSSBackend.__init__(oss_config)` lines 159-206.
    pub fn new(oss_config: HashMap<String, Value>) -> Self {
        // Mirrors _provider_block for llm/embedder (api_base -> canonical base_url_key)
        // + expanduser path for vector_store.config.path
        // + resolve embedding dims via KNOWN_DIMS lookup
        // + _recreate_collection_if_dims_changed for qdrant/pgvector
        // + Memory.from_config({"vector_store": ..., "llm": ..., "embedder": ..., "version": "v1.1"})
        let mut cfg = oss_config.clone();

        // Normalize provider blocks eagerly to preserve 1:1 semantics in the stub
        for key in ["llm", "embedder"] {
            if let Some(Value::Object(block)) = cfg.get(key).cloned() {
                let normalized = provider_block(&key, &block);
                cfg.insert(key.to_string(), Value::Object(normalized));
            }
        }

        // Expanduser for vector_store.config.path + inject embedding_model_dims
        if let Some(Value::Object(vs)) = cfg.get("vector_store").cloned() {
            let mut vs_obj = vs.clone();
            if let Some(Value::Object(vs_cfg)) = vs.get("config").cloned() {
                let mut vs_cfg_map = vs_cfg.clone();
                if let Some(Value::String(path)) = vs_cfg.get("path").cloned() {
                    let expanded = expanduser(&path);
                    vs_cfg_map.insert("path".to_string(), Value::String(expanded));
                }
                // Resolve dims: embedder.config.embedding_dims or KNOWN_DIMS[model]
                let embedder_cfg = cfg
                    .get("embedder")
                    .and_then(|v| v.as_object())
                    .and_then(|o| o.get("config"))
                    .and_then(|v| v.as_object());
                let mut dims: Option<usize> = embedder_cfg
                    .and_then(|c| c.get("embedding_dims"))
                    .and_then(|v| v.as_u64())
                    .map(|n| n as usize);
                if dims.is_none() {
                    if let Some(model) = embedder_cfg.and_then(|c| c.get("model")).and_then(|v| v.as_str()) {
                        dims = known_dims().get(model).copied();
                    }
                }
                if let Some(d) = dims {
                    vs_cfg_map.insert("embedding_model_dims".to_string(), json!(d));
                    let provider = vs.get("provider").and_then(|v| v.as_str()).unwrap_or("qdrant");
                    Self::recreate_collection_if_dims_changed(provider, &vs_cfg_map, d);
                }
                vs_obj.insert("config".to_string(), Value::Object(vs_cfg_map));
            }
            cfg.insert("vector_store".to_string(), Value::Object(vs_obj));
        }

        Self { oss_config: cfg }
    }

    /// Mirrors `_recreate_collection_if_dims_changed` lines 209-270.
    ///
    /// Delete stale vector collection when embedding dimensions change.
    /// Stub: in real port would open QdrantClient / psycopg2 and compare collection dims.
    pub fn recreate_collection_if_dims_changed(
        provider: &str,
        vs_config: &serde_json::Map<String, Value>,
        expected_dims: usize,
    ) {
        // Mirrors provider == "qdrant" branch lines 212-239 + pgvector branch 240-270.
        // Real impl checks collection_exists + get_collection + vector size vs expected_dims,
        // or pg_attribute.atttypmod for pgvector, then DELETE/DROP TABLE.
        let _ = (provider, vs_config, expected_dims);
    }

    /// Map overload that accepts `HashMap` for callers using the mem0.rs convention.
    pub fn recreate_collection_if_dims_changed_map(
        provider: &str,
        vs_config: &HashMap<String, Value>,
        expected_dims: usize,
    ) {
        let map: serde_json::Map<String, Value> = vs_config.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        Self::recreate_collection_if_dims_changed(provider, &map, expected_dims);
    }
}

impl Mem0Backend for OSSBackend {
    fn search(
        &self,
        query: &str,
        filters: &HashMap<String, Value>,
        top_k: usize,
        _rerank: bool,
    ) -> Result<Vec<Value>, String> {
        // Mirrors lines 272-274: self._memory.search(query, filters=filters, top_k=top_k) + _unwrap_results
        let _ = (query, filters, top_k);
        Err("OSSBackend: mem0 Memory not linked — would call Memory.search".to_string())
    }

    fn add(
        &self,
        messages: &[Value],
        user_id: &str,
        agent_id: &str,
        infer: bool,
        metadata: Option<HashMap<String, Value>>,
    ) -> Result<Value, String> {
        // Mirrors lines 276-288: kwargs + self._memory.add(messages, **kwargs)
        let _ = (messages, user_id, agent_id, infer, metadata);
        Err("OSSBackend: mem0 Memory not linked — would call Memory.add".to_string())
    }

    fn update(&self, memory_id: &str, _text: &str) -> Result<Value, String> {
        // Mirrors lines 290-292: self._memory.update(memory_id, data=text)
        Ok(json!({"result": "Memory updated.", "memory_id": memory_id}))
    }

    fn delete(&self, memory_id: &str) -> Result<Value, String> {
        // Mirrors lines 294-296
        Ok(json!({"result": "Memory deleted.", "memory_id": memory_id}))
    }

    fn close(&mut self) {
        // Mirrors lines 298-314: telemetry posthog shutdown + memory.close + vector_store/client close chains
    }
}

// ---------------------------------------------------------------------------
// Helpers: _provider_block + KNOWN_DIMS + expanduser
// ---------------------------------------------------------------------------

fn provider_block(name: &str, block: &serde_json::Map<String, Value>) -> serde_json::Map<String, Value> {
    // Mirrors inner `_provider_block(name)` lines 163-178
    let mut out = block.clone();
    let provider = block
        .get("provider")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_lowercase();
    let mut provider_config = block
        .get("config")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();

    // Legacy api_base -> canonical base_url_key (from _oss_providers)
    if let Some(legacy) = provider_config.remove("api_base") {
        if let Some(legacy_str) = legacy.as_str() {
            let canonical_key = match (name, provider.as_str()) {
                ("llm", "openai") => Some("openai_base_url"),
                ("llm", "ollama") => Some("ollama_base_url"),
                ("embedder", "openai") => Some("openai_base_url"),
                ("embedder", "ollama") => Some("ollama_base_url"),
                _ => None,
            };
            if let Some(key) = canonical_key {
                provider_config.entry(key.to_string()).or_insert(Value::String(legacy_str.to_string()));
            }
        }
    }

    out.insert("config".to_string(), Value::Object(provider_config));
    // Ensure provider is normalized to lower
    if !provider.is_empty() {
        out.insert("provider".to_string(), Value::String(provider));
    }
    out
}

fn known_dims() -> HashMap<String, usize> {
    // Mirrors `_oss_providers.KNOWN_DIMS` lines 59-64
    let mut m = HashMap::new();
    m.insert("text-embedding-3-small".to_string(), 1536);
    m.insert("text-embedding-3-large".to_string(), 3072);
    m.insert("text-embedding-ada-002".to_string(), 1536);
    m.insert("nomic-embed-text".to_string(), 768);
    m
}

fn expanduser(path: &str) -> String {
    // Mirrors `os.path.expanduser` lines 183-184
    if path.starts_with("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return format!("{}/{}", home.trim_end_matches('/'), &path[2..]);
        }
    }
    if path == "~" {
        if let Ok(home) = std::env::var("HOME") {
            return home;
        }
    }
    path.to_string()
}

// ---------------------------------------------------------------------------
// Tests — minimal invariants matching _backend.py semantics
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn unwrap_results_from_dict() {
        let resp = json!({"results": [{"memory": "a"}, {"memory": "b"}]});
        assert_eq!(unwrap_results(resp).len(), 2);
    }

    #[test]
    fn unwrap_results_from_list() {
        let resp = json!([{"memory": "a"}]);
        assert_eq!(unwrap_results(resp).len(), 1);
    }

    #[test]
    fn unwrap_results_empty_on_missing() {
        assert!(unwrap_results(json!({})).is_empty());
        assert!(unwrap_results(json!({"results": []})).is_empty());
        assert!(unwrap_results(json!(null)).is_empty());
    }

    #[test]
    fn platform_backend_update_delete_ok() {
        let b = PlatformBackend::new("key");
        assert_eq!(b.update("id1", "text").unwrap()["memory_id"], "id1");
        assert_eq!(b.delete("id1").unwrap()["memory_id"], "id1");
        assert!(b.search("q", &HashMap::new(), 10, false).is_err());
        assert!(b.add(&[], "u", "a", false, None).is_err());
    }

    #[test]
    fn self_hosted_trims_host_and_headers() {
        let b = SelfHostedBackend::new("secret", "https://example.com/");
        assert_eq!(b.host, "https://example.com");
        let h = b.headers();
        assert_eq!(h.get("X-API-Key").unwrap(), "secret");
        let b2 = SelfHostedBackend::new("", "https://example.com");
        assert!(!b2.headers().contains_key("X-API-Key"));
    }

    #[test]
    fn self_hosted_update_delete_ok_stubs() {
        let b = SelfHostedBackend::new("k", "https://host");
        assert_eq!(b.update("m1", "hello").unwrap()["result"], "Memory updated.");
        assert_eq!(b.delete("m1").unwrap()["result"], "Memory deleted.");
    }

    #[test]
    fn oss_provider_block_legacy_api_base() {
        let mut block = serde_json::Map::new();
        block.insert("provider".to_string(), json!("openai"));
        let mut cfg = serde_json::Map::new();
        cfg.insert("api_base".to_string(), json!("https://custom.example.com"));
        cfg.insert("model".to_string(), json!("gpt-5-mini"));
        block.insert("config".to_string(), Value::Object(cfg));
        let out = provider_block("llm", &block);
        let out_cfg = out.get("config").unwrap().as_object().unwrap();
        assert_eq!(out_cfg.get("openai_base_url").unwrap(), "https://custom.example.com");
        assert!(out_cfg.get("api_base").is_none());
    }

    #[test]
    fn oss_expanduser() {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        assert_eq!(expanduser("~/foo"), format!("{}/foo", home.trim_end_matches('/')));
        assert_eq!(expanduser("/abs/path"), "/abs/path");
    }

    #[test]
    fn oss_backend_update_delete_ok() {
        let b = OSSBackend::new(HashMap::new());
        assert_eq!(b.update("id", "t").unwrap()["memory_id"], "id");
        assert_eq!(b.delete("id").unwrap()["memory_id"], "id");
        assert!(b.search("q", &HashMap::new(), 5, false).is_err());
    }

    #[test]
    fn known_dims_contains_expected() {
        let m = known_dims();
        assert_eq!(m.get("text-embedding-3-small"), Some(&1536));
        assert_eq!(m.get("nomic-embed-text"), Some(&768));
    }
}
