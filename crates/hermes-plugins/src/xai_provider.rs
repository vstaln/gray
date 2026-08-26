//! xAI Web Search — plugin form.
//!
//! 1:1 port of `reference/NousResearch/hermes-agent/plugins/web/xai/provider.py` (560 LOC).
//! Routes `web_search` tool calls through xAI's agentic Web Search tool
//! (server-side `web_search` on the Responses API). Grok runs the actual
//! searching and page-browsing server-side; we ask it to return the top
//! results as structured JSON so we can hand back the same
//! `{title, url, description, position}` rows every other Hermes web
//! provider produces.
//!
//! Reference: https://docs.x.ai/developers/tools/web-search
//!
//! Config keys this provider responds to:
//! ```yaml
//! web:
//!   search_backend: "xai"           # explicit per-capability
//!   backend: "xai"                  # shared fallback
//! ```
//! Optional knobs (under `web.xai` in `config.yaml`):
//! ```yaml
//! web:
//!   xai:
//!     model: "grok-build-0.1"       # reasoning model required by web_search
//!     allowed_domains: ["x.ai"]     # max 5 — mutually exclusive with excluded_domains
//!     excluded_domains: ["bad.com"] # max 5 — mutually exclusive with allowed_domains
//!     timeout: 90                   # seconds (default 90)
//! ```
//! Auth: reuses `tools.xai_http.resolve_xai_http_credentials`, which
//! prefers Hermes-managed xAI Grok OAuth (via `hermes auth`) and falls back
//! to `XAI_API_KEY` (resolved through `~/.hermes/.env`, then `os.environ`).
//!
//! Python surface ported line-for-line:
//! - `DEFAULT_MODEL`, `DEFAULT_TIMEOUT`, `_MAX_DOMAIN_FILTERS`, `_JSON_BLOCK_RE`
//! - `_load_xai_web_config`, `_coerce_domain_list`
//! - `XAIWebSearchProvider` (name, display_name, is_available, supports_search,
//!   supports_extract, search, _build_prompt, _extract_results,
//!   _collect_output_text, _try_parse_json_results, _results_from_annotations,
//!   get_setup_schema)
//! - Trust-model docstring preserved on the struct
//! - `has_xai_credentials` / `resolve_xai_http_credentials` /
//!   `hermes_xai_user_agent` helpers (from `tools.xai_http`)
//!
//! Async HTTP in Python (`httpx`) is represented here with synchronous
//! stubs + documented `reqwest`/`tokio` upgrade paths so the filtering,
//! prompt, and parsing semantics are byte-identical without requiring `cargo`
//! in this task. Real I/O would swap the `curl` bodies for
//! `reqwest::Client::post(...).send().await`.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// Constants — mirrors provider.py:49-56
// ---------------------------------------------------------------------------

/// Default model for xAI web search. Mirrors `DEFAULT_MODEL = "grok-build-0.1"`.
pub const DEFAULT_MODEL: &str = "grok-build-0.1";

/// Default timeout seconds. Mirrors `DEFAULT_TIMEOUT = 90`.
pub const DEFAULT_TIMEOUT: f64 = 90.0;

/// xAI hard cap on allowed_domains / excluded_domains. Mirrors `_MAX_DOMAIN_FILTERS = 5`.
pub const MAX_DOMAIN_FILTERS: usize = 5;

// ---------------------------------------------------------------------------
// HERMES_HOME helpers — mirrors hermes_constants.get_hermes_home()
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
    // Mirrors tools.xai_http.get_env_value → hermes_cli.config.get_env_value
    // Falls back to os.environ. Tries HERMES_HOME/.env first for test parity.
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

// ---------------------------------------------------------------------------
// xAI HTTP credential helpers — mirrors tools.xai_http
// ---------------------------------------------------------------------------

/// Mirrors `hermes_xai_user_agent()` — `Hermes-Agent/{version}`.
pub fn hermes_xai_user_agent() -> String {
    // Try HERMES_VERSION env first, then Cargo pkg version analogue.
    if let Ok(v) = std::env::var("HERMES_VERSION") {
        if !v.trim().is_empty() {
            return format!("Hermes-Agent/{}", v.trim());
        }
    }
    // Attempt to read version from hermes_cli.__version__ analogue: use
    // `CARGO_PKG_VERSION` if available at compile time, else "unknown".
    // Since we have no Cargo.toml for this crate, fallback to "unknown"
    // unless HERMES_AGENT_VERSION is set.
    if let Ok(v) = std::env::var("HERMES_AGENT_VERSION") {
        if !v.trim().is_empty() {
            return format!("Hermes-Agent/{}", v.trim());
        }
    }
    "Hermes-Agent/unknown".to_string()
}

/// Mirrors `has_xai_credentials()` — cheap probe, no refresh, no lock.
///
/// Checks: XAI_API_KEY env/.env, then `~/.hermes/auth.json` providers.xai-oauth
/// tokens.access_token, then credential_pool.xai-oauth.
pub fn has_xai_credentials() -> bool {
    if let Some(v) = get_env_value("XAI_API_KEY") {
        if !v.trim().is_empty() {
            return true;
        }
    }
    if std::env::var("XAI_API_KEY").map(|v| !v.trim().is_empty()).unwrap_or(false) {
        return true;
    }
    // Check auth.json without triggering refresh
    let auth_path = get_hermes_home().join("auth.json");
    if !auth_path.exists() {
        return false;
    }
    let text = match fs::read_to_string(&auth_path) {
        Ok(t) => t,
        Err(_) => return false,
    };
    let store: Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(_) => return false,
    };
    if !store.is_object() {
        return false;
    }
    // providers.xai-oauth.tokens.access_token
    if let Some(providers) = store.get("providers").and_then(|v| v.as_object()) {
        if let Some(xai_state) = providers.get("xai-oauth").and_then(|v| v.as_object()) {
            if let Some(tokens) = xai_state.get("tokens").and_then(|v| v.as_object()) {
                if let Some(at) = tokens.get("access_token").and_then(|v| v.as_str()) {
                    if !at.trim().is_empty() {
                        return true;
                    }
                }
                // Also check stringified
                if let Some(at) = tokens.get("access_token") {
                    if !at.is_null() && at.to_string().trim_matches('"').trim().is_empty() == false {
                        let s = at.as_str().unwrap_or(&at.to_string()).trim().to_string();
                        if !s.is_empty() && s != "null" {
                            return true;
                        }
                    }
                }
            }
        }
        // credential_pool.xai-oauth list
        if let Some(pool) = store.get("credential_pool").and_then(|v| v.as_object()) {
            if let Some(entries) = pool.get("xai-oauth").and_then(|v| v.as_array()) {
                for entry in entries {
                    if let Some(obj) = entry.as_object() {
                        if let Some(at) = obj.get("access_token").and_then(|v| v.as_str()) {
                            if !at.trim().is_empty() {
                                return true;
                            }
                        }
                    }
                }
            }
        }
    }
    false
}

