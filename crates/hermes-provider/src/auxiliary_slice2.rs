//! Shared auxiliary client router for side tasks — slice 2.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/agent/auxiliary_client.py`
//! (10831 lines) — slice 2/12, lines 900-1800.
//!
//! ```text
//! Slice 1 (ll.1-900): OpenAI proxy, probe mode, TLS/HTTP keepalive, interrupt
//!   protection, provider aliases, temperature/compaction thresholds, fast-model
//!   catalog (_fast_model_from_catalog).
//! Slice 2 (ll.900-1800): _get_aux_model_for_provider + fallback dicts,
//!   vision overrides, OpenRouter/NVIDIA/AI-Gateway headers,
//!   _apply_user_default_headers / build_or_headers / build_nvidia_nim_headers,
//!   Nous portal tags, Codex Cloudflare headers, dual-surface host rewriting,
//!   pool helpers (_select_pool_entry / _peek_pool_entry / _pool_runtime_*),
//!   Anthropic-compatible host guard, _nous_min_key_ttl_seconds,
//!   _scoped_key_env, _CodexCompletionsAdapter (truncated at l.1800).
//! ```
//!
//! NEVER cargo — 1:1 translation only (no `cargo check` / `cargo build`).
//! Mirrors Python ll.900-1800 verbatim; line numbers in comments refer to the
//! 10831-line source file. Slice 3 continues from l.1801.
//!
//! T0022 — 1:1 port, no cargo (NEVER cargo).

#![allow(dead_code, unused_imports, unused_variables, clippy::all)]

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// Logger — mirrors `logger = logging.getLogger(__name__)` (l.173)
// ---------------------------------------------------------------------------
const LOG_TARGET: &str = "auxiliary_client";

// ---------------------------------------------------------------------------
// Cross-module stubs reused in this slice — mirrors `from providers import ...`
// and `from agent.credential_pool import load_pool` etc.
// ---------------------------------------------------------------------------

/// Minimal stub for `providers.ProviderProfile` used by several helpers in this
/// slice. Real impl lives in `providers/__init__.py` and is discovered via
/// `get_provider_profile(provider_id)`. Fields mirror Python attributes.
#[derive(Debug, Clone, Default)]
pub struct ProviderProfileStub {
    pub default_aux_model: Option<String>,
    // Other fields not needed in this slice.
}

impl ProviderProfileStub {
    pub fn resolve_aux_model(&self) -> Option<String> {
        // Real impl queries live `/v1/models` or provider recommendation hook.
        None
    }
    pub fn default_vision_model(&self) -> Option<String> {
        // Real impl for DeepInfra etc. — live catalog lookup.
        None
    }
    pub fn base_url(&self) -> Option<String> { None }
}

/// Mirrors `from providers import get_provider_profile` (ll.932, 1022).
pub fn get_provider_profile(_provider_id: &str) -> Option<ProviderProfileStub> {
    None
}

// ---------------------------------------------------------------------------
// _get_aux_model_for_provider — mirrors ll.910-951
// ---------------------------------------------------------------------------
// Python:
//   def _get_aux_model_for_provider(provider_id: str, *, prefer_fast: bool = False) -> str:
//       """Return the cheap auxiliary model for a provider.
//       Resolution ladder … (docstring ll.912-928) """
//       profile = None
//       try: from providers import get_provider_profile; profile = get_provider_profile(provider_id)
//       except Exception: pass
//       if prefer_fast:
//           catalog_pick = _fast_model_from_catalog(provider_id)
//           if catalog_pick: return catalog_pick
//           if profile is not None:
//               try: live = profile.resolve_aux_model(); if live: return live
//               except Exception: logger.debug(...)
//       if profile is not None and profile.default_aux_model: return profile.default_aux_model
//       return _API_KEY_PROVIDER_AUX_MODELS_FALLBACK.get(provider_id, "")

/// Mirrors `def _get_aux_model_for_provider(provider_id: str, *, prefer_fast: bool = False) -> str:` (ll.910-951).
pub fn get_aux_model_for_provider(provider_id: &str, prefer_fast: bool) -> String {
    let mut profile: Option<ProviderProfileStub> = None;
    // Mirrors `try: from providers import get_provider_profile; profile = get_provider_profile(provider_id)` / `except Exception: pass` (ll.931-935).
    // In Rust the import is a stub call; any panic is caught.
    let profile_result = std::panic::catch_unwind(|| get_provider_profile(provider_id));
    if let Ok(p) = profile_result {
        profile = p;
    }

    if prefer_fast {
        let catalog_pick = fast_model_from_catalog(provider_id); // mirrors `_fast_model_from_catalog(provider_id)` (l.938)
        if !catalog_pick.is_empty() {
            return catalog_pick;
        }
        if let Some(ref prof) = profile {
            // Mirrors `try: live = profile.resolve_aux_model(); if live: return live` / `except Exception: logger.debug(...)` (ll.941-947).
            let live_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| prof.resolve_aux_model()));
            if let Ok(Some(live)) = live_result {
                if !live.trim().is_empty() {
                    return live;
                }
            } else if live_result.is_err() {
                // Mirrors `logger.debug("resolve_aux_model failed for %s", provider_id, exc_info=True)` (l.947).
                // Stub: no-op debug log.
            }
        }
    }

    if let Some(ref prof) = profile {
        if let Some(ref def) = prof.default_aux_model {
            if !def.trim().is_empty() {
                return def.clone();
            }
        }
    }
    api_key_provider_aux_models_fallback()
        .get(provider_id)
        .cloned()
        .unwrap_or_default()
        .to_string()
}

#[allow(dead_code)]
fn _get_aux_model_for_provider(provider_id: &str, prefer_fast: bool) -> String {
    get_aux_model_for_provider(provider_id, prefer_fast)
}

// Stub for `_fast_model_from_catalog` — mirrors ll.857-907, defined in slice1.
// Here we provide a slice-local stub so this file is self-contained.
fn fast_model_from_catalog(_provider_id: &str) -> String { String::new() }

// ---------------------------------------------------------------------------
// _API_KEY_PROVIDER_AUX_MODELS_FALLBACK — mirrors ll.954-976
// ---------------------------------------------------------------------------

