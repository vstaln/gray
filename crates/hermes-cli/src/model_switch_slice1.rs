//! hermes-cli model_switch — slice 1/5
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/hermes_cli/model_switch.py`
//! slice 1/5 — lines 1–900 of 3 989 (first 900 LOC).
//! Covers: module docstring + imports, `_UNCAPPED_PICKER_PROVIDERS`,
//! `logger`, `_declared_model_ids`, `_entry_models_discovered`,
//! `_models_config_is_allowlist`, `_save_discovered_models_to_config`,
//! `_bare_custom_provider_def`, `_MODEL_DISCOVERY_ERRORS`,
//! `_NativePickerModelList`, `_fetch_picker_live_models`,
//! non-agentic warning (`_HERMES_MODEL_WARNING`, `_NOUS_HERMES_NON_AGENTIC_RE`),
//! opaque model-ID display (`_OPAQUE_MODEL_PREFIXES`, `format_model_for_display`),
//! `is_nous_hermes_non_agentic`, `_check_hermes_model_warning`,
//! `ModelIdentity`, `MODEL_ALIASES`, `DirectAlias`,
//! `_BUILTIN_DIRECT_ALIASES`, `DIRECT_ALIASES`, `_load_direct_aliases`,
//! `_ensure_direct_aliases`, `ModelSwitchResult`, `ModelFlagParseResult`,
//! `parse_model_flags_detailed`, `parse_model_flags`,
//! `resolve_persist_behavior`, `ModelSwitchRequest`,
//! `parse_model_switch_args`, and the opening of
//! `_effective_model_candidate` (through line 900).
//! Continued in `model_switch_slice2.rs` (from line 901).
//!
//! T0690 — 1:1 port, no cargo (NEVER cargo).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

// ---------------------------------------------------------------------------
// Module docstring — mirrors lines 1-19
// ---------------------------------------------------------------------------

/// Shared model-switching logic for CLI and gateway /model commands.
///
/// Both the CLI (cli.py) and gateway (gateway/run.py) /model handlers
/// share the same core pipeline:
///
///   parse flags -> alias resolution -> provider resolution ->
///   credential resolution -> normalize model name ->
///   metadata lookup -> build result
///
/// This module ties together the foundation layers:
///
/// - `agent.models_dev`            — models.dev catalog, ModelInfo, ProviderInfo
/// - `hermes_cli.providers`        — canonical provider identity + overlays
/// - `hermes_cli.model_normalize`  — per-provider name formatting
///
/// Provider switching uses the `--provider` flag exclusively.
/// No colon-based `provider:model` syntax — colons are reserved for
/// OpenRouter variant suffixes (`:free`, `:extended`, `:fast`).
/// Mirrors `hermes_cli/model_switch.py` lines 1-19.
pub const MODULE_DOC: &str =
    "model_switch: shared model-switching logic — see model_switch.py lines 1-19";

// ---------------------------------------------------------------------------
// Imports — mirrors lines 21-51
// ---------------------------------------------------------------------------
// Python: http.client, logging, os, re, time, dataclasses, typing,
// hermes_cli.providers (ProviderDef, custom_provider_aliases, custom_provider_slug,
//   determine_api_mode, get_label, host_mandated_api_mode, is_aggregator, resolve_provider_full),
// hermes_cli.model_normalize (normalize_model_for_provider),
// agent.models_dev (ModelCapabilities, ModelInfo, get_model_capabilities,
//   get_model_info, list_provider_models),
// utils (base_url_host_matches, base_url_hostname)
// Rust: std only (NEVER cargo). External hermes modules are stubbed for 1:1.

// ---------------------------------------------------------------------------
// ProviderDef + models_dev stubs — mirrors import shapes (lines 31-51)
// ---------------------------------------------------------------------------

/// Mirrors `hermes_cli.providers.ProviderDef` (lines 31-40).
#[derive(Debug, Clone)]
pub struct ProviderDef {
    pub id: String,
    pub name: String,
    pub transport: String,
    pub api_key_env_vars: Vec<String>,
    pub base_url: String,
    pub is_aggregator: bool,
    pub auth_type: String,
    pub source: String,
}

/// Mirrors `agent.models_dev.ModelCapabilities` (lines 44-50).
#[derive(Debug, Clone, Default)]
pub struct ModelCapabilities {
    pub context_window: Option<u64>,
    pub supports_tools: bool,
}

/// Mirrors `agent.models_dev.ModelInfo` (lines 44-50).
#[derive(Debug, Clone, Default)]
pub struct ModelInfo {
    pub id: String,
    pub context_window: Option<u64>,
    pub provider: String,
}

// Stub helpers that would come from hermes_cli.providers / agent.models_dev / utils
// Kept as free fns for 1:1 traceability; real wiring in later slices.

pub fn is_aggregator_stub(provider: &str) -> bool {
    // Mirrors `hermes_cli.providers.is_aggregator` — aggregators route vendor/model slugs.
    // Full list lives in providers registry; stub checks known aggregator set.
    matches!(
        provider.to_lowercase().as_str(),
        "openrouter" | "nous" | "opencode-zen" | "opencode-go" | "ai-gateway" | "kilocode" | "gmi"
    )
}
pub fn get_label_stub(provider: &str) -> String { provider.to_string() }
pub fn custom_provider_slug_stub(name: &str) -> String { format!("custom:{name}") }
pub fn determine_api_mode_stub(_provider: &str, _base_url: &str) -> String { "openai_chat".to_string() }
pub fn host_mandated_api_mode_stub(_host: &str) -> Option<String> { None }
pub fn resolve_provider_full_stub(provider: &str) -> Option<ProviderDef> { None::<ProviderDef>; let _ = provider; None }
pub fn custom_provider_aliases_stub() -> HashMap<String, String> { HashMap::new() }
pub fn normalize_model_for_provider_stub(model: &str, _provider: &str) -> String { model.to_string() }
pub fn get_model_info_stub(_model: &str, _provider: Option<&str>) -> Option<ModelInfo> { None }
pub fn get_model_capabilities_stub(_model: &str) -> Option<ModelCapabilities> { None }
pub fn list_provider_models_stub(_provider: &str) -> Vec<String> { Vec::new() }
pub fn base_url_host_matches_stub(_url: &str, _host: &str) -> bool { false }
pub fn base_url_hostname_stub(url: &str) -> String { url.to_string() }

// ---------------------------------------------------------------------------
// _UNCAPPED_PICKER_PROVIDERS — mirrors lines 53-56
// ---------------------------------------------------------------------------

/// Providers whose picker model list should NOT be capped by max_models.
/// OpenCode Zen / Go are aggregators whose full catalogs (70+ models each) must
/// be visible so users can pick any model they have access to.
/// Mirrors `_UNCAPPED_PICKER_PROVIDERS: frozenset[str] = frozenset({"opencode-zen", "opencode-go"})` (53-56).
pub const UNCAPPED_PICKER_PROVIDERS: &[&str] = &["opencode-zen", "opencode-go"];

pub fn is_uncapped_picker_provider(provider: &str) -> bool {
    UNCAPPED_PICKER_PROVIDERS.contains(&provider)
}

// ---------------------------------------------------------------------------
// logger — mirrors line 58
// ---------------------------------------------------------------------------
// Python: logger = logging.getLogger(__name__)
// Rust: no logging crate in slice 1 (NEVER cargo); use eprintln!/stub.
pub fn log_debug(msg: &str) { let _ = msg; }
pub fn log_warn(msg: &str) { eprintln!("[hermes model_switch WARN] {msg}"); }

// ---------------------------------------------------------------------------
// _declared_model_ids — mirrors lines 61-113
// ---------------------------------------------------------------------------

/// Return configured model IDs from supported config shapes.
///
/// Accepts:
/// - `{"model-id": {...}}`
/// - `["model-a", "model-b"]`
/// - `[{"id": "model-a"}, {"name": "model-b"}]`
/// - `"model-a"`
///
/// Mirrors `_declared_model_ids(value: Any) -> list[str]` (61-113).
pub fn declared_model_ids_from_str(value: &str) -> Vec<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() { return Vec::new(); }
    vec![trimmed.to_string()]
}

pub fn declared_model_ids_from_map_keys(value: &HashMap<String, serde_stub::JsonValue>) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for model_id in value.keys() {
        if model_id == "__explicit_model_allowlist__" || model_id == "__discovered_model_catalog__" {
            continue;
        }
        let candidate = model_id.trim();
        if candidate.is_empty() { continue; }
        let lowered = candidate.to_lowercase();
        if seen.contains(&lowered) { continue; }
        seen.insert(lowered);
        ids.push(candidate.to_string());
    }
    ids
}

/// Generic JSON-value variant — mirrors the full Python `value: Any` dispatch.
/// The `str` / `dict` / `list` / scalar branches are unified via `serde_stub::JsonValue`.
pub fn declared_model_ids(value: &serde_stub::JsonValue) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut add = |candidate: &str, ids: &mut Vec<String>, seen: &mut HashSet<String>| {
        let model_id = candidate.trim();
        if model_id.is_empty() { return; }
        let lowered = model_id.to_lowercase();
        if seen.contains(&lowered) { return; }
        seen.insert(lowered);
        ids.push(model_id.to_string());
    };
    match value {
        serde_stub::JsonValue::Str(s) => { add(s, &mut ids, &mut seen); }
        serde_stub::JsonValue::Object(map) => {
            for model_id in map.keys() {
                if model_id == "__explicit_model_allowlist__" || model_id == "__discovered_model_catalog__" {
                    continue;
                }
                add(model_id, &mut ids, &mut seen);
            }
        }
        serde_stub::JsonValue::Array(arr) => {
            for item in arr {
                match item {
                    serde_stub::JsonValue::Str(s) => add(s, &mut ids, &mut seen),
                    serde_stub::JsonValue::Object(map) => {
                        let mut model_id: Option<String> = None;
                        if let Some(serde_stub::JsonValue::Str(s)) = map.get("id") {
                            if !s.trim().is_empty() { model_id = Some(s.clone()); }
                        }
                        if model_id.is_none() {
                            if let Some(serde_stub::JsonValue::Str(s)) = map.get("name") {
                                if !s.trim().is_empty() { model_id = Some(s.clone()); }
                            }
                        }
                        if let Some(mid) = model_id { add(&mid, &mut ids, &mut seen); }
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
    ids
}

// ---------------------------------------------------------------------------
// _entry_models_discovered — mirrors lines 116-133
// ---------------------------------------------------------------------------

/// True when the entry's `models` mapping was auto-discovered by Hermes.
///
/// Mirrors `_entry_models_discovered(entry: Any) -> bool` (116-133).
pub fn entry_models_discovered(entry: &serde_stub::JsonValue) -> bool {
    if let serde_stub::JsonValue::Object(map) = entry {
        if let Some(serde_stub::JsonValue::Bool(true)) = map.get("models_discovered") {
            return true;
        }
        if let Some(serde_stub::JsonValue::Object(models)) = map.get("models") {
            if let Some(serde_stub::JsonValue::Bool(true)) = models.get("__discovered_model_catalog__") {
                return true;
            }
        }
    }
    false
}

// ---------------------------------------------------------------------------
// _models_config_is_allowlist — mirrors lines 136-163
// ---------------------------------------------------------------------------

/// Return True when `models:` is an intentional ID allowlist.
///
/// Mirrors `_models_config_is_allowlist(value: Any, discovered: bool = False) -> bool` (136-163).
pub fn models_config_is_allowlist(value: &serde_stub::JsonValue, discovered: bool) -> bool {
    if discovered { return false; }
    match value {
        serde_stub::JsonValue::Null => false,
        serde_stub::JsonValue::Str(s) => !s.trim().is_empty(),
        serde_stub::JsonValue::Object(_) => false,
        serde_stub::JsonValue::Array(arr) => {
            // Mirrors `bool(_declared_model_ids(value))`
            !declared_model_ids(value).is_empty()
        }
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// _save_discovered_models_to_config — mirrors lines 166-251
// ---------------------------------------------------------------------------

/// Persist discovered models into `custom_providers` in config.yaml.
///
/// Mirrors `_save_discovered_models_to_config(api_url, model_ids, *, api_mode, headers)` (166-251).
/// Matches entries by `base_url` (trailing-slash-normalised). A failed
/// config write is swallowed — the picker still shows the live models for
/// this session. In slice 1 this is a best-effort stub (no config crate linkage,
/// NEVER cargo); real YAML read/write in later slice.
pub fn save_discovered_models_to_config(
    api_url: &str,
    model_ids: &[String],
    api_mode: Option<&str>,
    headers: Option<&HashMap<String, String>>,
) {
    if api_url.trim().is_empty() || model_ids.is_empty() { return; }
    // Would: load_config(), match by base_url/api_mode/headers, migrate legacy sentinel,
    // preserve per-model metadata, only write when stale, save_config().
    // Stub preserves the swallow-on-failure contract and input guards for 1:1 traceability.
    let _ = (api_url, model_ids, api_mode, headers);
    // Best-effort no-op: real persistence when config slice is wired.
}

fn extra_headers_from_config_stub(_entry: &serde_stub::JsonValue) -> HashMap<String, String> {
    HashMap::new()
}

// ---------------------------------------------------------------------------
// _bare_custom_provider_def — mirrors lines 254-268
// ---------------------------------------------------------------------------

/// ProviderDef for a direct `model.provider: custom` endpoint.
/// Mirrors `_bare_custom_provider_def(current_base_url: str) -> Optional[ProviderDef]` (254-268).
pub fn bare_custom_provider_def(current_base_url: &str) -> Option<ProviderDef> {
    let base_url = current_base_url.trim().to_string();
    if base_url.is_empty() { return None; }
    Some(ProviderDef {
        id: "custom".to_string(),
        name: "Custom endpoint".to_string(),
        transport: "openai_chat".to_string(),
        api_key_env_vars: Vec::new(),
        base_url,
        is_aggregator: false,
        auth_type: "api_key".to_string(),
        source: "model-config".to_string(),
    })
}

// ---------------------------------------------------------------------------
// _MODEL_DISCOVERY_ERRORS — mirrors lines 271-279
// ---------------------------------------------------------------------------

/// Mirrors `_MODEL_DISCOVERY_ERRORS = (ImportError, OSError, RuntimeError, TimeoutError, TypeError, ValueError, http.client.HTTPException)` (271-279).
/// In Rust these surface as `String` errors from discovery helpers; this const
/// documents the Python exception tuple for 1:1 audit.
pub const MODEL_DISCOVERY_ERRORS: &[&str] = &[
    "ImportError",
    "OSError",
    "RuntimeError",
    "TimeoutError",
    "TypeError",
    "ValueError",
    "HTTPException",
];

// ---------------------------------------------------------------------------
// _NativePickerModelList — mirrors lines 282-283
// ---------------------------------------------------------------------------

/// A successful native catalog, including an authoritative empty one.
/// Mirrors `class _NativePickerModelList(list[str]):` (282-283).
#[derive(Debug, Clone, Default)]
pub struct NativePickerModelList(pub Vec<String>);

impl NativePickerModelList {
    pub fn new(models: Vec<String>) -> Self { Self(models) }
    pub fn is_native(&self) -> bool { true }
}
impl std::ops::Deref for NativePickerModelList {
    type Target = Vec<String>;
    fn deref(&self) -> &Self::Target { &self.0 }
}

// ---------------------------------------------------------------------------
// _fetch_picker_live_models — mirrors lines 286-351
// ---------------------------------------------------------------------------

/// Fetch picker models with native Ollama and cached generic discovery.
/// Mirrors `_fetch_picker_live_models(api_key, api_url, native_catalog_provider, preserve_native_models, headers, timeout, api_mode)` (286-351).
/// In slice 1 this is a stub (no hermes_cli.models linkage, NEVER cargo);
/// it preserves the header-merge contract and native-vs-generic branching
/// structure for 1:1 traceability. Real HTTP wiring in later slice.
pub fn fetch_picker_live_models(
    api_key: &str,
    api_url: &str,
    native_catalog_provider: &str,
    preserve_native_models: bool,
    headers: Option<&HashMap<String, String>>,
    timeout_secs: f64,
    api_mode: Option<&str>,
) -> Option<Vec<String>> {
    // Mirrors header merging:
    //   candidate_headers = _get_ollama_native_headers(api_url, api_key=api_key)
    //   strip Authorization if caller already has it, merge caller headers, re-add Bearer if needed
    //   use_native = should_use_ollama_native_catalog(native_catalog_provider, api_url, headers=...)
    //   resolved_headers = candidate_headers or None if use_native else headers
    //   if use_native: if preserve_native_models: return None; else native probe -> cached generic fallback
    //   else: cached generic fetch
    let _ = (api_key, api_url, native_catalog_provider, preserve_native_models, headers, timeout_secs, api_mode);
    // Stub: no network in slice 1; return None so caller falls back to cached catalog.
    // Preserves the "failed native probe is not authoritative" contract by returning None.
    None
}

// ---------------------------------------------------------------------------
// Non-agentic model warning — mirrors lines 354-363
// ---------------------------------------------------------------------------

/// Mirrors `_HERMES_MODEL_WARNING` (358-363).
pub const HERMES_MODEL_WARNING: &str =
    "Nous Research Hermes 3 & 4 models are NOT agentic and are not designed \
     for use with Hermes Agent. They lack the tool-calling capabilities \
     required for agent workflows. Consider using an agentic model instead \
     (Claude, GPT, Gemini, DeepSeek, etc.).";

// Match only the real Nous Research Hermes 3 / Hermes 4 chat families.
// Mirrors `_NOUS_HERMES_NON_AGENTIC_RE = re.compile(r\"(?:^|[/:])hermes[-_ ]?[34](?:[-_.:]|$)\", re.IGNORECASE)` (374-377).
/// Returns true if `text` matches `(?:^|[/:])hermes[-_ ]?[34](?:[-_.:]|$)`
/// case-insensitively, without pulling the `regex` crate (NEVER cargo).
fn nous_hermes_non_agentic_re_match(text: &str) -> bool {
    // Manual scan: look for "hermes" then check surrounding chars + 3/4.
    let lower = text.to_lowercase();
    let needle = "hermes";
    let mut search_from = 0usize;
    while let Some(rel) = lower[search_from..].find(needle) {
        let start = search_from + rel;
        let end = start + needle.len();
        let at_boundary_before = if start == 0 { true } else {
            let c = lower.as_bytes()[start - 1] as char;
            c == '/' || c == ':'
        };
        if !at_boundary_before {
            search_from = end;
            continue;
        }
        // After "hermes" optionally one of - _ space, then 3 or 4
        let mut pos = end;
        let bytes = lower.as_bytes();
        if pos < bytes.len() && (bytes[pos] as char == '-' || bytes[pos] as char == '_' || bytes[pos] as char == ' ') {
            pos += 1;
        }
        if pos < bytes.len() && (bytes[pos] as char == '3' || bytes[pos] as char == '4') {
            let after = pos + 1;
            if after >= bytes.len() {
                return true;
            }
            let c = bytes[after] as char;
            if c == '-' || c == '_' || c == '.' || c == ':' || c == '/' {
                return true;
            }
            // Also match end-of-segment: if next char is not alnum, treat as boundary?
            // Python regex requires [-_.:] or $, so "hermes3x" should NOT match.
            // Only the five delimiters + end count.
        }
        search_from = end;
    }
    false
}

// ---------------------------------------------------------------------------
// Opaque internal model-ID display — mirrors lines 380-400
// ---------------------------------------------------------------------------

/// Mirrors `_OPAQUE_MODEL_PREFIXES: tuple[str, ...] = ("ri.language-model-service..language-model.",)` (397-399).
pub const OPAQUE_MODEL_PREFIXES: &[&str] = &["ri.language-model-service..language-model."];

/// Return a human-friendly form of `model_name` for CLI status output.
///
/// Mirrors `format_model_for_display(model_name: str) -> str` (402-421).
pub fn format_model_for_display(model_name: &str) -> String {
    if model_name.is_empty() { return model_name.to_string(); }
    for prefix in OPAQUE_MODEL_PREFIXES {
        if model_name.starts_with(prefix) {
            let tail = &model_name[prefix.len()..];
            if !tail.is_empty() { return tail.to_string(); }
            return model_name.to_string();
        }
    }
    model_name.to_string()
}

// ---------------------------------------------------------------------------
// is_nous_hermes_non_agentic + _check_hermes_model_warning — mirrors lines 424-442
// ---------------------------------------------------------------------------

/// Return True if `model_name` is a real Nous Hermes 3/4 chat model.
/// Mirrors `is_nous_hermes_non_agentic(model_name: str) -> bool` (424-434).
pub fn is_nous_hermes_non_agentic(model_name: &str) -> bool {
    if model_name.is_empty() { return false; }
    nous_hermes_non_agentic_re_match(model_name)
}

/// Return a warning string if `model_name` is a Nous Hermes 3/4 chat model.
/// Mirrors `_check_hermes_model_warning(model_name: str) -> str` (437-442).
pub fn check_hermes_model_warning(model_name: &str) -> String {
    if is_nous_hermes_non_agentic(model_name) { HERMES_MODEL_WARNING.to_string() } else { String::new() }
}

// ---------------------------------------------------------------------------
// Model aliases — mirrors lines 444-504
// ---------------------------------------------------------------------------

/// Vendor slug and family prefix used for catalog resolution.
/// Mirrors `class ModelIdentity(NamedTuple): vendor, family` (449-452).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ModelIdentity {
    pub vendor: String,
    pub family: String,
}
impl ModelIdentity {
    pub fn new(vendor: &str, family: &str) -> Self {
        Self { vendor: vendor.to_string(), family: family.to_string() }
    }
}