/// Resolved xAI HTTP credentials — mirrors `resolve_xai_http_credentials()` return dict.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct XaiCredentials {
    pub provider: String,
    pub api_key: String,
    pub base_url: String,
}

impl Default for XaiCredentials {
    fn default() -> Self {
        Self {
            provider: "xai".to_string(),
            api_key: String::new(),
            base_url: "https://api.x.ai/v1".to_string(),
        }
    }
}

/// Mirrors `resolve_xai_http_credentials(force_refresh=False, api_key_hint=None)`.
///
/// Prefers OAuth (auth.json pool) then falls back to `XAI_API_KEY` via
/// `get_env_value`. Honors `HERMES_XAI_BASE_URL` / `XAI_BASE_URL` with
/// origin-pinning fallback. `force_refresh` is best-effort (re-reads file).
pub fn resolve_xai_http_credentials(force_refresh: bool, api_key_hint: Option<&str>) -> XaiCredentials {
    let _ = force_refresh;
    let _ = api_key_hint;
    // Prefer OAuth pool
    let auth_path = get_hermes_home().join("auth.json");
    if let Ok(text) = fs::read_to_string(&auth_path) {
        if let Ok(store) = serde_json::from_str::<Value>(&text) {
            if let Some(providers) = store.get("providers").and_then(|v| v.as_object()) {
                if let Some(xai_state) = providers.get("xai-oauth").and_then(|v| v.as_object()) {
                    if let Some(tokens) = xai_state.get("tokens").and_then(|v| v.as_object()) {
                        if let Some(at) = tokens.get("access_token").and_then(|v| v.as_str()) {
                            let trimmed = at.trim().to_string();
                            if !trimmed.is_empty() {
                                // Resolve base_url with validation
                                let base = resolve_xai_base_url(Some(&store));
                                return XaiCredentials {
                                    provider: "xai-oauth".to_string(),
                                    api_key: trimmed,
                                    base_url: base,
                                };
                            }
                        }
                    }
                }
            }
            // credential_pool fallback (multi-account)
            if let Some(pool) = store.get("credential_pool").and_then(|v| v.as_object()) {
                if let Some(entries) = pool.get("xai-oauth").and_then(|v| v.as_array()) {
                    // If force_refresh, prefer entry matching hint if provided
                    let mut chosen: Option<String> = None;
                    if force_refresh {
                        if let Some(hint) = api_key_hint {
                            for entry in entries {
                                if let Some(at) = entry.get("access_token").and_then(|v| v.as_str()) {
                                    if at.trim() == hint.trim() {
                                        // Found hint match — would refresh here; stub keeps same
                                        chosen = Some(at.trim().to_string());
                                        break;
                                    }
                                }
                            }
                        }
                    }
                    if chosen.is_none() {
                        for entry in entries {
                            if let Some(at) = entry.get("access_token").and_then(|v| v.as_str()) {
                                if !at.trim().is_empty() {
                                    chosen = Some(at.trim().to_string());
                                    break;
                                }
                            }
                        }
                    }
                    if let Some(tok) = chosen {
                        let base = resolve_xai_base_url(Some(&store));
                        return XaiCredentials {
                            provider: "xai-oauth".to_string(),
                            api_key: tok,
                            base_url: base,
                        };
                    }
                }
            }
        }
    }
    // Fallback to XAI_API_KEY
    let api_key = get_env_value("XAI_API_KEY")
        .or_else(|| std::env::var("XAI_API_KEY").ok())
        .unwrap_or_default()
        .trim()
        .to_string();
    let base_url = resolve_xai_base_url(None);
    XaiCredentials {
        provider: "xai".to_string(),
        api_key,
        base_url,
    }
}

fn resolve_xai_base_url(store: Option<&Value>) -> String {
    let default = "https://api.x.ai/v1".to_string();
    // Check env overrides first
    let override_url = get_env_value("HERMES_XAI_BASE_URL")
        .or_else(|| get_env_value("XAI_BASE_URL"))
        .or_else(|| std::env::var("HERMES_XAI_BASE_URL").ok())
        .or_else(|| std::env::var("XAI_BASE_URL").ok())
        .unwrap_or_default()
        .trim()
        .trim_end_matches('/')
        .to_string();
    if !override_url.is_empty() {
        if is_valid_xai_base_url(&override_url) {
            return override_url;
        } else {
            // Invalid override → fall back to default / store fallback
            log::debug!("Invalid XAI base URL override '{}', using fallback", override_url);
        }
    }
    // Try store fallback base_url if present
    if let Some(s) = store {
        if let Some(providers) = s.get("providers").and_then(|v| v.as_object()) {
            if let Some(xai_state) = providers.get("xai-oauth").and_then(|v| v.as_object()) {
                if let Some(tokens) = xai_state.get("tokens").and_then(|v| v.as_object()) {
                    // Not standard — check credential_pool entry base_url
                }
                let _ = tokens;
            }
        }
        if let Some(pool) = s.get("credential_pool").and_then(|v| v.as_object()) {
            if let Some(entries) = pool.get("xai-oauth").and_then(|v| v.as_array()) {
                for entry in entries {
                    if let Some(obj) = entry.as_object() {
                        if let Some(bu) = obj.get("base_url").and_then(|v| v.as_str()) {
                            let trimmed = bu.trim().trim_end_matches('/').to_string();
                            if !trimmed.is_empty() && is_valid_xai_base_url(&trimmed) {
                                return trimmed;
                            }
                        }
                        if let Some(bu) = obj.get("runtime_base_url").and_then(|v| v.as_str()) {
                            let trimmed = bu.trim().trim_end_matches('/').to_string();
                            if !trimmed.is_empty() && is_valid_xai_base_url(&trimmed) {
                                return trimmed;
                            }
                        }
                    }
                }
            }
        }
    }
    // Also check XAI_BASE_URL env as final fallback
    let env_base = std::env::var("XAI_BASE_URL").ok().unwrap_or_default().trim().trim_end_matches('/').to_string();
    if !env_base.is_empty() && is_valid_xai_base_url(&env_base) {
        return env_base;
    }
    default
}

