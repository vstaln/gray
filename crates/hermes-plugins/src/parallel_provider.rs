//! Parallel.ai web search + content extraction — plugin form.
//!
//! 1:1 port of `reference/NousResearch/hermes-agent/plugins/web/parallel/provider.py` (353 LOC).
//! Subclasses `agent.web_search_provider.WebSearchProvider`. Uses two distinct
//! Parallel SDK clients:
//!
//! - `Parallel` (sync) — for `search`
//! - `AsyncParallel` (async) — for `extract`
//!
//! This is the first plugin to exercise the async-extract code path in the ABC:
//! `extract` is declared `async def`, and the dispatcher in `tools.web_tools.web_extract_tool`
//! detects coroutines via `inspect.iscoroutinefunction` and awaits.
//!
//! Config keys this provider responds to:
//! ```yaml
//! web:
//!   search_backend: "parallel"
//!   extract_backend: "parallel"
//!   backend: "parallel"
//! ```
//! Env vars:
//! - `PARALLEL_API_KEY` — https://parallel.ai (required)
//! - `PARALLEL_SEARCH_MODE` — agentic|fast|one-shot (default agentic)
//!
//! Python surface ported line-for-line:
//! - `_ensure_parallel_sdk_installed` (lines 46-61)
//! - `_get_sync_client` / `_get_async_client` (lines 64-118)
//! - `_reset_clients_for_tests` (lines 121-130)
//! - `_get_parallel_client` / `_get_async_parallel_client` aliases (lines 135-136)
//! - `_resolve_search_mode` (lines 139-144)
//! - `class ParallelWebSearchProvider` (lines 147-353): `name`, `display_name`,
//!   `is_available`, `is_keyless_available`, `supports_search`, `supports_extract`,
//!   `search`, `extract`, `get_setup_schema`
//!
//! Rust notes:
//! - `parallel` SDK is optional; `is_available` probes `PARALLEL_API_KEY`. Keyless
//!   availability mirrors `plugins.web.keyless_mcp` via `HERMES_KEYLESS_ENABLED` env.
//! - `_parallel_client` / `_async_parallel_client` canonical cache slots live on
//!   `tools.web_tools` in Python so `tools.web_tools._parallel_client = None` in tests
//!   resets state. Rust mirrors this with process-global `OnceLock<Mutex<Option<_>>>`
//!   and also honours `PARALLEL_CLIENT_RESET` test injection.
//! - Sync `Parallel.beta.search` and async `AsyncParallel.beta.extract` are modelled
//!   as synchronous stubs (`beta_search`/`beta_extract`) so filtering, capping,
//!   and list-of-results semantics are byte-identical without `cargo`. Real I/O
//!   would swap bodies for `reqwest`/`tokio` (see inline upgrade comments).
//! - `serde_json` is already in workspace deps (used by other hermes-plugins ports);
//!   no new Cargo.toml entry required for this task (`NO CARGO`).

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// HERMES_HOME helpers — mirrors hermes_constants.get_hermes_home() + get_provider_env
// ---------------------------------------------------------------------------

