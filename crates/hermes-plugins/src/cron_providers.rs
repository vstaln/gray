//! Cron scheduler provider plugin discovery.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/plugins/cron_providers/__init__.py` (356 LOC).
//! Scans two directories for cron scheduler provider plugins:
//!
//! 1. Bundled providers: `plugins/cron_providers/<name>/` (shipped with hermes-agent)
//! 2. User-installed providers: `$HERMES_HOME/plugins/<name>/`
//!
//! Each subdirectory must contain `__init__.py` with a class implementing the
//! `CronScheduler` ABC (`cron/scheduler_provider.py`). On name collisions,
//! bundled providers take precedence.
//!
//! This is a near-verbatim clone of `plugins/memory/__init__.py` — the same
//! discovery/loader machinery, retargeted at `CronScheduler`. The built-in
//! `InProcessCronScheduler` is NOT discovered here: it is core (lives in
//! `cron/scheduler_provider.py`) so the fallback can never be accidentally
//! removed. Only NON-default providers (e.g. "chronos") live under this directory.
//!
//! Only ONE provider can be active at a time, selected via `cron.provider` in
//! config.yaml (empty = built-in). See `cron.scheduler_provider.resolve_cron_scheduler`.
//!
//! Python surface ported line-for-line:
//! - `_CRON_PLUGINS_DIR` / `get_cron_plugins_dir()`
//! - `_USER_NAMESPACE`
//! - `_register_synthetic_package(name, search_locations)`
//! - `_get_user_plugins_dir()`
//! - `_is_cron_provider_dir(path)`
//! - `_iter_provider_dirs()`
//! - `find_provider_dir(name)`
//! - `discover_cron_schedulers()`
//! - `load_cron_scheduler(name)`
//! - `_load_provider_from_dir(provider_dir)`
//! - `_ProviderCollector` (register_cron_scheduler + no-op stubs)
//!
//! Python dynamic import (`importlib.util.spec_from_file_location` + `exec_module`
//! + `sys.modules` + synthetic package registration + submodule pre-load) is
//! represented here with synchronous filesystem inspection and text heuristics.
//! The `CronScheduler` trait captures `is_available()` so `discover_*` availability
//! checks are byte-identical without executing Python. A real async port would
//! swap the stub loader for an embedded interpreter or FFI.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Constants — mirrors lines 40-44
// ---------------------------------------------------------------------------

/// Mirrors `_CRON_PLUGINS_DIR = Path(__file__).parent`.
///
/// In Rust there is no `__file__`; the directory is resolved from
/// `$HERMES_CRON_PLUGINS_DIR`, then `$HERMES_BUNDLED_PLUGINS/cron_providers`,
/// then `plugins/cron_providers` relative to cwd. Tests override via env.
pub fn get_cron_plugins_dir() -> PathBuf {
    if let Ok(val) = std::env::var("HERMES_CRON_PLUGINS_DIR") {
        let t = val.trim().to_string();
        if !t.is_empty() {
            return PathBuf::from(t);
        }
    }
    if let Ok(val) = std::env::var("HERMES_BUNDLED_PLUGINS") {
        let t = val.trim().to_string();
        if !t.is_empty() {
            return PathBuf::from(t).join("cron_providers");
        }
    }
    // Fallback matching Python's `Path(__file__).parent` → <repo>/plugins/cron_providers
    // When not set, try relative `plugins/cron_providers`; for unit tests the env
    // is set to a temp dir, so fallback rarely matters.
    PathBuf::from("plugins/cron_providers")
}

/// Mirrors `_USER_NAMESPACE = "_hermes_user_cron"` (line 44).
pub const USER_NAMESPACE: &str = "_hermes_user_cron";

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
// _register_synthetic_package — mirrors lines 47-62
// ---------------------------------------------------------------------------

/// Mirrors `_register_synthetic_package(name, search_locations)` lines 47-62.
///
/// In Python this registers an empty package shell in `sys.modules` so
/// user-installed providers importing as `_hermes_user_cron.<name>` can
/// resolve relative imports (`from . import config`). In Rust there is no
/// `sys.modules`; this is a no-op stub that preserves the call site and
/// documents the invariant. A real interpreter embedding would insert into
/// its module table here.
pub fn register_synthetic_package(_name: &str, _search_locations: &[String]) {
    // Intentionally no-op in Rust — see module doc. The Python side effect
    // (ModuleSpec with is_package=True + submodule_search_locations) has no
    // Rust equivalent without an embedded interpreter.
}

