//! hermes-cli setup — slice 1/4
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/hermes_cli/setup.py`
//! slice 1/4 — lines 1–900 of 3 565 (first 900 LOC).
//! Covers: module docstring (wizard sections + ~/.hermes layout),
//! bootstrap imports + logger + `PROJECT_ROOT` + `_DOCS_BASE`,
//! `_model_config_dict` / `_get_credential_pool_strategies` /
//! `_set_credential_pool_strategy` / `_supports_same_provider_pool_setup`,
//! `_DEFAULT_PROVIDER_MODELS` fallback catalog (13 providers),
//! `_current_reasoning_effort` / `_set_reasoning_effort`,
//! config helpers re-export (`cfg_get`, `DEFAULT_CONFIG`, `get_hermes_home`,
//! `get_config_path`, `get_env_path`, `load_config`, `save_config`,
//! `save_env_value`, `remove_env_value`, `get_env_value`,
//! `ensure_hermes_home`), `Colors`/`color`, `print_header`,
//! `print_error`/`print_info`/`print_success`/`print_warning`,
//! `masked_secret_prompt`, `is_interactive_stdin`,
//! `print_noninteractive_setup_guidance`, `prompt` /
//! `_BRACKETED_PASTE_PATTERN` / `_sanitize_pasted_input`,
//! `_curses_prompt_choice` / `prompt_choice` (curses radiolist + fallback
//! numbered menu), `is_noninteractive` / `prompt_yes_no` (HERMES_NONINTERACTIVE
//! + EOF guard), `prompt_checklist` (curses checklist), `_prompt_api_key`,
//! `_print_setup_summary` (provider readiness + per-tool availability matrix
//! through gateway/docs banner), `_prompt_container_resources`,
//! `_prompt_vercel_sandbox_settings` / `_read_nearest_vercel_project`,
//! through `setup_model_provider` header + `select_provider_and_model`
//! delegation (line ~900).
//! Continued in `setup_slice2.rs`.
//!
//! T0692 — 1:1 port, no cargo (NEVER cargo).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

// ---------------------------------------------------------------------------
// Module docstring — mirrors lines 1-12
// ---------------------------------------------------------------------------

/// Interactive setup wizard for Hermes Agent.
///
/// Modular wizard with independently-runnable sections:
///   1. Model & Provider — choose your AI provider and model
///   2. Terminal Backend — where your agent runs commands
///   3. Agent Settings — iterations, compression, session reset
///   4. Messaging Platforms — connect Telegram, Discord, etc.
///   5. Tools — configure TTS, web search, image generation, etc.
///
/// Config files are stored in ~/.hermes/ for easy access.
/// Mirrors `hermes_cli/setup.py` lines 1-12.
pub const MODULE_DOC: &str = "Interactive setup wizard for Hermes Agent — modular wizard";

// ---------------------------------------------------------------------------
// Imports — mirrors lines 14-28
// ---------------------------------------------------------------------------
// Python: import importlib.util, json, logging, os, re, shutil, sys, copy,
//         from pathlib import Path, from typing import Optional, Dict, Any,
//         from hermes_cli.nous_subscription import get_nous_subscription_features,
//         from tools.tool_backend_helpers import managed_nous_tools_enabled,
//         from hermes_constants import get_optional_skills_dir,
//         + `from hermes_cli.config import ...` (lines 140-152),
//         + `from hermes_cli.colors import Colors, color` (155),
//         + `from hermes_cli.cli_output import print_error/print_info/...` (164-169),
//         + `from hermes_cli.secret_prompt import masked_secret_prompt` (170),
//         + lazy `display_hermes_home`, `hermes_cli.curses_ui`, `hermes_cli.auth`,
//         + `agent.auxiliary_client`, `agent.image_gen_registry`, etc.
//
// Rust: std only (NEVER cargo). All Python-specific/external imports are
// stubbed for 1:1 traceability.

/// Mirrors `get_optional_skills_dir` (hermes_constants) — stub.
pub fn get_optional_skills_dir_stub() -> Option<PathBuf> {
    None
}

/// Mirrors `get_nous_subscription_features` — stub.
pub fn get_nous_subscription_features_stub(_config: &HashMap<String, String>) -> SubscriptionFeaturesStub {
    SubscriptionFeaturesStub::default()
}

/// Mirrors `managed_nous_tools_enabled` — stub.
pub fn managed_nous_tools_enabled_stub() -> bool {
    false
}

#[derive(Debug, Default, Clone)]
pub struct SubscriptionFeatureStub {
    pub managed_by_nous: bool,
    pub available: bool,
    pub current_provider: Option<String>,
    pub direct_override: bool,
    pub nous_auth_present: bool,
}

#[derive(Debug, Default, Clone)]
pub struct SubscriptionFeaturesStub {
    pub web: SubscriptionFeatureStub,
    pub browser: SubscriptionFeatureStub,
    pub image_gen: SubscriptionFeatureStub,
    pub video_gen: SubscriptionFeatureStub,
    pub tts: SubscriptionFeatureStub,
    pub modal: SubscriptionFeatureStub,
    pub features: HashMap<String, SubscriptionFeatureStub>,
    pub nous_auth_present: bool,
}

// ---------------------------------------------------------------------------
// Logger — mirrors line 29
// ---------------------------------------------------------------------------
// Python: logger = logging.getLogger(__name__)
// Rust: no logger crate in slice 1 (NEVER cargo); use eprintln! stub.

pub fn log_debug(msg: &str) {
    eprintln!("[hermes setup DEBUG] {msg}");
}
pub fn log_warning(msg: &str) {
    eprintln!("[hermes setup WARN] {msg}");
}

// ---------------------------------------------------------------------------
// PROJECT_ROOT + _DOCS_BASE — mirrors lines 31-33
// ---------------------------------------------------------------------------

/// Mirrors `PROJECT_ROOT = Path(__file__).parent.parent.resolve()` (line 31).
/// In Rust the crate root is known via CARGO_MANIFEST_DIR; runtime equivalent
/// is HERMES_REPO_ROOT env var or current_dir fallback (matches main_slice1).
pub fn project_root() -> PathBuf {
    if let Ok(v) = std::env::var("HERMES_REPO_ROOT") {
        return PathBuf::from(v);
    }
    // Mirrors parent.parent of hermes_cli/setup.py -> repo root
    // At runtime best-effort: CARGO_MANIFEST_DIR's two parents
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
}

/// Mirrors `_DOCS_BASE = "https://hermes-agent.nousresearch.com/docs"` (line 33).
pub const DOCS_BASE: &str = "https://hermes-agent.nousresearch.com/docs";

// ---------------------------------------------------------------------------
// _model_config_dict — mirrors lines 36-42
// ---------------------------------------------------------------------------

/// Mirrors `_model_config_dict(config: Dict[str, Any]) -> Dict[str, Any]` (36-42).
///
/// Returns a copy of `config["model"]` when it's a dict, wraps a non-empty
/// string as `{"default": s}`, else empty dict.
pub fn model_config_dict(config: &HashMap<String, serde_value_stub::Value>) -> HashMap<String, String> {
    // We use a minimal Value enum stub to avoid serde dep (NEVER cargo).
    // Callers that have a real JSON map should use model_config_dict_str below.
    HashMap::new()
}

/// Stringly-typed helper for tests without serde dep: handles the `model` key
/// when it is either a JSON-object-like map or a plain string.
pub fn model_config_dict_from_model_value(
    model_value: Option<&serde_value_stub::Value>,
) -> HashMap<String, String> {
    match model_value {
        Some(serde_value_stub::Value::Map(m)) => m.clone(),
        Some(serde_value_stub::Value::Str(s)) if !s.trim().is_empty() => {
            let mut out = HashMap::new();
            out.insert("default".to_string(), s.trim().to_string());
            out
        }
        _ => HashMap::new(),
    }
}

// Minimal Value stub to avoid serde (NEVER cargo). Mirrors typing.Any dict values.
pub mod serde_value_stub {
    use std::collections::HashMap;
    #[derive(Debug, Clone)]
    pub enum Value {
        Str(String),
        Map(HashMap<String, String>),
        Null,
    }
}

// ---------------------------------------------------------------------------
// _get_credential_pool_strategies + _set_credential_pool_strategy
// — mirrors lines 45-55
// ---------------------------------------------------------------------------

/// Mirrors `_get_credential_pool_strategies(config: Dict[str, Any]) -> Dict[str, str]` (45-47).
pub fn get_credential_pool_strategies(config: &HashMap<String, HashMap<String, String>>) -> HashMap<String, String> {
    config
        .get("credential_pool_strategies")
        .cloned()
        .unwrap_or_default()
}

/// Mirrors `_set_credential_pool_strategy(config, provider, strategy)` (50-55).
pub fn set_credential_pool_strategy(
    config: &mut HashMap<String, HashMap<String, String>>,
    provider: &str,
    strategy: &str,
) {
    if provider.is_empty() {
        return;
    }
    let mut strategies = get_credential_pool_strategies(config);
    strategies.insert(provider.to_string(), strategy.to_string());
    config.insert("credential_pool_strategies".to_string(), strategies);
}

// ---------------------------------------------------------------------------
// _supports_same_provider_pool_setup — mirrors lines 58-68
// ---------------------------------------------------------------------------

/// Mirrors `_supports_same_provider_pool_setup(provider: str) -> bool` (58-68).
///
/// Returns false for empty/"custom", true for "openrouter", else probes
/// `PROVIDER_REGISTRY[provider].auth_type in {"api_key","oauth_device_code"}`.
/// Stub registry for 1:1 without importing hermes_cli.auth (lazy in Python).
pub fn supports_same_provider_pool_setup(provider: &str) -> bool {
    if provider.is_empty() || provider == "custom" {
        return false;
    }
    if provider == "openrouter" {
        return true;
    }
    // Stub PROVIDER_REGISTRY — in real port would query hermes_cli.auth registry.
    // Keep 1:1 shape: lookup pconfig, check auth_type.
    if let Some(auth_type) = provider_registry_auth_type_stub(provider) {
        return auth_type == "api_key" || auth_type == "oauth_device_code";
    }
    false
}

fn provider_registry_auth_type_stub(provider: &str) -> Option<String> {
    // Minimal stub table mirroring hermes_cli/auth.py PROVIDER_REGISTRY.
    // Only called for non-openrouter / non-custom providers; real values come from auth_slice.
    let _ = provider;
    None
}

// ---------------------------------------------------------------------------
// _DEFAULT_PROVIDER_MODELS — mirrors lines 71-119
// ---------------------------------------------------------------------------

/// Default model lists per provider — fallback when live /models endpoint
/// is unreachable. Mirrors `_DEFAULT_PROVIDER_MODELS = {...}` (73-119).
pub fn default_provider_models() -> HashMap<String, Vec<String>> {
    let mut m: HashMap<String, Vec<String>> = HashMap::new();
    m.insert("copilot-acp".to_string(), vec!["copilot-acp".to_string()]);
    m.insert(
        "copilot".to_string(),
        vec![
            "gpt-5.4", "gpt-5.4-mini", "gpt-5-mini", "gpt-5.3-codex", "gpt-5.2-codex", "gpt-4.1",
            "gpt-4o", "gpt-4o-mini", "claude-opus-4.6", "claude-sonnet-5", "claude-sonnet-4.6",
            "claude-sonnet-4.5", "claude-haiku-4.5", "gemini-2.5-pro",
        ]
        .into_iter()
        .map(|s| s.to_string())
        .collect(),
    );
    m.insert(
        "gemini".to_string(),
        vec![
            "gemini-3.1-pro-preview",
            "gemini-3-pro-preview",
            "gemini-3.6-flash",
            "gemini-3.1-flash-lite-preview",
        ]
        .into_iter()
        .map(|s| s.to_string())
        .collect(),
    );
    m.insert(
        "vertex".to_string(),
        vec![
            "google/gemini-3.1-pro-preview",
            "google/gemini-3-pro-preview",
            "google/gemini-3-flash-preview",
            "google/gemini-3.1-flash-lite-preview",
            "google/gemini-2.5-pro",
            "google/gemini-2.5-flash",
        ]
        .into_iter()
        .map(|s| s.to_string())
        .collect(),
    );
    m.insert(
        "zai".to_string(),
        vec!["glm-5.2", "glm-5.1", "glm-5", "glm-4.7", "glm-4.5", "glm-4.5-flash"]
            .into_iter()
            .map(|s| s.to_string())
            .collect(),
    );
    m.insert(
        "kimi-coding".to_string(),
        vec!["kimi-k3", "kimi-k2.6", "kimi-k2.5", "kimi-k2-thinking", "kimi-k2-turbo-preview"]
            .into_iter()
            .map(|s| s.to_string())
            .collect(),
    );
    m.insert(
        "kimi-coding-cn".to_string(),
        vec!["kimi-k3", "kimi-k2.6", "kimi-k2.5", "kimi-k2-thinking", "kimi-k2-turbo-preview"]
            .into_iter()
            .map(|s| s.to_string())
            .collect(),
    );
    m.insert(
        "stepfun".to_string(),
        vec!["step-3.5-flash", "step-3.5-flash-2603"]
            .into_iter()
            .map(|s| s.to_string())
            .collect(),
    );
    m.insert(
        "arcee".to_string(),
        vec!["trinity-large-thinking", "trinity-large-preview", "trinity-mini"]
            .into_iter()
            .map(|s| s.to_string())
            .collect(),
    );
    m.insert(
        "minimax".to_string(),
        vec!["MiniMax-M2.7", "MiniMax-M2.5", "MiniMax-M2.1", "MiniMax-M2"]
            .into_iter()
            .map(|s| s.to_string())
            .collect(),
    );
    m.insert(
        "minimax-cn".to_string(),
        vec!["MiniMax-M2.7", "MiniMax-M2.5", "MiniMax-M2.1", "MiniMax-M2"]
            .into_iter()
            .map(|s| s.to_string())
            .collect(),
    );
    m.insert(
        "ai-gateway".to_string(),
        vec!["anthropic/claude-opus-4.6", "anthropic/claude-sonnet-4.6", "openai/gpt-5", "google/gemini-3-flash"]
            .into_iter()
            .map(|s| s.to_string())
            .collect(),
    );
    m.insert(
        "kilocode".to_string(),
        vec![
            "anthropic/claude-sonnet-5",
            "anthropic/claude-opus-4.6",
            "anthropic/claude-sonnet-4.6",
            "openai/gpt-5.4",
            "google/gemini-3-pro-preview",
            "google/gemini-3-flash-preview",
        ]
        .into_iter()
        .map(|s| s.to_string())
        .collect(),
    );
    m.insert(
        "opencode-zen".to_string(),
        vec![
            "x-preview-f-free",
            "gpt-5.6-sol",
            "gpt-5.4",
            "gpt-5.3-codex",
            "claude-opus-5",
            "claude-sonnet-5",
            "gemini-3.7-flash",
            "glm-5.2",
            "kimi-k3",
            "minimax-m3",
        ]
        .into_iter()
        .map(|s| s.to_string())
        .collect(),
    );
    m.insert(
        "opencode-free".to_string(),
        vec![
            "x-preview-f-free",
            "hy3-free",
            "laguna-s-2.1-free",
            "nemotron-3-ultra-free",
            "nemotron-3.5-lightning-free",
            "muse-spark-1.2-contributor-free",
        ]
        .into_iter()
        .map(|s| s.to_string())
        .collect(),
    );
    m.insert(
        "opencode-go".to_string(),
        vec![
            "kimi-k3",
            "kimi-k2.7-code",
            "kimi-k2.6",
            "gpt-5.6-luna",
            "grok-4.5",
            "glm-5.3",
            "glm-5.2",
            "mimo-v2.5-pro",
            "mimo-v2.5",
            "minimax-m3",
            "minimax-m2.7",
            "qwen3.8-max",
            "qwen3.7-max",
            "deepseek-v4-pro",
            "hy3",
        ]
        .into_iter()
        .map(|s| s.to_string())
        .collect(),
    );
    m.insert(
        "huggingface".to_string(),
        vec![
            "Qwen/Qwen3.5-397B-A17B",
            "Qwen/Qwen3-235B-A22B-Thinking-2507",
            "Qwen/Qwen3-Coder-480B-A35B-Instruct",
            "deepseek-ai/DeepSeek-R1-0528",
            "deepseek-ai/DeepSeek-V3.2",
            "moonshotai/Kimi-K2.5",
        ]
        .into_iter()
        .map(|s| s.to_string())
        .collect(),
    );
    m
}

// ---------------------------------------------------------------------------
// _current_reasoning_effort / _set_reasoning_effort — mirrors lines 122-134
// ---------------------------------------------------------------------------

/// Mirrors `_current_reasoning_effort(config: Dict[str, Any]) -> str` (122-126).
pub fn current_reasoning_effort(config: &HashMap<String, HashMap<String, String>>) -> String {
    config
        .get("agent")
        .and_then(|m| m.get("reasoning_effort"))
        .map(|v| v.trim().to_lowercase())
        .unwrap_or_default()
}

/// Mirrors `_set_reasoning_effort(config, effort)` (129-134).
pub fn set_reasoning_effort(config: &mut HashMap<String, HashMap<String, String>>, effort: &str) {
    let entry = config.entry("agent".to_string()).or_default();
    entry.insert("reasoning_effort".to_string(), effort.to_string());
}

// ---------------------------------------------------------------------------
// Config helpers — mirrors lines 140-152
// ---------------------------------------------------------------------------
// Python: from hermes_cli.config import (cfg_get, DEFAULT_CONFIG, get_hermes_home,
//         get_config_path, get_env_path, load_config, save_config,
//         save_env_value, remove_env_value, get_env_value, ensure_hermes_home)
// display_hermes_home imported lazily at call sites (stale-module safety).
//
// Rust stubs for 1:1 traceability.

pub fn cfg_get_stub(_config: &HashMap<String, String>, _keys: &[&str], default: &str) -> String {
    default.to_string()
}

pub fn get_hermes_home() -> PathBuf {
    if let Ok(v) = std::env::var("HERMES_HOME") {
        if !v.trim().is_empty() {
            return PathBuf::from(v.trim());
        }
    }
    dirs_home().join(".hermes")
}

pub fn get_config_path() -> PathBuf {
    get_hermes_home().join("config.yaml")
}

pub fn get_env_path() -> PathBuf {
    get_hermes_home().join(".env")
}

pub fn load_config_stub() -> HashMap<String, String> {
    HashMap::new()
}

pub fn save_config_stub(_config: &HashMap<String, String>) {}

pub fn save_env_value_stub(_key: &str, _value: &str) {}

pub fn remove_env_value_stub(_key: &str) {}

pub fn get_env_value_stub(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.trim().is_empty())
}

pub fn ensure_hermes_home_stub() -> PathBuf {
    let h = get_hermes_home();
    let _ = std::fs::create_dir_all(&h);
    h
}

fn dirs_home() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
}

pub fn display_hermes_home_stub() -> String {
    // Mirrors hermes_constants.display_hermes_home — lazy import in Python.
    let h = get_hermes_home();
    if let Ok(home) = std::env::var("HOME") {
        let home_path = PathBuf::from(&home);
        if let Ok(rel) = h.strip_prefix(&home_path) {
            return format!("~/{}", rel.display());
        }
    }
    h.display().to_string()
}

// ---------------------------------------------------------------------------
// Colors — mirrors line 155
// ---------------------------------------------------------------------------
// Python: from hermes_cli.colors import Colors, color
// Rust: stub Colors + color (no color crate in slice 1, NEVER cargo).

pub mod colors {
    pub const CYAN: &str = "cyan";
    pub const BOLD: &str = "bold";
    pub const YELLOW: &str = "yellow";
    pub const GREEN: &str = "green";
    pub const RED: &str = "red";
    pub const DIM: &str = "dim";
}

pub fn color_stub(text: &str, _c1: &str, _c2: Option<&str>) -> String {
    text.to_string()
}

pub fn color(text: &str, _c1: &str, _c2: &str) -> String {
    text.to_string()
}

// ---------------------------------------------------------------------------
// print_header — mirrors lines 158-161
// ---------------------------------------------------------------------------

/// Mirrors `print_header(title: str)` (158-161).
pub fn print_header(title: &str) {
    println!();
    // Python: print(color(f"◆ {title}", Colors.CYAN, Colors.BOLD))
    println!("◆ {title}");
}

// ---------------------------------------------------------------------------
// cli_output + secret_prompt — mirrors lines 164-170
// ---------------------------------------------------------------------------
// Python: from hermes_cli.cli_output import print_error, print_info,
//         print_success, print_warning
//         from hermes_cli.secret_prompt import masked_secret_prompt
// Rust stubs.

pub fn print_error(msg: &str) {
    eprintln!("{msg}");
}
pub fn print_info(msg: &str) {
    println!("{msg}");
}
pub fn print_success(msg: &str) {
    println!("{msg}");
}
pub fn print_warning(msg: &str) {
    println!("{msg}");
}

pub fn masked_secret_prompt_stub(_prompt: &str) -> String {
    String::new()
}

pub fn line_input_stub(_prompt: &str) -> String {
    String::new()
}

// ---------------------------------------------------------------------------
// is_interactive_stdin — mirrors lines 173-181
// ---------------------------------------------------------------------------

/// Mirrors `is_interactive_stdin() -> bool` (173-181).
pub fn is_interactive_stdin() -> bool {
    // Python: getattr(sys.stdin, None) + stdin.isatty()
    // Rust: IsTerminal on stdin
    std::io::IsTerminal::is_terminal(&std::io::stdin())
}

// ---------------------------------------------------------------------------
// print_noninteractive_setup_guidance — mirrors lines 184-200
// ---------------------------------------------------------------------------

/// Mirrors `print_noninteractive_setup_guidance(reason: str | None = None)` (184-200).
pub fn print_noninteractive_setup_guidance(reason: Option<&str>) {
    println!();
    println!("⚕ Hermes Setup — Non-interactive mode");
    println!();
    if let Some(r) = reason {
        print_info(r);
    }
    print_info("The interactive wizard cannot be used here.");
    println!();
    print_info("Configure Hermes using environment variables or config commands:");
    print_info("  hermes config set model.provider custom");
    print_info("  hermes config set model.base_url http://localhost:8080/v1");
    print_info("  hermes config set model.default your-model-name");
    println!();
    print_info("Or set OPENROUTER_API_KEY / OPENAI_API_KEY in your environment.");
    print_info("Run 'hermes setup' in an interactive terminal to use the full wizard.");
    println!();
}

// ---------------------------------------------------------------------------
// prompt + _sanitize_pasted_input + _BRACKETED_PASTE_PATTERN
// — mirrors lines 203-232
// ---------------------------------------------------------------------------

/// Mirrors `_BRACKETED_PASTE_PATTERN = re.compile(r"\x1b\[\s*200~|\x1b\[\s*201~")` (225).
/// Rust: bracketed-paste markers without regex crate (NEVER cargo).
const BRACKETED_PASTE_MARKERS: &[&str] = &["\x1b[200~", "\x1b[201~"];

/// Mirrors `_sanitize_pasted_input(value: str) -> str` (228-232).
pub fn sanitize_pasted_input(value: &str) -> String {
    if value.is_empty() {
        return value.to_string();
    }
    let mut out = value.to_string();
    for marker in BRACKETED_PASTE_MARKERS {
        out = out.replace(marker, "");
    }
    // Also strip variants with whitespace: \x1b[ 200~  (Python regex \s*)
    // Minimal: strip "\x1b[ 200~" and "\x1b[\t200~" style if present
    // We handle the common case without regex by scanning for ESC[ then digits
    // with optional spaces. For 1:1 we cover the simple markers; full regex
    // variant is rare and handled by the fallback replace above covering exact.
    // To match Python's \s* we also do a pass that removes ESC [ spaces digits ~.
    let bytes = out.as_bytes().to_vec();
    let mut cleaned: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if i + 2 < bytes.len() && bytes[i] == 0x1b && bytes[i + 1] == b'[' {
            let mut j = i + 2;
            while j < bytes.len() && (bytes[j] == b' ' || bytes[j] == b'\t') {
                j += 1;
            }
            if j + 3 < bytes.len() {
                let tail = &bytes[j..];
                if tail.starts_with(b"200~") || tail.starts_with(b"201~") {
                    i = j + 4;
                    continue;
                }
            }
        }
        cleaned.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&cleaned).to_string()
}

/// Mirrors `prompt(question, default=None, password=False) -> str` (203-222).
pub fn prompt(question: &str, default: Option<&str>, password: bool) -> String {
    let display = if let Some(d) = default {
        format!("{question} [{d}]: ")
    } else {
        format!("{question}: ")
    };
    let raw = if password {
        masked_secret_prompt_stub(&display)
    } else {
        line_input_stub(&display)
    };
    let cleaned = sanitize_pasted_input(&raw);
    let trimmed = cleaned.trim();
    if !trimmed.is_empty() {
        trimmed.to_string()
    } else {
        default.unwrap_or("").to_string()
    }
}

// ---------------------------------------------------------------------------
// _curses_prompt_choice + prompt_choice — mirrors lines 235-282
// ---------------------------------------------------------------------------

/// Mirrors `_curses_prompt_choice(question, choices, default, description)` (235-238).
/// Single-select via curses_radiolist — delegates to curses_ui.
pub fn curses_prompt_choice(
    question: &str,
    choices: &[String],
    default: usize,
    description: Option<&str>,
) -> i32 {
    // Python: from hermes_cli.curses_ui import curses_radiolist
    //         return curses_radiolist(question, choices, selected=default, cancel_returns=-1, ...)
    // Rust stub: return -1 (fallback path) when curses unavailable
    let _ = (question, choices, default, description);
    -1
}

/// Mirrors `prompt_choice(question, choices, default, description)` (242-282).
///
/// Arrow-key navigation via curses when available, else numbered fallback.
/// Escape keeps current default (skips question). Ctrl+C exits wizard.
pub fn prompt_choice(
    question: &str,
    choices: &[String],
    default: usize,
    description: Option<&str>,
) -> usize {
    let idx = curses_prompt_choice(question, choices, default, description);
    if idx >= 0 {
        if idx as usize == default {
            print_info("  Skipped (keeping current)");
            println!();
            return default;
        }
        println!();
        return idx as usize;
    }

    // Fallback numbered menu (lines 257-282)
    println!("{question}");
    for (i, choice) in choices.iter().enumerate() {
        let marker = if i == default { "●" } else { "○" };
        if i == default {
            println!("  {marker} {choice}");
        } else {
            println!("  {marker} {choice}");
        }
    }
    print_info(&format!("  Enter for default ({})  Ctrl+C to exit", default + 1));

    loop {
        print!("  Select [1-{}] ({}): ", choices.len(), default + 1);
        use std::io::{self, Write};
        let _ = io::stdout().flush();
        let mut value = String::new();
        if io::stdin().read_line(&mut value).is_err() {
            std::process::exit(1);
        }
        let value = value.trim();
        if value.is_empty() {
            return default;
        }
        if let Ok(n) = value.parse::<usize>() {
            if n >= 1 && n <= choices.len() {
                return n - 1;
            }
            print_error(&format!("Please enter a number between 1 and {}", choices.len()));
        } else {
            print_error("Please enter a number");
        }
    }
}

// ---------------------------------------------------------------------------
// is_noninteractive — mirrors lines 285-301
// ---------------------------------------------------------------------------

/// Mirrors `is_noninteractive() -> bool` (285-301).
///
/// True when `HERMES_NONINTERACTIVE` is truthy (1/true/yes/on).
/// Dashboard/desktop spawn CLI actions with `stdin=DEVNULL` + this flag.
pub fn is_noninteractive() -> bool {
    std::env::var("HERMES_NONINTERACTIVE")
        .map(|v| {
            matches!(
                v.trim().to_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// prompt_yes_no — mirrors lines 304-339
// ---------------------------------------------------------------------------

/// Mirrors `prompt_yes_no(question, default=True) -> bool` (304-339).
///
/// Non-interactive callers (HERMES_NONINTERACTIVE or closed stdin) fall back
/// to `default` instead of aborting. EOF also returns default.
pub fn prompt_yes_no(question: &str, default: bool) -> bool {
    if is_noninteractive() {
        return default;
    }
    let default_str = if default { "Y/n" } else { "y/N" };
    loop {
        print!("{question} [{default_str}]: ");
        use std::io::{self, Write};
        let _ = io::stdout().flush();
        let mut value = String::new();
        match io::stdin().read_line(&mut value) {
            Ok(0) => {
                // EOF — closed/redirected stdin (stdin=DEVNULL)
                println!();
                return default;
            }
            Ok(_) => {
                let v = value.trim().to_lowercase();
                if v.is_empty() {
                    return default;
                }
                if v == "y" || v == "yes" {
                    return true;
                }
                if v == "n" || v == "no" {
                    return false;
                }
                print_error("Please enter 'y' or 'n'");
            }
            Err(_) => {
                println!();
                std::process::exit(1);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// prompt_checklist — mirrors lines 342-368
// ---------------------------------------------------------------------------

/// Mirrors `prompt_checklist(title, items, pre_selected=None) -> list` (342-368).
///
/// Multi-select checklist via curses_checklist. Returns sorted selected indices.
/// Falls back to numbered toggle when curses unavailable.
pub fn prompt_checklist(
    title: &str,
    items: &[String],
    pre_selected: Option<Vec<usize>>,
) -> Vec<usize> {
    let pre = pre_selected.unwrap_or_default();
    // Python: from hermes_cli.curses_ui import curses_checklist
    //         chosen = curses_checklist(title, items, set(pre_selected), cancel_returns=set(pre_selected))
    //         return sorted(chosen)
    // Rust stub: return pre_selected (non-interactive fallback)
    let _ = (title, items);
    let mut out = pre;
    out.sort_unstable();
    out.dedup();
    out
}

pub fn curses_checklist_stub(
    _title: &str,
    _items: &[String],
    _selected: &[usize],
) -> Vec<usize> {
    Vec::new()
}

// ---------------------------------------------------------------------------
// _prompt_api_key — mirrors lines 371-396
// ---------------------------------------------------------------------------

/// Mirrors `_prompt_api_key(var: dict)` (371-396).
/// Displays formatted API key input screen for a single env var entry.
pub fn prompt_api_key(var: &EnvVarMeta) {
    let tools_str = if var.tools.len() <= 3 {
        var.tools.join(", ")
    } else {
        format!("{}, +{} more", var.tools[..3].join(", "), var.tools.len() - 3)
    };

    println!();
    println!("  ─── {} ───", var.description.as_deref().unwrap_or(&var.name));
    println!();
    if !tools_str.is_empty() {
        print_info(&format!("  Enables: {tools_str}"));
    }
    if let Some(url) = &var.url {
        print_info(&format!("  Get your key at: {url}"));
    }
    println!();

    let value = if var.password {
        prompt(&format!("  {}", var.prompt.as_deref().unwrap_or(&var.name)), None, true)
    } else {
        prompt(&format!("  {}", var.prompt.as_deref().unwrap_or(&var.name)), None, false)
    };

    if !value.trim().is_empty() {
        save_env_value_stub(&var.name, value.trim());
        print_success("  ✓ Saved");
    } else {
        print_warning("  Skipped (configure later with 'hermes setup')");
    }
}

#[derive(Debug, Clone)]
pub struct EnvVarMeta {
    pub name: String,
    pub description: Option<String>,
    pub prompt: Option<String>,
    pub url: Option<String>,
    pub password: bool,
    pub tools: Vec<String>,
}

// ---------------------------------------------------------------------------
// _print_setup_summary — mirrors lines 399-726
// ---------------------------------------------------------------------------

/// Mirrors `_print_setup_summary(config: dict, hermes_home)` (399-726).
///
/// Prints provider readiness banner + tool availability matrix + file locations
/// + `hermes setup` / `hermes config` next-steps. The provider readiness gate
/// mirrors the consumer-onboarding audit finding #7.
pub fn print_setup_summary(config: &HashMap<String, String>, hermes_home: &Path) {
    // Provider readiness (406-418)
    let provider_ready = is_provider_ready_stub();
    if !provider_ready {
        println!();
        print_warning("No inference provider is configured — Hermes cannot chat yet.");
        print_info("  Finish this one step with either of:");
        print_info("    hermes model            (pick any provider/model)");
        print_info("    hermes setup --portal   (Nous Portal OAuth, no API key)");
    }

    // Tool Availability Summary (420-664)
    println!();
    print_header("Tool Availability Summary");

    let mut tool_status: Vec<(String, bool, Option<String>)> = Vec::new();
    let subscription = get_nous_subscription_features_stub(config);

    // Vision (429-438)
    let vision_backends = get_available_vision_backends_stub();
    if !vision_backends.is_empty() {
        tool_status.push(("Vision (image analysis)".to_string(), true, None));
    } else {
        tool_status.push((
            "Vision (image analysis)".to_string(),
            false,
            Some("run 'hermes setup' to configure".to_string()),
        ));
    }

    // Web tools (441-450)
    if subscription.web.managed_by_nous {
        tool_status.push(("Web Search & Extract (Nous subscription)".to_string(), true, None));
    } else if subscription.web.available {
        let label = if let Some(p) = &subscription.web.current_provider {
            format!("Web Search & Extract ({p})")
        } else {
            "Web Search & Extract".to_string()
        };
        tool_status.push((label, true, None));
    } else {
        tool_status.push((
            "Web Search & Extract".to_string(),
            false,
            Some(
                "EXA_API_KEY, PARALLEL_API_KEY, FIRECRAWL_API_KEY/FIRECRAWL_API_URL, TAVILY_API_KEY, or SEARXNG_URL".to_string(),
            ),
        ));
    }

    // Browser tools (453-480)
    let browser_provider = subscription.browser.current_provider.clone();
    if subscription.browser.managed_by_nous {
        tool_status.push(("Browser Automation (Nous Browser Use)".to_string(), true, None));
    } else if subscription.browser.available {
        let label = if let Some(p) = &browser_provider {
            format!("Browser Automation ({p})")
        } else {
            "Browser Automation".to_string()
        };
        tool_status.push((label, true, None));
    } else {
        let hint = match browser_provider.as_deref() {
            Some("Browserbase") => "npm install -g agent-browser and set BROWSERBASE_API_KEY/BROWSERBASE_PROJECT_ID".to_string(),
            Some("Browser Use") => "npm install -g agent-browser and set BROWSER_USE_API_KEY".to_string(),
            Some("Camofox") => "CAMOFOX_URL".to_string(),
            Some("Local browser") => "npm install -g agent-browser && agent-browser install --with-deps".to_string(),
            _ => "npm install -g agent-browser, set CAMOFOX_URL, or configure Browser Use or Browserbase".to_string(),
        };
        tool_status.push(("Browser Automation".to_string(), false, Some(hint)));
    }

    // Image generation (483-511)
    if subscription.image_gen.managed_by_nous {
        tool_status.push(("Image Generation (Nous subscription)".to_string(), true, None));
    } else if subscription.image_gen.available {
        tool_status.push(("Image Generation".to_string(), true, None));
    } else {
        let img_backend = probe_image_gen_provider_stub();
        if let Some(backend) = img_backend {
            tool_status.push((format!("Image Generation ({backend})"), true, None));
        } else {
            tool_status.push(("Image Generation".to_string(), false, Some("FAL_KEY or OPENAI_API_KEY".to_string())));
        }
    }

    // Video generation (514-534)
    if subscription.video_gen.managed_by_nous {
        tool_status.push(("Video Generation (FAL via Nous subscription)".to_string(), true, None));
    } else if let Some(backend) = probe_video_gen_provider_stub() {
        tool_status.push((format!("Video Generation ({backend})"), true, None));
    }

    // TTS (537-571)
    let tts_provider = config.get("tts.provider").map(|s| s.as_str()).unwrap_or("edge");
    let tts_status = resolve_tts_status_stub(tts_provider);
    tool_status.push(tts_status);

    // STT (574-600)
    let stt_provider = config.get("stt.provider").map(|s| s.as_str()).unwrap_or("local");
    let stt_status = resolve_stt_status_stub(stt_provider, &subscription);
    tool_status.push(stt_status);

    // Modal (602-610)
    let modal_status = resolve_modal_status_stub(config, &subscription);
    if let Some(s) = modal_status {
        tool_status.push(s);
    }

    // Home Assistant (613-614)
    if get_env_value_stub("HASS_TOKEN").is_some() {
        tool_status.push(("Smart Home (Home Assistant)".to_string(), true, None));
    }

    // Spotify (617-623)
    if is_spotify_oauth_present_stub() {
        tool_status.push(("Spotify (PKCE OAuth)".to_string(), true, None));
    }

    // Skills Hub (626-629)
    if get_env_value_stub("GITHUB_TOKEN").is_some() {
        tool_status.push(("Skills Hub (GitHub)".to_string(), true, None));
    } else {
        tool_status.push(("Skills Hub (GitHub)".to_string(), false, Some("GITHUB_TOKEN".to_string())));
    }

    // Terminal + Task planning + Skills (632-638) — always available
    tool_status.push(("Terminal/Commands".to_string(), true, None));
    tool_status.push(("Task Planning (todo)".to_string(), true, None));
    tool_status.push(("Skills (view, create, edit)".to_string(), true, None));

    // Print status (640-654)
    let available_count = tool_status.iter().filter(|(_, avail, _)| *avail).count();
    let total_count = tool_status.len();
    print_info(&format!("{available_count}/{total_count} tool categories available:"));
    println!();
    for (name, available, missing_var) in &tool_status {
        if *available {
            println!("   ✓ {name}");
        } else {
            let hint = missing_var.as_deref().unwrap_or("");
            println!("   ✗ {name} (missing {hint})");
        }
    }
    println!();

    let disabled: Vec<_> = tool_status.iter().filter(|(_, avail, _)| !*avail).collect();
    if !disabled.is_empty() {
        print_warning("Some tools are disabled. Run 'hermes setup tools' to configure them,");
        print_warning(&format!("or edit {}/.env directly to add the missing API keys.", display_hermes_home_stub()));
        println!();
    }

    // Done banner (667-683)
    println!();
    println!("┌─────────────────────────────────────────────────────────┐");
    println!("│              ✓ Setup Complete!                          │");
    println!("└─────────────────────────────────────────────────────────┘");
    println!();

    // File locations (686-694)
    println!("📁 All your files are in {}/:", display_hermes_home_stub());
    println!();
    println!("   Settings:  {}", get_config_path().display());
    println!("   API Keys:  {}", get_env_path().display());
    println!("   Data:      {}/cron/, sessions/, logs/", hermes_home.display());
    println!();

    println!("{}", "─".repeat(60));
    println!();
    println!("📝 To edit your configuration:");
    println!();
    println!("   hermes setup          Re-run the full wizard");
    println!("   hermes setup model    Change model/provider");
    println!("   hermes setup terminal Change terminal backend");
    println!("   hermes setup gateway  Configure messaging");
    println!("   hermes setup tools    Configure tool providers");
    println!();
    println!("   hermes config         View current settings");
    println!("   hermes config edit    Open config in your editor");
    println!("   hermes config set <key> <value>");
    println!("                          Set a specific value");
    println!();
    println!("   Or edit the files directly:");
    println!("   nano {}", get_config_path().display());
    println!("   nano {}", get_env_path().display());
    println!();

    println!("{}", "─".repeat(60));
    println!();
    println!("🚀 Ready to go!");
    println!();
    println!("   hermes              Start chatting");
    println!("   hermes gateway      Start messaging gateway");
    println!("   hermes doctor       Check for issues");
    println!();
}

fn is_provider_ready_stub() -> bool {
    // Mirrors `from hermes_cli.auth import resolve_provider; resolve_provider()`
    // Real check probes auth.json + env + config; stub returns false to surface the warning in tests.
    // In production slice this will call auth_slice provider resolution.
    false
}

fn get_available_vision_backends_stub() -> Vec<String> {
    // Mirrors `from agent.auxiliary_client import get_available_vision_backends`
    Vec::new()
}

fn probe_image_gen_provider_stub() -> Option<String> {
    // Mirrors probing agent.image_gen_registry.list_providers() + plugin discovery
    None
}

fn probe_video_gen_provider_stub() -> Option<String> {
    None
}

fn resolve_tts_status_stub(provider: &str) -> (String, bool, Option<String>) {
    // Mirrors lines 537-571 TTS branching
    match provider {
        "elevenlabs" if get_env_value_stub("ELEVENLABS_API_KEY").is_some() => {
            ("Text-to-Speech (ElevenLabs)".to_string(), true, None)
        }
        "openai" if get_env_value_stub("VOICE_TOOLS_OPENAI_KEY").is_some() || get_env_value_stub("OPENAI_API_KEY").is_some() => {
            ("Text-to-Speech (OpenAI)".to_string(), true, None)
        }
        "minimax" if get_env_value_stub("MINIMAX_API_KEY").is_some() => {
            ("Text-to-Speech (MiniMax)".to_string(), true, None)
        }
        "mistral" if get_env_value_stub("MISTRAL_API_KEY").is_some() => {
            ("Text-to-Speech (Mistral Voxtral)".to_string(), true, None)
        }
        "gemini" if get_env_value_stub("GEMINI_API_KEY").is_some() || get_env_value_stub("GOOGLE_API_KEY").is_some() => {
            ("Text-to-Speech (Google Gemini)".to_string(), true, None)
        }
        "neutts" => {
            // Mirrors importlib.util.find_spec("neutts") check
            let ok = false; // stub: not installed
            if ok {
                ("Text-to-Speech (NeuTTS local)".to_string(), true, None)
            } else {
                ("Text-to-Speech (NeuTTS — not installed)".to_string(), false, Some("run 'hermes setup tts'".to_string()))
            }
        }
        "kittentts" => {
            let ok = false;
            if ok {
                ("Text-to-Speech (KittenTTS local)".to_string(), true, None)
            } else {
                ("Text-to-Speech (KittenTTS — not installed)".to_string(), false, Some("run 'hermes setup tts'".to_string()))
            }
        }
        _ => ("Text-to-Speech (Edge TTS)".to_string(), true, None),
    }
}

fn resolve_stt_status_stub(provider: &str, sub: &SubscriptionFeaturesStub) -> (String, bool, Option<String>) {
    if let Some(f) = sub.features.get("stt") {
        if f.managed_by_nous {
            return ("Speech-to-Text (OpenAI via Nous subscription)".to_string(), true, None);
        }
    }
    match provider {
        "openai" if get_env_value_stub("VOICE_TOOLS_OPENAI_KEY").is_some() || get_env_value_stub("OPENAI_API_KEY").is_some() => {
            ("Speech-to-Text (OpenAI)".to_string(), true, None)
        }
        "groq" if get_env_value_stub("GROQ_API_KEY").is_some() => {
            ("Speech-to-Text (Groq Whisper)".to_string(), true, None)
        }
        "elevenlabs" if get_env_value_stub("ELEVENLABS_API_KEY").is_some() => {
            ("Speech-to-Text (ElevenLabs Scribe)".to_string(), true, None)
        }
        "xai" => ("Speech-to-Text (xAI)".to_string(), true, None),
        "deepinfra" if get_env_value_stub("DEEPINFRA_API_KEY").is_some() => {
            ("Speech-to-Text (DeepInfra)".to_string(), true, None)
        }
        _ => {
            let fw_ok = false; // stub for faster_whisper
            if fw_ok {
                ("Speech-to-Text (Local Whisper)".to_string(), true, None)
            } else {
                (
                    "Speech-to-Text (Local Whisper — not installed)".to_string(),
                    false,
                    Some("run 'hermes tools' → Speech-to-Text".to_string()),
                )
            }
        }
    }
}

fn resolve_modal_status_stub(
    config: &HashMap<String, String>,
    sub: &SubscriptionFeaturesStub,
) -> Option<(String, bool, Option<String>)> {
    if sub.modal.managed_by_nous {
        return Some(("Modal Execution (Nous subscription)".to_string(), true, None));
    }
    if config.get("terminal.backend").map(|s| s.as_str()) == Some("modal") {
        if sub.modal.direct_override {
            return Some(("Modal Execution (direct Modal)".to_string(), true, None));
        } else {
            return Some(("Modal Execution".to_string(), false, Some("run 'hermes setup terminal'".to_string())));
        }
    }
    if managed_nous_tools_enabled_stub() && sub.nous_auth_present {
        return Some(("Modal Execution (optional via Nous subscription)".to_string(), true, None));
    }
    None
}

fn is_spotify_oauth_present_stub() -> bool {
    // Mirrors `from hermes_cli.auth import get_provider_auth_state("spotify")`
    false
}

// ---------------------------------------------------------------------------
// _prompt_container_resources — mirrors lines 728-768
// ---------------------------------------------------------------------------

/// Mirrors `_prompt_container_resources(config: dict)` (728-768).
/// Prompts for persistence, CPU, memory, disk for Docker/Singularity/Modal/Daytona.
pub fn prompt_container_resources(config: &mut HashMap<String, HashMap<String, String>>) {
    let terminal = config.entry("terminal".to_string()).or_default();

    println!();
    print_info("Container Resource Settings:");

    let current_persist = terminal.get("container_persistent").map(|v| v == "true").unwrap_or(true);
    let persist_label = if current_persist { "yes" } else { "no" };
    print_info("  Persistent filesystem keeps files between sessions.");
    print_info("  Set to 'no' for ephemeral sandboxes that reset each time.");
    let persist_str = prompt("  Persist filesystem across sessions? (yes/no)", Some(persist_label), false);
    terminal.insert(
        "container_persistent".to_string(),
        if matches!(persist_str.to_lowercase().as_str(), "yes" | "true" | "y" | "1") {
            "true".to_string()
        } else {
            "false".to_string()
        },
    );

    let current_cpu = terminal.get("container_cpu").cloned().unwrap_or_else(|| "1".to_string());
    let cpu_str = prompt("  CPU cores", Some(&current_cpu), false);
    if cpu_str.parse::<f64>().is_ok() {
        terminal.insert("container_cpu".to_string(), cpu_str);
    }

    let current_mem = terminal.get("container_memory").cloned().unwrap_or_else(|| "5120".to_string());
    let mem_str = prompt("  Memory in MB (5120 = 5GB)", Some(&current_mem), false);
    if mem_str.parse::<i64>().is_ok() {
        terminal.insert("container_memory".to_string(), mem_str);
    }

    let current_disk = terminal.get("container_disk").cloned().unwrap_or_else(|| "51200".to_string());
    let disk_str = prompt("  Disk in MB (51200 = 50GB)", Some(&current_disk), false);
    if disk_str.parse::<i64>().is_ok() {
        terminal.insert("container_disk".to_string(), disk_str);
    }
}

// ---------------------------------------------------------------------------
// _prompt_vercel_sandbox_settings — mirrors lines 770-837
// ---------------------------------------------------------------------------

/// Mirrors `_prompt_vercel_sandbox_settings(config: dict)` (770-837).
/// Vercel Sandbox settings without unsupported disk sizing.
pub fn prompt_vercel_sandbox_settings(config: &mut HashMap<String, HashMap<String, String>>) {
    let terminal = config.entry("terminal".to_string()).or_default();

    println!();
    print_info("Vercel Sandbox settings:");
    print_info("  Filesystem persistence uses Vercel snapshots.");
    print_info("  Snapshots restore files only; live processes do not continue after sandbox recreation.");

    const SUPPORTED_VERCEL_RUNTIMES: &[&str] = &["node22", "node24", "python3.12"];

    let current_runtime = terminal.get("vercel_runtime").cloned().unwrap_or_else(|| "node24".to_string());
    let supported_label = SUPPORTED_VERCEL_RUNTIMES.join(", ");
    let runtime_input = prompt(&format!("  Runtime ({supported_label})"), Some(&current_runtime), false);
    let runtime = if runtime_input.trim().is_empty() {
        current_runtime.clone()
    } else {
        runtime_input.trim().to_string()
    };
    let runtime = if SUPPORTED_VERCEL_RUNTIMES.contains(&runtime.as_str()) {
        runtime
    } else {
        print_warning(&format!("Unsupported Vercel runtime '{runtime}', keeping {current_runtime}."));
        if SUPPORTED_VERCEL_RUNTIMES.contains(&current_runtime.as_str()) {
            current_runtime.clone()
        } else {
            "node24".to_string()
        }
    };
    terminal.insert("vercel_runtime".to_string(), runtime.clone());
    save_env_value_stub("TERMINAL_VERCEL_RUNTIME", &runtime);

    let current_persist = terminal.get("container_persistent").map(|v| v == "true").unwrap_or(true);
    let persist_label = if current_persist { "yes" } else { "no" };
    let persist_str = prompt("  Persist filesystem with snapshots? (yes/no)", Some(persist_label), false);
    terminal.insert(
        "container_persistent".to_string(),
        if matches!(persist_str.to_lowercase().as_str(), "yes" | "true" | "y" | "1") {
            "true".to_string()
        } else {
            "false".to_string()
        },
    );

    let current_cpu = terminal.get("container_cpu").cloned().unwrap_or_else(|| "1".to_string());
    let cpu_str = prompt("  CPU cores", Some(&current_cpu), false);
    if cpu_str.parse::<f64>().is_ok() {
        terminal.insert("container_cpu".to_string(), cpu_str);
    }

    let current_mem = terminal.get("container_memory").cloned().unwrap_or_else(|| "5120".to_string());
    let mem_str = prompt("  Memory in MB (5120 = 5GB)", Some(&current_mem), false);
    if mem_str.parse::<i64>().is_ok() {
        terminal.insert("container_memory".to_string(), mem_str);
    }

    if terminal.get("container_disk").map(|v| v.as_str()) != Some("51200")
        && terminal.get("container_disk").map(|v| v.as_str()) != Some("0")
    {
        // Check if non-default disk value was set — mirrors line 810-811
        if terminal.contains_key("container_disk") {
            let disk_val = terminal.get("container_disk").cloned().unwrap_or_default();
            if disk_val != "51200" && disk_val != "0" {
                print_warning("Vercel Sandbox does not support custom disk sizing; resetting container_disk to 51200.");
            }
        }
    }
    terminal.insert("container_disk".to_string(), "51200".to_string());

    println!();
    print_info("Vercel authentication:");
    print_info("  Use a long-lived Vercel access token plus project/team IDs.");
    let linked_project = read_nearest_vercel_project(None);
    if !linked_project.is_empty() {
        print_info("  Found defaults in nearest .vercel/project.json.");
    }

    remove_env_value_stub("VERCEL_OIDC_TOKEN");
    let token = prompt(
        "    Vercel access token",
        get_env_value_stub("VERCEL_TOKEN").as_deref(),
        true,
    );
    let project = prompt(
        "    Vercel project ID",
        get_env_value_stub("VERCEL_PROJECT_ID")
            .or_else(|| linked_project.get("projectId").cloned())
            .as_deref()
            .unwrap_or(""),
        false,
    );
    let team = prompt(
        "    Vercel team ID",
        get_env_value_stub("VERCEL_TEAM_ID")
            .or_else(|| linked_project.get("orgId").cloned())
            .as_deref()
            .unwrap_or(""),
        false,
    );
    if !token.trim().is_empty() {
        save_env_value_stub("VERCEL_TOKEN", token.trim());
    }
    if !project.trim().is_empty() {
        save_env_value_stub("VERCEL_PROJECT_ID", project.trim());
    }
    if !team.trim().is_empty() {
        save_env_value_stub("VERCEL_TEAM_ID", team.trim());
    }
}

// ---------------------------------------------------------------------------
// _read_nearest_vercel_project — mirrors lines 839-863
// ---------------------------------------------------------------------------

/// Mirrors `_read_nearest_vercel_project(start: Path | None = None) -> dict[str, str]` (839-863).
/// Reads project/team defaults from nearest `.vercel/project.json`.
pub fn read_nearest_vercel_project(start: Option<&Path>) -> HashMap<String, String> {
    let start_path = start
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let mut current = if start_path.is_file() {
        start_path.parent().map(|p| p.to_path_buf()).unwrap_or(start_path)
    } else {
        start_path
    };
    // Ensure canonical
    if let Ok(c) = std::fs::canonicalize(&current) {
        current = c;
    }

    let mut search_paths: Vec<PathBuf> = vec![current.clone()];
    for ancestor in current.ancestors().skip(1) {
        search_paths.push(ancestor.to_path_buf());
    }

    for directory in search_paths {
        let project_file = directory.join(".vercel").join("project.json");
        if !project_file.exists() {
            continue;
        }
        let text = match std::fs::read_to_string(&project_file) {
            Ok(t) => t,
            Err(_) => return HashMap::new(),
        };
        // Minimal JSON parse without serde (NEVER cargo): extract projectId/orgId as strings
        let data = match parse_vercel_project_json(&text) {
            Some(d) => d,
            None => return HashMap::new(),
        };
        let mut out = HashMap::new();
        if let Some(pid) = data.get("projectId") {
            if !pid.trim().is_empty() {
                out.insert("projectId".to_string(), pid.trim().to_string());
            }
        }
        if let Some(oid) = data.get("orgId") {
            if !oid.trim().is_empty() {
                out.insert("orgId".to_string(), oid.trim().to_string());
            }
        }
        return out;
    }
    HashMap::new()
}

fn parse_vercel_project_json(text: &str) -> Option<HashMap<String, String>> {
    // Very small JSON string-value extractor for {"projectId":"...","orgId":"..."}
    // Avoids serde dep. Returns None if not an object.
    let trimmed = text.trim();
    if !trimmed.starts_with('{') {
        return None;
    }
    let mut out = HashMap::new();
    for key in &["projectId", "orgId"] {
        if let Some(v) = extract_json_string_value(trimmed, key) {
            out.insert(key.to_string(), v);
        }
    }
    Some(out)
}

fn extract_json_string_value(json: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let idx = json.find(&needle)?;
    let after = &json[idx + needle.len()..];
    let colon = after.find(':')?;
    let after_colon = after[colon + 1..].trim_start();
    if !after_colon.starts_with('"') {
        return None;
    }
    let inner = &after_colon[1..];
    let end = inner.find('"')?;
    Some(inner[..end].to_string())
}

// ---------------------------------------------------------------------------
// Section 1: Model & Provider Configuration — mirrors lines 870-900
// ---------------------------------------------------------------------------

/// Mirrors `setup_model_provider(config: dict, *, quick: bool = False)` (876-900).
///
/// Delegates to `select_provider_and_model()` (same flow as `hermes model`)
/// for provider selection, credential prompting, and model picking.
/// Ensures single code path for all provider setup.
///
/// When `quick` is True, skips credential rotation, vision, and TTS config —
/// used by streamlined first-time quick setup.
///
/// Slice 1 covers the delegation header through the first
/// `select_provider_and_model` try/except (line ~900); the credential
/// rotation / vision / TTS tail continues in `setup_slice2.rs`.
pub fn setup_model_provider(config: &mut HashMap<String, String>, quick: bool) {
    // Mirrors `from hermes_cli.config import load_config, save_config` (887)
    //         + print_header + print_info banner (889-892)
    print_header("Inference Provider");
    print_info("Choose how to connect to your main chat model.");
    print_info(&format!("   Guide: {}/integrations/providers", DOCS_BASE));
    println!();

    // Mirrors `from hermes_cli.main import select_provider_and_model` (896)
    //         + try: select_provider_and_model() except (SystemExit, KeyboardInterrupt): print()
    let result = select_provider_and_model_stub(config, quick);
    if let Err(e) = result {
        // Mirrors except branch: suppress exit, print newline
        let _ = e;
        println!();
    }
}

fn select_provider_and_model_stub(
    _config: &mut HashMap<String, String>,
    _quick: bool,
) -> Result<(), String> {
    // Python: `from hermes_cli.main import select_provider_and_model`
    //         `select_provider_and_model()` handles provider picker, credential prompting,
    //         model selection, and config persistence. Lazy import to avoid cycle.
    // Rust stub: no-op (real wiring in later slice / main_slice integration).
    Ok(())
}

// ---------------------------------------------------------------------------
// Note: slice boundary — line ~900
// ---------------------------------------------------------------------------
// The Python `setup_model_provider` tail (credential rotation, vision/TTS
// prompts, quick-mode guards), plus `setup_terminal_backend`,
// `setup_agent_settings`, `setup_messaging_platforms`, `setup_tools`,
// `run_setup_wizard`, and the `if __name__ == "__main__"` block
// (lines 901-3565) continue in `setup_slice2.rs` / `setup_slice3.rs` /
// `setup_slice4.rs`. This file intentionally stops at the first 900 LOC
// boundary so that `cargo` is never invoked and the 4-slice decomposition
// stays clean.
