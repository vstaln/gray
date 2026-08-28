//! hermes-cli auth — slice 1/11
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/hermes_cli/auth.py`
//! slice 1/11 — lines 1–900 of 9 459 (first 900 LOC).
//! Covers: module docstring + lazy-httpx / fcntl shims, constants
//! (AUTH_STORE_VERSION through SPOTIFY_* and OAUTH_OVER_SSH), helper
//! fns `is_actual_local_base_url` / `normalize_actual_base_url`,
//! provider registry (`ProviderConfig`, `PROVIDER_REGISTRY` + auto-extend
//! from `providers.list_providers`), Anthropic key helper
//! (`get_anthropic_key`), Kimi Code endpoint detection
//! (`KIMI_CODE_BASE_URL`, `_resolve_kimi_base_url`), placeholder-secret
//! filter (`has_usable_secret`, `_resolve_api_key_provider_secret`), and
//! Z.AI endpoint probing (`ZAI_ENDPOINTS`, `_probe_single_zai_endpoint`,
//! `detect_zai_endpoint`, `_resolve_zai_base_url` through line 900).
//! Continued in `auth_slice2.rs` (from `_normalize_lmstudio_runtime_base_url`,
//! line 905).
//!
//! T0683 — 1:1 port, no cargo (NEVER cargo).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

// ---------------------------------------------------------------------------
// Module docstring — mirrors lines 1-17
// ---------------------------------------------------------------------------

/// Module doc — multi-provider authentication for Hermes Agent.
/// Supports OAuth device-code flows (Nous Portal, OpenAI Codex, xAI, Qwen,
/// Spotify, MiniMax) and API-key providers (OpenRouter, custom). Auth state
/// persisted in `~/.hermes/auth.json` with cross-process file locking.
/// ProviderConfig registry, resolve_provider() priority chain, runtime
/// credential refresh, and logout_command() CLI entry point.
pub const MODULE_DOC: &str = "auth: multi-provider authentication — see lines 1-17";

// ---------------------------------------------------------------------------
// Imports — mirrors lines 19-93
// ---------------------------------------------------------------------------
// Python: json, logging, os, shutil, shlex, ssl, stat, sys, base64, hashlib,
// subprocess, threading, time, uuid, webbrowser, importlib, typing,
// contextlib, dataclasses, datetime, http.server, pathlib, urllib.parse,
// hermes_cli.config (get_hermes_home, get_config_path, ...),
// hermes_constants (OPENROUTER_BASE_URL, secure_parent_dir),
// agent.credential_persistence.sanitize_borrowed_credential_payload,
// utils (atomic_replace, atomic_yaml_write, env_float, is_truthy_value),
// fcntl/msvcrt optional imports, httpx lazy proxy.
//
// Rust: std only (NEVER cargo). External crates and hermes-internal
// modules are stubbed for 1:1 traceability; real wiring in later slices.

// httpx lazy proxy stub — mirrors _LazyHttpx (lines 45-75).
// In Python httpx is lazily imported to save ~30ms on CLI startup.
// In Rust HTTP is via `reqwest` in workspace, but not imported here (NEVER cargo).
pub struct LazyHttpxStub;
impl LazyHttpxStub {
    pub fn get(&self, _url: &str) -> Result<String, String> {
        Err("httpx stub — real HTTP in later slice".to_string())
    }
}
pub static HTTPX: LazyHttpxStub = LazyHttpxStub;

// fcntl / msvcrt stubs — mirrors lines 96-103
pub fn has_fcntl() -> bool {
    cfg!(unix)
}
pub fn has_msvcrt() -> bool {
    cfg!(windows)
}

// hermes_cli.config stubs — mirrors lines 84-89
pub fn get_hermes_home_stub() -> PathBuf {
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
pub fn get_config_path_stub() -> PathBuf {
    get_hermes_home_stub().join("config.yaml")
}

// utils stubs — mirrors lines 92
pub fn atomic_replace_stub(_src: &Path, _dst: &Path) -> Result<(), String> {
    Ok(())
}

// ---------------------------------------------------------------------------
// Constants — mirrors lines 105-194
// ---------------------------------------------------------------------------

pub const AUTH_STORE_VERSION: i32 = 1;
pub const AUTH_LOCK_TIMEOUT_SECONDS: f64 = 15.0;

// Nous Portal defaults
pub const DEFAULT_NOUS_PORTAL_URL: &str = "https://portal.nousresearch.com";
pub const DEFAULT_NOUS_INFERENCE_URL: &str = "https://inference-api.nousresearch.com/v1";
pub const DEFAULT_NOUS_CLIENT_ID: &str = "hermes-cli";
pub const NOUS_INFERENCE_INVOKE_SCOPE: &str = "inference:invoke";
pub const NOUS_BILLING_MANAGE_SCOPE: &str = "billing:manage";
pub const DEFAULT_NOUS_SCOPE: &str = NOUS_INFERENCE_INVOKE_SCOPE;
pub const NOUS_DEVICE_CODE_SOURCE: &str = "device_code";
pub const NOUS_AUTH_PATH_INVOKE_JWT: &str = "invoke_jwt";
pub const ACCESS_TOKEN_REFRESH_SKEW_SECONDS: i64 = 120;
pub const NOUS_INVOKE_JWT_MIN_TTL_SECONDS: i64 = ACCESS_TOKEN_REFRESH_SKEW_SECONDS;
pub const DEVICE_AUTH_POLL_INTERVAL_CAP_SECONDS: i64 = 1;
pub const DEFAULT_CODEX_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";
pub const DEFAULT_XAI_OAUTH_BASE_URL: &str = "https://api.x.ai/v1";
pub const MINIMAX_OAUTH_CLIENT_ID: &str = "78257093-7e40-4613-99e0-527b14b39113";
pub const MINIMAX_OAUTH_SCOPE: &str = "group_id profile model.completion";
pub const MINIMAX_OAUTH_GRANT_TYPE: &str = "urn:ietf:params:oauth:grant-type:user_code";
pub const MINIMAX_OAUTH_GLOBAL_BASE: &str = "https://api.minimax.io";
pub const MINIMAX_OAUTH_CN_BASE: &str = "https://api.minimaxi.com";
pub const MINIMAX_OAUTH_GLOBAL_INFERENCE: &str = "https://api.minimax.io/anthropic";
pub const MINIMAX_OAUTH_CN_INFERENCE: &str = "https://api.minimaxi.com/anthropic";
pub const MINIMAX_OAUTH_REFRESH_SKEW_SECONDS: i64 = 60;
pub const DEFAULT_QWEN_BASE_URL: &str = "https://portal.qwen.ai/v1";
pub const DEFAULT_GITHUB_MODELS_BASE_URL: &str = "https://api.githubcopilot.com";
pub const DEFAULT_COPILOT_ACP_BASE_URL: &str = "acp://copilot";
pub const DEFAULT_OLLAMA_CLOUD_BASE_URL: &str = "https://ollama.com/v1";
pub const DEFAULT_ACTUAL_BASE_URL: &str = "https://api.actual.inc/v1";
pub const DEFAULT_ACTUAL_LOCAL_BASE_URL: &str = "http://127.0.0.1:8080/v1";
pub const STEPFUN_STEP_PLAN_INTL_BASE_URL: &str = "https://api.stepfun.ai/step_plan/v1";
pub const STEPFUN_STEP_PLAN_CN_BASE_URL: &str = "https://api.stepfun.com/step_plan/v1";
pub const CODEX_OAUTH_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
pub const CODEX_OAUTH_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
/// Mirrors `hermes_cli.__version__` import (lines 144-148). Fallback "unknown".
pub const HERMES_CLI_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const CODEX_OAUTH_USER_AGENT: &str = "hermes-cli"; // formatted as `hermes-cli/{version}` at runtime via `codex_oauth_user_agent()`
pub fn codex_oauth_user_agent() -> String {
    format!("hermes-cli/{}", HERMES_CLI_VERSION)
}
pub const CODEX_ACCESS_TOKEN_REFRESH_SKEW_SECONDS: i64 = 120;
pub const XAI_OAUTH_ISSUER: &str = "https://auth.x.ai";
pub const XAI_OAUTH_DISCOVERY_URL: &str = "https://auth.x.ai/.well-known/openid-configuration";
pub const XAI_OAUTH_CLIENT_ID: &str = "b1a00492-073a-47ea-816f-4c329264a828";
pub const XAI_OAUTH_SCOPE: &str = "openid profile email offline_access grok-cli:access api:access";
pub const XAI_OAUTH_DEVICE_CODE_URL: &str = "https://auth.x.ai/oauth2/device/code";
/// xAI tokens are short-lived (~6h); refresh 1h early for cron/gateway workloads (lines 155-160)
pub const XAI_ACCESS_TOKEN_REFRESH_SKEW_SECONDS: i64 = 3600;
pub const QWEN_OAUTH_CLIENT_ID: &str = "f0304373b74a44d2b584a3fb70ca9e56";
pub const QWEN_OAUTH_TOKEN_URL: &str = "https://chat.qwen.ai/api/v1/oauth2/token";
pub const QWEN_ACCESS_TOKEN_REFRESH_SKEW_SECONDS: i64 = 120;
pub const DEFAULT_SPOTIFY_ACCOUNTS_BASE_URL: &str = "https://accounts.spotify.com";
pub const DEFAULT_SPOTIFY_API_BASE_URL: &str = "https://api.spotify.com/v1";
pub const DEFAULT_SPOTIFY_REDIRECT_URI: &str = "http://127.0.0.1:43827/spotify/callback";
pub const SPOTIFY_DOCS_URL: &str =
    "https://hermes-agent.nousresearch.com/docs/user-guide/features/spotify";
pub const SPOTIFY_DASHBOARD_URL: &str = "https://developer.spotify.com/dashboard";
pub const SPOTIFY_ACCESS_TOKEN_REFRESH_SKEW_SECONDS: i64 = 120;

pub const OAUTH_OVER_SSH_DOCS_URL: &str =
    "https://hermes-agent.nousresearch.com/docs/guides/oauth-over-ssh";
pub const DEFAULT_SPOTIFY_SCOPE: &str = "user-modify-playback-state user-read-playback-state user-read-currently-playing user-read-recently-played playlist-read-private playlist-read-collaborative playlist-modify-public playlist-modify-private user-library-read user-library-modify";

pub fn service_provider_names() -> HashMap<&'static str, &'static str> {
    let mut m = HashMap::new();
    m.insert("spotify", "Spotify");
    m
}

// LM Studio no-auth sentinel — mirrors lines 188-193
pub const LMSTUDIO_NOAUTH_PLACEHOLDER: &str = "dummy-lm-api-key";
pub const ACTUAL_LOCAL_NOAUTH_PLACEHOLDER: &str = "dummy-actual-local-api-key";

// ---------------------------------------------------------------------------
// is_actual_local_base_url / normalize_actual_base_url — lines 196-225
// ---------------------------------------------------------------------------

/// Mirrors `is_actual_local_base_url(base_url)` (196-202).
/// Returns true for Actual's loopback local API endpoint.
pub fn is_actual_local_base_url(base_url: &str) -> bool {
    let host = extract_hostname(base_url).unwrap_or_default().to_lowercase();
    let host = host.trim_end_matches('.').to_string();
    matches!(
        host.as_str(),
        "localhost" | "127.0.0.1" | "::1" | "0.0.0.0"
    )
}

fn extract_hostname(url: &str) -> Option<String> {
    // Cheap URL hostname extraction without `url` crate (std only, NEVER cargo).
    // Handles scheme://host[:port][/path][?query][#frag], plus userinfo.
    let url = url.trim();
    if url.is_empty() {
        return None;
    }
    // Find scheme separator
    let after_scheme = if let Some(idx) = url.find("://") {
        &url[idx + 3..]
    } else {
        url
    };
    // Strip userinfo if present (look for @ before first /)
    let host_port_path = after_scheme;
    let slash_pos = host_port_path.find('/').unwrap_or(host_port_path.len());
    let host_port = &host_port_path[..slash_pos];
    // If @ present, take after last @
    let host_port = if let Some(at_pos) = host_port.rfind('@') {
        &host_port[at_pos + 1..]
    } else {
        host_port
    };
    // Handle IPv6 bracketed host: [::1]:8080
    if host_port.starts_with('[') {
        if let Some(end) = host_port.find(']') {
            return Some(host_port[1..end].to_string());
        }
        return None;
    }
    // Strip port if present
    let host = if let Some(colon) = host_port.rfind(':') {
        // Ensure colon isn't part of IPv6 without brackets (rare) — treat as host if multiple colons
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
        Some(host.to_string())
    }
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
        // Strip query/fragment
        let end = path_and_rest
            .find(|c| c == '?' || c == '#')
            .unwrap_or(path_and_rest.len());
        path_and_rest[..end].to_string()
    } else {
        String::new()
    }
}

/// Mirrors `normalize_actual_base_url(base_url)` (205-225).
/// Returns Actual's OpenAI-compatible base URL, appending /v1 for bare hosts.
pub fn normalize_actual_base_url(base_url: &str) -> String {
    let url = base_url.trim().trim_end_matches('/').to_string();
    if url.is_empty() {
        return DEFAULT_ACTUAL_BASE_URL.to_string();
    }
    let host = extract_hostname(&url).unwrap_or_default().to_lowercase();
    let host = host.trim_end_matches('.').to_string();
    let path = extract_path(&url).trim_end_matches('/').to_string();
    if host == "api.actual.inc" && (path.is_empty() || path == "/") {
        return format!("{}/v1", url);
    }
    if is_actual_local_base_url(&url) && (path.is_empty() || path == "/") {
        return format!("{}/v1", url);
    }
    url
}

// ---------------------------------------------------------------------------
// Provider Registry — lines 228-598
// ---------------------------------------------------------------------------

/// Mirrors `ProviderConfig` dataclass (232-247).
#[derive(Debug, Clone)]
pub struct ProviderConfig {
    pub id: String,
    pub name: String,
    pub auth_type: String,
    pub portal_base_url: String,
    pub inference_base_url: String,
    pub client_id: String,
    pub scope: String,
    pub extra: HashMap<String, String>,
    pub api_key_env_vars: Vec<String>,
    pub base_url_env_var: String,
}

impl ProviderConfig {
    pub fn new(
        id: &str,
        name: &str,
        auth_type: &str,
        portal_base_url: &str,
        inference_base_url: &str,
        client_id: &str,
        scope: &str,
        extra: HashMap<String, String>,
        api_key_env_vars: Vec<&str>,
        base_url_env_var: &str,
    ) -> Self {
        Self {
            id: id.to_string(),
            name: name.to_string(),
            auth_type: auth_type.to_string(),
            portal_base_url: portal_base_url.to_string(),
            inference_base_url: inference_base_url.to_string(),
            client_id: client_id.to_string(),
            scope: scope.to_string(),
            extra,
            api_key_env_vars: api_key_env_vars.into_iter().map(|s| s.to_string()).collect(),
            base_url_env_var: base_url_env_var.to_string(),
        }
    }
}

/// Mirrors `PROVIDER_REGISTRY` dict (249-564).
/// Returns a fresh map (caller may cache via `provider_registry_global()`).
pub fn provider_registry() -> HashMap<String, ProviderConfig> {
    let mut m: HashMap<String, ProviderConfig> = HashMap::new();

    m.insert(
        "nous".to_string(),
        ProviderConfig::new(
            "nous",
            "Nous Portal",
            "oauth_device_code",
            DEFAULT_NOUS_PORTAL_URL,
            DEFAULT_NOUS_INFERENCE_URL,
            DEFAULT_NOUS_CLIENT_ID,
            DEFAULT_NOUS_SCOPE,
            HashMap::new(),
            vec![],
            "",
        ),
    );
    m.insert(
        "openai-codex".to_string(),
        ProviderConfig::new(
            "openai-codex",
            "OpenAI Codex",
            "oauth_external",
            "",
            DEFAULT_CODEX_BASE_URL,
            "",
            "",
            HashMap::new(),
            vec![],
            "",
        ),
    );
    m.insert(
        "openai-api".to_string(),
        ProviderConfig::new(
            "openai-api",
            "OpenAI API",
            "api_key",
            "",
            "https://api.openai.com/v1",
            "",
            "",
            HashMap::new(),
            vec!["OPENAI_API_KEY"],
            "OPENAI_BASE_URL",
        ),
    );
    m.insert(
        "xai-oauth".to_string(),
        ProviderConfig::new(
            "xai-oauth",
            "xAI Grok OAuth (SuperGrok / Premium+)",
            "oauth_external",
            "",
            DEFAULT_XAI_OAUTH_BASE_URL,
            "",
            "",
            HashMap::new(),
            vec![],
            "",
        ),
    );
    m.insert(
        "qwen-oauth".to_string(),
        ProviderConfig::new(
            "qwen-oauth",
            "Qwen OAuth",
            "oauth_external",
            "",
            DEFAULT_QWEN_BASE_URL,
            "",
            "",
            HashMap::new(),
            vec![],
            "",
        ),
    );
    m.insert(
        "lmstudio".to_string(),
        ProviderConfig::new(
            "lmstudio",
            "LM Studio",
            "api_key",
            "",
            "http://127.0.0.1:1234/v1",
            "",
            "",
            HashMap::new(),
            vec!["LM_API_KEY"],
            "LM_BASE_URL",
        ),
    );
    m.insert(
        "copilot".to_string(),
        ProviderConfig::new(
            "copilot",
            "GitHub Copilot",
            "api_key",
            "",
            DEFAULT_GITHUB_MODELS_BASE_URL,
            "",
            "",
            HashMap::new(),
            vec!["COPILOT_GITHUB_TOKEN", "GH_TOKEN", "GITHUB_TOKEN"],
            "COPILOT_API_BASE_URL",
        ),
    );
    m.insert(
        "copilot-acp".to_string(),
        ProviderConfig::new(
            "copilot-acp",
            "GitHub Copilot ACP",
            "external_process",
            "",
            DEFAULT_COPILOT_ACP_BASE_URL,
            "",
            "",
            HashMap::new(),
            vec![],
            "COPILOT_ACP_BASE_URL",
        ),
    );
    m.insert(
        "gemini".to_string(),
        ProviderConfig::new(
            "gemini",
            "Google AI Studio",
            "api_key",
            "",
            "https://generativelanguage.googleapis.com/v1beta",
            "",
            "",
            HashMap::new(),
            vec!["GOOGLE_API_KEY", "GEMINI_API_KEY"],
            "GEMINI_BASE_URL",
        ),
    );
    m.insert(
        "zai".to_string(),
        ProviderConfig::new(
            "zai",
            "Z.AI / GLM",
            "api_key",
            "",
            "https://api.z.ai/api/paas/v4",
            "",
            "",
            HashMap::new(),
            vec!["GLM_API_KEY", "ZAI_API_KEY", "Z_AI_API_KEY"],
            "GLM_BASE_URL",
        ),
    );
    m.insert(
        "kimi-coding".to_string(),
        ProviderConfig::new(
            "kimi-coding",
            "Kimi / Moonshot",
            "api_key",
            "",
            "https://api.moonshot.ai/v1",
            "",
            "",
            HashMap::new(),
            vec!["KIMI_API_KEY", "KIMI_CODING_API_KEY"],
            "KIMI_BASE_URL",
        ),
    );
    m.insert(
        "kimi-coding-cn".to_string(),
        ProviderConfig::new(
            "kimi-coding-cn",
            "Kimi / Moonshot (China)",
            "api_key",
            "",
            "https://api.moonshot.cn/v1",
            "",
            "",
            HashMap::new(),
            vec!["KIMI_CN_API_KEY"],
            "",
        ),
    );
    m.insert(
        "stepfun".to_string(),
        ProviderConfig::new(
            "stepfun",
            "StepFun Step Plan",
            "api_key",
            "",
            STEPFUN_STEP_PLAN_INTL_BASE_URL,
            "",
            "",
            HashMap::new(),
            vec!["STEPFUN_API_KEY"],
            "STEPFUN_BASE_URL",
        ),
    );
    m.insert(
        "arcee".to_string(),
        ProviderConfig::new(
            "arcee",
            "Arcee AI",
            "api_key",
            "",
            "https://api.arcee.ai/api/v1",
            "",
            "",
            HashMap::new(),
            vec!["ARCEEAI_API_KEY"],
            "ARCEE_BASE_URL",
        ),
    );
    m.insert(
        "gmi".to_string(),
        ProviderConfig::new(
            "gmi",
            "GMI Cloud",
            "api_key",
            "",
            "https://api.gmi-serving.com/v1",
            "",
            "",
            HashMap::new(),
            vec!["GMI_API_KEY"],
            "GMI_BASE_URL",
        ),
    );
    m.insert(
        "actual".to_string(),
        ProviderConfig::new(
            "actual",
            "Actual Computer",
            "api_key",
            "",
            DEFAULT_ACTUAL_BASE_URL,
            "",
            "",
            HashMap::new(),
            vec!["ACTUAL_API_KEY"],
            "ACTUAL_BASE_URL",
        ),
    );
    m.insert(
        "minimax".to_string(),
        ProviderConfig::new(
            "minimax",
            "MiniMax",
            "api_key",
            "",
            "https://api.minimax.io/anthropic",
            "",
            "",
            HashMap::new(),
            vec!["MINIMAX_API_KEY"],
            "MINIMAX_BASE_URL",
        ),
    );
    // minimax-oauth with region extra (lines 382-392)
    {
        let mut extra = HashMap::new();
        extra.insert("region".to_string(), "global".to_string());
        extra.insert(
            "cn_portal_base_url".to_string(),
            MINIMAX_OAUTH_CN_BASE.to_string(),
        );
        extra.insert(
            "cn_inference_base_url".to_string(),
            MINIMAX_OAUTH_CN_INFERENCE.to_string(),
        );
        m.insert(
            "minimax-oauth".to_string(),
            ProviderConfig::new(
                "minimax-oauth",
                "MiniMax (OAuth \u{00b7} minimax.io)",
                "oauth_minimax",
                MINIMAX_OAUTH_GLOBAL_BASE,
                MINIMAX_OAUTH_GLOBAL_INFERENCE,
                MINIMAX_OAUTH_CLIENT_ID,
                MINIMAX_OAUTH_SCOPE,
                extra,
                vec![],
                "",
            ),
        );
    }
    m.insert(
        "anthropic".to_string(),
        ProviderConfig::new(
            "anthropic",
            "Anthropic",
            "api_key",
            "",
            "https://api.anthropic.com",
            "",
            "",
            HashMap::new(),
            vec![
                "ANTHROPIC_API_KEY",
                "ANTHROPIC_TOKEN",
                "CLAUDE_CODE_OAUTH_TOKEN",
            ],
            "ANTHROPIC_BASE_URL",
        ),
    );
    m.insert(
        "alibaba".to_string(),
        ProviderConfig::new(
            "alibaba",
            "Qwen Cloud",
            "api_key",
            "",
            "https://dashscope-intl.aliyuncs.com/compatible-mode/v1",
            "",
            "",
            HashMap::new(),
            vec!["DASHSCOPE_API_KEY"],
            "DASHSCOPE_BASE_URL",
        ),
    );
    m.insert(
        "alibaba-coding-plan".to_string(),
        ProviderConfig::new(
            "alibaba-coding-plan",
            "Alibaba Cloud (Coding Plan)",
            "api_key",
            "",
            "https://coding-intl.dashscope.aliyuncs.com/v1",
            "",
            "",
            HashMap::new(),
            vec!["ALIBABA_CODING_PLAN_API_KEY", "DASHSCOPE_API_KEY"],
            "ALIBABA_CODING_PLAN_BASE_URL",
        ),
    );
    m.insert(
        "minimax-cn".to_string(),
        ProviderConfig::new(
            "minimax-cn",
            "MiniMax (China)",
            "api_key",
            "",
            "https://api.minimaxi.com/anthropic",
            "",
            "",
            HashMap::new(),
            vec!["MINIMAX_CN_API_KEY"],
            "MINIMAX_CN_BASE_URL",
        ),
    );
    m.insert(
        "deepseek".to_string(),
        ProviderConfig::new(
            "deepseek",
            "DeepSeek",
            "api_key",
            "",
            "https://api.deepseek.com/v1",
            "",
            "",
            HashMap::new(),
            vec!["DEEPSEEK_API_KEY"],
            "DEEPSEEK_BASE_URL",
        ),
    );
    m.insert(
        "xai".to_string(),
        ProviderConfig::new(
            "xai",
            "xAI",
            "api_key",
            "",
            "https://api.x.ai/v1",
            "",
            "",
            HashMap::new(),
            vec!["XAI_API_KEY"],
            "XAI_BASE_URL",
        ),
    );
    m.insert(
        "nvidia".to_string(),
        ProviderConfig::new(
            "nvidia",
            "NVIDIA NIM",
            "api_key",
            "",
            "https://integrate.api.nvidia.com/v1",
            "",
            "",
            HashMap::new(),
            vec!["NVIDIA_API_KEY"],
            "NVIDIA_BASE_URL",
        ),
    );
    m.insert(
        "ai-gateway".to_string(),
        ProviderConfig::new(
            "ai-gateway",
            "Vercel AI Gateway",
            "api_key",
            "",
            "https://ai-gateway.vercel.sh/v1",
            "",
            "",
            HashMap::new(),
            vec!["AI_GATEWAY_API_KEY"],
            "AI_GATEWAY_BASE_URL",
        ),
    );
    m.insert(
        "opencode-zen".to_string(),
        ProviderConfig::new(
            "opencode-zen",
            "OpenCode Zen",
            "api_key",
            "",
            "https://opencode.ai/zen/v1",
            "",
            "",
            HashMap::new(),
            vec!["OPENCODE_ZEN_API_KEY"],
            "OPENCODE_ZEN_BASE_URL",
        ),
    );
    m.insert(
        "opencode-go".to_string(),
        ProviderConfig::new(
            "opencode-go",
            "OpenCode Go",
            "api_key",
            "",
            "https://opencode.ai/zen/go/v1",
            "",
            "",
            HashMap::new(),
            vec!["OPENCODE_GO_API_KEY"],
            "OPENCODE_GO_BASE_URL",
        ),
    );
    m.insert(
        "opencode-free".to_string(),
        ProviderConfig::new(
            "opencode-free",
            "OpenCode Free",
            "api_key",
            "",
            "https://opencode.ai/zen/v1",
            "",
            "",
            HashMap::new(),
            vec![],
            "",
        ),
    );
    m.insert(
        "kilocode".to_string(),
        ProviderConfig::new(
            "kilocode",
            "Kilo Code",
            "api_key",
            "",
            "https://api.kilo.ai/api/gateway",
            "",
            "",
            HashMap::new(),
            vec!["KILOCODE_API_KEY"],
            "KILOCODE_BASE_URL",
        ),
    );
    m.insert(
        "huggingface".to_string(),
        ProviderConfig::new(
            "huggingface",
            "Hugging Face",
            "api_key",
            "",
            "https://router.huggingface.co/v1",
            "",
            "",
            HashMap::new(),
            vec!["HF_TOKEN"],
            "HF_BASE_URL",
        ),
    );
    m.insert(
        "xiaomi".to_string(),
        ProviderConfig::new(
            "xiaomi",
            "Xiaomi MiMo",
            "api_key",
            "",
            "https://api.xiaomimimo.com/v1",
            "",
            "",
            HashMap::new(),
            vec!["XIAOMI_API_KEY"],
            "XIAOMI_BASE_URL",
        ),
    );
    m.insert(
        "tencent-tokenhub".to_string(),
        ProviderConfig::new(
            "tencent-tokenhub",
            "Tencent TokenHub",
            "api_key",
            "",
            "https://tokenhub.tencentmaas.com/v1",
            "",
            "",
            HashMap::new(),
            vec!["TOKENHUB_API_KEY"],
            "TOKENHUB_BASE_URL",
        ),
    );
    m.insert(
        "ollama-cloud".to_string(),
        ProviderConfig::new(
            "ollama-cloud",
            "Ollama Cloud",
            "api_key",
            "",
            DEFAULT_OLLAMA_CLOUD_BASE_URL,
            "",
            "",
            HashMap::new(),
            vec!["OLLAMA_API_KEY"],
            "OLLAMA_BASE_URL",
        ),
    );
    m.insert(
        "bedrock".to_string(),
        ProviderConfig::new(
            "bedrock",
            "AWS Bedrock",
            "aws_sdk",
            "",
            "https://bedrock-runtime.us-east-1.amazonaws.com",
            "",
            "",
            HashMap::new(),
            vec![],
            "BEDROCK_BASE_URL",
        ),
    );
    m.insert(
        "vertex".to_string(),
        ProviderConfig::new(
            "vertex",
            "Google Vertex AI",
            "vertex",
            "",
            "",
            "",
            "",
            HashMap::new(),
            vec![],
            "",
        ),
    );
    m.insert(
        "azure-foundry".to_string(),
        ProviderConfig::new(
            "azure-foundry",
            "Azure Foundry",
            "api_key",
            "",
            "",
            "",
            "",
            HashMap::new(),
            vec!["AZURE_FOUNDRY_API_KEY"],
            "AZURE_FOUNDRY_BASE_URL",
        ),
    );

    // Auto-extend from providers.list_providers — mirrors lines 566-598
    // Python: try importing `providers.list_providers`, skip if not available or not api_key.
    // In Rust we stub `list_providers` as empty (NEVER cargo, no provider plugin linkage yet).
    // Logic preserved 1:1 so later wiring only needs to replace the stub source.
    let extra_providers = list_providers_stub();
    for pp in extra_providers {
        if m.contains_key(&pp.name) {
            continue;
        }
        if pp.auth_type != "api_key" || pp.env_vars.is_empty() {
            continue;
        }
        if matches!(
            pp.name.as_str(),
            "copilot" | "kimi-coding" | "kimi-coding-cn" | "zai" | "openrouter" | "custom"
        ) {
            continue;
        }
        let api_key_vars: Vec<String> = pp
            .env_vars
            .iter()
            .filter(|v| !v.ends_with("_BASE_URL") && !v.ends_with("_URL"))
            .cloned()
            .collect();
        let base_url_var = pp
            .env_vars
            .iter()
            .find(|v| v.ends_with("_BASE_URL") || v.ends_with("_URL"))
            .cloned()
            .unwrap_or_default();
        let api_key_refs: Vec<&str> = if api_key_vars.is_empty() {
            pp.env_vars.iter().map(|s| s.as_str()).collect()
        } else {
            api_key_vars.iter().map(|s| s.as_str()).collect()
        };
        let cfg = ProviderConfig::new(
            &pp.name,
            if pp.display_name.is_empty() {
                &pp.name
            } else {
                &pp.display_name
            },
            "api_key",
            &pp.base_url,
            &pp.base_url,
            "",
            "",
            HashMap::new(),
            api_key_refs,
            &base_url_var,
        );
        // Register aliases 1:1
        let aliases = pp.aliases.clone();
        m.insert(pp.name.clone(), cfg.clone());
        for alias in aliases {
            if !m.contains_key(&alias) {
                m.insert(alias, cfg.clone());
            }
        }
    }

    m
}

static PROVIDER_REGISTRY_CACHE: OnceLock<HashMap<String, ProviderConfig>> = OnceLock::new();

/// Global cached registry — mirrors module-level `PROVIDER_REGISTRY` dict (line 249).
pub fn provider_registry_global() -> &'static HashMap<String, ProviderConfig> {
    PROVIDER_REGISTRY_CACHE.get_or_init(provider_registry)
}

// Stub for `providers.list_providers` used in auto-extend (566-598).
#[derive(Debug, Clone)]
pub struct ProviderProfileStub {
    pub name: String,
    pub display_name: String,
    pub auth_type: String,
    pub base_url: String,
    pub env_vars: Vec<String>,
    pub aliases: Vec<String>,
}
fn list_providers_stub() -> Vec<ProviderProfileStub> {
    // No bundled provider discovery without `providers` crate; empty in slice 1.
    Vec::new()
}

// ---------------------------------------------------------------------------
// Anthropic Key Helper — lines 601-623
// ---------------------------------------------------------------------------

/// Mirrors `get_anthropic_key()` (605-622).
/// Checks `PROVIDER_REGISTRY["anthropic"].api_key_env_vars` via
/// `get_env_value_prefer_dotenv` order; returns first usable value or "".
pub fn get_anthropic_key() -> String {
    let registry = provider_registry_global();
    let pconfig = match registry.get("anthropic") {
        Some(c) => c,
        None => return String::new(),
    };
    for var in &pconfig.api_key_env_vars {
        let value = get_env_value_prefer_dotenv_stub(var).unwrap_or_default();
        if !value.trim().is_empty() {
            return value;
        }
    }
    String::new()
}

fn get_env_value_prefer_dotenv_stub(var: &str) -> Option<String> {
    // Mirrors `hermes_cli.config.get_env_value_prefer_dotenv` — prefers ~/.hermes/.env
    // over os.environ. In slice 1 we check env var then dotenv file best-effort.
    // Priority: dotenv file first, then os.environ (matches Python's prefer_dotenv).
    let dotenv_val = read_dotenv_value(var);
    if let Some(v) = dotenv_val {
        if !v.trim().is_empty() {
            return Some(v);
        }
    }
    std::env::var(var).ok().filter(|v| !v.trim().is_empty())
}

fn read_dotenv_value(var: &str) -> Option<String> {
    let env_file = get_hermes_home_stub().join(".env");
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

// ---------------------------------------------------------------------------
// Kimi Code Endpoint Detection — lines 625-656
// ---------------------------------------------------------------------------

/// Mirrors `KIMI_CODE_BASE_URL = "https://api.kimi.com/coding"` (639).
/// No /v1 suffix — SDK appends /v1/messages for Anthropic protocol.
pub const KIMI_CODE_BASE_URL: &str = "https://api.kimi.com/coding";

/// Mirrors `_resolve_kimi_base_url(api_key, default_url, env_override)` (642-655).
/// If `env_override` non-empty, it wins. Else `sk-kimi-` keys route to `KIMI_CODE_BASE_URL`.
pub fn resolve_kimi_base_url(api_key: &str, default_url: &str, env_override: &str) -> String {
    if !env_override.trim().is_empty() {
        return env_override.to_string();
    }
    if api_key.trim().is_empty() {
        return default_url.to_string();
    }
    if api_key.starts_with("sk-kimi-") {
        return KIMI_CODE_BASE_URL.to_string();
    }
    default_url.to_string()
}

// ---------------------------------------------------------------------------
// Placeholder secret filter — lines 659-728
// ---------------------------------------------------------------------------

static PLACEHOLDER_SECRET_VALUES: OnceLock<HashSet<String>> = OnceLock::new();

fn placeholder_secret_values() -> &'static HashSet<String> {
    PLACEHOLDER_SECRET_VALUES.get_or_init(|| {
        [
            "*",
            "**",
            "***",
            "changeme",
            "your_api_key",
            "your_api_key_here",
            "your-api-key",
            "placeholder",
            "example",
            "dummy",
            "null",
            "none",
        ]
        .into_iter()
        .map(|s| s.to_string())
        .collect()
    })
}

/// Mirrors `has_usable_secret(value, min_length=4)` (675-684).
pub fn has_usable_secret(value: &str, min_length: usize) -> bool {
    let cleaned = value.trim();
    if cleaned.len() < min_length {
        return false;
    }
    if placeholder_secret_values().contains(&cleaned.to_lowercase()) {
        return false;
    }
    true
}
/// Convenience with default min_length=4 (mirrors Python default).
pub fn has_usable_secret_default(value: &str) -> bool {
    has_usable_secret(value, 4)
}

/// Mirrors `_resolve_api_key_provider_secret(provider_id, pconfig)` (687-728).
/// Resolves token and source; falls back to credential_pool.peek() best-effort.
pub fn resolve_api_key_provider_secret(
    provider_id: &str,
    pconfig: &ProviderConfig,
) -> (String, String) {
    if provider_id == "copilot" {
        // Mirrors copilot special path: hermes_cli.copilot_auth.resolve_copilot_token()
        // In slice 1 we stub copilot token resolution as unavailable → return ("", "")
        // Real impl wired in later slice when copilot_auth module is ported.
        return (String::new(), String::new());
    }
    for env_var in &pconfig.api_key_env_vars {
        let val = get_env_value_prefer_dotenv_stub(env_var)
            .unwrap_or_default()
            .trim()
            .to_string();
        if has_usable_secret_default(&val) {
            return (val, env_var.clone());
        }
    }
    // Fallback: credential_pool.peek() — mirrors lines 714-726
    // In slice 1 we stub load_pool as unavailable.
    if let Some((key, source)) = credential_pool_peek_stub(provider_id) {
        if has_usable_secret_default(&key) {
            return (key, source);
        }
    }
    (String::new(), String::new())
}

fn credential_pool_peek_stub(provider_id: &str) -> Option<(String, String)> {
    let _ = provider_id;
    // Would call agent.credential_pool.load_pool(provider_id).peek()
    None
}

// ---------------------------------------------------------------------------
// Z.AI Endpoint Detection — lines 731-902
// ---------------------------------------------------------------------------

/// Mirrors `ZAI_ENDPOINTS` (742-748).
/// Each entry: (id, base_url, probe_models, label)
pub fn zai_endpoints() -> Vec<(&'static str, &'static str, Vec<&'static str>, &'static str)> {
    vec![
        (
            "global",
            "https://api.z.ai/api/paas/v4",
            vec!["glm-5"],
            "Global",
        ),
        (
            "cn",
            "https://open.bigmodel.cn/api/paas/v4",
            vec!["glm-5"],
            "China",
        ),
        (
            "coding-global",
            "https://api.z.ai/api/coding/paas/v4",
            vec!["glm-5.3", "glm-5.2", "glm-5.1", "glm-5v-turbo", "glm-4.7"],
            "Global (Coding Plan)",
        ),
        (
            "coding-cn",
            "https://open.bigmodel.cn/api/coding/paas/v4",
            vec!["glm-5.3", "glm-5.2", "glm-5.1", "glm-5v-turbo", "glm-4.7"],
            "China (Coding Plan)",
        ),
    ]
}

/// Mirrors `_probe_single_zai_endpoint(api_key, endpoint, timeout)` (751-788).
/// Probes `POST {base_url}/chat/completions` for each candidate model until 200.
pub fn probe_single_zai_endpoint(
    api_key: &str,
    endpoint: &(&str, &str, Vec<&str>, &str),
    timeout_secs: f64,
) -> Option<HashMap<String, String>> {
    let (ep_id, base_url, probe_models, label) = endpoint;
    for model in probe_models {
        // In Python this does httpx.post with Authorization: Bearer {api_key}
        // and JSON body {model, stream:false, max_tokens:1, messages:[{role:user, content:ping}]}
        // In Rust slice 1 we stub the HTTP call — real probe in later slice with reqwest.
        // Keep 1:1 control flow: would return on 200, else continue.
        let _ = (api_key, base_url, model, label, timeout_secs, ep_id);
        // Stub: no network in slice 1; simulate probe failure so caller falls through.
        // Real impl will perform blocking HTTP and check status == 200.
        continue;
    }
    None
}

/// Mirrors `detect_zai_endpoint(api_key, timeout=8.0)` (791-838).
/// Probes all endpoints in parallel, returns first working in priority order.
pub fn detect_zai_endpoint(
    api_key: &str,
    timeout_secs: f64,
) -> Option<HashMap<String, String>> {
    // Python uses ThreadPoolExecutor(max_workers=len(ZAI_ENDPOINTS)) with
    // as_completed + early-exit priority logic (lines 799-838) and
    // pool.shutdown(wait=False) to avoid joining slow probes.
    //
    // Rust slice 1: sequential stub (no cargo thread-pool dep, no network).
    // Preserves priority-order return contract and timeout param in signature
    // for 1:1 traceability; real parallel probe in later slice.
    for ep in zai_endpoints() {
        let ep_ref = (&ep.0, &ep.1, ep.2.clone(), &ep.3);
        // normalize to tuple ref expected by probe_single
        let probe = (ep.0, ep.1, ep.2.clone(), ep.3);
        if let Some(result) = probe_single_zai_endpoint(api_key, &probe, timeout_secs) {
            let _ = ep_ref;
            return Some(result);
        }
    }
    None
}

// Auth store helpers — stubs for _load_auth_store / _load_provider_state etc. (used in _resolve_zai_base_url)
// Mirrors lines 862-902; full auth.json persistence in later slices.

fn load_auth_store_stub() -> HashMap<String, String> {
    HashMap::new()
}
fn load_provider_state_stub(
    _store: &HashMap<String, String>,
    _provider: &str,
) -> Option<HashMap<String, String>> {
    None
}

/// Mirrors `_resolve_zai_base_url(api_key, default_url, env_override)` (841-902).
/// Env override wins; else check cached `detected_endpoint` in auth.json (hash-keyed);
/// else probe endpoints and persist result (write locked with `_auth_store_lock`).
pub fn resolve_zai_base_url(api_key: &str, default_url: &str, env_override: &str) -> String {
    if !env_override.trim().is_empty() {
        return env_override.to_string();
    }
    if api_key.trim().is_empty() {
        return default_url.to_string();
    }

    // Check provider-state cache for previously-detected endpoint (lines 862-868)
    let auth_store = load_auth_store_stub();
    let state = load_provider_state_stub(&auth_store, "zai");
    if let Some(cached) = state {
        // cached is HashMap with "detected_endpoint" serialized as JSON string in stub
        // Real impl checks cached["detected_endpoint"]["base_url"] and key_hash == sha256(api_key)[:16]
        // Stub: no cache hit in slice 1 (no JSON dep).
        let _ = cached;
    }

    // Probe — may take up to ~8s per endpoint (line 871)
    if let Some(detected) = detect_zai_endpoint(api_key, 8.0) {
        if let Some(base_url) = detected.get("base_url") {
            if !base_url.trim().is_empty() {
                // Persist detection keyed on api_key hash — mirrors lines 874-899
                // Would do: with _auth_store_lock(): reload, set state["detected_endpoint"], save
                // Stub: skip persistence in slice 1 (requires auth.json locking).
                // Log: "Z.AI: auto-detected endpoint {label} ({base_url})"
                return base_url.clone();
            }
        }
    }
    // Fallback — mirrors line 901-902
    // logger.debug("Z.AI: probe failed, falling back to default %s", default_url)
    default_url.to_string()
}

// ---------------------------------------------------------------------------
// Slice boundary — line 900
// ---------------------------------------------------------------------------
// Python `auth.py` lines 905-9459 ( _normalize_lmstudio_runtime_base_url,
// AuthError, _auth_store_lock, _load_auth_store, _load_provider_state,
// resolve_provider, resolve_*_runtime_credentials, logout_command, … )
// continue in `auth_slice2.rs` (from line 901).
// This file intentionally stops at the 900-line boundary (mid-function at
// `return default_url` of `_resolve_zai_base_url`) so that `cargo` is never
// invoked and the 11-slice decomposition stays clean.