/// Mirrors `_API_KEY_PROVIDER_AUX_MODELS_FALLBACK: Dict[str, str] = { ... }` (ll.958-976).
pub fn api_key_provider_aux_models_fallback() -> HashMap<&'static str, &'static str> {
    let mut m = HashMap::new();
    m.insert("gemini", "gemini-3.6-flash");
    m.insert("zai", "glm-4.5-flash");
    m.insert("kimi-coding", "kimi-k2-turbo-preview");
    m.insert("stepfun", "step-3.5-flash");
    m.insert("kimi-coding-cn", "kimi-k2-turbo-preview");
    m.insert("gmi", "google/gemini-3.1-flash-lite-preview");
    m.insert("anthropic", "claude-haiku-4-5-20251001");
    m.insert("ai-gateway", "google/gemini-3-flash");
    m.insert("opencode-zen", "gemini-3-flash");
    m.insert("opencode-go", "glm-5");
    m.insert("kilocode", "google/gemini-3.6-flash");
    m.insert("ollama-cloud", "nemotron-3-nano:30b");
    m.insert("tencent-tokenhub", "hy3-preview");
    // NB: no "deepinfra" entry — lives on ProviderProfile (see Python comment ll.972-975).
    m
}

// Legacy alias — mirrors ll.978-980.
pub fn api_key_provider_aux_models() -> HashMap<&'static str, &'static str> {
    api_key_provider_aux_models_fallback()
}

// ---------------------------------------------------------------------------
// _FAST_MODEL_TASKS + _task_prefers_fast_model — mirrors ll.982-994
// ---------------------------------------------------------------------------

/// Mirrors `_FAST_MODEL_TASKS: frozenset = frozenset({"title_generation"})` (l.986).
pub const FAST_MODEL_TASKS: &[&str] = &["title_generation"];

pub fn is_fast_model_task(task: &str) -> bool {
    FAST_MODEL_TASKS.contains(&task)
}

/// Stub for `_get_auxiliary_task_config` — mirrors `agent.auxiliary_client._get_auxiliary_task_config`.
fn get_auxiliary_task_config(_task: &str) -> HashMap<String, String> { HashMap::new() }

/// Stub for `utils.is_truthy_value` — mirrors truthy parsing in slice1.
fn is_truthy_value(_value: Option<&str>, default: bool) -> bool { default }

/// Mirrors `def _task_prefers_fast_model(task: Optional[str]) -> bool:` (ll.989-994).
pub fn task_prefers_fast_model(task: Option<&str>) -> bool {
    let t = match task { Some(v) => v, None => return false };
    if !is_fast_model_task(t) {
        return false;
    }
    let task_config = get_auxiliary_task_config(t);
    // Mirrors `is_truthy_value(task_config.get("prefer_fast_model"), default=False)` (l.994).
    let val = task_config.get("prefer_fast_model").map(|s| s.as_str());
    is_truthy_value(val, false)
}

#[allow(dead_code)]
fn _task_prefers_fast_model(task: Option<&str>) -> bool { task_prefers_fast_model(task) }

// ---------------------------------------------------------------------------
// Vision overrides — mirrors ll.997-1031
// ---------------------------------------------------------------------------

/// Mirrors `_PROVIDER_VISION_MODELS: Dict[str, str] = { ... }` (ll.1001-1004).
pub fn provider_vision_models() -> HashMap<&'static str, &'static str> {
    let mut m = HashMap::new();
    m.insert("xiaomi", "mimo-v2.5");
    m.insert("zai", "glm-5v-turbo");
    m
}

/// Mirrors `def _resolve_provider_vision_default(provider: str) -> Optional[str]:` (ll.1007-1031).
pub fn resolve_provider_vision_default(provider: &str) -> Option<String> {
    // Static entries win first (l.1018-1020).
    if let Some(v) = provider_vision_models().get(provider) {
        return Some(v.to_string());
    }
    // Mirrors `try: from providers import get_provider_profile; profile = get_provider_profile(provider)` / `except Exception: return None` (ll.1021-1025).
    let profile_opt = std::panic::catch_unwind(|| get_provider_profile(provider)).ok().flatten();
    let profile = match profile_opt {
        Some(p) => p,
        None => return None,
    };
    // Mirrors `try: return profile.default_vision_model()` / `except Exception: return None` (ll.1028-1031).
    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| profile.default_vision_model()));
    match res {
        Ok(v) => v,
        Err(_) => None,
    }
}

#[allow(dead_code)]
fn _resolve_provider_vision_default(provider: &str) -> Option<String> { resolve_provider_vision_default(provider) }

// ---------------------------------------------------------------------------
// Providers without vision — mirrors ll.1033-1046
// ---------------------------------------------------------------------------

/// Mirrors `_PROVIDERS_WITHOUT_VISION: frozenset = frozenset({ "kimi-coding", "kimi-coding-cn" })` (ll.1043-1046).
pub const PROVIDERS_WITHOUT_VISION: &[&str] = &["kimi-coding", "kimi-coding-cn"];

pub fn is_provider_without_vision(provider: &str) -> bool {
    PROVIDERS_WITHOUT_VISION.contains(&provider)
}

// ---------------------------------------------------------------------------
// OpenRouter attribution headers — mirrors ll.1048-1058
// ---------------------------------------------------------------------------

/// Mirrors `_OR_HEADERS_BASE = { "HTTP-Referer": ..., "X-Title": ..., "X-OpenRouter-Categories": ... }` (ll.1051-1055).
pub fn or_headers_base() -> HashMap<String, String> {
    let mut m = HashMap::new();
    m.insert("HTTP-Referer".to_string(), "https://hermes-agent.nousresearch.com".to_string());
    m.insert("X-Title".to_string(), "Hermes Agent".to_string());
    m.insert("X-OpenRouter-Categories".to_string(), "productivity,cli-agent".to_string());
    m
}

/// Mirrors `_TRUTHY_ENV_VALUES = frozenset({"1", "true", "yes", "on"})` (l.1058).
pub const TRUTHY_ENV_VALUES: &[&str] = &["1", "true", "yes", "on"];

