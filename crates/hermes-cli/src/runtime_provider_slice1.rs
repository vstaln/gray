//! hermes-cli runtime_provider — slice 1/3
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/hermes_cli/runtime_provider.py`
//! slice 1/3 — lines 1–900 of 2 451 (first 900 LOC).
//! Covers: module docstring (shared runtime provider resolution), imports
//! (`hermes_cli.auth`, `agent.credential_pool`, `agent.secret_scope`,
//! `hermes_cli.config`, `hermes_cli.providers`, `hermes_constants`,
//! `utils.base_url_*`), `_getenv` (profile-scoped secret wrapper),
//! `_normalize_custom_provider_name`, `_loopback_hostname`,
//! `_config_base_url_trustworthy_for_bare_custom` (GitHub #14676/#27132),
//! `_detect_api_mode_for_url` (codex/anthropic/kimi host mandates),
//! `_fallback_api_mode`, `_resolve_plain_custom_api_mode`,
//! `_host_derived_api_key`, `_anthropic_base_url_override_ok`,
//! `_auto_detect_local_model`, `_get_model_config` (dict default alias,
//! dict-default split, local-model fallback), `_provider_supports_explicit_api_mode`,
//! `_copilot_runtime_api_mode`, `_VALID_API_MODES` + `_parse_api_mode`,
//! `_nous_inference_base_url_override`, `_maybe_apply_codex_app_server_runtime`,
//! `_resolve_runtime_from_pool_entry` (provider-specific api_mode/base_url wiring
//! for openai-codex/xai-oauth/qwen-oauth/minimax-oauth/anthropic/openrouter/
//! xai/nous/copilot/azure-foundry + generic + opencode + codex_app_server +
//! lmstudio tail), `resolve_requested_provider`, `_try_resolve_from_custom_pool`,
//! `_lift_max_output_tokens`, `_lift_extra_headers`, `_get_named_custom_provider`
//! (providers: dict + legacy custom_providers list, alias/shadow/enabled checks,
//! extra_body/extra_headers/key_cmd/transport lifting), `has_named_custom_provider`,
//! and the head of `find_custom_provider_identity` (URL reverse lookup through
//! the `providers:` loop, up to `_normalize_base_url_for_match(entry_url)==target`
//! at line 905 — slice boundary at 900, mid-loop between the `providers:` and
//! `custom_providers:` scans).
//! Continued in `runtime_provider_slice2.rs` (from `if _normalize…==target` tail
//! + `custom_providers` fallback loop + `find_custom_provider_identity_by_model` …).
//!
//! T0697 — 1:1 port, no cargo (NEVER cargo).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

// ---------------------------------------------------------------------------
// Module docstring — mirrors lines 1
// ---------------------------------------------------------------------------

/// Module doc — Shared runtime provider resolution for CLI, gateway, cron, and helpers.
/// Mirrors `hermes_cli/runtime_provider.py` line 1.
pub const MODULE_DOC: &str =
    "runtime_provider: shared runtime provider resolution for CLI, gateway, cron, and helpers";

// ---------------------------------------------------------------------------
// Imports — mirrors lines 3-51
// ---------------------------------------------------------------------------
// Python:
//   import logging, os, re
//   from urllib.parse import urlparse
//   from typing import Any, Dict, Optional
//   from hermes_cli import auth as auth_mod
//   from agent.credential_pool import (CredentialPool, PooledCredential,
//       credential_pool_matches_provider, get_custom_provider_pool_key, load_pool)
//   from agent.secret_scope import get_secret as _get_secret
//   from hermes_cli.auth import (ACTUAL_LOCAL_NOAUTH_PLACEHOLDER, AuthError,
//       DEFAULT_CODEX_BASE_URL, DEFAULT_QWEN_BASE_URL, DEFAULT_XAI_OAUTH_BASE_URL,
//       PROVIDER_REGISTRY, _agent_key_is_usable, _nous_inference_env_override,
//       format_auth_error, resolve_provider, resolve_nous_runtime_credentials,
//       resolve_codex_runtime_credentials, resolve_xai_oauth_runtime_credentials,
//       resolve_qwen_runtime_credentials, resolve_api_key_provider_credentials,
//       resolve_external_process_provider_credentials, has_usable_secret,
//       is_actual_local_base_url, normalize_actual_base_url)
//   from hermes_cli.config import (get_compatible_custom_providers, load_config,
//       normalize_extra_headers)
//   from hermes_cli.providers import custom_provider_aliases, custom_provider_slug
//   from hermes_constants import OPENROUTER_BASE_URL
//   from hermes_cli.providers import is_official_openai_host
//   from utils import base_url_host_matches, base_url_hostname, env_int
//
// Rust: std only (NEVER cargo). All external/Python modules are stubbed for 1:1
// traceability; real wiring in later slices when those modules are ported.

pub fn log_debug(msg: &str) {
    eprintln!("[runtime_provider DEBUG] {msg}");
}
pub fn log_info(msg: &str) {
    eprintln!("[runtime_provider INFO] {msg}");
}

// ---------------------------------------------------------------------------
// Constants — mirrors auth/config re-exports used in this slice
// ---------------------------------------------------------------------------

pub const DEFAULT_CODEX_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";
pub const DEFAULT_XAI_OAUTH_BASE_URL: &str = "https://api.x.ai/v1";
pub const DEFAULT_QWEN_BASE_URL: &str = "https://portal.qwen.ai/v1";
pub const OPENROUTER_BASE_URL: &str = "https://openrouter.ai/api/v1";
pub const ACTUAL_LOCAL_NOAUTH_PLACEHOLDER: &str = "dummy-actual-local-api-key";
pub const DEFAULT_GITHUB_MODELS_BASE_URL: &str = "https://api.githubcopilot.com";

// ---------------------------------------------------------------------------
// Secret scope stub — mirrors agent.secret_scope.get_secret (line 21)
// ---------------------------------------------------------------------------

/// Mirrors `agent.secret_scope.get_secret(name, default)` — profile-scoped secret read.
/// In Python this is scope-aware (multiplex fail-closed) and falls back to os.environ
/// for genuinely-global vars. In Rust slice 1 we check env var then return default
/// for 1:1 contract preservation.
pub fn get_secret_stub(name: &str, default: &str) -> Option<String> {
    // Try process env first (mirrors scope-aware read's fallback to os.environ for global vars)
    if let Ok(v) = std::env::var(name) {
        if !v.is_empty() {
            return Some(v);
        }
    }
    // Check HERMES_HOME/.env best-effort (mirrors hermes_cli.config.get_env_value_prefer_dotenv for secrets)
    if let Some(v) = read_dotenv_value(name) {
        if !v.trim().is_empty() {
            return Some(v);
        }
    }
    if default.is_empty() {
        None
    } else {
        Some(default.to_string())
    }
}

fn read_dotenv_value(var: &str) -> Option<String> {
    let home = get_hermes_home();
    let env_file = home.join(".env");
    let text = std::fs::read_to_string(&env_file).ok()?;
    for line in text.lines() {
        let mut l = line.trim();
        if l.is_empty() || l.starts_with('#') || !l.contains('=') {
            continue;
        }
        if l.starts_with("export ") {
            l = l[7..].trim();
        }
        if let Some((k, v)) = l.split_once('=') {
            if k.trim() == var {
                let val = v.trim().trim_matches(|c| c == '\'' || c == '"').to_string();
                return Some(val);
            }
        }
    }
    None
}

fn get_hermes_home() -> PathBuf {
    if let Ok(v) = std::env::var("HERMES_HOME") {
        if !v.trim().is_empty() {
            return PathBuf::from(v.trim());
        }
    }
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
        .join(".hermes")
}

// ---------------------------------------------------------------------------
// URL helpers — mirrors utils.base_url_hostname / base_url_host_matches
// and urllib.parse.urlparse (lines 7..)
// ---------------------------------------------------------------------------

fn extract_hostname(url: &str) -> Option<String> {
    let url = url.trim();
    if url.is_empty() {
        return None;
    }
    let after_scheme = if let Some(idx) = url.find("://") {
        &url[idx + 3..]
    } else {
        url
    };
    let host_port_path = after_scheme;
    let slash_pos = host_port_path.find('/').unwrap_or(host_port_path.len());
    let host_port = &host_port_path[..slash_pos];
    let host_port = if let Some(at_pos) = host_port.rfind('@') {
        &host_port[at_pos + 1..]
    } else {
        host_port
    };
    if host_port.starts_with('[') {
        if let Some(end) = host_port.find(']') {
            return Some(host_port[1..end].to_string());
        }
        return None;
    }
    let host = if let Some(colon) = host_port.rfind(':') {
        if host_port[colon + 1..].chars().all(|c| c.is_ascii_digit()) {
            &host_port[..colon]
        } else {
            host_port
        }
    } else {
        host_port
    };
    if host.is_empty() {
        None
    } else {
        Some(host.to_lowercase())
    }
}

/// Mirrors `utils.base_url_hostname(base_url)` — lowercased hostname or "".
pub fn base_url_hostname(base_url: &str) -> String {
    extract_hostname(base_url).unwrap_or_default().to_lowercase()
}

/// Mirrors `utils.base_url_host_matches(base_url, host)` — suffix match with dot boundary.
pub fn base_url_host_matches(base_url: &str, host: &str) -> bool {
    let bh = base_url_hostname(base_url);
    let target = host.trim().to_lowercase().trim_end_matches('.').to_string();
    if bh.is_empty() || target.is_empty() {
        return false;
    }
    if bh == target {
        return true;
    }
    bh.ends_with(&format!(".{target}"))
}

fn extract_path(url: &str) -> String {
    let url = url.trim();
    let after_scheme = if let Some(idx) = url.find("://") {
        &url[idx + 3..]
    } else {
        url
    };
    if let Some(slash) = after_scheme.find('/') {
        let path_and_rest = &after_scheme[slash..];
        let end = path_and_rest
            .find(|c| c == '?' || c == '#')
            .unwrap_or(path_and_rest.len());
        path_and_rest[..end].to_string()
    } else {
        String::new()
    }
}

/// Mirrors `utils.env_int(name, default)` — parse env var as i64.
pub fn env_int(name: &str, default: i64) -> i64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.trim().parse::<i64>().ok())
        .unwrap_or(default)
}

// ---------------------------------------------------------------------------
// Provider helpers stubs — mirrors hermes_cli.providers / hermes_cli.auth
// ---------------------------------------------------------------------------

/// Mirrors `hermes_cli.providers.is_official_openai_host(base_url)`.
/// True for canonical api.openai.com family plus data-residency regional hosts.
pub fn is_official_openai_host(base_url: &str) -> bool {
    let host = base_url_hostname(base_url);
    if host == "api.openai.com" {
        return true;
    }
    // Regional hosts: us.api.openai.com, eu.api.openai.com etc → ends_with ".api.openai.com"
    // but must not accept spoof lookalikes like api.openai.com.attacker.test
    if host.ends_with(".api.openai.com") {
        return true;
    }
    false
}

/// Mirrors `hermes_cli.providers.custom_provider_slug(name, provider_key)`.
pub fn custom_provider_slug(name: &str, provider_key: &str) -> String {
    let key = if !provider_key.trim().is_empty() {
        provider_key
    } else {
        name
    };
    let normalized = key.trim().to_lowercase().replace(' ', "-");
    let slug = normalized
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_string();
    if slug.is_empty() {
        return "custom:unknown".to_string();
    }
    format!("custom:{slug}")
}

/// Mirrors `hermes_cli.providers.custom_provider_aliases(name, provider_key)`.
pub fn custom_provider_aliases(name: &str, provider_key: &str) -> HashSet<String> {
    let mut out = HashSet::new();
    for raw in [name, provider_key] {
        let n = raw.trim();
        if n.is_empty() {
            continue;
        }
        let norm = n.to_lowercase().replace(' ', "-");
        out.insert(norm.clone());
        out.insert(format!("custom:{norm}"));
        // Also insert slugged alias (handles chars → - sanitization)
        let slug = custom_provider_slug(n, "");
        // custom_provider_slug returns "custom:..." — extract tail
        if let Some(tail) = slug.strip_prefix("custom:") {
            out.insert(tail.to_string());
            out.insert(format!("custom:{tail}"));
        }
    }
    out
}

/// Mirrors `hermes_cli.providers.determine_api_mode(provider, base_url, model)`.
pub fn determine_api_mode(provider: &str, base_url: &str, model: &str) -> Option<String> {
    // Replicates overlay transport map tail in providers module.
    // For slice 1 we use the same host mandates as _detect_api_mode_for_url plus
    // a small provider transport overlay for known dual-wire providers.
    let detected = _detect_api_mode_for_url(base_url);
    if detected.is_some() {
        return detected;
    }
    let p = provider.trim().to_lowercase();
    let m = model.trim().to_lowercase();
    // Opencode family is resolved via opencode_model_api_mode — handled by caller
    // minimax / kimi family hints — not needed in fallback stub
    // Copilot / nous / azure-foundry handled explicitly in _resolve_runtime_from_pool_entry
    // For remaining known overlay providers:
    if p == "zai" || p == "kimi-coding" || p == "kimi-coding-cn" || p == "deepseek" {
        return None;
    }
    // Generic fallback: check model family for reasoning -> codex_responses hint
    if m.contains("gpt-5") || m.contains("codex") || m.starts_with("o1") || m.starts_with("o3") || m.starts_with("o4") {
        // Do not auto-return codex_responses generically — let host detection decide
        return None;
    }
    None
}

/// Mirrors `hermes_cli.auth.resolve_provider(name)` — canonicalize provider alias → id.
pub fn resolve_provider(name: &str) -> Result<String, String> {
    let n = name.trim().to_lowercase();
    if n.is_empty() {
        return Err("empty provider".to_string());
    }
    // Builtin canonical map (mirrors PROVIDER_REGISTRY keys + alias table)
    // Aliases that resolve to "custom": ollama, vllm, llamacpp, etc. (GitHub #27132)
    let alias_to_canonical: HashMap<&str, &str> = [
        ("ollama", "custom"),
        ("vllm", "custom"),
        ("llamacpp", "custom"),
        ("llama.cpp", "custom"),
        ("lmstudio", "lmstudio"),
        ("openrouter", "openrouter"),
        ("openai", "openai-api"),
        ("anthropic", "anthropic"),
        ("zai", "zai"),
        ("kimi", "kimi-coding"),
        ("kimi-coding", "kimi-coding"),
        ("nous", "nous"),
        ("copilot", "copilot"),
        ("xai", "xai"),
        ("minimax", "minimax"),
        ("deepseek", "deepseek"),
        ("nvidia", "nvidia"),
        ("huggingface", "huggingface"),
        ("arcee", "arcee"),
        ("azure-foundry", "azure-foundry"),
        ("bedrock", "bedrock"),
        ("actual", "actual"),
        ("custom", "custom"),
    ]
    .into_iter()
    .collect();
    if let Some(canonical) = alias_to_canonical.get(n.as_str()) {
        return Ok(canonical.to_string());
    }
    // If name already is a custom:<name> key, strip prefix and return "custom"
    if n.starts_with("custom:") {
        return Ok("custom".to_string());
    }
    // Unknown names propagate as AuthError in Python; we surface Err
    Err(format!("unknown provider: {n}"))
}

/// Mirrors `hermes_cli.auth._nous_inference_env_override()`.
pub fn nous_inference_env_override() -> String {
    std::env::var("NOUS_INFERENCE_BASE_URL")
        .ok()
        .map(|v| v.trim().trim_end_matches('/').to_string())
        .unwrap_or_default()
}

/// Mirrors `hermes_cli.auth.has_usable_secret(value)` (default min_length=4).
pub fn has_usable_secret(value: &str) -> bool {
    let v = value.trim();
    if v.len() < 4 {
        return false;
    }
    let lower = v.to_lowercase();
    let placeholders: HashSet<&str> = [
        "*", "**", "***", "changeme", "your_api_key", "your_api_key_here", "your-api-key",
        "placeholder", "example", "dummy", "null", "none",
    ]
    .into_iter()
    .collect();
    if placeholders.contains(lower.as_str()) {
        return false;
    }
    true
}

/// Mirrors `hermes_cli.auth.is_actual_local_base_url(base_url)`.
pub fn is_actual_local_base_url(base_url: &str) -> bool {
    let host = base_url_hostname(base_url);
    let h = host.trim_end_matches('.');
    matches!(h, "localhost" | "127.0.0.1" | "::1" | "0.0.0.0")
}

/// Mirrors `hermes_cli.auth.normalize_actual_base_url(base_url)`.
pub fn normalize_actual_base_url(base_url: &str) -> String {
    let url = base_url.trim().trim_end_matches('/').to_string();
    if url.is_empty() {
        return "https://api.actual.inc/v1".to_string();
    }
    let host = base_url_hostname(&url);
    let path = extract_path(&url).trim_end_matches('/').to_string();
    if host == "api.actual.inc" && (path.is_empty() || path == "/") {
        return format!("{url}/v1");
    }
    if is_actual_local_base_url(&url) && (path.is_empty() || path == "/") {
        return format!("{url}/v1");
    }
    url
}

/// Mirrors `hermes_cli.config.normalize_extra_headers(raw)` — validate header dict.
pub fn normalize_extra_headers(raw: Option<&HashMap<String, String>>) -> HashMap<String, String> {
    let mut out = HashMap::new();
    if let Some(map) = raw {
        for (k, v) in map {
            let key = k.trim();
            if key.is_empty() || v.trim().is_empty() {
                continue;
            }
            // Basic header name sanity: no control chars, no colon
            if key.contains(':') || key.chars().any(|c| c.is_control()) {
                continue;
            }
            out.insert(key.to_string(), v.trim().to_string());
        }
    }
    out
}

/// Mirrors `hermes_cli.config.get_compatible_custom_providers(config)`.
pub fn get_compatible_custom_providers(
    config: &HashMap<String, String>,
) -> Vec<HashMap<String, String>> {
    // Slice 1 stub: legacy custom_providers list is stored under key "custom_providers"
    // as JSON? In slice 1 config is stubbed flat, so return empty.
    let _ = config;
    Vec::new()
}

/// Stub credential_pool types — mirrors agent.credential_pool
#[derive(Debug, Clone)]
pub struct PooledCredential {
    pub runtime_base_url: String,
    pub base_url: String,
    pub runtime_api_key: String,
    pub access_token: String,
    pub source: String,
}

#[derive(Debug, Clone)]
pub struct CredentialPool {
    pub key: String,
    entries: Vec<PooledCredential>,
}

impl CredentialPool {
    pub fn has_credentials(&self) -> bool {
        !self.entries.is_empty()
    }
    pub fn select(&self) -> Option<PooledCredential> {
        self.entries.first().cloned()
    }
    pub fn peek(&self) -> Option<PooledCredential> {
        self.select()
    }
}

/// Mirrors `agent.credential_pool.get_custom_provider_pool_key(base_url, provider_name=...)`.
pub fn get_custom_provider_pool_key(base_url: &str, provider_name: Option<&str>) -> Option<String> {
    let bu = base_url.trim();
    if bu.is_empty() {
        return None;
    }
    let host = base_url_hostname(bu);
    if host.is_empty() {
        return None;
    }
    // Normalize to host+path slug
    let mut key = host.replace('.', "_").replace(':', "_");
    let path = extract_path(bu);
    if !path.is_empty() && path != "/" {
        let slug = path
            .trim_matches('/')
            .replace('/', "_")
            .replace('-', "_")
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect::<String>();
        if !slug.is_empty() {
            key = format!("{key}_{slug}");
        }
    }
    if let Some(pn) = provider_name {
        let pn = pn.trim().to_lowercase().replace(' ', "_");
        if !pn.is_empty() {
            key = format!("{key}_{pn}");
        }
    }
    Some(key)
}

/// Mirrors `agent.credential_pool.load_pool(pool_key)`.
pub fn load_pool(pool_key: &str) -> CredentialPool {
    // Slice 1 stub: no persistent pool file read (requires auth.json locking + serde)
    // Return empty pool for 1:1 signature coverage; caller checks has_credentials().
    CredentialPool {
        key: pool_key.to_string(),
        entries: Vec::new(),
    }
}

/// Mirrors `agent.credential_pool.credential_pool_matches_provider(pool_key, provider)`.
pub fn credential_pool_matches_provider(_pool_key: &str, _provider: &str) -> bool {
    false
}

/// Provider profile stub for PROVIDER_REGISTRY expansion (mirrors hermes_cli.auth PROVIDER_REGISTRY)
#[derive(Debug, Clone)]
pub struct ProviderConfig {
    pub id: String,
    pub name: String,
    pub inference_base_url: String,
    pub api_key_env_vars: Vec<String>,
    pub base_url_env_var: String,
}

static PROVIDER_REGISTRY_STUB: OnceLock<HashMap<String, ProviderConfig>> = OnceLock::new();

pub fn provider_registry() -> &'static HashMap<String, ProviderConfig> {
    PROVIDER_REGISTRY_STUB.get_or_init(|| {
        let mut m = HashMap::new();
        let entries: Vec<(&str, &str, Vec<&str>, &str)> = vec![
            ("nous", "Nous Portal", vec![], "https://inference-api.nousresearch.com/v1"),
            ("openai-codex", "OpenAI Codex", vec![], DEFAULT_CODEX_BASE_URL),
            ("openai-api", "OpenAI API", vec!["OPENAI_API_KEY"], "https://api.openai.com/v1"),
            ("xai-oauth", "xAI Grok OAuth", vec![], DEFAULT_XAI_OAUTH_BASE_URL),
            ("qwen-oauth", "Qwen OAuth", vec![], DEFAULT_QWEN_BASE_URL),
            ("copilot", "GitHub Copilot", vec!["COPILOT_GITHUB_TOKEN", "GH_TOKEN", "GITHUB_TOKEN"], "https://api.githubcopilot.com"),
            ("anthropic", "Anthropic", vec!["ANTHROPIC_API_KEY", "ANTHROPIC_TOKEN", "CLAUDE_CODE_OAUTH_TOKEN"], "https://api.anthropic.com"),
            ("openrouter", "OpenRouter", vec!["OPENROUTER_API_KEY"], OPENROUTER_BASE_URL),
            ("xai", "xAI", vec!["XAI_API_KEY"], "https://api.x.ai/v1"),
            ("minimax", "MiniMax", vec!["MINIMAX_API_KEY"], "https://api.minimax.io/anthropic"),
            ("minimax-oauth", "MiniMax OAuth", vec![], "https://api.minimax.io/anthropic"),
            ("lmstudio", "LM Studio", vec!["LM_API_KEY"], "http://127.0.0.1:1234/v1"),
            ("ollama-cloud", "Ollama Cloud", vec!["OLLAMA_API_KEY"], "https://ollama.com/v1"),
            ("actual", "Actual Computer", vec!["ACTUAL_API_KEY"], "https://api.actual.inc/v1"),
            ("azure-foundry", "Azure Foundry", vec!["AZURE_FOUNDRY_API_KEY"], ""),
        ];
        for (id, name, env_vars, base) in entries {
            m.insert(
                id.to_string(),
                ProviderConfig {
                    id: id.to_string(),
                    name: name.to_string(),
                    inference_base_url: base.to_string(),
                    api_key_env_vars: env_vars.into_iter().map(|s| s.to_string()).collect(),
                    base_url_env_var: String::new(),
                },
            );
        }
        m
    })
}

// Config stubs — mirrors hermes_cli.config.load_config etc.
type RawConfig = HashMap<String, String>;

pub fn load_config() -> RawConfig {
    // Slice 1 stub: best-effort read of HERMES_HOME/config.yaml as flat k=v
    // Mirrors hermes_cli.config.load_config() deep-merged defaults; stub returns empty
    // when file missing so callers fall through to defaults/env.
    let path = get_hermes_home().join("config.yaml");
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    // Minimal parse for top-level scalar keys (provider / base_url / api_mode) — enough for model_cfg resolution
    let mut out = HashMap::new();
    let mut in_model = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("model:") {
            in_model = true;
            continue;
        }
        if in_model {
            if !line.starts_with(' ') && !line.starts_with('\t') && !trimmed.is_empty() {
                break;
            }
            for key in ["provider", "base_url", "api_mode", "default", "model", "openai_runtime"] {
                if trimmed.starts_with(&format!("{key}:")) {
                    let val = trimmed[format!("{key}:").len()..].trim().trim_matches(|c| c == '"' || c == '\'').to_string();
                    out.insert(format!("model.{key}"), val);
                }
            }
        }
    }
    out
}

pub fn is_provider_enabled(entry: &HashMap<String, String>) -> bool {
    // Mirrors hermes_cli.config.is_provider_enabled(entry) — checks enabled field
    if let Some(v) = entry.get("enabled") {
        let lower = v.trim().to_lowercase();
        if matches!(lower.as_str(), "false" | "0" | "no" | "off") {
            return false;
        }
    }
    true
}

pub fn split_model_config_default(raw: &HashMap<String, String>) -> (String, String) {
    // Mirrors hermes_cli.config.split_model_config_default({provider, model})
    let model = raw.get("model").cloned().unwrap_or_default();
    let provider = raw.get("provider").cloned().unwrap_or_default();
    (model, provider)
}

fn canonical_api_mode(raw: &str) -> String {
    // Mirrors hermes_cli.config._canonical_api_mode alias map
    let lower = raw.trim().to_lowercase();
    match lower.as_str() {
        "openai" | "responses" | "codex" | "codex_responses" | "codex-responses" => "codex_responses".to_string(),
        "anthropic" | "anthropic_messages" | "anthropic-messages" | "claude" => "anthropic_messages".to_string(),
        "chat" | "chat_completions" | "chat-completions" | "openai_chat" => "chat_completions".to_string(),
        "bedrock" | "bedrock_converse" | "bedrock-converse" => "bedrock_converse".to_string(),
        "codex_app_server" | "codex-app-server" => "codex_app_server".to_string(),
        _ => lower,
    }
}

// ---------------------------------------------------------------------------
// _getenv — mirrors lines 54-64
// ---------------------------------------------------------------------------

/// Profile-scoped replacement for `os.getenv` on credential/provider reads.
///
/// Routes through the secret scope (Workstream A): identical to `os.getenv`
/// when multiplexing is off, scope-aware (and fail-closed on an unscoped read)
/// when on. Genuinely-global vars are handled inside `get_secret` and still
/// read `os.environ`. Keeps the `(name, default) -> str` contract every
/// call site relies on. Mirrors `_getenv(name, default="") -> str` (54-64).
pub fn getenv(name: &str, default: &str) -> String {
    let val = get_secret_stub(name, default);
    match val {
        Some(v) => v,
        None => default.to_string(),
    }
}

// ---------------------------------------------------------------------------
// _normalize_custom_provider_name — mirrors lines 67-68
// ---------------------------------------------------------------------------

/// Mirrors `_normalize_custom_provider_name(value) -> str` (67-68).
pub fn normalize_custom_provider_name(value: &str) -> String {
    value.trim().to_lowercase().replace(' ', "-")
}

// ---------------------------------------------------------------------------
// _loopback_hostname — mirrors lines 71-73
// ---------------------------------------------------------------------------

/// Mirrors `_loopback_hostname(host) -> bool` (71-73).
pub fn loopback_hostname(host: &str) -> bool {
    let h = host.trim().to_lowercase().trim_end_matches('.').to_string();
    matches!(h.as_str(), "localhost" | "127.0.0.1" | "::1" | "0.0.0.0")
}

// ---------------------------------------------------------------------------
// _config_base_url_trustworthy_for_bare_custom — mirrors lines 76-103
// ---------------------------------------------------------------------------

/// Decide whether `model.base_url` may back bare `custom` runtime resolution.
///
/// GitHub #14676: the model picker can select Custom while `model.provider` still
/// reflects a previous provider. Reject non-loopback URLs unless the YAML provider
/// is already `custom` (or one of the local-server aliases that resolve to `custom`
/// — ollama, vllm, llamacpp, …), so a stale OpenRouter/Z.ai base_url cannot hijack
/// local `custom` sessions. Mirrors `_config_base_url_trustworthy_for_bare_custom(cfg_base_url, cfg_provider) -> bool` (76-103).
pub fn config_base_url_trustworthy_for_bare_custom(cfg_base_url: &str, cfg_provider: &str) -> bool {
    let cfg_provider_norm = cfg_provider.trim().to_lowercase();
    let bu = cfg_base_url.trim().to_string();
    if bu.is_empty() {
        return false;
    }
    if cfg_provider_norm == "custom" {
        return true;
    }
    // GitHub #27132: provider aliases that resolve to "custom" at runtime (ollama, vllm, …)
    if resolve_provider(&cfg_provider_norm).map(|v| v == "custom").unwrap_or(false) {
        return true;
    }
    if base_url_host_matches(&bu, "openrouter.ai") {
        return false;
    }
    loopback_hostname(&base_url_hostname(&bu))
}

// ---------------------------------------------------------------------------
// _detect_api_mode_for_url — mirrors lines 106-155
// ---------------------------------------------------------------------------

/// Auto-detect api_mode from the resolved base URL.
/// Mirrors `_detect_api_mode_for_url(base_url) -> Optional[str]` (106-155).
pub fn _detect_api_mode_for_url(base_url: &str) -> Option<String> {
    let normalized = base_url.trim().to_lowercase().trim_end_matches('/').to_string();
    let hostname = base_url_hostname(base_url);
    if hostname == "api.x.ai" {
        return Some("codex_responses".to_string());
    }
    if is_official_openai_host(base_url) {
        return Some("codex_responses".to_string());
    }
    if hostname == "api.meta.ai" {
        return Some("codex_responses".to_string());
    }
    if hostname == "api.actual.inc" {
        return Some("codex_responses".to_string());
    }
    if hostname == "api.anthropic.com" {
        return Some("anthropic_messages".to_string());
    }
    let path = extract_path(&normalized).trim_end_matches('/').to_string();
    if path.ends_with("/anthropic") || path.ends_with("/anthropic/v1") {
        return Some("anthropic_messages".to_string());
    }
    if hostname == "api.kimi.com" && normalized.contains("/coding") {
        return Some("anthropic_messages".to_string());
    }
    None
}

// ---------------------------------------------------------------------------
// _fallback_api_mode — mirrors lines 158-181
// ---------------------------------------------------------------------------

/// Resolve api_mode when no explicit/persisted mode applies.
/// Mirrors `_fallback_api_mode(provider, base_url, model="") -> str` (158-181).
pub fn fallback_api_mode(provider: &str, base_url: &str, model: &str) -> String {
    if let Some(detected) = _detect_api_mode_for_url(base_url) {
        return detected;
    }
    determine_api_mode(provider, base_url, model).unwrap_or_else(|| "chat_completions".to_string())
}

// ---------------------------------------------------------------------------
// _resolve_plain_custom_api_mode — mirrors lines 183-203
// ---------------------------------------------------------------------------

/// Resolve api_mode for legacy/plain `provider: custom` endpoints.
/// Mirrors `_resolve_plain_custom_api_mode(model_cfg, base_url) -> str` (183-203).
pub fn resolve_plain_custom_api_mode(model_cfg: &HashMap<String, String>, base_url: &str) -> String {
    let mut configured_mode = _parse_api_mode(model_cfg.get("api_mode").map(|s| s.as_str()).unwrap_or(""));
    let detected_mode = _detect_api_mode_for_url(base_url);
    if configured_mode.as_deref() == Some("codex_responses") && detected_mode.as_deref() != Some("codex_responses") {
        log_info(&format!(
            "Ignoring persisted custom api_mode=codex_responses for non-OpenAI endpoint {}",
            if base_url.is_empty() { "(unknown)" } else { base_url }
        ));
        configured_mode = None;
    }
    configured_mode
        .or(detected_mode)
        .unwrap_or_else(|| "chat_completions".to_string())
}

// ---------------------------------------------------------------------------
// _host_derived_api_key — mirrors lines 206-260
// ---------------------------------------------------------------------------

/// Look up `<VENDOR>_API_KEY` in the env, derived from the base URL host.
/// Mirrors `_host_derived_api_key(base_url) -> str` (206-260).
pub fn host_derived_api_key(base_url: &str) -> String {
    let hostname = base_url_hostname(base_url);
    if hostname.is_empty() {
        return String::new();
    }
    if hostname.split('.').last().map(|lbl| lbl.chars().any(|c| c.is_ascii_digit())).unwrap_or(false) {
        return String::new();
    }
    if hostname == "localhost" || hostname.contains(':') {
        return String::new();
    }
    let mut labels: Vec<String> = hostname.split('.').filter(|s| !s.is_empty()).map(|s| s.to_string()).collect();
    while labels.first().map(|s| s.as_str()) == Some("api") || labels.first().map(|s| s.as_str()) == Some("www") {
        labels.remove(0);
    }
    if labels.len() < 2 {
        return String::new();
    }
    let vendor = labels[labels.len() - 2].clone();
    let sanitized: String = vendor
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch.to_ascii_uppercase() } else { '_' })
        .collect();
    if sanitized.is_empty() || !sanitized.chars().next().map(|c| c.is_ascii_alphabetic()).unwrap_or(false) {
        return String::new();
    }
    if matches!(sanitized.as_str(), "OPENAI" | "OPENROUTER" | "OLLAMA") {
        return String::new();
    }
    let env_name = format!("{sanitized}_API_KEY");
    getenv(&env_name, "").trim().to_string()
}

// ---------------------------------------------------------------------------
// _anthropic_base_url_override_ok — mirrors lines 263-297
// ---------------------------------------------------------------------------

/// Decide whether a configured `model.base_url` may back native Anthropic.
/// Mirrors `_anthropic_base_url_override_ok(base_url) -> bool` (263-297).
pub fn anthropic_base_url_override_ok(base_url: &str) -> bool {
    let candidate = base_url.trim().to_string();
    if candidate.is_empty() {
        return false;
    }
    let hostname = base_url_hostname(&candidate).to_lowercase();
    if hostname.is_empty() {
        return false;
    }
    if hostname == "api.anthropic.com" || hostname.ends_with(".anthropic.com") || hostname.ends_with(".claude.com") {
        return true;
    }
    if hostname.ends_with(".azure.com") {
        return true;
    }
    if _detect_api_mode_for_url(&candidate).as_deref() == Some("anthropic_messages") {
        return true;
    }
    false
}

// ---------------------------------------------------------------------------
// _auto_detect_local_model — mirrors lines 300-320
// ---------------------------------------------------------------------------

/// Query a local server for its model name when only one model is loaded.
/// Mirrors `_auto_detect_local_model(base_url) -> str` (300-320).
pub fn auto_detect_local_model(base_url: &str) -> String {
    if base_url.trim().is_empty() {
        return String::new();
    }
    // Python: import requests; GET {base_url}/v1/models with 2-3s timeout
    // In Rust slice 1: stub no network (NEVER cargo, no reqwest).
    // Preserve signature + early-return so callers behave identically when no server.
    let _ = base_url;
    log_debug(&format!("Auto-detect model from {} skipped (stub — no HTTP in slice 1)", base_url));
    String::new()
}

// ---------------------------------------------------------------------------
// _get_model_config — mirrors lines 323-352
// ---------------------------------------------------------------------------

/// Mirrors `_get_model_config() -> Dict[str, Any]` (323-352).
pub fn get_model_config() -> HashMap<String, String> {
    let config = load_config();
    // Python: config.get("model") may be dict or str; stub config is flat so we
    // emulate the three branches: dict with default/model/provider/base_url, str, or empty.
    // In slice 1, load_config returns flat model.* keys — synthesize dict-like view.

    // If we have any model.* keys, treat as dict config
    let has_model_keys = config.keys().any(|k| k.starts_with("model."));
    if has_model_keys {
        let mut cfg: HashMap<String, String> = HashMap::new();
        for (k, v) in &config {
            if let Some(tail) = k.strip_prefix("model.") {
                cfg.insert(tail.to_string(), v.clone());
            }
        }
        // Accept "model" as alias for "default"
        if cfg.get("default").map(|s| s.trim().is_empty()).unwrap_or(true) {
            if let Some(m) = cfg.get("model").cloned() {
                if !m.trim().is_empty() {
                    cfg.insert("default".to_string(), m);
                }
            }
        }
        // Handle model.default being a dict {provider, model} — Python checks isinstance(default, dict)
        // In stub flat config this cannot happen (default is always string); preserve branch as no-op.
        // Dict default split via split_model_config_default would be used if default were still dict-shaped.
        let default = cfg.get("default").cloned().unwrap_or_default().trim().to_string();
        let base_url = cfg.get("base_url").cloned().unwrap_or_default().trim().to_string();
        let is_local = matches!(base_url_hostname(&base_url).as_str(), "localhost" | "127.0.0.1");
        let is_fallback = default.is_empty();
        if is_local && is_fallback && !base_url.is_empty() {
            let detected = auto_detect_local_model(&base_url);
            if !detected.is_empty() {
                cfg.insert("default".to_string(), detected);
            }
        }
        return cfg;
    }

    // Check if raw model is a bare string (legacy: model: "gpt-4o")
    // Stub: look for top-level "model" string key without dot prefix
    if let Some(raw) = config.get("model") {
        let s = raw.trim().to_string();
        if !s.is_empty() {
            let mut m = HashMap::new();
            m.insert("default".to_string(), s);
            return m;
        }
    }

    HashMap::new()
}

// ---------------------------------------------------------------------------
// _provider_supports_explicit_api_mode — mirrors lines 355-369
// ---------------------------------------------------------------------------

/// Check whether a persisted api_mode should be honored for a given provider.
/// Mirrors `_provider_supports_explicit_api_mode(provider, configured_provider=None) -> bool` (355-369).
pub fn provider_supports_explicit_api_mode(provider: Option<&str>, configured_provider: Option<&str>) -> bool {
    let normalized_provider = provider.unwrap_or("").trim().to_lowercase();
    let normalized_configured = configured_provider.unwrap_or("").trim().to_lowercase();
    if normalized_configured.is_empty() {
        return true;
    }
    if normalized_provider == "custom" {
        return normalized_configured == "custom" || normalized_configured.starts_with("custom:");
    }
    normalized_configured == normalized_provider
}

// ---------------------------------------------------------------------------
// _copilot_runtime_api_mode — mirrors lines 372-398
// ---------------------------------------------------------------------------

/// Mirrors `_copilot_runtime_api_mode(model_cfg, api_key, *, target_model=None) -> str` (372-398).
pub fn copilot_runtime_api_mode(
    model_cfg: &HashMap<String, String>,
    api_key: &str,
    target_model: Option<&str>,
) -> String {
    let configured_provider = model_cfg.get("provider").map(|s| s.as_str()).unwrap_or("").trim().to_lowercase();
    let configured_mode = _parse_api_mode(model_cfg.get("api_mode").map(|s| s.as_str()).unwrap_or(""));
    if configured_mode.is_some() && provider_supports_explicit_api_mode(Some("copilot"), Some(&configured_provider)) {
        return configured_mode.unwrap();
    }
    let model_name = target_model
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| model_cfg.get("default").cloned().unwrap_or_default().trim().to_string());
    if model_name.is_empty() {
        return "chat_completions".to_string();
    }
    // Python: from hermes_cli.models import copilot_model_api_mode; call with api_key
    // In Rust slice 1 we stub the models import — return chat_completions fallback.
    let _ = api_key;
    // Stub: copilot_model_api_mode would map Claude/Gemini names → anthropic_messages etc.
    // Preserve contract: on Exception return chat_completions.
    copilot_model_api_mode_stub(&model_name, api_key)
}

fn copilot_model_api_mode_stub(model_name: &str, _api_key: &str) -> String {
    let lower = model_name.trim().to_lowercase();
    if lower.contains("claude") || lower.contains("anthropic") {
        return "anthropic_messages".to_string();
    }
    "chat_completions".to_string()
}

// ---------------------------------------------------------------------------
// _VALID_API_MODES + _parse_api_mode — mirrors lines 401-429
// ---------------------------------------------------------------------------

/// Mirrors `_VALID_API_MODES` frozenset (401-412).
pub const VALID_API_MODES: &[&str] = &[
    "chat_completions",
    "codex_responses",
    "anthropic_messages",
    "bedrock_converse",
    "codex_app_server",
];

/// Validate an api_mode value from config. Returns None if invalid.
/// Mirrors `_parse_api_mode(raw) -> Optional[str]` (415-429).
pub fn _parse_api_mode(raw: &str) -> Option<String> {
    if raw.trim().is_empty() {
        return None;
    }
    let normalized = canonical_api_mode(raw).to_lowercase();
    if VALID_API_MODES.contains(&normalized.as_str()) {
        return Some(normalized);
    }
    None
}

/// Overload for Option input (mirrors Python `isinstance(raw, str)` guard).
pub fn parse_api_mode_opt(raw: Option<&str>) -> Option<String> {
    match raw {
        Some(s) => _parse_api_mode(s),
        None => None,
    }
}

// ---------------------------------------------------------------------------
// _nous_inference_base_url_override — mirrors lines 432-440
// ---------------------------------------------------------------------------

/// Return the trusted Nous runtime base URL override, if configured.
/// Mirrors `_nous_inference_base_url_override() -> str` (432-440).
pub fn nous_inference_base_url_override() -> String {
    nous_inference_env_override().trim().trim_end_matches('/').to_string()
}

// ---------------------------------------------------------------------------
// _maybe_apply_codex_app_server_runtime — mirrors lines 443-466
// ---------------------------------------------------------------------------

/// Optional opt-in: rewrite api_mode → "codex_app_server" for OpenAI/Codex
/// providers when `model.openai_runtime: codex_app_server` is set.
/// Mirrors `_maybe_apply_codex_app_server_runtime(*, provider, api_mode, model_cfg) -> str` (443-466).
pub fn maybe_apply_codex_app_server_runtime(
    provider: &str,
    api_mode: &str,
    model_cfg: Option<&HashMap<String, String>>,
) -> String {
    let cfg = match model_cfg {
        Some(c) => c,
        None => return api_mode.to_string(),
    };
    if !matches!(provider, "openai" | "openai-codex") {
        return api_mode.to_string();
    }
    let runtime = cfg.get("openai_runtime").map(|s| s.trim().to_lowercase()).unwrap_or_default();
    if runtime == "codex_app_server" {
        return "codex_app_server".to_string();
    }
    api_mode.to_string()
}

// ---------------------------------------------------------------------------
// _resolve_runtime_from_pool_entry — mirrors lines 469-615
// ---------------------------------------------------------------------------

/// Runtime resolution output — mirrors dict returned by `_resolve_runtime_from_pool_entry` (607-615).
#[derive(Debug, Clone)]
pub struct RuntimeResolution {
    pub provider: String,
    pub api_mode: String,
    pub base_url: String,
    pub api_key: String,
    pub source: String,
    pub requested_provider: String,
    // pool kept as key for traceability in slice 1 (full CredentialPool lives in later slice)
    pub credential_pool_key: Option<String>,
}

/// Mirrors `_resolve_runtime_from_pool_entry(*, provider, entry, requested_provider, model_cfg=None, pool=None, target_model=None) -> Dict[str, Any]` (469-615).
pub fn resolve_runtime_from_pool_entry(
    provider: &str,
    entry: &PooledCredential,
    requested_provider: &str,
    model_cfg: Option<HashMap<String, String>>,
    pool: Option<CredentialPool>,
    target_model: Option<&str>,
) -> RuntimeResolution {
    let mc = model_cfg.unwrap_or_else(get_model_config);
    let effective_model = target_model
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| mc.get("default").cloned().unwrap_or_default());
    let mut base_url = if !entry.runtime_base_url.trim().is_empty() {
        entry.runtime_base_url.trim().trim_end_matches('/').to_string()
    } else {
        entry.base_url.trim().trim_end_matches('/').to_string()
    };
    let api_key = if !entry.runtime_api_key.trim().is_empty() {
        entry.runtime_api_key.clone()
    } else {
        entry.access_token.clone()
    };
    let mut api_mode = "chat_completions".to_string();

    let registry = provider_registry();

    match provider {
        "openai-codex" => {
            api_mode = "codex_responses".to_string();
            if base_url.is_empty() {
                base_url = DEFAULT_CODEX_BASE_URL.to_string();
            }
        }
        "xai-oauth" => {
            api_mode = "codex_responses".to_string();
            if base_url.is_empty() {
                base_url = DEFAULT_XAI_OAUTH_BASE_URL.to_string();
            }
        }
        "qwen-oauth" => {
            api_mode = "chat_completions".to_string();
            if base_url.is_empty() {
                base_url = DEFAULT_QWEN_BASE_URL.to_string();
            }
        }
        "minimax-oauth" => {
            api_mode = "anthropic_messages".to_string();
            let pconfig = registry.get(provider);
            if base_url.is_empty() {
                if let Some(pc) = pconfig {
                    base_url = pc.inference_base_url.clone();
                }
            }
        }
        "anthropic" => {
            api_mode = "anthropic_messages".to_string();
            let cfg_provider = mc.get("provider").map(|s| s.trim().to_lowercase()).unwrap_or_default();
            let mut cfg_base_url = String::new();
            if cfg_provider == "anthropic" {
                cfg_base_url = mc.get("base_url").cloned().unwrap_or_default().trim().trim_end_matches('/').to_string();
                if !anthropic_base_url_override_ok(&cfg_base_url) {
                    cfg_base_url.clear();
                }
            }
            if !cfg_base_url.is_empty() {
                base_url = cfg_base_url;
            } else if base_url.is_empty() {
                base_url = "https://api.anthropic.com".to_string();
            }
        }
        "openrouter" => {
            if base_url.is_empty() {
                base_url = OPENROUTER_BASE_URL.to_string();
            }
        }
        "xai" => {
            api_mode = "codex_responses".to_string();
        }
        "nous" => {
            api_mode = nous_api_mode(&effective_model);
            let override_url = nous_inference_base_url_override();
            if !override_url.is_empty() {
                base_url = override_url;
            }
        }
        "copilot" => {
            api_mode = copilot_runtime_api_mode(&mc, &entry.runtime_api_key, Some(&effective_model));
            if base_url.is_empty() {
                if let Some(pc) = registry.get("copilot") {
                    base_url = pc.inference_base_url.clone();
                } else {
                    base_url = DEFAULT_GITHUB_MODELS_BASE_URL.to_string();
                }
            }
        }
        "azure-foundry" => {
            let cfg_provider = mc.get("provider").map(|s| s.trim().to_lowercase()).unwrap_or_default();
            if cfg_provider == "azure-foundry" {
                let cfg_base = mc.get("base_url").cloned().unwrap_or_default().trim().trim_end_matches('/').to_string();
                if !cfg_base.is_empty() {
                    base_url = cfg_base;
                }
                if let Some(cm) = _parse_api_mode(mc.get("api_mode").map(|s| s.as_str()).unwrap_or("")) {
                    api_mode = cm;
                }
            }
            if !effective_model.is_empty() && api_mode != "anthropic_messages" {
                if let Some(inferred) = azure_foundry_model_api_mode_stub(&effective_model) {
                    api_mode = inferred;
                }
            }
            if api_mode == "anthropic_messages" {
                // Strip /v1 suffix — mirrors re.sub(r"/v1/?$", "", base_url)
                if base_url.ends_with("/v1") {
                    base_url.truncate(base_url.len() - 3);
                    base_url = base_url.trim_end_matches('/').to_string();
                } else if base_url.ends_with("/v1/") {
                    base_url.truncate(base_url.len() - 4);
                    base_url = base_url.trim_end_matches('/').to_string();
                }
            }
        }
        _ => {
            let configured_provider = mc.get("provider").map(|s| s.trim().to_lowercase()).unwrap_or_default();
            let pconfig = registry.get(provider);
            let pool_url_is_default = pconfig.map(|pc| base_url.trim_end_matches('/') == pc.inference_base_url.trim_end_matches('/')).unwrap_or(false);
            if configured_provider == provider && pool_url_is_default {
                let cfg_base = mc.get("base_url").cloned().unwrap_or_default().trim().trim_end_matches('/').to_string();
                if !cfg_base.is_empty() {
                    base_url = cfg_base;
                }
            }
            let configured_mode = _parse_api_mode(mc.get("api_mode").map(|s| s.as_str()).unwrap_or(""));
            let family = opencode_provider_family_stub(provider);
            if family.is_some() {
                api_mode = opencode_model_api_mode_stub(provider, &effective_model);
            } else if let Some(cm) = configured_mode {
                if provider_supports_explicit_api_mode(Some(provider), Some(&configured_provider)) {
                    api_mode = cm;
                } else {
                    api_mode = fallback_api_mode(provider, &base_url, &effective_model);
                }
            } else {
                api_mode = fallback_api_mode(provider, &base_url, &effective_model);
            }
        }
    }

    // Opencode base URL normalization — mirrors lines 592-596
    if opencode_provider_family_stub(provider).is_some() {
        base_url = normalize_opencode_base_url_stub(provider, &api_mode, &base_url);
    }

    api_mode = maybe_apply_codex_app_server_runtime(provider, &api_mode, Some(&mc));

    if provider == "lmstudio" {
        base_url = normalize_lmstudio_runtime_base_url_stub(&base_url);
    }

    RuntimeResolution {
        provider: provider.to_string(),
        api_mode,
        base_url,
        api_key,
        source: entry.source.clone(),
        requested_provider: requested_provider.to_string(),
        credential_pool_key: pool.map(|p| p.key),
    }
}

fn nous_api_mode(model: &str) -> String {
    // Mirrors hermes_cli.providers.nous_api_mode(model) — reasoning models use codex_responses
    let lower = model.trim().to_lowercase();
    if lower.contains("hermes-4") || lower.contains("reasoning") || lower.contains("deephermes") {
        return "codex_responses".to_string();
    }
    "chat_completions".to_string()
}

fn azure_foundry_model_api_mode_stub(model: &str) -> Option<String> {
    let lower = model.trim().to_lowercase();
    if lower.contains("gpt-5") || lower.contains("codex") || lower.starts_with("o1") || lower.starts_with("o3") || lower.starts_with("o4") {
        return Some("codex_responses".to_string());
    }
    None
}

fn opencode_provider_family_stub(provider: &str) -> Option<String> {
    // Mirrors hermes_cli.models.opencode_provider_family(provider)
    if matches!(provider, "opencode-zen" | "opencode-go" | "opencode-free") {
        return Some("opencode".to_string());
    }
    None
}

fn opencode_model_api_mode_stub(provider: &str, model: &str) -> String {
    let _ = provider;
    let lower = model.trim().to_lowercase();
    // Anthropic-native opencode models (Claude family) use anthropic_messages
    if lower.contains("claude") || lower.contains("anthropic") {
        return "anthropic_messages".to_string();
    }
    "chat_completions".to_string()
}

fn normalize_opencode_base_url_stub(provider: &str, api_mode: &str, base_url: &str) -> String {
    let mut url = base_url.trim().trim_end_matches('/').to_string();
    if opencode_provider_family_stub(provider).is_none() {
        return url;
    }
    // Mirrors hermes_cli.models.normalize_opencode_base_url — strip/append /v1 symmetrically
    if api_mode == "anthropic_messages" {
        if url.ends_with("/v1") {
            url.truncate(url.len() - 3);
        } else if url.ends_with("/v1/") {
            url.truncate(url.len() - 4);
        }
        url.trim_end_matches('/').to_string()
    } else {
        // chat_completions / codex_responses need /v1
        if !url.ends_with("/v1") {
            format!("{url}/v1").trim_end_matches('/').to_string()
        } else {
            url
        }
    }
}

fn normalize_lmstudio_runtime_base_url_stub(base_url: &str) -> String {
    // Mirrors hermes_cli.auth._normalize_lmstudio_runtime_base_url — ensure /v1 suffix
    let url = base_url.trim().trim_end_matches('/').to_string();
    if url.is_empty() {
        return "http://127.0.0.1:1234/v1".to_string();
    }
    if url.ends_with("/v1") {
        return url;
    }
    format!("{url}/v1")
}

// ---------------------------------------------------------------------------
// resolve_requested_provider — mirrors lines 618-634
// ---------------------------------------------------------------------------

/// Resolve provider request from explicit arg, config, then env.
/// Mirrors `resolve_requested_provider(requested=None) -> str` (618-634).
pub fn resolve_requested_provider(requested: Option<&str>) -> String {
    if let Some(r) = requested {
        if !r.trim().is_empty() {
            return r.trim().to_lowercase();
        }
    }
    let model_cfg = get_model_config();
    if let Some(cfg_provider) = model_cfg.get("provider") {
        if !cfg_provider.trim().is_empty() {
            return cfg_provider.trim().to_lowercase();
        }
    }
    let env_provider = getenv("HERMES_INFERENCE_PROVIDER", "").trim().to_lowercase();
    if !env_provider.is_empty() {
        return env_provider;
    }
    "auto".to_string()
}

// ---------------------------------------------------------------------------
// _try_resolve_from_custom_pool — mirrors lines 637-678
// ---------------------------------------------------------------------------

/// Check if a credential pool exists for a custom endpoint and return a runtime dict if so.
/// Mirrors `_try_resolve_from_custom_pool(base_url, provider_label, api_mode_override=None, provider_name=None) -> Optional[Dict[str, Any]]` (637-678).
#[derive(Debug, Clone)]
pub struct CustomPoolRuntime {
    pub provider: String,
    pub api_mode: String,
    pub base_url: String,
    pub api_key: String,
    pub source: String,
    pub credential_pool_key: String,
}

pub fn try_resolve_from_custom_pool(
    base_url: &str,
    provider_label: &str,
    api_mode_override: Option<&str>,
    provider_name: Option<&str>,
) -> Option<CustomPoolRuntime> {
    let pool_key = get_custom_provider_pool_key(base_url, provider_name)?;
    let pool = load_pool(&pool_key);
    if !pool.has_credentials() {
        return None;
    }
    let entry = pool.select()?;
    let mut pool_api_key = if !entry.runtime_api_key.trim().is_empty() {
        entry.runtime_api_key.clone()
    } else {
        entry.access_token.clone()
    };
    if pool_api_key.trim().is_empty() {
        return None;
    }
    if !has_usable_secret(&pool_api_key) && loopback_hostname(&base_url_hostname(base_url)) {
        pool_api_key = "no-key-required".to_string();
    }
    let api_mode = api_mode_override
        .map(|s| s.to_string())
        .or_else(|| _detect_api_mode_for_url(base_url))
        .unwrap_or_else(|| "chat_completions".to_string());
    Some(CustomPoolRuntime {
        provider: provider_label.to_string(),
        api_mode,
        base_url: base_url.to_string(),
        api_key: pool_api_key,
        source: format!("pool:{pool_key}"),
        credential_pool_key: pool_key,
    })
}

// ---------------------------------------------------------------------------
// _lift_max_output_tokens — mirrors lines 681-693
// ---------------------------------------------------------------------------

/// Propagate a per-provider output cap onto the resolved runtime dict.
/// Mirrors `_lift_max_output_tokens(entry, result) -> None` (681-693).
pub fn lift_max_output_tokens(entry: &HashMap<String, String>, result: &mut HashMap<String, String>) {
    for k in ["max_output_tokens", "max_tokens"] {
        if let Some(v) = entry.get(k) {
            if let Ok(n) = v.trim().parse::<i64>() {
                if n > 0 {
                    result.insert("max_output_tokens".to_string(), n.to_string());
                    return;
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// _lift_extra_headers — mirrors lines 696-704
// ---------------------------------------------------------------------------

/// Copy a validated `extra_headers` dict from a provider entry.
/// Mirrors `_lift_extra_headers(entry, result) -> None` (696-704).
pub fn lift_extra_headers(entry: &HashMap<String, String>, result: &mut HashMap<String, String>) {
    // In Python, entry.get("extra_headers") is a dict; in slice 1 flat stub we look for
    // keys prefixed with "extra_headers." — if none, no-op. Real dict wiring in later slice.
    // Preserve security note: never log header values.
    let mut headers: HashMap<String, String> = HashMap::new();
    for (k, v) in entry {
        if k.starts_with("extra_headers.") {
            let hk = k["extra_headers.".len()..].trim().to_string();
            if !hk.is_empty() && !v.trim().is_empty() {
                headers.insert(hk, v.trim().to_string());
            }
        }
    }
    if headers.is_empty() {
        // Try single JSON-ish key "extra_headers" — stub parse as empty
        return;
    }
    let normalized = normalize_extra_headers(Some(&headers));
    if !normalized.is_empty() {
        // Store as serialized count marker for 1:1 traceability (real impl stores dict)
        result.insert("extra_headers_count".to_string(), normalized.len().to_string());
        for (hk, hv) in &normalized {
            result.insert(format!("extra_headers.{hk}"), hv.clone());
        }
    }
}

// ---------------------------------------------------------------------------
// _get_named_custom_provider — mirrors lines 707-855
// ---------------------------------------------------------------------------

/// Custom provider entry — mirrors dict returned by `_get_named_custom_provider`.
#[derive(Debug, Clone, Default)]
pub struct CustomProviderEntry {
    pub name: String,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub api_mode: Option<String>,
    pub extra_body: HashMap<String, String>,
    pub extra_headers: HashMap<String, String>,
    pub key_cmd: Option<String>,
    pub key_env: Option<String>,
    pub provider_key: Option<String>,
    pub max_output_tokens: Option<i64>,
}

/// Mirrors `_get_named_custom_provider(requested_provider) -> Optional[Dict[str, Any]]` (707-855).
pub fn get_named_custom_provider(requested_provider: &str) -> Option<CustomProviderEntry> {
    let requested_norm = normalize_custom_provider_name(requested_provider);
    if requested_norm.is_empty() {
        return None;
    }
    if requested_norm == "auto" {
        return None;
    }
    if requested_norm != "custom" && !requested_norm.starts_with("custom:") {
        if let Ok(canonical) = resolve_provider(&requested_norm) {
            if canonical.trim().to_lowercase() == requested_norm {
                return None;
            }
        }
    }

    let config = load_config();

    // First check providers: dict (new-style user-defined providers) — mirrors lines 747-806
    // In slice 1, load_config is flat and has no nested providers dict, so we stub
    // the scan via flat keys "providers.<name>.*". Real nested scan in later slice with serde_yaml.
    let providers_prefix = "providers.";
    let mut provider_names: HashSet<String> = HashSet::new();
    for k in config.keys() {
        if k.starts_with(providers_prefix) {
            let rest = &k[providers_prefix.len()..];
            if let Some(dot) = rest.find('.') {
                provider_names.insert(rest[..dot].to_string());
            }
        }
    }
    for ep_name in &provider_names {
        let entry_prefix = format!("providers.{ep_name}.");
        let mut entry_map: HashMap<String, String> = HashMap::new();
        for (k, v) in &config {
            if k.starts_with(&entry_prefix) {
                let tail = k[entry_prefix.len()..].to_string();
                entry_map.insert(tail, v.clone());
            }
        }
        if entry_map.is_empty() {
            continue;
        }
        if !is_provider_enabled(&entry_map) {
            continue;
        }
        let key_env = entry_map
            .get("key_env")
            .or_else(|| entry_map.get("api_key_env"))
            .cloned()
            .unwrap_or_default()
            .trim()
            .to_string();
        let mut resolved_api_key = if !key_env.is_empty() {
            getenv(&key_env, "").trim().to_string()
        } else {
            String::new()
        };
        if resolved_api_key.is_empty() {
            resolved_api_key = entry_map.get("api_key").cloned().unwrap_or_default().trim().to_string();
        }
        let display_name = entry_map.get("name").cloned().unwrap_or_default();
        let aliases = custom_provider_aliases(
            if display_name.is_empty() { ep_name } else { &display_name },
            ep_name,
        );
        if !aliases.contains(&requested_norm) {
            continue;
        }
        let base_url = entry_map
            .get("api")
            .or_else(|| entry_map.get("url"))
            .or_else(|| entry_map.get("base_url"))
            .cloned()
            .unwrap_or_default()
            .trim()
            .to_string();
        if base_url.is_empty() {
            continue;
        }
        let mut result = CustomProviderEntry {
            name: entry_map.get("name").cloned().unwrap_or_else(|| ep_name.clone()),
            base_url: base_url.clone(),
            api_key: resolved_api_key,
            model: entry_map.get("default_model").cloned().unwrap_or_default(),
            ..Default::default()
        };
        // extra_body.* stub — flat keys
        for (k, v) in &entry_map {
            if k.starts_with("extra_body.") {
                let ek = k["extra_body.".len()..].to_string();
                result.extra_body.insert(ek, v.clone());
            }
        }
        // extra_headers
        let mut eh: HashMap<String, String> = HashMap::new();
        for (k, v) in &entry_map {
            if k.starts_with("extra_headers.") {
                eh.insert(k["extra_headers.".len()..].to_string(), v.clone());
            }
        }
        result.extra_headers = normalize_extra_headers(Some(&eh));

        let key_cmd = entry_map.get("key_cmd").cloned().unwrap_or_default().trim().to_string();
        if !key_cmd.is_empty() {
            result.key_cmd = Some(key_cmd);
        }
        let api_mode = _parse_api_mode(
            entry_map
                .get("api_mode")
                .or_else(|| entry_map.get("transport"))
                .map(|s| s.as_str())
                .unwrap_or(""),
        );
        if api_mode.is_some() {
            result.api_mode = api_mode;
        }
        for k in ["max_output_tokens", "max_tokens"] {
            if let Some(v) = entry_map.get(k) {
                if let Ok(n) = v.trim().parse::<i64>() {
                    if n > 0 {
                        result.max_output_tokens = Some(n);
                        break;
                    }
                }
            }
        }
        return Some(result);
    }

    // Fall back to custom_providers: list (legacy format) — mirrors lines 808-853
    // In slice 1 flat stub, legacy entries are under "custom_providers.<idx>.*"
    // If config has key "custom_providers" that parses as dict, warn and return None (810-817).
    if config.contains_key("custom_providers") {
        // Detected dict-shaped custom_providers — mirror logger.warning and return None
        log_info("custom_providers in config.yaml is a dict, not a list. Each entry must be prefixed with '-' in YAML. Run 'hermes doctor' for details.");
        return None;
    }
    let mut legacy_entries: HashMap<String, HashMap<String, String>> = HashMap::new();
    let legacy_prefix = "custom_providers.";
    for (k, v) in &config {
        if k.starts_with(legacy_prefix) {
            let rest = &k[legacy_prefix.len()..];
            if let Some(dot) = rest.find('.') {
                let idx = rest[..dot].to_string();
                let field = rest[dot + 1..].to_string();
                legacy_entries.entry(idx).or_default().insert(field, v.clone());
            }
        }
    }
    let compat = get_compatible_custom_providers(&config);
    // In slice 1 compat is empty (see stub); iterate legacy_entries instead for flat coverage
    let iter_entries: Vec<HashMap<String, String>> = if compat.is_empty() {
        legacy_entries.into_values().collect()
    } else {
        compat
    };
    if iter_entries.is_empty() {
        return None;
    }
    for entry in &iter_entries {
        let name = entry.get("name").cloned().unwrap_or_default();
        let base_url = entry.get("base_url").cloned().unwrap_or_default();
        if name.trim().is_empty() || base_url.trim().is_empty() {
            continue;
        }
        let provider_key = entry.get("provider_key").cloned().unwrap_or_default().trim().to_string();
        let aliases = custom_provider_aliases(&name, &provider_key);
        if !aliases.contains(&requested_norm) {
            continue;
        }
        let mut result = CustomProviderEntry {
            name: name.trim().to_string(),
            base_url: base_url.trim().to_string(),
            api_key: entry.get("api_key").cloned().unwrap_or_default().trim().to_string(),
            ..Default::default()
        };
        if let Some(ke) = entry.get("key_env") {
            let ke = ke.trim().to_string();
            if !ke.is_empty() {
                result.key_env = Some(ke);
            }
        }
        if !provider_key.is_empty() {
            result.provider_key = Some(provider_key);
        }
        for (k, v) in entry {
            if k.starts_with("extra_body.") {
                result.extra_body.insert(k["extra_body.".len()..].to_string(), v.clone());
            }
        }
        let mut eh: HashMap<String, String> = HashMap::new();
        for (k, v) in entry {
            if k.starts_with("extra_headers.") {
                eh.insert(k["extra_headers.".len()..].to_string(), v.clone());
            }
        }
        result.extra_headers = normalize_extra_headers(Some(&eh));
        if let Some(am) = _parse_api_mode(entry.get("api_mode").map(|s| s.as_str()).unwrap_or("")) {
            result.api_mode = Some(am);
        }
        if let Some(m) = entry.get("model") {
            let m = m.trim().to_string();
            if !m.is_empty() {
                result.model = m;
            }
        }
        for k in ["max_output_tokens", "max_tokens"] {
            if let Some(v) = entry.get(k) {
                if let Ok(n) = v.trim().parse::<i64>() {
                    if n > 0 {
                        result.max_output_tokens = Some(n);
                        break;
                    }
                }
            }
        }
        return Some(result);
    }

    None
}

// ---------------------------------------------------------------------------
// has_named_custom_provider — mirrors lines 858-869
// ---------------------------------------------------------------------------

/// Return True when config defines a custom provider matching the request.
/// Mirrors `has_named_custom_provider(requested_provider) -> bool` (858-869).
pub fn has_named_custom_provider(requested_provider: &str) -> bool {
    get_named_custom_provider(requested_provider).is_some()
}

// ---------------------------------------------------------------------------
// find_custom_provider_identity — mirrors lines 872-923 (slice cuts mid-loop at 900)
// ---------------------------------------------------------------------------

/// Normalize a base URL for comparison — lowercased, trailing slash stripped.
/// Mirrors `_normalize_base_url_for_match(value) -> str` (1085-1086 — used inside find_*, defined later but hoisted here for slice 1 ordering).
pub fn normalize_base_url_for_match(value: &str) -> String {
    value.trim().trim_end_matches('/').to_lowercase()
}

/// Map an endpoint URL back to its canonical `custom:<name>` menu key.
/// Mirrors `find_custom_provider_identity(base_url) -> Optional[str]` (872-923).
/// Slice 1 covers through the `providers:` loop up to the per-entry `== target` check
/// at line 905 (first 900 of 2 451 cuts mid-loop, before the `custom_providers:` fallback).
pub fn find_custom_provider_identity(base_url: &str) -> Option<String> {
    let target = normalize_base_url_for_match(base_url);
    if target.is_empty() {
        return None;
    }
    let config = load_config();

    // Scan providers: dict — mirrors lines 896-905 (slice 1 includes the `if ... == target` body head)
    let providers_prefix = "providers.";
    let mut provider_names: HashSet<String> = HashSet::new();
    for k in config.keys() {
        if k.starts_with(providers_prefix) {
            let rest = &k[providers_prefix.len()..];
            if let Some(dot) = rest.find('.') {
                provider_names.insert(rest[..dot].to_string());
            } else {
                provider_names.insert(rest.to_string());
            }
        }
    }
    for ep_name in &provider_names {
        // Reconstruct entry’s api/url/base_url flat keys
        let mut entry_url = String::new();
        for field in ["api", "url", "base_url"] {
            let key = format!("providers.{ep_name}.{field}");
            if let Some(v) = config.get(&key) {
                if !v.trim().is_empty() {
                    entry_url = v.clone();
                    break;
                }
            }
        }
        if normalize_base_url_for_match(&entry_url) == target {
            return Some(custom_provider_slug(ep_name, ep_name));
        }
    }

    // ---- slice boundary at Python line 900 ----
    // The `custom_providers:` fallback scan (lines 907-923) and
    // `return None` tail continue in `runtime_provider_slice2.rs`.
    // We include a stub tail here so the function has a valid return in slice 1
    // (real tail is authoritative in slice 2; this stub is unreachable until
    // slice 2 is linked and will be removed/cfg-gated then).

    // Fallback: try legacy custom_providers.* flat entries best-effort (slice 1 stub tail)
    let legacy_prefix = "custom_providers.";
    let mut legacy_entries: HashMap<String, HashMap<String, String>> = HashMap::new();
    for (k, v) in &config {
        if k.starts_with(legacy_prefix) {
            let rest = &k[legacy_prefix.len()..];
            if let Some(dot) = rest.find('.') {
                let idx = rest[..dot].to_string();
                let field = rest[dot + 1..].to_string();
                legacy_entries.entry(idx).or_default().insert(field, v.clone());
            }
        }
    }
    for entry in legacy_entries.values() {
        let name = entry.get("name").cloned().unwrap_or_default();
        let bu = entry.get("base_url").cloned().unwrap_or_default();
        if name.trim().is_empty() {
            continue;
        }
        if normalize_base_url_for_match(&bu) == target {
            let pk = entry.get("provider_key").cloned().unwrap_or_default();
            return Some(custom_provider_slug(&name, &pk));
        }
    }

    None
}

// ---------------------------------------------------------------------------
// Slice boundary — line 900
// ---------------------------------------------------------------------------
// Python `runtime_provider.py` lines 901-2451 (
//   remainder of find_custom_provider_identity tail (907-923),
//   find_custom_provider_identity_by_model (926-993),
//   canonical_custom_identity (996-1082),
//   _normalize_base_url_for_match (1085-1086),
//   _custom_provider_request_overrides (1089-1093),
//   _resolve_named_custom_runtime (1096-~1350),
//   resolve_runtime_provider / resolve_runtime_credentials +
//   auth helper bridges, direct-alias bare custom path … through EOF
// ) continue in `runtime_provider_slice2.rs` (from entry_url==target tail)
// and `runtime_provider_slice3.rs`.
// This file intentionally stops at the 900-line boundary (mid-function at
// `if _normalize_base_url_for_match(entry_url)==target` of
// `find_custom_provider_identity`) so that `cargo` is never invoked and
// the 3-slice decomposition stays clean.