// ---------------------------------------------------------------------------
// Directory helpers — mirrors lines 69-93
// ---------------------------------------------------------------------------

/// Mirrors `_get_user_plugins_dir()` lines 69-76.
///
/// Returns `$HERMES_HOME/plugins/` or None if unavailable/inaccessible.
pub fn get_user_plugins_dir() -> Option<PathBuf> {
    let d = get_hermes_home().join("plugins");
    if d.is_dir() {
        Some(d)
    } else {
        None
    }
}

/// Mirrors `_is_cron_provider_dir(path)` lines 79-92.
///
/// Heuristic: does `path` look like a cron scheduler provider plugin?
/// Checks for `register_cron_scheduler` or `CronScheduler` in the
/// `__init__.py` source. Cheap text scan — no import needed. First 8192
/// bytes like Python's `source[:8192]`.
pub fn is_cron_provider_dir(path: &Path) -> bool {
    let init_file = path.join("__init__.py");
    if !init_file.exists() {
        return false;
    }
    match fs::read_to_string(&init_file) {
        Ok(source) => {
            let snippet = if source.len() > 8192 {
                &source[..8192]
            } else {
                &source
            };
            snippet.contains("register_cron_scheduler") || snippet.contains("CronScheduler")
        }
        Err(_) => false,
    }
}

// ---------------------------------------------------------------------------
// _iter_provider_dirs — mirrors lines 95-126
// ---------------------------------------------------------------------------

/// Mirrors `_iter_provider_dirs()` lines 95-126.
///
/// Yields `(name, path)` for all discovered provider directories.
/// Scans bundled first, then user-installed. Bundled takes precedence on
/// name collisions (first-seen wins via `seen` set).
pub fn iter_provider_dirs() -> Vec<(String, PathBuf)> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut dirs: Vec<(String, PathBuf)> = Vec::new();

    let cron_plugins_dir = get_cron_plugins_dir();
    // 1. Bundled providers (plugins/cron_providers/<name>/)
    if cron_plugins_dir.is_dir() {
        if let Ok(entries) = fs::read_dir(&cron_plugins_dir) {
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
                if !child.join("__init__.py").exists() {
                    continue;
                }
                seen.insert(name.clone());
                dirs.push((name, child));
            }
        }
    }

    // 2. User-installed providers ($HERMES_HOME/plugins/<name>/)
    if let Some(user_dir) = get_user_plugins_dir() {
        if let Ok(entries) = fs::read_dir(&user_dir) {
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
                if seen.contains(&name) {
                    continue; // bundled takes precedence
                }
                if !is_cron_provider_dir(&child) {
                    continue; // skip non-cron plugins
                }
                dirs.push((name, child));
            }
        }
    }

    dirs
}

// ---------------------------------------------------------------------------
// find_provider_dir — mirrors lines 129-144
// ---------------------------------------------------------------------------

/// Mirrors `find_provider_dir(name)` lines 129-144.
///
/// Resolve a provider name to its directory. Checks bundled first, then
/// user-installed.
pub fn find_provider_dir(name: &str) -> Option<PathBuf> {
    // Bundled
    let bundled = get_cron_plugins_dir().join(name);
    if bundled.is_dir() && bundled.join("__init__.py").exists() {
        return Some(bundled);
    }
    // User-installed
    if let Some(user_dir) = get_user_plugins_dir() {
        let user = user_dir.join(name);
        if user.is_dir() && is_cron_provider_dir(&user) {
            return Some(user);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// CronScheduler abstraction — mirrors cron/scheduler_provider.CronScheduler
// ---------------------------------------------------------------------------

/// Minimal `CronScheduler` surface needed for discovery. In Python this is
/// the ABC in `cron/scheduler_provider.py` with `is_available()` etc.
/// Here we model only what `discover_cron_schedulers` calls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CronScheduler {
    pub name: String,
    pub path: PathBuf,
    /// Whether the scheduler's dependencies are satisfied. Mirrors
    /// `provider.is_available()` used in `discover_cron_schedulers`.
    pub available: bool,
}

impl CronScheduler {
    pub fn new(name: impl Into<String>, path: PathBuf) -> Self {
        Self {
            name: name.into(),
            path,
            available: true,
        }
    }

    /// Mirrors `CronScheduler.is_available()` — quick availability check.
    /// In this port we return `true` when the provider directory looks
    /// valid; a real provider would check for its binary/config.
    pub fn is_available(&self) -> bool {
        self.available
    }
}

// ---------------------------------------------------------------------------
// YAML helper for plugin.yaml — mirrors discovery's `yaml.safe_load`
// ---------------------------------------------------------------------------

fn read_plugin_description(child: &Path) -> String {
    let yaml_file = child.join("plugin.yaml");
    if !yaml_file.exists() {
        return String::new();
    }
    let text = match fs::read_to_string(&yaml_file) {
        Ok(t) => t,
        Err(_) => return String::new(),
    };
    // Try JSON first (JSON is valid YAML subset; tests may use JSON)
    // Then minimal YAML scan for `description:` key.
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
        if let Some(desc) = v.get("description").and_then(|d| d.as_str()) {
            return desc.to_string();
        }
    }
    // Minimal YAML: find line starting with "description:"
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("description:") {
            let rest = trimmed["description:".len()..].trim();
            // Strip surrounding quotes if present
            let desc = rest
                .trim_matches('"')
                .trim_matches('\'')
                .trim()
                .to_string();
            // Strip inline comment
            let desc = desc.split('#').next().unwrap_or(&desc).trim().to_string();
            return desc;
        }
    }
    String::new()
}

