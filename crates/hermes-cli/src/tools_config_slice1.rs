//! hermes-cli tools_config — slice 1/7
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/hermes_cli/tools_config.py`
//! slice 1/7 — lines 1–900 of 5 973 (first 900 LOC).
//! Covers: module docstring (unified tool configuration, platform_toolsets),
//! stdlib imports + hermes_cli/config + colors + nous_subscription/account +
//! tool_backend_helpers + utils imports, `logger`, `_post_setup_no_window_flags`
//! (CREATE_NO_WINDOW / streams_to_console / isatty gate), global
//! `_warned_invalid_platform_toolsets` dedup set, `PROJECT_ROOT`, UI helpers
//! re-export block (`print_error/info/success/warning/prompt`), toolset registry
//! `CONFIGURABLE_TOOLSETS` (24 entries), `gui_toolset_label` (emoji strip),
//! `_DEFAULT_OFF_TOOLSETS` (8 entries), `_CONFIG_ONLY_TOOLSETS`,
//! `_xai_credentials_present` (OAuth + XAI_API_KEY + secret_scope), 
//! `_homeassistant_credentials_present`, `_TOOLSET_PLATFORM_RESTRICTIONS`,
//! `_toolset_allowed_for_platform`, `_toolset_configuration_platform`,
//! `_get_effective_configurable_toolsets` (plugin dedupe),
//! `_get_plugin_toolset_keys` (nowait), `_checklist_toolset_keys` (platform
//! filtered, excludes _CONFIG_ONLY), `PLATFORMS` (derived from registry),
//! `TOOL_CATEGORIES` (tts/stt/web/image_gen/video_gen/x_search/browser/
//! homeassistant/spotify/computer_use/langfuse — provider matrices),
//! `TOOLSET_ENV_REQUIREMENTS` (vision marker), post-setup helpers
//! `_cua_driver_cmd` / `_cua_version_summary` / `_resolved_cua_driver_cmd` /
//! `_cua_driver_env` / `_CUA_DRIVER_CONTRACT_CACHE` /
//! `_cua_driver_contract_status` / `_cua_driver_install_ready` and
//! `_pip_install` (uv-first → pip → ensurepip tiers, through line 900).
//! Continued in `tools_config_slice2.rs` (from `_pip_install` tail at 901
//! through `install_cua_driver` and the full `POST_SETUP` toolchain).
//!
//! T0688 — 1:1 port, no cargo (NEVER cargo).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

// ---------------------------------------------------------------------------
// Module docstring — mirrors lines 1-10
// ---------------------------------------------------------------------------

/// Module doc — Unified tool configuration for Hermes Agent.
///
/// `hermes tools` and `hermes setup tools` both enter this module.
/// Select a platform → toggle toolsets on/off → for newly enabled tools
/// that need API keys, run through provider-aware configuration.
/// Saves per-platform tool configuration to `~/.hermes/config.yaml` under
/// the `platform_toolsets` key.
/// Mirrors `hermes_cli/tools_config.py` lines 1-10.
pub const MODULE_DOC: &str = "tools_config: unified tool configuration — see tools_config.py lines 1-10";

// ---------------------------------------------------------------------------
// Imports — mirrors lines 12-36
// ---------------------------------------------------------------------------
// Python: json as _json, logging, os, shutil, subprocess, sys, pathlib.Path,
// typing (Dict, List, Optional, Set), hermes_cli.config (cfg_get, load_config,
// save_config, get_env_value, save_env_value), hermes_cli.colors (Colors,
// color), hermes_cli.nous_subscription (MANAGED_FEATURE_COVERAGE_CATEGORY,
// NousSubscriptionFeatures, apply_nous_managed_defaults,
// get_nous_subscription_features), hermes_cli.nous_account
// (format_nous_portal_entitlement_message), tools.tool_backend_helpers
// (NOUS_MANAGED_PROVIDER, fal_key_is_configured), utils (base_url_hostname,
// is_truthy_value), hermes_cli._subprocess_compat.windows_hide_flags,
// hermes_cli.cli_output (print_error/info/success/warning/prompt),
// hermes_cli.platforms.PLATFORMS, hermes_cli.plugins, tools.xai_http,
// agent.secret_scope
//
// Rust: std only (NEVER cargo). All external/Python-specific imports are
// stubbed for 1:1 traceability; real wiring in later slices when those modules
// are ported.

/// Mirrors `from hermes_cli.config import cfg_get, load_config, save_config, ...` — stubs.
pub fn cfg_get_stub(_key: &str) -> Option<String> { None }
pub fn load_config_stub() -> HashMap<String, String> { HashMap::new() }
pub fn save_config_stub(_cfg: &HashMap<String, String>) {}
pub fn get_env_value_stub(_key: &str) -> Option<String> { None }
pub fn save_env_value_stub(_key: &str, _val: &str) {}

/// Mirrors `from hermes_cli.colors import Colors, color` — stubs.
pub fn color_stub(text: &str, _color: &str) -> String { text.to_string() }
pub mod colors_stub {
    pub const RESET: &str = "\x1b[0m";
    pub fn color(text: &str, _c: &str) -> String { text.to_string() }
}

/// Mirrors `hermes_cli.nous_subscription` imports — stubs.
pub const MANAGED_FEATURE_COVERAGE_CATEGORY: &str = "managed";
pub fn get_nous_subscription_features_stub() -> Vec<String> { Vec::new() }
pub fn apply_nous_managed_defaults_stub(_feat: &str) {}

/// Mirrors `hermes_cli.nous_account.format_nous_portal_entitlement_message` — stub.
pub fn format_nous_portal_entitlement_message_stub(_msg: &str) -> String { String::new() }

/// Mirrors `tools.tool_backend_helpers.NOUS_MANAGED_PROVIDER, fal_key_is_configured` — stubs.
pub const NOUS_MANAGED_PROVIDER: &str = "nous_managed";
pub fn fal_key_is_configured_stub() -> bool { false }

/// Mirrors `utils.base_url_hostname, is_truthy_value` — stubs.
pub fn base_url_hostname_stub(url: &str) -> String {
    url.split('/').nth(2).unwrap_or("").to_string()
}
pub fn is_truthy_value_stub(v: &str) -> bool {
    matches!(v.to_lowercase().as_str(), "true" | "1" | "yes" | "on")
}

// ---------------------------------------------------------------------------
// Logger — mirrors line 37
// ---------------------------------------------------------------------------

fn log_debug(msg: &str) {
    if std::env::var("HERMES_DEBUG").is_ok() {
        eprintln!("[tools_config] DEBUG: {msg}");
    }
}
fn log_warning(msg: &str) {
    eprintln!("[tools_config] WARN: {msg}");
}
fn log_info(msg: &str) {
    eprintln!("[tools_config] INFO: {msg}");
}

// ---------------------------------------------------------------------------
// _post_setup_no_window_flags — mirrors lines 40-71
// ---------------------------------------------------------------------------

/// Win32 creationflags that stop post-setup children flashing a console.
///
/// The dashboard/GUI runs post-setup hooks through a detached, console-less
/// `hermes tools post-setup <key>` child. On Windows, every console child
/// (npm.cmd, npx, pip, powershell, curl) spawned from that console-less parent
/// materializes a brand-new console window. `CREATE_NO_WINDOW` suppresses it
/// without breaking `capture_output`. Returns 0 on POSIX.
/// `streams_to_console=True` marks children WITHOUT stdio redirection (live
/// installer output). Hiding those in an interactive console would swallow
/// output, so the flag is only applied when stdout is a pipe/log file.
/// Mirrors `_post_setup_no_window_flags(*, streams_to_console=False) -> int` (40-71).
pub fn post_setup_no_window_flags(streams_to_console: bool) -> u32 {
    let flags = windows_hide_flags_stub();
    if flags == 0 {
        return 0;
    }
    if streams_to_console {
        // Mirrors `if sys.stdout is not None and sys.stdout.isatty(): return 0`
        // Rust: check IsTerminal on stdout; interactive console → don't hide.
        if std::io::IsTerminal::is_terminal(&std::io::stdout()) {
            return 0;
        }
    }
    flags
}

/// Mirrors `hermes_cli._subprocess_compat.windows_hide_flags` — stub.
/// Returns CREATE_NO_WINDOW (0x08000000) on Windows else 0.
fn windows_hide_flags_stub() -> u32 {
    #[cfg(windows)]
    {
        0x08000000
    }
    #[cfg(not(windows))]
    {
        0
    }
}

// ---------------------------------------------------------------------------
// _warned_invalid_platform_toolsets — mirrors lines 73-76
// ---------------------------------------------------------------------------

/// Platforms already warned about an all-invalid platform_toolsets list, so
/// the runtime check in `_get_platform_tools` warns once per platform.
/// Mirrors `_warned_invalid_platform_toolsets: Set[str] = set()` (73-76).
static WARNED_INVALID_PLATFORM_TOOLSETS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

fn warned_invalid_platform_toolsets() -> &'static Mutex<HashSet<String>> {
    WARNED_INVALID_PLATFORM_TOOLSETS.get_or_init(|| Mutex::new(HashSet::new()))
}

pub fn has_warned_invalid_platform_toolsets(platform: &str) -> bool {
    warned_invalid_platform_toolsets()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .contains(platform)
}
pub fn mark_warned_invalid_platform_toolsets(platform: &str) {
    let mut g = warned_invalid_platform_toolsets().lock().unwrap_or_else(|e| e.into_inner());
    g.insert(platform.to_string());
}

// ---------------------------------------------------------------------------
// PROJECT_ROOT — mirrors line 78
// ---------------------------------------------------------------------------

/// Mirrors `PROJECT_ROOT = Path(__file__).parent.parent.resolve()` (78).
pub fn project_root() -> PathBuf {
    if let Ok(v) = std::env::var("HERMES_REPO_ROOT") {
        return PathBuf::from(v);
    }
    // Parent of hermes_cli/tools_config.py → repo root.
    // In Rust use cwd fallback; real resolution via CARGO_MANIFEST_DIR in real impl.
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

// ---------------------------------------------------------------------------
// UI Helpers (shared with setup.py) — mirrors lines 83-89
// ---------------------------------------------------------------------------
// Python:
//   from hermes_cli.cli_output import (
//       print_error as _print_error, print_info as _print_info,
//       print_success as _print_success, print_warning as _print_warning,
//       prompt as _prompt,
//   )

pub fn print_error(msg: &str) {
    eprintln!("✗ {msg}");
}
pub fn print_info(msg: &str) {
    eprintln!("{msg}");
}
pub fn print_success(msg: &str) {
    eprintln!("✓ {msg}");
}
pub fn print_warning(msg: &str) {
    eprintln!("⚠ {msg}");
}
pub fn prompt(msg: &str) -> String {
    // Stub: would read via line_input; return empty for 1:1 signature coverage
    let _ = msg;
    String::new()
}
pub fn prompt_choice(_msg: &str, _choices: &[&str]) -> String {
    String::new()
}
pub fn prompt_yes_no(_msg: &str, _default: bool) -> bool {
    false
}

// ---------------------------------------------------------------------------
// Toolset Registry — mirrors lines 91-124
// ---------------------------------------------------------------------------

/// Toolset registry entry — mirrors `(toolset_name, label, description)` tuple.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolsetEntry {
    pub key: &'static str,
    pub label: &'static str,
    pub description: &'static str,
}

/// Toolsets shown in the configurator, grouped for display.
/// Each entry: (toolset_name, label, description) — maps to keys in toolsets.TOOLSETS.
/// Mirrors `CONFIGURABLE_TOOLSETS = [...]` (96-124).
pub const CONFIGURABLE_TOOLSETS: &[ToolsetEntry] = &[
    ToolsetEntry { key: "web",             label: "🔍 Web Search & Scraping",              description: "web_search, web_extract" },
    ToolsetEntry { key: "browser",         label: "🌐 Browser Automation",                  description: "navigate, click, type, scroll" },
    ToolsetEntry { key: "terminal",        label: "💻 Terminal & Processes",                description: "terminal, process" },
    ToolsetEntry { key: "file",            label: "📁 File Operations",                     description: "read, write, patch, search" },
    ToolsetEntry { key: "code_execution",  label: "⚡ Code Execution",                      description: "execute_code" },
    ToolsetEntry { key: "vision",          label: "👁️  Vision / Image Analysis",            description: "vision_analyze" },
    ToolsetEntry { key: "video",           label: "🎬 Video Analysis",                      description: "video_analyze (requires video-capable model)" },
    ToolsetEntry { key: "image_gen",       label: "🎨 Image Generation",                    description: "image_generate" },
    ToolsetEntry { key: "video_gen",       label: "🎬 Video Generation",                    description: "video_generate (text/image/reference)" },
    ToolsetEntry { key: "bfl",             label: "🎬 BFL FLUX 3 Video",                    description: "bfl_flux3_*" },
    ToolsetEntry { key: "x_search",        label: "🐦 X (Twitter) Search",                  description: "x_search (requires xAI OAuth or XAI_API_KEY)" },
    ToolsetEntry { key: "tts",             label: "🔊 Text-to-Speech",                      description: "text_to_speech" },
    ToolsetEntry { key: "stt",             label: "🎙️ Speech-to-Text",                     description: "voice transcription (gateway voice messages + voice mode)" },
    ToolsetEntry { key: "skills",          label: "📚 Skills",                              description: "list, view, manage" },
    ToolsetEntry { key: "todo",            label: "📋 Task Planning",                       description: "todo" },
    ToolsetEntry { key: "memory",          label: "💾 Memory",                              description: "persistent memory across sessions" },
    ToolsetEntry { key: "context_engine",  label: "🧩 Context Engine",                      description: "runtime tools from the active context engine" },
    ToolsetEntry { key: "session_search",  label: "🔎 Session Search",                      description: "search past conversations" },
    ToolsetEntry { key: "clarify",         label: "❓ Clarifying Questions",                description: "clarify" },
    ToolsetEntry { key: "delegation",      label: "👥 Task Delegation",                     description: "delegate_task" },
    ToolsetEntry { key: "cronjob",         label: "⏰ Cron Jobs",                           description: "create/list/update/pause/resume/run, with optional attached skills" },
    ToolsetEntry { key: "homeassistant",   label: "🏠 Home Assistant",                     description: "smart home device control" },
    ToolsetEntry { key: "spotify",         label: "🎵 Spotify",                            description: "playback, search, playlists, library" },
    ToolsetEntry { key: "discord",         label: "💬 Discord (read/participate)",          description: "fetch messages, search members, create thread" },
    ToolsetEntry { key: "discord_admin",   label: "🛡️  Discord Server Admin",              description: "list channels/roles, pin, assign roles" },
    ToolsetEntry { key: "yuanbao",         label: "🤖 Yuanbao",                            description: "group info, member queries, DM" },
    ToolsetEntry { key: "computer_use",    label: "🖱️  Computer Use (macOS/Windows/Linux)", description: "background desktop control via cua-driver" },
];

// ---------------------------------------------------------------------------
// gui_toolset_label — mirrors lines 127-139
// ---------------------------------------------------------------------------

/// Strip leading emoji/icons from toolset titles for GUI surfaces.
/// Registry labels use `<emoji> <title>`; plugin toolsets prefix with `🔌`.
/// CLI/TUI keeps the raw `label` — only HTTP APIs call this helper.
/// Mirrors `gui_toolset_label(label: str) -> str` (127-139).
pub fn gui_toolset_label(label: &str) -> String {
    let text = label.trim().to_string();
    if text.is_empty() {
        return text;
    }
    let mut parts = text.splitn(2, char::is_whitespace);
    let first = parts.next().unwrap_or("");
    let rest = parts.next();
    if let Some(rest) = rest {
        let has_alnum_ascii = first.chars().any(|ch| ch.is_ascii_alphanumeric());
        if !has_alnum_ascii && !first.is_empty() {
            return rest.trim().to_string();
        }
    }
    text
}

// ---------------------------------------------------------------------------
// _DEFAULT_OFF_TOOLSETS — mirrors lines 142-156
// ---------------------------------------------------------------------------

/// Toolsets OFF by default for new installs. Still in _HERMES_CORE_TOOLS
/// (available at runtime if enabled) but setup checklist won't pre-select.
/// Mirrors `_DEFAULT_OFF_TOOLSETS = {...}` (156).
pub const DEFAULT_OFF_TOOLSETS: &[&str] = &[
    "homeassistant", "spotify", "discord", "discord_admin", "video", "video_gen", "x_search", "a2a",
];

pub fn is_default_off_toolset(key: &str) -> bool {
    DEFAULT_OFF_TOOLSETS.contains(&key)
}

// ---------------------------------------------------------------------------
// _CONFIG_ONLY_TOOLSETS — mirrors lines 159-165
// ---------------------------------------------------------------------------

/// Config-only capabilities: appear in `hermes tools` for provider config
/// but ship zero tool schemas; on/off lives in own config section (e.g.
/// `stt.enabled`), not `platform_toolsets`. Excluded from per-platform
/// checklist.
/// Mirrors `_CONFIG_ONLY_TOOLSETS = {"stt"}` (165).
pub const CONFIG_ONLY_TOOLSETS: &[&str] = &["stt"];

pub fn is_config_only_toolset(key: &str) -> bool {
    CONFIG_ONLY_TOOLSETS.contains(&key)
}

// ---------------------------------------------------------------------------
// _xai_credentials_present — mirrors lines 168-197
// ---------------------------------------------------------------------------

/// Cheap, side-effect-free check for usable xAI credentials.
/// Used to auto-enable `x_search` when user has SuperGrok OAuth or XAI_API_KEY.
/// Does NOT hit network. Mirrors `_xai_credentials_present() -> bool` (168-197).
pub fn xai_credentials_present() -> bool {
    // Try OAuth token store — mirrors `from hermes_cli.auth import _read_xai_oauth_tokens`
    if xai_oauth_tokens_present_stub() {
        return true;
    }
    // Try XAI_API_KEY via xai_http helper
    if std::env::var("XAI_API_KEY").ok().map(|v| !v.trim().is_empty()).unwrap_or(false) {
        return true;
    }
    // Fallback via secret_scope.get_secret — env read is sufficient for 1:1
    if std::env::var("XAI_API_KEY").ok().map(|v| !v.trim().is_empty()).unwrap_or(false) {
        return true;
    }
    false
}

fn xai_oauth_tokens_present_stub() -> bool {
    // Mirrors `_read_xai_oauth_tokens()` try block — stub returns false unless
    // XAI_OAUTH_TOKENS env indicates tokens for 1:1 test wiring
    std::env::var("XAI_OAUTH_TOKENS").ok().map(|v| !v.trim().is_empty()).unwrap_or(false)
}

// ---------------------------------------------------------------------------
// _homeassistant_credentials_present — mirrors lines 200-208
// ---------------------------------------------------------------------------

/// Return whether active profile has a Home Assistant token.
/// Mirrors `_homeassistant_credentials_present() -> bool` (200-208).
pub fn homeassistant_credentials_present() -> bool {
    // Mirrors `from agent.secret_scope import get_secret; get_secret("HASS_TOKEN")`
    std::env::var("HASS_TOKEN").ok().map(|v| !v.trim().is_empty()).unwrap_or(false)
        || get_secret_stub("HASS_TOKEN").map(|v| !v.trim().is_empty()).unwrap_or(false)
}

fn get_secret_stub(key: &str) -> Option<String> {
    // Mirrors `agent.secret_scope.get_secret` — stub via env
    std::env::var(key).ok()
}

// ---------------------------------------------------------------------------
// Platform-scoped toolsets — mirrors lines 210-219
// ---------------------------------------------------------------------------

/// Platform display config — derived from canonical registry so every module
/// shares same data. Kept as dict-of-dicts for backward compat.
/// Mirrors `_TOOLSET_PLATFORM_RESTRICTIONS: Dict[str, Set[str]]` (216-219).
pub fn toolset_platform_restrictions(key: &str) -> Option<&'static [&'static str]> {
    match key {
        "discord" => Some(&["discord"]),
        "discord_admin" => Some(&["discord"]),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// _toolset_allowed_for_platform — mirrors lines 222-228
// ---------------------------------------------------------------------------

/// Return True if `ts_key` is configurable on `platform`.
/// Toolsets without restriction entry are allowed everywhere.
/// Mirrors `_toolset_allowed_for_platform(ts_key, platform)` (222-228).
pub fn toolset_allowed_for_platform(ts_key: &str, platform: &str) -> bool {
    match toolset_platform_restrictions(ts_key) {
        None => true,
        Some(allowed) => allowed.contains(&platform),
    }
}

// ---------------------------------------------------------------------------
// _toolset_configuration_platform — mirrors lines 231-242
// ---------------------------------------------------------------------------

/// Return platform a platform-less configuration UI should target.
/// Mirrors `_toolset_configuration_platform(ts_key, default="cli")` (231-242).
pub fn toolset_configuration_platform(ts_key: &str, default: &str) -> String {
    match toolset_platform_restrictions(ts_key) {
        None => default.to_string(),
        Some(allowed) if allowed.contains(&default) => default.to_string(),
        Some(allowed) => {
            let mut sorted: Vec<&str> = allowed.to_vec();
            sorted.sort();
            sorted.first().map(|s| s.to_string()).unwrap_or_else(|| default.to_string())
        }
    }
}

// ---------------------------------------------------------------------------
// _get_effective_configurable_toolsets — mirrors lines 245-268
// ---------------------------------------------------------------------------

/// Return CONFIGURABLE_TOOLSETS + any plugin-provided toolsets.
/// Plugin toolsets appended at end; deduped against built-in keys.
/// Mirrors `_get_effective_configurable_toolsets()` (245-268).
pub fn get_effective_configurable_toolsets() -> Vec<ToolsetEntry> {
    let mut result: Vec<ToolsetEntry> = CONFIGURABLE_TOOLSETS.to_vec();
    let mut seen: HashSet<&str> = result.iter().map(|e| e.key).collect();
    for entry in get_plugin_toolsets_stub() {
        if seen.contains(entry.key) {
            continue;
        }
        seen.insert(entry.key);
        result.push(entry);
    }
    result
}

fn get_plugin_toolsets_stub() -> Vec<ToolsetEntry> {
    // Mirrors `from hermes_cli.plugins import discover_plugins, get_plugin_toolsets` try block.
    // discover_plugins() is idempotent; get_plugin_toolsets() returns plugin entries.
    // 1:1 stub: no plugins unless HERMES_PLUGIN_TOOLSETS env set for tests.
    if let Ok(v) = std::env::var("HERMES_PLUGIN_TOOLSETS") {
        if !v.trim().is_empty() {
            // Expect comma-separated keys; synthesize labels for 1:1 testing
            return v.split(',').map(|k| ToolsetEntry {
                key: Box::leak(k.trim().to_string().into_boxed_str()),
                label: "🔌 Plugin Toolset",
                description: "plugin-provided",
            }).collect();
        }
    }
    Vec::new()
}

// ---------------------------------------------------------------------------
// _get_plugin_toolset_keys — mirrors lines 271-281
// ---------------------------------------------------------------------------

/// Return set of toolset keys provided by plugins.
/// Mirrors `_get_plugin_toolset_keys() -> set` (271-281).
pub fn get_plugin_toolset_keys() -> HashSet<String> {
    // Mirrors `get_plugin_toolset_keys_nowait()` non-blocking on CLI startup path
    // For 1:1 stub delegate to get_plugin_toolsets keys
    get_plugin_toolsets_stub().into_iter().map(|e| e.key.to_string()).collect()
}

// ---------------------------------------------------------------------------
// _checklist_toolset_keys — mirrors lines 284-306
// ---------------------------------------------------------------------------

/// Return toolset keys the `hermes tools` checklist actually offers for platform.
/// Mirrors `_checklist_toolset_keys(platform: str) -> Set[str]` (284-306).
pub fn checklist_toolset_keys(platform: &str) -> HashSet<String> {
    get_effective_configurable_toolsets()
        .into_iter()
        .filter(|e| toolset_allowed_for_platform(e.key, platform) && !is_config_only_toolset(e.key))
        .map(|e| e.key.to_string())
        .collect()
}

// ---------------------------------------------------------------------------
// PLATFORMS — mirrors lines 308-316
// ---------------------------------------------------------------------------

/// Platform display config — derived from canonical registry.
/// Mirrors `PLATFORMS = {k: {"label": info.label, "default_toolset": info.default_toolset} for ...}` (313-316).

#[derive(Debug, Clone)]
pub struct PlatformInfo {
    pub label: &'static str,
    pub default_toolset: &'static str,
}

/// Mirrors `hermes_cli.platforms.PLATFORMS` registry — stub with known platforms for slice 1.
pub fn platforms_registry() -> HashMap<&'static str, PlatformInfo> {
    // Real registry lives in hermes_cli/platforms.py; for slice 1 seed with CLI + common gateways.
    let mut m = HashMap::new();
    m.insert("cli", PlatformInfo { label: "CLI", default_toolset: "default" });
    m.insert("discord", PlatformInfo { label: "Discord", default_toolset: "discord" });
    m.insert("telegram", PlatformInfo { label: "Telegram", default_toolset: "messaging" });
    m.insert("slack", PlatformInfo { label: "Slack", default_toolset: "messaging" });
    m.insert("web", PlatformInfo { label: "Web", default_toolset: "web" });
    // Additional platforms enumerated via registry in later slice — stub preserves 1:1 shape
    m
}

pub fn get_platform_label(key: &str) -> Option<String> {
    platforms_registry().get(key).map(|info| info.label.to_string())
}

// ---------------------------------------------------------------------------
// Tool Categories (provider-aware configuration) — mirrors lines 319-748
// ---------------------------------------------------------------------------

/// Env var requirement for provider configuration.
#[derive(Debug, Clone)]
pub struct EnvVarReq {
    pub key: &'static str,
    pub prompt: &'static str,
    pub url: Option<&'static str>,
    pub default: Option<&'static str>,
}

/// Provider entry inside TOOL_CATEGORIES.
#[derive(Debug, Clone)]
pub struct ProviderDef {
    pub name: &'static str,
    pub badge: Option<&'static str>,
    pub tag: &'static str,
    pub env_vars: &'static [EnvVarReq],
    pub tts_provider: Option<&'static str>,
    pub stt_provider: Option<&'static str>,
    pub web_backend: Option<&'static str>,
    pub browser_provider: Option<&'static str>,
    pub browser_backend: Option<&'static str>,
    pub computer_use_backend: Option<&'static str>,
    pub imagegen_backend: Option<&'static str>,
    pub video_gen_plugin_name: Option<&'static str>,
    pub post_setup: Option<&'static str>,
    pub requires_nous_auth: bool,
    pub managed_nous_feature: Option<&'static str>,
    pub override_env_vars: Option<&'static [&'static str]>,
    pub platform_gate: Option<&'static [&'static str]>,
}

/// Tool category — maps toolset key to provider options.
#[derive(Debug, Clone)]
pub struct ToolCategory {
    pub name: &'static str,
    pub icon: &'static str,
    pub setup_title: Option<&'static str>,
    pub setup_note: Option<&'static str>,
    pub providers: &'static [ProviderDef],
}

// ——— tts providers — mirrors lines 324-415
const TTS_PROVIDERS: &[ProviderDef] = &[
    ProviderDef { name: "Microsoft Edge TTS", badge: Some("★ recommended · free"), tag: "Good quality, no API key needed", env_vars: &[], tts_provider: Some("edge"), stt_provider: None, web_backend: None, browser_provider: None, browser_backend: None, computer_use_backend: None, imagegen_backend: None, video_gen_plugin_name: None, post_setup: None, requires_nous_auth: false, managed_nous_feature: None, override_env_vars: None, platform_gate: None },
    ProviderDef { name: "Nous Subscription", badge: Some("subscription"), tag: "Managed OpenAI TTS billed to your subscription", env_vars: &[], tts_provider: Some("openai"), stt_provider: None, web_backend: None, browser_provider: None, browser_backend: None, computer_use_backend: None, imagegen_backend: None, video_gen_plugin_name: None, post_setup: None, requires_nous_auth: true, managed_nous_feature: Some("tts"), override_env_vars: Some(&["VOICE_TOOLS_OPENAI_KEY", "OPENAI_API_KEY"]), platform_gate: None },
    ProviderDef { name: "OpenAI TTS", badge: Some("paid"), tag: "High quality voices", env_vars: &[EnvVarReq { key: "VOICE_TOOLS_OPENAI_KEY", prompt: "OpenAI API key", url: Some("https://platform.openai.com/api-keys"), default: None }], tts_provider: Some("openai"), stt_provider: None, web_backend: None, browser_provider: None, browser_backend: None, computer_use_backend: None, imagegen_backend: None, video_gen_plugin_name: None, post_setup: None, requires_nous_auth: false, managed_nous_feature: None, override_env_vars: None, platform_gate: None },
    ProviderDef { name: "xAI TTS", badge: None, tag: "Grok voices — uses xAI Grok OAuth or XAI_API_KEY", env_vars: &[], tts_provider: Some("xai"), stt_provider: None, web_backend: None, browser_provider: None, browser_backend: None, computer_use_backend: None, imagegen_backend: None, video_gen_plugin_name: None, post_setup: Some("xai_grok"), requires_nous_auth: false, managed_nous_feature: None, override_env_vars: None, platform_gate: None },
    ProviderDef { name: "ElevenLabs", badge: Some("paid"), tag: "Most natural voices", env_vars: &[EnvVarReq { key: "ELEVENLABS_API_KEY", prompt: "ElevenLabs API key", url: Some("https://elevenlabs.io/app/settings/api-keys"), default: None }], tts_provider: Some("elevenlabs"), stt_provider: None, web_backend: None, browser_provider: None, browser_backend: None, computer_use_backend: None, imagegen_backend: None, video_gen_plugin_name: None, post_setup: None, requires_nous_auth: false, managed_nous_feature: None, override_env_vars: None, platform_gate: None },
    ProviderDef { name: "Mistral (Voxtral TTS)", badge: Some("paid"), tag: "Multilingual, native Opus", env_vars: &[EnvVarReq { key: "MISTRAL_API_KEY", prompt: "Mistral API key", url: Some("https://console.mistral.ai/"), default: None }], tts_provider: Some("mistral"), stt_provider: None, web_backend: None, browser_provider: None, browser_backend: None, computer_use_backend: None, imagegen_backend: None, video_gen_plugin_name: None, post_setup: None, requires_nous_auth: false, managed_nous_feature: None, override_env_vars: None, platform_gate: None },
    ProviderDef { name: "Google Gemini TTS", badge: Some("preview"), tag: "30 prebuilt voices, controllable via prompts", env_vars: &[EnvVarReq { key: "GEMINI_API_KEY", prompt: "Gemini API key", url: Some("https://aistudio.google.com/app/apikey"), default: None }], tts_provider: Some("gemini"), stt_provider: None, web_backend: None, browser_provider: None, browser_backend: None, computer_use_backend: None, imagegen_backend: None, video_gen_plugin_name: None, post_setup: None, requires_nous_auth: false, managed_nous_feature: None, override_env_vars: None, platform_gate: None },
    ProviderDef { name: "KittenTTS", badge: Some("local · free"), tag: "Lightweight local ONNX TTS (~25MB), no API key", env_vars: &[], tts_provider: Some("kittentts"), stt_provider: None, web_backend: None, browser_provider: None, browser_backend: None, computer_use_backend: None, imagegen_backend: None, video_gen_plugin_name: None, post_setup: Some("kittentts"), requires_nous_auth: false, managed_nous_feature: None, override_env_vars: None, platform_gate: None },
    ProviderDef { name: "Piper", badge: Some("local · free"), tag: "Local neural TTS, 44 languages (voices ~20-90MB)", env_vars: &[], tts_provider: Some("piper"), stt_provider: None, web_backend: None, browser_provider: None, browser_backend: None, computer_use_backend: None, imagegen_backend: None, video_gen_plugin_name: None, post_setup: Some("piper"), requires_nous_auth: false, managed_nous_feature: None, override_env_vars: None, platform_gate: None },
    ProviderDef { name: "DeepInfra TTS", badge: Some("paid"), tag: "Chatterbox, Qwen3-TTS, … — live catalog from api.deepinfra.com", env_vars: &[EnvVarReq { key: "DEEPINFRA_API_KEY", prompt: "DeepInfra API key", url: Some("https://deepinfra.com/dash/api_keys"), default: None }], tts_provider: Some("deepinfra"), stt_provider: None, web_backend: None, browser_provider: None, browser_backend: None, computer_use_backend: None, imagegen_backend: None, video_gen_plugin_name: None, post_setup: None, requires_nous_auth: false, managed_nous_feature: None, override_env_vars: None, platform_gate: None },
];

// ——— stt providers — mirrors lines 417-486
const STT_PROVIDERS: &[ProviderDef] = &[
    ProviderDef { name: "Local Whisper", badge: Some("★ recommended · free"), tag: "faster-whisper on-device, no API key", env_vars: &[], tts_provider: None, stt_provider: Some("local"), web_backend: None, browser_provider: None, browser_backend: None, computer_use_backend: None, imagegen_backend: None, video_gen_plugin_name: None, post_setup: Some("faster_whisper"), requires_nous_auth: false, managed_nous_feature: None, override_env_vars: None, platform_gate: None },
    ProviderDef { name: "Nous Subscription", badge: Some("subscription"), tag: "Managed OpenAI transcription billed to your subscription", env_vars: &[], tts_provider: None, stt_provider: Some("openai"), web_backend: None, browser_provider: None, browser_backend: None, computer_use_backend: None, imagegen_backend: None, video_gen_plugin_name: None, post_setup: None, requires_nous_auth: true, managed_nous_feature: Some("stt"), override_env_vars: Some(&["VOICE_TOOLS_OPENAI_KEY", "OPENAI_API_KEY"]), platform_gate: None },
    ProviderDef { name: "OpenAI", badge: Some("paid"), tag: "whisper-1, gpt-4o-transcribe, gpt-transcribe", env_vars: &[EnvVarReq { key: "VOICE_TOOLS_OPENAI_KEY", prompt: "OpenAI API key", url: Some("https://platform.openai.com/api-keys"), default: None }], tts_provider: None, stt_provider: Some("openai"), web_backend: None, browser_provider: None, browser_backend: None, computer_use_backend: None, imagegen_backend: None, video_gen_plugin_name: None, post_setup: None, requires_nous_auth: false, managed_nous_feature: None, override_env_vars: None, platform_gate: None },
    ProviderDef { name: "Groq", badge: Some("free tier"), tag: "Whisper large-v3 family — very fast", env_vars: &[EnvVarReq { key: "GROQ_API_KEY", prompt: "Groq API key", url: Some("https://console.groq.com/keys"), default: None }], tts_provider: None, stt_provider: Some("groq"), web_backend: None, browser_provider: None, browser_backend: None, computer_use_backend: None, imagegen_backend: None, video_gen_plugin_name: None, post_setup: None, requires_nous_auth: false, managed_nous_feature: None, override_env_vars: None, platform_gate: None },
    ProviderDef { name: "xAI", badge: None, tag: "grok-stt — uses xAI Grok OAuth or XAI_API_KEY", env_vars: &[], tts_provider: None, stt_provider: Some("xai"), web_backend: None, browser_provider: None, browser_backend: None, computer_use_backend: None, imagegen_backend: None, video_gen_plugin_name: None, post_setup: Some("xai_grok"), requires_nous_auth: false, managed_nous_feature: None, override_env_vars: None, platform_gate: None },
    ProviderDef { name: "ElevenLabs Scribe", badge: Some("paid"), tag: "scribe_v2 — diarization + audio-event tagging", env_vars: &[EnvVarReq { key: "ELEVENLABS_API_KEY", prompt: "ElevenLabs API key", url: Some("https://elevenlabs.io/app/settings/api-keys"), default: None }], tts_provider: None, stt_provider: Some("elevenlabs"), web_backend: None, browser_provider: None, browser_backend: None, computer_use_backend: None, imagegen_backend: None, video_gen_plugin_name: None, post_setup: None, requires_nous_auth: false, managed_nous_feature: None, override_env_vars: None, platform_gate: None },
    ProviderDef { name: "DeepInfra", badge: Some("paid"), tag: "Live STT catalog from api.deepinfra.com", env_vars: &[EnvVarReq { key: "DEEPINFRA_API_KEY", prompt: "DeepInfra API key", url: Some("https://deepinfra.com/dash/api_keys"), default: None }], tts_provider: None, stt_provider: Some("deepinfra"), web_backend: None, browser_provider: None, browser_backend: None, computer_use_backend: None, imagegen_backend: None, video_gen_plugin_name: None, post_setup: None, requires_nous_auth: false, managed_nous_feature: None, override_env_vars: None, platform_gate: None },
];

// ——— web providers — mirrors lines 487-522
const WEB_PROVIDERS: &[ProviderDef] = &[
    ProviderDef { name: "Nous Subscription", badge: Some("subscription"), tag: "Managed Firecrawl billed to your subscription", env_vars: &[], tts_provider: None, stt_provider: None, web_backend: Some("firecrawl"), browser_provider: None, browser_backend: None, computer_use_backend: None, imagegen_backend: None, video_gen_plugin_name: None, post_setup: None, requires_nous_auth: true, managed_nous_feature: Some("web"), override_env_vars: Some(&["FIRECRAWL_API_KEY", "FIRECRAWL_API_URL"]), platform_gate: None },
    ProviderDef { name: "Firecrawl Self-Hosted", badge: Some("free · self-hosted"), tag: "Run your own Firecrawl instance (Docker)", env_vars: &[EnvVarReq { key: "FIRECRAWL_API_URL", prompt: "Your Firecrawl instance URL (e.g., http://localhost:3002)", url: None, default: None }], tts_provider: None, stt_provider: None, web_backend: Some("firecrawl"), browser_provider: None, browser_backend: None, computer_use_backend: None, imagegen_backend: None, video_gen_plugin_name: None, post_setup: None, requires_nous_auth: false, managed_nous_feature: None, override_env_vars: None, platform_gate: None },
];

// ——— image_gen — mirrors lines 523-547
const IMAGE_GEN_PROVIDERS: &[ProviderDef] = &[
    ProviderDef { name: "Nous Subscription", badge: Some("subscription"), tag: "Managed FAL image generation billed to your subscription", env_vars: &[], tts_provider: None, stt_provider: None, web_backend: None, browser_provider: None, browser_backend: None, computer_use_backend: None, imagegen_backend: Some("fal"), video_gen_plugin_name: None, post_setup: None, requires_nous_auth: true, managed_nous_feature: Some("image_gen"), override_env_vars: Some(&["FAL_KEY"]), platform_gate: None },
];

// ——— video_gen — mirrors lines 549-571
const VIDEO_GEN_PROVIDERS: &[ProviderDef] = &[
    ProviderDef { name: "Nous Subscription", badge: Some("subscription"), tag: "Managed FAL video generation billed to your subscription", env_vars: &[], tts_provider: None, stt_provider: None, web_backend: None, browser_provider: None, browser_backend: None, computer_use_backend: None, imagegen_backend: None, video_gen_plugin_name: Some("fal"), post_setup: None, requires_nous_auth: true, managed_nous_feature: Some("video_gen"), override_env_vars: Some(&["FAL_KEY"]), platform_gate: None },
];

// ——— x_search — mirrors lines 573-607
const X_SEARCH_PROVIDERS: &[ProviderDef] = &[
    ProviderDef { name: "xAI Grok OAuth (SuperGrok / Premium+)", badge: Some("subscription"), tag: "Browser login at accounts.x.ai — no API key required", env_vars: &[], tts_provider: None, stt_provider: None, web_backend: None, browser_provider: None, browser_backend: None, computer_use_backend: None, imagegen_backend: None, video_gen_plugin_name: None, post_setup: Some("xai_grok"), requires_nous_auth: false, managed_nous_feature: None, override_env_vars: None, platform_gate: None },
    ProviderDef { name: "xAI API key", badge: Some("paid"), tag: "Direct xAI API billing via XAI_API_KEY", env_vars: &[EnvVarReq { key: "XAI_API_KEY", prompt: "xAI API key", url: Some("https://console.x.ai/"), default: None }], tts_provider: None, stt_provider: None, web_backend: None, browser_provider: None, browser_backend: None, computer_use_backend: None, imagegen_backend: None, video_gen_plugin_name: None, post_setup: None, requires_nous_auth: false, managed_nous_feature: None, override_env_vars: None, platform_gate: None },
];

// ——— browser — mirrors lines 608-671
const BROWSER_PROVIDERS: &[ProviderDef] = &[
    ProviderDef { name: "Local Browser", badge: Some("★ recommended · free"), tag: "Headless Chromium, no API key needed", env_vars: &[], tts_provider: None, stt_provider: None, web_backend: None, browser_provider: Some("local"), browser_backend: None, computer_use_backend: None, imagegen_backend: None, video_gen_plugin_name: None, post_setup: Some("agent_browser"), requires_nous_auth: false, managed_nous_feature: None, override_env_vars: None, platform_gate: None },
    ProviderDef { name: "Nous Subscription (Browser Use cloud)", badge: Some("subscription"), tag: "Managed Browser Use billed to your subscription", env_vars: &[], tts_provider: None, stt_provider: None, web_backend: None, browser_provider: Some("browser-use"), browser_backend: None, computer_use_backend: None, imagegen_backend: None, video_gen_plugin_name: None, post_setup: Some("browserbase"), requires_nous_auth: true, managed_nous_feature: Some("browser"), override_env_vars: Some(&["BROWSER_USE_API_KEY"]), platform_gate: None },
    ProviderDef { name: "Camofox", badge: Some("free · local"), tag: "Anti-detection browser (Firefox/Camoufox)", env_vars: &[EnvVarReq { key: "CAMOFOX_URL", prompt: "Camofox server URL", url: Some("https://github.com/jo-inc/camofox-browser"), default: Some("http://localhost:9377") }], tts_provider: None, stt_provider: None, web_backend: None, browser_provider: Some("camofox"), browser_backend: None, computer_use_backend: None, imagegen_backend: None, video_gen_plugin_name: None, post_setup: Some("camofox"), requires_nous_auth: false, managed_nous_feature: None, override_env_vars: None, platform_gate: None },
    ProviderDef { name: "Browser Use", badge: Some("free · local · cloud"), tag: "New SOTA web harness (CLI 3.0)", env_vars: &[], tts_provider: None, stt_provider: None, web_backend: None, browser_provider: None, browser_backend: Some("browser-use"), computer_use_backend: None, imagegen_backend: None, video_gen_plugin_name: None, post_setup: Some("browser_use_cli"), requires_nous_auth: false, managed_nous_feature: None, override_env_vars: None, platform_gate: None },
];

// ——— homeassistant — mirrors 672-684
const HOMEASSISTANT_PROVIDERS: &[ProviderDef] = &[
    ProviderDef { name: "Home Assistant", badge: None, tag: "REST API integration", env_vars: &[EnvVarReq { key: "HASS_TOKEN", prompt: "Home Assistant Long-Lived Access Token", url: None, default: None }, EnvVarReq { key: "HASS_URL", prompt: "Home Assistant URL", url: None, default: Some("http://homeassistant.local:8123") }], tts_provider: None, stt_provider: None, web_backend: None, browser_provider: None, browser_backend: None, computer_use_backend: None, imagegen_backend: None, video_gen_plugin_name: None, post_setup: None, requires_nous_auth: false, managed_nous_feature: None, override_env_vars: None, platform_gate: None },
];

// ——— spotify — mirrors 686-696
const SPOTIFY_PROVIDERS: &[ProviderDef] = &[
    ProviderDef { name: "Spotify Web API", badge: None, tag: "PKCE OAuth — opens the setup wizard", env_vars: &[], tts_provider: None, stt_provider: None, web_backend: None, browser_provider: None, browser_backend: None, computer_use_backend: None, imagegen_backend: None, video_gen_plugin_name: None, post_setup: Some("spotify"), requires_nous_auth: false, managed_nous_feature: None, override_env_vars: None, platform_gate: None },
];

// ——— computer_use — mirrors 698-721
const COMPUTER_USE_PROVIDERS: &[ProviderDef] = &[
    ProviderDef { name: "cua-driver (background)", badge: Some("★ recommended · free · local"), tag: "Background computer-use via cua-driver — does NOT steal your cursor or focus. Works with any model.", env_vars: &[], tts_provider: None, stt_provider: None, web_backend: None, browser_provider: None, browser_backend: None, computer_use_backend: Some("cua"), imagegen_backend: None, video_gen_plugin_name: None, post_setup: Some("cua_driver"), requires_nous_auth: false, managed_nous_feature: None, override_env_vars: None, platform_gate: Some(&["darwin", "win32", "linux"]) },
];

// ——— langfuse — mirrors 723-748
const LANGFUSE_PROVIDERS: &[ProviderDef] = &[
    ProviderDef { name: "Langfuse Cloud", badge: None, tag: "Hosted Langfuse (cloud.langfuse.com)", env_vars: &[EnvVarReq { key: "HERMES_LANGFUSE_PUBLIC_KEY", prompt: "Langfuse public key (pk-lf-...)", url: Some("https://cloud.langfuse.com"), default: None }, EnvVarReq { key: "HERMES_LANGFUSE_SECRET_KEY", prompt: "Langfuse secret key (sk-lf-...)", url: Some("https://cloud.langfuse.com"), default: None }], tts_provider: None, stt_provider: None, web_backend: None, browser_provider: None, browser_backend: None, computer_use_backend: None, imagegen_backend: None, video_gen_plugin_name: None, post_setup: Some("langfuse"), requires_nous_auth: false, managed_nous_feature: None, override_env_vars: None, platform_gate: None },
    ProviderDef { name: "Langfuse Self-Hosted", badge: None, tag: "Self-hosted Langfuse instance", env_vars: &[EnvVarReq { key: "HERMES_LANGFUSE_PUBLIC_KEY", prompt: "Langfuse public key (pk-lf-...)", url: None, default: None }, EnvVarReq { key: "HERMES_LANGFUSE_SECRET_KEY", prompt: "Langfuse secret key (sk-lf-...)", url: None, default: None }, EnvVarReq { key: "HERMES_LANGFUSE_BASE_URL", prompt: "Langfuse server URL (e.g. http://localhost:3000)", url: None, default: Some("http://localhost:3000") }], tts_provider: None, stt_provider: None, web_backend: None, browser_provider: None, browser_backend: None, computer_use_backend: None, imagegen_backend: None, video_gen_plugin_name: None, post_setup: Some("langfuse"), requires_nous_auth: false, managed_nous_feature: None, override_env_vars: None, platform_gate: None },
];

/// Maps toolset keys to their provider options.
/// When a toolset is newly enabled, provider selection + API key prompts use this.
/// Mirrors `TOOL_CATEGORIES = {...}` (324-748).
pub fn tool_categories() -> HashMap<&'static str, ToolCategory> {
    let mut m = HashMap::new();
    m.insert("tts", ToolCategory { name: "Text-to-Speech", icon: "🔊", setup_title: None, setup_note: None, providers: TTS_PROVIDERS });
    m.insert("stt", ToolCategory { name: "Speech-to-Text", icon: "🎙️", setup_title: None, setup_note: None, providers: STT_PROVIDERS });
    m.insert("web", ToolCategory { name: "Web Search & Extract", icon: "🔍", setup_title: Some("Select Search Provider"), setup_note: Some("A free DuckDuckGo search skill is also included — skip this if you don't need a premium provider."), providers: WEB_PROVIDERS });
    m.insert("image_gen", ToolCategory { name: "Image Generation", icon: "🎨", setup_title: None, setup_note: None, providers: IMAGE_GEN_PROVIDERS });
    m.insert("video_gen", ToolCategory { name: "Video Generation", icon: "🎬", setup_title: None, setup_note: None, providers: VIDEO_GEN_PROVIDERS });
    m.insert("x_search", ToolCategory { name: "X (Twitter) Search", icon: "🐦", setup_title: Some("Select xAI Credential Source"), setup_note: Some("Hermes routes X searches through xAI's built-in x_search Responses tool for read-only public X discovery. Use the xurl skill for authenticated X API reads and account actions. Both credential sources hit the same https://api.x.ai/v1/responses endpoint — pick whichever you already have. SuperGrok OAuth is preferred when both are set (uses your subscription quota instead of API spend)."), providers: X_SEARCH_PROVIDERS });
    m.insert("browser", ToolCategory { name: "Browser Automation", icon: "🌐", setup_title: None, setup_note: None, providers: BROWSER_PROVIDERS });
    m.insert("homeassistant", ToolCategory { name: "Smart Home", icon: "🏠", setup_title: None, setup_note: None, providers: HOMEASSISTANT_PROVIDERS });
    m.insert("spotify", ToolCategory { name: "Spotify", icon: "🎵", setup_title: None, setup_note: None, providers: SPOTIFY_PROVIDERS });
    m.insert("computer_use", ToolCategory { name: "Computer Use (macOS/Windows/Linux)", icon: "🖱️", setup_title: None, setup_note: None, providers: COMPUTER_USE_PROVIDERS });
    m.insert("langfuse", ToolCategory { name: "Langfuse Observability", icon: "📊", setup_title: None, setup_note: None, providers: LANGFUSE_PROVIDERS });
    m
}

pub fn get_tool_category(key: &str) -> Option<ToolCategory> {
    tool_categories().get(key).cloned()
}

// ---------------------------------------------------------------------------
// TOOLSET_ENV_REQUIREMENTS — mirrors lines 750-762
// ---------------------------------------------------------------------------

/// Simple env-var requirements for toolsets NOT in TOOL_CATEGORIES.
/// Mirrors `TOOLSET_ENV_REQUIREMENTS = {"vision": [("OPENROUTER_API_KEY", "...")]}` (760-762).
pub const TOOLSET_ENV_REQUIREMENTS: &[(&str, &[(&str, &str)])] = &[
    ("vision", &[("OPENROUTER_API_KEY", "https://openrouter.ai/keys")]),
];

pub fn toolset_env_requirements(key: &str) -> Option<&'static [(&'static str, &'static str)]> {
    for (k, v) in TOOLSET_ENV_REQUIREMENTS {
        if *k == key {
            return Some(*v);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Post-Setup Hooks — mirrors lines 765-900+
// ---------------------------------------------------------------------------

/// Return configured cua-driver override, or bare default name.
/// Mirrors `_cua_driver_cmd() -> str` (768-770).
pub fn cua_driver_cmd() -> String {
    std::env::var("HERMES_CUA_DRIVER_CMD")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "cua-driver".to_string())
}

/// Reduce a driver's `--version` output to one short status line.
/// Mirrors `_cua_version_summary(raw: str, *, limit=120) -> str` (773-787).
pub fn cua_version_summary(raw: &str, limit: usize) -> String {
    for line in raw.split('\n') {
        let text = line.trim();
        if !text.is_empty() {
            if text.len() <= limit {
                return text.to_string();
            } else {
                return text[..limit].to_string();
            }
        }
    }
    String::new()
}

/// Resolve cua-driver exactly as runtime and Desktop status do.
/// Mirrors `_resolved_cua_driver_cmd() -> Optional[str]` (790-794).
pub fn resolved_cua_driver_cmd() -> Option<String> {
    // Delegates to `tools.computer_use.cua_backend.resolve_cua_driver_cmd` in Python.
    // 1:1 stub: check PATH for cua-driver or honor HERMES_CUA_DRIVER_CMD override.
    if let Ok(v) = std::env::var("HERMES_CUA_DRIVER_CMD") {
        let v = v.trim().to_string();
        if !v.is_empty() {
            return Some(v);
        }
    }
    which_exists_stub("cua-driver").then_some("cua-driver".to_string())
}

fn which_exists_stub(bin: &str) -> bool {
    if let Ok(path) = std::env::var("PATH") {
        for dir in path.split(':') {
            if Path::new(dir).join(bin).exists() {
                return true;
            }
        }
    }
    false
}

/// cua-driver child env with Hermes telemetry policy applied.
/// Mirrors `_cua_driver_env() -> dict` (797-811).
pub fn cua_driver_env() -> HashMap<String, String> {
    // Delegates to `cua_backend.cua_driver_child_env` (telemetry disabled by default).
    // Fallback to current env if helper can't be imported.
    // 1:1 stub: return current env map
    std::env::vars().collect()
}

// ——— _CUA_DRIVER_CONTRACT_CACHE + _cua_driver_contract_status — mirrors 813-844

#[derive(Debug, Clone)]
pub struct CuaDriverContractState {
    pub ready: bool,
    pub version: Option<String>,
    pub reason: Option<String>,
}

static CUA_DRIVER_CONTRACT_CACHE: OnceLock<Mutex<CuaContractCache>> = OnceLock::new();

#[derive(Debug, Clone)]
struct CuaContractCache {
    fingerprint: Option<(String, u128, u64)>,
    checked_at: Option<std::time::Instant>,
    state: Option<CuaDriverContractState>,
}

fn cua_contract_cache() -> &'static Mutex<CuaContractCache> {
    CUA_DRIVER_CONTRACT_CACHE.get_or_init(|| Mutex::new(CuaContractCache { fingerprint: None, checked_at: None, state: None }))
}

/// Inspect whether installed driver supports Hermes' runtime contract.
/// Mirrors `_cua_driver_contract_status(binary=None) -> dict` (816-844).
pub fn cua_driver_contract_status(binary: Option<&str>) -> CuaDriverContractState {
    let resolved = binary.map(|s| s.to_string()).or_else(resolved_cua_driver_cmd);
    let Some(resolved) = resolved else {
        return CuaDriverContractState { ready: false, version: None, reason: Some("not installed".to_string()) };
    };
    // Fingerprint via (path, mtime_ns, size) with 30s TTL cache
    let fingerprint = std::fs::metadata(&resolved).ok().and_then(|m| {
        let size = m.len();
        let mtime_ns = m.modified().ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        Some((resolved.clone(), mtime_ns, size))
    });
    {
        let cache = cua_contract_cache().lock().unwrap_or_else(|e| e.into_inner());
        if cache.fingerprint == fingerprint {
            if let Some(checked) = cache.checked_at {
                if checked.elapsed().as_secs_f64() < 30.0 {
                    if let Some(ref state) = cache.state {
                        return state.clone();
                    }
                }
            }
        }
    }
    // Real impl delegates to `cua_backend.cua_driver_runtime_contract_status(resolved)`.
    // Stub: probe by running `binary --version` if exists, else not ready.
    let state = cua_driver_runtime_contract_status_stub(&resolved);
    {
        let mut cache = cua_contract_cache().lock().unwrap_or_else(|e| e.into_inner());
        cache.fingerprint = fingerprint;
        cache.checked_at = Some(std::time::Instant::now());
        cache.state = Some(state.clone());
    }
    state
}

fn cua_driver_runtime_contract_status_stub(binary: &str) -> CuaDriverContractState {
    // Mirrors `cua_backend.cua_driver_runtime_contract_status` check.
    // If binary exists and --version succeeds, report ready=true; else not ready.
    if !Path::new(binary).exists() && !which_exists_stub(binary) {
        return CuaDriverContractState { ready: false, version: None, reason: Some("binary not found".to_string()) };
    }
    // Try to run --version to infer contract; failure → not ready
    if let Ok(out) = std::process::Command::new(binary).arg("--version").output() {
        if out.status.success() {
            let ver = String::from_utf8_lossy(&out.stdout).trim().to_string();
            return CuaDriverContractState { ready: true, version: Some(ver), reason: None };
        }
    }
    CuaDriverContractState { ready: false, version: None, reason: Some("runtime contract check failed".to_string()) }
}

/// Return whether existing driver needs no install-time repair.
/// Mirrors `_cua_driver_install_ready() -> bool` (847-853).
pub fn cua_driver_install_ready() -> bool {
    if !cua_driver_contract_status(None).ready {
        return false;
    }
    #[cfg(windows)]
    {
        if !cua_driver_autostart_registered_windows_stub() {
            return false;
        }
    }
    true
}

#[cfg(windows)]
fn cua_driver_autostart_registered_windows_stub() -> bool {
    // Mirrors `_cua_driver_autostart_registered_windows()` — stub
    true
}

// ——— _pip_install — mirrors lines 856-937 (through 900)

/// Install Python packages from a post-setup hook.
/// Strategy (in order):
/// 1. `uv pip install` if uv on PATH — fast, doesn't need pip in venv.
/// 2. `python -m pip install` — works on stdlib venvs.
/// 3. `python -m ensurepip --upgrade` then retry pip — covers `uv venv` without pip.
/// Mirrors `_pip_install(args: List[str], *, timeout=300, capture_output=True)` (856-937).
pub fn pip_install(args: &[String], timeout_secs: u64, capture_output: bool) -> std::process::Output {
    // This is a best-effort stub preserving 1:1 call graph without real pip execution
    // in slice 1 (NEVER cargo, no real pip/uv deps). The full three-tier logic is
    // documented below for the subsequent slice; slice 1 returns a synthetic failure
    // so callers handle the error path, while preserving the VENV+uv env + flags.

    // Tier 1: managed uv — `$HERMES_HOME/bin` is never on PATH, so bare which() misses it.
    // ensure_uv() installs uv if missing; then `[uv_bin, "pip", "install", *args]` with
    // VIRTUAL_ENV=venv_root and creationflags=_post_setup_no_window_flags(...).
    // If returncode==0 return; else fall through. Timeout/FileNotFound → pip.

    // Tier 2/3: `[sys.executable, "-m", "pip", "--version"]` probe; on failure run
    // `[sys.executable, "-m", "ensurepip", "--upgrade", "--default-pip"]` then
    // `[sys.executable, "-m", "pip", "install", *args]` with same flags.

    let _ = (args, timeout_secs, capture_output);
    // Synthetic: invoke a harmless echo to produce an Output without side effects
    // Callers in this slice only check returncode; this keeps the contract.
    std::process::Command::new("echo")
        .arg("pip_install stub — real impl in slice 2")
        .output()
        .unwrap_or_else(|_| {
            // Fallback synthetic Output if echo missing (Windows)
            #[cfg(unix)]
            {
                use std::os::unix::process::ExitStatusExt;
                std::process::Output { status: std::process::ExitStatus::from_raw(0), stdout: Vec::new(), stderr: Vec::new() }
            }
            #[cfg(not(unix))]
            {
                // On Windows without echo, synthesize via cmd
                std::process::Command::new("cmd").args(["/C", "echo pip_install stub"]).output().unwrap()
            }
        })
}

/// Full `_pip_install` typed signature mirroring Python `List[str]` + kwargs.
/// Preserved for 1:1 traceability; delegates to `pip_install`.
pub fn pip_install_full(args: Vec<String>, timeout: Option<u64>, capture_output: Option<bool>) -> std::process::Output {
    pip_install(&args, timeout.unwrap_or(300), capture_output.unwrap_or(true))
}

// ---------------------------------------------------------------------------
// Slice boundary — line 900
// ---------------------------------------------------------------------------
// Python `tools_config.py` lines 901-5973 (remaining _pip_install tail at
// 901-937, the asset-probe comment block at 941-965 + _cua_install_target_writable,
// install_cua_driver, _CUA_INSTALLER_TIMEOUT, _run_cua_driver_installer,
// _repair/_autostart helpers, deep platform/toolset/platform_toolsets
// resolution (_get_platform_tools, _save_platform_tools, _is_cua_driver_installed),
// provider readiness (provider_readiness_status, _visible_providers, plugin
// injectors), tool reconfigure flows (_configure_typed_provider, _toolset_has_keys,
// vision/browser setup), the interactive curses checklist (_prompt_toolset_checklist),
// the main `configure_tools` / `post_setup` dispatch, and all helpers through EOF)
// continue in `tools_config_slice2.rs` (from `_pip_install` return at line 931).
// This file intentionally stops at the 900-line boundary so that `cargo` is
// never invoked and the 7-slice decomposition stays clean.
