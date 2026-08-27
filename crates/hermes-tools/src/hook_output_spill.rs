//! Spill oversized hook-injected context to disk with a preview placeholder.
//! Port of `tools/hook_output_spill.py` (241 lines) — 1:1 behavior.
//!
//! Ported from openai/codex PR #21069 (``Spill large hook outputs from context``).
//!
//! Background
//! ----------
//! Both shell hooks (``agent/shell_hooks.py``) and Python plugins
//! (``pre_llm_call`` hook in ``run_agent.py``) can return ``{"context": "..."}``
//! which gets concatenated into the current turn's user message on EVERY
//! subsequent API call. If a hook emits a large blob, that blob inflates
//! every turn and blows out the prompt cache prefix.
//!
//! This mirrors what Codex does for its ``PreToolUse``/``Stop``/feedback
//! hooks: once the injected text exceeds a configured budget, write the
//! full content to a per-session directory on disk and replace the in-prompt
//! payload with a head/tail preview plus the saved path. The model can still
//! inspect the full content via ``read_file`` or ``terminal`` if it needs to.
//!
//! Config (``config.yaml``):: 
//!
//!     hooks:
//!       output_spill:
//!         enabled: true          # default: true; set false to disable spilling
//!         max_chars: 10000       # default; context above this is spilled
//!         preview_head: 500      # chars shown at the start of the preview
//!         preview_tail: 500      # chars shown at the end of the preview
//!         directory: null        # default: <HERMES_HOME>/hook_outputs
//!
//! Design invariants
//! -----------------
//! * Behaviour-preserving when ``enabled: false`` or when content is under
//!   the cap — return the input string unchanged.
//! * Never raises. Any I/O error falls back to a byte-length truncation with
//!   an in-prompt notice — the hook context still reaches the model, just
//!   bounded in size.
//! * Spill files are grouped by session so a ``/new`` session doesn't grow
//!   them forever in one directory.
//!
//! Mapping
//! -------
//! - `DEFAULT_MAX_CHARS = 10_000` → [`DEFAULT_MAX_CHARS`]
//! - `DEFAULT_PREVIEW_HEAD = 500` → [`DEFAULT_PREVIEW_HEAD`]
//! - `DEFAULT_PREVIEW_TAIL = 500` → [`DEFAULT_PREVIEW_TAIL`]
//! - `DEFAULT_ENABLED = True` → [`DEFAULT_ENABLED`]
//! - `_coerce_positive_int` → [`coerce_positive_int`]
//! - `_coerce_non_negative_int` → [`coerce_non_negative_int`]
//! - `get_spill_config()` → [`get_spill_config`] / [`get_spill_config_from_value`]
//! - `_resolve_spill_dir` → [`resolve_spill_dir`]
//! - `_build_preview` → [`build_preview`]
//! - `spill_if_oversized` → [`spill_if_oversized`]
//! - `__all__` → [`ALL`]

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Constants — mirrors lines 54-57
// ---------------------------------------------------------------------------

/// Mirrors `DEFAULT_MAX_CHARS = 10_000` (line 54).
pub const DEFAULT_MAX_CHARS: usize = 10_000;
/// Mirrors `DEFAULT_PREVIEW_HEAD = 500` (line 55).
pub const DEFAULT_PREVIEW_HEAD: usize = 500;
/// Mirrors `DEFAULT_PREVIEW_TAIL = 500` (line 56).
pub const DEFAULT_PREVIEW_TAIL: usize = 500;
/// Mirrors `DEFAULT_ENABLED = True` (line 57).
pub const DEFAULT_ENABLED: bool = true;

/// Mirrors `__all__` (lines 234-241).
pub const ALL: &[&str] = &[
    "DEFAULT_MAX_CHARS",
    "DEFAULT_PREVIEW_HEAD",
    "DEFAULT_PREVIEW_TAIL",
    "DEFAULT_ENABLED",
    "get_spill_config",
    "spill_if_oversized",
];

// ---------------------------------------------------------------------------
// Config struct — mirrors `get_spill_config` return dict (lines 102-112)
// ---------------------------------------------------------------------------

/// Resolved hook output-spill config. Mirrors the dict returned by
/// `get_spill_config()` (lines 102-112).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpillConfig {
    /// Mirrors `enabled` (default `true`).
    pub enabled: bool,
    /// Mirrors `max_chars` (default `10_000`).
    pub max_chars: usize,
    /// Mirrors `preview_head` (default `500`).
    pub preview_head: usize,
    /// Mirrors `preview_tail` (default `500`).
    pub preview_tail: usize,
    /// Mirrors `directory` (`None` → `<HERMES_HOME>/hook_outputs`).
    pub directory: Option<String>,
}

impl Default for SpillConfig {
    fn default() -> Self {
        Self {
            enabled: DEFAULT_ENABLED,
            max_chars: DEFAULT_MAX_CHARS,
            preview_head: DEFAULT_PREVIEW_HEAD,
            preview_tail: DEFAULT_PREVIEW_TAIL,
            directory: None,
        }
    }
}

// ---------------------------------------------------------------------------
// _coerce helpers — mirrors lines 60-78
// ---------------------------------------------------------------------------

/// Mirrors `def _coerce_positive_int(value: Any, default: int) -> int:` (60-67).
///
/// Tries `int(value)` semantics: parse as integer, return `default` if
/// parsing fails or if `iv <= 0`.
pub fn coerce_positive_int(value: Option<&serde_json::Value>, default: i64) -> i64 {
    let v = match value {
        Some(val) => val,
        None => return default,
    };
    let iv = match value_to_int(v) {
        Some(i) => i,
        None => return default,
    };
    if iv <= 0 {
        default
    } else {
        iv
    }
}

