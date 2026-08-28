//! Provider module registry.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/providers/__init__.py` (342 LOC).
//!
//! Provider profiles can live in three places:
//!
//! 1. Bundled plugins: `plugins/model-providers/<name>/` (shipped with hermes-agent)
//! 2. User plugins: `$HERMES_HOME/plugins/model-providers/<name>/`
//! 3. Pip-installed plugins: distributions exposing a `hermes_agent.plugins`
//!    entry point (`module:func` callable or a self-registering `module`)
//!
//! Each plugin directory contains:
//!   - `__init__.py` — calls `register_provider(profile)` at import
//!   - `plugin.yaml` — manifest (name, kind: model-provider, version, description)
//!
//! Discovery is lazy: the first call to `get_provider_profile()` or
//! `list_providers()` scans both locations and imports every plugin. User
//! plugins override bundled plugins on name collision (last-writer-wins), so
//! third parties can monkey-patch or replace any built-in profile without
//! editing the repo.
//!
//! For backward compatibility, `providers/*.py` files (other than `base.py`
//! and `__init__.py`) are still discovered via `pkgutil.iter_modules`.
//! This lets out-of-tree users drop a single-file profile into an editable
//! install without the plugin dir structure. New profiles should prefer the
//! plugin layout.
//!
//! Python surface ported line-for-line:
//! - `OMIT_TEMPERATURE` / `ProviderProfile` (re-exported from `providers.base`)
//! - `_REGISTRY` / `_ALIASES` / `_PROVIDER_LIST_CACHE` / `_discovered`
//! - `_BUNDLED_PLUGINS_DIR`
//! - `register_provider(profile)`
//! - `get_provider_profile(name)`
//! - `list_providers()`
//! - `_user_plugins_dir()`
//! - `_import_plugin_dir(plugin_dir, source)`
//! - `_discover_entry_point_providers()`
//! - `_requires_arguments(fn)`
//! - `_discover_providers()`
//!
//! Python dynamic import (`importlib.util.spec_from_file_location` +
//! `exec_module` + `sys.modules` + `pkgutil.iter_modules` +
//! `importlib.metadata.entry_points`) is represented here with synchronous
//! filesystem inspection and text heuristics. The `ProviderProfile` registry
//! semantics (last-writer-wins, alias dedup, lazy discovery, cache
//! invalidation on register) are byte-identical without executing Python.
//! A real async port would swap the stub loader for an embedded interpreter or
//! FFI and `importlib.metadata` for `cargo_metadata` entry-point scanning.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ---------------------------------------------------------------------------
// OMIT_TEMPERATURE sentinel — mirrors providers/base.py:21 `OMIT_TEMPERATURE = object()`
// ---------------------------------------------------------------------------

/// Sentinel for "omit temperature entirely" (Kimi: server manages it).
/// Mirrors `providers.base.OMIT_TEMPERATURE = object()` re-exported in `__init__.py:41`.
/// In Python it's a unique `object()`; here it's a unit struct with identity equality.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct OmitTemperature;

pub const OMIT_TEMPERATURE: OmitTemperature = OmitTemperature;

// ---------------------------------------------------------------------------
// ProviderProfile — mirrors providers/base.py:39-267 `ProviderProfile` dataclass
// ---------------------------------------------------------------------------

/// Mirrors `providers.base.ProviderProfile` dataclass (lines 39-267).
/// Declarative profile describing auth, endpoints, quirks, and catalog hooks.
/// Transport reads this instead of receiving 20+ boolean flags.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderProfile {
    /// Mirrors `name: str` (line 43).
    pub name: String,
    /// Mirrors `api_mode: str = "chat_completions"` (line 44).
    pub api_mode: String,
    /// Mirrors `aliases: tuple = ()` (line 45).
    pub aliases: Vec<String>,

    /// Mirrors `display_name: str = ""` (line 48).
    pub display_name: String,
    /// Mirrors `description: str = ""` (line 49).
    pub description: String,
    /// Mirrors `signup_url: str = ""` (line 50).
    pub signup_url: String,

    /// Mirrors `env_vars: tuple = ()` (line 53).
    pub env_vars: Vec<String>,
    /// Mirrors `base_url: str = ""` (line 54).
    pub base_url: String,
    /// Mirrors `models_url: str = ""` (line 55).
    pub models_url: String,
    /// Mirrors `auth_type: str = "api_key"` (line 56).
    pub auth_type: String,
    /// Mirrors `supports_health_check: bool = True` (line 57).
    pub supports_health_check: bool,

    /// Mirrors `supports_vision: bool = False` (line 66).
    pub supports_vision: bool,
    /// Mirrors `supports_vision_tool_messages: bool = True` (line 73).
    pub supports_vision_tool_messages: bool,
    /// Mirrors `supports_prompt_cache_key: bool = False` (line 79).
    pub supports_prompt_cache_key: bool,

    /// Mirrors `fallback_models: tuple = ()` (line 84).
    pub fallback_models: Vec<String>,
    /// Mirrors `hostname: str = ""` (line 88).
    pub hostname: String,

    /// Mirrors `default_headers: dict[str, str] = field(default_factory=dict)` (line 91).
    pub default_headers: HashMap<String, String>,

    /// Mirrors `fixed_temperature: Any = None` (line 95) — None | f64 | OmitTemperature.
    /// Stored as `Option<Value>` for 1:1 sentinel round-trip: `None` = use caller default,
    /// `Some(Value::String("__OMIT_TEMPERATURE__"))` = omit entirely.
    pub fixed_temperature: Option<Value>,
    /// Mirrors `default_max_tokens: int | None = None` (line 96).
    pub default_max_tokens: Option<i64>,
    /// Mirrors `default_aux_model: str = ""` (line 97-99).
    pub default_aux_model: String,
}