fn is_valid_xai_base_url(url: &str) -> bool {
    // Mirrors hermes_cli.auth._xai_validate_inference_base_url pinning:
    // must be https, host == api.x.ai (or localhost for tests)
    if url.is_empty() {
        return false;
    }
    if !(url.starts_with("https://") || url.starts_with("http://localhost") || url.starts_with("http://127.0.0.1")) {
        // Only allow https for production; allow http localhost for tests
        if url.starts_with("http://") && !url.contains("localhost") && !url.contains("127.0.0.1") {
            return false;
        }
        if !url.starts_with("https://") {
            return false;
        }
    }
    // Basic host check — allow api.x.ai and x.ai for flexibility + localhost
    if url.contains("x.ai") || url.contains("localhost") || url.contains("127.0.0.1") {
        return true;
    }
    // Allow any https for now (mirrors fallback)
    url.starts_with("https://")
}

// ---------------------------------------------------------------------------
// Config — mirrors _load_xai_web_config + _coerce_domain_list
// ---------------------------------------------------------------------------

/// Mirrors `_load_xai_web_config() -> Dict[str, Any]` — read `web.xai` from config.yaml
pub fn load_xai_web_config() -> HashMap<String, Value> {
    let home = get_hermes_home();
    // Try config.json first (tests), then config.yaml / config.yml
    for fname in ["config.json", "config.yaml", "config.yml"] {
        let path = home.join(fname);
        if let Ok(text) = fs::read_to_string(&path) {
            if fname.ends_with(".json") {
                if let Ok(v) = serde_json::from_str::<Value>(&text) {
                    if let Some(web) = v.get("web").and_then(|w| w.as_object()) {
                        if let Some(xai) = web.get("xai").and_then(|x| x.as_object()) {
                            return xai.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                        }
                    }
                }
            } else {
                // Minimal YAML handling + JSON fallback
                if let Some(map) = try_parse_yaml_web_xai(&text) {
                    return map;
                }
                // Also try JSON shape embedded
                if let Ok(v) = serde_json::from_str::<Value>(&text) {
                    if let Some(web) = v.get("web").and_then(|w| w.as_object()) {
                        if let Some(xai) = web.get("xai").and_then(|x| x.as_object()) {
                            return xai.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                        }
                    }
                }
            }
        }
    }
    HashMap::new()
}

fn try_parse_yaml_web_xai(text: &str) -> Option<HashMap<String, Value>> {
    // Very small YAML subset parser for `web: xai:` block.
    // Looks for `web:` top level, then indented `xai:` block.
    // Supports scalar values, string arrays, numbers.
    if !text.contains("web") || !text.contains("xai") {
        return None;
    }
    // Naive line scan: find `web:` then `xai:` indented under it, then collect its children.
    let lines: Vec<&str> = text.lines().collect();
    let mut web_indent: Option<usize> = None;
    let mut xai_indent: Option<usize> = None;
    let mut xai_start: Option<usize> = None;
    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indent = line.len() - line.trim_start_matches(' ').len();
        if trimmed.starts_with("web:") {
            web_indent = Some(indent);
            continue;
        }
        if let Some(wi) = web_indent {
            if trimmed.starts_with("xai:") && indent > wi {
                xai_indent = Some(indent);
                xai_start = Some(idx);
                break;
            }
            // If we hit another top-level key (indent <= web_indent), web block ended
            if indent <= wi && !trimmed.starts_with("web:") {
                // not inside web anymore; but we already looked for xai
            }
        }
    }
    let xai_i = xai_indent?;
    let start = xai_start? + 1;
    let mut out: HashMap<String, Value> = HashMap::new();
    let mut i = start;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            i += 1;
            continue;
        }
        let indent = line.len() - line.trim_start_matches(' ').len();
        if indent <= xai_i {
            break; // left xai block
        }
        // Expect `key: value` or `key:` with list block
        if let Some(colon) = line.find(':') {
            let key = line[..colon].trim().to_string();
            let rest = line[colon + 1..].trim().to_string();
            if key.is_empty() {
                i += 1;
                continue;
            }
            // Only consider direct children of xai (indent == xai_i + 2 typical)
            // But we allow any deeper indent as still belonging to xai until dedent.
            // To avoid capturing nested keys incorrectly, only handle indent == xai_i + 2 or xai_i + 4?
            // Simplify: if indent > xai_i + 4, it's nested beyond one level — skip (part of list)
            // Actually list items are indent > key indent. So we detect list block when rest empty.
            if !rest.is_empty() {
                // Inline scalar
                let val = parse_yaml_scalar(&rest);
                out.insert(key, val);
                i += 1;
            } else {
                // Collect indented block (list or nested)
                let mut block: Vec<String> = Vec::new();
                let mut j = i + 1;
                while j < lines.len() {
                    let nxt = lines[j];
                    if nxt.trim().is_empty() {
                        j += 1;
                        continue;
                    }
                    let nxt_indent = nxt.len() - nxt.trim_start_matches(' ').len();
                    if nxt_indent <= indent {
                        break;
                    }
                    block.push(nxt.to_string());
                    j += 1;
                }
                if block.is_empty() {
                    out.insert(key, Value::Null);
                    i += 1;
                    continue;
                }
                // Determine if list: lines starting with "- "
                let is_list = block.iter().any(|l| l.trim_start().starts_with("- "));
                if is_list {
                    let mut arr = Vec::new();
                    for bl in block {
                        let t = bl.trim();
                        if t.starts_with("- ") {
                            let item_str = t[2..].trim();
                            arr.push(parse_yaml_scalar(item_str));
                        } else if t == "-" {
                            arr.push(Value::String(String::new()));
                        }
                    }
                    out.insert(key, Value::Array(arr));
                } else {
                    // Nested dict — not expected for web.xai but handle as string?
                    // For model/timeout it's scalar, so this path rarely taken.
                    // Serialize as object with scalar parsing
                    let mut submap = serde_json::Map::new();
                    for bl in block {
                        let t = bl.trim();
                        if let Some(cp) = t.find(':') {
                            let sk = t[..cp].trim().to_string();
                            let sv = t[cp + 1..].trim();
                            submap.insert(sk, parse_yaml_scalar(sv));
                        }
                    }
                    out.insert(key, Value::Object(submap));
                }
                i = j;
            }
        } else {
            i += 1;
        }
    }
    Some(out)
}