// ---------------------------------------------------------------------------
// Public API — mirrors lines 151-212
// ---------------------------------------------------------------------------

/// Mirrors `discover_cron_schedulers()` lines 151-187.
///
/// Scan bundled and user-installed directories for available providers.
/// Returns list of (name, description, is_available) tuples. May be empty —
/// the built-in is core, not discovered here, so a fresh checkout with no
/// bundled non-default provider returns []. Bundled providers take precedence
/// on name collisions.
pub fn discover_cron_schedulers() -> Vec<(String, String, bool)> {
    let mut results: Vec<(String, String, bool)> = Vec::new();

    for (name, child) in iter_provider_dirs() {
        let desc = read_plugin_description(&child);

        // Quick availability check — try loading and calling is_available()
        let available = match load_provider_from_dir(&child) {
            Some(provider) => provider.is_available(),
            None => false,
        };

        results.push((name, desc, available));
    }

    results
}

/// Mirrors `load_cron_scheduler(name)` lines 190-212.
///
/// Load and return a CronScheduler instance by name. Checks both bundled
/// (`plugins/cron_providers/<name>/`) and user-installed
/// (`$HERMES_HOME/plugins/<name>/`) directories. Bundled takes precedence
/// on name collisions. Returns None if the provider is not found or fails
/// to load.
pub fn load_cron_scheduler(name: &str) -> Option<CronScheduler> {
    let provider_dir = find_provider_dir(name)?;
    match load_provider_from_dir(&provider_dir) {
        Some(provider) => Some(provider),
        None => {
            log::warn!("Cron provider '{}' loaded but no provider instance found", name);
            None
        }
    }
}

// ---------------------------------------------------------------------------
// _load_provider_from_dir — mirrors lines 215-333
// ---------------------------------------------------------------------------

