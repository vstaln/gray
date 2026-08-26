//! Hermes Constants — slice 3/3
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/hermes_constants.py`
//! slice 3/3 — lines 1200–1710 of 1710 (inclusive, 511 LOC).
//!
//! Covers tail of `_canonical_model_variants` header (line 1200) through EOF:
//!   remainder of `_canonical_model_variants` (1214–1274),
//!   `resolve_per_model_reasoning_effort` (1277–1309),
//!   `resolve_reasoning_config` (1312–1370),
//!   `is_termux` (1373–1381), `is_wsl` (1386–1401),
//!   `windows_path_to_wsl` (1404–1413), `wsl_unc_path_to_posix` (1416–1426),
//!   `translate_cwd_for_wsl_backend` (1429–1443),
//!   `is_container` (1449–1500),
//!   `get_config_path` / `get_skills_dir` / `get_env_path` (1506–1523),
//!   `apply_ipv4_preference` (1529–1568),
//!   streaming constants (1573–1583),
//!   `venv_bin_dir` / `project_venv_dir` / `venv_python_path` (1587–1635),
//!   `FIRST_PARTY_MODULE_ROOTS` / `is_first_party_module` / `partial_update_hint` (1645–1710).
//!
//! Slice boundaries:
//!   - Lines 1–599 → `hermes_constants_slice1.rs`
//!   - Lines 600–1200 → `hermes_constants_slice2.rs`
//!   - Lines 1200–1710 → this file.
//!
//! Overlap at line 1200 is intentional — slice2 ends with the `_canonical_model_variants`
//! docstring header truncated at line 1200 (`recovery to EACH…`); this slice repeats
//! that tail line for audit and then implements the 74-line body (1214–1274) plus
//! everything through EOF.
//!
//! T0001 — 1:1 port, no cargo (NEVER cargo).
//!
//! Notes on 1:1 fidelity vs. Rust idioms:
//! - `re.sub(r'(\d)-(\d)', r'\1.\2', s)` / `r'(\d)\.(\d)'` → hand-rolled scan, no `regex` crate.
//! - `dict | None` / `str | None` ↔ `Option<ReasoningConfig>` / `Option<String>`.
//! - `os.environ` → `std::env::var`; `Path` → `std::path::{Path,PathBuf}`.
//! - Global `None`-sentinel caches (`_wsl_detected`, `_container_detected`) → `OnceLock<Option<bool>>`.
//! - `socket.getaddrinfo` monkey-patch → stub guarded by `AtomicBool` (no socket import, no dep).
//! - `logging.warning` → `eprintln!("[hermes] WARNING: …")`.
//! - When three slices are merged into a single `hermes_constants` module, duplicate
//!   cross-slice helpers (`get_hermes_home`, `VALID_REASONING_EFFORTS`, `parse_reasoning_effort`, etc.)
//!   collapse to the single canonical defs in slice1/slice2.

use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::SystemTime;

// ---------------------------------------------------------------------------
// Cross-slice helpers (canonical in slice1/slice2; redeclared here so this
// slice compiles standalone and `grep` traces land. Merge step dedupes.)
// ---------------------------------------------------------------------------

fn dirs_home() -> PathBuf {
    if let Ok(h) = env::var("HOME") {
        let t = h.trim().to_string();
        if !t.is_empty() {
            return PathBuf::from(t);
        }
    }
    env::current_dir().unwrap_or_else(|_| PathBuf::from("/tmp"))
}

fn platform_default_hermes_home() -> PathBuf {
    if cfg!(windows) {
        if let Ok(v) = env::var("LOCALAPPDATA") {
            let t = v.trim().to_string();
            if !t.is_empty() {
                return PathBuf::from(t).join("hermes");
            }
        }
        dirs_home().join("AppData").join("Local").join("hermes")
    } else {
        dirs_home().join(".hermes")
    }
}

fn get_hermes_home_override() -> Option<String> {
    env::var("HERMES_HOME_OVERRIDE")
        .ok()
        .filter(|v| !v.trim().is_empty())
}

/// Mirrors `get_hermes_home() -> Path` (lines 114-139).
pub fn get_hermes_home() -> PathBuf {
    if let Some(o) = get_hermes_home_override() {
        if !o.trim().is_empty() {
            return PathBuf::from(o);
        }
    }
    if let Ok(v) = env::var("HERMES_HOME") {
        let t = v.trim().to_string();
        if !t.is_empty() {
            return PathBuf::from(t);
        }
    }
    platform_default_hermes_home()
}

// ---------------------------------------------------------------------------
// VALID_REASONING_EFFORTS + parse_reasoning_effort
// (canonical in slice2 lines 1155-1184; duplicated here for self-containment)
// ---------------------------------------------------------------------------

/// Mirrors `VALID_REASONING_EFFORTS = ("minimal","low",…)` (lines 1155-1157).
pub const VALID_REASONING_EFFORTS: &[&str] =
    &["minimal", "low", "medium", "high", "xhigh", "max", "ultra"];

/// Mirrors Python `{"enabled": bool, "effort": str | None}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReasoningConfig {
    pub enabled: bool,
    pub effort: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EffortArg {
    Bool(bool),
    Str(String),
    Null,
}

impl From<bool> for EffortArg {
    fn from(b: bool) -> Self {
        EffortArg::Bool(b)
    }
}
impl From<&str> for EffortArg {
    fn from(s: &str) -> Self {
        EffortArg::Str(s.to_string())
    }
}
impl From<String> for EffortArg {
    fn from(s: String) -> Self {
        EffortArg::Str(s)
    }
}
impl From<Option<String>> for EffortArg {
    fn from(o: Option<String>) -> Self {
        match o {
            Some(s) => EffortArg::Str(s),
            None => EffortArg::Null,
        }
    }
}

/// Mirrors `parse_reasoning_effort(effort)` (lines 1160-1184).
pub fn parse_reasoning_effort(arg: EffortArg) -> Option<ReasoningConfig> {
    match arg {
        EffortArg::Bool(false) => Some(ReasoningConfig { enabled: false, effort: None }),
        EffortArg::Bool(true) => None,
        EffortArg::Null => None,
        EffortArg::Str(s) => {
            if s.trim().is_empty() {
                return None;
            }
            let lower = s.trim().to_lowercase();
            if ["none", "false", "disabled"].contains(&lower.as_str()) {
                return Some(ReasoningConfig { enabled: false, effort: None });
            }
            if VALID_REASONING_EFFORTS.contains(&lower.as_str()) {
                return Some(ReasoningConfig { enabled: true, effort: Some(lower) });
            }
            None
        }
    }
}

pub fn parse_reasoning_effort_str(effort: Option<&str>) -> Option<ReasoningConfig> {
    match effort {
        None => None,
        Some(s) => parse_reasoning_effort(EffortArg::Str(s.to_string())),
    }
}

// ---------------------------------------------------------------------------
// ConfigValue — minimal dynamic value for 1:1 `dict | None` handling
// Mirrors Python's loose `cfg: dict | None` where values can be str|bool|dict.
// No serde dep — hand-rolled enum covers the shapes touched by
// `resolve_reasoning_config` (agent dict, model str/dict, reasoning_overrides,
// reasoning_effort str|bool).
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigValue {
    Null,
    Bool(bool),
    Str(String),
    Dict(HashMap<String, ConfigValue>),
}

impl From<&str> for ConfigValue {
    fn from(s: &str) -> Self {
        ConfigValue::Str(s.to_string())
    }
}
impl From<String> for ConfigValue {
    fn from(s: String) -> Self {
        ConfigValue::Str(s)
    }
}
impl From<bool> for ConfigValue {
    fn from(b: bool) -> Self {
        ConfigValue::Bool(b)
    }
}
impl ConfigValue {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            ConfigValue::Str(s) => Some(s.as_str()),
            _ => None,
        }
    }
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            ConfigValue::Bool(b) => Some(*b),
            _ => None,
        }
    }
    pub fn as_dict(&self) -> Option<&HashMap<String, ConfigValue>> {
        match self {
            ConfigValue::Dict(m) => Some(m),
            _ => None,
        }
    }
    /// Keep raw string for logging — mirrors Python `str(effort)`.
    pub fn display_string(&self) -> String {
        match self {
            ConfigValue::Null => String::new(),
            ConfigValue::Bool(b) => b.to_string(),
            ConfigValue::Str(s) => s.clone(),
            ConfigValue::Dict(_) => "[dict]".into(),
        }
    }
    /// Convert to EffortArg for parse_reasoning_effort — mirrors Python passing
    /// the raw `overrides[variant]` / `effort` straight through.
    pub fn to_effort_arg(&self) -> EffortArg {
        match self {
            ConfigValue::Null => EffortArg::Null,
            ConfigValue::Bool(b) => EffortArg::Bool(*b),
            ConfigValue::Str(s) => EffortArg::Str(s.clone()),
            ConfigValue::Dict(_) => EffortArg::Null, // dict never valid effort → None
        }
    }
}