/// Mirrors `MODEL_ALIASES: dict[str, ModelIdentity] = { ... }` (455-504).
pub fn model_aliases() -> HashMap<String, ModelIdentity> {
    let mut m = HashMap::new();
    // Anthropic
    m.insert("sonnet".to_string(), ModelIdentity::new("anthropic", "claude-sonnet"));
    m.insert("opus".to_string(), ModelIdentity::new("anthropic", "claude-opus"));
    m.insert("haiku".to_string(), ModelIdentity::new("anthropic", "claude-haiku"));
    m.insert("claude".to_string(), ModelIdentity::new("anthropic", "claude"));
    // OpenAI
    m.insert("gpt5".to_string(), ModelIdentity::new("openai", "gpt-5"));
    m.insert("gpt".to_string(), ModelIdentity::new("openai", "gpt"));
    m.insert("codex".to_string(), ModelIdentity::new("openai", "codex"));
    m.insert("o3".to_string(), ModelIdentity::new("openai", "o3"));
    m.insert("o4".to_string(), ModelIdentity::new("openai", "o4"));
    // Google
    m.insert("gemini".to_string(), ModelIdentity::new("google", "gemini"));
    // DeepSeek
    m.insert("deepseek".to_string(), ModelIdentity::new("deepseek", "deepseek-chat"));
    // X.AI
    m.insert("grok".to_string(), ModelIdentity::new("x-ai", "grok"));
    // Meta
    m.insert("llama".to_string(), ModelIdentity::new("meta-llama", "llama"));
    // Qwen / Alibaba
    m.insert("qwen".to_string(), ModelIdentity::new("qwen", "qwen"));
    // MiniMax
    m.insert("minimax".to_string(), ModelIdentity::new("minimax", "minimax"));
    // Nvidia
    m.insert("nemotron".to_string(), ModelIdentity::new("nvidia", "nemotron"));
    // Moonshot / Kimi
    m.insert("kimi".to_string(), ModelIdentity::new("moonshotai", "kimi"));
    // Z.AI / GLM
    m.insert("glm".to_string(), ModelIdentity::new("z-ai", "glm"));
    // Step Plan (StepFun)
    m.insert("step".to_string(), ModelIdentity::new("stepfun", "step"));
    // Xiaomi
    m.insert("mimo".to_string(), ModelIdentity::new("xiaomi", "mimo"));
    // Arcee
    m.insert("trinity".to_string(), ModelIdentity::new("arcee-ai", "trinity"));
    m
}

