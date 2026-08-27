//! Configurable budget constants for tool result persistence.
//! Port of `tools/budget_config.py` (174 lines) — 1:1 behavior.
//!
//! Per-tool resolution: pinned > config overrides > registry > default.
//!
//! Python mapping
//! --------------
//! - `PINNED_THRESHOLDS` → [`PINNED_THRESHOLDS`] / [`is_pinned`] / [`Threshold::Infinite`]
//! - `DEFAULT_RESULT_SIZE_CHARS` → [`DEFAULT_RESULT_SIZE_CHARS`] (15)
//! - `DEFAULT_TURN_BUDGET_CHARS` → [`DEFAULT_TURN_BUDGET_CHARS`] (16)
//! - `DEFAULT_PREVIEW_SIZE_CHARS` → [`DEFAULT_PREVIEW_SIZE_CHARS`] (17)
//! - `DEFAULT_MCP_RESULT_SIZE_CHARS` → [`DEFAULT_MCP_RESULT_SIZE_CHARS`] (32)
//! - `MCP_TOOL_PREFIX` → [`MCP_TOOL_PREFIX`] (36)
//! - `_configured_mcp_result_size()` → [`configured_mcp_result_size()`] / [`configured_mcp_result_size_from_str`] (39-63)
//! - `BudgetConfig` → [`BudgetConfig`] (67-112) + [`Threshold`]
//! - `DEFAULT_BUDGET` → [`default_budget()`] / [`DEFAULT_BUDGET_CELL`] (116)
//! - `_CHARS_PER_TOKEN` → [`CHARS_PER_TOKEN`] (123)
//! - `_PER_RESULT_WINDOW_FRACTION` → [`PER_RESULT_WINDOW_FRACTION`] (130)
//! - `_PER_TURN_WINDOW_FRACTION` → [`PER_TURN_WINDOW_FRACTION`] (131)
//! - `_MIN_RESULT_SIZE_CHARS` → [`MIN_RESULT_SIZE_CHARS`] (135)
//! - `_MIN_TURN_BUDGET_CHARS` → [`MIN_TURN_BUDGET_CHARS`] (136)
//! - `budget_for_context_window()` → [`budget_for_context_window()`] (139-174)
//!
//! Notes
//! -----
//! * `float("inf")` (pinned + registry inf) is modeled as [`Threshold::Infinite`]
//!   (`f64::INFINITY` on the wire). All other thresholds are `Finite(usize)`.
//! * Registry lookup is injected via `resolve_threshold_with` so the module has
//!   no hard dependency on `tools/registry` at compile time; the zero-arg
//!   `resolve_threshold` uses the default (return `default_result_size`).
//! * `_configured_mcp_result_size` reads `<HERMES_HOME>/config.yaml`
//!   (`tool_budget.mcp_result_size_chars`) via a line-scanning parser
//!   (no yaml crate) — guarded, any error/missing/non-positive returns the
//!   built-in default, identical to Python's `try/except: return DEFAULT`.
//! * `budget_for_context_window` keeps large models byte-identical to history
//!   (clamped to defaults) while shrinking for small windows, floored.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;

// ---------------------------------------------------------------------------
// Constants — mirrors lines 9-36
// ---------------------------------------------------------------------------

/// Tools whose thresholds must never be overridden.
/// Mirrors `PINNED_THRESHOLDS: Dict[str, float] = {"read_file": float("inf")}` (11-13).
/// `read_file=inf` prevents infinite persist→read→persist loops.
pub const PINNED_THRESHOLDS: &[(&str, f64)] = &[("read_file", f64::INFINITY)];

/// Quick check for pinned tools (just `"read_file"` today).
pub fn is_pinned(tool_name: &str) -> bool {
    PINNED_THRESHOLDS.iter().any(|(k, _)| *k == tool_name)
}