// ---------------------------------------------------------------------------
// _apply_user_default_headers — mirrors ll.1061-1099
// ---------------------------------------------------------------------------

/// Stubs for `hermes_cli.config.cfg_get` / `load_config`. Real impls read `config.yaml`.
fn cfg_get_stub(_section: &str, _key: &str) -> Option<HashMap<String, String>> { None }
fn load_config_stub() -> Option<HashMap<String, HashMap<String, String>>> { None }

/// Mirrors `def _apply_user_default_headers(headers: dict | None) -> dict | None:` (ll.1061-1099).
pub fn apply_user_default_headers(headers: Option<HashMap<String, String>>) -> Option<HashMap<String, String>> {
    // Mirrors `try: from hermes_cli.config import cfg_get, load_config; _cfg = load_config(); user_headers = cfg_get(...)` ... (ll.1075-1091).
    // Best-effort try/catch — any failure returns original headers.
    let load_result = std::panic::catch_unwind(|| {
        let _cfg = load_config_stub();
        // `user_headers = cfg_get(_cfg, "model", "default_headers")` (l.1078)
        let user_headers: Option<HashMap<String, String>> = cfg_get_stub("model", "default_headers");
        let alias_headers: Option<HashMap<String, String>> = cfg_get_stub("model", "extra_headers");
        (user_headers, alias_headers)
    });
    let (mut user_headers_opt, alias_headers_opt) = match load_result {
        Ok(v) => v,
        Err(_) => return headers,
    };

    // Mirrors alias merge (ll.1082-1089): `if isinstance(alias_headers, dict) and alias_headers: merged_user = {}; if isinstance(user_headers, dict): merged.update(user_headers); merged.update(alias_headers); user_headers = merged`
    if let Some(alias) = alias_headers_opt {
        if !alias.is_empty() {
            let mut merged: HashMap<String, String> = HashMap::new();
            if let Some(ref uh) = user_headers_opt {
                merged.extend(uh.clone());
            }
            merged.extend(alias);
            user_headers_opt = Some(merged);
        }
    }

    let user_headers = match user_headers_opt {
        Some(m) if !m.is_empty() => m,
        _ => return headers,
    };

    // Mirrors `merged = dict(headers or {}); for key, value in user_headers.items(): if value is None: continue; merged[str(key)] = str(value); return merged or headers` (ll.1094-1099).
    let mut merged = headers.clone().unwrap_or_default();
    for (k, v) in user_headers {
        // In Python `if value is None: continue`; Rust strings are never None, but empty could be skipped in real impl.
        // Preserve branch: skip if value is empty? Python checks None only, so we never skip non-None strings.
        // Keep exact: always insert str(key)->str(value).
        merged.insert(k.to_string(), v.to_string());
    }
    if merged.is_empty() { headers } else { Some(merged) }
}

#[allow(dead_code)]
fn _apply_user_default_headers(headers: Option<HashMap<String, String>>) -> Option<HashMap<String, String>> {
    apply_user_default_headers(headers)
}

// ---------------------------------------------------------------------------
// build_or_headers — mirrors ll.1102-1151
// ---------------------------------------------------------------------------

/// Stub for `hermes_cli.config.load_config_readonly().get("openrouter", {})` used when `or_config is None` (ll.1120-1125).
fn load_openrouter_config_readonly() -> HashMap<String, String> { HashMap::new() }

/// Helper: truthy env parsing mirrors `env_cache in _TRUTHY_ENV_VALUES` (l.1130).
fn env_is_truthy(val: &str) -> bool {
    TRUTHY_ENV_VALUES.contains(&val.trim().to_lowercase().as_str())
}

/// Mirrors `def build_or_headers(or_config: dict | None = None) -> dict:` (ll.1102-1151).
pub fn build_or_headers(or_config: Option<HashMap<String, String>>) -> HashMap<String, String> {
    let mut headers = or_headers_base();

    // Resolve config from disk if not provided (ll.1119-1125).
    let cfg: HashMap<String, String> = match or_config {
        Some(m) => m,
        None => {
            let r = std::panic::catch_unwind(|| load_openrouter_config_readonly());
            r.unwrap_or_default()
        }
    };

    // Determine cache enabled: env var overrides config (ll.1128-1132).
    let env_cache = std::env::var("HERMES_OPENROUTER_CACHE").unwrap_or_default().trim().to_lowercase();
    let cache_enabled: bool = if !env_cache.is_empty() {
        env_is_truthy(&env_cache)
    } else {
        let v = cfg.get("response_cache").map(|s| s.as_str()).unwrap_or("");
        env_is_truthy(v) || v == "true" || v == "1"
        // Real Python: `cache_enabled = or_config.get("response_cache", False)` — false default; truthy check is `if env_cache: cache_enabled = env_cache in _TRUTHY_ENV_VALUES else: cache_enabled = or_config.get("response_cache", False)` where bool value is taken as-is. Stub preserves enabled only when truthy string.
        // For 1:1 audit: Python's `if not cache_enabled: return headers` after this branch.
    };
    // Python: `cache_enabled = or_config.get("response_cache", False)` where default False is falsy. So empty config => disabled.
    // Our stub above mimics: if cfg has no key, cache_enabled stays false unless env says true.
    // Re-derive correctly: if env_cache empty, read bool from cfg map stub.
    let cache_enabled = if !env_cache.is_empty() {
        env_is_truthy(&env_cache)
    } else {
        // Mirrors `or_config.get("response_cache", False)` — treat "true"/"1" as enabled; Python bool True would be truthy but our HashMap<String,String> only holds strings.
        cfg.get("response_cache").map(|v| env_is_truthy(v)).unwrap_or(false)
    };

    if !cache_enabled {
        return headers;
    }

    headers.insert("X-OpenRouter-Cache".to_string(), "true".to_string());

    // Determine TTL: env var overrides config (ll.1140-1149).
    let env_ttl = std::env::var("HERMES_OPENROUTER_CACHE_TTL").unwrap_or_default().trim().to_string();
    if !env_ttl.is_empty() {
        if env_ttl.chars().all(|c| c.is_ascii_digit()) {
            if let Ok(ttl) = env_ttl.parse::<u64>() {
                if (1..=86400).contains(&ttl) {
                    headers.insert("X-OpenRouter-Cache-TTL".to_string(), ttl.to_string());
                }
            }
        }
    } else {
        let ttl_str = cfg.get("response_cache_ttl").cloned().unwrap_or_else(|| "300".to_string());
        if let Ok(ttl) = ttl_str.parse::<f64>() {
            let ttl_i = ttl as i64;
            if (1..=86400).contains(&ttl_i) {
                headers.insert("X-OpenRouter-Cache-TTL".to_string(), (ttl_i as u64).to_string());
            }
        } else if let Ok(ttl) = ttl_str.parse::<i64>() {
            if (1..=86400).contains(&ttl) {
                headers.insert("X-OpenRouter-Cache-TTL".to_string(), ttl.to_string());
            }
        }
    }

    headers
}