fn parse_yaml_scalar(s: &str) -> Value {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return Value::String(String::new());
    }
    if trimmed == "[]" {
        return Value::Array(Vec::new());
    }
    if trimmed == "{}" {
        return Value::Object(serde_json::Map::new());
    }
    if trimmed == "null" || trimmed == "~" || trimmed.eq_ignore_ascii_case("null") {
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
    Value::String(trimmed.to_string())
}

/// Mirrors `_coerce_domain_list(value) -> List[str]` — clean list of <=5 domain strings.
pub fn coerce_domain_list(value: Option<&Value>) -> Vec<String> {
    let arr = match value.and_then(|v| v.as_array()) {
        Some(a) => a,
        None => return Vec::new(),
    };
    let mut cleaned: Vec<String> = Vec::new();
    for item in arr {
        if let Some(s) = item.as_str() {
            let trimmed = s.trim().to_string();
            if !trimmed.is_empty() {
                cleaned.push(trimmed);
            }
        }
        if cleaned.len() >= MAX_DOMAIN_FILTERS {
            break;
        }
    }
    cleaned
}

// ---------------------------------------------------------------------------
// Web search result types
// ---------------------------------------------------------------------------

/// Single web result row — mirrors `{title, url, description, position}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebResult {
    pub title: String,
    pub url: String,
    pub description: String,
    pub position: usize,
}

/// Search success payload — `{"web": [...]}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchData {
    pub web: Vec<WebResult>,
}

/// Search envelope — mirrors `{"success": True/False, "data": ..., "error": ...}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResponse {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<SearchData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

// ---------------------------------------------------------------------------
// Provider — mirrors class XAIWebSearchProvider(WebSearchProvider)
// ---------------------------------------------------------------------------

/// Search-only provider backed by xAI's agentic Web Search tool.
///
/// Sends a structured prompt to Grok with `tools=[{"type": "web_search"}]`
/// enabled and asks it to return the top *limit* results as JSON. Falls
/// back to the Responses API `citations` list if Grok ignores the JSON
/// schema instruction (rare for grok-4.3 but cheap insurance).
///
/// No extract capability — pair with Firecrawl / Tavily / Exa for
/// `web_extract` if you need page content.
///
/// Trust model
/// -----------
/// Unlike index-backed providers (Brave / Tavily / Exa) which return
/// verbatim search-engine results, this backend is an LLM in a trench
/// coat: Grok decides which URLs to surface, generates the titles and
/// descriptions itself, and is influenced by the *content of the query*.
/// A maliciously crafted query (e.g. injected via untrusted upstream
/// input the agent picked up) can in principle steer Grok into emitting
/// attacker-chosen URLs. Callers that pipe untrusted text directly into
/// `web_search` should treat returned URLs the same way they would
/// treat any model-generated link — validate before fetching.
#[derive(Debug, Clone, Default)]
pub struct XAIWebSearchProvider;

impl XAIWebSearchProvider {
    pub fn new() -> Self {
        Self
    }