/// Mirrors `DEFAULT_RESULT_SIZE_CHARS: int = 100_000` (17).
pub const DEFAULT_RESULT_SIZE_CHARS: usize = 100_000;
/// Mirrors `DEFAULT_TURN_BUDGET_CHARS: int = 200_000` (18).
pub const DEFAULT_TURN_BUDGET_CHARS: usize = 200_000;
/// Mirrors `DEFAULT_PREVIEW_SIZE_CHARS: int = 1_500` (19).
pub const DEFAULT_PREVIEW_SIZE_CHARS: usize = 1_500;

/// Tighter default per-result threshold for MCP tools (`mcp_` prefix).
/// Mirrors `DEFAULT_MCP_RESULT_SIZE_CHARS: int = 50_000` (32).
pub const DEFAULT_MCP_RESULT_SIZE_CHARS: usize = 50_000;

/// Tool-name prefix that identifies MCP-served tools.
/// Mirrors `MCP_TOOL_PREFIX: str = "mcp_"` (36).
pub const MCP_TOOL_PREFIX: &str = "mcp_";

// Token↔char / window-fraction constants — mirrors lines 119-136

/// Mirrors `_CHARS_PER_TOKEN: int = 4` (123).
const CHARS_PER_TOKEN: usize = 4;
/// Mirrors `_PER_RESULT_WINDOW_FRACTION: float = 0.15` (130).
const PER_RESULT_WINDOW_FRACTION: f64 = 0.15;
/// Mirrors `_PER_TURN_WINDOW_FRACTION: float = 0.30` (131).
const PER_TURN_WINDOW_FRACTION: f64 = 0.30;
/// Mirrors `_MIN_RESULT_SIZE_CHARS: int = 8_000` (135).
const MIN_RESULT_SIZE_CHARS: usize = 8_000;
/// Mirrors `_MIN_TURN_BUDGET_CHARS: int = 16_000` (136).
const MIN_TURN_BUDGET_CHARS: usize = 16_000;

// ---------------------------------------------------------------------------
// Threshold — models `int | float(inf)` from Python
// ---------------------------------------------------------------------------

/// Models `int | float(inf)` returned by `BudgetConfig.resolve_threshold`.
///
/// `Finite(n)` = threshold in chars, `Infinite` = `float("inf")` (never persist).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Threshold {
    Finite(usize),
    Infinite,
}

impl Threshold {
    /// True when `float("inf")`.
    pub fn is_infinite(&self) -> bool {
        matches!(self, Threshold::Infinite)
    }
    /// `Some(n)` for `Finite`, `None` for `Infinite`.
    pub fn as_usize(&self) -> Option<usize> {
        match self {
            Threshold::Finite(n) => Some(*n),
            Threshold::Infinite => None,
        }
    }
    /// Wire value: `n as f64` or `f64::INFINITY`.
    pub fn as_f64(&self) -> f64 {
        match self {
            Threshold::Finite(n) => *n as f64,
            Threshold::Infinite => f64::INFINITY,
        }
    }
}

impl std::fmt::Display for Threshold {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Threshold::Finite(n) => write!(f, "{}", n),
            Threshold::Infinite => write!(f, "inf"),
        }
    }
}

impl From<usize> for Threshold {
    fn from(n: usize) -> Self {
        Threshold::Finite(n)
    }
}

// ---------------------------------------------------------------------------
// _configured_mcp_result_size — mirrors lines 39-63
// ---------------------------------------------------------------------------

fn get_hermes_home() -> PathBuf {
    for key in ["GRAY_HOME", "HERMES_HOME"] {
        if let Ok(val) = std::env::var(key) {
            let trimmed = val.trim().to_string();
            if !trimmed.is_empty() {
                return PathBuf::from(trimmed);
            }
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        let trimmed = home.trim().to_string();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed).join(".hermes");
        }
    }
    PathBuf::from("/tmp/.hermes")
}