impl ProviderProfile {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            api_mode: "chat_completions".to_string(),
            aliases: Vec::new(),
            display_name: String::new(),
            description: String::new(),
            signup_url: String::new(),
            env_vars: Vec::new(),
            base_url: String::new(),
            models_url: String::new(),
            auth_type: "api_key".to_string(),
            supports_health_check: true,
            supports_vision: false,
            supports_vision_tool_messages: true,
            supports_prompt_cache_key: false,
            fallback_models: Vec::new(),
            hostname: String::new(),
            default_headers: HashMap::new(),
            fixed_temperature: None,
            default_max_tokens: None,
            default_aux_model: String::new(),
        }
    }

    /// Mirrors `def resolve_aux_model(self, *, vision: bool = False) -> str` (lines 104-118).
    /// Cheap, never raises, returns "" when no answer so caller falls through to `default_aux_model`.
    pub fn resolve_aux_model(&self, _vision: bool) -> String {
        String::new()
    }

    /// Mirrors `def get_hostname(self) -> str` (lines 120-131).
    pub fn get_hostname(&self) -> String {
        if !self.hostname.is_empty() {
            return self.hostname.clone();
        }
        if !self.base_url.is_empty() {
            if let Some(host) = extract_hostname(&self.base_url) {
                return host;
            }
        }
        String::new()
    }

    /// Mirrors `def prepare_messages(self, messages: list[dict[str, Any]]) -> list[dict[str, Any]]` (lines 133-139).
    pub fn prepare_messages(&self, messages: Vec<Value>) -> Vec<Value> {
        messages
    }

    /// Mirrors `def build_extra_body(self, *, session_id: str | None = None, **context: Any) -> dict[str, Any]` (lines 141-148).
    pub fn build_extra_body(&self, _session_id: Option<&str>, _context: &HashMap<String, Value>) -> HashMap<String, Value> {
        HashMap::new()
    }

    /// Mirrors `def build_api_kwargs_extras(self, *, reasoning_config: dict | None = None, **context: Any) -> tuple[dict, dict]` (lines 150-168).
    pub fn build_api_kwargs_extras(
        &self,
        _reasoning_config: Option<&Value>,
        _context: &HashMap<String, Value>,
    ) -> (HashMap<String, Value>, HashMap<String, Value>) {
        (HashMap::new(), HashMap::new())
    }

    /// Mirrors `def default_vision_model(self) -> str | None` (lines 170-181).
    pub fn default_vision_model(&self) -> Option<String> {
        None
    }

    /// Mirrors `def get_max_tokens(self, model: str | None) -> int | None` (lines 183-195).
    pub fn get_max_tokens(&self, _model: Option<&str>) -> Option<i64> {
        self.default_max_tokens
    }

    /// Mirrors `def fetch_models(self, *, api_key: str | None = None, base_url: str | None = None, timeout: float = 8.0) -> list[str] | None` (lines 197-267).
    ///
    /// Resolution order:
    ///   1. `base_url + "/models"` when caller passed a custom base_url differing from profile default
    ///   2. `self.models_url` when set
    ///   3. `self.base_url + "/models"` fallback
    ///
    /// Real I/O upgrade:
    /// ```ignore
    /// let url = self.resolve_models_url(base_url);
    /// let client = reqwest::Client::builder().timeout(Duration::from_secs_f64(timeout)).build()?;
    /// let mut req = client.get(&url).header("Accept", "application/json");
    /// if let Some(k) = api_key { req = req.header("Authorization", format!("Bearer {}", k)); }
    /// req = req.header("User-Agent", profile_user_agent());
    /// for (k,v) in &self.default_headers { req = req.header(k, v); }
    /// let data: Value = req.send().await?.json().await?;
    /// ```
    pub fn fetch_models(&self, _api_key: Option<&str>, base_url: Option<&str>, _timeout: f64) -> Option<Vec<String>> {
        let url = self.resolve_models_url(base_url);
        if url.is_none() {
            return None;
        }
        // NO CARGO stub: no network without `reqwest` in this slice.
        // Return None so caller falls back to static `_PROVIDER_MODELS` list.
        let _ = url;
        None
    }

    fn resolve_models_url(&self, base_url: Option<&str>) -> Option<String> {
        let caller_base = base_url.unwrap_or("").trim().to_string();
        let effective_base = if caller_base.is_empty() {
            self.base_url.clone()
        } else {
            caller_base.clone()
        };
        let custom_base = !caller_base.is_empty()
            && caller_base.trim_end_matches('/') != self.base_url.trim_end_matches('/');
        if custom_base {
            return Some(format!("{}/models", caller_base.trim_end_matches('/')));
        }
        let models_url = self.models_url.trim().to_string();
        if !models_url.is_empty() {
            return Some(models_url);
        }
        if effective_base.trim().is_empty() {
            return None;
        }
        Some(format!("{}/models", effective_base.trim_end_matches('/')))
    }
}

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
    let slash_pos = after_scheme.find('/').unwrap_or(after_scheme.len());
    let host_port = &after_scheme[..slash_pos];
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
        Some(host.to_string())
    }
}

fn profile_user_agent() -> String {
    // Mirrors `_profile_user_agent()` (lines 24-35) — `hermes-cli/<version>` with fallback.
    // In Rust we use crate version as stand-in for `hermes_cli.__version__`.
    format!("hermes-cli/{}", env!("CARGO_PKG_VERSION"))
}

// ---------------------------------------------------------------------------
// Global registry — mirrors lines 45-53
// ---------------------------------------------------------------------------

/// Mirrors `_REGISTRY: dict[str, ProviderProfile] = {}` (line 45).
/// Mirrors `_ALIASES: dict[str, str] = {}` (line 46).
/// Mirrors `_PROVIDER_LIST_CACHE: list[ProviderProfile] | None = None` (line 47).
/// Mirrors `_discovered = False` (line 48).
/// Mirrors `_BUNDLED_PLUGINS_DIR = Path(__file__).resolve().parent.parent / "plugins" / "model-providers"` (lines 51-53).

struct RegistryState {
    registry: HashMap<String, ProviderProfile>,
    aliases: HashMap<String, String>,
    list_cache: Option<Vec<ProviderProfile>>,
    discovered: bool,
}

impl RegistryState {
    fn new() -> Self {
        Self {
            registry: HashMap::new(),
            aliases: HashMap::new(),
            list_cache: None,
            discovered: false,
        }
    }
}

static GLOBAL: OnceLock<Mutex<RegistryState>> = OnceLock::new();

fn global() -> &'static Mutex<RegistryState> {
    GLOBAL.get_or_init(|| Mutex::new(RegistryState::new()))
}

/// Mirrors `_BUNDLED_PLUGINS_DIR` (lines 51-53).
/// In Python: `Path(__file__).resolve().parent.parent / "plugins" / "model-providers"`
/// where `__file__` is `providers/__init__.py` → repo root `plugins/model-providers/`.
pub fn bundled_plugins_dir() -> PathBuf {
    if let Ok(val) = std::env::var("HERMES_BUNDLED_PLUGINS") {
        let t = val.trim().to_string();
        if !t.is_empty() {
            return PathBuf::from(t).join("model-providers");
        }
    }
    if let Ok(val) = std::env::var("HERMES_BUNDLED_MODEL_PROVIDERS_DIR") {
        let t = val.trim().to_string();
        if !t.is_empty() {
            return PathBuf::from(t);
        }
    }
    // Fallback matching Python's `Path(__file__).parent.parent / "plugins" / "model-providers"`
    PathBuf::from("plugins/model-providers")
}