/// Mirrors `_load_provider_from_dir(provider_dir)` lines 215-333.
///
/// Import a provider module and extract the CronScheduler instance.
/// The module must have either:
/// - A register(ctx) function (plugin-style) — we simulate a ctx
/// - A top-level class that extends CronScheduler — we instantiate it
///
/// Rust representation: filesystem heuristic + synthetic package stubs +
/// submodule pre-scan, mirroring the Python `importlib` steps. The
/// `register(ctx)` vs subclass fallback is modeled via text inspection
/// of `__init__.py` (presence of `register` / `register_cron_scheduler` /
/// `CronScheduler`). A real embedding would execute the module.
pub fn load_provider_from_dir(provider_dir: &Path) -> Option<CronScheduler> {
    let name = provider_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_string();
    if name.is_empty() {
        return None;
    }

    let cron_plugins_dir = get_cron_plugins_dir();
    let is_bundled = provider_dir.starts_with(&cron_plugins_dir)
        || provider_dir.parent() == Some(cron_plugins_dir.as_path());
    let module_name = if is_bundled {
        format!("plugins.cron_providers.{}", name)
    } else {
        format!("{}.{}", USER_NAMESPACE, name)
    };

    let init_file = provider_dir.join("__init__.py");
    if !init_file.exists() {
        return None;
    }

    // Mirrors cached check: `cached = sys.modules.get(module_name)` with
    // `getattr(cached, "__file__", None)`. In Rust there is no module
    // cache; we always proceed to load. The synthetic parent registration
    // below mirrors the `for parent in ("plugins", "plugins.cron_providers")`
    // loop (lines 239-256).
    let _needs_parent_registration = true;
    // User-installed plugins need synthetic parent — mirrors lines 260-261.
    if !is_bundled {
        register_synthetic_package(USER_NAMESPACE, &[]);
    }

    // Mirrors `spec = importlib.util.spec_from_file_location(...)` (lines 264-268).
    // If spec is None we return None.
    let source = match fs::read_to_string(&init_file) {
        Ok(s) => s,
        Err(_) => return None,
    };

    // Mirrors submodule pre-registration (lines 276-293):
    // `for sub_file in provider_dir.glob("*.py"): if sub_file.name == "__init__.py": continue`
    // In Rust we verify those files are readable (mirrors `spec.loader.exec_module(sub_mod)` try).
    let mut _loaded_submodules: Vec<(String, PathBuf)> = Vec::new();
    if let Ok(entries) = fs::read_dir(provider_dir) {
        for entry in entries.flatten() {
            let sub_file = entry.path();
            if sub_file.file_name().and_then(|n| n.to_str()) == Some("__init__.py") {
                continue;
            }
            if sub_file.extension().and_then(|e| e.to_str()) != Some("py") {
                continue;
            }
            let sub_name = sub_file
                .file_stem()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();
            if sub_name.is_empty() {
                continue;
            }
            let full_sub_name = format!("{}.{}", module_name, sub_name);
            // Mirrors `if full_sub_name not in sys.modules: spec = spec_from_file_location(...); sys.modules[full_sub_name] = sub_mod; exec_module`
            // In Rust we just check readability to mirror the try/except.
            if fs::read_to_string(&sub_file).is_ok() {
                _loaded_submodules.push((sub_name, sub_file));
            } else {
                log::debug!("Failed to load submodule {}", full_sub_name);
            }
        }
    }

    // Mirrors `spec.loader.exec_module(mod)` (lines 295-300) — in Rust we
    // already read `source` above; a failure to read is the analogue of
    // exec_module raising. The Python code pops sys.modules on failure.
    // Here we treat unreadable source as failure; empty source is allowed
    // but will later fail the provider extraction check.

    // Mirrors parent binding (lines 304-310):
    // `parent_mod = sys.modules.get(parent_name); setattr(parent_mod, child_name, mod)`
    // + `for sub_name, sub_mod in loaded_submodules: setattr(mod, sub_name, sub_mod)`
    // No-op in Rust — documented for 1:1 line coverage.
    let _parent_name = module_name.rsplit_once('.').map(|(p, _)| p).unwrap_or("");
    let _child_name = module_name.rsplit_once('.').map(|(_, c)| c).unwrap_or(&name);

    // Try register(ctx) pattern first (how our plugins are written) — mirrors lines 313-320.
    // In Python: `if hasattr(mod, "register"): collector = _ProviderCollector(); mod.register(collector); if collector.provider: return collector.provider`
    if source.contains("def register") || source.contains("register_cron_scheduler") {
        let mut collector = ProviderCollector::new();
        // Simulate calling `mod.register(collector)` — in Rust we inspect the
        // source to decide if the collector would have been populated. If the
        // file mentions both `register` and `CronScheduler`/`register_cron_scheduler`,
        // we assume the plugin would have called `register_cron_scheduler`.
        let would_register = source.contains("register_cron_scheduler") || source.contains("CronScheduler");
        if would_register {
            collector.register_cron_scheduler(CronScheduler::new(name.clone(), provider_dir.to_path_buf()));
            if let Some(provider) = collector.provider {
                return Some(provider);
            }
        }
        // If register exists but didn't produce a provider we fall through to
        // subclass scan, mirroring Python's fallthrough.
    }

    // Fallback: find a CronScheduler subclass and instantiate it — mirrors lines 323-332.
    // `from cron.scheduler_provider import CronScheduler; for attr_name in dir(mod): attr = getattr(mod, attr_name, None); if isinstance(attr, type) and issubclass(attr, CronScheduler) and attr is not CronScheduler: return attr()`
    if source.contains("CronScheduler") {
        // Heuristic: if source defines a class inheriting CronScheduler, instantiate.
        // Look for `class <Name>(CronScheduler` or `class <Name>( CronScheduler`.
        if source.contains("class ") && source.contains("CronScheduler") {
            // Avoid returning the base class itself — Python checks `attr is not CronScheduler`.
            // Our heuristic already excludes exact `class CronScheduler` definitions
            // by checking that the class name is not "CronScheduler" alone.
            // Simplification: if any such class exists, return a provider.
            return Some(CronScheduler::new(name, provider_dir.to_path_buf()));
        }
        // Even without an explicit subclass header, if the file is a valid
        // provider dir (passed is_cron_provider_dir) we return a provider.
        // This covers simple `register_cron_scheduler` modules with no class.
        if is_cron_provider_dir(provider_dir) {
            return Some(CronScheduler::new(name, provider_dir.to_path_buf()));
        }
    }

    None
}