/// Parse `tool_budget.mcp_result_size_chars` from raw config text.
///
/// Handles both JSON (`{"tool_budget": {"mcp_result_size_chars": 123}}`) and
/// YAML (`tool_budget:\n  mcp_result_size_chars: 123`). Returns `Some(n)` only
/// when `n > 0`; otherwise `None` so caller falls back to default.
/// Mirrors the `int(raw)` + `value > 0` guard (58-60).
fn parse_mcp_from_text(text: &str) -> Option<usize> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    // JSON fast path: starts with `{`
    if trimmed.starts_with('{') {
        return parse_mcp_from_json(text);
    }
    parse_mcp_from_yaml(text)
}

fn parse_mcp_from_json(text: &str) -> Option<usize> {
    // Cheap string search without serde_json: locate key then colon then number.
    let key = "\"mcp_result_size_chars\"";
    let idx = text.find(key)?;
    let after = &text[idx + key.len()..];
    let colon = after.find(':')?;
    let rest = after[colon + 1..].trim_start();
    // rest may start with optional quote (if value was string), handle both
    let rest = rest.trim_start_matches(|c| c == '"' || c == '\'');
    // collect optional sign/digits
    let mut end = 0usize;
    let chars: Vec<char> = rest.chars().collect();
    if chars.is_empty() {
        return None;
    }
    let mut i = 0;
    if chars[0] == '-' || chars[0] == '+' {
        i += 1;
    }
    let start_digits = i;
    while i < chars.len() && chars[i].is_ascii_digit() {
        i += 1;
        end = i;
    }
    // allow ".0"? Python int("123.7") would fail for string, but int(123.7) truncates.
    // For JSON numbers, we handle integer part only and ignore fractional.
    if end == start_digits {
        return None;
    }
    let num_str: String = chars[..end].iter().collect();
    // strip sign for parsing, but we need to check >0 after
    let cleaned = num_str.trim().trim_matches(|c| c == '"' || c == '\'');
    // Handle float-like string "123.0" — Python int("123.0") raises, so we reject if contains '.'
    if cleaned.contains('.') {
        return None;
    }
    // Parse as i64 to allow negative check
    let v: i64 = cleaned.parse().ok()?;
    if v > 0 {
        Some(v as usize)
    } else {
        None
    }
}

fn parse_mcp_from_yaml(text: &str) -> Option<usize> {
    // Minimal YAML scanner: find `tool_budget:` then indented `mcp_result_size_chars:`
    let lines: Vec<&str> = text.lines().collect();
    let mut tb_indent: Option<usize> = None;
    for line in &lines {
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indent = line.len() - trimmed.len();
        if trimmed.starts_with("tool_budget:") {
            tb_indent = Some(indent);
            continue;
        }
        if let Some(tbi) = tb_indent {
            if indent <= tbi {
                // dedented out of tool_budget
                tb_indent = None;
                if trimmed.starts_with("tool_budget:") {
                    tb_indent = Some(indent);
                }
                continue;
            }
            if trimmed.starts_with("mcp_result_size_chars:") {
                let rest = trimmed["mcp_result_size_chars:".len()..].trim();
                let raw = rest.split('#').next().unwrap_or("").trim();
                let raw = raw.trim_matches(|c| c == '"' || c == '\'');
                if raw.is_empty() || raw == "null" || raw == "~" {
                    return None;
                }
                // Reject float strings like "3.7" (Python int("3.7") fails)
                if raw.contains('.') {
                    // But allow "50000.0" ? Python int(50000.0) works when raw is float, not string.
                    // For YAML string values, we follow strict int parse.
                    return None;
                }
                // Try int parse; allow leading +/-
                let v: i64 = match raw.parse() {
                    Ok(n) => n,
                    Err(_) => return None,
                };
                if v > 0 {
                    return Some(v as usize);
                } else {
                    return None;
                }
            }
        }
    }
    None
}

/// Testable entry: parse `mcp_result_size_chars` from raw config text.
/// Returns built-in default when parsing fails (mirrors Python fallback).
pub fn configured_mcp_result_size_from_str(text: &str) -> usize {
    parse_mcp_from_text(text).unwrap_or(DEFAULT_MCP_RESULT_SIZE_CHARS)
}