/// Mirrors `def _coerce_non_negative_int(value: Any, default: int) -> int:` (70-78).
///
/// Like `coerce_positive_int` but allows zero.
pub fn coerce_non_negative_int(value: Option<&serde_json::Value>, default: i64) -> i64 {
    let v = match value {
        Some(val) => val,
        None => return default,
    };
    let iv = match value_to_int(v) {
        Some(i) => i,
        None => return default,
    };
    if iv < 0 {
        default
    } else {
        iv
    }
}

/// Helper: emulate Python `int(value)` for serde_json::Value.
///
/// - Null/missing → None (caller returns default)
/// - Bool → 1 / 0 (Python bool is subclass of int)
/// - Number → truncates floats like `int(3.7) == 3`
/// - String → trimmed decimal parse (``int(" 123 ") == 123``; ``int("3.7")`` fails)
/// - Other (array/object) → None
fn value_to_int(v: &serde_json::Value) -> Option<i64> {
    match v {
        serde_json::Value::Null => None,
        serde_json::Value::Bool(b) => Some(if *b { 1 } else { 0 }),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Some(i)
            } else if let Some(u) = n.as_u64() {
                // Clamp to i64 range like Python's big int → but for config values fits.
                if u <= i64::MAX as u64 {
                    Some(u as i64)
                } else {
                    Some(i64::MAX)
                }
            } else if let Some(f) = n.as_f64() {
                // Python int(3.7) truncates toward zero.
                Some(f.trunc() as i64)
            } else {
                None
            }
        }
        serde_json::Value::String(s) => {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                return None;
            }
            // Python int(" 123 ") allows surrounding whitespace but not float strings.
            // Try direct i64 parse; if fails try to handle leading +/-.
            // We do not parse "3.7" as 3 — Python int("3.7") raises ValueError.
            match trimmed.parse::<i64>() {
                Ok(i) => Some(i),
                Err(_) => None,
            }
        }
        _ => None,
    }
}

/// Convenience: coerce from raw i64 string-ish helper used by file parser.
pub fn coerce_positive_int_str(raw: Option<&str>, default: i64) -> i64 {
    match raw {
        None => default,
        Some(s) => {
            let trimmed = s.trim().trim_matches(|c| c == '"' || c == '\'');
            match trimmed.parse::<i64>() {
                Ok(iv) if iv > 0 => iv,
                _ => default,
            }
        }
    }
}