static MODEL_ALIASES_CACHE: OnceLock<HashMap<String, ModelIdentity>> = OnceLock::new();
pub fn model_aliases_global() -> &'static HashMap<String, ModelIdentity> {
    MODEL_ALIASES_CACHE.get_or_init(model_aliases)
}

// ---------------------------------------------------------------------------
// Direct aliases — mirrors lines 506-527
// ---------------------------------------------------------------------------

/// Exact model mapping that bypasses catalog resolution.
/// Mirrors `class DirectAlias(NamedTuple): model, provider, base_url` (515-519).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectAlias {
    pub model: String,
    pub provider: String,
    pub base_url: String,
}
impl DirectAlias {
    pub fn new(model: &str, provider: &str, base_url: &str) -> Self {
        Self { model: model.to_string(), provider: provider.to_string(), base_url: base_url.to_string() }
    }
}

/// Built-in direct aliases (can be extended via config.yaml model_aliases:).
/// Mirrors `_BUILTIN_DIRECT_ALIASES: dict[str, DirectAlias] = {}` (523).
pub fn builtin_direct_aliases() -> HashMap<String, DirectAlias> { HashMap::new() }

/// Merged dict (builtins + user config); populated by _load_direct_aliases().
/// Mirrors `DIRECT_ALIASES: dict[str, DirectAlias] = {}` (526).
static DIRECT_ALIASES: OnceLock<Mutex<HashMap<String, DirectAlias>>> = OnceLock::new();
fn direct_aliases_lock() -> &'static Mutex<HashMap<String, DirectAlias>> {
    DIRECT_ALIASES.get_or_init(|| Mutex::new(HashMap::new()))
}
pub fn direct_aliases_snapshot() -> HashMap<String, DirectAlias> {
    direct_aliases_lock().lock().map(|g| g.clone()).unwrap_or_default()
}