/// Mirrors `def _configured_mcp_result_size() -> int:` (39-63).
///
/// Reads `tool_budget.mcp_result_size_chars` from the active config via
/// `<HERMES_HOME>/config.yaml` (the sanctioned read path). Fully guarded:
/// any error, missing key, or non-positive value returns the built-in default.
pub fn configured_mcp_result_size() -> usize {
    let home = get_hermes_home();
    let path = home.join("config.yaml");
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => return DEFAULT_MCP_RESULT_SIZE_CHARS,
    };
    configured_mcp_result_size_from_str(&text)
}

// ---------------------------------------------------------------------------
// BudgetConfig — mirrors lines 66-112
// ---------------------------------------------------------------------------

/// Immutable budget constants for the 3-layer tool result persistence system.
///
/// Layer 2 (per-result): `resolve_threshold(tool_name)` → threshold in chars.
/// Layer 3 (per-turn):   `turn_budget` → aggregate char budget across all tool
///                       results in a single assistant turn.
/// Preview:              `preview_size` → inline snippet size after persistence.
///
/// Mirrors `@dataclass(frozen=True) class BudgetConfig:` (67-112).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BudgetConfig {
    /// Mirrors `default_result_size: int = DEFAULT_RESULT_SIZE_CHARS` (76).
    pub default_result_size: usize,
    /// Mirrors `turn_budget: int = DEFAULT_TURN_BUDGET_CHARS` (77).
    pub turn_budget: usize,
    /// Mirrors `preview_size: int = DEFAULT_PREVIEW_SIZE_CHARS` (78).
    pub preview_size: usize,
    /// Mirrors `mcp_result_size: int = DEFAULT_MCP_RESULT_SIZE_CHARS` (79).
    pub mcp_result_size: usize,
    /// Mirrors `tool_overrides: Dict[str, int] = field(default_factory=dict)` (80).
    pub tool_overrides: HashMap<String, usize>,
}

impl Default for BudgetConfig {
    fn default() -> Self {
        Self {
            default_result_size: DEFAULT_RESULT_SIZE_CHARS,
            turn_budget: DEFAULT_TURN_BUDGET_CHARS,
            preview_size: DEFAULT_PREVIEW_SIZE_CHARS,
            mcp_result_size: DEFAULT_MCP_RESULT_SIZE_CHARS,
            tool_overrides: HashMap::new(),
        }
    }
}

impl BudgetConfig {
    /// Create a config with custom fields (mirrors dataclass constructor).
    pub fn new(
        default_result_size: usize,
        turn_budget: usize,
        preview_size: usize,
        mcp_result_size: usize,
        tool_overrides: HashMap<String, usize>,
    ) -> Self {
        Self {
            default_result_size,
            turn_budget,
            preview_size,
            mcp_result_size,
            tool_overrides,
        }
    }

    /// Resolve the persistence threshold for a tool.
    ///
    /// Priority: pinned → tool_overrides → mcp_ prefix → registry per-tool → default.
    ///
    /// Mirrors `def resolve_threshold(self, tool_name: str) -> int | float:` (82-112).
    /// The `registry_get` closure mirrors `registry.get_max_result_size(tool_name, default=...)` (109):
    /// it receives `(tool_name, default_result_size)` and returns `Threshold::Finite(n)` or `Infinite`.
    pub fn resolve_threshold_with<F>(&self, tool_name: &str, registry_get: F) -> Threshold
    where
        F: Fn(&str, usize) -> Threshold,
    {
        // Pinned — line 102-103
        if is_pinned(tool_name) {
            return Threshold::Infinite;
        }
        // Overrides — line 104-105
        if let Some(&v) = self.tool_overrides.get(tool_name) {
            return Threshold::Finite(v);
        }
        // MCP prefix — line 106-107
        if tool_name.starts_with(MCP_TOOL_PREFIX) {
            return Threshold::Finite(self.mcp_result_size.min(self.default_result_size));
        }
        // Registry — lines 108-112
        let registry_value = registry_get(tool_name, self.default_result_size);
        if registry_value.is_infinite() {
            return registry_value;
        }
        match registry_value {
            Threshold::Finite(v) => Threshold::Finite(v.min(self.default_result_size)),
            Threshold::Infinite => Threshold::Infinite,
        }
    }