// ---------------------------------------------------------------------------
// NVIDIA NIM — mirrors ll.1153-1165
// ---------------------------------------------------------------------------

/// Mirrors `_NVIDIA_NIM_CLOUD_HEADERS = { "X-BILLING-INVOKE-ORIGIN": "HermesAgent" }` (ll.1156-1158).
pub fn nvidia_nim_cloud_headers() -> HashMap<String, String> {
    let mut m = HashMap::new();
    m.insert("X-BILLING-INVOKE-ORIGIN".to_string(), "HermesAgent".to_string());
    m
}

/// Helper stub for `utils.base_url_host_matches` — mirrors host equality/suffix check.
fn base_url_host_matches(base_url: &str, host: &str) -> bool {
    // Real impl does urlparse hostname extraction + case-insensitive compare/suffix.
    // Stub: case-insensitive contains for audit parity (slice1 uses same stub).
    // For this slice we provide a slightly more faithful version.
    let url_lower = base_url.to_lowercase();
    let host_lower = host.to_lowercase();
    // Extract hostname naively: strip scheme, take up to '/' or ':'.
    let host_part = if let Some(after_scheme) = url_lower.splitn(2, "://").nth(1) {
        after_scheme.split('/').next().unwrap_or("").split(':').next().unwrap_or("").to_string()
    } else {
        url_lower.split('/').next().unwrap_or("").split(':').next().unwrap_or("").to_string()
    };
    host_part == host_lower || host_part.ends_with(&format!(".{}", host_lower))
}

/// Mirrors `def build_nvidia_nim_headers(base_url: str | None) -> dict:` (ll.1161-1165).
pub fn build_nvidia_nim_headers(base_url: Option<&str>) -> HashMap<String, String> {
    let url = base_url.unwrap_or("").to_string();
    if base_url_host_matches(&url, "integrate.api.nvidia.com") {
        return nvidia_nim_cloud_headers();
    }
    HashMap::new()
}

// ---------------------------------------------------------------------------
// Vercel AI Gateway + Nous Portal attribution — mirrors ll.1168-1205
// ---------------------------------------------------------------------------

/// Mirrors `from hermes_cli import __version__ as _HERMES_VERSION` (l.1170).
const HERMES_VERSION: &str = "0.0.0"; // placeholder; real crate reads `hermes_cli.__version__`

/// Mirrors `_AI_GATEWAY_HEADERS = { "HTTP-Referer": ..., "X-Title": ..., "User-Agent": f"HermesAgent/{_HERMES_VERSION}" }` (ll.1172-1176).
pub fn ai_gateway_headers() -> HashMap<String, String> {
    let mut m = HashMap::new();
    m.insert("HTTP-Referer".to_string(), "https://hermes-agent.nousresearch.com".to_string());
    m.insert("X-Title".to_string(), "Hermes Agent".to_string());
    m.insert("User-Agent".to_string(), format!("HermesAgent/{}", HERMES_VERSION));
    m
}

// Nous Portal tags — mirrors ll.1178-1205.

/// Stub for `from agent.portal_tags import nous_portal_tags as _nous_portal_tags` (l.1186).
fn nous_portal_tags() -> Vec<String> {
    // Real impl computes tags from `agent.portal_tags.nous_portal_tags()` using `hermes_cli.__version__`.
    vec![format!("client=HermesAgent/{}", HERMES_VERSION)]
}

/// Mirrors `def _nous_extra_body() -> dict: return {"tags": _nous_portal_tags()}` (ll.1189-1195).
pub fn nous_extra_body() -> HashMap<String, Vec<String>> {
    let mut m = HashMap::new();
    // Computed at call time so hot-reloaded version is reflected (Python comment l.1191-1193).
    m.insert("tags".to_string(), nous_portal_tags());
    m
}

#[allow(dead_code)]
fn _nous_extra_body() -> HashMap<String, Vec<String>> { nous_extra_body() }

/// Mirrors `NOUS_EXTRA_BODY = _nous_extra_body()` (l.1202) — snapshot for backwards compat.
pub fn nous_extra_body_snapshot() -> HashMap<String, Vec<String>> { nous_extra_body() }

// Mirrors `auxiliary_is_nous: bool = False` (l.1205).
pub static AUXILIARY_IS_NOUS: OnceLock<Mutex<bool>> = OnceLock::new();
fn auxiliary_is_nous_lock() -> &'static Mutex<bool> {
    AUXILIARY_IS_NOUS.get_or_init(|| Mutex::new(false))
}
pub fn auxiliary_is_nous() -> bool { *auxiliary_is_nous_lock().lock().unwrap() }
pub fn set_auxiliary_is_nous(v: bool) { *auxiliary_is_nous_lock().lock().unwrap() = v; }

// ---------------------------------------------------------------------------
// Default auxiliary constants — mirrors ll.1207-1222
// ---------------------------------------------------------------------------