// ---------------------------------------------------------------------------
// _load_direct_aliases — mirrors lines 529-593
// ---------------------------------------------------------------------------

/// Load direct aliases from config.yaml `model_aliases:` section.
/// Mirrors `_load_direct_aliases() -> dict[str, DirectAlias]` (529-593).
/// In slice 1 this is a stub (no config crate linkage, NEVER cargo);
/// it returns builtins only so that `_ensure_direct_aliases` still
/// populates DIRECT_ALIASES with the correct shape. Full YAML parsing
/// (model_aliases + model.aliases) in later slice when config is wired.
pub fn load_direct_aliases() -> HashMap<String, DirectAlias> {
    let mut merged = builtin_direct_aliases();
    // Would: load_config(), merge model_aliases dict + model.aliases string forms
    // Stub preserves the swallow-on-failure contract (try/except: pass).
    let _ = &merged;
    merged
}

// ---------------------------------------------------------------------------
// _ensure_direct_aliases — mirrors lines 596-605
// ---------------------------------------------------------------------------

/// Lazy-load direct aliases on first use.
/// Mirrors `_ensure_direct_aliases() -> None` (596-605).
pub fn ensure_direct_aliases() {
    let lock = direct_aliases_lock();
    if let Ok(mut g) = lock.lock() {
        if g.is_empty() {
            *g = load_direct_aliases();
        }
    }
}