    /// Convenience wrapper without a registry (registry returns `default`).
    ///
    /// Equivalent to a registry with no per-tool entry, so the result is
    /// `min(default_result_size, default_result_size)` = `default_result_size`
    /// for non-MCP, non-pinned, non-overridden tools.
    pub fn resolve_threshold(&self, tool_name: &str) -> Threshold {
        self.resolve_threshold_with(tool_name, |_, default| Threshold::Finite(default))
    }
}

// ---------------------------------------------------------------------------
// DEFAULT_BUDGET — mirrors line 116
// ---------------------------------------------------------------------------

static DEFAULT_BUDGET_CELL: OnceLock<BudgetConfig> = OnceLock::new();

/// Mirrors `DEFAULT_BUDGET = BudgetConfig()` (116).
pub fn default_budget() -> &'static BudgetConfig {
    DEFAULT_BUDGET_CELL.get_or_init(BudgetConfig::default)
}

/// Owned copy of the default budget (convenience).
pub fn default_budget_owned() -> BudgetConfig {
    BudgetConfig::default()
}

// ---------------------------------------------------------------------------
// budget_for_context_window — mirrors lines 139-174
// ---------------------------------------------------------------------------

/// Return a `BudgetConfig` scaled to the active model's context window.
///
/// Mirrors `def budget_for_context_window(context_length: int | None) -> BudgetConfig:` (139-174).
///
/// Scaling keeps large models byte-identical to today (clamped to defaults)
/// while shrinking for small models proportionally, floored so a usable preview
/// always survives.
///
/// `context_length` is `Option<i64>` to allow `None` (Python `None`) and
/// non-positive values which both fall back to the default/MCP-only path.
pub fn budget_for_context_window(context_length: Option<i64>) -> BudgetConfig {
    let mcp_result_size = configured_mcp_result_size();
    budget_for_context_window_with_mcp(context_length, mcp_result_size)
}

/// Testable core with injected `mcp_result_size` (avoids filesystem).
pub fn budget_for_context_window_with_mcp(
    context_length: Option<i64>,
    mcp_result_size: usize,
) -> BudgetConfig {
    // Mirrors lines 155-158
    let is_empty_or_non_positive = match context_length {
        None => true,
        Some(n) => n <= 0,
    };
    if is_empty_or_non_positive {
        if mcp_result_size == DEFAULT_MCP_RESULT_SIZE_CHARS {
            return BudgetConfig::default();
        }
        return BudgetConfig {
            mcp_result_size,
            ..BudgetConfig::default()
        };
    }
    let ctx = context_length.unwrap() as usize;
    let window_chars = ctx * CHARS_PER_TOKEN; // line 160
    let per_result = (window_chars as f64 * PER_RESULT_WINDOW_FRACTION) as usize; // line 161
    let per_turn = (window_chars as f64 * PER_TURN_WINDOW_FRACTION) as usize; // line 162

    // Clamp: never exceed historical defaults, never drop below floor — lines 166-167
    let per_result = MIN_RESULT_SIZE_CHARS.max(per_result.min(DEFAULT_RESULT_SIZE_CHARS));
    let per_turn = MIN_TURN_BUDGET_CHARS.max(per_turn.min(DEFAULT_TURN_BUDGET_CHARS));

    BudgetConfig {
        default_result_size: per_result,
        turn_budget: per_turn,
        preview_size: DEFAULT_PREVIEW_SIZE_CHARS,
        mcp_result_size,
        tool_overrides: HashMap::new(),
    }
}

// ---------------------------------------------------------------------------
// __all__ equivalent
// ---------------------------------------------------------------------------