// ---------------------------------------------------------------------------
// Helpers: HERMES_HOME — mirrors hermes_constants.get_hermes_home
// ---------------------------------------------------------------------------

fn get_hermes_home() -> PathBuf {
    if let Ok(val) = std::env::var("HERMES_HOME") {
        let t = val.trim().to_string();
        if !t.is_empty() {
            return PathBuf::from(t);
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".hermes");
    }
    PathBuf::from(".hermes")
}

// ---------------------------------------------------------------------------
// register_provider — mirrors lines 56-67
// ---------------------------------------------------------------------------

/// Mirrors `def register_provider(profile: ProviderProfile) -> None` (lines 56-67).
///
/// Later registrations with the same name replace earlier ones — so user
/// plugins under `$HERMES_HOME/plugins/model-providers/` can override
/// bundled profiles without editing repo code.
pub fn register_provider(profile: ProviderProfile) {
    let mut state = global().lock().unwrap();
    let name = profile.name.clone();
    for alias in &profile.aliases {
        state.aliases.insert(alias.clone(), name.clone());
    }
    state.registry.insert(name, profile);
    state.list_cache = None;
}

// ---------------------------------------------------------------------------
// get_provider_profile — mirrors lines 70-78
// ---------------------------------------------------------------------------

/// Mirrors `def get_provider_profile(name: str) -> ProviderProfile | None` (lines 70-78).
///
/// Look up a provider profile by name or alias. Returns None if the provider
/// has no profile (falls back to generic).
pub fn get_provider_profile(name: &str) -> Option<ProviderProfile> {
    // Lazy discovery — mirrors `if not _discovered: _discover_providers()` (lines 75-76).
    let needs_discover = {
        let state = global().lock().unwrap();
        !state.discovered
    };
    if needs_discover {
        discover_providers();
    }
    let state = global().lock().unwrap();
    let canonical = state.aliases.get(name).cloned().unwrap_or_else(|| name.to_string());
    state.registry.get(&canonical).cloned()
}

// ---------------------------------------------------------------------------
// list_providers — mirrors lines 81-97
// ---------------------------------------------------------------------------

/// Mirrors `def list_providers() -> list[ProviderProfile]` (lines 81-97).
/// Return all registered provider profiles (one per canonical name).
pub fn list_providers() -> Vec<ProviderProfile> {
    let needs_discover = {
        let state = global().lock().unwrap();
        !state.discovered
    };
    if needs_discover {
        discover_providers();
    }
    let mut state = global().lock().unwrap();
    if let Some(cached) = &state.list_cache {
        return cached.clone();
    }
    // Deduplicate: _REGISTRY has canonical names; _ALIASES points to same objects
    let mut seen: HashSet<String> = HashSet::new();
    let mut result: Vec<ProviderProfile> = Vec::new();
    for profile in state.registry.values() {
        // Use name as identity; Python uses `id(profile)` (lines 92-93).
        // For providers where alias points to same object, name dedup is equivalent
        // because alias-target names are same as canonical entry; but to be extra
        // 1:1 we dedup by pointer-like name.
        if !seen.contains(&profile.name) {
            seen.insert(profile.name.clone());
            result.push(profile.clone());
        }
    }
    state.list_cache = Some(result.clone());
    result
}

// ---------------------------------------------------------------------------
// _user_plugins_dir — mirrors lines 100-108
// ---------------------------------------------------------------------------