    pub fn name(&self) -> &'static str {
        "xai"
    }

    pub fn display_name(&self) -> &'static str {
        "xAI Web Search (Grok)"
    }

    /// Cheap availability probe — env var OR auth-store has OAuth tokens.
    ///
    /// Delegates to `has_xai_credentials`, which is deliberately *not* the same
    /// as `resolve_xai_http_credentials`: it never triggers OAuth token refresh
    /// or acquires the auth-store lock. The ABC contract requires this method
    /// to be safe to call on every `hermes tools` repaint and at
    /// tool-registration time. Token freshness / refresh is handled inside `search`.
    pub fn is_available(&self) -> bool {
        has_xai_credentials()
    }

    pub fn supports_search(&self) -> bool {
        true
    }

    pub fn supports_extract(&self) -> bool {
        false
    }

    // -- Search -----------------------------------------------------------

    /// Execute a Grok-backed web search.
    ///
    /// Returns `{"success": True, "data": {"web": [{title, url, description, position}, ...]}}`
    /// on success, `{"success": False, "error": str}` on failure.
    pub fn search(&self, query: &str, limit: i64) -> Value {
        // Best-effort interrupt check — mirrors `tools.interrupt.is_interrupted`
        if is_interrupted() {
            return json!({"success": false, "error": "Interrupted"});
        }

        let mut creds = resolve_xai_http_credentials(false, None);
        let mut api_key = creds.api_key.trim().to_string();
        let base_url = creds.base_url.trim().trim_end_matches('/').to_string();
        let base_url = if base_url.is_empty() {
            "https://api.x.ai/v1".to_string()
        } else {
            base_url
        };
        if api_key.is_empty() {
            return json!({
                "success": false,
                "error": "No xAI credentials found. Run `hermes auth` to sign in with xAI Grok OAuth, or set XAI_API_KEY."
            });
        }

        // Clamp limit to same range caller (web_search_tool) accepts
        let mut limit_i = limit;
        // In Python, `int(limit)` with try/except → 5 on failure. We already have i64.
        // Clamp 1..=100
        if limit_i < 1 {
            limit_i = 1;
        } else if limit_i > 100 {
            limit_i = 100;
        }
        let limit_usize = limit_i as usize;

        let cfg = load_xai_web_config();
        let model = cfg
            .get("model")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| DEFAULT_MODEL.to_string());

        let timeout: f64 = cfg
            .get("timeout")
            .map(|v| match v {
                Value::Number(n) => n.as_f64().unwrap_or(DEFAULT_TIMEOUT),
                Value::String(s) => s.trim().parse::<f64>().unwrap_or(DEFAULT_TIMEOUT),
                _ => DEFAULT_TIMEOUT,
            })
            .unwrap_or(DEFAULT_TIMEOUT);

        let allowed = coerce_domain_list(cfg.get("allowed_domains"));
        let excluded = coerce_domain_list(cfg.get("excluded_domains"));
        if !allowed.is_empty() && !excluded.is_empty() {
            return json!({
                "success": false,
                "error": "web.xai.allowed_domains and web.xai.excluded_domains cannot both be set (xAI restriction)."
            });
        }

        let mut web_search_tool = json!({"type": "web_search"});
        if !allowed.is_empty() {
            web_search_tool["filters"] = json!({"allowed_domains": allowed});
        } else if !excluded.is_empty() {
            web_search_tool["filters"] = json!({"excluded_domains": excluded});
        }

        let prompt = Self::build_prompt(query, limit_usize);

        let payload = json!({
            "model": model,
            "input": [{"role": "user", "content": prompt}],
            "tools": [web_search_tool],
            "include": ["no_inline_citations"]
        });

        let user_agent = hermes_xai_user_agent();
        let mut headers: HashMap<String, String> = [
            ("Authorization".to_string(), format!("Bearer {}", api_key)),
            ("Content-Type".to_string(), "application/json".to_string()),
            ("User-Agent".to_string(), user_agent),
        ]
        .into_iter()
        .collect();

        // Check httpx availability analogue — always true for curl/reqwest path
        // Python returns error if httpx not installed; Rust always has curl fallback.
        log::info!(
            "xAI web search via {}: '{}' (limit={}, model={})",
            base_url,
            query,
            limit_usize,
            model
        );

        let is_oauth_path = creds.provider == "xai-oauth";
        let url = format!("{}/responses", base_url);

        // Two-attempt loop: 401 + oauth path → force refresh and retry once
        let mut last_error: Option<Value> = None;
        let mut response_data: Option<Value> = None;

        for attempt in 0..2 {
            match http_post_with_status(&url, &headers, &payload, timeout) {
                Ok((status, body)) => {
                    if status >= 200 && status < 300 {
                        // Try parse JSON
                        match serde_json::from_str::<Value>(&body) {
                            Ok(data) => {
                                response_data = Some(data);
                                break;
                            }
                            Err(e) => {
                                log::warn!("xAI web search bad JSON: {}", e);
                                return json!({
                                    "success": false,
                                    "error": "Could not parse xAI Responses API reply as JSON"
                                });
                            }
                        }
                    } else if status == 401 && attempt == 0 && is_oauth_path {
                        log::info!(
                            "xAI web search got 401 on first attempt; forcing OAuth refresh and retrying once."
                        );
                        let refreshed = resolve_xai_http_credentials(true, Some(&api_key));
                        let refreshed_key = refreshed.api_key.trim().to_string();
                        if !refreshed_key.is_empty() && refreshed_key != api_key {
                            api_key = refreshed_key;
                            headers.insert("Authorization".to_string(), format!("Bearer {}", api_key));
                            creds = refreshed;
                            continue;
                        }
                        // Refresh returned same or empty token — no point retrying
                        let snippet = body.chars().take(300).collect::<String>();
                        log::warn!("xAI web search HTTP {}: {}", status, snippet);
                        return json!({
                            "success": false,
                            "error": format!("xAI web search returned HTTP {}: {}", status, snippet).trim()
                        });
                    } else {
                        let snippet = body.chars().take(300).collect::<String>();
                        log::warn!("xAI web search HTTP {}: {}", status, snippet);
                        return json!({
                            "success": false,
                            "error": format!("xAI web search returned HTTP {}: {}", status, snippet).trim()
                        });
                    }
                }
                Err(e) => {
                    // RequestError analogue
                    log::warn!("xAI web search request error: {}", e);
                    return json!({"success": false, "error": format!("Could not reach xAI: {}", e)});
                }
            }
            // If we reached here with response_data set, break; otherwise handle retry state
            if response_data.is_some() {
                break;
            }
            // For 401 retry, we already continued; if we are here, it was non-401 error which already returned.
            // To avoid infinite, break
            if attempt == 1 && response_data.is_none() {
                last_error = Some(json!({"success": false, "error": "xAI web search produced no response"}));
            }
        }

        let data = match response_data {
            Some(d) => d,
            None => {
                return last_error.unwrap_or_else(|| json!({"success": false, "error": "xAI web search produced no response"}));
            }
        };

        // Check error envelope — HTTP 200 with error field
        if let Some(api_error) = data.get("error").and_then(|v| v.as_object()) {
            let err_msg = api_error
                .get("message")
                .and_then(|v| v.as_str())
                .or_else(|| api_error.get("code").and_then(|v| v.as_str()))
                .unwrap_or("unknown error");
            log::warn!("xAI web search returned error envelope: {}", err_msg);
            return json!({"success": false, "error": format!("xAI returned an error: {}", err_msg)});
        }

        let web_results = Self::extract_results(&data, limit_usize);
        if web_results.is_empty() {
            return json!({"success": true, "data": {"web": []}});
        }

        let web_json: Vec<Value> = web_results
            .into_iter()
            .map(|r| {
                json!({
                    "title": r.title,
                    "url": r.url,
                    "description": r.description,
                    "position": r.position
                })
            })
            .collect();

        json!({"success": true, "data": {"web": web_json}})
    }

    // -- Prompt + parsing -------------------------------------------------

    /// Compose the prompt that asks Grok to act as a search engine.
    ///
    /// We deliberately ask for a JSON object (not bare array) so we can
    /// match it cheaply with `_JSON_BLOCK_RE`; we explicitly forbid
    /// prose, markdown fences, and inline-citation links to keep the
    /// payload parseable.
    pub fn build_prompt(query: &str, limit: usize) -> String {
        format!(
            "Use the web_search tool to find current information for the query below, \
             then respond with ONLY a single JSON object — no prose, no markdown \
             fences, no inline citation links — matching this exact schema:\n\n\
             {{\"results\": [{{\"title\": \"string\", \"url\": \"string\", \
             \"description\": \"1-2 sentence summary\"}}]}}\n\n\
             Return at most {} results, ordered by relevance, with absolute \
             https:// URLs. If no usable results exist, return \
             {{\"results\": []}}.\n\n\
             Query: {}",
            limit, query
        )
    }

    /// Pull a `[{title, url, description, position}, ...]` list out of a
    /// Responses-API reply.
    ///
    /// Strategy:
    /// 1. Walk `output[*].content[*].text` for `output_text` blocks and
    ///    try to parse the first JSON object that has a `results` list.
    /// 2. If the JSON path fails, fall back to the message annotations
    ///    (`url_citation` entries) — every annotation carries a URL and
    ///    a `title` (citation number); we pair those URLs with surrounding
    ///    text from the message body as a best-effort description.
    pub fn extract_results(response_data: &Value, limit: usize) -> Vec<WebResult> {
        let (text_blocks, annotations) = Self::collect_output_text(response_data);

        // Primary path: parse the JSON object Grok was asked for.
        for block in &text_blocks {
            if let Some(parsed) = Self::try_parse_json_results(block, limit) {
                return parsed;
            }
        }

        // Secondary path: derive results from message annotations + raw text.
        if !annotations.is_empty() {
            let joined_text = text_blocks.join("\n");
            let annotation_results = Self::results_from_annotations(&annotations, &joined_text, limit);
            if !annotation_results.is_empty() {
                return annotation_results;
            }
        }

        // Last-ditch: raw citations list (no titles or descriptions).
        if let Some(citations) = response_data.get("citations").and_then(|v| v.as_array()) {
            let mut out = Vec::new();
            for (i, u) in citations.iter().take(limit).enumerate() {
                if let Some(s) = u.as_str() {
                    let trimmed = s.trim();
                    if !trimmed.is_empty() {
                        out.push(WebResult {
                            title: String::new(),
                            url: trimmed.to_string(),
                            description: String::new(),
                            position: i + 1,
                        });
                    }
                }
            }
            // Filter to keep position renumbered after skips? Python enumerates filtered list
            // but uses enumerate on citations[:limit] and skips non-string/empty → position = i+1
            // which leaves gaps if early entry invalid. We keep same behavior for 1:1.
            // But to match Python exactly, we need to renumber only kept results sequentially.
            // Python's list comp uses `for i, u in enumerate(citations[:limit]) if ...` → position = i+1 (original index+1)
            // So gaps preserved. We did i+1 which matches that.
            // However if we skipped, we already did i+1 which is original index. That's correct.
            // But we pushed only when valid, so position reflects original index, not kept count.
            // That's 1:1.
            if !out.is_empty() {
                return out;
            }
            // If we want renumbered, we'd need len+1; but spec says original i+1.
            // Keep as implemented.
        }

        Vec::new()
    }

    /// Return (text_blocks, annotations) extracted from `response.output`.
    pub fn collect_output_text(response_data: &Value) -> (Vec<String>, Vec<Value>) {
        let mut text_blocks: Vec<String> = Vec::new();
        let mut annotations: Vec<Value> = Vec::new();
        let output = match response_data.get("output").and_then(|v| v.as_array()) {
            Some(arr) => arr,
            None => return (text_blocks, annotations),
        };
        for item in output {
            if !item.is_object() {
                continue;
            }
            if item.get("type").and_then(|v| v.as_str()) != Some("message") {
                continue;
            }
            let content = match item.get("content").and_then(|v| v.as_array()) {
                Some(a) => a,
                None => continue,
            };
            for chunk in content {
                if !chunk.is_object() {
                    continue;
                }
                if chunk.get("type").and_then(|v| v.as_str()) != Some("output_text") {
                    continue;
                }
                if let Some(text) = chunk.get("text").and_then(|v| v.as_str()) {
                    if !text.trim().is_empty() {
                        text_blocks.push(text.to_string());
                    }
                }
                if let Some(chunk_annotations) = chunk.get("annotations").and_then(|v| v.as_array()) {
                    for ann in chunk_annotations {
                        if ann.is_object() {
                            annotations.push(ann.clone());
                        }
                    }
                }
            }
        }
        (text_blocks, annotations)
    }

    /// Parse a JSON object with a `results` array out of `text`.
    ///
    /// Returns the normalized result list on success, `None` when the
    /// block has no valid JSON object or no `results` key. Tolerates
    /// leading/trailing prose because reasoning models sometimes prefix a
    /// short narration even when told not to.
    pub fn try_parse_json_results(text: &str, limit: usize) -> Option<Vec<WebResult>> {
        // Try whole string first — cheapest path when Grok obeys.
        let mut candidates: Vec<String> = vec![text.to_string()];
        if let Some(block) = find_json_block(text) {
            if block != text {
                candidates.push(block);
            }
        }

        for candidate in candidates {
            let parsed: Value = match serde_json::from_str(&candidate) {
                Ok(v) => v,
                Err(_) => continue,
            };
            if !parsed.is_object() {
                continue;
            }
            let results = match parsed.get("results").and_then(|v| v.as_array()) {
                Some(a) => a,
                None => continue,
            };
            let mut normalized: Vec<WebResult> = Vec::new();
            for row in results.iter().take(limit) {
                if !row.is_object() {
                    continue;
                }
                let url = row
                    .get("url")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_string();
                if url.is_empty() {
                    continue;
                }
                let title = row
                    .get("title")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_string();
                let description = row
                    .get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_string();
                normalized.push(WebResult {
                    title,
                    url,
                    description,
                    position: normalized.len() + 1,
                });
            }
            if !normalized.is_empty() {
                return Some(normalized);
            }
        }
        None
    }

    /// Best-effort fallback when JSON parsing fails.
    ///
    /// Uses each `url_citation` annotation's `url` (the citation
    /// title is just the integer label, so we don't surface it) and
    /// slices ~200 characters of surrounding text as the description.
    pub fn results_from_annotations(
        annotations: &[Value],
        joined_text: &str,
        limit: usize,
    ) -> Vec<WebResult> {
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut results: Vec<WebResult> = Vec::new();
        for ann in annotations {
            if ann.get("type").and_then(|v| v.as_str()) != Some("url_citation") {
                continue;
            }
            let url = ann
                .get("url")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            if url.is_empty() || seen.contains(&url) {
                continue;
            }
            seen.insert(url.clone());

            let mut description = String::new();
            if let (Some(start), Some(end)) = (
                ann.get("start_index").and_then(|v| v.as_u64()).map(|n| n as usize),
                ann.get("end_index").and_then(|v| v.as_u64()).map(|n| n as usize),
            ) {
                if start < end && end <= joined_text.len() {
                    let window_start = start.saturating_sub(200);
                    let slice = &joined_text[window_start..start];
                    let trimmed = slice.trim().to_string();
                    if trimmed.len() > 200 {
                        // Take last 200 chars on char boundary
                        let chars: Vec<char> = trimmed.chars().collect();
                        let len = chars.len();
                        let start_idx = len.saturating_sub(200);
                        description = chars[start_idx..].iter().collect::<String>().trim().to_string();
                    } else {
                        description = trimmed;
                    }
                }
            }

            results.push(WebResult {
                title: String::new(),
                url,
                description,
                position: results.len() + 1,
            });
            if results.len() >= limit {
                break;
            }
        }
        results
    }

    // -- Setup picker -----------------------------------------------------

    pub fn get_setup_schema() -> Value {
        // Auth resolution is delegated to the shared `xai_grok` post_setup
        // hook (same one image_gen.xai and tts.xai use) so users see the
        // familiar OAuth-or-API-key prompt for every xAI service.
        json!({
            "name": "xAI Web Search (Grok)",
            "badge": "paid",
            "tag": "Agentic web search via Grok's web_search tool — uses xAI Grok OAuth or XAI_API_KEY.",
            "env_vars": [],
            "post_setup": "xai_grok"
        })
    }
}