/// Mirrors `__all__` surface of budget_config.py.
pub const ALL: &[&str] = &[
    "PINNED_THRESHOLDS",
    "DEFAULT_RESULT_SIZE_CHARS",
    "DEFAULT_TURN_BUDGET_CHARS",
    "DEFAULT_PREVIEW_SIZE_CHARS",
    "DEFAULT_MCP_RESULT_SIZE_CHARS",
    "MCP_TOOL_PREFIX",
    "BudgetConfig",
    "DEFAULT_BUDGET",
    "budget_for_context_window",
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn constants_match_python() {
        assert_eq!(DEFAULT_RESULT_SIZE_CHARS, 100_000);
        assert_eq!(DEFAULT_TURN_BUDGET_CHARS, 200_000);
        assert_eq!(DEFAULT_PREVIEW_SIZE_CHARS, 1_500);
        assert_eq!(DEFAULT_MCP_RESULT_SIZE_CHARS, 50_000);
        assert_eq!(MCP_TOOL_PREFIX, "mcp_");
        assert_eq!(CHARS_PER_TOKEN, 4);
        assert!((PER_RESULT_WINDOW_FRACTION - 0.15).abs() < f64::EPSILON);
        assert!((PER_TURN_WINDOW_FRACTION - 0.30).abs() < f64::EPSILON);
        assert_eq!(MIN_RESULT_SIZE_CHARS, 8_000);
        assert_eq!(MIN_TURN_BUDGET_CHARS, 16_000);
        assert_eq!(PINNED_THRESHOLDS.len(), 1);
        assert_eq!(PINNED_THRESHOLDS[0].0, "read_file");
        assert!(PINNED_THRESHOLDS[0].1.is_infinite());
    }

    #[test]
    fn pinned_threshold_is_infinite() {
        let cfg = BudgetConfig::default();
        assert_eq!(cfg.resolve_threshold("read_file"), Threshold::Infinite);
        assert!(cfg.resolve_threshold("read_file").is_infinite());
        // pinned beats overrides
        let mut overrides = HashMap::new();
        overrides.insert("read_file".to_string(), 123);
        let cfg2 = BudgetConfig {
            tool_overrides: overrides,
            ..BudgetConfig::default()
        };
        assert_eq!(cfg2.resolve_threshold("read_file"), Threshold::Infinite);
    }

    #[test]
    fn overrides_beat_default() {
        let mut overrides = HashMap::new();
        overrides.insert("my_tool".to_string(), 42_000);
        let cfg = BudgetConfig {
            tool_overrides: overrides,
            ..BudgetConfig::default()
        };
        assert_eq!(cfg.resolve_threshold("my_tool"), Threshold::Finite(42_000));
    }

    #[test]
    fn mcp_prefix_tighter_and_capped() {
        // default budget: mcp 50k, default 100k → min 50k
        let cfg = BudgetConfig::default();
        assert_eq!(
            cfg.resolve_threshold("mcp_some_tool"),
            Threshold::Finite(50_000)
        );
        // when default is scaled down, mcp is capped at default
        let cfg_small = BudgetConfig {
            default_result_size: 30_000,
            mcp_result_size: 50_000,
            ..BudgetConfig::default()
        };
        assert_eq!(
            cfg_small.resolve_threshold("mcp_x"),
            Threshold::Finite(30_000)
        );
        // mcp smaller than default
        let cfg2 = BudgetConfig {
            default_result_size: 100_000,
            mcp_result_size: 10_000,
            ..BudgetConfig::default()
        };
        assert_eq!(cfg2.resolve_threshold("mcp_x"), Threshold::Finite(10_000));
    }

    #[test]
    fn registry_capped_at_default() {
        let cfg = BudgetConfig::default(); // default 100k
        // registry returns 100k → capped at 100k = 100k
        let r = cfg.resolve_threshold_with("web_search", |_, _| Threshold::Finite(100_000));
        assert_eq!(r, Threshold::Finite(100_000));
        // registry returns 200k (large) → capped at 100k
        let r2 = cfg.resolve_threshold_with("web_search", |_, _| Threshold::Finite(200_000));
        assert_eq!(r2, Threshold::Finite(100_000));
        // registry returns 60k → min(60k,100k)=60k
        let r3 = cfg.resolve_threshold_with("some_tool", |_, _| Threshold::Finite(60_000));
        assert_eq!(r3, Threshold::Finite(60_000));
        // small budget: default 20k, registry 100k → capped at 20k
        let cfg_small = BudgetConfig {
            default_result_size: 20_000,
            ..BudgetConfig::default()
        };
        let r4 = cfg_small.resolve_threshold_with("web_search", |_, _| Threshold::Finite(100_000));
        assert_eq!(r4, Threshold::Finite(20_000));
    }

    #[test]
    fn registry_inf_passes_through() {
        let cfg = BudgetConfig::default();
        let r = cfg.resolve_threshold_with("any", |_, _| Threshold::Infinite);
        assert_eq!(r, Threshold::Infinite);
    }

    #[test]
    fn priority_order() {
        // pinned > overrides > mcp > registry
        let mut overrides = HashMap::new();
        overrides.insert("mcp_tool".to_string(), 99_000);
        let cfg = BudgetConfig {
            tool_overrides: overrides,
            ..BudgetConfig::default()
        };
        // mcp_tool is both override and mcp prefix → override wins
        assert_eq!(cfg.resolve_threshold("mcp_tool"), Threshold::Finite(99_000));
        // non-mcp override
        assert_eq!(cfg.resolve_threshold("mcp_tool"), Threshold::Finite(99_000));
    }

    #[test]
    fn default_budget_singleton() {
        let a = default_budget();
        let b = default_budget();
        assert_eq!(a as *const _, b as *const _);
        assert_eq!(a.default_result_size, DEFAULT_RESULT_SIZE_CHARS);
        assert_eq!(a.turn_budget, DEFAULT_TURN_BUDGET_CHARS);
        assert_eq!(a.preview_size, DEFAULT_PREVIEW_SIZE_CHARS);
        assert_eq!(a.mcp_result_size, DEFAULT_MCP_RESULT_SIZE_CHARS);
    }

    #[test]
    fn configured_mcp_from_str_yaml() {
        let yaml = "tool_budget:\n  mcp_result_size_chars: 60000\n";
        assert_eq!(configured_mcp_result_size_from_str(yaml), 60_000);
        // missing block → default
        assert_eq!(
            configured_mcp_result_size_from_str("model: foo\n"),
            DEFAULT_MCP_RESULT_SIZE_CHARS
        );
        // non-positive → default
        assert_eq!(
            configured_mcp_result_size_from_str("tool_budget:\n  mcp_result_size_chars: 0\n"),
            DEFAULT_MCP_RESULT_SIZE_CHARS
        );
        assert_eq!(
            configured_mcp_result_size_from_str("tool_budget:\n  mcp_result_size_chars: -5\n"),
            DEFAULT_MCP_RESULT_SIZE_CHARS
        );
        // string float → default (Python int("3.7") fails)
        assert_eq!(
            configured_mcp_result_size_from_str("tool_budget:\n  mcp_result_size_chars: \"3.7\"\n"),
            DEFAULT_MCP_RESULT_SIZE_CHARS
        );
    }

    #[test]
    fn configured_mcp_from_str_json() {
        let json = r#"{"tool_budget": {"mcp_result_size_chars": 70000}}"#;
        assert_eq!(configured_mcp_result_size_from_str(json), 70_000);
        let json2 = r#"{"tool_budget": {"mcp_result_size_chars": 0}}"#;
        assert_eq!(
            configured_mcp_result_size_from_str(json2),
            DEFAULT_MCP_RESULT_SIZE_CHARS
        );
    }

    #[test]
    fn configured_mcp_yaml_with_inline_comment() {
        let yaml = "tool_budget:\n  mcp_result_size_chars: 55000 # inline comment\n";
        assert_eq!(configured_mcp_result_size_from_str(yaml), 55_000);
    }

    #[test]
    fn budget_for_context_window_none_and_zero() {
        // None, 0, negative all fallback to default
        let b = budget_for_context_window_with_mcp(None, DEFAULT_MCP_RESULT_SIZE_CHARS);
        assert_eq!(b.default_result_size, DEFAULT_RESULT_SIZE_CHARS);
        assert_eq!(b.turn_budget, DEFAULT_TURN_BUDGET_CHARS);
        let b2 = budget_for_context_window_with_mcp(Some(0), DEFAULT_MCP_RESULT_SIZE_CHARS);
        assert_eq!(b2.default_result_size, DEFAULT_RESULT_SIZE_CHARS);
        let b3 = budget_for_context_window_with_mcp(Some(-5), DEFAULT_MCP_RESULT_SIZE_CHARS);
        assert_eq!(b3.default_result_size, DEFAULT_RESULT_SIZE_CHARS);
        // with custom mcp, fallback returns config with that mcp
        let b4 = budget_for_context_window_with_mcp(None, 60_000);
        assert_eq!(b4.default_result_size, DEFAULT_RESULT_SIZE_CHARS);
        assert_eq!(b4.mcp_result_size, 60_000);
    }

    #[test]
    fn budget_for_context_window_large_unchanged() {
        // Large model: 200k tokens → window 800k chars → 15% =120k capped to 100k, 30%=240k capped to 200k
        let b = budget_for_context_window_with_mcp(Some(200_000), DEFAULT_MCP_RESULT_SIZE_CHARS);
        assert_eq!(b.default_result_size, DEFAULT_RESULT_SIZE_CHARS);
        assert_eq!(b.turn_budget, DEFAULT_TURN_BUDGET_CHARS);
        assert_eq!(b.preview_size, DEFAULT_PREVIEW_SIZE_CHARS);
    }

    #[test]
    fn budget_for_context_window_small_scaled() {
        // 32k tokens → 128k window → per_result 19200, per_turn 38400 (both above floor, below cap)
        let b = budget_for_context_window_with_mcp(Some(32_000), DEFAULT_MCP_RESULT_SIZE_CHARS);
        assert_eq!(b.default_result_size, 19_200);
        assert_eq!(b.turn_budget, 38_400);
    }

    #[test]
    fn budget_for_context_window_tiny_floored() {
        // 4k tokens → 16k window → per_result 2400 floored 8000, per_turn 4800 floored 16000
        let b = budget_for_context_window_with_mcp(Some(4_000), DEFAULT_MCP_RESULT_SIZE_CHARS);
        assert_eq!(b.default_result_size, MIN_RESULT_SIZE_CHARS);
        assert_eq!(b.turn_budget, MIN_TURN_BUDGET_CHARS);
    }

    #[test]
    fn budget_for_context_window_mid() {
        // 65k tokens → 260k window → per_result 39000, per_turn 78000
        let b = budget_for_context_window_with_mcp(Some(65_000), DEFAULT_MCP_RESULT_SIZE_CHARS);
        assert_eq!(b.default_result_size, 39_000);
        assert_eq!(b.turn_budget, 78_000);
    }

    #[test]
    fn budget_preserves_mcp_custom() {
        let b = budget_for_context_window_with_mcp(Some(32_000), 60_000);
        assert_eq!(b.mcp_result_size, 60_000);
        let b2 = budget_for_context_window_with_mcp(Some(200_000), 60_000);
        assert_eq!(b2.mcp_result_size, 60_000);
    }

    #[test]
    fn threshold_display_and_conversions() {
        assert_eq!(Threshold::Finite(123).to_string(), "123");
        assert_eq!(Threshold::Infinite.to_string(), "inf");
        assert_eq!(Threshold::Finite(10).as_usize(), Some(10));
        assert_eq!(Threshold::Infinite.as_usize(), None);
        assert!(Threshold::Infinite.as_f64().is_infinite());
        assert_eq!(Threshold::Finite(42).as_f64(), 42.0);
        assert!(Threshold::Infinite.is_infinite());
        assert!(!Threshold::Finite(1).is_infinite());
    }
}