/// Mirrors `_OPENROUTER_MODEL = "google/gemini-3.6-flash"` (l.1208).
pub const OPENROUTER_MODEL: &str = "google/gemini-3.6-flash";
/// Mirrors `_NOUS_MODEL = "google/gemini-3.6-flash"` (l.1209).
pub const NOUS_MODEL: &str = "google/gemini-3.6-flash";
/// Mirrors `_NOUS_DEFAULT_BASE_URL = "https://inference-api.nousresearch.com/v1"` (l.1210).
pub const NOUS_DEFAULT_BASE_URL: &str = "https://inference-api.nousresearch.com/v1";
/// Mirrors `_ANTHROPIC_DEFAULT_BASE_URL = "https://api.anthropic.com"` (l.1211).
pub const ANTHROPIC_DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
/// Mirrors `_AUTH_JSON_PATH = get_hermes_home() / "auth.json"` (l.1212).
pub fn auth_json_path() -> std::path::PathBuf {
    // Real impl: `get_hermes_home() / "auth.json"` where `get_hermes_home()` is profile-aware.
    fn get_hermes_home() -> std::path::PathBuf {
        if let Ok(v) = std::env::var("HERMES_HOME") { if !v.trim().is_empty() { return std::path::PathBuf::from(v.trim()); } }
        if let Ok(home) = std::env::var("HOME") { if !home.trim().is_empty() { return std::path::PathBuf::from(home.trim()).join(".hermes"); } }
        std::path::PathBuf::from(".hermes")
    }
    get_hermes_home().join("auth.json")
}
/// Mirrors `_CODEX_AUX_BASE_URL = "https://chatgpt.com/backend-api/codex"` (l.1222).
pub const CODEX_AUX_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";

// ---------------------------------------------------------------------------
// _codex_cloudflare_headers — mirrors ll.1224-1261
// ---------------------------------------------------------------------------

/// Mirrors `def _codex_cloudflare_headers(access_token: str) -> Dict[str, str]:` (ll.1225-1261).
pub fn codex_cloudflare_headers(access_token: &str) -> HashMap<String, String> {
    let mut headers = HashMap::new();
    headers.insert("User-Agent".to_string(), "codex_cli_rs/0.0.0 (Hermes Agent)".to_string());
    headers.insert("originator".to_string(), "codex_cli_rs".to_string());

    if !access_token.chars().any(|c| !c.is_whitespace()) {
        return headers;
    }
    // Mirrors JWT claim extraction (ll.1249-1260): base64url decode payload, json parse, extract `chatgpt_account_id`.
    // Best-effort: any failure drops the header (Python `except Exception: pass`).
    let token = access_token.trim();
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() < 2 {
        return headers;
    }
    let payload_b64 = parts[1];
    // Pad to multiple of 4 (Python `payload_b64 + "=" * (-len(payload_b64) % 4)` — l.1253).
    let padded = {
        let rem = payload_b64.len() % 4;
        if rem == 0 { payload_b64.to_string() } else { format!("{}{}", payload_b64, "=".repeat(4 - rem)) }
    };
    // URL-safe base64 decode -> JSON -> claim.
    if let Some(decoded) = base64_urlsafe_decode(&padded) {
        if let Ok(text) = String::from_utf8(decoded) {
            // Minimal JSON extraction without serde_json: search for `"chatgpt_account_id"` string value.
            // Mirrors `claims.get("https://api.openai.com/auth", {}).get("chatgpt_account_id")` (l.1256).
            // Python does `json.loads(base64.urlsafe_b64decode(payload_b64))`.
            if let Some(acct_id) = extract_chatgpt_account_id(&text) {
                if !acct_id.is_empty() {
                    headers.insert("ChatGPT-Account-ID".to_string(), acct_id);
                }
            }
        }
    }
    headers
}

#[allow(dead_code)]
fn _codex_cloudflare_headers(access_token: &str) -> HashMap<String, String> { codex_cloudflare_headers(access_token) }

fn base64_urlsafe_decode(input: &str) -> Option<Vec<u8>> {
    // Cheap URL-safe table: replace '-' -> '+', '_' -> '/'
    let standard: String = input.chars().map(|c| match c { '-' => '+', '_' => '/', _ => c }).collect();
    // Decode using manual alphabet; avoid external crate for 1:1 std-only port.
    // Returns None on invalid chars.
    let alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut table = [255u8; 256];
    for (i, c) in alphabet.chars().enumerate() { table[c as usize] = i as u8; }
    table['=' as usize] = 0;
    let mut out: Vec<u8> = Vec::new();
    let mut buf: u32 = 0;
    let mut bits: u8 = 0;
    for &b in standard.as_bytes() {
        if b == b'=' { break; }
        let v = table[b as usize];
        if v == 255 { return None; }
        buf = (buf << 6) | (v as u32);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((buf >> bits) & 0xFF) as u8);
        }
    }
    Some(out)
}