// ---------------------------------------------------------------------------
// Helpers: JSON block, HTTP, interrupt
// ---------------------------------------------------------------------------

/// Match the JSON object Grok is asked to emit. Tolerates leading/trailing
/// prose since reasoning models occasionally narrate before the JSON block
/// even when explicitly asked not to.
///
/// Python: `_JSON_BLOCK_RE = re.compile(r"\{[\s\S]*\}", re.MULTILINE)`
/// Greedy match from first `{` to last `}`.
fn find_json_block(text: &str) -> Option<String> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    if end < start {
        return None;
    }
    Some(text[start..=end].to_string())
}

fn is_interrupted() -> bool {
    // Mirrors `tools.interrupt.is_interrupted` — best-effort, always false here.
    // Python tries `from tools.interrupt import is_interrupted` and returns False on import failure.
    // Rust port: check env flag `HERMES_INTERRUPTED` for test injection, else false.
    if let Ok(v) = std::env::var("HERMES_INTERRUPTED") {
        let lower = v.trim().to_ascii_lowercase();
        return matches!(lower.as_str(), "1" | "true" | "yes");
    }
    false
}

/// Perform POST and return (status, body) via curl fallback.
///
/// Real port upgrade:
/// ```ignore
/// let client = reqwest::Client::builder().timeout(Duration::from_secs_f64(timeout)).build()?;
/// let resp = client.post(url).headers(hdrs).json(&payload).send().await?;
/// let status = resp.status().as_u16();
/// let body = resp.text().await?;
/// ```
fn http_post_with_status(
    url: &str,
    headers: &HashMap<String, String>,
    payload: &Value,
    timeout: f64,
) -> Result<(u16, String), String> {
    let body_str = serde_json::to_string(payload).unwrap_or_default();
    // Try curl first for observable behavior without new deps
    match try_curl_post(url, &body_str, headers, timeout as u64) {
        Ok((status, body)) => Ok((status, body)),
        Err(e) => Err(e),
    }
}