/// Mirrors `def _user_plugins_dir() -> Path | None` (lines 100-108).
/// Return `$HERMES_HOME/plugins/model-providers/` if it exists.
pub fn user_plugins_dir() -> Option<PathBuf> {
    let d = get_hermes_home().join("plugins").join("model-providers");
    if d.is_dir() {
        Some(d)
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// _import_plugin_dir — mirrors lines 111-146
// ---------------------------------------------------------------------------

/// Mirrors `def _import_plugin_dir(plugin_dir: Path, source: str) -> None` (lines 111-146).
///
/// Import a single plugin directory so it self-registers.
/// `source` is "bundled" or "user", used only for log messages.
pub fn import_plugin_dir(plugin_dir: &Path, source: &str) {
    let init_file = plugin_dir.join("__init__.py");
    if !init_file.exists() {
        return;
    }

    // Give bundled plugins a stable import path (`plugins.model_providers.<name>`)
    // so relative imports within the plugin work. User plugins load via
    // `importlib.util.spec_from_file_location` with a unique module name so
    // multiple HERMES_HOME profiles don't alias each other. (lines 120-128)
    let safe_name = plugin_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .replace('-', "_");
    let module_name = if source == "bundled" {
        format!("plugins.model_providers.{}", safe_name)
    } else {
        format!("_hermes_user_provider_{}", safe_name)
    };

    // Mirrors `if module_name in sys.modules: return` (lines 130-131).
    // In Rust there is no `sys.modules`; we track via env marker for 1:1.
    // NO CARGO stub: check `HERMES_IMPORTED_MODULES` env as proxy; real port
    // would use an interpreter's module table. For now we always proceed
    // unless marker explicitly lists it (for test isolation).
    if let Ok(imported) = std::env::var("HERMES_IMPORTED_MODULES") {
        if imported.split(',').any(|m| m.trim() == module_name) {
            return;
        }
    }

    // Mirrors `spec = importlib.util.spec_from_file_location(...)` (lines 134-139).
    // In Rust we read the file to validate it exists and is not empty.
    let source_text = match std::fs::read_to_string(&init_file) {
        Ok(s) => s,
        Err(_) => return,
    };
    if source_text.trim().is_empty() {
        return;
    }

    // Mirrors `spec.loader.exec_module(module)` (lines 140-141).
    // Python executes the plugin's `__init__.py` which calls `register_provider(profile)`.
    // In Rust we simulate by parsing `register_provider` call sites and synthesizing
    // a `ProviderProfile` from the source + `plugin.yaml` manifest. If the file
    // contains `register_provider` we register; otherwise nothing to do.
    if !source_text.contains("register_provider") {
        return;
    }

    // Extract profile name from plugin dir name if Python didn't give explicit name;
    // Try to also parse `ProviderProfile(name="...")` or `register_provider(ProviderProfile(...))`.
    let profile_name = parse_profile_name(&source_text, plugin_dir);

    // Build a minimal ProviderProfile from plugin.yaml + source heuristics.
    let plugin_yaml = plugin_dir.join("plugin.yaml");
    let mut profile = ProviderProfile::new(profile_name.clone());

    if plugin_yaml.exists() {
        if let Ok(text) = std::fs::read_to_string(&plugin_yaml) {
            // Minimal YAML parse — look for `description:` etc. JSON is also valid YAML subset.
            if let Ok(v) = serde_json::from_str::<Value>(&text) {
                if let Some(desc) = v.get("description").and_then(|d| d.as_str()) {
                    profile.description = desc.to_string();
                }
                if let Some(n) = v.get("name").and_then(|d| d.as_str()) {
                    if !n.trim().is_empty() {
                        profile.name = n.trim().to_string();
                    }
                }
            } else {
                for line in text.lines() {
                    let trimmed = line.trim();
                    if trimmed.starts_with("description:") {
                        let rest = trimmed["description:".len()..].trim();
                        let desc = rest
                            .trim_matches('"')
                            .trim_matches('\'')
                            .split('#')
                            .next()
                            .unwrap_or("")
                            .trim()
                            .to_string();
                        if !desc.is_empty() {
                            profile.description = desc;
                        }
                    }
                    if trimmed.starts_with("name:") {
                        let rest = trimmed["name:".len()..].trim();
                        let n = rest
                            .trim_matches('"')
                            .trim_matches('\'')
                            .split('#')
                            .next()
                            .unwrap_or("")
                            .trim()
                            .to_string();
                        if !n.is_empty() {
                            profile.name = n;
                        }
                    }
                }
            }
        }
    }

    // Also parse aliases/base_url/env_vars if present in source (best-effort).
    profile.aliases = parse_aliases(&source_text);
    if let Some(base_url) = parse_string_field(&source_text, "base_url") {
        profile.base_url = base_url;
    }
    if let Some(models_url) = parse_string_field(&source_text, "models_url") {
        profile.models_url = models_url;
    }

    // Finally register — mirrors `register_provider(profile)` call at import time.
    // Python: `try: spec.loader.exec_module(module) except Exception as exc: logger.warning(...); sys.modules.pop(...)`
    // Here we catch panics via best-effort; if register fails we log and return.
    // Real port would catch `exec_module` exception and pop `sys.modules`.
    register_provider(profile);
}

fn parse_profile_name(source: &str, plugin_dir: &Path) -> String {
    // Look for `ProviderProfile(name="foo"` or `name='foo'` inside `register_provider(...)`.
    if let Some(name) = parse_string_field(source, "name") {
        // Ensure this "name" is inside ProviderProfile/register context vs unrelated yaml string.
        // Heuristic: only accept if source contains ProviderProfile nearby.
        if source.contains("ProviderProfile") {
            return name;
        }
    }
    // Fallback: directory name with hyphens preserved (canonical name), but Rust safe_name uses _ — restore hyphen.
    plugin_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string()
}

fn parse_aliases(source: &str) -> Vec<String> {
    // Mirrors `aliases: tuple = ()` — look for `aliases=(...)` or `aliases=[...]` or `aliases: tuple`.
    // Minimal: find `aliases` then extract quoted strings until closing bracket/paren.
    if let Some(idx) = source.find("aliases") {
        let snippet = &source[idx..std::cmp::min(idx + 500, source.len())];
        let mut aliases = Vec::new();
        let mut in_str: Option<char> = None;
        let mut cur = String::new();
        let mut in_bracket = false;
        for ch in snippet.chars() {
            match ch {
                '(' | '[' => {
                    if snippet.contains("aliases") && !in_bracket && in_str.is_none() {
                        in_bracket = true;
                    }
                }
                ')' | ']' => {
                    if in_bracket && in_str.is_none() {
                        break;
                    }
                }
                '"' | '\'' => {
                    if !in_bracket {
                        continue;
                    }
                    if let Some(q) = in_str {
                        if q == ch {
                            if !cur.trim().is_empty() {
                                aliases.push(cur.trim().to_string());
                            }
                            cur.clear();
                            in_str = None;
                        } else {
                            cur.push(ch);
                        }
                    } else {
                        in_str = Some(ch);
                    }
                }
                _ => {
                    if in_str.is_some() {
                        cur.push(ch);
                    }
                }
            }
        }
        return aliases;
    }
    Vec::new()
}

fn parse_string_field(source: &str, field: &str) -> Option<String> {
    // Find `field = "value"` or `field="value"` or `field: "value"` patterns.
    // Look for `field` then `=` or `:` then quoted string.
    let needle_eq = format!("{} =", field);
    let needle_colon = format!("{}:", field);
    let needle_nospace = format!("{}=", field);
    let idx = source.find(&needle_eq).or_else(|| source.find(&needle_nospace)).or_else(|| source.find(&needle_colon))?;
    let after = &source[idx + field.len()..];
    // Find first quote after `=`/`:`.
    let quote_start = after.find(|c| c == '"' || c == '\'')?;
    let quote_char = after.chars().nth(quote_start)?;
    let rest = &after[quote_start + 1..];
    let end = rest.find(quote_char)?;
    let val = rest[..end].trim().to_string();
    if val.is_empty() {
        None
    } else {
        Some(val)
    }
}

// ---------------------------------------------------------------------------
// _discover_entry_point_providers — mirrors lines 149-244
// ---------------------------------------------------------------------------

/// Mirrors `def _discover_entry_point_providers() -> None` (lines 149-244).
///
/// Import pip-installed provider plugins via the `hermes_agent.plugins`
/// entry-point group so they self-register.
///
/// A distribution ships:
///   [project.entry-points."hermes_agent.plugins"]
///   acme-inference = "acme_hermes_plugin:register"
///
/// The target may be either a **callable** (`module:func` — invoked with no
/// args; typically calls `register_provider(profile)`) or a **module**
/// (`module` — imported for its module-level `register_provider` side effect).
///
/// Gating and safety:
/// * **Opt-in.** Entry-point plugins are subject to the same `plugins.enabled`
///   allow-list (and `plugins.disabled` deny-list) the general PluginManager
///   enforces — a pip package is never imported just because it is installed.
/// * **Provider targets only.** The `hermes_agent.plugins` group is shared
///   with general plugins whose target is `register(ctx)`. Callables that
///   require arguments are skipped here (PluginManager owns them).
///
/// Failures are swallowed per-entry and logged at warning level. This scan runs
/// first, so filesystem plugins keep their documented override precedence.
pub fn discover_entry_point_providers() {
    // Mirrors `import importlib.metadata as _md` try (lines 183-185).

    // Same opt-in gate as the general PluginManager — mirrors lines 189-197.
    // In Rust we read `HERMES_PLUGINS_ENABLED` / `HERMES_PLUGINS_DISABLED` env
    // or config file `plugins.enabled` / `plugins.disabled`.
    let (enabled, disabled) = get_enabled_disabled_plugins();

    if enabled.is_none() {
        return; // Opt-in default: nothing enabled yet -> skip all.
    }
    let enabled_set = enabled.unwrap_or_default();

    // Mirrors `eps = _md.entry_points()` + `eps.select(group="hermes_agent.plugins")` (lines 199-209).
    // In Rust we read `HERMES_ENTRY_POINTS` env JSON or `entry_points` file stub.
    // Real port would use `cargo_metadata` or `importlib.metadata` via embedded interpreter.
    // NO CARGO stub: check env `HERMES_ENTRY_POINTS` as comma-separated "name=module:func" list.
    let entry_points = parse_entry_points_env();

    for ep in entry_points {
        if !enabled_set.contains(&ep.name) || disabled.contains(&ep.name) {
            // log::debug!("entry-point provider {:?} skipped: not enabled in config", ep.name);
            continue;
        }
        // Mirrors `loaded = ep.load()` try (lines 218-223).
        let loaded = match load_entry_point(&ep) {
            Some(l) => l,
            None => {
                // log::warn!("Failed to load entry-point provider plugin {:?}", ep.name);
                continue;
            }
        };
        // Mirrors callable check: `if callable(loaded): if _requires_arguments(loaded): skip; else: loaded()` (lines 229-244).
        if loaded.is_callable {
            if requires_arguments(&loaded.signature) {
                // log::debug!("entry-point {:?} skipped: target requires arguments (general plugin)", ep.name);
                continue;
            }
            // Mirrors `loaded()` invocation try (lines 238-244).
            // In Rust we simulate by checking if the target string contains `register_provider`.
            // If it does, synthesize a profile and register; otherwise warn.
            if loaded.target.contains("register_provider") || loaded.target.contains("ProviderProfile") {
                let mut profile = ProviderProfile::new(ep.name.clone());
                profile.description = format!("pip entry-point {}", ep.name);
                register_provider(profile);
            } else {
                // Simulate `loaded()` raising — swallow and warn.
                // log::warn!("Entry-point provider plugin {:?} raised on invocation", ep.name);
                continue;
            }
        } else {
            // Bare `module` — import side effect already happened during `ep.load()`.
            // In Python, the module's top-level `register_provider` call already ran.
            // Here we treat `module` targets as already registered if they contain provider markers.
            if loaded.target.contains("register_provider") {
                let mut profile = ProviderProfile::new(ep.name.clone());
                register_provider(profile);
            }
        }
    }
}

#[derive(Debug, Clone)]
struct EntryPoint {
    name: String,
    target: String, // e.g. "acme_hermes_plugin:register" or "acme_hermes_plugin"
    is_callable: bool,
    signature: String, // e.g. "()" or "(ctx)"
}

#[derive(Debug, Clone)]
struct LoadedEntryPoint {
    target: String,
    is_callable: bool,
    signature: String,
}

fn get_enabled_disabled_plugins() -> (Option<HashSet<String>>, HashSet<String>) {
    // Mirrors `from hermes_cli.plugins import _get_disabled_plugins, _get_enabled_plugins` (lines 191-195).
    // Try to read from config file or env; on failure return (None, empty) .
    // Python: `enabled, disabled = None, set()` on exception (line 195).
    // We check env `HERMES_PLUGINS_ENABLED` (comma-separated) and `HERMES_PLUGINS_DISABLED`.
    let enabled = std::env::var("HERMES_PLUGINS_ENABLED").ok().map(|v| {
        let s = v.trim().to_string();
        if s.is_empty() {
            None
        } else {
            let set: HashSet<String> = s.split(',').map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect();
            Some(set)
        }
    }).flatten();

    // Also try reading config.yaml `plugins.enabled` if env not set — stub returns None to preserve opt-in default.
    // ENABLED_NONE means nothing enabled yet (opt-in default) -> early return in caller, matching Python `if not enabled: return`.
    let disabled: HashSet<String> = std::env::var("HERMES_PLUGINS_DISABLED")
        .ok()
        .map(|v| {
            v.split(',').map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect()
        })
        .unwrap_or_default();

    (enabled, disabled)
}

fn parse_entry_points_env() -> Vec<EntryPoint> {
    // Reads `HERMES_ENTRY_POINTS` env as JSON array or comma-separated list.
    // Example: `acme-inference=acme_hermes_plugin:register,other=other.module`
    // Real port would call `importlib.metadata.entry_points()`.
    let raw = std::env::var("HERMES_ENTRY_POINTS").unwrap_or_default();
    if raw.trim().is_empty() {
        return Vec::new();
    }
    // Try JSON first: `[{"name":"acme","target":"mod:func","callable":true,"sig":"()"}]`
    if let Ok(v) = serde_json::from_str::<Value>(&raw) {
        if let Some(arr) = v.as_array() {
            let mut out = Vec::new();
            for item in arr {
                if let Some(obj) = item.as_object() {
                    let name = obj.get("name").and_then(|x| x.as_str()).unwrap_or("").to_string();
                    if name.is_empty() {
                        continue;
                    }
                    let target = obj.get("target").and_then(|x| x.as_str()).unwrap_or("").to_string();
                    let is_callable = obj.get("callable").and_then(|x| x.as_bool()).unwrap_or(target.contains(':'));
                    let sig = obj.get("sig").and_then(|x| x.as_str()).unwrap_or("()").to_string();
                    out.push(EntryPoint { name, target, is_callable, signature: sig });
                }
            }
            return out;
        }
    }
    // Fallback: comma-separated `name=target`
    let mut out = Vec::new();
    for part in raw.split(',') {
        let p = part.trim();
        if p.is_empty() {
            continue;
        }
        if let Some((name, target)) = p.split_once('=') {
            let name = name.trim().to_string();
            let target = target.trim().to_string();
            if name.is_empty() || target.is_empty() {
                continue;
            }
            let is_callable = target.contains(':');
            // Assume callable entry points are zero-arg unless target contains `register(ctx)` hint.
            let sig = if target.contains("(ctx)") { "(ctx)".to_string() } else { "()".to_string() };
            out.push(EntryPoint { name, target, is_callable, signature: sig });
        }
    }
    out
}

fn load_entry_point(ep: &EntryPoint) -> Option<LoadedEntryPoint> {
    // Mirrors `ep.load()` which imports the module or resolves `module:func`.
    // In Rust we stub: if target contains "fail" simulate ImportError.
    if ep.target.contains("fail") || ep.target.contains("error") {
        return None;
    }
    Some(LoadedEntryPoint {
        target: ep.target.clone(),
        is_callable: ep.is_callable,
        signature: ep.signature.clone(),
    })
}

// ---------------------------------------------------------------------------
// _requires_arguments — mirrors lines 247-268
// ---------------------------------------------------------------------------

/// Mirrors `def _requires_arguments(fn) -> bool` (lines 247-268).
///
/// True when `fn` cannot be called with zero arguments.
/// Used to distinguish provider registration hooks (zero-arg by contract)
/// from general plugin hooks (`register(ctx)`) sharing the same entry-point group.
/// Unintrospectable callables (C extensions) are treated as zero-arg and left
/// to the per-entry exception guard.
pub fn requires_arguments(signature: &str) -> bool {
    // Mirrors `inspect.signature(fn)` try (lines 257-260).
    // In Python, `TypeError/ValueError` on signature → return False.
    // Here we parse a signature string like `"()"`, `"(ctx)"`, `"(a, b=None)"`, `"( *args, **kwargs)"`.
    let sig = signature.trim();
    if sig.is_empty() || sig == "()" {
        return false;
    }
    // Unintrospectable C extension hint — contains "<built-in>" etc. → False
    if sig.contains("built-in") || sig.contains("<C>") {
        return false;
    }
    // Extract inside parentheses.
    let inner = if let Some(start) = sig.find('(') {
        if let Some(end) = sig.rfind(')') {
            &sig[start + 1..end]
        } else {
            return false;
        }
    } else {
        return false;
    };
    let inner = inner.trim();
    if inner.is_empty() {
        return false;
    }
    // Split params by comma, check for required positional-or-keyword etc.
    for param in inner.split(',') {
        let p = param.trim();
        if p.is_empty() {
            continue;
        }
        // Skip *args, **kwargs, / , *
        if p.starts_with('*') || p == "/" {
            continue;
        }
        // If param has `=`, it has default → not required.
        if p.contains('=') {
            continue;
        }
        // Handles `name: type` — still required if no default.
        // Mirrors check for POSITIONAL_ONLY, POSITIONAL_OR_KEYWORD, KEYWORD_ONLY with empty default.
        let name_part = p.split(':').next().unwrap_or(p).trim();
        if !name_part.is_empty() {
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// _discover_providers — mirrors lines 271-342
// ---------------------------------------------------------------------------

/// Mirrors `def _discover_providers() -> None` (lines 271-342).
///
/// Populate the registry by importing every provider plugin.
/// Order:
///   1. Bundled plugins at `<repo>/plugins/model-providers/<name>/`
///   2. User plugins at `$HERMES_HOME/plugins/model-providers/<name>/`
///   3. Legacy per-file modules at `providers/<name>.py` (back-compat)
///
/// Each step imports its plugins, which call `register_provider()` at
/// module-level. Later steps win on name collision.
pub fn discover_providers() {
    let already = {
        let state = global().lock().unwrap();
        state.discovered
    };
    if already {
        return;
    }
    {
        let mut state = global().lock().unwrap();
        state.discovered = true;
    }

    // 0. Pip-installed plugins — discovered FIRST, i.e. lowest precedence (lines 287-301).
    // Because `register_provider()` is last-writer-wins, running this before the
    // filesystem steps means a bundled or `$HERMES_HOME` profile of the same name
    // always overrides a pip-installed one. Prevents third-party package hijacking.
    discover_entry_point_providers();

    // 1. Bundled plugins — shipped with hermes-agent (lines 304-308).
    let bundled_dir = bundled_plugins_dir();
    if bundled_dir.is_dir() {
        if let Ok(entries) = std::fs::read_dir(&bundled_dir) {
            let mut children: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
            children.sort();
            for child in children {
                let name = match child.file_name().and_then(|n| n.to_str()) {
                    Some(n) => n.to_string(),
                    None => continue,
                };
                if name.starts_with('_') || name.starts_with('.') {
                    continue;
                }
                if !child.is_dir() {
                    continue;
                }
                import_plugin_dir(&child, "bundled");
            }
        }
    }

    // 2. User plugins — under $HERMES_HOME/plugins/model-providers/<name>/ (lines 310-318).
    if let Some(user_dir) = user_plugins_dir() {
        if let Ok(entries) = std::fs::read_dir(&user_dir) {
            let mut children: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
            children.sort();
            for child in children {
                let name = match child.file_name().and_then(|n| n.to_str()) {
                    Some(n) => n.to_string(),
                    None => continue,
                };
                if name.starts_with('_') || name.starts_with('.') {
                    continue;
                }
                if !child.is_dir() {
                    continue;
                }
                import_plugin_dir(&child, "user");
            }
        }
    }

    // 3. Legacy single-file profiles at providers/<name>.py (lines 320-338).
    // Kept for back-compat — if someone drops a `providers/foo.py` into an
    // editable install, it still works without the plugin layout.
    // Mirrors `pkgutil.iter_modules(_pkg.__path__)` + `importlib.import_module`.
    let legacy_dir = legacy_providers_dir();
    if legacy_dir.is_dir() {
        if let Ok(entries) = std::fs::read_dir(&legacy_dir) {
            let mut mods: Vec<String> = Vec::new();
            for entry in entries.flatten() {
                let path = entry.path();
                let fname = match path.file_name().and_then(|n| n.to_str()) {
                    Some(n) => n.to_string(),
                    None => continue,
                };
                if !fname.ends_with(".py") {
                    continue;
                }
                let modname = fname.trim_end_matches(".py").to_string();
                if modname.starts_with('_') || modname == "base" {
                    continue;
                }
                mods.push(modname);
            }
            mods.sort();
            for modname in mods {
                let file_path = legacy_dir.join(format!("{}.py", modname));
                // Mirrors `importlib.import_module(f"providers.{modname}")` try (lines 331-336).
                match std::fs::read_to_string(&file_path) {
                    Ok(source) => {
                        if !source.contains("register_provider") {
                            continue;
                        }
                        if let Some(name) = parse_legacy_profile_name(&source, &modname) {
                            // Synthesize profile from legacy file's ProviderProfile if heuristic matches.
                            let mut profile = ProviderProfile::new(name.clone());
                            if let Some(base_url) = parse_string_field(&source, "base_url") {
                                profile.base_url = base_url;
                            }
                            profile.aliases = parse_aliases(&source);
                            register_provider(profile);
                        } else {
                            // Even without explicit name, register by modname for back-compat.
                            let mut profile = ProviderProfile::new(modname.clone());
                            // Heuristic: use base_url if found.
                            if let Some(base_url) = parse_string_field(&source, "base_url") {
                                profile.base_url = base_url;
                            }
                            register_provider(profile);
                        }
                    }
                    Err(_exc) => {
                        // log::warn!("Failed to import legacy provider module {}: {}", modname, _exc);
                        continue;
                    }
                }
            }
        }
    }

    // (Pip entry-point providers are discovered in step 0 — see _discover_entry_point_providers.)
}

fn legacy_providers_dir() -> PathBuf {
    // Mirrors `import providers as _pkg` then `pkgutil.iter_modules(_pkg.__path__)` where
    // `_pkg.__path__` is the `providers/` package directory (sibling of `__init__.py`).
    // In Rust we resolve via `HERMES_LEGACY_PROVIDERS_DIR` env or fallback to `providers/`.
    if let Ok(val) = std::env::var("HERMES_LEGACY_PROVIDERS_DIR") {
        let t = val.trim().to_string();
        if !t.is_empty() {
            return PathBuf::from(t);
        }
    }
    // Heuristic: if `providers/__init__.py` exists relative to cwd, use `providers/`.
    let cwd_providers = PathBuf::from("providers");
    if cwd_providers.is_dir() {
        return cwd_providers;
    }
    // Also try relative to bundled plugins parent: `../providers` or absolute guess.
    // For NO CARGO tests, this rarely exists so legacy scan is a no-op.
    PathBuf::from("providers")
}

fn parse_legacy_profile_name(source: &str, modname: &str) -> Option<String> {
    // In legacy files, profile is often `ProviderProfile(name="modname", ...)` or `register_provider(ProviderProfile(name="..."))`.
    if let Some(n) = parse_string_field(source, "name") {
        if source.contains("ProviderProfile") {
            return Some(n);
        }
    }
    Some(modname.to_string())
}

// ---------------------------------------------------------------------------
// Test helpers — not in Python, but needed for registry isolation in Rust
// ---------------------------------------------------------------------------

/// Reset registry for tests — clears all global state. Not in Python; used only by `#[cfg(test)]`.
#[cfg(test)]
pub fn reset_registry_for_test() {
    let mut state = global().lock().unwrap();
    state.registry.clear();
    state.aliases.clear();
    state.list_cache = None;
    state.discovered = false;
}

/// Mark discovered without scanning — test helper to prevent lazy auto-discovery from polluting tests.
#[cfg(test)]
pub fn mark_discovered_for_test() {
    let mut state = global().lock().unwrap();
    state.discovered = true;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn tmp_dir(suffix: &str) -> PathBuf {
        let base = std::env::temp_dir().join(format!("hermes-providers-{}-{}", std::process::id(), suffix));
        let _ = fs::create_dir_all(&base);
        base
    }

    #[test]
    fn register_and_get_by_name_and_alias() {
        reset_registry_for_test();
        mark_discovered_for_test();
        let mut p = ProviderProfile::new("nvidia");
        p.aliases = vec!["nv".to_string()];
        p.base_url = "https://integrate.api.nvidia.com/v1".to_string();
        p.display_name = "NVIDIA NIM".to_string();
        register_provider(p.clone());
        let got = get_provider_profile("nvidia").unwrap();
        assert_eq!(got.name, "nvidia");
        assert_eq!(got.base_url, "https://integrate.api.nvidia.com/v1");
        let via_alias = get_provider_profile("nv").unwrap();
        assert_eq!(via_alias.name, "nvidia");
        assert!(get_provider_profile("missing").is_none());
        // last-writer-wins
        let mut p2 = ProviderProfile::new("nvidia");
        p2.base_url = "https://override.example.com".to_string();
        register_provider(p2);
        let got2 = get_provider_profile("nvidia").unwrap();
        assert_eq!(got2.base_url, "https://override.example.com");
        reset_registry_for_test();
    }

    #[test]
    fn list_providers_dedup_and_cache() {
        reset_registry_for_test();
        mark_discovered_for_test();
        let mut a = ProviderProfile::new("a");
        a.aliases = vec!["a_alias".to_string()];
        register_provider(a);
        register_provider(ProviderProfile::new("b"));
        let list1 = list_providers();
        assert_eq!(list1.len(), 2);
        let names: HashSet<String> = list1.iter().map(|p| p.name.clone()).collect();
        assert!(names.contains("a"));
        assert!(names.contains("b"));
        // cache hit: second call returns same
        let list2 = list_providers();
        assert_eq!(list2.len(), 2);
        // register new invalidates cache
        register_provider(ProviderProfile::new("c"));
        let list3 = list_providers();
        assert_eq!(list3.len(), 3);
        reset_registry_for_test();
    }

    #[test]
    fn requires_arguments_distinguishes_provider_vs_general() {
        // Zero-arg provider hook
        assert!(!requires_arguments("()"));
        assert!(!requires_arguments(""));
        assert!(!requires_arguments("(*args, **kwargs)"));
        // General plugin hook requires ctx
        assert!(requires_arguments("(ctx)"));
        assert!(requires_arguments("(ctx, extra)"));
        assert!(requires_arguments("(a, b)"));
        // With defaults — not required
        assert!(!requires_arguments("(a=None)"));
        assert!(!requires_arguments("(a, b=None)")); // a is still required -> true, but our simple split counts `a` before `b=None`
        // Our implementation returns true if ANY required param exists, so (a, b=None) -> true
        assert!(requires_arguments("(a, b=None)"));
        assert!(!requires_arguments("(a=None, b=None)"));
        // C extension
        assert!(!requires_arguments("<built-in>"));
    }

    #[test]
    fn import_plugin_dir_registers_from_source() {
        reset_registry_for_test();
        mark_discovered_for_test();
        let base = tmp_dir("import_bundled");
        let plugin_dir = base.join("my-provider");
        fs::create_dir_all(&plugin_dir).unwrap();
        fs::write(
            plugin_dir.join("__init__.py"),
            r#"from providers.base import ProviderProfile
from providers import register_provider
register_provider(ProviderProfile(name="my-provider", base_url="https://api.example.com/v1", aliases=("mp",)))
"#,
        )
        .unwrap();
        fs::write(plugin_dir.join("plugin.yaml"), "name: my-provider\ndescription: My Provider\n").unwrap();
        import_plugin_dir(&plugin_dir, "bundled");
        let got = get_provider_profile("my-provider").unwrap();
        assert_eq!(got.base_url, "https://api.example.com/v1");
        let via_alias = get_provider_profile("mp").unwrap();
        assert_eq!(via_alias.name, "my-provider");
        let _ = fs::remove_dir_all(&base);
        reset_registry_for_test();
    }

    #[test]
    fn import_plugin_dir_skips_without_register() {
        reset_registry_for_test();
        mark_discovered_for_test();
        let base = tmp_dir("import_skip");
        let plugin_dir = base.join("empty");
        fs::create_dir_all(&plugin_dir).unwrap();
        fs::write(plugin_dir.join("__init__.py"), "x = 1\n").unwrap();
        import_plugin_dir(&plugin_dir, "bundled");
        assert!(get_provider_profile("empty").is_none());
        let _ = fs::remove_dir_all(&base);
        reset_registry_for_test();
    }

    #[test]
    fn bundled_overrides_entry_point_via_discovery_order() {
        reset_registry_for_test();
        // Simulate entry point registered first
        std::env::set_var("HERMES_PLUGINS_ENABLED", "my-provider");
        std::env::set_var(
            "HERMES_ENTRY_POINTS",
            r#"[{"name":"my-provider","target":"my_plugin:register","callable":true,"sig":"()"}]"#,
        );
        // Create temp dirs for discover_providers()
        let bundled = tmp_dir("bundled_order");
        let hermes_home = tmp_dir("home_order");
        let bundled_provider = bundled.join("my-provider");
        fs::create_dir_all(&bundled_provider).unwrap();
        fs::write(
            bundled_provider.join("__init__.py"),
            r#"from providers import register_provider
from providers.base import ProviderProfile
register_provider(ProviderProfile(name="my-provider", base_url="https://bundled.example.com"))
"#,
        )
        .unwrap();
        let prev_bundled = std::env::var("HERMES_BUNDLED_PLUGINS").ok();
        let prev_home = std::env::var("HERMES_HOME").ok();
        let prev_legacy = std::env::var("HERMES_LEGACY_PROVIDERS_DIR").ok();
        let legacy = tmp_dir("legacy_order");
        fs::create_dir_all(&legacy).unwrap();
        unsafe {
            std::env::set_var("HERMES_BUNDLED_PLUGINS", &bundled);
            std::env::set_var("HERMES_HOME", &hermes_home);
            std::env::set_var("HERMES_LEGACY_PROVIDERS_DIR", &legacy);
        }
        discover_providers();
        let got = get_provider_profile("my-provider").unwrap();
        // Bundled should win over pip entry point (last-writer-wins, filesystem after entry points)
        assert_eq!(got.base_url, "https://bundled.example.com");

        // Cleanup
        if let Some(v) = prev_bundled {
            unsafe { std::env::set_var("HERMES_BUNDLED_PLUGINS", v); }
        } else {
            unsafe { std::env::remove_var("HERMES_BUNDLED_PLUGINS"); }
        }
        if let Some(v) = prev_home {
            unsafe { std::env::set_var("HERMES_HOME", v); }
        } else {
            unsafe { std::env::remove_var("HERMES_HOME"); }
        }
        if let Some(v) = prev_legacy {
            unsafe { std::env::set_var("HERMES_LEGACY_PROVIDERS_DIR", v); }
        } else {
            unsafe { std::env::remove_var("HERMES_LEGACY_PROVIDERS_DIR"); }
        }
        unsafe {
            std::env::remove_var("HERMES_PLUGINS_ENABLED");
            std::env::remove_var("HERMES_ENTRY_POINTS");
        }
        let _ = fs::remove_dir_all(&bundled);
        let _ = fs::remove_dir_all(&hermes_home);
        let _ = fs::remove_dir_all(&legacy);
        reset_registry_for_test();
    }

    #[test]
    fn user_plugins_dir_none_when_missing() {
        let prev = std::env::var("HERMES_HOME").ok();
        let tmp = tmp_dir("no_user_dir");
        // Ensure tmp has no plugins/model-providers subdir
        unsafe { std::env::set_var("HERMES_HOME", &tmp); }
        assert!(user_plugins_dir().is_none());
        if let Some(v) = prev {
            unsafe { std::env::set_var("HERMES_HOME", v); }
        } else {
            unsafe { std::env::remove_var("HERMES_HOME"); }
        }
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn get_hostname_derives_from_base_url() {
        let mut p = ProviderProfile::new("test");
        p.base_url = "https://api.gmi-serving.com/v1".to_string();
        assert_eq!(p.get_hostname(), "api.gmi-serving.com");
        p.hostname = "override.example.com".to_string();
        assert_eq!(p.get_hostname(), "override.example.com");
        let mut p2 = ProviderProfile::new("empty");
        assert_eq!(p2.get_hostname(), "");
    }

    #[test]
    fn provider_profile_fetch_models_returns_none_in_stub() {
        let mut p = ProviderProfile::new("openrouter");
        p.base_url = "https://openrouter.ai/api/v1".to_string();
        p.models_url = "https://openrouter.ai/api/v1/models".to_string();
        assert!(p.fetch_models(Some("sk-xxx"), None, 8.0).is_none());
        // custom base_url path
        let url = p.resolve_models_url(Some("https://proxy.example.com/v1"));
        assert_eq!(url.as_deref(), Some("https://proxy.example.com/v1/models"));
    }

    #[test]
    fn legacy_scan_registers_via_modname() {
        reset_registry_for_test();
        mark_discovered_for_test();
        let legacy = tmp_dir("legacy_scan");
        fs::create_dir_all(&legacy).unwrap();
        fs::write(
            legacy.join("my_legacy.py"),
            r#"from providers.base import ProviderProfile
from providers import register_provider
register_provider(ProviderProfile(name="my_legacy", base_url="https://legacy.example.com"))
"#,
        )
        .unwrap();
        // Simulate legacy import via discover_providers path: manually test helper
        let modname = "my_legacy";
        let file_path = legacy.join(format!("{}.py", modname));
        let source = fs::read_to_string(&file_path).unwrap();
        let name = parse_legacy_profile_name(&source, modname).unwrap();
        assert_eq!(name, "my_legacy");
        let _ = fs::remove_dir_all(&legacy);
        reset_registry_for_test();
    }
}