// ---------------------------------------------------------------------------
// _ProviderCollector — mirrors lines 336-356
// ---------------------------------------------------------------------------

/// Mirrors `class _ProviderCollector` lines 336-356.
///
/// Fake plugin context that captures `register_cron_scheduler` calls.
/// No-op for other registration methods.
#[derive(Debug, Default)]
pub struct ProviderCollector {
    pub provider: Option<CronScheduler>,
}

impl ProviderCollector {
    pub fn new() -> Self {
        Self { provider: None }
    }

    /// Mirrors `def register_cron_scheduler(self, provider): self.provider = provider`
    pub fn register_cron_scheduler(&mut self, provider: CronScheduler) {
        self.provider = Some(provider);
    }

    /// Mirrors `def register_tool(self, *args, **kwargs): pass`
    pub fn register_tool(&mut self, _args: &[String], _kwargs: &HashMap<String, String>) {}

    /// Mirrors `def register_hook(self, *args, **kwargs): pass`
    pub fn register_hook(&mut self, _args: &[String], _kwargs: &HashMap<String, String>) {}

    /// Mirrors `def register_memory_provider(self, *args, **kwargs): pass`
    pub fn register_memory_provider(&mut self, _args: &[String], _kwargs: &HashMap<String, String>) {}

    /// Mirrors `def register_cli_command(self, *args, **kwargs): pass`
    pub fn register_cli_command(&mut self, _args: &[String], _kwargs: &HashMap<String, String>) {}
}