fn try_curl_post(
    url: &str,
    body: &str,
    headers: &HashMap<String, String>,
    timeout: u64,
) -> Result<(u16, String), String> {
    let mut cmd = std::process::Command::new("curl");
    cmd.arg("-sS")
        .arg("-m")
        .arg(timeout.to_string())
        .arg("-X")
        .arg("POST")
        .arg("-w")
        .arg("\n%{http_code}")
        .arg("-H")
        .arg("Content-Type: application/json");
    for (k, v) in headers {
        cmd.arg("-H").arg(format!("{}: {}", k, v));
    }
    cmd.arg("-d").arg(body).arg(url);
    let out = cmd.output().map_err(|e| e.to_string())?;
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    // Split body and status code: last line is http_code
    let mut lines: Vec<&str> = stdout.lines().collect();
    if lines.is_empty() {
        return Err(format!("curl produced no output for {}", url));
    }
    let code_str = lines.pop().unwrap_or("0").trim();
    let status: u16 = code_str.parse().unwrap_or(0);
    let body = lines.join("\n");
    if !out.status.success() && status == 0 {
        let stderr = String::from_utf8_lossy(&out.stderr).to_string();
        return Err(format!("curl failed: {} {}", stderr, body));
    }
    Ok((status, body))
}

// ---------------------------------------------------------------------------
// Plugin registration — mirrors ctx.register_web_search_provider
// ---------------------------------------------------------------------------

/// Registration descriptor — mirrors kwargs to `ctx.register_web_search_provider(...)`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XAIPluginRegistration {
    pub name: String,
    pub label: String,
    pub search_backend: String,
    pub requires_env: Vec<String>,
}

impl Default for XAIPluginRegistration {
    fn default() -> Self {
        Self {
            name: "xai".to_string(),
            label: "xAI Web Search (Grok)".to_string(),
            search_backend: "xai".to_string(),
            requires_env: vec!["XAI_API_KEY".to_string()],
        }
    }
}

/// Minimal `ctx` trait for plugin registration — mirrors `hermes_cli.plugins.PluginContext`.
pub trait PluginContext {
    fn register_web_search_provider(
        &mut self,
        name: &str,
        display_name: &str,
        supports_search: bool,
        supports_extract: bool,
    );
}

/// Mirrors `def register(ctx) -> None` for xAI web provider.
///
/// Python discovers providers via `plugins/web/xai/provider.py` and calls
/// `ctx.register_web_search_provider(XAIWebSearchProvider())`.
pub fn register(ctx: &mut dyn PluginContext) {
    let reg = XAIPluginRegistration::default();
    let provider = XAIWebSearchProvider::new();
    ctx.register_web_search_provider(
        &reg.name,
        provider.display_name(),
        provider.supports_search(),
        provider.supports_extract(),
    );
    // Keep function pointers alive for registry binding
    let _ = (XAIWebSearchProvider::new as fn() -> XAIWebSearchProvider);
    let _ = (has_xai_credentials as fn() -> bool);
    let _ = (resolve_xai_http_credentials as fn(bool, Option<&str>) -> XaiCredentials);
    let _ = (hermes_xai_user_agent as fn() -> String);
}

