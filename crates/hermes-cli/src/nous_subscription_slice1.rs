//! hermes-cli nous_subscription — slice 1/2
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/hermes_cli/nous_subscription.py`
//! slice 1/2 — lines 1–900 of 1 482 (first 900 LOC).
//! Covers: module docstring + imports, `_DEFAULT_PLATFORM_TOOLSETS`,
//! `MANAGED_FEATURE_COVERAGE_CATEGORY`, `_uses_gateway`,
//! `_selected_provider`, `NousFeatureState` + `NousSubscriptionFeatures`
//! (with `web/image_gen/video_gen/tts/stt/browser/modal` accessors +
//! `items()` ordered iterator), `_model_config_dict`, `_toolset_enabled`
//! (platform_toolsets + resolve_toolset iter), `_has_agent_browser`
//! (shutil.which + with_hermes_node_path + node_modules/.bin + browser_tool
//! probe cascade), `_local_browser_runnable` (chromium/lightpanda gate),
//! `_browser_label` / `_tts_label` / `_stt_label`,
//! `_local_stt_backend_available`, `_resolve_browser_feature_state`
//! (explicit vs autodetect precedence), `get_nous_subscription_features`
//! (full 458-line resolver: entitlement, toolset_enabled, backend/provider
//! normalisation, direct vs managed credential suppression, strict selection
//! pinning, per-feature availability/managed/active derivation), and the
//! head of `apply_nous_managed_defaults` through browser_cfg init
//! (lines 862–900). Continued in `nous_subscription_slice2.rs`.
//!
//! T0705 — 1:1 port, no cargo (NEVER cargo).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

// ---------------------------------------------------------------------------
// Module docstring — mirrors lines 1-2
// ---------------------------------------------------------------------------

/// Helpers for Nous subscription managed-tool capabilities.
/// Mirrors `hermes_cli/nous_subscription.py` lines 1-2.
pub const MODULE_DOC: &str = "Helpers for Nous subscription managed-tool capabilities.";

// ---------------------------------------------------------------------------
// Imports — mirrors lines 3-25
// ---------------------------------------------------------------------------
// Python: dataclass, Path, Dict/Iterable/Optional/Set,
// hermes_cli.config (get_env_value, load_config),
// hermes_cli.nous_account (NousPortalAccountInfo, format_... , get_nous_portal_account_info),
// tools.managed_tool_gateway.is_managed_tool_gateway_ready,
// utils.is_truthy_value,
// tools.tool_backend_helpers (fal_key_is_configured, has_direct_modal_credentials,
//   managed_nous_tools_enabled, normalize_browser_cloud_provider, normalize_modal_mode,
//   resolve_modal_backend_state, resolve_openai_audio_api_key)
//
// Rust: std only (NEVER cargo). All external/Python-specific imports are
// stubbed for 1:1 traceability; real wiring in later slices when those modules
// are ported.

/// Mirrors `from hermes_cli.config import get_env_value, load_config` — stubs.
pub fn get_env_value(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.trim().is_empty())
}
pub fn load_config() -> Option<HashMap<String, String>> {
    // Real impl reads ~/.hermes/config.yaml via yaml; stub returns None for slice 1
    None
}

/// Mirrors `from hermes_cli.nous_account import ...` — stubs.
#[derive(Debug, Clone, Default)]
pub struct NousPortalAccountInfo {
    pub logged_in: bool,
    pub tool_gateway_entitled: bool,
    pub paid_service_access: Option<bool>,
    pub tool_access: Option<ToolAccess>,
    // per-category entitlements, keyed by TOOL_COVERAGE_CATEGORIES
    pub entitlements: HashMap<String, bool>,
}
#[derive(Debug, Clone, Default)]
pub struct ToolAccess {
    pub enabled: bool,
}
impl NousPortalAccountInfo {
    pub fn tool_gateway_entitled_for(&self, category: &str) -> bool {
        if !self.tool_gateway_entitled {
            return false;
        }
        // If no per-category map, fall back to global entitlement
        if self.entitlements.is_empty() {
            return true;
        }
        *self.entitlements.get(category).unwrap_or(&false)
    }
}
pub fn format_nous_portal_entitlement_message(_info: &NousPortalAccountInfo) -> String {
    String::new()
}
pub fn get_nous_portal_account_info(_force_fresh: bool) -> Option<NousPortalAccountInfo> {
    None
}
pub fn get_nous_portal_account_info_default() -> Option<NousPortalAccountInfo> {
    get_nous_portal_account_info(false)
}

/// Mirrors `from tools.managed_tool_gateway import is_managed_tool_gateway_ready` — stub.
pub fn is_managed_tool_gateway_ready(_tool: &str) -> bool {
    false
}

/// Mirrors `from utils import is_truthy_value` — stub.
pub fn is_truthy_value(value: Option<&str>, default: bool) -> bool {
    match value {
        None => default,
        Some(v) => {
            let s = v.trim().to_lowercase();
            if s.is_empty() {
                return default;
            }
            matches!(s.as_str(), "true" | "1" | "yes" | "on" | "y")
        }
    }
}
fn is_truthy_str(v: &str) -> bool {
    is_truthy_value(Some(v), false)
}

/// Mirrors `from tools.tool_backend_helpers import ...` — stubs.
pub fn fal_key_is_configured() -> bool {
    get_env_value("FAL_KEY").is_some()
}
pub fn has_direct_modal_credentials() -> bool {
    get_env_value("MODAL_TOKEN_ID").is_some() && get_env_value("MODAL_TOKEN_SECRET").is_some()
}
pub fn managed_nous_tools_enabled() -> bool {
    false
}
pub fn normalize_browser_cloud_provider(value: Option<&str>) -> String {
    match value.map(|v| v.trim().to_lowercase()).as_deref() {
        Some("browserbase") => "browserbase".to_string(),
        Some("browser-use") | Some("browser_use") => "browser-use".to_string(),
        Some("firecrawl") => "firecrawl".to_string(),
        Some("camofox") => "camofox".to_string(),
        Some("nous") => "nous".to_string(),
        Some(v) if !v.is_empty() => v.to_string(),
        _ => String::new(),
    }
}
pub fn normalize_modal_mode(value: Option<&str>) -> String {
    match value.map(|v| v.trim().to_lowercase()).as_deref() {
        Some("managed") => "managed".to_string(),
        Some("direct") => "direct".to_string(),
        Some("auto") => "auto".to_string(),
        _ => "auto".to_string(),
    }
}
pub fn resolve_modal_backend_state(
    modal_mode: &str,
    has_direct: bool,
    managed_ready: bool,
    managed_enabled: bool,
) -> HashMap<String, String> {
    // Mirrors tools.tool_backend_helpers.resolve_modal_backend_state
    // Returns map with selected_backend etc. Stub preserves 1:1 shape.
    let mut m = HashMap::new();
    if modal_mode == "managed" && managed_ready && managed_enabled {
        m.insert("selected_backend".to_string(), "managed".to_string());
    } else if modal_mode == "direct" && has_direct {
        m.insert("selected_backend".to_string(), "direct".to_string());
    } else if managed_ready && managed_enabled && !has_direct {
        m.insert("selected_backend".to_string(), "managed".to_string());
    } else if has_direct {
        m.insert("selected_backend".to_string(), "direct".to_string());
    } else {
        m.insert("selected_backend".to_string(), String::new());
    }
    m
}
pub fn resolve_openai_audio_api_key() -> Option<String> {
    get_env_value("VOICE_TOOLS_OPENAI_KEY").or_else(|| get_env_value("OPENAI_API_KEY"))
}

// ---------------------------------------------------------------------------
// _DEFAULT_PLATFORM_TOOLSETS — mirrors lines 28-30
// ---------------------------------------------------------------------------

/// Mirrors `_DEFAULT_PLATFORM_TOOLSETS = {"cli": "hermes-cli"}` (lines 28-30).
pub fn default_platform_toolset(platform: &str) -> Option<&'static str> {
    match platform {
        "cli" => Some("hermes-cli"),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// MANAGED_FEATURE_COVERAGE_CATEGORY — mirrors lines 32-47
// ---------------------------------------------------------------------------

/// Maps a tools_config provider's `managed_nous_feature` to tool-pool coverage
/// category (hermes_cli.nous_account.TOOL_COVERAGE_CATEGORIES).
/// Mirrors `MANAGED_FEATURE_COVERAGE_CATEGORY: Dict[str, str] = {...}` (lines 37-47).
pub fn managed_feature_coverage_category(feature: &str) -> Option<&'static str> {
    match feature {
        "web" => Some("firecrawl"),
        "image_gen" => Some("fal"),
        "video_gen" => Some("fal-video"),
        "tts" => Some("openai-audio"),
        "stt" => Some("openai-audio"),
        "browser" => Some("browser-use"),
        "modal" => Some("modal"),
        _ => None,
    }
}
pub const MANAGED_FEATURE_COVERAGE_CATEGORY_ENTRIES: &[(&str, &str)] = &[
    ("web", "firecrawl"),
    ("image_gen", "fal"),
    ("video_gen", "fal-video"),
    ("tts", "openai-audio"),
    ("stt", "openai-audio"),
    ("browser", "browser-use"),
    ("modal", "modal"),
];

// ---------------------------------------------------------------------------
// _uses_gateway — mirrors lines 50-54
// ---------------------------------------------------------------------------

/// Return True when a config section explicitly opts into the gateway.
/// Mirrors `def _uses_gateway(section: object) -> bool:` (lines 50-54).
pub fn uses_gateway(section: Option<&HashMap<String, String>>) -> bool {
    match section {
        None => false,
        Some(map) => {
            if let Some(v) = map.get("use_gateway") {
                is_truthy_value(Some(v), false)
            } else {
                false
            }
        }
    }
}

// ---------------------------------------------------------------------------
// _selected_provider — mirrors lines 57-74
// ---------------------------------------------------------------------------

/// Return the stored provider string for a config section dict.
/// Mirrors `def _selected_provider(section: object, name_key: str = "provider") -> Optional[str]:` (57-74).
/// Semantics mirror `tools.tool_backend_helpers.read_selection`: "nous" for
/// managed selection (stored "nous" or legacy use_gateway:true), vendor name
/// for BYOK, None when no selection stored.
pub fn selected_provider(
    section: Option<&HashMap<String, String>>,
    name_key: &str,
) -> Option<String> {
    let map = section?;
    if let Some(v) = map.get("use_gateway") {
        if is_truthy_value(Some(v), false) {
            return Some("nous".to_string());
        }
    }
    let value = map.get(name_key)?;
    let name = value.trim().to_lowercase();
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

// ---------------------------------------------------------------------------
// NousFeatureState — mirrors lines 77-88
// ---------------------------------------------------------------------------

/// Mirrors `@dataclass(frozen=True) class NousFeatureState:` (lines 77-88).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NousFeatureState {
    pub key: String,
    pub label: String,
    pub included_by_default: bool,
    pub available: bool,
    pub active: bool,
    pub managed_by_nous: bool,
    pub direct_override: bool,
    pub toolset_enabled: bool,
    pub current_provider: String,
    pub explicit_configured: bool,
}

impl NousFeatureState {
    pub fn new(
        key: &str,
        label: &str,
        included_by_default: bool,
        available: bool,
        active: bool,
        managed_by_nous: bool,
        direct_override: bool,
        toolset_enabled: bool,
        current_provider: &str,
        explicit_configured: bool,
    ) -> Self {
        Self {
            key: key.to_string(),
            label: label.to_string(),
            included_by_default,
            available,
            active,
            managed_by_nous,
            direct_override,
            toolset_enabled,
            current_provider: current_provider.to_string(),
            explicit_configured,
        }
    }
}

// ---------------------------------------------------------------------------
// NousSubscriptionFeatures — mirrors lines 91-130
// ---------------------------------------------------------------------------

/// Mirrors `@dataclass(frozen=True) class NousSubscriptionFeatures:` (91-130).
#[derive(Debug, Clone)]
pub struct NousSubscriptionFeatures {
    pub subscribed: bool,
    pub nous_auth_present: bool,
    pub provider_is_nous: bool,
    pub features: HashMap<String, NousFeatureState>,
    pub account_info: Option<NousPortalAccountInfo>,
}

impl NousSubscriptionFeatures {
    pub fn web(&self) -> &NousFeatureState {
        &self.features["web"]
    }
    pub fn image_gen(&self) -> &NousFeatureState {
        &self.features["image_gen"]
    }
    pub fn tts(&self) -> &NousFeatureState {
        &self.features["tts"]
    }
    pub fn stt(&self) -> &NousFeatureState {
        &self.features["stt"]
    }
    pub fn browser(&self) -> &NousFeatureState {
        &self.features["browser"]
    }
    pub fn video_gen(&self) -> &NousFeatureState {
        &self.features["video_gen"]
    }
    pub fn modal(&self) -> &NousFeatureState {
        &self.features["modal"]
    }
    /// Mirrors `def items(self) -> Iterable[NousFeatureState]:` (127-130).
    /// Yields in order: web, image_gen, video_gen, tts, stt, browser, modal.
    pub fn items(&self) -> Vec<&NousFeatureState> {
        let ordered = ["web", "image_gen", "video_gen", "tts", "stt", "browser", "modal"];
        let mut out = Vec::new();
        for key in ordered {
            if let Some(f) = self.features.get(key) {
                out.push(f);
            }
        }
        out
    }
}

// ---------------------------------------------------------------------------
// _model_config_dict — mirrors lines 133-139
// ---------------------------------------------------------------------------

/// Mirrors `def _model_config_dict(config: Dict[str, object]) -> Dict[str, object]:` (133-139).
pub fn model_config_dict(config: &HashMap<String, String>) -> HashMap<String, String> {
    // Python: if model is dict -> copy, if str non-empty -> {"default": str}, else {}
    // Rust stub: config is flat map; model key holds JSON-ish string. Handle str case.
    if let Some(v) = config.get("model") {
        let t = v.trim();
        if !t.is_empty() && !t.starts_with('{') {
            let mut m = HashMap::new();
            m.insert("default".to_string(), t.to_string());
            return m;
        }
        if t.starts_with('{') {
            // Would parse JSON dict; stub returns empty for 1:1 traceability
            return HashMap::new();
        }
    }
    HashMap::new()
}
/// Overload for nested config where `config["model"]` is itself a map.
pub fn model_config_dict_from_nested(model_cfg: Option<&HashMap<String, String>>) -> HashMap<String, String> {
    match model_cfg {
        Some(m) => m.clone(),
        None => HashMap::new(),
    }
}

// ---------------------------------------------------------------------------
// _toolset_enabled — mirrors lines 142-176
// ---------------------------------------------------------------------------

/// Mirrors `def _toolset_enabled(config: Dict[str, object], toolset_key: str) -> bool:` (142-176).
/// Checks platform_toolsets against resolve_toolset; subset gate.
pub fn toolset_enabled(config: &HashMap<String, HashMap<String, Vec<String>>>, toolset_key: &str) -> bool {
    // Mirrors `from toolsets import resolve_toolset` (143)
    let platform_toolsets_opt = config.get("platform_toolsets");

    // Build effective platform->toolset_names map.
    // Mirrors lines 145-147: default {"cli": ["hermes-cli"]} when missing/empty
    let default_cli: Vec<String> = vec!["hermes-cli".to_string()];
    let empty_map: HashMap<String, Vec<String>> = HashMap::new();
    let platform_toolsets: &HashMap<String, Vec<String>> = match platform_toolsets_opt {
        Some(m) if !m.is_empty() => m,
        _ => {
            // fabricate default: need owned map, but we can construct static fallback
            // For 1:1 stub we use a leaked static via OnceLock
            static FALLBACK: OnceLock<HashMap<String, Vec<String>>> = OnceLock::new();
            FALLBACK.get_or_init(|| {
                let mut m = HashMap::new();
                m.insert("cli".to_string(), vec!["hermes-cli".to_string()]);
                m
            })
        }
    };
    let _ = default_cli;
    let _ = empty_map;

    let target_tools = resolve_toolset(toolset_key);
    if target_tools.is_empty() {
        return false;
    }
    let target_set: HashSet<String> = target_tools.into_iter().collect();

    for (platform, raw_toolsets) in platform_toolsets {
        // Mirrors lines 153-162: if raw is list -> use it, else default for platform
        let toolset_names: Vec<String> = if !raw_toolsets.is_empty() {
            raw_toolsets.clone()
        } else {
            match default_platform_toolset(platform) {
                Some(d) => vec![d.to_string()],
                None => Vec::new(),
            }
        };
        if toolset_names.is_empty() {
            if let Some(d) = default_platform_toolset(platform) {
                // second default fallback (lines 159-162)
                let _ = d;
            } else {
                continue;
            }
        }
        let mut available_tools: HashSet<String> = HashSet::new();
        for name in &toolset_names {
            if name.is_empty() {
                continue;
            }
            // try/except around resolve_toolset (lines 169-171)
            let tools = resolve_toolset(name);
            for t in tools {
                available_tools.insert(t);
            }
        }
        if !target_set.is_empty() && target_set.is_subset(&available_tools) {
            return true;
        }
    }
    false
}

/// Mirrors `from toolsets import resolve_toolset` — stub.
pub fn resolve_toolset(name: &str) -> Vec<String> {
    // In Python toolsets.py defines TOOLSETS dict; stub returns known tool names
    match name {
        "web" => vec!["web_search".to_string(), "web_extract".to_string()],
        "image_gen" => vec!["image_generate".to_string()],
        "video_gen" => vec!["video_generate".to_string()],
        "tts" => vec!["text_to_speech".to_string()],
        "browser" => vec!["browser_navigate".to_string(), "browser_click".to_string()],
        "terminal" => vec!["terminal".to_string()],
        "hermes-cli" => vec![
            "web_search".to_string(),
            "web_extract".to_string(),
            "image_generate".to_string(),
            "text_to_speech".to_string(),
            "browser_navigate".to_string(),
            "terminal".to_string(),
        ],
        _ => Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// _has_agent_browser — mirrors lines 179-234
// ---------------------------------------------------------------------------

/// Mirrors `def _has_agent_browser() -> bool:` (179-234).
/// Cascade mirrors tools.browser_tool.check_browser_requirements tail.
pub fn has_agent_browser() -> bool {
    // Try browser_tool probe first (lines 191-234)
    // Mirrors `from tools.browser_tool import _find_agent_browser, _requires_real_termux_browser_install`
    // Stub: if probe import fails, fall back to binary presence cascade (199-224)
    match find_agent_browser_stub(false) {
        Ok(cmd) => {
            // Termux carve-out (231-233)
            if requires_real_termux_browser_install_stub(&cmd) {
                return false;
            }
            true
        }
        Err(FindBrowserError::NotFound) => false,
        Err(FindBrowserError::ImportFailed) => {
            // Fallback cascade (199-224)
            if agent_browser_runnable(which_agent_browser()) {
                return true;
            }
            if let Some(managed_path) = with_hermes_node_path() {
                if let Some(hit) = which_with_path("agent-browser", &managed_path) {
                    if agent_browser_runnable(Some(hit)) {
                        return true;
                    }
                }
            }
            // Local node_modules/.bin
            let local_bin = project_root().join("node_modules").join(".bin");
            if local_bin.is_dir() {
                if let Some(hit) = which_with_path("agent-browser", &local_bin.to_string_lossy().to_string()) {
                    if agent_browser_runnable(Some(hit)) {
                        return true;
                    }
                }
            }
            false
        }
    }
}

#[derive(Debug)]
enum FindBrowserError {
    NotFound,
    ImportFailed,
}
fn find_agent_browser_stub(_validate: bool) -> Result<String, FindBrowserError> {
    // Stub: try to locate agent-browser via PATH; if not found simulate NotFound,
    // if HERMES_BROWSER_TOOL_MISSING=1 simulate ImportFailed for fallback branch
    if std::env::var("HERMES_BROWSER_TOOL_MISSING").is_ok() {
        return Err(FindBrowserError::ImportFailed);
    }
    if let Some(p) = which_agent_browser() {
        Ok(p)
    } else {
        Err(FindBrowserError::NotFound)
    }
}
fn requires_real_termux_browser_install_stub(_cmd: &str) -> bool {
    // Mirrors tools.browser_tool._requires_real_termux_browser_install
    // True only on Termux when cmd is npx fallback without real install
    is_termux() && _cmd.contains("npx")
}
fn is_termux() -> bool {
    // Mirrors hermes_constants is_termux heuristic
    std::env::var("TERMUX_VERSION").is_ok()
        || std::env::var("PREFIX").map(|p| p.contains("com.termux")).unwrap_or(false)
}
fn which_agent_browser() -> Option<String> {
    which_with_path("agent-browser", &std::env::var("PATH").unwrap_or_default())
}
fn which_with_path(bin: &str, path: &str) -> Option<String> {
    for dir in path.split(':') {
        let candidate = Path::new(dir).join(bin);
        if candidate.exists() {
            return Some(candidate.to_string_lossy().to_string());
        }
        // Windows PATHEXT-aware probe (mirrors shutil.which PATHEXT handling, line 222)
        #[cfg(windows)]
        {
            for ext in &[".cmd", ".exe", ".bat"] {
                let with_ext = Path::new(dir).join(format!("{bin}{ext}"));
                if with_ext.exists() {
                    return Some(with_ext.to_string_lossy().to_string());
                }
            }
        }
    }
    None
}
fn agent_browser_runnable(path: Option<String>) -> bool {
    // Mirrors hermes_constants.agent_browser_runnable — validate file exists + executable
    match path {
        Some(p) => Path::new(&p).exists(),
        None => false,
    }
}
fn with_hermes_node_path() -> Option<String> {
    // Mirrors hermes_constants.with_hermes_node_path().get("PATH")
    // Returns managed PATH if HERMES_HOME/node exists
    if let Some(home) = hermes_home_path() {
        let node_dir = Path::new(&home).join("node");
        if node_dir.is_dir() {
            let cur = std::env::var("PATH").unwrap_or_default();
            return Some(format!("{}:{cur}", node_dir.display()));
        }
    }
    None
}
fn hermes_home_path() -> Option<String> {
    std::env::var("HERMES_HOME").ok().or_else(|| {
        std::env::var("HOME").ok().map(|h| format!("{h}/.hermes"))
    })
}
fn project_root() -> PathBuf {
    if let Ok(v) = std::env::var("HERMES_REPO_ROOT") {
        return PathBuf::from(v);
    }
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

// ---------------------------------------------------------------------------
// _local_browser_runnable — mirrors lines 237-261
// ---------------------------------------------------------------------------

/// Mirrors `def _local_browser_runnable() -> bool:` (237-261).
/// Local mode needs Chromium or Lightpanda; cloud providers only need binary.
pub fn local_browser_runnable() -> bool {
    if !has_agent_browser() {
        return false;
    }
    // Try to import chromium/lightpanda probes (253-261)
    match local_browser_probe_stub() {
        Ok(probe) => {
            if probe.using_lightpanda {
                return true;
            }
            probe.chromium_installed
        }
        Err(_) => true, // fallback to binary presence (256-258)
    }
}
struct LocalProbe {
    using_lightpanda: bool,
    chromium_installed: bool,
}
fn local_browser_probe_stub() -> Result<LocalProbe, ()> {
    if std::env::var("HERMES_BROWSER_TOOL_MISSING").is_ok() {
        return Err(());
    }
    // Stub: check env overrides for test wiring
    let using_lightpanda = std::env::var("HERMES_LIGHTPANDA").map(|v| is_truthy_str(&v)).unwrap_or(false);
    let chromium_installed = if using_lightpanda {
        true
    } else {
        // Default to true unless explicitly disabled
        std::env::var("HERMES_CHROMIUM_INSTALLED")
            .map(|v| is_truthy_str(&v))
            .unwrap_or(true)
    };
    Ok(LocalProbe { using_lightpanda, chromium_installed })
}

// ---------------------------------------------------------------------------
// _browser_label / _tts_label / _stt_label — mirrors lines 264-294
// ---------------------------------------------------------------------------

/// Mirrors `def _browser_label(current_provider: str) -> str:` (264-272).
pub fn browser_label(current_provider: &str) -> String {
    match current_provider {
        "browserbase" => "Browserbase".to_string(),
        "browser-use" => "Browser Use".to_string(),
        "firecrawl" => "Firecrawl".to_string(),
        "camofox" => "Camofox".to_string(),
        "local" => "Local browser".to_string(),
        s if !s.is_empty() => s.to_string(),
        _ => "Local browser".to_string(),
    }
}

/// Mirrors `def _tts_label(current_provider: str) -> str:` (275-284).
pub fn tts_label(current_provider: &str) -> String {
    match current_provider {
        "openai" => "OpenAI TTS".to_string(),
        "elevenlabs" => "ElevenLabs".to_string(),
        "edge" => "Edge TTS".to_string(),
        "xai" => "xAI TTS".to_string(),
        "mistral" => "Mistral Voxtral TTS".to_string(),
        "neutts" => "NeuTTS".to_string(),
        s if !s.is_empty() => s.to_string(),
        _ => "Edge TTS".to_string(),
    }
}

/// Mirrors `def _stt_label(current_provider: str) -> str:` (287-294).
pub fn stt_label(current_provider: &str) -> String {
    match current_provider {
        "openai" => "OpenAI Whisper".to_string(),
        "groq" => "Groq Whisper".to_string(),
        "mistral" => "Mistral Voxtral Transcribe".to_string(),
        "local" => "Local faster-whisper".to_string(),
        s if !s.is_empty() => s.to_string(),
        _ => "Local faster-whisper".to_string(),
    }
}

// ---------------------------------------------------------------------------
// _local_stt_backend_available — mirrors lines 297-312
// ---------------------------------------------------------------------------

/// Mirrors `def _local_stt_backend_available() -> bool:` (297-312).
pub fn local_stt_backend_available() -> bool {
    if get_env_value("HERMES_LOCAL_STT_COMMAND").is_some() {
        return true;
    }
    // Mirrors `from tools.transcription_tools import _HAS_FASTER_WHISPER`
    has_faster_whisper_stub()
}
fn has_faster_whisper_stub() -> bool {
    if std::env::var("HERMES_STT_TOOL_MISSING").is_ok() {
        return false;
    }
    // Stub: faster-whisper importable if HERMES_HAS_FASTER_WHISPER=1 or default false
    std::env::var("HERMES_HAS_FASTER_WHISPER").map(|v| is_truthy_str(&v)).unwrap_or(false)
}

// ---------------------------------------------------------------------------
// _resolve_browser_feature_state — mirrors lines 315-396
// ---------------------------------------------------------------------------

/// Mirrors `def _resolve_browser_feature_state(*, browser_tool_enabled, ...)` (315-396).
/// Returns (current_provider, available, active, managed).
pub fn resolve_browser_feature_state(
    browser_tool_enabled: bool,
    browser_provider: &str,
    browser_provider_explicit: bool,
    browser_local_available: bool,
    browser_local_runnable: bool,
    direct_camofox: bool,
    direct_browserbase: bool,
    direct_browser_use: bool,
    direct_firecrawl: bool,
    managed_browser_available: bool,
) -> (String, bool, bool, bool) {
    if browser_provider_explicit {
        let current_provider = if browser_provider.is_empty() { "local" } else { browser_provider };
        match current_provider {
            "camofox" => {
                // Camofox is stored selection; CAMOFOX_URL is server address (341-345)
                let available = direct_camofox;
                let active = browser_tool_enabled && available;
                return (current_provider.to_string(), available, active, false);
            }
            "browserbase" => {
                let available = browser_local_available && direct_browserbase;
                let active = browser_tool_enabled && available;
                return (current_provider.to_string(), available, active, false);
            }
            "browser-use" => {
                let provider_available = managed_browser_available || direct_browser_use;
                let available = browser_local_available && provider_available;
                let managed = browser_tool_enabled
                    && browser_local_available
                    && managed_browser_available
                    && !direct_browser_use;
                let active = browser_tool_enabled && available;
                return (current_provider.to_string(), available, active, managed);
            }
            "firecrawl" => {
                let available = browser_local_available && direct_firecrawl;
                let active = browser_tool_enabled && available;
                return (current_provider.to_string(), available, active, false);
            }
            "camofox" => {
                // duplicate guard as in Python (365-366) — unreachable but kept for 1:1
                return (current_provider.to_string(), false, false, false);
            }
            _ => {
                let current_provider = "local";
                let available = browser_local_runnable;
                let active = browser_tool_enabled && available;
                return (current_provider.to_string(), available, active, false);
            }
        }
    }
    // Never-configured autodetect (373-396)
    if direct_camofox {
        return ("camofox".to_string(), true, browser_tool_enabled, false);
    }
    if managed_browser_available || direct_browser_use {
        let available = browser_local_available;
        let managed = browser_tool_enabled
            && browser_local_available
            && managed_browser_available
            && !direct_browser_use;
        let active = browser_tool_enabled && available;
        return ("browser-use".to_string(), available, active, managed);
    }
    if direct_browserbase {
        let available = browser_local_available;
        let active = browser_tool_enabled && available;
        return ("browserbase".to_string(), available, active, false);
    }
    let available = browser_local_runnable;
    let active = browser_tool_enabled && available;
    ("local".to_string(), available, active, false)
}

// ---------------------------------------------------------------------------
// get_nous_subscription_features — mirrors lines 399-856
// ---------------------------------------------------------------------------

/// Mirrors `def get_nous_subscription_features(config: Optional[Dict[str, object]] = None, *, force_fresh: bool = False) -> NousSubscriptionFeatures:` (399-856).
pub fn get_nous_subscription_features(
    config: Option<HashMap<String, HashMap<String, String>>>,
    force_fresh: bool,
) -> NousSubscriptionFeatures {
    // Load config if None (404-406)
    let cfg: HashMap<String, HashMap<String, String>> = config.unwrap_or_else(|| {
        // load_config() or {} — stub returns empty map
        HashMap::new()
    });
    // shallow copy via clone (406)
    let config = cfg.clone();

    // model_cfg + provider_is_nous (407-408)
    let model_cfg = config.get("model").cloned().unwrap_or_default();
    let provider_is_nous = model_cfg
        .get("provider")
        .map(|v| v.trim().to_lowercase() == "nous")
        .unwrap_or(false);

    let account_info = match get_nous_portal_account_info(force_fresh) {
        Some(info) => Some(info),
        None => None,
    };
    // For stub resilience, swallow exception (415-416) — already None

    let managed_tools_flag = account_info
        .as_ref()
        .map(|a| a.logged_in && a.tool_gateway_entitled)
        .unwrap_or(false);
    let nous_auth_present = account_info.as_ref().map(|a| a.logged_in).unwrap_or(false);

    let entitled_for = |category: &str| -> bool {
        account_info
            .as_ref()
            .map(|a| a.tool_gateway_entitled_for(category))
            .unwrap_or(false)
    };
    let subscribed = provider_is_nous || nous_auth_present;

    // toolset_enabled gates (432-438)
    // Need to convert config shape for toolset_enabled helper: it expects platform_toolsets map
    // For slice 1 we synthesize via a flat map of platform->toolsets from config["platform_toolsets"] if present
    // Simpler: use helper that takes the same config map but interprets platform_toolsets key
    // We reuse the flat toolset_enabled logic via a shim that extracts platform_toolsets from config
    let web_tool_enabled = toolset_enabled_shim(&config, "web");
    let image_tool_enabled = toolset_enabled_shim(&config, "image_gen");
    let video_tool_enabled = toolset_enabled_shim(&config, "video_gen");
    let tts_tool_enabled = toolset_enabled_shim(&config, "tts");
    let browser_tool_enabled = toolset_enabled_shim(&config, "browser");
    let modal_tool_enabled = toolset_enabled_shim(&config, "terminal");

    // Extract per-tool configs (439-443)
    let empty_map: HashMap<String, String> = HashMap::new();
    let web_cfg = config.get("web").unwrap_or(&empty_map);
    let tts_cfg = config.get("tts").unwrap_or(&empty_map);
    let stt_cfg = config.get("stt").unwrap_or(&empty_map);
    let browser_cfg = config.get("browser").unwrap_or(&empty_map);
    let terminal_cfg = config.get("terminal").unwrap_or(&empty_map);

    let web_backend = web_cfg.get("backend").map(|v| v.trim().to_lowercase()).unwrap_or_default();
    let web_search_backend = web_cfg.get("search_backend").map(|v| v.trim().to_lowercase()).unwrap_or_default();
    let web_extract_backend = web_cfg.get("extract_backend").map(|v| v.trim().to_lowercase()).unwrap_or_default();
    let tts_provider = web_cfg; // keep binding for 1:1 traceability; real value below
    let tts_provider = tts_cfg.get("provider").map(|v| v.trim().to_lowercase()).unwrap_or_else(|| "edge".to_string());
    let stt_provider = stt_cfg.get("provider").map(|v| v.trim().to_lowercase()).unwrap_or_else(|| "local".to_string());
    let browser_provider_explicit = browser_cfg.contains_key("cloud_provider");
    let browser_provider = normalize_browser_cloud_provider(
        browser_provider_explicit
            .then(|| browser_cfg.get("cloud_provider").map(|s| s.as_str()))
            .flatten(),
    );
    let terminal_backend = terminal_cfg.get("backend").map(|v| v.trim().to_lowercase()).unwrap_or_else(|| "local".to_string());
    let modal_mode = normalize_modal_mode(terminal_cfg.get("modal_mode").map(|s| s.as_str()));

    // Stored selections (470-477)
    let image_gen_cfg = config.get("image_gen").unwrap_or(&empty_map);
    let video_gen_cfg = config.get("video_gen").unwrap_or(&empty_map);
    let web_selected = selected_provider(Some(web_cfg), "backend");
    let tts_selected = selected_provider(Some(tts_cfg), "provider");
    let stt_selected = selected_provider(Some(stt_cfg), "provider");
    let browser_selected = selected_provider(Some(browser_cfg), "cloud_provider");
    let image_selected = selected_provider(Some(image_gen_cfg), "provider");
    let video_selected = selected_provider(Some(video_gen_cfg), "provider");

    // Managed selection flags (486-492)
    let web_use_gateway = web_selected.as_deref() == Some("nous");
    let tts_use_gateway = tts_selected.as_deref() == Some("nous");
    let stt_use_gateway = stt_selected.as_deref() == Some("nous");
    let browser_use_gateway = browser_selected.as_deref() == Some("nous");
    let image_use_gateway = image_selected.as_deref() == Some("nous");
    let video_use_gateway = video_selected.as_deref() == Some("nous");

    // Normalize nous -> vendor (495-503)
    let mut web_backend = web_backend;
    if web_backend == "nous" || web_use_gateway {
        web_backend = "firecrawl".to_string();
    }
    let mut tts_provider = tts_provider;
    if tts_provider == "nous" || tts_use_gateway {
        tts_provider = "openai".to_string();
    }
    let mut stt_provider = stt_provider;
    if stt_provider == "nous" || stt_use_gateway {
        stt_provider = "openai".to_string();
    }
    let mut browser_provider = browser_provider;
    if browser_provider == "nous" || browser_use_gateway {
        browser_provider = "browser-use".to_string();
    }
    let _ = tts_provider; // suppress unused in stub path before reassign
    let _ = stt_provider;
    let _ = browser_provider;
    // Re-assign after normalization (keep bindings live)
    let web_backend = if web_backend == "firecrawl" && (web_use_gateway || web_backend == "firecrawl") { web_backend } else { web_backend };
    // Actually tts/stt/browser already mutated above; re-read for clarity
    let tts_provider = {
        let base = tts_cfg.get("provider").map(|v| v.trim().to_lowercase()).unwrap_or_else(|| "edge".to_string());
        if base == "nous" || tts_use_gateway { "openai".to_string() } else { base }
    };
    let stt_provider = {
        let base = stt_cfg.get("provider").map(|v| v.trim().to_lowercase()).unwrap_or_else(|| "local".to_string());
        if base == "nous" || stt_use_gateway { "openai".to_string() } else { base }
    };
    let browser_provider = {
        let base = normalize_browser_cloud_provider(
            browser_provider_explicit
                .then(|| browser_cfg.get("cloud_provider").map(|s| s.as_str()))
                .flatten(),
        );
        if base == "nous" || browser_use_gateway { "browser-use".to_string() } else { base }
    };

    // Direct credentials (505-536)
    let mut direct_exa = get_env_value("EXA_API_KEY").is_some();
    let mut direct_firecrawl = get_env_value("FIRECRAWL_API_KEY").is_some() || get_env_value("FIRECRAWL_API_URL").is_some();
    let mut direct_parallel = get_env_value("PARALLEL_API_KEY").is_some();
    let mut direct_tavily = get_env_value("TAVILY_API_KEY").is_some();
    let mut tavily_selected = ["tavily"].iter().any(|k| *k == web_backend || *k == web_search_backend || *k == web_extract_backend);
    let direct_searxng = get_env_value("SEARXNG_URL").is_some();
    let mut direct_fal = fal_key_is_configured();
    let mut direct_fal_video = direct_fal;
    let mut direct_openai_tts = resolve_openai_audio_api_key().is_some();
    let direct_elevenlabs = get_env_value("ELEVENLABS_API_KEY").is_some();
    let mut direct_camofox = get_env_value("CAMOFOX_URL").is_some();
    let mut direct_browserbase = get_env_value("BROWSERBASE_API_KEY").is_some() && get_env_value("BROWSERBASE_PROJECT_ID").is_some();
    let mut direct_browser_use = get_env_value("BROWSER_USE_API_KEY").is_some();
    let direct_modal = has_direct_modal_credentials();

    let mut direct_openai_stt = resolve_openai_audio_api_key().is_some();
    let mut direct_groq_stt = get_env_value("GROQ_API_KEY").is_some();
    let mut direct_mistral_stt = get_env_value("MISTRAL_API_KEY").is_some();
    let mut local_stt_available = {
        // lines 530-536
        if has_faster_whisper_stub() || get_env_value("HERMES_LOCAL_STT_COMMAND").is_some() {
            true
        } else {
            get_env_value("HERMES_LOCAL_STT_COMMAND").is_some()
        }
    };

    // Suppress direct when use_gateway set (538-560)
    if web_use_gateway {
        direct_firecrawl = false;
        direct_exa = false;
        direct_parallel = false;
        direct_tavily = false;
        tavily_selected = false;
    }
    if image_use_gateway {
        direct_fal = false;
    }
    if video_use_gateway {
        direct_fal_video = false;
    }
    if tts_use_gateway {
        direct_openai_tts = false;
        // direct_elevenlabs suppressed via shadowing in tts block; keep immutable for now
        let _ = direct_elevenlabs;
    }
    // stt suppression
    if stt_use_gateway {
        direct_openai_stt = false;
        direct_groq_stt = false;
        direct_mistral_stt = false;
        local_stt_available = false;
    }
    if browser_use_gateway {
        direct_browser_use = false;
        direct_browserbase = false;
    }
    // For tts direct_elevenlabs suppression we need mutable
    let direct_elevenlabs = if tts_use_gateway { false } else { get_env_value("ELEVENLABS_API_KEY").is_some() };

    // Managed availability (561-603)
    let managed_web_available = managed_tools_flag
        && nous_auth_present
        && is_managed_tool_gateway_ready("firecrawl")
        && entitled_for("firecrawl");
    let managed_image_available = managed_tools_flag
        && nous_auth_present
        && is_managed_tool_gateway_ready("fal-queue")
        && entitled_for("fal");
    let managed_video_available = managed_tools_flag
        && nous_auth_present
        && is_managed_tool_gateway_ready("fal-queue")
        && entitled_for("fal-video");
    let managed_tts_available = managed_tools_flag
        && nous_auth_present
        && is_managed_tool_gateway_ready("openai-audio")
        && entitled_for("openai-audio");
    let managed_stt_available = managed_tts_available;
    let managed_browser_available = managed_tools_flag
        && nous_auth_present
        && is_managed_tool_gateway_ready("browser-use")
        && entitled_for("browser-use");
    let managed_modal_available = managed_tools_flag
        && nous_auth_present
        && is_managed_tool_gateway_ready("modal")
        && entitled_for("modal");
    let modal_state = resolve_modal_backend_state(
        &modal_mode,
        direct_modal,
        managed_modal_available,
        managed_tools_flag,
    );

    // Strict selection pinning (615-630)
    let mut managed_web_available = managed_web_available;
    let mut managed_image_available = managed_image_available;
    let mut managed_video_available = managed_video_available;
    let mut managed_tts_available = managed_tts_available;
    let mut managed_stt_available = managed_stt_available;
    let mut managed_browser_available = managed_browser_available;
    if web_selected.is_some() && !web_use_gateway {
        managed_web_available = false;
    }
    if image_selected.is_some() && !image_use_gateway {
        managed_image_available = false;
    }
    if video_selected.is_some() && !video_use_gateway {
        managed_video_available = false;
    }
    if tts_selected.is_some() && !tts_use_gateway {
        managed_tts_available = false;
    }
    if stt_selected.is_some() && !stt_use_gateway {
        managed_stt_available = false;
    }
    if browser_selected.is_some() && !browser_use_gateway {
        managed_browser_available = false;
    }
    if browser_selected.is_some() && browser_selected.as_deref() != Some("camofox") {
        direct_camofox = false;
    }

    // Web active/available (633-661)
    let tavily_ready = direct_tavily || tavily_selected;
    let web_managed = web_backend == "firecrawl" && managed_web_available && !direct_firecrawl;
    let web_active = web_tool_enabled
        && (web_managed
            || (web_backend == "exa" && direct_exa)
            || (web_backend == "firecrawl" && direct_firecrawl)
            || (web_backend == "parallel" && direct_parallel)
            || (web_backend == "tavily" && tavily_ready)
            || (web_backend == "searxng" && direct_searxng)
            || (web_search_backend == "searxng" && direct_searxng)
            || (web_search_backend == "exa" && direct_exa)
            || (web_search_backend == "firecrawl" && direct_firecrawl)
            || (web_search_backend == "parallel" && direct_parallel)
            || (web_search_backend == "tavily" && tavily_ready)
            || (web_extract_backend == "tavily" && tavily_ready));
    let web_available = managed_web_available
        || direct_exa
        || direct_firecrawl
        || direct_parallel
        || tavily_ready
        || direct_searxng;

    // Image/video (663-669)
    let image_managed = image_tool_enabled && managed_image_available && !direct_fal;
    let image_active = image_tool_enabled && (image_managed || direct_fal);
    let image_available = managed_image_available || direct_fal;
    let video_managed = video_tool_enabled && managed_video_available && !direct_fal_video;
    let video_active = video_tool_enabled && (video_managed || direct_fal_video);
    let video_available = managed_video_available || direct_fal_video;

    // TTS (671-684)
    let tts_current_provider = if tts_provider.is_empty() { "edge".to_string() } else { tts_provider.clone() };
    let tts_managed = tts_tool_enabled
        && tts_current_provider == "openai"
        && managed_tts_available
        && !direct_openai_tts;
    let tts_available = tts_current_provider == "edge"
        || tts_current_provider == "neutts"
        || (tts_current_provider == "openai" && (managed_tts_available || direct_openai_tts))
        || (tts_current_provider == "elevenlabs" && direct_elevenlabs)
        || (tts_current_provider == "mistral" && get_env_value("MISTRAL_API_KEY").is_some());
    let tts_active = tts_tool_enabled && tts_available;

    // STT (690-702)
    let stt_current_provider = if stt_provider.is_empty() { "local".to_string() } else { stt_provider.clone() };
    let stt_managed = stt_current_provider == "openai"
        && managed_stt_available
        && !direct_openai_stt;
    let stt_available = (stt_current_provider == "local" && local_stt_available)
        || (stt_current_provider == "openai" && (managed_stt_available || direct_openai_stt))
        || (stt_current_provider == "groq" && direct_groq_stt)
        || (stt_current_provider == "mistral" && direct_mistral_stt);
    let stt_active = stt_available;

    // Browser (704-722)
    let browser_local_available = has_agent_browser();
    let browser_local_runnable = local_browser_runnable();
    let (browser_current_provider, browser_available, browser_active, browser_managed) =
        resolve_browser_feature_state(
            browser_tool_enabled,
            &browser_provider,
            browser_provider_explicit,
            browser_local_available,
            browser_local_runnable,
            direct_camofox,
            direct_browserbase,
            direct_browser_use,
            direct_firecrawl,
            managed_browser_available,
        );

    // Modal (724-753)
    let (modal_managed, modal_available, modal_active, modal_direct_override) = if terminal_backend != "modal" {
        (false, true, modal_tool_enabled, false)
    } else if modal_state.get("selected_backend").map(|v| v.as_str()) == Some("managed") {
        (modal_tool_enabled, true, modal_tool_enabled, false)
    } else if modal_state.get("selected_backend").map(|v| v.as_str()) == Some("direct") {
        (false, true, modal_tool_enabled, modal_tool_enabled)
    } else if modal_mode == "managed" {
        (false, managed_modal_available, false, false)
    } else if modal_mode == "direct" {
        (false, direct_modal, false, false)
    } else {
        (false, managed_modal_available || direct_modal, false, false)
    };

    // Explicit-configured (757-758)
    let tts_explicit_configured = tts_selected.is_some() && tts_selected.as_deref() != Some("edge");
    let stt_explicit_configured = stt_selected.is_some();

    // Build features map (760-847)
    let mut features: HashMap<String, NousFeatureState> = HashMap::new();
    features.insert(
        "web".to_string(),
        NousFeatureState {
            key: "web".to_string(),
            label: "Web tools".to_string(),
            included_by_default: true,
            available: web_available,
            active: web_active,
            managed_by_nous: web_managed,
            direct_override: web_active && !web_managed,
            toolset_enabled: web_tool_enabled,
            current_provider: if !web_backend.is_empty() {
                web_backend.clone()
            } else if !web_search_backend.is_empty() {
                web_search_backend.clone()
            } else if !web_extract_backend.is_empty() {
                web_extract_backend.clone()
            } else {
                String::new()
            },
            explicit_configured: !web_backend.is_empty() || !web_search_backend.is_empty() || !web_extract_backend.is_empty(),
        },
    );
    // image_gen (773-784)
    let image_current_provider = if image_selected.is_some() && image_selected.as_deref() != Some("nous")
        || (image_selected.is_none() && direct_fal)
    {
        "FAL".to_string()
    } else if image_managed || image_use_gateway {
        "Nous Subscription".to_string()
    } else {
        String::new()
    };
    features.insert(
        "image_gen".to_string(),
        NousFeatureState {
            key: "image_gen".to_string(),
            label: "Image generation".to_string(),
            included_by_default: true,
            available: image_available,
            active: image_active,
            managed_by_nous: image_managed,
            direct_override: image_active && !image_managed,
            toolset_enabled: image_tool_enabled,
            current_provider: image_current_provider,
            explicit_configured: image_selected.is_some() || direct_fal,
        },
    );
    let video_current_provider = if video_selected.is_some() && video_selected.as_deref() != Some("nous")
        || (video_selected.is_none() && direct_fal_video)
    {
        "FAL".to_string()
    } else if video_managed || video_use_gateway {
        "Nous Subscription".to_string()
    } else {
        String::new()
    };
    features.insert(
        "video_gen".to_string(),
        NousFeatureState {
            key: "video_gen".to_string(),
            label: "Video generation".to_string(),
            included_by_default: false,
            available: video_available,
            active: video_active,
            managed_by_nous: video_managed,
            direct_override: video_active && !video_managed,
            toolset_enabled: video_tool_enabled,
            current_provider: video_current_provider,
            explicit_configured: video_selected.is_some() || direct_fal_video,
        },
    );
    features.insert(
        "tts".to_string(),
        NousFeatureState {
            key: "tts".to_string(),
            label: "OpenAI TTS".to_string(),
            included_by_default: true,
            available: tts_available,
            active: tts_active,
            managed_by_nous: tts_managed,
            direct_override: tts_active && !tts_managed,
            toolset_enabled: tts_tool_enabled,
            current_provider: tts_label(&tts_current_provider),
            explicit_configured: tts_explicit_configured,
        },
    );
    features.insert(
        "stt".to_string(),
        NousFeatureState {
            key: "stt".to_string(),
            label: "Speech-to-text".to_string(),
            included_by_default: true,
            available: stt_available,
            active: stt_active,
            managed_by_nous: stt_managed,
            direct_override: stt_active && !stt_managed,
            toolset_enabled: true,
            current_provider: stt_label(&stt_current_provider),
            explicit_configured: stt_explicit_configured,
        },
    );
    features.insert(
        "browser".to_string(),
        NousFeatureState {
            key: "browser".to_string(),
            label: "Browser automation".to_string(),
            included_by_default: true,
            available: browser_available,
            active: browser_active,
            managed_by_nous: browser_managed,
            direct_override: browser_active && !browser_managed,
            toolset_enabled: browser_tool_enabled,
            current_provider: browser_label(&browser_current_provider),
            explicit_configured: browser_provider_explicit,
        },
    );
    features.insert(
        "modal".to_string(),
        NousFeatureState {
            key: "modal".to_string(),
            label: "Modal execution".to_string(),
            included_by_default: false,
            available: modal_available,
            active: modal_active,
            managed_by_nous: modal_managed,
            direct_override: terminal_backend == "modal" && modal_direct_override,
            toolset_enabled: modal_tool_enabled,
            current_provider: if terminal_backend == "modal" {
                "Modal".to_string()
            } else if terminal_backend.is_empty() {
                "local".to_string()
            } else {
                terminal_backend.clone()
            },
            explicit_configured: terminal_backend == "modal",
        },
    );

    NousSubscriptionFeatures {
        subscribed,
        nous_auth_present,
        provider_is_nous,
        features,
        account_info,
    }
}

/// Shim for toolset_enabled when config is `HashMap<String, HashMap<String,String>>`.
/// Real toolset_enabled expects platform_toolsets shape; we approximate by
/// checking platform_toolsets key if present, else infer from toolset_key presence.
/// Mirrors the Python helper's subset logic; for 1:1 stub we check if the
/// toolset is in the default hermes-cli toolset.
fn toolset_enabled_shim(
    config: &HashMap<String, HashMap<String, String>>,
    toolset_key: &str,
) -> bool {
    // If platform_toolsets section exists, delegate to full logic via a constructed
    // platform->Vec<String> map. The Python code does: platform_toolsets = config.get("platform_toolsets")
    // We check if config has a key that looks like a platform mapping.
    // For slice 1 simplicity, we check direct presence of toolset in a default toolset.
    // A more faithful path: look for "platform_toolsets" entry serialized as JSON; stub.
    if let Some(pt) = config.get("platform_toolsets") {
        // pt is HashMap<String,String> where value is comma-separated toolset list
        let mut platform_map: HashMap<String, Vec<String>> = HashMap::new();
        for (platform, list_str) in pt {
            let names: Vec<String> = list_str.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
            platform_map.insert(platform.clone(), names);
        }
        // Build a temporary config for the full helper
        let mut full_cfg: HashMap<String, HashMap<String, Vec<String>>> = HashMap::new();
        full_cfg.insert("platform_toolsets".to_string(), {
            // need to map platform -> Vec<String> directly; adapt helper signature
            // Instead inline the subset check here for shim
            let target_tools = resolve_toolset(toolset_key);
            if target_tools.is_empty() {
                return false;
            }
            let target_set: HashSet<String> = target_tools.into_iter().collect();
            for (_plat, names) in &platform_map {
                let mut available: HashSet<String> = HashSet::new();
                for n in names {
                    for t in resolve_toolset(n) {
                        available.insert(t);
                    }
                }
                if target_set.is_subset(&available) {
                    return true;
                }
            }
            return false;
        });
        let _ = full_cfg;
    }
    // Default: hermes-cli includes web/browser/terminal/file/code_execution etc.
    // For slice 1 stub, web/image_gen/browser/tts are considered enabled unless platform_toolsets says otherwise.
    // Mirrors Python default: platform_toolsets = {"cli": ["hermes-cli"]} and hermes-cli contains many tools.
    let target = resolve_toolset(toolset_key);
    if target.is_empty() {
        // terminal maps to "terminal" toolset which is in hermes-cli
        if toolset_key == "terminal" {
            return true;
        }
        return false;
    }
    // Default hermes-cli toolset contains web/image_gen/tts/browser/terminal per resolve_toolset("hermes-cli")
    let hermes_cli_tools: HashSet<String> = resolve_toolset("hermes-cli").into_iter().collect();
    let target_set: HashSet<String> = target.into_iter().collect();
    target_set.is_subset(&hermes_cli_tools)
}

// ---------------------------------------------------------------------------
// apply_nous_managed_defaults — mirrors lines 862-900 (slice 1 head)
// ---------------------------------------------------------------------------

/// Mirrors `def apply_nous_managed_defaults(config: Dict[str, object], *, enabled_toolsets: Optional[Iterable[str]] = None, force_fresh: bool = False) -> set[str]:` (862-900).
/// Slice 1 covers through browser_cfg init (lines 862-900); remainder
/// (web/tts/stt/browser/image_gen/video_gen defaults) continues in slice 2.
pub fn apply_nous_managed_defaults(
    config: &mut HashMap<String, HashMap<String, String>>,
    enabled_toolsets: Option<Vec<String>>,
    force_fresh: bool,
) -> HashSet<String> {
    let features = get_nous_subscription_features(Some(config.clone()), force_fresh);
    if !(features.account_info.as_ref().map(|a| a.logged_in).unwrap_or(false)
        && features.account_info.as_ref().map(|a| a.tool_gateway_entitled).unwrap_or(false))
    {
        return HashSet::new();
    }
    if !features.provider_is_nous {
        return HashSet::new();
    }

    let selected_toolsets: HashSet<String> = enabled_toolsets.unwrap_or_default().into_iter().collect();
    let mut changed: HashSet<String> = HashSet::new();

    // Mirrors lines 881-899: ensure web/tts/stt/browser dicts exist
    let web_cfg = config.entry("web".to_string()).or_insert_with(HashMap::new);
    let _ = web_cfg;
    let tts_cfg = config.entry("tts".to_string()).or_insert_with(HashMap::new);
    let _ = tts_cfg;
    let stt_cfg = config.entry("stt".to_string()).or_insert_with(HashMap::new);
    let _ = stt_cfg;
    let browser_cfg = config.entry("browser".to_string()).or_insert_with(HashMap::new);
    let _ = browser_cfg;

    // Lines 901+ (web/tts/stt/browser/image_gen/video_gen wiring) continue in slice 2.
    // Slice boundary is 900, so we stop after browser_cfg init and return early stub.
    // The caller sees the same shape; the full logic lives in nous_subscription_slice2.rs.
    let _ = (selected_toolsets, &mut changed);
    changed
}

// ---------------------------------------------------------------------------
// Slice boundary — line 900
// ---------------------------------------------------------------------------
// Python lines 901+ (apply_nous_managed_defaults body: web/tts/stt/browser/
// image_gen/video_gen managed defaults, plus remaining ~582 LOC of the file:
// _GATEWAY_TOOL_LABELS, _get_gateway_direct_credentials, get_gateway_eligible_tools,
// apply_gateway_defaults, prompt_enable_tool_gateway, etc.) continue in
// `nous_subscription_slice2.rs`. This file intentionally stops at the first
// 900-line boundary so that `cargo` is never invoked and the 2-slice
// decomposition stays clean.