// ---------------------------------------------------------------------------
// `_canonical_model_variants` — lines 1187-1274
// ---------------------------------------------------------------------------

/// Mirrors `_dash_to_dot = lambda s: re.sub(r'(\d)-(\d)', r'\1.\2', s)` (line 1217).
/// Non-overlapping, left-to-right, digit-dash-digit only.
fn dash_to_dot(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < chars.len() {
        if i + 2 < chars.len()
            && chars[i].is_ascii_digit()
            && chars[i + 1] == '-'
            && chars[i + 2].is_ascii_digit()
        {
            out.push(chars[i]);
            out.push('.');
            out.push(chars[i + 2]);
            i += 3;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

/// Mirrors `_dot_to_dash = lambda s: re.sub(r'(\d)\.(\d)', r'\1-\2', s)` (line 1218).
fn dot_to_dash(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < chars.len() {
        if i + 2 < chars.len()
            && chars[i].is_ascii_digit()
            && chars[i + 1] == '.'
            && chars[i + 2].is_ascii_digit()
        {
            out.push(chars[i]);
            out.push('-');
            out.push(chars[i + 2]);
            i += 3;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

/// Generate bounded spelling variants for tolerant override matching.
///
/// Mirrors `_canonical_model_variants(model: str) -> list[str]` (lines 1187-1274).
///
/// Python docstring (lines 1188-1213):
///   Model names mix two types of separators:
///   - Word separators: dashes between words (`claude-opus`)
///   - Version separators: dots or dashes between version digits (`4.5`, `4-5`)
///
///   The tricky case is that `.` appears in BOTH roles (word sep in some
///   spellings, version sep in others), so a blanket `.replace('.', '-')`
///   is lossy — it collapses version dots into dashes and no later step
///   recovers the canonical form (`claude-opus-4.5`).
///
///   Strategy: generate a small set of base forms, then apply version-dot
///   recovery to EACH of them. This ensures symmetry:
///   `claude-opus-4.5`, `claude-opus-4-5`, and `claude-opus.4.5` all
///   produce the same variant set.
///
///   Steps:
///   1. Exact input
///   2. Dots/dashes cross-substitution on the entire string
///   3. Version-dot recovery applied to ALL derivatives
///   4. Strip provider/aggregator prefix → bare model variants
///   5. Apply version-dot recovery to bare derivatives
///   6. Prepend known provider/aggregator prefixes
///
///   Duplicates removed in insertion order (exact always wins).
///
/// Lines 1214-1274 — import re, version-dot lambdas, Seen/variants, provider lists.
pub fn canonical_model_variants(model: &str) -> Vec<String> {
    // Lines 1214-1218 are already realized as dash_to_dot / dot_to_dash above.
    // Python's `import re` is not needed — we use hand-rolled scans.

    let mut seen: HashSet<String> = HashSet::new();
    let mut variants: Vec<String> = Vec::new();

    // Helper mirrors inner `_add(v)` (lines 1223-1226)
    let mut add = |v: String, seen: &mut HashSet<String>, variants: &mut Vec<String>| {
        if !v.is_empty() && !seen.contains(&v) {
            seen.insert(v.clone());
            variants.push(v);
        }
    };

    // Helper mirrors `_add_with_derivatives(s)` (lines 1228-1239)
    // Capture `add` inline to avoid borrow complexity — use closure that mutates seen/variants
    // via direct checks. For 1:1 we inline the body where called.

    // We need a function that given &str adds 7 variants:
    //  s, all_dashed, all_dotted, dash_to_dot(s), dot_to_dash(s), dash_to_dot(all_dashed), dot_to_dash(all_dotted)
    // Mirrors lines 1230-1239 exactly.
    let mut add_with_derivatives = |s: &str, seen: &mut HashSet<String>, variants: &mut Vec<String>| {
        let mut inner_add = |v: String| {
            if !v.is_empty() && !seen.contains(&v) {
                seen.insert(v.clone());
                variants.push(v);
            }
        };
        inner_add(s.to_string());
        let all_dashed = s.replace('.', "-");
        inner_add(all_dashed.clone());
        let all_dotted = s.replace('-', ".");
        inner_add(all_dotted.clone());
        inner_add(dash_to_dot(s));
        inner_add(dot_to_dash(s));
        inner_add(dash_to_dot(&all_dashed));
        inner_add(dot_to_dash(&all_dotted));
    };

    // Silence unused `add` closure (kept for audit showing _add shape)
    let _ = &mut add;

    // 1-3. Base variants for the full string (line 1242)
    add_with_derivatives(model, &mut seen, &mut variants);

    // Split by / to handle provider prefix (line 1245)
    let parts: Vec<&str> = model.split('/').collect();

    // 4. Bare model variants (strip provider/aggregator prefix) (lines 1248-1250)
    if parts.len() >= 2 {
        let bare = parts[parts.len() - 1];
        add_with_derivatives(bare, &mut seen, &mut variants);
    }

    // Strip aggregator only (3+ parts) (lines 1253-1255)
    // e.g. "openrouter/anthropic/claude-opus-4.5" → "anthropic/claude-opus-4.5"
    if parts.len() >= 3 {
        let joined = parts[1..].join("/");
        add_with_derivatives(&joined, &mut seen, &mut variants);
    }

    // 5. Prepend known provider prefixes to bare variants (lines 1257-1265)
    let known_providers = [
        "anthropic",
        "openai",
        "google",
        "openrouter",
        "groq",
        "mistral",
        "xai",
        "cohere",
        "perplexity",
        "together",
        "fireworks",
        "deepseek",
    ];
    // Snapshot bare variants before mutating (Python: `[v for v in variants if '/' not in v]`)
    let bare_variants: Vec<String> = variants.iter().filter(|v| !v.contains('/')).cloned().collect();
    for v in &bare_variants {
        for provider in &known_providers {
            let cand = format!("{provider}/{v}");
            if !cand.is_empty() && !seen.contains(&cand) {
                seen.insert(cand.clone());
                variants.push(cand);
            }
        }
    }

    // Prepend aggregator to single-slash variants (lines 1268-1272)
    let single_slash_variants: Vec<String> = variants.iter().filter(|v| v.matches('/').count() == 1).cloned().collect();
    let known_aggregators = ["openrouter", "opencode", "fireworks", "groq", "together"];
    for v in &single_slash_variants {
        for agg in &known_aggregators {
            let cand = format!("{agg}/{v}");
            if !cand.is_empty() && !seen.contains(&cand) {
                seen.insert(cand.clone());
                variants.push(cand);
            }
        }
    }

    variants
}

// ---------------------------------------------------------------------------
// `resolve_per_model_reasoning_effort(model: str, overrides: dict | None) -> dict | None`
// lines 1277-1309
// ---------------------------------------------------------------------------

/// Lookup a per-model reasoning_effort override with spelling-tolerance.
///
/// Mirrors lines 1277-1309.
///
/// Args:
///   model: any spelling — exact, normalized, bare, with provider prefix, etc.
///   overrides: dict of per-model overrides from `agent.reasoning_overrides` in config.yaml.
///
/// Returns parsed `ReasoningConfig` if a match is found, else `None`.
///
/// Resolution order (per docstring lines 1291-1298):
/// 1. Exact match  2. Dots↔dashes variants  3. Strip provider prefix
/// 4. Strip aggregator prefix  5. Prepend known aggregator prefixes to bare/single-slash variants
/// First non-None `parse_reasoning_effort` result wins.
pub fn resolve_per_model_reasoning_effort(
    model: &str,
    overrides: Option<&HashMap<String, ConfigValue>>,
) -> Option<ReasoningConfig> {
    // Lines 1300-1301: `if not overrides or not isinstance(overrides, dict) or not model: return None`
    let map = match overrides {
        Some(m) if !m.is_empty() => m,
        _ => return None,
    };
    if model.is_empty() {
        return None;
    }

    // Lines 1303-1307
    for variant in canonical_model_variants(model) {
        if let Some(val) = map.get(&variant) {
            let result = parse_reasoning_effort(val.to_effort_arg());
            if result.is_some() {
                return result;
            }
        }
    }

    // Line 1309
    None
}

// ---------------------------------------------------------------------------
// `resolve_reasoning_config(cfg: dict | None, model: str = "") -> dict | None`
// lines 1312-1370
// ---------------------------------------------------------------------------

/// Resolve the effective reasoning config for `model` from a config dict.
///
/// Mirrors lines 1312-1370. Single chokepoint for reasoning-effort resolution,
/// shared by every surface (CLI startup, messaging gateway, Desktop/TUI, cron,
/// `/model` switch, fallback activation). Priority:
/// 1. Per-model override from `agent.reasoning_overrides` (spelling-tolerant)
/// 2. Global `agent.reasoning_effort` — raw value is passed through so a YAML
///    boolean `False` (`reasoning_effort: false`/`off`/`no`) means "thinking disabled",
///    never silently re-enabled.
///
/// Session-scoped overrides (gateway `/reasoning --session`) are resolved by the
/// caller BEFORE this function — they always win.
pub fn resolve_reasoning_config(
    cfg: Option<&HashMap<String, ConfigValue>>,
    model: &str,
) -> Option<ReasoningConfig> {
    // Line 1339: `cfg = cfg if isinstance(cfg, dict) else {}`
    let cfg_map: HashMap<String, ConfigValue> = match cfg {
        Some(m) => m.clone(),
        None => HashMap::new(),
    };
    // Line 1340-1342
    let agent_cfg: HashMap<String, ConfigValue> = match cfg_map.get("agent") {
        Some(ConfigValue::Dict(d)) => d.clone(),
        _ => HashMap::new(),
    };

    // Lines 1344-1353: derive model from cfg's `model` section when empty
    let effective_model: String = if !model.is_empty() {
        model.trim().to_string()
    } else {
        match cfg_map.get("model") {
            Some(ConfigValue::Str(s)) => s.trim().to_string(),
            Some(ConfigValue::Dict(d)) => {
                // `model_cfg.get("default") or model_cfg.get("model") or ""`
                let v = d
                    .get("default")
                    .or_else(|| d.get("model"))
                    .map(|cv| cv.display_string())
                    .unwrap_or_default();
                v.trim().to_string()
            }
            _ => String::new(),
        }
    };

    // Lines 1355-1358
    let overrides_map: HashMap<String, ConfigValue> = match agent_cfg.get("reasoning_overrides") {
        Some(ConfigValue::Dict(d)) => d.clone(),
        Some(ConfigValue::Null) | None => HashMap::new(),
        _ => HashMap::new(), // non-dict overrides treated as empty (isinstance guard in callee)
    };
    let overrides_opt = if overrides_map.is_empty() {
        None
    } else {
        Some(&overrides_map)
    };
    let per_model = resolve_per_model_reasoning_effort(&effective_model, overrides_opt);
    if per_model.is_some() {
        return per_model;
    }

    // Lines 1360-1370: Global fallback — keep raw value; coercing with `or ""` turns
    // a YAML boolean False into "", silently re-enabling thinking.
    let effort_val: &ConfigValue = agent_cfg.get("reasoning_effort").unwrap_or(&ConfigValue::Null);
    // Need to distinguish "key absent" vs present — Python `agent_cfg.get("reasoning_effort", "")`
    // returns "" when absent; we treat absent as ConfigValue::Null → EffortArg::Null.
    let result = parse_reasoning_effort(effort_val.to_effort_arg());
    // `if effort and str(effort).strip() and result is None: log warning`
    let effort_present = !matches!(effort_val, ConfigValue::Null);
    let effort_str = effort_val.display_string();
    if effort_present && !effort_str.trim().is_empty() && result.is_none() {
        eprintln!(
            "[hermes] WARNING: Unknown reasoning_effort '{}', using default (medium)",
            effort_str
        );
    }
    result
}

// ---------------------------------------------------------------------------
// `is_termux() -> bool` — lines 1373-1381
// ---------------------------------------------------------------------------

/// Return True when running inside a Termux (Android) environment.
///
/// Mirrors lines 1373-1381. Checks `TERMUX_VERSION` (set by Termux) or the
/// Termux-specific `PREFIX` path. Import-safe — no heavy deps.
pub fn is_termux() -> bool {
    // Line 1379-1380: `prefix = os.getenv("PREFIX", "")` ; `bool(os.getenv("TERMUX_VERSION") or "com.termux/files/usr" in prefix)`
    let has_version = env::var("TERMUX_VERSION")
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false);
    if has_version {
        return true;
    }
    let prefix = env::var("PREFIX").unwrap_or_default();
    prefix.contains("com.termux/files/usr")
}

// ---------------------------------------------------------------------------
// `is_wsl() -> bool` — lines 1383-1401
// ---------------------------------------------------------------------------

static WSL_DETECTED: OnceLock<Option<bool>> = OnceLock::new();

/// Return True when running inside WSL (Windows Subsystem for Linux).
///
/// Mirrors lines 1386-1401. Checks `/proc/version` for the `microsoft` marker
/// that both WSL1 and WSL2 inject. Result is cached for the process lifetime.
pub fn is_wsl() -> bool {
    // Python: `global _wsl_detected; if _wsl_detected is not None: return _wsl_detected`
    // Rust: OnceLock<Option<bool>> — first call populates.
    if let Some(cached) = WSL_DETECTED.get() {
        return cached.unwrap_or(false);
    }
    let detected: Option<bool> = (|| {
        let content = fs::read_to_string("/proc/version").ok()?;
        Some(content.to_lowercase().contains("microsoft"))
    })();
    let val = detected.unwrap_or(false);
    let _ = WSL_DETECTED.set(Some(val));
    val
}

// For test isolation — reset helper not in Python, but useful for hermetic tests.
// Python mutates global `_wsl_detected = None` between tests.
#[cfg(test)]
fn reset_wsl_detected() {
    // OnceLock cannot be reset; tests that need isolation use an inner helper that takes content.
    // This is a no-op in production; test helper `is_wsl_from_content` covers logic.
}

/// Test-only helper that mirrors the `microsoft in f.read().lower()` check.
#[cfg(test)]
fn is_wsl_from_content(content: &str) -> bool {
    content.to_lowercase().contains("microsoft")
}

// ---------------------------------------------------------------------------
// `windows_path_to_wsl(path: str) -> str | None` — lines 1404-1413
// ---------------------------------------------------------------------------

/// Convert a Windows drive path (`C:\...`) to its `/mnt/<drive>/...` form.
///
/// Mirrors lines 1404-1413. `re.match(r"^([A-Za-z]):[\\/](.*)$", str(path or "").strip())`
pub fn windows_path_to_wsl(path: &str) -> Option<String> {
    // Python: `import re` inline + `str(path or "").strip()`
    let s = path.trim();
    if s.len() < 2 {
        return None;
    }
    let bytes = s.as_bytes();
    let drive = bytes[0] as char;
    if !drive.is_ascii_alphabetic() {
        return None;
    }
    if bytes[1] != b':' {
        return None;
    }
    // Need separator at index 2: either '/' or '\'
    if s.len() == 2 {
        return None; // no separator → no match (Python requires [\\/])
    }
    let sep = bytes[2] as char;
    if sep != '/' && sep != '\\' {
        return None;
    }
    let tail = &s[3..];
    let drive_lower = drive.to_ascii_lowercase();
    let tail_posix = tail.replace('\\', "/");
    Some(format!("/mnt/{}/{}", drive_lower, tail_posix))
}

// ---------------------------------------------------------------------------
// `wsl_unc_path_to_posix(path: str) -> str | None` — lines 1416-1426
// ---------------------------------------------------------------------------

/// Convert a Windows WSL UNC path (`\\wsl.localhost\<distro>\...` or the legacy
/// `\\wsl$\...`) to a POSIX path inside the distro.
///
/// Mirrors lines 1416-1426. Inline `import re`, normalize `"/"`→`"\\"`, then
/// `re.match(r"^\\\\wsl(?:\.localhost|\$)\\[^\\]+\\(.*)$", normalized, re.IGNORECASE)`
pub fn wsl_unc_path_to_posix(path: &str) -> Option<String> {
    let normalized = path.trim().replace('/', "\\");
    // Need to match `^\\\\wsl(\.localhost|\$)\\[^\\]+\\(.*)$` case-insensitive.
    // Manual parse: normalized must start with `\\wsl` (case-insensitive), then
    // either `.localhost` or `$`, then `\`, then distro (one or more non-\ chars), then `\`, then tail (any).
    let lower = normalized.to_lowercase();
    let prefix_wsl = r"\\wsl";
    if !lower.starts_with(prefix_wsl) {
        return None;
    }
    let rest_lower = &lower[prefix_wsl.len()..];
    let rest = &normalized[prefix_wsl.len()..]; // preserve original case for tail extraction
    // rest must start with `.localhost` or `$`
    let after_wsl: &str;
    if rest_lower.starts_with(".localhost") {
        after_wsl = &rest[".localhost".len()..];
        // Need the original suffix for tail extraction too — but we track indices via rest
        // Use lower for structure, original tail later.
        if after_wsl.is_empty() || !after_wsl.starts_with('\\') {
            return None;
        }
    } else if rest_lower.starts_with('$') {
        after_wsl = &rest["$".len()..];
        if after_wsl.is_empty() || !after_wsl.starts_with('\\') {
            return None;
        }
    } else {
        return None;
    }
    // after_wsl is like `\distro\rest` or `\distro` (no trailing)
    // Need `\\[^\\]+\\(.*)` — at least one distro segment plus `\` + tail
    // Strip leading `\`
    let after_slash = &after_wsl[1..];
    // Find next `\` separating distro from tail
    let distro_end = after_slash.find('\\');
    let tail_raw = match distro_end {
        Some(idx) => &after_slash[idx + 1..],
        None => return None, // need `\` after distro per `(.*)` but Python requires `\\(.*)` → must have tail separator
    };
    let tail = tail_raw.replace('\\', "/");
    if tail.is_empty() {
        Some("/".to_string())
    } else {
        Some(format!("/{}", tail))
    }
}

// ---------------------------------------------------------------------------
// `translate_cwd_for_wsl_backend(cwd: str) -> str` — lines 1429-1443
// ---------------------------------------------------------------------------

/// Normalize a cross-boundary cwd when Hermes itself runs inside WSL.
///
/// Mirrors lines 1429-1443. A Windows-host UI (native picker / drive path /
/// `\\wsl.localhost\` UNC) can hand the WSL backend a path it can't `chdir` into.
pub fn translate_cwd_for_wsl_backend(cwd: &str) -> String {
    // Line 1437: `if not is_wsl(): return cwd`
    if !is_wsl() {
        return cwd.to_string();
    }
    // Lines 1439-1442
    for translator in [wsl_unc_path_to_posix as fn(&str) -> Option<String>, windows_path_to_wsl] {
        if let Some(translated) = translator(cwd) {
            return translated;
        }
    }
    cwd.to_string()
}

// ---------------------------------------------------------------------------
// `is_container() -> bool` — lines 1446-1500
// ---------------------------------------------------------------------------

static CONTAINER_DETECTED: OnceLock<Option<bool>> = OnceLock::new();

/// Return True when running inside a container.
///
/// Mirrors lines 1449-1500. Recognizes Docker (`/.dockerenv`), Podman (`/run/.containerenv`),
/// and — via `/proc/1/cgroup` — the docker/podman/lxc cgroup-v1 markers.
/// cgroup v2 collapses `/proc/1/cgroup` to a single `0::/` line, so also checks
/// `KUBERNETES_SERVICE_HOST`, `kubepods`/`containerd`/`crio` in cgroup and mountinfo.
pub fn is_container() -> bool {
    if let Some(cached) = CONTAINER_DETECTED.get() {
        return cached.unwrap_or(false);
    }
    let result = is_container_inner();
    let _ = CONTAINER_DETECTED.set(Some(result));
    result
}

fn is_container_inner() -> bool {
    // Lines 1469-1474
    if Path::new("/.dockerenv").exists() {
        return true;
    }
    if Path::new("/run/.containerenv").exists() {
        return true;
    }
    // Line 1476
    if env::var("KUBERNETES_SERVICE_HOST")
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false)
    {
        return true;
    }
    const CGROUP_MARKERS: &[&str] = &["docker", "podman", "/lxc/", "kubepods", "containerd", "crio"];
    // Lines 1480-1487
    if let Ok(cgroup) = fs::read_to_string("/proc/1/cgroup") {
        if CGROUP_MARKERS.iter().any(|m| cgroup.contains(m)) {
            return true;
        }
    }
    // Lines 1491-1496: cgroup v2 fallback via mountinfo
    if let Ok(mountinfo) = fs::read_to_string("/proc/self/mountinfo") {
        if ["kubepods", "containerd", "crio"]
            .iter()
            .any(|m| mountinfo.contains(m))
        {
            return true;
        }
    }
    // Line 1499
    false
}

#[cfg(test)]
fn is_container_from_parts(
    dockerenv_exists: bool,
    containerenv_exists: bool,
    k8s_host: Option<&str>,
    cgroup: Option<&str>,
    mountinfo: Option<&str>,
) -> bool {
    if dockerenv_exists || containerenv_exists {
        return true;
    }
    if k8s_host.map(|v| !v.trim().is_empty()).unwrap_or(false) {
        return true;
    }
    const CGROUP_MARKERS: &[&str] = &["docker", "podman", "/lxc/", "kubepods", "containerd", "crio"];
    if let Some(cg) = cgroup {
        if CGROUP_MARKERS.iter().any(|m| cg.contains(m)) {
            return true;
        }
    }
    if let Some(mi) = mountinfo {
        if ["kubepods", "containerd", "crio"].iter().any(|m| mi.contains(m)) {
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Well-Known Paths — lines 1503-1523
// ---------------------------------------------------------------------------

/// Return the path to `config.yaml` under HERMES_HOME.
///
/// Mirrors lines 1506-1512. Replaces `get_hermes_home() / "config.yaml"` repeated
/// in 7+ files.
pub fn get_config_path() -> PathBuf {
    get_hermes_home().join("config.yaml")
}

/// Return the path to the skills directory under HERMES_HOME.
///
/// Mirrors lines 1515-1517.
pub fn get_skills_dir() -> PathBuf {
    get_hermes_home().join("skills")
}

/// Return the path to the `.env` file under HERMES_HOME.
///
/// Mirrors lines 1521-1523.
pub fn get_env_path() -> PathBuf {
    get_hermes_home().join(".env")
}

// ---------------------------------------------------------------------------
// Network Preferences — lines 1526-1568
// ---------------------------------------------------------------------------

static IPV4_PATCHED: AtomicBool = AtomicBool::new(false);

/// Monkey-patch `socket.getaddrinfo` to prefer IPv4 connections.
///
/// Mirrors lines 1529-1568. On servers with broken/unreachable IPv6, Python tries
/// AAAA records first and hangs for the full TCP timeout before falling back to IPv4.
///
/// When `force` is true, patches `getaddrinfo` so that calls with `family=AF_UNSPEC`
/// resolve as `AF_INET` instead, skipping IPv6 entirely. If no A record exists,
/// falls back to full resolution so pure-IPv6 hosts still work.
///
/// In Rust there is no `socket.getaddrinfo` to patch; this stub records the
/// preference via a process-global flag so callers can branch on `ipv4_preference_forced()`.
/// The docstring and guard semantics are preserved 1:1.
pub fn apply_ipv4_preference(force: bool) {
    // Line 1545-1546: if not force: return
    if !force {
        return;
    }
    // Line 1551: Guard against double-patching
    if IPV4_PATCHED.load(Ordering::SeqCst) {
        return;
    }
    // In Python, `_hermes_ipv4_patched` is set on the wrapper function and `socket.getaddrinfo`
    // is replaced. In Rust we just set the flag — actual DNS resolution is outside this
    // import-safe constants module. Network crates should query `ipv4_preference_forced()`.
    IPV4_PATCHED.store(true, Ordering::SeqCst);
}

/// Query whether `apply_ipv4_preference(true)` has been called (process-global).
pub fn ipv4_preference_forced() -> bool {
    IPV4_PATCHED.load(Ordering::SeqCst)
}

// ---------------------------------------------------------------------------
// Streaming Response Constants — lines 1571-1583
// ---------------------------------------------------------------------------

/// Response ID for partial stream stubs used during error recovery (line 1574).
pub const PARTIAL_STREAM_STUB_ID: &str = "partial-stream-stub";

/// Mirrors `FINISH_REASON_LENGTH = "length"` (line 1576).
pub const FINISH_REASON_LENGTH: &str = "length";

/// Mirrors `OPENROUTER_BASE_URL = "https://openrouter.ai/api/v1"` (line 1579).
pub const OPENROUTER_BASE_URL: &str = "https://openrouter.ai/api/v1";

/// Mirrors `OPENROUTER_MODELS_URL = f"{OPENROUTER_BASE_URL}/models"` (line 1580).
pub const OPENROUTER_MODELS_URL: &str = "https://openrouter.ai/api/v1/models";

/// Mirrors `AI_GATEWAY_BASE_URL = "https://ai-gateway.vercel.sh/v1"` (line 1582).
pub const AI_GATEWAY_BASE_URL: &str = "https://ai-gateway.vercel.sh/v1";

// ---------------------------------------------------------------------------
// Venv layout — lines 1585-1635
// ---------------------------------------------------------------------------

/// Directory holding a venv's executables (`Scripts` / `bin`).
///
/// Mirrors `venv_bin_dir(venv_dir, *, windows: bool | None = None) -> Path` (lines 1587-1610).
/// Canonical helper for venv layout. `windows` lets a caller pass its own platform
/// verdict for tests that patch `sys.platform` on Linux CI.
pub fn venv_bin_dir(venv_dir: impl AsRef<Path>, windows: Option<bool>) -> PathBuf {
    let is_windows = windows.unwrap_or_else(|| cfg!(windows));
    let base = PathBuf::from(venv_dir.as_ref());
    base.join(if is_windows { "Scripts" } else { "bin" })
}

/// The project's venv directory, `venv` or `.venv`, when one exists.
///
/// Mirrors `project_venv_dir(project_root) -> Path | None` (lines 1613-1626).
/// `uv venv` defaults to `.venv` while installers create `venv`; `venv` wins when both exist.
pub fn project_venv_dir(project_root: impl AsRef<Path>) -> Option<PathBuf> {
    let root = project_root.as_ref();
    for name in ["venv", ".venv"] {
        let candidate = root.join(name);
        if candidate.is_dir() {
            return Some(candidate);
        }
    }
    None
}

/// Path to the Python interpreter inside `venv_dir` (may not exist).
///
/// Mirrors `venv_python_path(venv_dir, *, windows: bool | None = None) -> Path` (lines 1629-1635).
pub fn venv_python_path(venv_dir: impl AsRef<Path>, windows: Option<bool>) -> PathBuf {
    let is_windows = windows.unwrap_or_else(|| cfg!(windows));
    venv_bin_dir(venv_dir, Some(is_windows)).join(if is_windows { "python.exe" } else { "python" })
}

// ---------------------------------------------------------------------------
// Partial-update diagnostics — lines 1638-1710
// ---------------------------------------------------------------------------

/// Top-level packages/modules that ship as part of Hermes itself.
///
/// Mirrors `FIRST_PARTY_MODULE_ROOTS = frozenset({...})` (lines 1645-1661).
/// Single source of truth — `hermes_cli.update_cmd`'s post-update probe consumes
/// this same set so the guard that BLOCKS and the hint that EXPLAINS can never disagree.
pub const FIRST_PARTY_MODULE_ROOTS: &[&str] = &[
    "agent",
    "acp_adapter",
    "cli",
    "cron",
    "gateway",
    "model_tools",
    "plugins",
    "providers",
    "tools",
    "toolsets",
    "run_agent",
    "tui_gateway",
    "utils",
];

/// True when `name` is a module that ships with Hermes.
///
/// Mirrors `is_first_party_module(name: str | None) -> bool` (lines 1664-1674).
/// Matches on the first dotted segment against an exact set — a substring or
/// `startswith` test would also claim third-party `agents`, `agentops`, etc.
pub fn is_first_party_module(name: Option<&str>) -> bool {
    let raw = match name {
        Some(s) if !s.is_empty() => s,
        _ => return false,
    };
    let root = raw.split('.').next().unwrap_or("");
    if root.is_empty() {
        return false;
    }
    if FIRST_PARTY_MODULE_ROOTS.contains(&root) {
        return true;
    }
    // `root.startswith("hermes_")` — line 1674
    root.starts_with("hermes_")
}

/// Return recovery guidance lines when `exc` looks like a half-updated tree.
///
/// Mirrors `partial_update_hint(exc: BaseException) -> list[str]` (lines 1677-1710).
/// An interrupted update can leave one package refreshed and a sibling stale;
/// every file still parses, but a sibling import dies with `ImportError: cannot
/// import name 'X' from 'y'`. Returns empty vec for unrelated exceptions so
/// callers can splat it unconditionally.
///
/// Python distinguishes `ImportError` vs `ModuleNotFoundError` (subclass of ImportError)
/// and checks `exc.name`. In Rust we model this as:
///   `kind`: "ImportError" | "ModuleNotFoundError" | other
///   `module_name`: `Option<&str>` carried by `ImportError.name`
/// Callers that have a real Python exception should map it; file-not-found style
/// errors map to `"ModuleNotFoundError"` and thus return empty.
pub fn partial_update_hint(kind: &str, module_name: Option<&str>) -> Vec<String> {
    // Line 1694: `if not isinstance(exc, ImportError): return []`
    if kind != "ImportError" {
        return vec![];
    }
    // Lines 1698-1699: ModuleNotFoundError is distinct — don't claim partial update.
    // Python: `if isinstance(exc, ModuleNotFoundError): return []`
    // But Rust callers pass kind explicitly; if they pass "ModuleNotFoundError" we return [].
    // This branch is actually unreachable when kind=="ImportError", but kept for audit:
    // callers that collapse both into "ImportError" need the module-name check below to filter
    // third-party misses. We handle the explicit ModuleNotFoundError string for completeness.
    if kind == "ModuleNotFoundError" {
        return vec![];
    }
    // Lines 1700-1702: `name = getattr(exc, "name", None); if not is_first_party_module(name): return []`
    if !is_first_party_module(module_name) {
        return vec![];
    }
    // Lines 1703-1710
    vec![
        String::new(),
        "This looks like a partially-updated install: one module was refreshed and a related one was not.".to_string(),
        "Re-run the update to bring the whole tree to the same version:".to_string(),
        "    hermes update".to_string(),
        "If that also fails, reinstall: https://hermes-agent.nousresearch.com".to_string(),
    ]
}

/// Convenience overload that mirrors the Python `isinstance(exc, ModuleNotFoundError)` distinct path.
/// When the caller knows the exception is a `ModuleNotFoundError`, this always returns empty.
pub fn partial_update_hint_module_not_found(_module_name: Option<&str>) -> Vec<String> {
    vec![]
}

// ---------------------------------------------------------------------------
// Self-check (ponytail: one runnable check, no framework)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_variants_symmetry() {
        let a = canonical_model_variants("claude-opus-4.5");
        let b = canonical_model_variants("claude-opus-4-5");
        let c = canonical_model_variants("claude-opus.4.5");
        // All three spellings must produce the same set (strategy guarantee line 1200-1202)
        let sa: HashSet<_> = a.iter().cloned().collect();
        let sb: HashSet<_> = b.iter().cloned().collect();
        let sc: HashSet<_> = c.iter().cloned().collect();
        assert_eq!(sa, sb);
        assert_eq!(sb, sc);
        // Exact input always first
        assert_eq!(a[0], "claude-opus-4.5");
        assert_eq!(b[0], "claude-opus-4-5");
        assert!(a.contains(&"claude-opus-4.5".to_string()));
        assert!(a.contains(&"claude-opus-4-5".to_string()));
        assert!(a.contains(&"4.5".to_string()) || a.contains(&"openai/claude-opus-4.5".to_string()));
    }

    #[test]
    fn dash_dot_helpers() {
        assert_eq!(dash_to_dot("4-5"), "4.5");
        assert_eq!(dot_to_dash("4.5"), "4-5");
        assert_eq!(dash_to_dot("claude-opus-4-5"), "claude-opus-4.5");
        assert_eq!(dot_to_dash("claude-opus-4.5"), "claude-opus-4-5");
        // Non-digit separators unchanged
        assert_eq!(dash_to_dot("a-b"), "a-b");
        assert_eq!(dot_to_dash("a.b"), "a.b");
        // Overlapping digit case: Python re.sub is non-overlapping
        assert_eq!(dash_to_dot("1-2-3"), "1.2-3");
        assert_eq!(dot_to_dash("1.2.3"), "1-2.3");
    }

    #[test]
    fn resolve_per_model_exact_and_variants() {
        let mut overrides = HashMap::new();
        overrides.insert("claude-opus-4.5".into(), ConfigValue::Str("high".into()));
        let got = resolve_per_model_reasoning_effort("claude-opus-4-5", Some(&overrides)).unwrap();
        assert_eq!(got.effort.as_deref(), Some("high"));

        // Bare model fallback
        let mut overrides2 = HashMap::new();
        overrides2.insert("claude-opus-4.5".into(), ConfigValue::Str("low".into()));
        let got2 = resolve_per_model_reasoning_effort("openai/claude-opus-4.5", Some(&overrides2)).unwrap();
        assert_eq!(got2.effort.as_deref(), Some("low"));
    }

    #[test]
    fn resolve_reasoning_config_global_fallback() {
        let mut agent = HashMap::new();
        agent.insert("reasoning_effort".into(), ConfigValue::Str("high".into()));
        let mut cfg = HashMap::new();
        cfg.insert("agent".into(), ConfigValue::Dict(agent));
        let got = resolve_reasoning_config(Some(&cfg), "any-model").unwrap();
        assert_eq!(got.effort.as_deref(), Some("high"));
        assert!(got.enabled);
    }

    #[test]
    fn resolve_reasoning_config_bool_false_preserved() {
        let mut agent = HashMap::new();
        agent.insert("reasoning_effort".into(), ConfigValue::Bool(false));
        let mut cfg = HashMap::new();
        cfg.insert("agent".into(), ConfigValue::Dict(agent));
        let got = resolve_reasoning_config(Some(&cfg), "").unwrap();
        assert!(!got.enabled);
        assert_eq!(got.effort, None);
    }

    #[test]
    fn windows_path_to_wsl_cases() {
        assert_eq!(windows_path_to_wsl(r"C:\Users\a"), Some("/mnt/c/Users/a".into()));
        assert_eq!(windows_path_to_wsl("D:/foo/bar"), Some("/mnt/d/foo/bar".into()));
        assert_eq!(windows_path_to_wsl("C:"), None);
        assert_eq!(windows_path_to_wsl(""), None);
        assert_eq!(windows_path_to_wsl("/usr/local"), None);
    }

    #[test]
    fn wsl_unc_cases() {
        assert_eq!(
            wsl_unc_path_to_posix(r"\\wsl.localhost\Ubuntu\home\user"),
            Some("/home/user".into())
        );
        assert_eq!(
            wsl_unc_path_to_posix(r"\\wsl$\Ubuntu\home\user"),
            Some("/home/user".into())
        );
        assert_eq!(
            wsl_unc_path_to_posix(r"\\wsl.localhost\Ubuntu\"),
            Some("/".into())
        );
        assert_eq!(wsl_unc_path_to_posix(r"C:\foo"), None);
        // forward slashes normalized
        assert_eq!(
            wsl_unc_path_to_posix("//wsl.localhost/Ubuntu/home/x"),
            Some("/home/x".into())
        );
    }

    #[test]
    fn well_known_paths() {
        let cfg = get_config_path();
        assert!(cfg.ends_with("config.yaml"));
        let skills = get_skills_dir();
        assert!(skills.ends_with("skills"));
        let env_path = get_env_path();
        assert!(env_path.ends_with(".env"));
    }

    #[test]
    fn venv_layout_helpers() {
        let bin = venv_bin_dir("/tmp/proj/venv", Some(false));
        assert_eq!(bin, PathBuf::from("/tmp/proj/venv/bin"));
        let bin_win = venv_bin_dir("/tmp/proj/venv", Some(true));
        assert_eq!(bin_win, PathBuf::from("/tmp/proj/venv/Scripts"));
        let py = venv_python_path("/tmp/proj/venv", Some(false));
        assert_eq!(py, PathBuf::from("/tmp/proj/venv/bin/python"));
        let py_win = venv_python_path("/tmp/proj/venv", Some(true));
        assert_eq!(py_win, PathBuf::from("/tmp/proj/venv/Scripts/python.exe"));
    }

    #[test]
    fn first_party_module() {
        assert!(is_first_party_module(Some("hermes_constants")));
        assert!(is_first_party_module(Some("agent.foo")));
        assert!(!is_first_party_module(Some("agents")));
        assert!(!is_first_party_module(Some("toolsets_x")));
        assert!(!is_first_party_module(None));
        assert!(!is_first_party_module(Some("")));
    }

    #[test]
    fn partial_update_hint_gate() {
        assert!(partial_update_hint("ImportError", Some("agent.foo")).len() > 0);
        assert!(partial_update_hint("ImportError", Some("requests")).is_empty());
        assert!(partial_update_hint("ValueError", Some("agent.foo")).is_empty());
        assert!(partial_update_hint_module_not_found(Some("agent.foo")).is_empty());
    }

    #[test]
    fn streaming_consts() {
        assert_eq!(PARTIAL_STREAM_STUB_ID, "partial-stream-stub");
        assert_eq!(FINISH_REASON_LENGTH, "length");
        assert_eq!(OPENROUTER_BASE_URL, "https://openrouter.ai/api/v1");
        assert_eq!(OPENROUTER_MODELS_URL, "https://openrouter.ai/api/v1/models");
    }

    #[test]
    fn ipv4_preference_idempotent() {
        // apply_ipv4_preference(false) is no-op
        apply_ipv4_preference(false);
        let before = ipv4_preference_forced();
        apply_ipv4_preference(true);
        assert!(ipv4_preference_forced());
        apply_ipv4_preference(true); // second call no-op (guard)
        assert!(ipv4_preference_forced());
        let _ = before;
    }
}