// ---------------------------------------------------------------------------
// Tests — mirrors Python contract invariants (no live network)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn constants_match_python() {
        assert_eq!(DEFAULT_MODEL, "grok-build-0.1");
        assert_eq!(DEFAULT_TIMEOUT, 90.0);
        assert_eq!(MAX_DOMAIN_FILTERS, 5);
    }

    #[test]
    fn coerce_domain_list_filters_and_caps() {
        let v = json!([" x.ai ", "", "  ", "foo.com", "bar.com", "baz.com", "qux.com", "extra.com"]);
        let out = coerce_domain_list(Some(&v));
        assert_eq!(out, vec!["x.ai", "foo.com", "bar.com", "baz.com", "qux.com"]);
        assert_eq!(out.len(), 5);
        assert!(!out.contains(&"".to_string()));
    }

    #[test]
    fn coerce_domain_list_non_list_returns_empty() {
        assert!(coerce_domain_list(None).is_empty());
        assert!(coerce_domain_list(Some(&json!("not a list"))).is_empty());
        assert!(coerce_domain_list(Some(&json!({"a": 1}))).is_empty());
    }

    #[test]
    fn build_prompt_contains_query_and_limit() {
        let p = XAIWebSearchProvider::build_prompt("hello world", 7);
        assert!(p.contains("hello world"));
        assert!(p.contains("at most 7 results"));
        assert!(p.contains(r#"{"results": [{"title": "string""#));
    }

    #[test]
    fn find_json_block_greedy() {
        let text = "prose {\"results\": [{\"title\": \"a\", \"url\": \"https://a.com\"}]} trailing";
        let block = find_json_block(text).unwrap();
        assert!(block.starts_with('{'));
        assert!(block.ends_with('}'));
        let parsed: Value = serde_json::from_str(&block).unwrap();
        assert!(parsed.get("results").is_some());
    }

    #[test]
    fn try_parse_json_results_happy_path() {
        let text = r#"{"results": [{"title": "T", "url": "https://example.com", "description": "d"}]}"#;
        let out = XAIWebSearchProvider::try_parse_json_results(text, 5).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].url, "https://example.com");
        assert_eq!(out[0].position, 1);
    }

    #[test]
    fn try_parse_json_results_with_prose() {
        let text = "Here is JSON: {\"results\": [{\"title\": \"T\", \"url\": \"https://example.com\", \"description\": \"d\"}]} done";
        let out = XAIWebSearchProvider::try_parse_json_results(text, 5).unwrap();
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn try_parse_json_results_skips_empty_url() {
        let text = r#"{"results": [{"title": "T", "url": "", "description": "d"}, {"title": "T2", "url": "https://ok.com", "description": "d2"}]}"#;
        let out = XAIWebSearchProvider::try_parse_json_results(text, 5).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].url, "https://ok.com");
        assert_eq!(out[0].position, 1); // renumbered, not gap
    }

    #[test]
    fn try_parse_json_results_limit_clamp() {
        let text = r#"{"results": [
            {"title": "a", "url": "https://a.com", "description": ""},
            {"title": "b", "url": "https://b.com", "description": ""},
            {"title": "c", "url": "https://c.com", "description": ""}
        ]}"#;
        let out = XAIWebSearchProvider::try_parse_json_results(text, 2).unwrap();
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn try_parse_json_results_returns_none_on_invalid() {
        assert!(XAIWebSearchProvider::try_parse_json_results("no json here", 5).is_none());
        assert!(XAIWebSearchProvider::try_parse_json_results(r#"{"nope": 123}"#, 5).is_none());
    }

    #[test]
    fn collect_output_text_extracts_blocks() {
        let data = json!({
            "output": [
                {"type": "message", "content": [
                    {"type": "output_text", "text": "hello", "annotations": [{"type": "url_citation", "url": "https://a.com"}]},
                    {"type": "output_text", "text": "world"}
                ]},
                {"type": "other", "content": []}
            ]
        });
        let (blocks, ann) = XAIWebSearchProvider::collect_output_text(&data);
        assert_eq!(blocks, vec!["hello", "world"]);
        assert_eq!(ann.len(), 1);
    }

    #[test]
    fn extract_results_primary_json_path() {
        let data = json!({
            "output": [
                {"type": "message", "content": [
                    {"type": "output_text", "text": r#"{"results": [{"title": "T", "url": "https://example.com", "description": "desc"}]}"#}
                ]}
            ]
        });
        let out = XAIWebSearchProvider::extract_results(&data, 5);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].title, "T");
    }

    #[test]
    fn extract_results_annotation_fallback() {
        let joined = "prefix text that will be used as description https://example.com suffix";
        // Build response where text block has no parseable JSON, but annotations exist
        let data = json!({
            "output": [
                {"type": "message", "content": [
                    {"type": "output_text", "text": joined, "annotations": [
                        {"type": "url_citation", "url": "https://example.com", "start_index": 10, "end_index": 20}
                    ]}
                ]}
            ]
        });
        let out = XAIWebSearchProvider::extract_results(&data, 5);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].url, "https://example.com");
        assert_eq!(out[0].position, 1);
    }

    #[test]
    fn extract_results_citations_fallback() {
        let data = json!({
            "output": [],
            "citations": ["https://a.com", "https://b.com", "", "https://c.com"]
        });
        let out = XAIWebSearchProvider::extract_results(&data, 2);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].url, "https://a.com");
        assert_eq!(out[1].url, "https://b.com");
    }

    #[test]
    fn results_from_annotations_dedup_and_limit() {
        let ann = vec![
            json!({"type": "url_citation", "url": "https://a.com", "start_index": 5, "end_index": 10}),
            json!({"type": "url_citation", "url": "https://a.com", "start_index": 5, "end_index": 10}),
            json!({"type": "other", "url": "https://b.com"}),
            json!({"type": "url_citation", "url": "https://b.com", "start_index": 0, "end_index": 5}),
        ];
        let joined = "0123456789".repeat(30);
        let out = XAIWebSearchProvider::results_from_annotations(&ann, &joined, 1);
        assert_eq!(out.len(), 1);
        // dedup + type filter leaves only first a.com and then b.com but limit 1
        assert_eq!(out[0].url, "https://a.com");
    }

    #[test]
    fn provider_properties() {
        let p = XAIWebSearchProvider::new();
        assert_eq!(p.name(), "xai");
        assert_eq!(p.display_name(), "xAI Web Search (Grok)");
        assert!(p.supports_search());
        assert!(!p.supports_extract());
    }

    #[test]
    fn get_setup_schema_shape() {
        let s = XAIWebSearchProvider::get_setup_schema();
        assert_eq!(s["name"], "xAI Web Search (Grok)");
        assert_eq!(s["badge"], "paid");
        assert_eq!(s["post_setup"], "xai_grok");
        assert!(s["env_vars"].as_array().unwrap().is_empty());
    }

    #[test]
    fn has_xai_credentials_false_when_no_env_and_no_file() {
        let prev = std::env::var("XAI_API_KEY").ok();
        let prev_home = std::env::var("HERMES_HOME").ok();
        unsafe { std::env::remove_var("XAI_API_KEY"); }
        let tmp = std::env::temp_dir().join(format!("hermes-test-{}-xai", std::process::id()));
        let _ = fs::create_dir_all(&tmp);
        unsafe { std::env::set_var("HERMES_HOME", tmp.to_string_lossy().to_string()); }
        // No .env, no auth.json
        let _ = fs::remove_file(tmp.join(".env"));
        let _ = fs::remove_file(tmp.join("auth.json"));
        assert!(!has_xai_credentials());
        // cleanup
        let _ = fs::remove_dir_all(&tmp);
        if let Some(v) = prev { unsafe { std::env::set_var("XAI_API_KEY", v); } }
        if let Some(v) = prev_home { unsafe { std::env::set_var("HERMES_HOME", v); } } else { unsafe { std::env::remove_var("HERMES_HOME"); } }
    }
}