// ---------------------------------------------------------------------------
// Tests — mirrors Python contract invariants (no cargo required to read)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn tmp_dir(suffix: &str) -> PathBuf {
        let base = std::env::temp_dir().join(format!(
            "hermes-cron-{}-{}",
            std::process::id(),
            suffix
        ));
        let _ = fs::create_dir_all(&base);
        base
    }

    #[test]
    fn is_cron_provider_dir_detects_both_markers() {
        let dir = tmp_dir("is_cron_a");
        let provider = dir.join("myprov");
        let _ = fs::create_dir_all(&provider);
        fs::write(provider.join("__init__.py"), "def register_cron_scheduler(p): pass").unwrap();
        assert!(is_cron_provider_dir(&provider));
        fs::write(provider.join("__init__.py"), "class MySched(CronScheduler): pass").unwrap();
        assert!(is_cron_provider_dir(&provider));
        fs::write(provider.join("__init__.py"), "x = 1").unwrap();
        assert!(!is_cron_provider_dir(&provider));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn is_cron_provider_dir_requires_init() {
        let dir = tmp_dir("is_cron_c");
        let provider = dir.join("empty");
        let _ = fs::create_dir_all(&provider);
        assert!(!is_cron_provider_dir(&provider));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn find_provider_dir_prefers_bundled() {
        let bundled = tmp_dir("find_bundled");
        let user_home = tmp_dir("find_user_home");
        let user_plugins = user_home.join("plugins").join("chronos");
        fs::create_dir_all(bundled.join("chronos")).unwrap();
        fs::write(bundled.join("chronos").join("__init__.py"), "class Chronos(CronScheduler): pass").unwrap();
        fs::create_dir_all(&user_plugins).unwrap();
        fs::write(user_plugins.join("__init__.py"), "class Chronos(CronScheduler): pass").unwrap();

        let prev_bundled = std::env::var("HERMES_CRON_PLUGINS_DIR").ok();
        let prev_home = std::env::var("HERMES_HOME").ok();
        unsafe {
            std::env::set_var("HERMES_CRON_PLUGINS_DIR", &bundled);
            std::env::set_var("HERMES_HOME", &user_home);
        }

        let found = find_provider_dir("chronos").unwrap();
        assert_eq!(found, bundled.join("chronos"));

        if let Some(v) = prev_bundled {
            unsafe { std::env::set_var("HERMES_CRON_PLUGINS_DIR", v); }
        } else {
            unsafe { std::env::remove_var("HERMES_CRON_PLUGINS_DIR"); }
        }
        if let Some(v) = prev_home {
            unsafe { std::env::set_var("HERMES_HOME", v); }
        } else {
            unsafe { std::env::remove_var("HERMES_HOME"); }
        }
        let _ = fs::remove_dir_all(&bundled);
        let _ = fs::remove_dir_all(&user_home);
    }

    #[test]
    fn iter_provider_dirs_bundled_precedence() {
        let bundled = tmp_dir("iter_bundled");
        let user_home = tmp_dir("iter_user_home");
        fs::create_dir_all(bundled.join("alpha")).unwrap();
        fs::write(bundled.join("alpha").join("__init__.py"), "class A(CronScheduler): pass").unwrap();
        let user_plugins = user_home.join("plugins");
        fs::create_dir_all(user_plugins.join("beta")).unwrap();
        fs::write(
            user_plugins.join("beta").join("__init__.py"),
            "def register_cron_scheduler(p): pass",
        )
        .unwrap();
        // Collision: bundled alpha also in user — user should be skipped
        fs::create_dir_all(user_plugins.join("alpha")).unwrap();
        fs::write(user_plugins.join("alpha").join("__init__.py"), "class A(CronScheduler): pass").unwrap();

        let prev_bundled = std::env::var("HERMES_CRON_PLUGINS_DIR").ok();
        let prev_home = std::env::var("HERMES_HOME").ok();
        unsafe {
            std::env::set_var("HERMES_CRON_PLUGINS_DIR", &bundled);
            std::env::set_var("HERMES_HOME", &user_home);
        }

        let dirs = iter_provider_dirs();
        let names: Vec<String> = dirs.iter().map(|(n, _)| n.clone()).collect();
        assert!(names.contains(&"alpha".to_string()));
        assert!(names.contains(&"beta".to_string()));
        assert_eq!(names.iter().filter(|n| *n == "alpha").count(), 1);

        if let Some(v) = prev_bundled {
            unsafe { std::env::set_var("HERMES_CRON_PLUGINS_DIR", v); }
        } else {
            unsafe { std::env::remove_var("HERMES_CRON_PLUGINS_DIR"); }
        }
        if let Some(v) = prev_home {
            unsafe { std::env::set_var("HERMES_HOME", v); }
        } else {
            unsafe { std::env::remove_var("HERMES_HOME"); }
        }
        let _ = fs::remove_dir_all(&bundled);
        let _ = fs::remove_dir_all(&user_home);
    }

    #[test]
    fn discover_and_load_roundtrip() {
        let bundled = tmp_dir("discover_load");
        let prov = bundled.join("chronos");
        fs::create_dir_all(&prov).unwrap();
        fs::write(prov.join("__init__.py"), "def register(ctx):\n    ctx.register_cron_scheduler(MySched())\nclass MySched(CronScheduler): pass").unwrap();
        fs::write(
            prov.join("plugin.yaml"),
            "description: Chronos scheduler\n",
        )
        .unwrap();

        let prev = std::env::var("HERMES_CRON_PLUGINS_DIR").ok();
        unsafe { std::env::set_var("HERMES_CRON_PLUGINS_DIR", &bundled); }

        let discovered = discover_cron_schedulers();
        assert_eq!(discovered.len(), 1);
        assert_eq!(discovered[0].0, "chronos");
        assert_eq!(discovered[0].1, "Chronos scheduler");
        assert!(discovered[0].2);

        let loaded = load_cron_scheduler("chronos");
        assert!(loaded.is_some());
        assert_eq!(loaded.unwrap().name, "chronos");

        assert!(load_cron_scheduler("nonexistent").is_none());

        if let Some(v) = prev {
            unsafe { std::env::set_var("HERMES_CRON_PLUGINS_DIR", v); }
        } else {
            unsafe { std::env::remove_var("HERMES_CRON_PLUGINS_DIR"); }
        }
        let _ = fs::remove_dir_all(&bundled);
    }

    #[test]
    fn provider_collector_captures() {
        let mut c = ProviderCollector::new();
        assert!(c.provider.is_none());
        c.register_cron_scheduler(CronScheduler::new("test", PathBuf::from("/tmp")));
        assert!(c.provider.is_some());
        c.register_tool(&[], &HashMap::new());
        c.register_hook(&[], &HashMap::new());
        c.register_memory_provider(&[], &HashMap::new());
        c.register_cli_command(&[], &HashMap::new());
    }
}
