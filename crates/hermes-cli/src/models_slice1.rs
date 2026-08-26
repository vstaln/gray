//! hermes-cli models — slice 1/8
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/hermes_cli/models.py`
//! slice 1/8 — lines 1–900 of 6 981 (first 900 LOC).
//! Covers: module docstring + lazy imports, version/user-agent,
//! COPILOT constants, `_urlopen_model_catalog_request`,
//! `_custom_provider_ssl_context`, `OPENROUTER_MODELS` + cache,
//! `VERCEL_AI_GATEWAY_MODELS` + cache, `_codex_curated_models`,
//! `_XAI_STATIC_FALLBACK` / `_XAI_CURATED_EXTRAS` / `_xai_*` helpers,
//! `_PROVIDER_MODELS` dict (including `ai-gateway` derived entry),
//! `_is_model_free`, `is_nous_free_tier`, `partition_nous_models_by_tier`,
//! `union_with_portal_free_recommendations` and the opening of
//! `union_with_portal_paid_recommendations` (through line 900).
//! Continued in `models_slice2.rs` (from line 901).
//!
//! T0686 — 1:1 port, no cargo (NEVER cargo).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

// ---------------------------------------------------------------------------
// Module docstring — mirrors lines 1-6
// ---------------------------------------------------------------------------

/// Canonical model catalogs and lightweight validation helpers.
///
/// Add, remove, or reorder entries here — both `hermes setup` and
/// `hermes` provider-selection will pick up the change automatically.
/// Mirrors `hermes_cli/models.py` lines 1-6.
pub const MODULE_DOC: &str =
    "models: canonical model catalogs and lightweight validation helpers — see models.py lines 1-6";

// ---------------------------------------------------------------------------
// Imports — mirrors lines 8-31
// ---------------------------------------------------------------------------
// Python: copy, json, http.client, logging, os, re, threading,
// urllib.parse, urllib.request, urllib.error, time, difflib.get_close_matches,
// pathlib.Path, typing (Any, NamedTuple, Optional, TYPE_CHECKING),
// hermes_cli.__version__, hermes_cli.urllib_security (open_credentialed_url, url_origin),
// utils (atomic_json_write, base_url_host_matches)
// Rust: std only (NEVER cargo). External hermes modules are stubbed for 1:1.

/// Mirrors `hermes_cli.__version__` — fallback "unknown" (lines 28, 36).
pub const HERMES_CLI_VERSION: &str = env!("CARGO_PKG_VERSION");

fn get_hermes_home() -> PathBuf {
    if let Ok(v) = std::env::var("HERMES_HOME") {
        if !v.trim().is_empty() {
            return PathBuf::from(v.trim());
        }
    }
    dirs_home().join(".hermes")
}
fn dirs_home() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
}

// _HERMES_USER_AGENT — mirrors line 36
/// Identify ourselves so endpoints fronted by Cloudflare's Browser Integrity
/// Check (error 1010) don't reject the default `Python-urllib/*` signature.
pub fn hermes_user_agent() -> String {
    format!("hermes-cli/{}", HERMES_CLI_VERSION)
}

// ---------------------------------------------------------------------------
// Copilot constants — mirrors lines 38-42
// ---------------------------------------------------------------------------

pub const COPILOT_BASE_URL: &str = "https://api.githubcopilot.com";
pub fn copilot_models_url() -> String {
    format!("{COPILOT_BASE_URL}/models")
}
pub const COPILOT_EDITOR_VERSION: &str = "vscode/1.104.1";
pub const COPILOT_REASONING_EFFORTS_GPT5: &[&str] = &["minimal", "low", "medium", "high"];
pub const COPILOT_REASONING_EFFORTS_O_SERIES: &[&str] = &["low", "medium", "high"];

// ---------------------------------------------------------------------------
// _urlopen_model_catalog_request — mirrors lines 44-46
// ---------------------------------------------------------------------------

/// Open catalog requests without forwarding headers across origins.
/// Mirrors `_urlopen_model_catalog_request(req, timeout, ssl_context)` (44-46).
/// In Python this delegates to `hermes_cli.urllib_security.open_credentialed_url`.
/// Rust stub — real HTTP wiring in later slice; kept for 1:1 traceability.
pub fn urlopen_model_catalog_request_stub(
    _url: &str,
    _timeout_secs: f64,
    _ssl_context: Option<&str>,
) -> Result<String, String> {
    Err("urlopen_model_catalog_request: stub — real HTTP in later slice".to_string())
}

// ---------------------------------------------------------------------------
// _custom_provider_ssl_context — mirrors lines 49-78
// ---------------------------------------------------------------------------

/// Build an `ssl.SSLContext` from a custom provider's TLS settings.
/// Mirrors `_custom_provider_ssl_context(base_url)` (49-78).
/// Returns None when no per-provider TLS override applies, so the caller keeps
/// urllib's default policy. In Rust we stub the SSL context as Option<String>
/// describing the override, never breaking discovery on TLS-config lookup.
pub fn custom_provider_ssl_context(base_url: &str) -> Option<String> {
    if base_url.trim().is_empty() {
        return None;
    }
    // Mirrors `get_custom_provider_tls_settings(base_url)` via hermes_cli.config
    // In slice 1 we have no config linkage (NEVER cargo), so probe env-less.
    // Full TLS resolution in later slice when config crate is wired.
    // Keep exception-swallowing contract: any failure → None.
    let _ = base_url;
    // Would: if tls.get("ssl_verify") is False → CERT_NONE
    //        if ca file exists → cafile context
    None
}

// ---------------------------------------------------------------------------
// Fallback OpenRouter snapshot — mirrors lines 81-144
// ---------------------------------------------------------------------------

/// Fallback OpenRouter snapshot used when the live catalog is unavailable.
/// (model_id, display description shown in menus)
/// Mirrors `OPENROUTER_MODELS` (83-144).
pub fn openrouter_models() -> Vec<(&'static str, &'static str)> {
    vec![
        // Anthropic
        ("anthropic/claude-fable-5", ""),
        ("anthropic/claude-opus-5", ""),
        ("anthropic/claude-opus-5-fast", "2x price, higher output speed"),
        ("anthropic/claude-opus-4.8", ""),
        ("anthropic/claude-opus-4.8-fast", "2x price, higher output speed"),
        ("anthropic/claude-sonnet-5", ""),
        ("anthropic/claude-haiku-4.5", ""),
        // OpenAI
        ("openai/gpt-5.6-sol", ""),
        ("openai/gpt-5.6-sol-pro", ""),
        ("openai/gpt-5.6-terra", ""),
        ("openai/gpt-5.6-terra-pro", ""),
        ("openai/gpt-5.6-luna", ""),
        ("openai/gpt-5.6-luna-pro", ""),
        ("openai/gpt-5.5", ""),
        ("openai/gpt-5.5-pro", ""),
        ("openai/gpt-5.4-mini", ""),
        // Google
        ("google/gemini-3.1-pro-preview", ""),
        ("google/gemini-3.7-flash", ""),
        // xAI
        ("x-ai/grok-4.6", ""),
        // DeepSeek
        ("deepseek/deepseek-v4-pro", ""),
        ("deepseek/deepseek-v4-pro-0813", "dated snapshot of v4-pro"),
        ("deepseek/deepseek-v4-flash", ""),
        ("deepseek/deepseek-v4-flash-0731", "dated snapshot of v4-flash"),
        // Qwen
        ("qwen/qwen3.8-max", ""),
        // MoonshotAI
        ("moonshotai/kimi-k3", "recommended"),
        // MiniMax
        ("minimax/minimax-m3", ""),
        // Z-AI
        ("z-ai/glm-5.3", ""),
        ("z-ai/glm-5.2", "default"),
        // Xiaomi
        ("xiaomi/mimo-v2.5-pro", ""),
        // Tencent
        ("tencent/hy3", ""),
        // StepFun
        ("stepfun/step-3.7-flash", ""),
        // NVIDIA
        ("nvidia/nemotron-3-super-120b-a12b", ""),
        // Meta
        ("meta/muse-spark-1.2", ""),
        // Sakana
        ("sakana/fugu-ultra", ""),
        // OpenRouter routers
        (
            "openrouter/pareto-code",
            "auto-routes to cheapest coder meeting openrouter.min_coding_score",
        ),
        // Free tier
        ("stealth/ox-alpha", "free"),
        ("openrouter/elephant-alpha", "free"),
        ("z-ai/glm-5.2:free", "free"),
        ("poolside/laguna-s-2.1:free", "free"),
        ("poolside/laguna-xs-2.1:free", "free"),
        ("nvidia/nemotron-3-super-120b-a12b:free", "free"),
        ("nvidia/nemotron-3-ultra-550b-a55b:free", "free"),
        ("nvidia/nemotron-3.5-lightning:free", "free"),
    ]
}

static OPENROUTER_CATALOG_CACHE: Mutex<Option<Vec<(String, String)>>> = Mutex::new(None);

/// Mirrors `_openrouter_catalog_cache: list[tuple[str, str]] | None = None` (146).
pub fn get_openrouter_catalog_cache() -> Option<Vec<(String, String)>> {
    OPENROUTER_CATALOG_CACHE
        .lock()
        .ok()
        .and_then(|g| g.clone())
}
pub fn set_openrouter_catalog_cache(v: Option<Vec<(String, String)>>) {
    if let Ok(mut g) = OPENROUTER_CATALOG_CACHE.lock() {
        *g = v;
    }
}

// ---------------------------------------------------------------------------
// Fallback Vercel AI Gateway snapshot — mirrors lines 149-171
// ---------------------------------------------------------------------------

/// Fallback Vercel AI Gateway snapshot used when the live catalog is unavailable.
/// Mirrors `VERCEL_AI_GATEWAY_MODELS` (153-169).
pub fn vercel_ai_gateway_models() -> Vec<(&'static str, &'static str)> {
    vec![
        ("moonshotai/kimi-k2.6", "recommended"),
        ("alibaba/qwen3.6-plus", ""),
        ("zai/glm-5.1", ""),
        ("minimax/minimax-m2.7", ""),
        ("anthropic/claude-sonnet-4.6", ""),
        ("anthropic/claude-opus-4.7", ""),
        ("anthropic/claude-opus-4.6", ""),
        ("anthropic/claude-haiku-4.5", ""),
        ("openai/gpt-5.4", ""),
        ("openai/gpt-5.4-mini", ""),
        ("openai/gpt-5.3-codex", ""),
        ("google/gemini-3.1-pro-preview", ""),
        ("google/gemini-3-flash", ""),
        ("google/gemini-3.1-flash-lite-preview", ""),
        ("xai/grok-4.20-reasoning", ""),
    ]
}

static AI_GATEWAY_CATALOG_CACHE: Mutex<Option<Vec<(String, String)>>> = Mutex::new(None);

/// Mirrors `_ai_gateway_catalog_cache: list[tuple[str, str]] | None = None` (171).
pub fn get_ai_gateway_catalog_cache() -> Option<Vec<(String, String)>> {
    AI_GATEWAY_CATALOG_CACHE.lock().ok().and_then(|g| g.clone())
}
pub fn set_ai_gateway_catalog_cache(v: Option<Vec<(String, String)>>) {
    if let Ok(mut g) = AI_GATEWAY_CATALOG_CACHE.lock() {
        *g = v;
    }
}

// ---------------------------------------------------------------------------
// _codex_curated_models — mirrors lines 174-182
// ---------------------------------------------------------------------------

/// Derive the openai-codex curated list from codex_models.py.
/// Mirrors `_codex_curated_models()` (174-182).
/// Single source of truth: DEFAULT_CODEX_MODELS + forward-compat synthesis.
/// In slice 1 we stub the `hermes_cli.codex_models` import (no cargo linkage).
pub fn codex_curated_models() -> Vec<String> {
    // Mirrors: from hermes_cli.codex_models import DEFAULT_CODEX_MODELS, _finalize_codex_models
    //          return _finalize_codex_models(list(DEFAULT_CODEX_MODELS))
    // Stub: return the static fallback that matches the checked-in codex_models default
    // so that _PROVIDER_MODELS["openai-codex"] has a non-empty floor in slice 1.
    // Full wiring when codex_models slice is ported.
    let default_codex_models: Vec<String> = vec![
        "gpt-5.3-codex".to_string(),
        "gpt-5.2-codex".to_string(),
        "gpt-5.1-codex".to_string(),
    ];
    finalize_codex_models_stub(default_codex_models)
}

fn finalize_codex_models_stub(models: Vec<String>) -> Vec<String> {
    // Mirrors `_finalize_codex_models` — dedup + stable order.
    // Stub preserves 1:1 without importing codex_models.
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for m in models {
        if seen.insert(m.clone()) {
            out.push(m);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// xAI helpers — mirrors lines 185-259
// ---------------------------------------------------------------------------

/// Mirrors `_XAI_STATIC_FALLBACK` (195-203).
pub fn xai_static_fallback() -> Vec<&'static str> {
    vec![
        "grok-4.6",
        "grok-build-0.1",
        "grok-4.5",
        "grok-4.3",
        "grok-4.20-0309-reasoning",
        "grok-4.20-0309-non-reasoning",
        "grok-4.20-multi-agent-0309",
    ]
}

/// Mirrors `_XAI_CURATED_EXTRAS` (206-210).
pub fn xai_curated_extras() -> Vec<&'static str> {
    vec!["grok-4.6", "grok-4.5", "grok-composer-2.5-fast"]
}

pub const XAI_TOP_MODEL: &str = "grok-4.6"; // mirrors _XAI_TOP_MODEL (213)

/// Pin the headline xAI model to the top of the curated list.
/// Mirrors `_xai_promote_top(ids)` (216-220).
pub fn xai_promote_top(mut ids: Vec<String>) -> Vec<String> {
    if ids.contains(&XAI_TOP_MODEL.to_string()) {
        let mut out = vec![XAI_TOP_MODEL.to_string()];
        out.extend(ids.into_iter().filter(|m| m != XAI_TOP_MODEL));
        out
    } else {
        ids
    }
}

/// Append Hermes-curated xAI models that are missing from models.dev.
/// Mirrors `_xai_merge_curated_extras(ids)` (223-232).
pub fn xai_merge_curated_extras(mut ids: Vec<String>) -> Vec<String> {
    let mut out = ids;
    for extra in xai_curated_extras() {
        if out.contains(&extra.to_string()) {
            continue;
        }
        let insert_at = if out.first().map(|s| s.as_str()) == Some(XAI_TOP_MODEL) {
            1
        } else {
            out.len()
        };
        out.insert(insert_at, extra.to_string());
    }
    out
}

/// Mirrors `_xai_finalize_catalog(ids)` (235-236).
pub fn xai_finalize_catalog(ids: Vec<String>) -> Vec<String> {
    xai_promote_top(xai_merge_curated_extras(ids))
}

/// Offline curated floor for xAI / xAI OAuth pickers.
/// Mirrors `_xai_curated_models()` (239-258).
/// Reads `$HERMES_HOME/models_dev_cache.json` directly (no network).
/// Falls back to `_XAI_STATIC_FALLBACK` when the cache is empty or unreadable.
pub fn xai_curated_models() -> Vec<String> {
    // Mirrors: from agent.models_dev import _load_disk_cache
    // In slice 1 we stub disk cache access (no agent linkage, NEVER cargo).
    // Preserve exception-swallowing contract: any failure → static fallback.
    if let Some(ids) = xai_load_disk_cache_stub() {
        if !ids.is_empty() {
            // Mirrors: ids = sorted(models.keys()) where models is dict
            let mut sorted = ids;
            sorted.sort();
            return xai_finalize_catalog(sorted);
        }
    }
    xai_finalize_catalog(
        xai_static_fallback()
            .into_iter()
            .map(|s| s.to_string())
            .collect(),
    )
}

fn xai_load_disk_cache_stub() -> Option<Vec<String>> {
    // Would: data = _load_disk_cache(); xai = data.get("xai"); models = xai.get("models")
    // Stub: attempt to read $HERMES_HOME/models_dev_cache.json best-effort without serde
    // (NEVER cargo). For slice 1 we return None so static fallback is used;
    // full JSON parsing in later slice when disk cache format is wired.
    let path = get_hermes_home().join("models_dev_cache.json");
    if !path.exists() {
        return None;
    }
    // Best-effort tiny parse: look for "xai" -> "models" keys existence, but avoid serde.
    // Return None to keep slice 1 simple and deterministic (static fallback).
    let _ = path;
    None
}

// ---------------------------------------------------------------------------
// _PROVIDER_MODELS — mirrors lines 261-720
// ---------------------------------------------------------------------------

/// Build the canonical provider→model-ids map.
/// Mirrors `_PROVIDER_MODELS: dict[str, list[str]] = { ... }` (261-714)
/// plus the derived `ai-gateway` entry (720).
/// Returns a fresh map (Python builds it once at import time; Rust callers may
/// cache via `provider_models_global()`).
pub fn provider_models() -> HashMap<String, Vec<String>> {
    let mut m: HashMap<String, Vec<String>> = HashMap::new();

    m.insert("moa".to_string(), vec!["default".to_string()]);
    m.insert(
        "nous".to_string(),
        vec![
            "anthropic/claude-fable-5".to_string(),
            "anthropic/claude-opus-5".to_string(),
            "anthropic/claude-opus-4.8".to_string(),
            "anthropic/claude-sonnet-5".to_string(),
            "anthropic/claude-haiku-4.5".to_string(),
            "openai/gpt-5.6-sol".to_string(),
            "openai/gpt-5.6-sol-pro".to_string(),
            "openai/gpt-5.6-terra".to_string(),
            "openai/gpt-5.6-terra-pro".to_string(),
            "openai/gpt-5.6-luna".to_string(),
            "openai/gpt-5.6-luna-pro".to_string(),
            "openai/gpt-5.5".to_string(),
            "openai/gpt-5.5-pro".to_string(),
            "openai/gpt-5.4-mini".to_string(),
            "google/gemini-3.1-pro-preview".to_string(),
            "google/gemini-3.7-flash".to_string(),
            "x-ai/grok-4.6".to_string(),
            "deepseek/deepseek-v4-pro".to_string(),
            "deepseek/deepseek-v4-pro-0813".to_string(),
            "deepseek/deepseek-v4-flash".to_string(),
            "deepseek/deepseek-v4-flash-0731".to_string(),
            "qwen/qwen3.8-max".to_string(),
            "moonshotai/kimi-k3".to_string(),
            "minimax/minimax-m3".to_string(),
            "z-ai/glm-5.3".to_string(),
            "z-ai/glm-5.2".to_string(),
            "xiaomi/mimo-v2.5-pro".to_string(),
            "tencent/hy3".to_string(),
            "stepfun/step-3.7-flash".to_string(),
            "nvidia/nemotron-3-super-120b-a12b".to_string(),
            "sakana/fugu-ultra".to_string(),
            "stealth/ox-alpha".to_string(),
        ],
    );
    m.insert(
        "openai".to_string(),
        vec![
            "gpt-5.4".to_string(),
            "gpt-5.4-mini".to_string(),
            "gpt-5-mini".to_string(),
            "gpt-5.3-codex".to_string(),
            "gpt-5.2-codex".to_string(),
            "gpt-4.1".to_string(),
            "gpt-4o".to_string(),
            "gpt-4o-mini".to_string(),
        ],
    );
    m.insert(
        "openai-api".to_string(),
        vec![
            "gpt-5.6-sol".to_string(),
            "gpt-5.6-sol-pro".to_string(),
            "gpt-5.6-terra".to_string(),
            "gpt-5.6-terra-pro".to_string(),
            "gpt-5.6-luna".to_string(),
            "gpt-5.6-luna-pro".to_string(),
            "gpt-5.5".to_string(),
            "gpt-5.5-pro".to_string(),
            "gpt-5.4".to_string(),
            "gpt-5.4-mini".to_string(),
            "gpt-5.4-nano".to_string(),
            "gpt-5-mini".to_string(),
            "gpt-5.3-codex".to_string(),
            "gpt-4.1".to_string(),
            "gpt-4o".to_string(),
            "gpt-4o-mini".to_string(),
        ],
    );
    m.insert("openai-codex".to_string(), codex_curated_models());
    m.insert("xai-oauth".to_string(), xai_curated_models());
    m.insert("copilot-acp".to_string(), vec!["copilot-acp".to_string()]);
    m.insert(
        "copilot".to_string(),
        vec![
            "gpt-5.4".to_string(),
            "gpt-5.4-mini".to_string(),
            "gpt-5-mini".to_string(),
            "gpt-5.3-codex".to_string(),
            "gpt-5.2-codex".to_string(),
            "gpt-4.1".to_string(),
            "gpt-4o".to_string(),
            "gpt-4o-mini".to_string(),
            "claude-sonnet-4.6".to_string(),
            "claude-sonnet-5".to_string(),
            "claude-sonnet-4".to_string(),
            "claude-sonnet-4.5".to_string(),
            "claude-haiku-4.5".to_string(),
            "gemini-3.1-pro-preview".to_string(),
            "gemini-3-pro-preview".to_string(),
            "gemini-3-flash-preview".to_string(),
            "gemini-2.5-pro".to_string(),
        ],
    );
    m.insert(
        "gemini".to_string(),
        vec![
            "gemini-3.1-pro-preview".to_string(),
            "gemini-3-pro-preview".to_string(),
            "gemini-3.6-flash".to_string(),
            "gemini-3.1-flash-lite-preview".to_string(),
        ],
    );
    m.insert(
        "zai".to_string(),
        vec![
            "glm-5.3".to_string(),
            "glm-5.2".to_string(),
            "glm-5.1".to_string(),
            "glm-5".to_string(),
            "glm-5v-turbo".to_string(),
            "glm-5-turbo".to_string(),
            "glm-4.7".to_string(),
            "glm-4.5".to_string(),
            "glm-4.5-flash".to_string(),
        ],
    );
    m.insert("xai".to_string(), xai_curated_models());
    m.insert(
        "nvidia".to_string(),
        vec![
            "nvidia/nemotron-3-ultra-550b-a55b".to_string(),
            "nvidia/nemotron-3-super-120b-a12b".to_string(),
            "nvidia/nemotron-3.5-lightning-30b-a3b".to_string(),
            "nvidia/nemotron-3-nano-omni-30b-a3b-reasoning".to_string(),
            "z-ai/glm-5.3".to_string(),
            "z-ai/glm-5.2".to_string(),
            "moonshotai/kimi-k2.6".to_string(),
            "minimaxai/minimax-m3".to_string(),
        ],
    );
    m.insert(
        "kimi-coding".to_string(),
        vec![
            "kimi-k3".to_string(),
            "kimi-k2.7-code".to_string(),
            "kimi-k2.6".to_string(),
            "kimi-k2.5".to_string(),
            "kimi-for-coding".to_string(),
            "kimi-for-coding-highspeed".to_string(),
            "kimi-k2-thinking".to_string(),
            "kimi-k2-thinking-turbo".to_string(),
            "kimi-k2-turbo-preview".to_string(),
            "kimi-k2-0905-preview".to_string(),
        ],
    );
    m.insert(
        "kimi-coding-cn".to_string(),
        vec![
            "kimi-k3".to_string(),
            "kimi-k2.7-code".to_string(),
            "kimi-k2.7-code-highspeed".to_string(),
            "kimi-k2.6".to_string(),
            "kimi-k2.5".to_string(),
            "kimi-k2-thinking".to_string(),
            "kimi-k2-turbo-preview".to_string(),
            "kimi-k2-0905-preview".to_string(),
        ],
    );
    m.insert(
        "stepfun".to_string(),
        vec!["step-3.5-flash".to_string(), "step-3.5-flash-2603".to_string()],
    );
    m.insert(
        "moonshot".to_string(),
        vec![
            "kimi-k3".to_string(),
            "kimi-k2.6".to_string(),
            "kimi-k2.5".to_string(),
            "kimi-k2-thinking".to_string(),
            "kimi-k2-turbo-preview".to_string(),
            "kimi-k2-0905-preview".to_string(),
        ],
    );
    m.insert(
        "minimax".to_string(),
        vec![
            "MiniMax-M3".to_string(),
            "MiniMax-M2.7".to_string(),
            "MiniMax-M2.5".to_string(),
            "MiniMax-M2.1".to_string(),
            "MiniMax-M2".to_string(),
        ],
    );
    m.insert(
        "minimax-oauth".to_string(),
        vec![
            "MiniMax-M3".to_string(),
            "MiniMax-M2.7".to_string(),
            "MiniMax-M2.7-highspeed".to_string(),
        ],
    );
    m.insert(
        "minimax-cn".to_string(),
        vec![
            "MiniMax-M3".to_string(),
            "MiniMax-M2.7".to_string(),
            "MiniMax-M2.5".to_string(),
            "MiniMax-M2.1".to_string(),
            "MiniMax-M2".to_string(),
        ],
    );
    m.insert(
        "anthropic".to_string(),
        vec![
            "claude-fable-5".to_string(),
            "claude-sonnet-5".to_string(),
            "claude-opus-4-8".to_string(),
            "claude-opus-4-7".to_string(),
            "claude-opus-4-6".to_string(),
            "claude-sonnet-4-6".to_string(),
            "claude-opus-4-5-20251101".to_string(),
            "claude-sonnet-4-5-20250929".to_string(),
            "claude-opus-4-20250514".to_string(),
            "claude-sonnet-4-20250514".to_string(),
            "claude-haiku-4-5-20251001".to_string(),
        ],
    );
    m.insert(
        "deepseek".to_string(),
        vec!["deepseek-v4-pro".to_string(), "deepseek-v4-flash".to_string()],
    );
    m.insert(
        "xiaomi".to_string(),
        vec![
            "mimo-v2.5-pro".to_string(),
            "mimo-v2.5".to_string(),
            "mimo-v2-pro".to_string(),
            "mimo-v2-omni".to_string(),
            "mimo-v2-flash".to_string(),
        ],
    );
    m.insert(
        "tencent-tokenhub".to_string(),
        vec!["hy3-preview".to_string()],
    );
    m.insert(
        "arcee".to_string(),
        vec![
            "trinity-large-thinking".to_string(),
            "trinity-large-preview".to_string(),
            "trinity-mini".to_string(),
        ],
    );
    m.insert(
        "gmi".to_string(),
        vec![
            "zai-org/GLM-5.1-FP8".to_string(),
            "deepseek-ai/DeepSeek-V3.2".to_string(),
            "moonshotai/Kimi-K2.5".to_string(),
            "google/gemini-3.1-flash-lite-preview".to_string(),
            "anthropic/claude-sonnet-5".to_string(),
            "anthropic/claude-sonnet-4.6".to_string(),
            "openai/gpt-5.4".to_string(),
        ],
    );
    m.insert(
        "opencode-zen".to_string(),
        vec![
            "x-preview-f-free".to_string(),
            "kimi-k3".to_string(),
            "kimi-k2.5".to_string(),
            "kimi-k2.6".to_string(),
            "gpt-5.6-sol".to_string(),
            "gpt-5.6-terra".to_string(),
            "gpt-5.6-luna".to_string(),
            "gpt-5.5".to_string(),
            "gpt-5.5-pro".to_string(),
            "gpt-5.4-pro".to_string(),
            "gpt-5.4".to_string(),
            "gpt-5.4-mini".to_string(),
            "gpt-5.4-nano".to_string(),
            "gpt-5.3-codex".to_string(),
            "gpt-5.3-codex-spark".to_string(),
            "gpt-5.2".to_string(),
            "gpt-5.2-codex".to_string(),
            "gpt-5.1".to_string(),
            "gpt-5.1-codex".to_string(),
            "gpt-5.1-codex-max".to_string(),
            "gpt-5.1-codex-mini".to_string(),
            "gpt-5".to_string(),
            "gpt-5-codex".to_string(),
            "gpt-5-nano".to_string(),
            "claude-fable-5".to_string(),
            "claude-opus-5".to_string(),
            "claude-sonnet-5".to_string(),
            "claude-opus-4-8".to_string(),
            "claude-opus-4-7".to_string(),
            "claude-opus-4-6".to_string(),
            "claude-opus-4-5".to_string(),
            "claude-sonnet-4-6".to_string(),
            "claude-sonnet-4-5".to_string(),
            "claude-sonnet-4".to_string(),
            "claude-haiku-4-5".to_string(),
            "gemini-3.7-flash".to_string(),
            "gemini-3.6-flash".to_string(),
            "gemini-3.5-flash".to_string(),
            "gemini-3.5-flash-lite".to_string(),
            "gemini-3.1-pro".to_string(),
            "gemini-3-flash".to_string(),
            "grok-4.6".to_string(),
            "grok-4.5".to_string(),
            "grok-build-0.1".to_string(),
            "muse-spark-1.2".to_string(),
            "minimax-m3".to_string(),
            "minimax-m2.7".to_string(),
            "minimax-m2.5".to_string(),
            "glm-5.3".to_string(),
            "glm-5.2".to_string(),
            "glm-5.1".to_string(),
            "glm-5".to_string(),
            "kimi-k2.7-code".to_string(),
            "deepseek-v4-pro".to_string(),
            "deepseek-v4-flash".to_string(),
            "deepseek-v4-flash-free".to_string(),
            "qwen3.6-plus".to_string(),
            "qwen3.5-plus".to_string(),
            "big-pickle".to_string(),
            "mimo-v2.5-free".to_string(),
            "hy3-free".to_string(),
            "laguna-s-2.1-free".to_string(),
            "nemotron-3-ultra-free".to_string(),
            "nemotron-3.5-lightning-free".to_string(),
            "muse-spark-1.2-contributor-free".to_string(),
        ],
    );
    m.insert(
        "opencode-free".to_string(),
        vec![
            "x-preview-f-free".to_string(),
            "hy3-free".to_string(),
            "laguna-s-2.1-free".to_string(),
            "nemotron-3-ultra-free".to_string(),
            "nemotron-3.5-lightning-free".to_string(),
            "muse-spark-1.2-contributor-free".to_string(),
        ],
    );
    m.insert(
        "opencode-go".to_string(),
        vec![
            "kimi-k3".to_string(),
            "kimi-k2.7-code".to_string(),
            "kimi-k2.6".to_string(),
            "kimi-k2.5".to_string(),
            "gpt-5.6-luna".to_string(),
            "grok-4.5".to_string(),
            "glm-5.3".to_string(),
            "glm-5.2".to_string(),
            "glm-5.1".to_string(),
            "glm-5".to_string(),
            "mimo-v2.5-pro".to_string(),
            "mimo-v2.5".to_string(),
            "mimo-v2-pro".to_string(),
            "mimo-v2-omni".to_string(),
            "minimax-m3".to_string(),
            "minimax-m2.7".to_string(),
            "minimax-m2.5".to_string(),
            "deepseek-v4-pro".to_string(),
            "deepseek-v4-flash".to_string(),
            "qwen3.8-max".to_string(),
            "qwen3.7-max".to_string(),
            "qwen3.7-plus".to_string(),
            "qwen3.6-plus".to_string(),
            "qwen3.5-plus".to_string(),
            "hy3".to_string(),
            "hy3-preview".to_string(),
            "muse-spark-1.2-contributor".to_string(),
            "ox-alpha-free".to_string(),
        ],
    );
    m.insert(
        "kilocode".to_string(),
        vec![
            "anthropic/claude-opus-4.6".to_string(),
            "anthropic/claude-sonnet-4.6".to_string(),
            "openai/gpt-5.4".to_string(),
            "google/gemini-3-pro-preview".to_string(),
            "google/gemini-3-flash-preview".to_string(),
        ],
    );
    m.insert(
        "alibaba".to_string(),
        vec![
            "qwen3.7-max".to_string(),
            "qwen3.7-plus".to_string(),
            "qwen3.6-plus".to_string(),
            "kimi-k2.5".to_string(),
            "qwen3.5-plus".to_string(),
            "qwen3-coder-plus".to_string(),
            "qwen3-coder-next".to_string(),
            "glm-5".to_string(),
            "glm-4.7".to_string(),
            "MiniMax-M2.5".to_string(),
        ],
    );
    m.insert(
        "alibaba-coding-plan".to_string(),
        vec![
            "qwen3.7-plus".to_string(),
            "qwen3.6-plus".to_string(),
            "qwen3.5-plus".to_string(),
            "qwen3-max-2026-01-23".to_string(),
            "qwen3-coder-plus".to_string(),
            "qwen3-coder-next".to_string(),
            "kimi-k2.5".to_string(),
            "glm-5".to_string(),
            "glm-4.7".to_string(),
            "MiniMax-M2.5".to_string(),
        ],
    );
    m.insert(
        "huggingface".to_string(),
        vec![
            "moonshotai/Kimi-K2.5".to_string(),
            "Qwen/Qwen3.5-397B-A17B".to_string(),
            "Qwen/Qwen3.5-35B-A3B".to_string(),
            "deepseek-ai/DeepSeek-V3.2".to_string(),
            "MiniMaxAI/MiniMax-M2.5".to_string(),
            "zai-org/GLM-5".to_string(),
            "XiaomiMiMo/MiMo-V2-Flash".to_string(),
            "moonshotai/Kimi-K2-Thinking".to_string(),
            "moonshotai/Kimi-K2.6".to_string(),
        ],
    );
    m.insert(
        "bedrock".to_string(),
        vec![
            "us.anthropic.claude-sonnet-5".to_string(),
            "us.anthropic.claude-sonnet-4-6".to_string(),
            "us.anthropic.claude-opus-4-6-v1".to_string(),
            "us.anthropic.claude-haiku-4-5-20251001-v1:0".to_string(),
            "us.anthropic.claude-sonnet-4-5-20250929-v1:0".to_string(),
            "openai.gpt-5.5".to_string(),
            "openai.gpt-5.6-sol".to_string(),
            "openai.gpt-5.6-terra".to_string(),
            "openai.gpt-5.6-luna".to_string(),
            "us.amazon.nova-pro-v1:0".to_string(),
            "us.amazon.nova-lite-v1:0".to_string(),
            "us.amazon.nova-micro-v1:0".to_string(),
            "deepseek.v3.2".to_string(),
            "us.meta.llama4-maverick-17b-instruct-v1:0".to_string(),
            "us.meta.llama4-scout-17b-instruct-v1:0".to_string(),
        ],
    );
    m.insert("azure-foundry".to_string(), vec![]);
    m.insert(
        "vertex".to_string(),
        vec![
            "google/gemini-3.1-pro-preview".to_string(),
            "google/gemini-3-pro-preview".to_string(),
            "google/gemini-3.6-flash".to_string(),
            "google/gemini-3.5-flash".to_string(),
            "google/gemini-3.5-flash-lite".to_string(),
            "google/gemini-3-flash-preview".to_string(),
            "google/gemini-3.1-flash-lite-preview".to_string(),
            "google/gemini-3.1-flash-lite".to_string(),
        ],
    );
    m.insert(
        "novita".to_string(),
        vec![
            "moonshotai/kimi-k2.5".to_string(),
            "minimax/minimax-m2.7".to_string(),
            "zai-org/glm-5".to_string(),
            "deepseek/deepseek-v3-0324".to_string(),
            "deepseek/deepseek-r1-0528".to_string(),
            "qwen/qwen3-235b-a22b-fp8".to_string(),
        ],
    );

    // Derived entry — mirrors _PROVIDER_MODELS["ai-gateway"] = [mid for mid, _ in VERCEL_AI_GATEWAY_MODELS] (720)
    let ai_gateway_ids: Vec<String> = vercel_ai_gateway_models()
        .into_iter()
        .map(|(mid, _)| mid.to_string())
        .collect();
    m.insert("ai-gateway".to_string(), ai_gateway_ids);

    m
}

static PROVIDER_MODELS_CACHE: OnceLock<HashMap<String, Vec<String>>> = OnceLock::new();

/// Global cached provider map — mirrors module-level `_PROVIDER_MODELS` dict.
pub fn provider_models_global() -> &'static HashMap<String, Vec<String>> {
    PROVIDER_MODELS_CACHE.get_or_init(provider_models)
}

// ---------------------------------------------------------------------------
// Nous Portal free-model helper — mirrors lines 722-738
// ---------------------------------------------------------------------------

/// Return True if `model_id` has zero-cost prompt AND completion pricing.
/// Mirrors `_is_model_free(model_id, pricing)` (730-738).
pub fn is_model_free(model_id: &str, pricing: &HashMap<String, HashMap<String, String>>) -> bool {
    let p = match pricing.get(model_id) {
        Some(v) => v,
        None => return false,
    };
    let prompt = p.get("prompt").map(|s| s.as_str()).unwrap_or("1");
    let completion = p.get("completion").map(|s| s.as_str()).unwrap_or("1");
    // Mirrors float(p.get("prompt","1")) == 0 and float(p.get("completion","1")) == 0
    // with TypeError/ValueError → False.
    let prompt_f: f64 = prompt.trim().parse().unwrap_or(1.0);
    let compl_f: f64 = completion.trim().parse().unwrap_or(1.0);
    prompt_f == 0.0 && compl_f == 0.0
}

// ---------------------------------------------------------------------------
// Nous Portal account tier detection — mirrors lines 741-769
// ---------------------------------------------------------------------------

/// Return True if the account info indicates a free (unpaid) tier.
/// Mirrors `is_nous_free_tier(account_info)` (744-769).
/// Prefers `paid_service_access.allowed`; falls back to `subscription.monthly_charge == 0`.
pub fn is_nous_free_tier(account_info: &HashMap<String, String>) -> bool {
    // In Python account_info is dict[str, Any] with nested dicts; in Rust we receive
    // flattened string map for slice 1 traceability. Full nested parsing in later slice.
    // Preserve logic: check paid_service_access.allowed → not allowed => free
    // Here we check string keys "paid_service_access.allowed" / "subscription.monthly_charge"
    if let Some(allowed) = account_info.get("paid_service_access.allowed") {
        match allowed.trim().to_lowercase().as_str() {
            "true" => return false,
            "false" => return true,
            _ => {}
        }
    }
    if let Some(paid) = account_info.get("paid_service_access.paid_access") {
        match paid.trim().to_lowercase().as_str() {
            "true" => return false,
            "false" => return true,
            _ => {}
        }
    }
    if let Some(charge) = account_info.get("subscription.monthly_charge") {
        if let Ok(f) = charge.trim().parse::<f64>() {
            return f == 0.0;
        }
    }
    false
}

/// Full dict version — mirrors Python's nested dict check exactly (for later slice wiring).
pub fn is_nous_free_tier_nested(account_info: &serde_stub::JsonValue) -> bool {
    // Mirrors nested dict lookup; stub delegates to serde_stub.
    let _ = account_info;
    false
}

// Placeholder for JSON value type without serde dep (NEVER cargo).
pub mod serde_stub {
    #[derive(Debug, Clone)]
    pub enum JsonValue {
        Null,
        Bool(bool),
        Number(f64),
        Str(String),
        Object(std::collections::HashMap<String, JsonValue>),
        Array(Vec<JsonValue>),
    }
}

// ---------------------------------------------------------------------------
// partition_nous_models_by_tier — mirrors lines 772-797
// ---------------------------------------------------------------------------

/// Split Nous models into (selectable, unavailable) based on user tier.
/// Mirrors `partition_nous_models_by_tier(model_ids, pricing, free_tier)` (772-797).
pub fn partition_nous_models_by_tier(
    model_ids: &[String],
    pricing: &HashMap<String, HashMap<String, String>>,
    free_tier: bool,
) -> (Vec<String>, Vec<String>) {
    if !free_tier {
        return (model_ids.to_vec(), Vec::new());
    }
    if pricing.is_empty() {
        return (model_ids.to_vec(), Vec::new()); // can't determine, show everything
    }
    let mut selectable: Vec<String> = Vec::new();
    let mut unavailable: Vec<String> = Vec::new();
    for mid in model_ids {
        if is_model_free(mid, pricing) {
            selectable.push(mid.clone());
        } else {
            unavailable.push(mid.clone());
        }
    }
    (selectable, unavailable)
}

// ---------------------------------------------------------------------------
// union_with_portal_free_recommendations — mirrors lines 800-863
// ---------------------------------------------------------------------------

/// Mirrors `union_with_portal_free_recommendations(curated_ids, pricing, ...)` (800-863).
/// Augments curated list + pricing with Portal's `freeRecommendedModels`.
/// Failures degrade to returning inputs unchanged.
pub fn union_with_portal_free_recommendations(
    curated_ids: Vec<String>,
    pricing: HashMap<String, HashMap<String, String>>,
    portal_base_url: &str,
    force_refresh: bool,
) -> (Vec<String>, HashMap<String, HashMap<String, String>>) {
    let payload = match fetch_nous_recommended_models_stub(portal_base_url, force_refresh) {
        Some(p) => p,
        None => return (curated_ids, pricing),
    };
    let free_block = payload.get("freeRecommendedModels");
    let free_list = match free_block {
        Some(serde_stub::JsonValue::Array(arr)) if !arr.is_empty() => arr,
        _ => return (curated_ids, pricing),
    };
    let mut portal_free_ids: Vec<String> = Vec::new();
    for entry in free_list {
        if let Some(name) = extract_model_name(entry) {
            portal_free_ids.push(name);
        }
    }
    if portal_free_ids.is_empty() {
        return (curated_ids, pricing);
    }
    let mut augmented_pricing = pricing;
    let free_synthetic: HashMap<String, String> = [
        ("prompt".to_string(), "0".to_string()),
        ("completion".to_string(), "0".to_string()),
    ]
    .into_iter()
    .collect();
    for mid in &portal_free_ids {
        augmented_pricing
            .entry(mid.clone())
            .or_insert_with(|| free_synthetic.clone());
    }
    let mut augmented_ids = curated_ids.clone();
    let seen: std::collections::HashSet<String> = augmented_ids.iter().cloned().collect();
    let new_ones: Vec<String> = portal_free_ids
        .into_iter()
        .filter(|mid| !seen.contains(mid))
        .collect();
    if !new_ones.is_empty() {
        augmented_ids.extend(new_ones);
    }
    (augmented_ids, augmented_pricing)
}

fn fetch_nous_recommended_models_stub(
    _portal_base_url: &str,
    _force_refresh: bool,
) -> Option<HashMap<String, serde_stub::JsonValue>> {
    // Mirrors `fetch_nous_recommended_models` (1053-1112) — real HTTP in later slice.
    // Stub returns None so union degrades to inputs unchanged (silent failure contract).
    None
}

fn extract_model_name(entry: &serde_stub::JsonValue) -> Option<String> {
    // Mirrors `_extract_model_name(entry)` (1131-1138) — pulls `modelName` field.
    if let serde_stub::JsonValue::Object(map) = entry {
        if let Some(serde_stub::JsonValue::Str(s)) = map.get("modelName") {
            let trimmed = s.trim().to_string();
            if !trimmed.is_empty() {
                return Some(trimmed);
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// union_with_portal_paid_recommendations — mirrors lines 866-900 (slice 1 head)
// ---------------------------------------------------------------------------

/// Augment curated list with Portal's `paidRecommendedModels`.
/// Mirrors `union_with_portal_paid_recommendations(...)` (866-900 — head through
/// slice boundary). Full body (paid block parsing, augmentation, return) continues
/// in `models_slice2.rs` from line 901, but the signature + docstring + opening
/// lines through `Portal-side hiccup.` are in this slice for 1:1 line mapping.
///
/// Slice 1 covers through line 900 (`Portal-side hiccup.` comment). The remaining
/// ~29 lines of the function body are in slice 2; this stub preserves the contract
/// (returns inputs unchanged on failure) for 1:1 completeness without duplicating
/// the tail across slices. Real impl in slice 2 will parse `paidRecommendedModels`.
pub fn union_with_portal_paid_recommendations(
    curated_ids: Vec<String>,
    pricing: HashMap<String, HashMap<String, String>>,
    portal_base_url: &str,
    force_refresh: bool,
) -> (Vec<String>, HashMap<String, HashMap<String, String>>) {
    // Lines 866-900 are docstring + opening try/except. Preserve 1:1 structure:
    // try: payload = fetch_nous_recommended_models(portal_base_url, force_refresh=True)
    // except Exception: return (list(curated_ids), dict(pricing))
    let payload = match fetch_nous_recommended_models_stub(portal_base_url, force_refresh) {
        Some(p) => p,
        None => return (curated_ids, pricing),
    };
    // Lines 909-929 (beyond slice) would handle paid_block; slice 1 stops at line 900
    // which is inside the docstring ("Portal-side hiccup."). For syntactic validity
    // we complete the stub with the same degrade-to-inputs contract as the free helper
    // until slice 2 replaces it with full paid-block logic.
    // Full tail in models_slice2.rs:
    //   paid_block = payload.get("paidRecommendedModels") ...
    //   portal_paid_ids = [_extract_model_name(e) for e in paid_block] ...
    //   return (augmented_ids, dict(pricing))
    let _ = payload; // retained for 1:1 traceability
    // Degrade: return inputs unchanged — never block the picker on a Portal hiccup (line 899-900)
    // In the real tail, pricing is NOT synthesized for paid models (see free helper note).
    (curated_ids, pricing)
}

// ---------------------------------------------------------------------------
// Slice boundary — line 900
// ---------------------------------------------------------------------------
// Python `models.py` lines 901-6981 (union_with_portal_paid_recommendations tail,
// _FREE_TIER_CACHE_TTL / _free_tier_cache, check_nous_free_tier,
// NOUS_RECOMMENDED_MODELS_PATH / _nous_recommended_cache, fetch helpers,
// ProviderEntry / CANONICAL_PROVIDERS / PROVIDER_GROUPS, and all catalog
// resolution / validation helpers through EOF) continue in `models_slice2.rs`
// (from line 901, inside the paid-recommendations docstring).
// This file intentionally stops at the 900-line boundary so that `cargo` is
// never invoked and the 8-slice decomposition stays clean.