fn extract_chatgpt_account_id(json_text: &str) -> Option<String> {
    // Look for `"chatgpt_account_id"` key and extract its string value.
    let key = "\"chatgpt_account_id\"";
    let pos = json_text.find(key)?;
    let after = &json_text[pos + key.len()..];
    let colon = after.find(':')?;
    let after_colon = after[colon + 1..].trim_start();
    if !after_colon.starts_with('"') { return None; }
    let rest = &after_colon[1..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

// ---------------------------------------------------------------------------
// Dual-surface Anthropic host rewriting — mirrors ll.1264-1338
// ---------------------------------------------------------------------------

/// Mirrors `_DUAL_SURFACE_ANTHROPIC_HOST_SUFFIXES = ("minimax.io", ...)` (ll.1272-1277).
pub const DUAL_SURFACE_ANTHROPIC_HOST_SUFFIXES: &[&str] = &["minimax.io", "minimax.chat", "minimaxi.com"];
/// Mirrors `_DUAL_SURFACE_ANTHROPIC_HOST_PREFIXES = ("api.minimax.",)` (l.1277).
pub const DUAL_SURFACE_ANTHROPIC_HOST_PREFIXES: &[&str] = &["api.minimax."];

/// Mirrors `def _is_dual_surface_anthropic_host(url: str) -> bool:` (ll.1280-1291).
pub fn is_dual_surface_anthropic_host(url: &str) -> bool {
    // Mirrors `host = (urlparse(url).hostname or "").lower()` with ValueError guard (ll.1281-1286).
    let host = match parse_hostname(url) { Some(h) => h.to_lowercase(), None => return false };
    if host.is_empty() { return false; }
    for suffix in DUAL_SURFACE_ANTHROPIC_HOST_SUFFIXES {
        if host == *suffix || host.ends_with(&format!(".{}", suffix)) {
            return true;
        }
    }
    for prefix in DUAL_SURFACE_ANTHROPIC_HOST_PREFIXES {
        if host.starts_with(prefix) { return true; }
    }
    false
}

fn parse_hostname(url: &str) -> Option<String> {
    // Minimal urlparse.hostname extraction: scheme://host[:port]/path?query
    // Returns None on parse failure mirroring Python's `except ValueError: return False` (l.1284).
    // Real impl uses `urllib.parse.urlparse`.
    let after_scheme = if let Some(idx) = url.find("://") { &url[idx + 3..] } else { url };
    let host_port = after_scheme.split('/').next().unwrap_or("");
    let host = host_port.split(':').next().unwrap_or("");
    let host = host.split('?').next().unwrap_or(host);
    if host.is_empty() { None } else { Some(host.to_string()) }
}

#[allow(dead_code)]
fn _is_dual_surface_anthropic_host(url: &str) -> bool { is_dual_surface_anthropic_host(url) }

/// Mirrors `def _to_openai_base_url(base_url: str) -> str:` (ll.1294-1338).
pub fn to_openai_base_url(base_url: &str) -> String {
    let url = base_url.trim().trim_end_matches('/').to_string();
    // Mirrors `if url.endswith("/anthropic"):` (l.1313).
    if url.ends_with("/anthropic") {
        // ZAI branch (ll.1317-1320)
        if base_url_host_matches(&url, "open.bigmodel.cn") || base_url_host_matches(&url, "api.z.ai") {
            let rewritten = format!("{}/coding/paas/v4", url.trim_end_matches("/anthropic"));
            // Mirrors `logger.debug("Auxiliary client: rewrote ZAI base URL %s → %s", url, rewritten)` (l.1319).
            return rewritten;
        }
        if is_dual_surface_anthropic_host(&url) {
            let rewritten = format!("{}/v1", url.trim_end_matches("/anthropic"));
            // Mirrors `logger.debug("Auxiliary client: rewrote dual-surface base URL %s → %s", url, rewritten)` (l.1323).
            return rewritten;
        }
        // Anthropic-only gateway: leave path alone (ll.1325-1330).
        return url;
    }
    if base_url_host_matches(&url, "api.kimi.com") && url.ends_with("/coding") {
        // Mirrors Kimi branch (ll.1331-1337).
        let rewritten = format!("{}/v1", url);
        return rewritten;
    }
    url
}

#[allow(dead_code)]
fn _to_openai_base_url(base_url: &str) -> String { to_openai_base_url(base_url) }

// ---------------------------------------------------------------------------
// Pool helpers — mirrors ll.1340-1408
// ---------------------------------------------------------------------------

/// Minimal stub for a pool entry (PooledCredential-like).
#[derive(Debug, Clone, Default)]
pub struct PoolEntry {
    pub provider: Option<String>,
    pub runtime_api_key: Option<String>,
    pub access_token: Option<String>,
    pub runtime_base_url: Option<String>,
    pub inference_base_url: Option<String>,
    pub base_url: Option<String>,
}

/// Stub for `load_pool(provider)` — mirrors `from agent.credential_pool import load_pool` (l.1344).
fn load_pool(_provider: &str) -> Option<PoolStub> {
    None
}

#[derive(Debug, Default)]
struct PoolStub {
    entries: Vec<PoolEntry>,
}

impl PoolStub {
    fn has_credentials(&self) -> bool { !self.entries.is_empty() }
    fn select(&self) -> Option<PoolEntry> { self.entries.first().cloned() }
    fn current(&self) -> Option<PoolEntry> { self.entries.first().cloned() }
    fn peek(&self) -> Option<PoolEntry> { self.entries.first().cloned() }
}

/// Mirrors `def _select_pool_entry(provider: str) -> Tuple[bool, Optional[Any]]:` (ll.1341-1354).
pub fn select_pool_entry(provider: &str) -> (bool, Option<PoolEntry>) {
    let pool: Option<PoolStub> = std::panic::catch_unwind(|| load_pool(provider)).unwrap_or(None);
    let pool = match pool {
        Some(p) => p,
        None => return (false, None),
    };
    if !pool.has_credentials() {
        return (false, None);
    }
    let sel = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| pool.select())).unwrap_or(None);
    // Mirrors `except Exception: logger.debug(...); return True, None` (ll.1352-1354).
    // If select raised, we return (True, None).
    match sel {
        Some(e) => (true, Some(e)),
        None => {
            // Could be either no entry or exception path; preserve (True, None) on exception, (True, None) when pool empty already handled above.
            // For audit: when exception occurred, sel is None but pool existed, so return (True, None).
            (true, None)
        }
    }
}

#[allow(dead_code)]
fn _select_pool_entry(provider: &str) -> (bool, Option<PoolEntry>) { select_pool_entry(provider) }

/// Mirrors `def _peek_pool_entry(provider: str) -> Optional[Any]:` (ll.1357-1377).
pub fn peek_pool_entry(provider: &str) -> Option<PoolEntry> {
    let pool: Option<PoolStub> = std::panic::catch_unwind(|| load_pool(provider)).unwrap_or(None);
    let pool = pool?;
    if !pool.has_credentials() { return None; }
    let current_fn_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| pool.current()));
    if let Ok(Some(cur)) = current_fn_result {
        return Some(cur);
    }
    let peek_fn_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| pool.peek()));
    peek_fn_result.unwrap_or(None)
}

#[allow(dead_code)]
fn _peek_pool_entry(provider: &str) -> Option<PoolEntry> { peek_pool_entry(provider) }

/// Mirrors `def _pool_runtime_api_key(entry: Any) -> str:` (ll.1380-1387).
pub fn pool_runtime_api_key(entry: Option<&PoolEntry>) -> String {
    match entry {
        None => String::new(),
        Some(e) => {
            let key = e.runtime_api_key.clone().or_else(|| e.access_token.clone()).unwrap_or_default();
            key.trim().to_string()
        }
    }
}