// ---------------------------------------------------------------------------
// Result dataclasses — mirrors lines 608-642
// ---------------------------------------------------------------------------

/// Result of a model switch attempt.
/// Mirrors `@dataclass class ModelSwitchResult:` (612-629).
#[derive(Debug, Clone, Default)]
pub struct ModelSwitchResult {
    pub success: bool,
    pub new_model: String,
    pub target_provider: String,
    pub provider_changed: bool,
    pub api_key: String,
    pub base_url: String,
    pub api_mode: String,
    pub error_message: String,
    pub warning_message: String,
    pub provider_label: String,
    pub resolved_via_alias: String,
    pub capabilities: Option<ModelCapabilities>,
    pub model_info: Option<ModelInfo>,
    pub is_global: bool,
}

/// Parsed flags for a /model command.
/// Mirrors `@dataclass(frozen=True) class ModelFlagParseResult:` (632-641).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelFlagParseResult {
    pub model_input: String,
    pub explicit_provider: String,
    pub is_global: bool,
    pub force_refresh: bool,
    pub is_session: bool,
    pub is_once: bool,
}
impl Default for ModelFlagParseResult {
    fn default() -> Self {
        Self { model_input: String::new(), explicit_provider: String::new(), is_global: false, force_refresh: false, is_session: false, is_once: false }
    }
}