pub fn coerce_non_negative_int_str(raw: Option<&str>, default: i64) -> i64 {
    match raw {
        None => default,
        Some(s) => {
            let trimmed = s.trim().trim_matches(|c| c == '"' || c == '\'');
            match trimmed.parse::<i64>() {
                Ok(iv) if iv >= 0 => iv,
                _ => default,
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers: get_hermes_home, expand_user, format_commas, random_hex
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

fn expand_user(path_str: &str) -> String {
    if path_str == "~" || path_str.starts_with("~/") {
        if let Ok(home) = std::env::var("HOME") {
            if path_str == "~" {
                return home;
            }
            return format!("{}{}", home, &path_str[1..]);
        }
    }
    path_str.to_string()
}

fn format_with_commas(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    let bytes = s.as_bytes();
    let first = bytes.len() % 3;
    let mut start = 0usize;
    if first != 0 {
        out.push_str(&s[..first]);
        start = first;
        if start < bytes.len() {
            out.push(',');
        }
    }
    while start < bytes.len() {
        out.push_str(&s[start..start + 3]);
        start += 3;
        if start < bytes.len() {
            out.push(',');
        }
    }
    if out.ends_with(',') {
        out.pop();
    }
    if out.is_empty() {
        s
    } else {
        out
    }
}

fn random_hex32() -> String {
    let mut buf = [0u8; 16];
    // Try /dev/urandom first (mirrors subagent_worktree::random_hex8).
    if let Ok(mut f) = fs::File::open("/dev/urandom") {
        use std::io::Read;
        if f.read_exact(&mut buf).is_ok() {
            return buf.iter().map(|b| format!("{b:02x}")).collect();
        }
    }
    // Fallback: time + pid mixing.
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id() as u128;
    // Mix into 16 bytes via simple split.
    let mut out = String::with_capacity(32);
    let mut v = nanos.wrapping_add(pid).wrapping_add(0x9e3779b97f4a7c15u128);
    for _ in 0..16 {
        let b = (v & 0xFF) as u8;
        out.push_str(&format!("{b:02x}"));
        v >>= 8;
        if v == 0 {
            v = nanos.wrapping_mul(0x85ebca6b).wrapping_add(pid as u128);
        }
    }
    out.truncate(32);
    if out.len() < 32 {
        out.push_str(&"0".repeat(32 - out.len()));
    }
    out
}

// ---------------------------------------------------------------------------
// ensure_spill_dir / write_text_exclusive — mirrors tools/spill_safety.py
// ---------------------------------------------------------------------------

/// Mirrors `spill_safety.ensure_spill_dir(path, private=True)` (lines 51-68).
///
/// Create `path` (and parents) as a directory, refusing symlinks. With
/// `private=True` the leaf is `0o700` and tightened if already exists.
pub fn ensure_spill_dir(path: &Path, private: bool) -> std::io::Result<PathBuf> {
    if private {
        fs::create_dir_all(path)?;
    } else {
        fs::create_dir_all(path)?;
    }
    let meta = fs::symlink_metadata(path)?;
    if !meta.file_type().is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("spill dir is not a directory (symlink?): {}", path.display()),
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if private {
            let mode = meta.permissions().mode() & 0o777;
            if mode != 0o700 {
                let mut perms = meta.permissions();
                perms.set_mode(0o700);
                let _ = fs::set_permissions(path, perms);
            }
        }
    }
    Ok(path.to_path_buf())
}

/// Mirrors `spill_safety.write_text_exclusive(path, text, private=True)` (105-118).
///
/// Exclusive create (O_CREAT|O_EXCL|O_NOFOLLOW), never follows a symlink.
/// `private=True` → `0o600`, `private=False` → `0o666` (umask honored).
pub fn write_text_exclusive(path: &Path, text: &str, private: bool) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut opts = fs::OpenOptions::new();
        opts.write(true);
        opts.create_new(true);
        // O_NOFOLLOW: 0o400000 on Linux, 0x0100 on macOS. Use 0o400000 which is harmless
        // on macOS (will be ignored or fail closed if kernel enforces). Best effort.
        #[cfg(target_os = "linux")]
        const O_NOFOLLOW: i32 = 0o400000;
        #[cfg(not(target_os = "linux"))]
        const O_NOFOLLOW: i32 = 0x0100;
        opts.custom_flags(O_NOFOLLOW);
        if private {
            opts.mode(0o600);
        } else {
            opts.mode(0o666);
        }
        let mut file = opts.open(path)?;
        file.write_all(text.as_bytes())?;
        file.flush()?;
        return Ok(());
    }
    #[cfg(not(unix))]
    {
        // Windows: O_NOFOLLOW not needed; O_EXCL already refuses any existing path.
        let mut opts = fs::OpenOptions::new();
        opts.write(true);
        opts.create_new(true);
        let mut file = opts.open(path)?;
        file.write_all(text.as_bytes())?;
        file.flush()?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// get_spill_config — mirrors lines 81-112
// ---------------------------------------------------------------------------

/// Mirrors `def get_spill_config() -> Dict[str, Any]:` (81-112).
///
/// Return resolved hook output-spill config. Never raises — any error
/// returns defaults. Reads `<HERMES_HOME>/config.yaml` and extracts
/// `hooks.output_spill` section without requiring a yaml crate.
pub fn get_spill_config() -> SpillConfig {
    let home = get_hermes_home();
    let cfg_path = home.join("config.yaml");
    let text = match fs::read_to_string(&cfg_path) {
        Ok(t) => t,
        Err(_) => return SpillConfig::default(),
    };
    // Try to parse as JSON first (if config is JSON) via Value, else use line scanner.
    // Attempt JSON parse for robustness (some configs may be JSON).
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(&text) {
        return get_spill_config_from_value(&val);
    }
    // Minimal YAML scanner for hooks.output_spill.
    parse_spill_config_from_yaml_text(&text)
}

/// Testable core: mirrors `get_spill_config` extraction logic against a `serde_json::Value`
/// that represents the loaded config dict (as `hermes_cli.config.load_config()` would).
///
/// ```python
/// cfg = load_config() or {}
/// hooks = cfg.get("hooks") if isinstance(cfg, dict) else None
/// if isinstance(hooks, dict):
///     sub = hooks.get("output_spill")
///     if isinstance(sub, dict):
///         section = sub
/// ```
/// Enabled uses `bool(enabled_raw)` semantics (see module docs).
pub fn get_spill_config_from_value(root: &serde_json::Value) -> SpillConfig {
    let mut section: Option<&serde_json::Value> = None;
    if let Some(obj) = root.as_object() {
        if let Some(hooks) = obj.get("hooks") {
            if let Some(hooks_obj) = hooks.as_object() {
                if let Some(sub) = hooks_obj.get("output_spill") {
                    if sub.is_object() {
                        section = Some(sub);
                    }
                }
            }
        }
    }
    let sec = match section {
        Some(s) => s,
        None => return SpillConfig::default(),
    };
    let map = sec.as_object().expect("section is object");

    // enabled: bool(enabled_raw) if enabled_raw is not None else DEFAULT_ENABLED
    let enabled = match map.get("enabled") {
        None => DEFAULT_ENABLED,
        Some(v) if v.is_null() => DEFAULT_ENABLED,
        Some(v) => match v {
            serde_json::Value::Bool(b) => *b,
            serde_json::Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    i != 0
                } else if let Some(u) = n.as_u64() {
                    u != 0
                } else if let Some(f) = n.as_f64() {
                    f != 0.0
                } else {
                    true
                }
            }
            serde_json::Value::String(s) => !s.is_empty(),
            // Python bool([]) == False, bool({}) == False, but config's enabled is never list/dict
            // For other types, treat as true (non-empty container is truthy)
            _ => true,
        },
    };

    // directory: None if not string, else Some(string) (even empty string is falsy later)
    let directory = match map.get("directory") {
        Some(serde_json::Value::String(s)) => Some(s.clone()),
        Some(serde_json::Value::Null) => None,
        None => None,
        Some(_) => None, // not a string → None (mirrors Python `if directory is not None and not isinstance(directory, str): directory = None`)
        _ => None,
    };

    // max_chars, preview_head, preview_tail via coercers
    let max_chars = coerce_positive_int(map.get("max_chars"), DEFAULT_MAX_CHARS as i64) as usize;
    let preview_head = coerce_non_negative_int(map.get("preview_head"), DEFAULT_PREVIEW_HEAD as i64) as usize;
    let preview_tail = coerce_non_negative_int(map.get("preview_tail"), DEFAULT_PREVIEW_TAIL as i64) as usize;

    SpillConfig {
        enabled,
        max_chars,
        preview_head,
        preview_tail,
        directory,
    }
}

/// Minimal YAML scanner for `hooks.output_spill` without a yaml crate.
///
/// Finds `hooks:` then `output_spill:` indented beneath it, then captures
/// `enabled`, `max_chars`, `preview_head`, `preview_tail`, `directory` lines
/// indented deeper until dedent. Returns defaults for any missing/invalid.
fn parse_spill_config_from_yaml_text(text: &str) -> SpillConfig {
    let mut cfg = SpillConfig::default();
    let lines: Vec<&str> = text.lines().collect();
    let mut hooks_indent: Option<usize> = None;
    let mut spill_indent: Option<usize> = None;

    for line in &lines {
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indent = line.len() - trimmed.len();
        // Detect hooks:
        if trimmed.starts_with("hooks:") {
            hooks_indent = Some(indent);
            spill_indent = None;
            continue;
        }
        if let Some(hi) = hooks_indent {
            if indent <= hi {
                // Dedented out of hooks
                hooks_indent = None;
                spill_indent = None;
                // Check if this line itself is a new hooks:
                if trimmed.starts_with("hooks:") {
                    hooks_indent = Some(indent);
                }
                continue;
            }
            if trimmed.starts_with("output_spill:") {
                spill_indent = Some(indent);
                continue;
            }
            if let Some(si) = spill_indent {
                if indent <= si {
                    // Dedented out of output_spill
                    spill_indent = None;
                    // Might be another key inside hooks
                    if trimmed.starts_with("output_spill:") {
                        spill_indent = Some(indent);
                    }
                    continue;
                }
                // Inside output_spill — parse key: value
                if let Some(colon) = trimmed.find(':') {
                    let key = trimmed[..colon].trim();
                    let raw_val = trimmed[colon + 1..].trim();
                    // Strip inline comment
                    let raw_val = raw_val.split('#').next().unwrap_or("").trim();
                    // Strip surrounding quotes
                    let raw_val_unquoted = raw_val.trim_matches(|c| c == '"' || c == '\'');
                    match key {
                        "enabled" => {
                            // bool(enabled_raw) semantics: check raw string
                            // YAML booleans are unquoted: true/false, yes/no, on/off, 1/0
                            let lower = raw_val_unquoted.to_ascii_lowercase();
                            if raw_val_unquoted.is_empty() || lower == "null" || lower == "~" {
                                cfg.enabled = DEFAULT_ENABLED;
                            } else if lower == "true" || lower == "yes" || lower == "on" || lower == "1" {
                                cfg.enabled = true;
                            } else if lower == "false" || lower == "no" || lower == "off" || lower == "0" {
                                cfg.enabled = false;
                            } else {
                                // Any other non-empty string → bool("...") == true in Python
                                // But for YAML, a string value for enabled would be quoted; treat as true if non-empty
                                cfg.enabled = !raw_val_unquoted.is_empty();
                            }
                        }
                        "max_chars" => {
                            let v = raw_val_unquoted.parse::<i64>().unwrap_or(0);
                            if v > 0 {
                                cfg.max_chars = v as usize;
                            }
                        }
                        "preview_head" => {
                            let v = raw_val_unquoted.parse::<i64>().unwrap_or(-1);
                            if v >= 0 {
                                cfg.preview_head = v as usize;
                            }
                        }
                        "preview_tail" => {
                            let v = raw_val_unquoted.parse::<i64>().unwrap_or(-1);
                            if v >= 0 {
                                cfg.preview_tail = v as usize;
                            }
                        }
                        "directory" => {
                            if raw_val_unquoted.is_empty() || raw_val_unquoted == "null" || raw_val_unquoted == "~" {
                                cfg.directory = None;
                            } else {
                                // Remove surrounding quotes already done; keep as string
                                // Handle quoted vs unquoted
                                let dir = raw_val.trim().trim_matches(|c| c == '"' || c == '\'').to_string();
                                // Python checks `if directory is not None and not isinstance(directory, str): directory = None`
                                // YAML strings are always strings, so keep it.
                                // But if raw was null (~), we already handled None.
                                // Empty string? Keep as Some("")? Python would treat empty as falsy later in _resolve_spill_dir
                                cfg.directory = Some(dir);
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    cfg
}

// ---------------------------------------------------------------------------
// _resolve_spill_dir — mirrors lines 115-129
// ---------------------------------------------------------------------------

/// Mirrors `def _resolve_spill_dir(directory_override: Optional[str], session_id: Optional[str]) -> Path:` (115-129).
///
/// Return the directory where spill files for this session live.
pub fn resolve_spill_dir(directory_override: Option<&str>, session_id: Option<&str>) -> PathBuf {
    let base = if let Some(dir) = directory_override {
        let trimmed = dir.trim();
        if trimmed.is_empty() {
            get_hermes_home().join("hook_outputs")
        } else {
            let expanded = expand_user(trimmed);
            PathBuf::from(expanded)
        }
    } else {
        get_hermes_home().join("hook_outputs")
    };

    // Group by session so spills are contained per conversation.
    let mut session_segment = session_id.unwrap_or("no-session").to_string();
    // Defensive: strip path separators so a weird session id can't escape.
    // Mirrors: `session_segment.replace("/", "_").replace("\\", "_").replace("..", "_")`
    session_segment = session_segment.replace('/', "_");
    session_segment = session_segment.replace('\\', "_");
    session_segment = session_segment.replace("..", "_");
    base.join(session_segment)
}

// ---------------------------------------------------------------------------
// _build_preview — mirrors lines 132-155
// ---------------------------------------------------------------------------

/// Mirrors `def _build_preview(text: str, head: int, tail: int, saved_path: Optional[str], *, source: str) -> str:` (132-155).
///
/// Assemble the in-prompt preview with head/tail and saved-path footer.
pub fn build_preview(text: &str, head: usize, tail: usize, saved_path: Option<&str>, source: &str) -> String {
    let total = text.chars().count();
    let head_chunk = if head > 0 {
        text.chars().take(head).collect::<String>()
    } else {
        String::new()
    };
    let tail_chunk = if tail > 0 && total > head {
        let skip = total.saturating_sub(tail);
        text.chars().skip(skip).collect::<String>()
    } else {
        String::new()
    };

    let total_fmt = format_with_commas(total);
    let mut parts: Vec<String> = Vec::new();
    let header = if let Some(p) = saved_path {
        format!("[{} output truncated — {} chars; full content saved to {}]", source, total_fmt, p)
    } else {
        format!("[{} output truncated — {} chars; full content unavailable — spill write failed]", source, total_fmt)
    };
    parts.push(header);
    if !head_chunk.is_empty() {
        parts.push("--- head ---".to_string());
        parts.push(head_chunk);
    }
    if !tail_chunk.is_empty() {
        parts.push("--- tail ---".to_string());
        parts.push(tail_chunk);
    }
    parts.join("\n")
}

// ---------------------------------------------------------------------------
// spill_if_oversized — mirrors lines 158-231
// ---------------------------------------------------------------------------

/// Mirrors `def spill_if_oversized(text: str, *, session_id: Optional[str] = None, source: str = "hook", config: Optional[Dict[str, Any]] = None) -> str:` (158-231).
///
/// Spill `text` to disk if it exceeds the configured cap. Returns either
/// `text` unchanged (when under the cap, disabled, or empty) or a preview
/// string with a filesystem path pointing at the full content.
///
/// * `text` — raw injected-context string from a hook.
/// * `session_id` — used to group spill files by conversation. Falls back to
///   `"no-session"` if missing.
/// * `source` — human-readable label used in the preview header (``"hook"``,
///   ``"plugin hook"``, ``"shell hook"``, etc.).
/// * `config` — optional override for tests; normally resolved from
///   ``config.yaml`` via [`get_spill_config`].
pub fn spill_if_oversized(
    text: &str,
    session_id: Option<&str>,
    source: &str,
    config: Option<&SpillConfig>,
) -> String {
    // Python handles None/non-string; in Rust caller passes &str, but we keep
    // the same early returns for consistency: empty is allowed.
    // Note: `text` is always &str here; the `None` and `str()` coercion
    // branches are not needed in the typed API, but we provide a wrapper
    // `spill_if_oversized_optional` for Option handling.

    // Resolve cfg: `cfg = config if config is not None else get_spill_config()`
    let owned_cfg;
    let cfg = match config {
        Some(c) => c,
        None => {
            owned_cfg = get_spill_config();
            &owned_cfg
        }
    };
    if !cfg.enabled {
        return text.to_string();
    }

    let max_chars = cfg.max_chars;
    // Python uses `len(text)` which counts chars (codepoints), not bytes.
    let len = text.chars().count();
    if len <= max_chars {
        return text.to_string();
    }

    let head = cfg.preview_head;
    let tail = cfg.preview_tail;
    let directory_override = cfg.directory.as_deref();

    // Try to write the spill file. If that fails we still need to return
    // something bounded — never let a disk failure blow up the turn.
    let mut saved_path: Option<String> = None;
    // Encapsulate the fallible block so any error maps to None.
    let write_result: std::io::Result<String> = (|| {
        let spill_dir = resolve_spill_dir(directory_override, session_id);
        // Hook context may embed raw secrets: private dir/file perms, and an
        // exclusive symlink-refusing create so a planted link can't redirect
        // the write (predictable per-session directory).
        ensure_spill_dir(&spill_dir, true)?;
        let filename = format!("{}.txt", random_hex32());
        let spill_path = spill_dir.join(filename);
        // Write the raw text plus a trailing newline so tail readers don't
        // report "missing newline". Mirrors `text if text.endswith("\n") else text + "\n"`
        let to_write = if text.ends_with('\n') {
            text.to_string()
        } else {
            format!("{text}\n")
        };
        write_text_exclusive(&spill_path, &to_write, true)?;
        Ok(spill_path.to_string_lossy().to_string())
    })();

    match write_result {
        Ok(p) => saved_path = Some(p),
        Err(_exc) => {
            // Mirrors `logger.warning("hook output spill failed: %s", exc)`
            // In Rust we don't have a logger; swallow and keep saved_path None.
            saved_path = None;
        }
    }

    build_preview(text, head, tail, saved_path.as_deref(), source)
}

/// Variant that handles `Option<&str>` for `text` like Python's `None` case.
///
/// Mirrors:
/// ```python
/// if text is None:
///     return ""
/// if not isinstance(text, str):
///     try:
///         text = str(text)
///     except Exception:
///         return ""
/// ```
pub fn spill_if_oversized_optional(
    text: Option<&str>,
    session_id: Option<&str>,
    source: &str,
    config: Option<&SpillConfig>,
) -> String {
    match text {
        None => String::new(),
        Some(s) => spill_if_oversized(s, session_id, source, config),
    }
}

/// Convenience with default source `"hook"` and current session.
///
/// Mirrors Python defaults `session_id=None, source="hook"`.
pub fn spill_if_oversized_default(text: &str) -> String {
    spill_if_oversized(text, None, "hook", None)
}

/// Convenience with injected config and default session/source.
pub fn spill_if_oversized_with_config(text: &str, config: &SpillConfig) -> String {
    spill_if_oversized(text, None, "hook", Some(config))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp_home(prefix: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "hermes-hook-spill-test-{}-{}-{}",
            prefix,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::create_dir_all(&p);
        p
    }

    #[test]
    fn constants_match_python() {
        assert_eq!(DEFAULT_MAX_CHARS, 10_000);
        assert_eq!(DEFAULT_PREVIEW_HEAD, 500);
        assert_eq!(DEFAULT_PREVIEW_TAIL, 500);
        assert!(DEFAULT_ENABLED);
        assert_eq!(ALL.len(), 6);
    }

    #[test]
    fn coerce_positive_int_behaviour() {
        use serde_json::json;
        assert_eq!(coerce_positive_int(None, 10), 10);
        assert_eq!(coerce_positive_int(Some(&json!(0)), 10), 10);
        assert_eq!(coerce_positive_int(Some(&json!(-5)), 10), 10);
        assert_eq!(coerce_positive_int(Some(&json!(5)), 10), 5);
        assert_eq!(coerce_positive_int(Some(&json!("3")), 10), 3);
        assert_eq!(coerce_positive_int(Some(&json!("0")), 10), 10);
        assert_eq!(coerce_positive_int(Some(&json!("abc")), 10), 10);
        assert_eq!(coerce_positive_int(Some(&json!(null)), 10), 10);
        assert_eq!(coerce_positive_int(Some(&json!(true)), 10), 1);
        assert_eq!(coerce_positive_int(Some(&json!(false)), 10), 10); // 0 -> default
        assert_eq!(coerce_positive_int(Some(&json!(3.7)), 10), 3); // int(3.7)=3
        assert_eq!(coerce_positive_int(Some(&json!("3.7")), 10), 10); // int("3.7") fails
    }

    #[test]
    fn coerce_non_negative_allows_zero() {
        use serde_json::json;
        assert_eq!(coerce_non_negative_int(Some(&json!(0)), 5), 0);
        assert_eq!(coerce_non_negative_int(Some(&json!(-1)), 5), 5);
        assert_eq!(coerce_non_negative_int(Some(&json!(3)), 5), 3);
        assert_eq!(coerce_non_negative_int(Some(&json!("0")), 5), 0);
        assert_eq!(coerce_non_negative_int(None, 5), 5);
    }

    #[test]
    fn get_spill_config_from_value_defaults() {
        use serde_json::json;
        let cfg = get_spill_config_from_value(&json!({}));
        assert_eq!(cfg, SpillConfig::default());
        let cfg2 = get_spill_config_from_value(&json!({"hooks": null}));
        assert_eq!(cfg2, SpillConfig::default());
        let cfg3 = get_spill_config_from_value(&json!({"hooks": {"output_spill": null}}));
        assert_eq!(cfg3, SpillConfig::default());
    }

    #[test]
    fn get_spill_config_from_value_parses() {
        use serde_json::json;
        let raw = json!({
            "hooks": {
                "output_spill": {
                    "enabled": false,
                    "max_chars": 2000,
                    "preview_head": 10,
                    "preview_tail": 20,
                    "directory": "/tmp/my_spill"
                }
            }
        });
        let cfg = get_spill_config_from_value(&raw);
        assert!(!cfg.enabled);
        assert_eq!(cfg.max_chars, 2000);
        assert_eq!(cfg.preview_head, 10);
        assert_eq!(cfg.preview_tail, 20);
        assert_eq!(cfg.directory.as_deref(), Some("/tmp/my_spill"));

        // enabled string "false" is truthy in Python bool("false") => true
        let raw2 = json!({
            "hooks": {
                "output_spill": {
                    "enabled": "false"
                }
            }
        });
        let cfg2 = get_spill_config_from_value(&raw2);
        assert!(cfg2.enabled, "bool('false') is true in Python");

        // non-string directory -> None
        let raw3 = json!({
            "hooks": {
                "output_spill": {
                    "directory": 123
                }
            }
        });
        let cfg3 = get_spill_config_from_value(&raw3);
        assert_eq!(cfg3.directory, None);

        // invalid max_chars -> default
        let raw4 = json!({
            "hooks": {
                "output_spill": {
                    "max_chars": -5
                }
            }
        });
        let cfg4 = get_spill_config_from_value(&raw4);
        assert_eq!(cfg4.max_chars, DEFAULT_MAX_CHARS);

        // zero preview_head allowed via non_negative
        let raw5 = json!({
            "hooks": {
                "output_spill": {
                    "preview_head": 0,
                    "preview_tail": 0
                }
            }
        });
        let cfg5 = get_spill_config_from_value(&raw5);
        assert_eq!(cfg5.preview_head, 0);
        assert_eq!(cfg5.preview_tail, 0);
    }

    #[test]
    fn resolve_spill_dir_defaults_and_override() {
        let tmp = tmp_home("resolve");
        // Use override
        let p = resolve_spill_dir(Some("/tmp/custom"), Some("sess123"));
        assert!(p.starts_with("/tmp/custom"));
        assert!(p.ends_with("sess123"));
        // Default uses hermes home — just check it ends with hook_outputs/no-session when no override
        let home = get_hermes_home();
        let p2 = resolve_spill_dir(None, None);
        assert_eq!(p2, home.join("hook_outputs").join("no-session"));
        // Defensive sanitization
        let p3 = resolve_spill_dir(None, Some("a/b\\c..d"));
        // "/" -> "_", "\" -> "_", ".." -> "_"
        // "a/b\\c..d" -> "a_b_c..d" after slash/backslash, then ".." -> "_" => "a_b_c_d"
        // Actually "a/b\\c..d" -> replace "/" => "a_b\\c..d" -> replace "\\" => "a_b_c..d" -> replace ".." => "a_b_c_d"
        assert!(p3.ends_with("a_b_c_d"), "got {}", p3.display());
        let _ = fs::remove_dir_all(tmp);
    }

    #[test]
    fn resolve_spill_dir_empty_override_falls_back() {
        let home = get_hermes_home();
        let p = resolve_spill_dir(Some(""), Some("s"));
        assert_eq!(p, home.join("hook_outputs").join("s"));
        let p2 = resolve_spill_dir(Some("   "), Some("s"));
        assert_eq!(p2, home.join("hook_outputs").join("s"));
    }

    #[test]
    fn build_preview_formats() {
        let text = "a".repeat(10_000 + 5);
        let preview = build_preview(&text, 500, 500, Some("/tmp/hook_outputs/sess/abc.txt"), "hook");
        assert!(preview.contains("[hook output truncated — 10,005 chars; full content saved to /tmp/hook_outputs/sess/abc.txt]"));
        assert!(preview.contains("--- head ---"));
        assert!(preview.contains("--- tail ---"));
        // head chunk is first 500 chars
        let head_part = preview.split("--- head ---\n").nth(1).unwrap().split("\n--- tail ---").next().unwrap();
        assert_eq!(head_part.chars().count(), 500);
        assert_eq!(head_part, "a".repeat(500));

        // Without saved_path
        let preview2 = build_preview(&text, 500, 500, None, "shell hook");
        assert!(preview2.contains("unavailable — spill write failed]"));
        assert!(preview2.contains("[shell hook output truncated"));

        // head 0 -> no head section
        let preview3 = build_preview(&text, 0, 10, Some("/tmp/p"), "hook");
        assert!(!preview3.contains("--- head ---"));
        assert!(preview3.contains("--- tail ---"));

        // tail 0 -> no tail
        let preview4 = build_preview(&text, 10, 0, Some("/tmp/p"), "hook");
        assert!(preview4.contains("--- head ---"));
        assert!(!preview4.contains("--- tail ---"));

        // total <= head -> only head, no tail even if tail>0 (Python condition total>head)
        let short = "hello";
        let preview5 = build_preview(short, 10, 5, Some("/tmp/p"), "hook");
        assert!(preview5.contains("--- head ---"));
        assert!(!preview5.contains("--- tail ---"));
    }

    #[test]
    fn build_preview_comma_formatting() {
        let text = "x".repeat(1_000);
        let p = build_preview(&text, 1, 1, Some("/tmp/p"), "hook");
        assert!(p.contains("1,000 chars"));
        let text2 = "x".repeat(10_000);
        let p2 = build_preview(&text2, 1, 1, Some("/tmp/p"), "hook");
        assert!(p2.contains("10,000 chars"));
    }

    #[test]
    fn spill_if_oversized_under_cap_unchanged() {
        let cfg = SpillConfig {
            max_chars: 100,
            preview_head: 10,
            preview_tail: 10,
            enabled: true,
            directory: None,
        };
        let text = "a".repeat(50);
        let out = spill_if_oversized(&text, Some("sess1"), "hook", Some(&cfg));
        assert_eq!(out, text);
        // exactly at cap
        let text2 = "a".repeat(100);
        let out2 = spill_if_oversized(&text2, Some("sess1"), "hook", Some(&cfg));
        assert_eq!(out2, text2);
    }

    #[test]
    fn spill_if_oversized_disabled_returns_unchanged() {
        let cfg = SpillConfig {
            max_chars: 10,
            enabled: false,
            ..Default::default()
        };
        let text = "a".repeat(100);
        let out = spill_if_oversized(&text, Some("sess1"), "hook", Some(&cfg));
        assert_eq!(out, text);
    }

    #[test]
    fn spill_if_oversized_spills_and_previews() {
        let tmp = tmp_home("spill");
        let dir = tmp.join("hook_outputs");
        let cfg = SpillConfig {
            max_chars: 10,
            preview_head: 3,
            preview_tail: 2,
            enabled: true,
            directory: Some(dir.to_string_lossy().to_string()),
        };
        let text = "abcdefghijklmno"; // 15 chars > 10
        let out = spill_if_oversized(text, Some("mysess"), "hook", Some(&cfg));
        assert!(out.contains("[hook output truncated — 15 chars; full content saved to"));
        assert!(out.contains("--- head ---"));
        assert!(out.contains("abc")); // head 3
        assert!(out.contains("--- tail ---"));
        assert!(out.contains("no")); // tail 2 = "no"
        // Verify file was written under spill dir
        let spill_dir = dir.join("mysess");
        assert!(spill_dir.is_dir());
        let entries: Vec<_> = fs::read_dir(&spill_dir).unwrap().collect();
        assert_eq!(entries.len(), 1);
        let path = entries[0].as_ref().unwrap().path();
        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content, format!("{text}\n")); // trailing newline added
        // Clean
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn spill_if_oversized_adds_newline() {
        let tmp = tmp_home("spill-nl");
        let dir = tmp.join("hook_outputs");
        let cfg = SpillConfig {
            max_chars: 5,
            preview_head: 2,
            preview_tail: 2,
            enabled: true,
            directory: Some(dir.to_string_lossy().to_string()),
        };
        let text_no_nl = "abcdefghij";
        let _ = spill_if_oversized(text_no_nl, Some("s"), "hook", Some(&cfg));
        let spill_dir = dir.join("s");
        let entry = fs::read_dir(&spill_dir).unwrap().next().unwrap().unwrap();
        let content = fs::read_to_string(entry.path()).unwrap();
        assert!(content.ends_with('\n'));
        assert_eq!(content, format!("{text_no_nl}\n"));

        // Already has newline
        let text_with_nl = "abcdefghij\n";
        let cfg2 = SpillConfig {
            directory: Some(dir.join("s2").to_string_lossy().to_string()),
            ..cfg.clone()
        };
        // Need fresh dir for second
        let dir2 = dir.join("s2");
        let _ = fs::create_dir_all(&dir2);
        let cfg2 = SpillConfig {
            max_chars: 5,
            preview_head: 2,
            preview_tail: 2,
            enabled: true,
            directory: Some(dir.to_string_lossy().to_string()),
        };
        let _ = spill_if_oversized(text_with_nl, Some("s2"), "hook", Some(&cfg2));
        let spill_dir2 = dir.join("s2");
        let entries: Vec<_> = fs::read_dir(&spill_dir2).unwrap().collect();
        // Find file that contains original with single newline (not double)
        let mut found = false;
        for e in entries {
            let p = e.unwrap().path();
            let c = fs::read_to_string(&p).unwrap();
            if c == text_with_nl {
                found = true;
            }
        }
        assert!(found, "should write with single trailing newline");
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn spill_if_oversized_fallback_on_io_error() {
        // Use a directory override that is a file, not a dir, to force ensure_spill_dir failure
        let tmp = tmp_home("spill-fail");
        let file_path = tmp.join("not_a_dir");
        fs::write(&file_path, "i am a file").unwrap();
        let cfg = SpillConfig {
            max_chars: 5,
            preview_head: 2,
            preview_tail: 2,
            enabled: true,
            directory: Some(file_path.to_string_lossy().to_string()),
        };
        let text = "abcdefghij";
        let out = spill_if_oversized(text, Some("s"), "hook", Some(&cfg));
        // Should still return preview but with unavailable marker, not panic
        assert!(out.contains("unavailable — spill write failed"));
        assert!(out.contains("--- head ---"));
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn spill_if_oversized_session_sanitization() {
        let tmp = tmp_home("spill-sess-sanitize");
        let dir = tmp.join("hook_outputs");
        let cfg = SpillConfig {
            max_chars: 5,
            enabled: true,
            preview_head: 1,
            preview_tail: 1,
            directory: Some(dir.to_string_lossy().to_string()),
        };
        let text = "abcdefghij";
        let out = spill_if_oversized(text, Some("a/b\\c..d"), "hook", Some(&cfg));
        assert!(out.contains("saved to"));
        // The dir should be sanitized: a/b\c..d -> a_b_c_d
        let expected = dir.join("a_b_c_d");
        assert!(expected.is_dir(), "expected sanitized dir {}", expected.display());
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn spill_if_oversized_optional_none_returns_empty() {
        let cfg = SpillConfig::default();
        let out = spill_if_oversized_optional(None, None, "hook", Some(&cfg));
        assert_eq!(out, "");
    }

    #[test]
    fn get_spill_config_yaml_scanner() {
        let yaml = r#"
hooks:
  output_spill:
    enabled: false
    max_chars: 12345
    preview_head: 11
    preview_tail: 22
    directory: /tmp/custom_spill
"#;
        let cfg = parse_spill_config_from_yaml_text(yaml);
        assert!(!cfg.enabled);
        assert_eq!(cfg.max_chars, 12345);
        assert_eq!(cfg.preview_head, 11);
        assert_eq!(cfg.preview_tail, 22);
        assert_eq!(cfg.directory.as_deref(), Some("/tmp/custom_spill"));

        // Missing hooks -> defaults
        let cfg2 = parse_spill_config_from_yaml_text("model: foo\n");
        assert_eq!(cfg2, SpillConfig::default());

        // Invalid max_chars (negative) -> default preserved
        let yaml2 = "hooks:\n  output_spill:\n    max_chars: -1\n";
        let cfg3 = parse_spill_config_from_yaml_text(yaml2);
        assert_eq!(cfg3.max_chars, DEFAULT_MAX_CHARS);
    }

    #[test]
    fn get_spill_config_missing_file_returns_default() {
        // Point HERMES_HOME to empty tmp
        let tmp = tmp_home("missing-config");
        let prev = std::env::var("HERMES_HOME").ok();
        unsafe { std::env::set_var("HERMES_HOME", tmp.to_string_lossy().to_string()) };
        let prev_gray = std::env::var("GRAY_HOME").ok();
        unsafe { std::env::remove_var("GRAY_HOME") };
        let cfg = get_spill_config();
        assert_eq!(cfg, SpillConfig::default());
        // Restore
        if let Some(p) = prev { unsafe { std::env::set_var("HERMES_HOME", p) }; } else { unsafe { std::env::remove_var("HERMES_HOME") }; }
        if let Some(p) = prev_gray { unsafe { std::env::set_var("GRAY_HOME", p) }; }
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn ensure_spill_dir_private_and_write() {
        let tmp = tmp_home("ensure");
        let dir = tmp.join("a/b/c");
        let res = ensure_spill_dir(&dir, true).unwrap();
        assert!(res.is_dir());
        assert!(dir.is_dir());
        // symlink check: create a file and try to ensure dir where file exists should fail
        let file = tmp.join("file");
        fs::write(&file, "x").unwrap();
        let err = ensure_spill_dir(&file, true).unwrap_err();
        assert!(err.to_string().contains("not a directory"));
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn write_text_exclusive_exclusive() {
        let tmp = tmp_home("write-excl");
        let p = tmp.join("out.txt");
        write_text_exclusive(&p, "hello\n", true).unwrap();
        assert_eq!(fs::read_to_string(&p).unwrap(), "hello\n");
        // Second write without overwrite should fail
        let err = write_text_exclusive(&p, "again\n", true).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
        let _ = fs::remove_dir_all(&tmp);
    }
}