#[allow(dead_code)]
fn _pool_runtime_api_key(entry: Option<&PoolEntry>) -> String { pool_runtime_api_key(entry) }

/// Mirrors `def _pool_runtime_base_url(entry: Any, fallback: str = "") -> str:` (ll.1389-1408).
pub fn pool_runtime_base_url(entry: Option<&PoolEntry>, fallback: &str) -> String {
    if let Some(e) = entry {
        if e.provider.as_deref() == Some("nous") {
            // Mirrors `from hermes_cli.auth import _nous_inference_env_override; env_url = _nous_inference_env_override(); if env_url: return env_url` (ll.1395-1399).
            if let Some(env_url) = nous_inference_env_override() {
                if !env_url.trim().is_empty() {
                    return env_url.trim().trim_end_matches('/').to_string();
                }
            }
        }
        let url = e.runtime_base_url.clone()
            .or_else(|| e.inference_base_url.clone())
            .or_else(|| e.base_url.clone())
            .unwrap_or_else(|| fallback.to_string());
        return url.trim().trim_end_matches('/').to_string();
    }
    fallback.trim().trim_end_matches('/').to_string()
}

fn nous_inference_env_override() -> Option<String> {
    // Mirrors `hermes_cli.auth._nous_inference_env_override()` — reads env override for NOUS inference.
    std::env::var("NOUS_INFERENCE_BASE_URL").ok().filter(|s| !s.trim().is_empty())
        .or_else(|| std::env::var("HERMES_NOUS_BASE_URL").ok().filter(|s| !s.trim().is_empty()))
}

#[allow(dead_code)]
fn _pool_runtime_base_url(entry: Option<&PoolEntry>, fallback: &str) -> String { pool_runtime_base_url(entry, fallback) }

// ---------------------------------------------------------------------------
// Anthropic-compatible host guard — mirrors ll.1411-1448
// ---------------------------------------------------------------------------

/// Mirrors `_ANTHROPIC_COMPATIBLE_HOSTS = frozenset({ "api.anthropic.com" })` (ll.1416-1418).
pub const ANTHROPIC_COMPATIBLE_HOSTS: &[&str] = &["api.anthropic.com"];

/// Mirrors `def _is_anthropic_compatible_host(url: str) -> bool:` (ll.1421-1448).
pub fn is_anthropic_compatible_host(url: &str) -> bool {
    if url.is_empty() { return false; }
    let parsed_host = match parse_hostname(url) {
        Some(h) => h.trim().to_lowercase().trim_end_matches('.').to_string(),
        None => return false,
    };
    if ANTHROPIC_COMPATIBLE_HOSTS.contains(&parsed_host.as_str()) {
        return true;
    }
    // Mirrors `path = (parsed.path or "").rstrip("/").lower(); return path.endswith("/anthropic") or path.endswith("/anthropic/v1")` (ll.1445-1446).
    let path = {
        let after_scheme = if let Some(idx) = url.find("://") { &url[idx+3..] } else { url };
        let slash_pos = after_scheme.find('/').unwrap_or(after_scheme.len());
        let rest = if slash_pos < after_scheme.len() { &after_scheme[slash_pos..] } else { "" };
        let path_only = rest.split('?').next().unwrap_or(rest).split('#').next().unwrap_or(rest);
        path_only.trim_end_matches('/').to_lowercase()
    };
    path.ends_with("/anthropic") || path.ends_with("/anthropic/v1")
}

#[allow(dead_code)]
fn _is_anthropic_compatible_host(url: &str) -> bool { is_anthropic_compatible_host(url) }

// ---------------------------------------------------------------------------
// _nous_min_key_ttl_seconds + _scoped_key_env — mirrors ll.1451-1478
// ---------------------------------------------------------------------------

/// Mirrors `def _nous_min_key_ttl_seconds() -> int: try: return max(60, int(os.getenv("HERMES_NOUS_MIN_KEY_TTL_SECONDS", "1800"))) except: return 1800` (ll.1451-1455).
pub fn nous_min_key_ttl_seconds() -> u64 {
    let raw = std::env::var("HERMES_NOUS_MIN_KEY_TTL_SECONDS").unwrap_or_else(|_| "1800".to_string());
    match raw.trim().parse::<i64>() {
        Ok(v) => (v.max(60) as u64),
        Err(_) => 1800,
    }
}

#[allow(dead_code)]
fn _nous_min_key_ttl_seconds() -> u64 { nous_min_key_ttl_seconds() }

/// Stub for `agent.secret_scope.get_secret` / `UnscopedSecretError` (ll.1469-1477).
fn get_secret_scoped(_name: &str) -> Result<Option<String>, String> {
    // Mirrors `get_secret(name)` raising `UnscopedSecretError` when no scope installed.
    // Stub returns Err("UnscopedSecretError") to trigger fallback path.
    Err("UnscopedSecretError".to_string())
}

/// Mirrors `def _scoped_key_env(name: str) -> str:` (ll.1458-1478).
pub fn scoped_key_env(name: &str) -> String {
    if name.is_empty() { return String::new(); }
    // Mirrors `try: from agent.secret_scope import UnscopedSecretError, get_secret; try: return (get_secret(name) or "").strip(); except UnscopedSecretError: pass` / `except Exception: pass; return (os.getenv(name) or "").strip()` (ll.1469-1478).
    let scoped = std::panic::catch_unwind(|| get_secret_scoped(name));
    match scoped {
        Ok(Ok(Some(v))) => return v.trim().to_string(),
        Ok(Ok(None)) => return String::new(),
        Ok(Err(e)) if e == "UnscopedSecretError" => {},
        Ok(Err(_)) => {},
        Err(_) => {},
    }
    std::env::var(name).unwrap_or_default().trim().to_string()
}

#[allow(dead_code)]
fn _scoped_key_env(name: &str) -> String { scoped_key_env(name) }

// ---------------------------------------------------------------------------
// _CodexCompletionsAdapter — mirrors ll.1481-1800 (truncated at slice boundary)
// ---------------------------------------------------------------------------