// ---------------------------------------------------------------------------
// Flag parsing — mirrors lines 642-729
// ---------------------------------------------------------------------------

/// Parse flags from /model command args.
///
/// Mirrors `parse_model_flags_detailed(raw_args: str) -> ModelFlagParseResult` (646-713).
pub fn parse_model_flags_detailed(raw_args: &str) -> ModelFlagParseResult {
    let mut is_global = false;
    let mut explicit_provider = String::new();
    let mut force_refresh = false;
    let mut is_session = false;
    let mut is_once = false;

    // Normalize Unicode dashes (Telegram/iOS auto-converts -- to em/en dash)
    // Mirrors `re.sub(r'[\u2012\u2013\u2014\u2015](provider|global|session|refresh|once)', r'--\1', raw_args)` (678)
    // Implemented without regex crate (NEVER cargo): scan for those codepoints followed by keyword.
    let normalized = normalize_unicode_dashes(raw_args);

    // Keep this hand-rolled because model IDs may contain colons/slashes
    let parts: Vec<String> = normalized.split_whitespace().map(|s| s.to_string()).collect();
    let mut i = 0usize;
    let mut filtered: Vec<String> = Vec::new();
    while i < parts.len() {
        if parts[i] == "--global" {
            is_global = true;
            i += 1;
        } else if parts[i] == "--session" {
            is_session = true;
            i += 1;
        } else if parts[i] == "--refresh" {
            force_refresh = true;
            i += 1;
        } else if parts[i] == "--once" {
            is_once = true;
            i += 1;
        } else if parts[i] == "--provider" && i + 1 < parts.len() {
            explicit_provider = parts[i + 1].clone();
            i += 2;
        } else {
            filtered.push(parts[i].clone());
            i += 1;
        }
    }
    let model_input = filtered.join(" ").trim().to_string();
    ModelFlagParseResult { model_input, explicit_provider, is_global, force_refresh, is_session, is_once }
}