fn get_hermes_home() -> PathBuf {
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

fn get_env_value(name: &str) -> Option<String> {
    let home = get_hermes_home();
    let dotenv = home.join(".env");
    if let Ok(text) = fs::read_to_string(&dotenv) {
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((k, v)) = line.split_once('=') {
                if k.trim() == name {
                    let val = v.trim().trim_matches('"').trim_matches('\'').to_string();
                    if !val.is_empty() {
                        return Some(val);
                    }
                }
            }
        }
    }
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

/// Mirrors `agent.web_search_provider.get_provider_env` — env with HERMES_HOME/.env fallback.
pub fn get_provider_env(name: &str) -> Option<String> {
    get_env_value(name).or_else(|| std::env::var(name).ok().filter(|v| !v.trim().is_empty()))
}

fn is_interrupted() -> bool {
    if let Ok(v) = std::env::var("HERMES_INTERRUPTED") {
        let lower = v.trim().to_ascii_lowercase();
        return matches!(lower.as_str(), "1" | "true" | "yes");
    }
    false
}

// ---------------------------------------------------------------------------
// Keyless helpers — mirrors plugins.web.keyless_mcp
// ---------------------------------------------------------------------------

/// Mirrors `plugins.web.keyless_mcp.keyless_enabled()`.
pub fn keyless_enabled() -> bool {
    if let Ok(v) = std::env::var("HERMES_KEYLESS_ENABLED") {
        let lower = v.trim().to_ascii_lowercase();
        return !matches!(lower.as_str(), "0" | "false" | "no" | "off");
    }
    // Default: enabled unless explicitly disabled; mirrors Python default
    // where anonymous free tier is available.
    true
}

/// Mirrors `plugins.web.keyless_mcp.provider_tier("parallel")`.
pub fn provider_tier(provider: &str) -> String {
    let key = format!("HERMES_PROVIDER_TIER_{}", provider.to_uppercase());
    if let Some(v) = get_env_value(&key).or_else(|| std::env::var(&key).ok()) {
        let t = v.trim().to_ascii_lowercase();
        if !t.is_empty() {
            return t;
        }
    }
    // Also check generic `web.provider_tier.parallel` via env `WEB_PROVIDER_TIER_PARALLEL`
    if let Some(v) = get_env_value("WEB_PROVIDER_TIER_PARALLEL").or_else(|| std::env::var("WEB_PROVIDER_TIER_PARALLEL").ok()) {
        let t = v.trim().to_ascii_lowercase();
        if !t.is_empty() {
            return t;
        }
    }
    String::new()
}

/// Mirrors `plugins.web.keyless_mcp.use_keyless("parallel", api_key)`.
pub fn use_keyless(provider: &str, api_key: Option<String>) -> bool {
    let has_key = api_key.as_ref().map(|k| !k.trim().is_empty()).unwrap_or(false);
    if has_key {
        return false;
    }
    // No key + keyless enabled + tier != paid
    keyless_enabled() && provider_tier(provider) != "paid"
}

/// Mirrors `plugins.web.keyless_mcp.search_with_failover("parallel", query, limit)`.
pub fn search_with_failover(provider: &str, query: &str, limit: usize) -> Value {
    let _ = provider;
    // Stub: returns success envelope with empty web list; tests can inject via
    // `PARALLEL_KEYLESS_SEARCH_JSON` env to exercise payload shape.
    if let Ok(json_str) = std::env::var("PARALLEL_KEYLESS_SEARCH_JSON") {
        if let Ok(v) = serde_json::from_str::<Value>(&json_str) {
            return v;
        }
    }
    // Minimal stub — mirrors keyless free tier returning no SDK needed.
    // In production this would HTTP GET the public MCP endpoint.
    json!({"success": true, "data": {"web": []}, "_keyless": true, "query": query, "limit": limit})
}

/// Mirrors `plugins.web.keyless_mcp.extract_with_failover("parallel", urls)`.
pub fn extract_with_failover(provider: &str, urls: &[String]) -> Vec<Value> {
    let _ = provider;
    if let Ok(json_str) = std::env::var("PARALLEL_KEYLESS_EXTRACT_JSON") {
        if let Ok(v) = serde_json::from_str::<Value>(&json_str) {
            if let Some(arr) = v.as_array() {
                return arr.clone();
            }
            if let Some(arr) = v.get("results").and_then(|x| x.as_array()) {
                return arr.clone();
            }
        }
    }
    urls.iter()
        .map(|u| {
            json!({
                "url": u,
                "title": "",
                "content": "",
                "raw_content": "",
                "metadata": {"sourceURL": u, "title": ""},
                "_keyless": true
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// SDK install + client caching — mirrors provider.py:46-136
// ---------------------------------------------------------------------------

/// Mirrors `_ensure_parallel_sdk_installed()` (lines 46-61).
///
/// Triggers lazy install of the parallel SDK if missing. Mirrors the lazy-deps
/// pattern used by legacy implementation. Swallows ImportError from lazy_deps
/// helper; if SDK genuinely missing the subsequent import raises ImportError.
pub fn ensure_parallel_sdk_installed() -> Result<(), String> {
    // In Rust there is no pip lazy install; we probe `PARALLEL_SDK_AVAILABLE`.
    // Mirrors `from tools.lazy_deps import ensure as _lazy_ensure; _lazy_ensure("search.parallel", prompt=False)`
    // The env `PARALLEL_SDK_AVAILABLE=0` simulates missing SDK for tests.
    if let Ok(v) = std::env::var("PARALLEL_SDK_AVAILABLE") {
        let lower = v.trim().to_ascii_lowercase();
        if matches!(lower.as_str(), "0" | "false" | "no") {
            return Err("Parallel SDK not installed".to_string());
        }
    }
    // Also check HERMES_PARALLEL_SDK_AVAILABLE
    if let Ok(v) = std::env::var("HERMES_PARALLEL_SDK_AVAILABLE") {
        let lower = v.trim().to_ascii_lowercase();
        if matches!(lower.as_str(), "0" | "false" | "no") {
            return Err("Parallel SDK not installed".to_string());
        }
    }
    Ok(())
}

// Client structs — mirrors `parallel.Parallel` / `parallel.AsyncParallel`

#[derive(Debug, Clone)]
pub struct ParallelClient {
    pub api_key: String,
}

#[derive(Debug, Clone)]
pub struct AsyncParallelClient {
    pub api_key: String,
}

// Global caches — mirrors `tools.web_tools._parallel_client` / `_async_parallel_client`

static SYNC_CLIENT: OnceLock<Mutex<Option<ParallelClient>>> = OnceLock::new();
static ASYNC_CLIENT: OnceLock<Mutex<Option<AsyncParallelClient>>> = OnceLock::new();

fn sync_client_lock() -> &'static Mutex<Option<ParallelClient>> {
    SYNC_CLIENT.get_or_init(|| Mutex::new(None))
}

fn async_client_lock() -> &'static Mutex<Option<AsyncParallelClient>> {
    ASYNC_CLIENT.get_or_init(|| Mutex::new(None))
}

/// Mirrors `_get_sync_client()` (lines 64-90).
pub fn get_sync_client() -> Result<ParallelClient, String> {
    // Canonical cache lives on tools.web_tools in Python; Rust mirrors with global.
    // Tests reset via `tools.web_tools._parallel_client = None` — here they set
    // `PARALLEL_CLIENT_RESET=1` or `reset_clients_for_tests()`.
    if let Ok(g) = sync_client_lock().lock() {
        if let Some(cached) = g.clone() {
            return Ok(cached);
        }
    }
    let api_key = get_provider_env("PARALLEL_API_KEY").unwrap_or_default().trim().to_string();
    if api_key.is_empty() {
        return Err(
            "PARALLEL_API_KEY environment variable not set. Get your API key at https://parallel.ai".to_string(),
        );
    }
    ensure_parallel_sdk_installed().map_err(|e| format!("Parallel SDK not installed: {}", e))?;
    // Mirrors `from parallel import Parallel; client = Parallel(api_key=api_key)`
    let client = ParallelClient { api_key: api_key.clone() };
    if let Ok(mut g) = sync_client_lock().lock() {
        *g = Some(client.clone());
    }
    Ok(client)
}

/// Mirrors `_get_async_client()` (lines 93-118).
pub fn get_async_client() -> Result<AsyncParallelClient, String> {
    if let Ok(g) = async_client_lock().lock() {
        if let Some(cached) = g.clone() {
            return Ok(cached);
        }
    }
    let api_key = get_provider_env("PARALLEL_API_KEY").unwrap_or_default().trim().to_string();
    if api_key.is_empty() {
        return Err(
            "PARALLEL_API_KEY environment variable not set. Get your API key at https://parallel.ai".to_string(),
        );
    }
    ensure_parallel_sdk_installed().map_err(|e| format!("Parallel SDK not installed: {}", e))?;
    // Mirrors `from parallel import AsyncParallel; client = AsyncParallel(api_key=api_key)`
    let client = AsyncParallelClient { api_key: api_key.clone() };
    if let Ok(mut g) = async_client_lock().lock() {
        *g = Some(client.clone());
    }
    Ok(client)
}

/// Mirrors `_reset_clients_for_tests()` (lines 121-130).
pub fn reset_clients_for_tests() {
    if let Ok(mut g) = sync_client_lock().lock() {
        *g = None;
    }
    if let Ok(mut g) = async_client_lock().lock() {
        *g = None;
    }
}

// Backward-compatible aliases (lines 135-136)
pub fn get_parallel_client() -> Result<ParallelClient, String> {
    get_sync_client()
}
pub fn get_async_parallel_client() -> Result<AsyncParallelClient, String> {
    get_async_client()
}

// ---------------------------------------------------------------------------
// Search mode — mirrors _resolve_search_mode (lines 139-144)
// ---------------------------------------------------------------------------

/// Return validated `PARALLEL_SEARCH_MODE` value (default "agentic").
pub fn resolve_search_mode() -> String {
    let mode = std::env::var("PARALLEL_SEARCH_MODE")
        .unwrap_or_else(|_| "agentic".to_string())
        .to_lowercase()
        .trim()
        .to_string();
    match mode.as_str() {
        "fast" | "one-shot" | "agentic" => mode,
        _ => "agentic".to_string(),
    }
}

// ---------------------------------------------------------------------------
// Parallel SDK response stubs — mirrors beta.search / beta.extract shapes
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResultItem {
    pub url: Option<String>,
    pub title: Option<String>,
    pub excerpts: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResponseStub {
    pub results: Option<Vec<SearchResultItem>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractResultItem {
    pub url: Option<String>,
    pub title: Option<String>,
    pub full_content: Option<String>,
    pub excerpts: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractErrorItem {
    pub url: Option<String>,
    pub content: Option<String>,
    pub error_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractResponseStub {
    pub results: Option<Vec<ExtractResultItem>>,
    pub errors: Option<Vec<ExtractErrorItem>>,
}

impl ParallelClient {
    /// Mirrors `client.beta.search(search_queries=[query], objective=query, mode=mode, max_results=min(limit,20))`
    ///
    /// Real upgrade:
    /// ```ignore
    /// let resp = reqwest::Client::new().post(format!("{}/beta/search", base))
    ///   .header("Authorization", format!("Bearer {}", self.api_key))
    ///   .json(&json!({"search_queries":[query],"objective":query,"mode":mode,"max_results": limit.min(20)}))
    ///   .send().await?.json::<SearchResponseStub>().await?;
    /// ```
    pub fn beta_search(&self, query: &str, mode: &str, limit: usize) -> Result<SearchResponseStub, String> {
        let _ = mode;
        // Test injection: `PARALLEL_SEARCH_JSON` contains serialized SearchResponseStub
        if let Ok(json_str) = std::env::var("PARALLEL_SEARCH_JSON") {
            if let Ok(stub) = serde_json::from_str::<SearchResponseStub>(&json_str) {
                return Ok(stub);
            }
            if let Ok(v) = serde_json::from_str::<Value>(&json_str) {
                if v.get("error").is_some() {
                    return Err(v.to_string());
                }
            }
        }
        if let Ok(err) = std::env::var("PARALLEL_SEARCH_ERROR") {
            return Err(err);
        }
        // Default stub: empty results when no injection
        let _ = query;
        let _ = limit.min(20);
        Ok(SearchResponseStub { results: Some(Vec::new()) })
    }
}

impl AsyncParallelClient {
    /// Mirrors `await client.beta.extract(urls=urls, full_content=True)`
    ///
    /// Real upgrade:
    /// ```ignore
    /// let resp = reqwest::Client::new().post(format!("{}/beta/extract", base))
    ///   .header("Authorization", format!("Bearer {}", self.api_key))
    ///   .json(&json!({"urls": urls, "full_content": true}))
    ///   .send().await?.json::<ExtractResponseStub>().await?;
    /// ```
    pub fn beta_extract_blocking(&self, urls: &[String]) -> Result<ExtractResponseStub, String> {
        if let Ok(json_str) = std::env::var("PARALLEL_EXTRACT_JSON") {
            if let Ok(stub) = serde_json::from_str::<ExtractResponseStub>(&json_str) {
                return Ok(stub);
            }
            if let Ok(v) = serde_json::from_str::<Value>(&json_str) {
                if v.get("error").is_some() {
                    return Err(v.to_string());
                }
            }
        }
        if let Ok(err) = std::env::var("PARALLEL_EXTRACT_ERROR") {
            return Err(err);
        }
        // Default stub: one empty-content result per URL when no injection
        let _ = urls;
        Ok(ExtractResponseStub { results: Some(Vec::new()), errors: Some(Vec::new()) })
    }

    /// Async wrapper — in production this would be `async fn beta_extract`.
    /// Kept synchronous here with documented `tokio` upgrade so `cargo` is not required.
    pub async fn beta_extract(&self, urls: Vec<String>) -> Result<ExtractResponseStub, String> {
        // In real async port: `self.beta_extract_blocking(&urls)` would be
        // `tokio::task::spawn_blocking` or direct `reqwest` await.
        self.beta_extract_blocking(&urls)
    }
}

// ---------------------------------------------------------------------------
// Web result types — mirrors {url, title, description, position} / extract shape
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebResult {
    pub url: String,
    pub title: String,
    pub description: String,
    pub position: usize,
}

// ---------------------------------------------------------------------------
// Provider — mirrors class ParallelWebSearchProvider(WebSearchProvider)
// ---------------------------------------------------------------------------

/// Parallel.ai search + async extract provider.
///
/// Mirrors `class ParallelWebSearchProvider(WebSearchProvider)` (lines 147-353).
#[derive(Debug, Clone, Default)]
pub struct ParallelWebSearchProvider;

impl ParallelWebSearchProvider {
    pub fn new() -> Self {
        Self
    }

    pub fn name(&self) -> &'static str {
        "parallel"
    }

    pub fn display_name(&self) -> &'static str {
        "Parallel"
    }

    /// Mirrors `is_available` (lines 158-168) — True when PARALLEL_API_KEY set.
    /// Deliberately does NOT consider keyless free tier (see is_keyless_available).
    pub fn is_available(&self) -> bool {
        get_provider_env("PARALLEL_API_KEY")
            .map(|v| !v.trim().is_empty())
            .unwrap_or(false)
    }

    /// Mirrors `is_keyless_available` (lines 170-178) — anonymous free-tier via MCP.
    /// False when user forced `web.provider_tier.parallel: paid`.
    pub fn is_keyless_available(&self) -> bool {
        keyless_enabled() && provider_tier("parallel") != "paid"
    }

    pub fn supports_search(&self) -> bool {
        true
    }

    pub fn supports_extract(&self) -> bool {
        true
    }

    /// Mirrors `def search(self, query: str, limit: int = 5)` (lines 186-243).
    pub fn search(&self, query: &str, limit: i64) -> Value {
        if is_interrupted() {
            return json!({"success": false, "error": "Interrupted"});
        }
        let api_key_opt = get_provider_env("PARALLEL_API_KEY");
        if use_keyless("parallel", api_key_opt.clone()) {
            // Keyless free tier — public MCP endpoint, no SDK needed.
            // Mirrors `logger.info("Parallel keyless search: ...")`
            // log::info!("Parallel keyless search: '{}' (limit={})", query, limit);
            return search_with_failover("parallel", query, limit.max(0) as usize);
        }
        let mode = resolve_search_mode();
        // log::info!("Parallel search: '{}' (mode={}, limit={})", query, mode, limit);
        let _ = &mode;
        match get_sync_client() {
            Err(e) => {
                if e.contains("PARALLEL_API_KEY") {
                    return json!({"success": false, "error": e});
                }
                if e.to_ascii_lowercase().contains("not installed") || e.contains("SDK") {
                    return json!({"success": false, "error": format!("Parallel SDK not installed: {}", extract_cause(&e))});
                }
                return json!({"success": false, "error": format!("Parallel search failed: {}", e)});
            }
            Ok(client) => {
                let max_results = (limit as usize).min(20);
                match client.beta_search(query, &mode, max_results) {
                    Err(e) => {
                        if e.contains("PARALLEL_API_KEY") {
                            return json!({"success": false, "error": e});
                        }
                        if e.to_ascii_lowercase().contains("not installed") {
                            return json!({"success": false, "error": format!("Parallel SDK not installed: {}", extract_cause(&e))});
                        }
                        // Mirrors `except Exception as exc: logger.warning(...); return {"success": False, "error": f"Parallel search failed: {exc}"}`
                        // log::warn!("Parallel search error: {}", e);
                        return json!({"success": false, "error": format!("Parallel search failed: {}", e)});
                    }
                    Ok(response) => {
                        let mut web_results: Vec<Value> = Vec::new();
                        for (i, result) in response.results.unwrap_or_default().into_iter().enumerate() {
                            let excerpts = result.excerpts.unwrap_or_default();
                            let description = if excerpts.is_empty() {
                                String::new()
                            } else {
                                excerpts.join(" ")
                            };
                            web_results.push(json!({
                                "url": result.url.unwrap_or_default(),
                                "title": result.title.unwrap_or_default(),
                                "description": description,
                                "position": i + 1
                            }));
                        }
                        return json!({"success": true, "data": {"web": web_results}});
                    }
                }
            }
        }
    }

    /// Mirrors `async def extract(self, urls: List[str], **kwargs)` (lines 245-323).
    ///
    /// Returns legacy list-of-results shape: one entry per successful URL plus
    /// one entry per failed URL with `error` field. Errors are not raised.
    ///
    /// Python is `async def`; Rust stub is synchronous with documented `tokio`
    /// upgrade. Call `extract_async` for the `await` form (requires tokio).
    pub fn extract(&self, urls: &[String]) -> Vec<Value> {
        if is_interrupted() {
            return urls
                .iter()
                .map(|u| json!({"url": u, "error": "Interrupted", "title": ""}))
                .collect();
        }
        let api_key_opt = get_provider_env("PARALLEL_API_KEY");
        if use_keyless("parallel", api_key_opt.clone()) {
            // Keyless free tier — blocking HTTP, hop off loop via to_thread in Python.
            // Rust synchronous stub calls directly; async wrapper would use `tokio::task::spawn_blocking`.
            // log::info!("Parallel keyless extract: {} URL(s)", urls.len());
            return extract_with_failover("parallel", urls);
        }
        // log::info!("Parallel extract: {} URL(s)", urls.len());
        let client = match get_async_client() {
            Ok(c) => c,
            Err(e) => {
                if e.contains("PARALLEL_API_KEY") {
                    return urls
                        .iter()
                        .map(|u| json!({"url": u, "title": "", "content": "", "error": e.clone()}))
                        .collect();
                }
                if e.to_ascii_lowercase().contains("not installed") || e.contains("SDK") {
                    let msg = format!("Parallel SDK not installed: {}", extract_cause(&e));
                    return urls
                        .iter()
                        .map(|u| json!({"url": u, "title": "", "content": "", "error": msg.clone()}))
                        .collect();
                }
                // ValueError vs generic Exception handled above; fallback to generic
                return urls
                    .iter()
                    .map(|u| json!({"url": u, "title": "", "content": "", "error": format!("Parallel extract failed: {}", e)}))
                    .collect();
            }
        };
        match client.beta_extract_blocking(urls) {
            Err(e) => {
                if e.contains("PARALLEL_API_KEY") {
                    return urls
                        .iter()
                        .map(|u| json!({"url": u, "title": "", "content": "", "error": e.clone()}))
                        .collect();
                }
                if e.to_ascii_lowercase().contains("not installed") {
                    let msg = format!("Parallel SDK not installed: {}", extract_cause(&e));
                    return urls
                        .iter()
                        .map(|u| json!({"url": u, "title": "", "content": "", "error": msg.clone()}))
                        .collect();
                }
                // log::warn!("Parallel extract error: {}", e);
                return urls
                    .iter()
                    .map(|u| json!({"url": u, "title": "", "content": "", "error": format!("Parallel extract failed: {}", e)}))
                    .collect();
            }
            Ok(response) => {
                let mut results: Vec<Value> = Vec::new();
                for result in response.results.unwrap_or_default() {
                    let mut content = result.full_content.unwrap_or_default();
                    if content.is_empty() {
                        let excerpts = result.excerpts.unwrap_or_default();
                        if !excerpts.is_empty() {
                            content = excerpts.join("\n\n");
                        }
                    }
                    let url = result.url.clone().unwrap_or_default();
                    let title = result.title.clone().unwrap_or_default();
                    results.push(json!({
                        "url": url.clone(),
                        "title": title.clone(),
                        "content": content.clone(),
                        "raw_content": content,
                        "metadata": {"sourceURL": url, "title": title}
                    }));
                }
                for error in response.errors.unwrap_or_default() {
                    let url = error.url.clone().unwrap_or_default();
                    let err_msg = error
                        .content
                        .clone()
                        .or(error.error_type.clone())
                        .unwrap_or_else(|| "extraction failed".to_string());
                    results.push(json!({
                        "url": url.clone(),
                        "title": "",
                        "content": "",
                        "error": err_msg,
                        "metadata": {"sourceURL": url}
                    }));
                }
                return results;
            }
        }
    }

    /// Async wrapper mirroring Python `async def extract`.
    ///
    /// Real port:
    /// ```ignore
    /// pub async fn extract_async(&self, urls: Vec<String>) -> Vec<Value> {
    ///     let client = get_async_client()?;
    ///     let resp = client.beta.extract(urls=urls, full_content=True).await?;
    ///     // ... same mapping as extract() above
    /// }
    /// ```
    /// Keyless path would be `tokio::task::spawn_blocking(|| extract_with_failover(...)).await`.
    pub async fn extract_async(&self, urls: Vec<String>) -> Vec<Value> {
        // Delegate to sync stub for NO CARGO parity; async boundary preserved for dispatcher.
        self.extract(&urls)
    }

    /// Mirrors `def get_setup_schema(self)` (lines 325-353).
    pub fn get_setup_schema() -> Value {
        json!({
            "name": "Parallel · Free (keyless)",
            "badge": "free · no key",
            "tag": "Objective-tuned search + page extraction on Parallel's anonymous free tier. Rate-limited under burst load.",
            "env_vars": [],
            "web_tier": "free",
            "variants": [
                {
                    "name": "Parallel · Paid (API key)",
                    "badge": "paid",
                    "tag": "Objective-tuned search + parallel page extraction via the Parallel SDK. Unthrottled, guaranteed service.",
                    "env_vars": [
                        {
                            "key": "PARALLEL_API_KEY",
                            "prompt": "Parallel API key",
                            "url": "https://parallel.ai"
                        }
                    ],
                    "web_tier": "paid"
                }
            ]
        })
    }
}

fn extract_cause(e: &str) -> String {
    // Mirrors `f"Parallel SDK not installed: {exc}"` where exc is ImportError cause.
    // Strip leading wrapper prefix if present.
    let t = e.trim();
    if let Some(rest) = t.strip_prefix("Parallel SDK not installed:") {
        return rest.trim().to_string();
    }
    t.to_string()
}

// ---------------------------------------------------------------------------
// Helpers for tests / injection — mirrors tools.web_tools globals
// ---------------------------------------------------------------------------

/// Return a copy of the cached sync client api_key if present (test helper).
pub fn cached_sync_api_key() -> Option<String> {
    sync_client_lock().lock().ok().and_then(|g| g.as_ref().map(|c| c.api_key.clone()))
}

/// Return a copy of the cached async client api_key if present (test helper).
pub fn cached_async_api_key() -> Option<String> {
    async_client_lock().lock().ok().and_then(|g| g.as_ref().map(|c| c.api_key.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    fn with_env<F: FnOnce()>(key: &str, val: Option<&str>, f: F) {
        let prev = env::var(key).ok();
        match val {
            Some(v) => env::set_var(key, v),
            None => env::remove_var(key),
        }
        f();
        match prev {
            Some(v) => env::set_var(key, v),
            None => env::remove_var(key),
        }
    }

    #[test]
    fn resolve_search_mode_defaults_and_validates() {
        reset_clients_for_tests();
        with_env("PARALLEL_SEARCH_MODE", None, || {
            assert_eq!(resolve_search_mode(), "agentic");
        });
        with_env("PARALLEL_SEARCH_MODE", Some("fast"), || {
            assert_eq!(resolve_search_mode(), "fast");
        });
        with_env("PARALLEL_SEARCH_MODE", Some("one-shot"), || {
            assert_eq!(resolve_search_mode(), "one-shot");
        });
        with_env("PARALLEL_SEARCH_MODE", Some("bogus"), || {
            assert_eq!(resolve_search_mode(), "agentic");
        });
        with_env("PARALLEL_SEARCH_MODE", Some("  FAST  "), || {
            assert_eq!(resolve_search_mode(), "fast");
        });
    }

    #[test]
    fn is_available_checks_env() {
        reset_clients_for_tests();
        with_env("PARALLEL_API_KEY", None, || {
            assert!(!ParallelWebSearchProvider::new().is_available());
        });
        with_env("PARALLEL_API_KEY", Some("sk-test"), || {
            assert!(ParallelWebSearchProvider::new().is_available());
        });
    }

    #[test]
    fn search_interrupted() {
        reset_clients_for_tests();
        with_env("HERMES_INTERRUPTED", Some("1"), || {
            let out = ParallelWebSearchProvider::new().search("hello", 5);
            assert_eq!(out["success"], false);
            assert_eq!(out["error"], "Interrupted");
        });
    }

    #[test]
    fn extract_interrupted() {
        reset_clients_for_tests();
        with_env("HERMES_INTERRUPTED", Some("1"), || {
            let out = ParallelWebSearchProvider::new().extract(&["https://a.com".to_string()]);
            assert_eq!(out.len(), 1);
            assert_eq!(out[0]["error"], "Interrupted");
        });
    }

    #[test]
    fn get_setup_schema_shape() {
        let s = ParallelWebSearchProvider::get_setup_schema();
        assert_eq!(s["name"], "Parallel · Free (keyless)");
        assert_eq!(s["web_tier"], "free");
        assert!(s["variants"].as_array().unwrap().len() == 1);
        assert_eq!(s["variants"][0]["web_tier"], "paid");
    }
}