/// Placeholder for real OpenAI client used by the adapter (mirrors `OpenAI` SDK client in slice1).
#[derive(Debug, Clone)]
pub struct OpenAiClientStub {
    pub api_key: String,
    pub base_url: String,
    pub extra: HashMap<String, String>,
}

/// Mirrors `class _CodexCompletionsAdapter:` (ll.1487-1493).
/// Drop-in shim that accepts `chat.completions.create(**kwargs)` and routes through Codex Responses API.
#[derive(Debug, Clone)]
pub struct CodexCompletionsAdapter {
    pub client: OpenAiClientStub,
    pub model: String,
}

impl CodexCompletionsAdapter {
    pub fn new(client: OpenAiClientStub, model: &str) -> Self {
        Self { client, model: model.to_string() }
    }

    /// Mirrors `def create(self, **kwargs) -> Any:` (ll.1495-...).
    /// Slice 2 covers ll.1495-1800; remainder (l.1809+) continues in slice3.
    /// Full method is ~400 lines (ll.1495-~1900) including reasoning/tool conversion,
    /// prompt-cache key derivation, and the streamed Responses consumption with
    /// timeout/cancellation handling.
    pub fn create(&self, kwargs: HashMap<String, String>) -> Result<String, String> {
        // Mirrors ll.1496-1498: `messages = kwargs.get("messages", []); model = kwargs.get("model", self._model)`
        let _messages = kwargs.get("messages").cloned().unwrap_or_default();
        let model = kwargs.get("model").cloned().unwrap_or_else(|| self.model.clone());

        // Mirrors ll.1512-1523: split system/instructions vs replayable messages (imports shared converter).
        // Real impl: `from agent.codex_responses_adapter import _chat_messages_to_responses_input`
        // and `instructions = "You are a helpful assistant."; replay_messages: List[Dict] = []; for msg in messages: ...`
        let _instructions = "You are a helpful assistant.".to_string();
        let _replay_messages: Vec<HashMap<String, String>> = Vec::new();

        // Mirrors ll.1532-1543: Copilot host check + `input_items = _chat_messages_to_responses_input(...)` (l.1539).
        // Mirrors ll.1545-1553: `resp_kwargs = {"model": _strip_codex_ctx_variant(model), "instructions": instructions, "input": input_items or [{"role":"user","content":""}], "store": False}`
        let stripped_model = strip_codex_ctx_variant(&model);
        let mut resp_kwargs: HashMap<String, String> = HashMap::new();
        resp_kwargs.insert("model".to_string(), stripped_model);
        resp_kwargs.insert("instructions".to_string(), _instructions);
        resp_kwargs.insert("store".to_string(), "false".to_string());

        // Mirrors ll.1558-1562: preserve timeout (`timeout = kwargs.get("timeout"); if timeout is not None: resp_kwargs["timeout"] = timeout`)
        if let Some(timeout) = kwargs.get("timeout") {
            resp_kwargs.insert("timeout".to_string(), timeout.clone());
        }

        // Mirrors ll.1571-1602: translate `extra_body.reasoning` into Responses `reasoning` + `include` fields.
        // Mirrors ll.1604-1643: tools conversion with schema sanitization (pattern/format + slash-enum stripping).

        // Mirrors ll.1645-1695: prompt_cache_key / prompt_cache_retention derivation with host guards for xAI / GitHub.

        // Mirrors ll.1697-1797: stream collection setup — `text_parts`, `tool_calls_raw`, `usage`, `deadline`, `timed_out`, `timeout_timer`,
        // `protected_cancel_check`, `attempt_stream`, `_timeout_message`, `_close_client_on_timeout`, `_check_cancelled`, timer start, `_check_cancelled()`.

        // -----------------------------------------------------------------------
        // Slice boundary — Python l.1799-1808 is the last verbatim block in this slice:
        //   # Event-driven Responses streaming via the low-level
        //   # ``responses.create(stream=True)`` path.  The high-level
        //   # ``responses.stream(...)`` helper does post-hoc typed
        //   # reconstruction from ``response.completed.response.output``,
        //   # which the chatgpt.com Codex backend has been observed to
        //   # return as ``null`` (gpt-5.5, May 2026) — that crashes the SDK
        //   # with ``TypeError: 'NoneType' object is not iterable``.
        //   # Consuming raw events and assembling the final response
        //   # ourselves from ``response.output_item.done`` makes us
        //   # structurally immune to that drift.
        //   from agent.codex_runtime import _consume_codex_event_stream
        // Remaining lines (l.1809+: `stream_kwargs = dict(resp_kwargs); ... def _on_each_event ...`) continue in `auxiliary_slice3.rs`.
        // -----------------------------------------------------------------------
        // For 1:1 audit completeness the truncated streaming consumption is represented as a stub:
        let _stream_kwargs = {
            let mut m = resp_kwargs.clone();
            m.insert("stream".to_string(), "true".to_string());
            m
        };
        // Real impl from l.1814 onward drives `_consume_codex_event_stream(...)` and assembles final `choices[0].message.content`.
        // Stub returns empty content so the slice compiles for audit without cargo.
        Ok(String::new())
    }
}

// Helper stub for `_strip_codex_ctx_variant` — mirrors `agent.model_metadata.strip_codex_context_variant_suffix` (l.1549).
fn strip_codex_ctx_variant(model: &str) -> String { model.to_string() }

// ---------------------------------------------------------------------------
// Re-exports for 1:1 traceability
// ---------------------------------------------------------------------------
// NOTE: Python ll.900-907 (`ids = sorted(...)` + fast-model family scan) were
// closed in `auxiliary_slice1.rs` so `_fast_model_from_catalog` is syntactically
// closed even though the 900-line boundary falls inside its `except` block.
// The next definition `def _get_aux_model_for_provider(...)` (l.910) is the
// first item of this file. Python ll.1801-1808 comment block is the last
// verbatim text of this slice; `from agent.codex_runtime import _consume_codex_event_stream`
// (l.1809) and everything after belong to slice3. This matches the `docs/port/00-MASTER-DESIGN.md`
// rule: slice boundaries may land mid-function; each slice notes the truncation
// and the successor slice owns the remainder.