fn normalize_unicode_dashes(s: &str) -> String {
    // U+2012 FIGURE DASH, U+2013 EN DASH, U+2014 EM DASH, U+2015 HORIZONTAL BAR
    // Mirrors: re.sub(r'[\u2012\u2013\u2014\u2015](provider|global|session|refresh|once)', r'--\1', raw_args)
    // Only replace when the dash is directly before one of those keywords.
    let keywords = ["provider", "global", "session", "refresh", "once"];
    let dashes = ['\u{2012}', '\u{2013}', '\u{2014}', '\u{2015}'];
    let mut out = String::with_capacity(s.len());
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if dashes.contains(&c) {
            let rest: String = chars[i+1..].iter().collect();
            let mut matched: Option<&str> = None;
            for kw in &keywords {
                if rest.starts_with(kw) {
                    matched = Some(kw);
                    break;
                }
            }
            if let Some(kw) = matched {
                out.push_str("--");
                out.push_str(kw);
                i += 1 + kw.len();
                continue;
            }
        }
        out.push(c);
        i += 1;
    }
    out
}

/// Parse legacy /model flags and return the historical 5-tuple.
/// Mirrors `parse_model_flags(raw_args: str) -> tuple[str, str, bool, bool, bool]` (716-729).
pub fn parse_model_flags(raw_args: &str) -> (String, String, bool, bool, bool) {
    let p = parse_model_flags_detailed(raw_args);
    (p.model_input, p.explicit_provider, p.is_global, p.force_refresh, p.is_session)
}

/// Decide whether a `/model` switch should persist to `config.yaml`.
///
/// Mirrors `resolve_persist_behavior(is_global, is_session, is_once, explicit_provider)` (732-775).
pub fn resolve_persist_behavior(
    is_global: bool,
    is_session: bool,
    is_once: bool,
    explicit_provider: &str,
) -> bool {
    if is_once { return false; }
    if is_session { return false; }
    if is_global { return true; }
    if !explicit_provider.trim().is_empty() { return false; }
    // Would: load_config().get("model") -> persist_switch_by_default
    // Stub in slice 1 (no config linkage, NEVER cargo): default false, swallow on failure.
    // Mirrors the try/except: pass branch.
    let _ = is_global;
    // Best-effort: probe HERMES_HOME/config.yaml without yaml crate (NEVER cargo)
    // We intentionally return false here; full config read in later slice.
    false
}

// ---------------------------------------------------------------------------
// Single-owner /model request parsing — mirrors lines 778-896
// ---------------------------------------------------------------------------

/// Error codes emitted by parse_model_switch_args().
/// Mirrors `MODEL_SWITCH_ERR_ONCE_WITH_GLOBAL = "once_with_global"` (792).
pub const MODEL_SWITCH_ERR_ONCE_WITH_GLOBAL: &str = "once_with_global";
/// Mirrors `MODEL_SWITCH_ERR_ONCE_REQUIRES_TARGET = "once_requires_target"` (793).
pub const MODEL_SWITCH_ERR_ONCE_REQUIRES_TARGET: &str = "once_requires_target";

/// Canonical (surface-neutral) error copy.
/// Mirrors `MODEL_SWITCH_ERROR_TEXT = { ... }` (798-801).
pub fn model_switch_error_text(code: &str) -> Option<&'static str> {
    match code {
        "once_with_global" => Some("/model --once cannot be combined with --global"),
        "once_requires_target" => Some("/model --once requires a model or provider."),
        _ => None,
    }
}

/// A fully parsed /model command request.
/// Mirrors `@dataclass(frozen=True) class ModelSwitchRequest:` (804-846).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelSwitchRequest {
    pub raw: String,
    pub target: String,
    pub explicit_provider: String,
    pub is_global: bool,
    pub is_session: bool,
    pub is_once: bool,
    pub force_refresh: bool,
    pub scope: String,
    pub errors: Vec<String>,
}
impl ModelSwitchRequest {
    /// Compat: `model_input` property — mirrors `@property def model_input(self) -> str: return self.target` (830-831)
    pub fn model_input(&self) -> &str { &self.target }
    /// Compat: `flags` property — mirrors `@property def flags(self) -> ModelFlagParseResult` (834-842)
    pub fn flags(&self) -> ModelFlagParseResult {
        ModelFlagParseResult {
            model_input: self.target.clone(),
            explicit_provider: self.explicit_provider.clone(),
            is_global: self.is_global,
            force_refresh: self.force_refresh,
            is_session: self.is_session,
            is_once: self.is_once,
        }
    }
    /// Canonical (undecorated) error strings for this request.
    /// Mirrors `def error_messages(self) -> list:` (844-846).
    pub fn error_messages(&self) -> Vec<String> {
        self.errors.iter().filter_map(|c| model_switch_error_text(c).map(|s| s.to_string())).collect()
    }
}

/// Parse a raw /model argument string into a `ModelSwitchRequest`.
///
/// Mirrors `parse_model_switch_args(raw: str) -> ModelSwitchRequest` (849-895).
pub fn parse_model_switch_args(raw: &str) -> ModelSwitchRequest {
    let raw_owned = raw.to_string();
    let parsed = parse_model_flags_detailed(&raw_owned);
    let mut errors: Vec<String> = Vec::new();
    if parsed.is_once && parsed.is_global {
        errors.push(MODEL_SWITCH_ERR_ONCE_WITH_GLOBAL.to_string());
    }
    if parsed.is_once && parsed.model_input.trim().is_empty() && parsed.explicit_provider.trim().is_empty() {
        errors.push(MODEL_SWITCH_ERR_ONCE_REQUIRES_TARGET.to_string());
    }
    let scope = if parsed.is_once { "once" } else if parsed.is_session { "session" } else if parsed.is_global { "global" } else { "default" };
    ModelSwitchRequest {
        raw: raw_owned,
        target: parsed.model_input.clone(),
        explicit_provider: parsed.explicit_provider.clone(),
        is_global: parsed.is_global,
        is_session: parsed.is_session,
        is_once: parsed.is_once,
        force_refresh: parsed.force_refresh,
        scope: scope.to_string(),
        errors,
    }
}

// ---------------------------------------------------------------------------
// _effective_model_candidate — mirrors lines 898-900 (slice 1 head)
// ---------------------------------------------------------------------------

/// Extract a model-name candidate from a str / dict / attr-object.
/// Mirrors `def _effective_model_candidate(value: Any) -> str:` (898-900 — slice 1 head).
/// Slice 1 covers through line 900 (`if value is None: return ""` / `if isinstance(value, str):`).
/// The `dict` / `attr` branches (lines 901+) continue in `model_switch_slice2.rs`.
/// This stub preserves the contract for the head so callers in slice 1 compile;
/// the full body (dict `value.get("model")` + `getattr(value, "model")` fallback) is wired in slice 2.
///
/// For 1:1 line mapping inside slice 1 we implement the two branches that are in-bounds:
/// - `None` → `""`
/// - `str` → `value.strip()`
pub fn effective_model_candidate_str(value: Option<&str>) -> String {
    match value {
        None => String::new(),
        Some(s) => s.trim().to_string(),
    }
}

/// JSON-value variant of `_effective_model_candidate` covering the slice-1-visible branches.
/// For `Str` and `Null` this is exact; `Object` (dict) and attr-object branches
/// are completed in slice 2, but we include a best-effort dict probe here so
/// the function is usable before slice 2 is wired (still 1:1 within slice 1).
pub fn effective_model_candidate(value: &serde_stub::JsonValue) -> String {
    match value {
        serde_stub::JsonValue::Null => String::new(),
        serde_stub::JsonValue::Str(s) => s.trim().to_string(),
        serde_stub::JsonValue::Object(map) => {
            // Lines 904-905 (first dict branch) would be in slice 2, but we include it so the stub is functional
            // without duplicating the tail across slices. Real impl in slice 2 will be authoritative.
            if let Some(serde_stub::JsonValue::Str(s)) = map.get("model") {
                return s.trim().to_string();
            }
            String::new()
        }
        _ => {
            // Attr-object branch (`getattr(value, "model")`) is slice 2; return "" for non-dict non-str here.
            String::new()
        }
    }
}

// ---------------------------------------------------------------------------
// Minimal JSON value stub — mirrors typing.Any / dict shapes without serde (NEVER cargo)
// ---------------------------------------------------------------------------

pub mod serde_stub {
    use std::collections::HashMap;
    #[derive(Debug, Clone, PartialEq)]
    pub enum JsonValue {
        Null,
        Bool(bool),
        Number(f64),
        Str(String),
        Object(HashMap<String, JsonValue>),
        Array(Vec<JsonValue>),
    }
    impl JsonValue {
        pub fn str_value(&self) -> Option<&str> {
            if let JsonValue::Str(s) = self { Some(s) } else { None }
        }
    }
}

// ---------------------------------------------------------------------------
// Slice boundary — line 900
// ---------------------------------------------------------------------------
// Python `model_switch.py` lines 901-3989 (effective_model_candidate tail,
// resolve_effective_model, _model_sort_key, AmbiguousAliasError,
// _ambiguous_alias_message, resolve_alias, get_authenticated_provider_slugs,
// _resolve_alias_fallback, resolve_display_context_length (+ async variant),
// _configured_provider_matches, plus the remaining ~2 500 lines of
// credential/provider/model pipeline through switch_model and helpers)
// continue in `model_switch_slice2.rs` (from line 901, inside
// `_effective_model_candidate`'s `if isinstance(value, str):` tail).
// This file intentionally stops at the 900-line boundary so that `cargo` is
// never invoked and the 5-slice decomposition stays clean.
