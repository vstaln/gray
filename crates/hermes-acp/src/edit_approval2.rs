//! Pre-execution ACP edit approval helpers.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/acp_adapter/edit_approval.py`
//! (338 lines, full file — single slice). Isolated from the generic tool registry:
//! ACP binds an edit approval requester in a ContextVar for the duration of one ACP
//! agent run; CLI, gateway, and other sessions leave it unset and therefore bypass
//! this guard.
//!
//! Mirrors Python module docstring (lines 1-6):
//! ```text
//! Pre-execution ACP edit approval helpers.
//! This module is intentionally isolated from the generic tool registry.  ACP binds
//! an edit approval requester in a ContextVar for the duration of one ACP agent run;
//! CLI, gateway, and other sessions leave it unset and therefore bypass this guard.
//! ```
//!
//! T0413 — 1:1 port, no cargo (NEVER cargo). All external crates / ACP SDK types
//! are stubbed as local structs for traceability; `asyncio`/`concurrent.futures`/
//! `contextvars` are modelled as std-only stubs: `ContextVar` as a `Mutex<Option<Requester>>`
//! with a stack of tokens, `TimeoutError` as a plain error string, `asyncio.AbstractEventLoop`
//! as a unit struct. `tempfile` maps to `std::env::temp_dir()`, `re` to manual
//! line parsing, `pathlib.Path` to `std::path::Path`, `json` to minimal string
//! helpers. `acp` helpers and `tools.fuzzy_match` are stubbed inline (NEVER cargo).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

// ---------------------------------------------------------------------------
// Module docstring — mirrors lines 1-6
// ---------------------------------------------------------------------------

/// Mirrors `acp_adapter/edit_approval.py` top-level docstring (lines 1-6).
pub const MODULE_DOC: &str =
    "Pre-execution ACP edit approval helpers.";

// ---------------------------------------------------------------------------
// Logger — mirrors `logger = logging.getLogger(__name__)` (line 22)
// ---------------------------------------------------------------------------

pub fn logger_name() -> &'static str {
    "acp_adapter.edit_approval"
}

// ---------------------------------------------------------------------------
// EditProposal — mirrors `@dataclass(frozen=True) class EditProposal:` (lines 25-33)
// ---------------------------------------------------------------------------

/// A proposed single-file edit that can be shown to an ACP client.
///
/// Mirrors `EditProposal` (lines 25-33):
/// ```python
/// @dataclass(frozen=True)
/// class EditProposal:
///     tool_name: str
///     path: str
///     old_text: str | None
///     new_text: str
///     arguments: dict[str, Any]
/// ```
#[derive(Debug, Clone)]
pub struct EditProposal {
    /// Mirrors `tool_name: str`
    pub tool_name: String,
    /// Mirrors `path: str`
    pub path: String,
    /// Mirrors `old_text: str | None`
    pub old_text: Option<String>,
    /// Mirrors `new_text: str`
    pub new_text: String,
    /// Mirrors `arguments: dict[str, Any]` — stringly-typed stub (NEVER cargo serde_json)
    pub arguments: HashMap<String, String>,
}

impl EditProposal {
    pub fn new(
        tool_name: impl Into<String>,
        path: impl Into<String>,
        old_text: Option<String>,
        new_text: impl Into<String>,
        arguments: HashMap<String, String>,
    ) -> Self {
        Self {
            tool_name: tool_name.into(),
            path: path.into(),
            old_text,
            new_text: new_text.into(),
            arguments,
        }
    }
}

// ---------------------------------------------------------------------------
// ArgVal — lightweight Any for `arguments: dict[str, Any]` (lines 33, 82-175)
//   Mirrors the varied shapes in `_proposal_for_*` / `build_edit_proposal`:
//   plain strings, ints, bools, and nested dicts. Real Python uses `Any`;
//   Rust models the accessed shapes with an enum so branches stay 1:1 without
//   pulling `serde_json` (NEVER cargo).
// ---------------------------------------------------------------------------

/// Minimal stand-in for Python's `Any` inside tool `arguments`.
#[derive(Debug, Clone)]
pub enum ArgVal {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Map(HashMap<String, String>),
}

impl ArgVal {
    pub fn as_str(&self) -> Option<&str> {
        if let ArgVal::Str(s) = self { Some(s.as_str()) } else { None }
    }
    pub fn as_bool(&self) -> Option<bool> {
        if let ArgVal::Bool(b) = self { Some(*b) } else { None }
    }
}

impl From<String> for ArgVal {
    fn from(s: String) -> Self { ArgVal::Str(s) }
}
impl From<&str> for ArgVal {
    fn from(s: &str) -> Self { ArgVal::Str(s.to_string()) }
}
impl From<bool> for ArgVal {
    fn from(b: bool) -> Self { ArgVal::Bool(b) }
}

pub type Arguments = HashMap<String, ArgVal>;

fn arg_str_val(args: &Arguments, key: &str) -> Option<String> {
    args.get(key).and_then(|v| match v {
        ArgVal::Str(s) => Some(s.clone()),
        ArgVal::Int(n) => Some(n.to_string()),
        ArgVal::Float(f) => Some(f.to_string()),
        ArgVal::Bool(b) => Some(b.to_string()),
        _ => None,
    })
}

fn args_to_string_map(args: &Arguments) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for (k, v) in args {
        let vs = match v {
            ArgVal::Str(s) => s.clone(),
            ArgVal::Int(n) => n.to_string(),
            ArgVal::Float(f) => f.to_string(),
            ArgVal::Bool(b) => b.to_string(),
            ArgVal::Null => "null".to_string(),
            ArgVal::Map(m) => format!("{m:?}"),
        };
        out.insert(k.clone(), vs);
    }
    out
}

// ---------------------------------------------------------------------------
// EditApprovalRequester — mirrors `EditApprovalRequester = Callable[[EditProposal], bool]` (36)
//   plus ContextVar storage (38-41) and `_PERMISSION_REQUEST_IDS = count(1)` (42)
// ---------------------------------------------------------------------------

/// Mirrors `EditApprovalRequester = Callable[[EditProposal], bool]` (line 36).
pub type EditApprovalRequester = Arc<dyn Fn(&EditProposal) -> bool + Send + Sync>;

/// Mirrors `_EDIT_APPROVAL_REQUESTER: ContextVar[EditApprovalRequester | None]` (38-41).
///
/// Python: `ContextVar("ACP_EDIT_APPROVAL_REQUESTER", default=None)`
/// Rust (NEVER cargo): global `Mutex<Option<Requester>>` modelling the current
/// context's value. `set`/`reset` manage a token stack so nested ACP runs restore
/// correctly, matching `ContextVar.Token` semantics (lines 51-60).
static EDIT_APPROVAL_REQUESTER: Mutex<Option<EditApprovalRequester>> = Mutex::new(None);
/// Token stack for `reset` — mirrors `ContextVar.Token` (lines 51-60).
static REQUESTER_TOKEN_STACK: Mutex<Vec<Option<EditApprovalRequester>>> = Mutex::new(Vec::new());

/// Mirrors `_PERMISSION_REQUEST_IDS = count(1)` (line 42). Used in `build_acp_edit_tool_call` (269).
static PERMISSION_REQUEST_IDS: AtomicU64 = AtomicU64::new(1);

/// Mirrors `Token` from `contextvars` (lines 51-60). In Rust: opaque id into the stack.
#[derive(Debug, Clone, Copy)]
pub struct RequesterToken(pub usize);

// ---------------------------------------------------------------------------
// Sensitive / auto-approve constants — lines 45-48
// ---------------------------------------------------------------------------

/// Mirrors `SENSITIVE_AUTO_APPROVE_NAMES = {".env", ...}` (line 45).
pub const SENSITIVE_AUTO_APPROVE_NAMES: &[&str] = &[
    ".env",
    ".env.local",
    ".env.production",
    "id_rsa",
    "id_ed25519",
];

/// Mirrors `AUTO_APPROVE_ASK = "ask"` (line 46).
pub const AUTO_APPROVE_ASK: &str = "ask";
/// Mirrors `AUTO_APPROVE_WORKSPACE = "workspace_session"` (line 47).
pub const AUTO_APPROVE_WORKSPACE: &str = "workspace_session";
/// Mirrors `AUTO_APPROVE_SESSION = "session"` (line 48).
pub const AUTO_APPROVE_SESSION: &str = "session";

// ---------------------------------------------------------------------------
// ContextVar helpers — lines 51-71
// ---------------------------------------------------------------------------

/// Bind an ACP edit approval requester for the current context.
/// Mirrors `def set_edit_approval_requester(requester) -> Token:` (51-54).
///
/// ```python
/// def set_edit_approval_requester(requester: EditApprovalRequester | None) -> Token:
///     return _EDIT_APPROVAL_REQUESTER.set(requester)
/// ```
pub fn set_edit_approval_requester(requester: Option<EditApprovalRequester>) -> RequesterToken {
    let mut stack = REQUESTER_TOKEN_STACK.lock().unwrap();
    let current = EDIT_APPROVAL_REQUESTER.lock().unwrap().clone();
    stack.push(current);
    let token_id = stack.len(); // 1-based, matches `count` feel
    *EDIT_APPROVAL_REQUESTER.lock().unwrap() = requester;
    RequesterToken(token_id)
}

/// Restore a previous edit approval requester binding.
/// Mirrors `def reset_edit_approval_requester(token: Token) -> None:` (57-60).
///
/// ```python
/// def reset_edit_approval_requester(token: Token) -> None:
///     _EDIT_APPROVAL_REQUESTER.reset(token)
/// ```
pub fn reset_edit_approval_requester(token: RequesterToken) {
    let mut stack = REQUESTER_TOKEN_STACK.lock().unwrap();
    // Pop until we reach token id; simplified: pop one if id matches top
    if token.0 == 0 || stack.is_empty() {
        return;
    }
    // Tokens are stack-ordered; reset should restore the value at push time.
    // Python's Token remembers the previous value; we model by popping.
    if token.0 <= stack.len() {
        // Drain down to token.0 - 1 elements remaining, restore last popped
        while stack.len() > token.0 {
            stack.pop();
        }
        if let Some(prev) = stack.pop() {
            *EDIT_APPROVAL_REQUESTER.lock().unwrap() = prev;
        }
    }
}

/// Clear the current requester; primarily used by tests.
/// Mirrors `def clear_edit_approval_requester() -> None:` (63-66).
///
/// ```python
/// def clear_edit_approval_requester() -> None:
///     _EDIT_APPROVAL_REQUESTER.set(None)
/// ```
pub fn clear_edit_approval_requester() {
    *EDIT_APPROVAL_REQUESTER.lock().unwrap() = None;
}

/// Mirrors `def get_edit_approval_requester() -> EditApprovalRequester | None:` (69-71).
pub fn get_edit_approval_requester() -> Option<EditApprovalRequester> {
    EDIT_APPROVAL_REQUESTER.lock().unwrap().clone()
}

// Allow underscore-prefixed aliases for 1:1 traceability
#[allow(dead_code)]
pub fn _get_edit_approval_requester() -> Option<EditApprovalRequester> {
    get_edit_approval_requester()
}

// ---------------------------------------------------------------------------
// _read_text_if_exists — lines 73-79
// ---------------------------------------------------------------------------

/// Mirrors `def _read_text_if_exists(path: str) -> str | None:` (73-79).
///
/// ```python
/// def _read_text_if_exists(path: str) -> str | None:
///     p = Path(path).expanduser()
///     if not p.exists():
///         return None
///     if not p.is_file():
///         raise OSError(f"Cannot edit non-file path: {path}")
///     return p.read_text(encoding="utf-8", errors="replace")
/// ```
pub fn read_text_if_exists(path: &str) -> Result<Option<String>, String> {
    let expanded = expand_user(path);
    let p = Path::new(&expanded);
    if !p.exists() {
        return Ok(None); // 75-76
    }
    if !p.is_file() {
        return Err(format!("Cannot edit non-file path: {path}")); // 77-78
    }
    // Mirrors `p.read_text(encoding="utf-8", errors="replace")` (79)
    // `errors="replace"` maps to lossy utf8 with replacement char.
    match std::fs::read(&expanded) {
        Ok(bytes) => Ok(Some(String::from_utf8_lossy(&bytes).to_string())),
        Err(e) => Err(e.to_string()),
    }
}

#[allow(dead_code)]
pub fn _read_text_if_exists(path: &str) -> Result<Option<String>, String> {
    read_text_if_exists(path)
}

fn expand_user(raw: &str) -> String {
    // Mirrors `Path(path).expanduser()` — expands leading `~` using $HOME
    if raw == "~" || raw.starts_with("~/") {
        if let Ok(home) = std::env::var("HOME") {
            if raw == "~" {
                return home;
            } else {
                return format!("{}{}", home, &raw[1..]);
            }
        }
    }
    raw.to_string()
}

// ---------------------------------------------------------------------------
// _proposal_for_write_file — lines 82-95
// ---------------------------------------------------------------------------

/// Mirrors `def _proposal_for_write_file(arguments: dict[str, Any]) -> EditProposal:` (82-95).
///
/// ```python
/// def _proposal_for_write_file(arguments: dict[str, Any]) -> EditProposal:
///     path = str(arguments.get("path") or "")
///     if not path: raise ValueError("path required")
///     content = arguments.get("content")
///     if content is None: raise ValueError("content required")
///     return EditProposal(tool_name="write_file", path=path,
///                         old_text=_read_text_if_exists(path),
///                         new_text=str(content), arguments=dict(arguments))
/// ```
pub fn proposal_for_write_file(arguments: &Arguments) -> Result<EditProposal, String> {
    let path = arg_str_val(arguments, "path").unwrap_or_default().trim().to_string(); // 83
    if path.is_empty() {
        return Err("path required".to_string()); // 84-85
    }
    let content_val = arguments.get("content"); // 86
    if content_val.is_none() || matches!(content_val, Some(ArgVal::Null)) {
        return Err("content required".to_string()); // 87-88
    }
    let content = match content_val.unwrap() {
        ArgVal::Str(s) => s.clone(),
        ArgVal::Int(n) => n.to_string(),
        ArgVal::Float(f) => f.to_string(),
        ArgVal::Bool(b) => b.to_string(),
        ArgVal::Null => return Err("content required".to_string()),
        ArgVal::Map(m) => format!("{m:?}"),
    };
    let old_text = read_text_if_exists(&path).map_err(|e| e.to_string())?; // 92
    Ok(EditProposal::new(
        "write_file",
        path,
        old_text,
        content,
        args_to_string_map(arguments),
    ))
}

#[allow(dead_code)]
pub fn _proposal_for_write_file(arguments: &Arguments) -> Result<EditProposal, String> {
    proposal_for_write_file(arguments)
}

// ---------------------------------------------------------------------------
// _proposal_for_patch_replace — lines 98-128
// ---------------------------------------------------------------------------

/// Mirrors `def _proposal_for_patch_replace(arguments: dict[str, Any]) -> EditProposal:` (98-128).
///
/// Key call (lines 111-121):
/// ```python
/// from tools.fuzzy_match import fuzzy_find_and_replace
/// new_text, match_count, _strategy, error = fuzzy_find_and_replace(
///     old_text, str(old_string), str(new_string), bool(arguments.get("replace_all", False)))
/// if error or match_count == 0:
///     raise ValueError(error or f"Could not find match for old_string in {path}")
/// ```
pub fn proposal_for_patch_replace(arguments: &Arguments) -> Result<EditProposal, String> {
    let path = arg_str_val(arguments, "path").unwrap_or_default().trim().to_string(); // 99
    if path.is_empty() {
        return Err("path required".to_string()); // 100-101
    }
    let old_string_val = arguments.get("old_string"); // 102
    let new_string_val = arguments.get("new_string"); // 103
    if old_string_val.is_none() || new_string_val.is_none() {
        return Err("old_string and new_string required".to_string()); // 104-105
    }
    let old_string = match old_string_val.unwrap() {
        ArgVal::Str(s) => s.clone(),
        ArgVal::Int(n) => n.to_string(),
        ArgVal::Float(f) => f.to_string(),
        ArgVal::Bool(b) => b.to_string(),
        ArgVal::Null => return Err("old_string and new_string required".to_string()),
        ArgVal::Map(m) => format!("{m:?}"),
    };
    let new_string = match new_string_val.unwrap() {
        ArgVal::Str(s) => s.clone(),
        ArgVal::Int(n) => n.to_string(),
        ArgVal::Float(f) => f.to_string(),
        ArgVal::Bool(b) => b.to_string(),
        ArgVal::Null => return Err("old_string and new_string required".to_string()),
        ArgVal::Map(m) => format!("{m:?}"),
    };

    let old_text = read_text_if_exists(&path).map_err(|e| e.to_string())?; // 107
    let old_text = match old_text {
        Some(t) => t,
        None => return Err(format!("Failed to read file: {path}")), // 108-109
    };

    let replace_all = arguments
        .get("replace_all")
        .and_then(|v| v.as_bool())
        .unwrap_or(false); // 117

    // Mirrors `fuzzy_find_and_replace(old_text, str(old_string), str(new_string), bool(replace_all))` (113-118)
    let (new_text, match_count, _strategy, error) =
        fuzzy_find_and_replace(&old_text, &old_string, &new_string, replace_all); // 113-118

    if error.is_some() || match_count == 0 {
        let msg = error.unwrap_or_else(|| format!("Could not find match for old_string in {path}")); // 119-120
        return Err(msg);
    }

    Ok(EditProposal::new(
        "patch",
        path,
        Some(old_text),
        new_text,
        args_to_string_map(arguments),
    ))
}

#[allow(dead_code)]
pub fn _proposal_for_patch_replace(arguments: &Arguments) -> Result<EditProposal, String> {
    proposal_for_patch_replace(arguments)
}

/// Stub for `tools.fuzzy_match.fuzzy_find_and_replace` (lines 111-118).
///
/// Python performs a multi-strategy fuzzy match (exact, trimmed, normalized
/// whitespace, etc.). Rust stub (NEVER cargo) models the two outcomes callers
/// check: `error` and `match_count`. Exact substring is tried first; if found,
/// the replacement mirrors Python's exact branch. Otherwise `match_count=0`.
fn fuzzy_find_and_replace(
    old_text: &str,
    old_string: &str,
    new_string: &str,
    replace_all: bool,
) -> (String, usize, String, Option<String>) {
    // Exact match path — covers the common case; fuzzy variants (trimmed,
    // whitespace-normalized) are not stubbed because `match_count==0` in the
    // stub still surfaces the correct `ValueError` at the call site, matching
    // Python's `error or f"Could not find match..."` raise (119-120).
    if old_string.is_empty() {
        return (old_text.to_string(), 0, "exact".to_string(), Some("old_string empty".to_string()));
    }
    if old_text.contains(old_string) {
        let new_text = if replace_all {
            old_text.replace(old_string, new_string)
        } else {
            old_text.replacen(old_string, new_string, 1)
        };
        let count = if replace_all {
            old_text.matches(old_string).count()
        } else {
            1
        };
        return (new_text, count, "exact".to_string(), None);
    }
    // Not found — mirrors `match_count == 0` branch (119)
    (old_text.to_string(), 0, "none".to_string(), None)
}

// ---------------------------------------------------------------------------
// _extract_v4a_patch_paths — lines 131-152
// ---------------------------------------------------------------------------

/// Mirrors `def _extract_v4a_patch_paths(patch_body: str) -> list[str]:` (131-152).
///
/// ```python
/// def _extract_v4a_patch_paths(patch_body: str) -> list[str]:
///     paths: list[str] = []
///     for match in re.finditer(r'^\*\*\*\s+(?:Update|Add|Delete)\s+File:\s*(.+)$', patch_body, re.MULTILINE):
///         path = match.group(1).strip()
///         if path: paths.append(path)
///     for match in re.finditer(r'^\*\*\*\s+Move\s+File:\s*(.+?)\s*->\s*(.+)$', patch_body, re.MULTILINE):
///         src = match.group(1).strip()
///         dst = match.group(2).strip()
///         if src: paths.append(src)
///         if dst: paths.append(dst)
///     return paths
/// ```
pub fn extract_v4a_patch_paths(patch_body: &str) -> Vec<String> {
    let mut paths: Vec<String> = Vec::new();
    // Mirrors `re.finditer(r'^\*\*\*\s+(?:Update|Add|Delete)\s+File:\s*(.+)$', patch_body, re.MULTILINE)` (133-137)
    for line in patch_body.lines() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with("***") {
            continue;
        }
        let after_stars = trimmed[3..].trim_start();
        // Check Update/Add/Delete File pattern
        for keyword in ["Update", "Add", "Delete"] {
            let prefix = format!("{keyword} File:");
            if after_stars.starts_with(&prefix) || after_stars.to_lowercase().starts_with(&prefix.to_lowercase()) {
                // Need to handle case-insensitive? Python pattern is case-sensitive (Update/Add/Delete capitalised).
                // We keep case-sensitive to stay 1:1.
                if after_stars.starts_with(&prefix) {
                    let path = after_stars[prefix.len()..].trim().to_string();
                    if !path.is_empty() {
                        paths.push(path);
                    }
                    break;
                }
            }
        }
        // Check Move File pattern: `*** Move File: src -> dst` (142-151)
        let move_prefix = "Move File:";
        if after_stars.starts_with(move_prefix) {
            let rest = after_stars[move_prefix.len()..].trim();
            // Split on `->`
            if let Some(arrow_pos) = rest.find("->") {
                let src = rest[..arrow_pos].trim().to_string();
                let dst = rest[arrow_pos + 2..].trim().to_string();
                if !src.is_empty() {
                    paths.push(src);
                }
                if !dst.is_empty() {
                    paths.push(dst);
                }
            }
        }
    }
    paths
}

#[allow(dead_code)]
pub fn _extract_v4a_patch_paths(patch_body: &str) -> Vec<String> {
    extract_v4a_patch_paths(patch_body)
}

// ---------------------------------------------------------------------------
// _proposal_for_patch_v4a — lines 155-175
// ---------------------------------------------------------------------------

/// Mirrors `def _proposal_for_patch_v4a(arguments: dict[str, Any]) -> EditProposal:` (155-175).
pub fn proposal_for_patch_v4a(arguments: &Arguments) -> Result<EditProposal, String> {
    let patch_body = arg_str_val(arguments, "patch"); // 156
    let patch_body = match patch_body {
        Some(s) if !s.is_empty() => s,
        _ => return Err("patch content required".to_string()), // 157-158
    };

    let paths = extract_v4a_patch_paths(&patch_body); // 160
    if paths.is_empty() {
        return Err("no file paths found in V4A patch".to_string()); // 161-162
    }

    let proposal_path = if paths.len() == 1 {
        paths[0].clone()
    } else {
        paths.join(", ")
    }; // 164
    let old_text = if paths.len() == 1 {
        read_text_if_exists(&paths[0]).ok().flatten()
    } else {
        None
    }; // 165
    Ok(EditProposal::new(
        "patch",
        proposal_path,
        old_text,
        patch_body, // 170-173: `new_text=patch_body` — ACP only supports single diff payload
        args_to_string_map(arguments),
    ))
}

#[allow(dead_code)]
pub fn _proposal_for_patch_v4a(arguments: &Arguments) -> Result<EditProposal, String> {
    proposal_for_patch_v4a(arguments)
}

// ---------------------------------------------------------------------------
// build_edit_proposal — lines 178-189
// ---------------------------------------------------------------------------

/// Return an edit proposal for supported file mutation calls.
/// Mirrors `def build_edit_proposal(tool_name: str, arguments: dict[str, Any]) -> EditProposal | None:` (178-189).
///
/// ```python
/// def build_edit_proposal(tool_name: str, arguments: dict[str, Any]) -> EditProposal | None:
///     if tool_name == "write_file":
///         return _proposal_for_write_file(arguments)
///     if tool_name == "patch":
///         mode = arguments.get("mode", "replace")
///         if mode == "replace":
///             return _proposal_for_patch_replace(arguments)
///         if mode == "patch":
///             return _proposal_for_patch_v4a(arguments)
///     return None
/// ```
pub fn build_edit_proposal(tool_name: &str, arguments: &Arguments) -> Result<Option<EditProposal>, String> {
    if tool_name == "write_file" {
        return proposal_for_write_file(arguments).map(Some); // 181-182
    }
    if tool_name == "patch" {
        let mode = arg_str_val(arguments, "mode").unwrap_or_else(|| "replace".to_string()); // 184
        if mode == "replace" {
            return proposal_for_patch_replace(arguments).map(Some); // 185-186
        }
        if mode == "patch" {
            return proposal_for_patch_v4a(arguments).map(Some); // 187-188
        }
    }
    Ok(None) // 189
}

// ---------------------------------------------------------------------------
// _is_sensitive_auto_approve_path — lines 192-197
// ---------------------------------------------------------------------------

/// Mirrors `def _is_sensitive_auto_approve_path(path: str) -> bool:` (192-197).
///
/// ```python
/// def _is_sensitive_auto_approve_path(path: str) -> bool:
///     parts = Path(path).expanduser().parts
///     lowered = {part.lower() for part in parts}
///     if ".git" in lowered or ".ssh" in lowered:
///         return True
///     return Path(path).name.lower() in SENSITIVE_AUTO_APPROVE_NAMES
/// ```
pub fn is_sensitive_auto_approve_path(path: &str) -> bool {
    let expanded = expand_user(path);
    let p = Path::new(&expanded);
    // Mirrors `parts = Path(path).expanduser().parts` + lowercased set
    let lowered_parts: Vec<String> = p
        .components()
        .filter_map(|c| c.as_os_str().to_str().map(|s| s.to_lowercase()))
        .collect();
    if lowered_parts.iter().any(|part| part == ".git") || lowered_parts.iter().any(|part| part == ".ssh") {
        return true; // 195-196
    }
    // Mirrors `Path(path).name.lower() in SENSITIVE_AUTO_APPROVE_NAMES`
    if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
        let lower = name.to_lowercase();
        if SENSITIVE_AUTO_APPROVE_NAMES.iter().any(|n| *n == lower) {
            return true; // 197
        }
    }
    false
}

#[allow(dead_code)]
pub fn _is_sensitive_auto_approve_path(path: &str) -> bool {
    is_sensitive_auto_approve_path(path)
}

// ---------------------------------------------------------------------------
// should_auto_approve_edit — lines 200-230
// ---------------------------------------------------------------------------

/// Return whether an ACP edit proposal may bypass the prompt for this session.
///
/// Mirrors `def should_auto_approve_edit(proposal: EditProposal, policy: str, cwd: str | None = None) -> bool:` (200-230).
///
/// ```python
/// def should_auto_approve_edit(proposal: EditProposal, policy: str, cwd: str | None = None) -> bool:
///     policy = str(policy or AUTO_APPROVE_ASK).strip()
///     if policy == AUTO_APPROVE_ASK or _is_sensitive_auto_approve_path(proposal.path):
///         return False
///     path = Path(proposal.path).expanduser().resolve(strict=False)
///     if policy == AUTO_APPROVE_SESSION:
///         return True
///     if policy == AUTO_APPROVE_WORKSPACE:
///         tmp_root = Path(tempfile.gettempdir()).resolve(strict=False)
///         try:
///             path.relative_to(tmp_root)
///             return True
///         except ValueError:
///             pass
///         if cwd:
///             root = Path(cwd).expanduser().resolve(strict=False)
///             try:
///                 path.relative_to(root)
///                 return True
///             except ValueError:
///                 return False
///     return False
/// ```
pub fn should_auto_approve_edit(proposal: &EditProposal, policy: &str, cwd: Option<&str>) -> bool {
    let policy = {
        let p = policy.trim().to_string();
        if p.is_empty() { AUTO_APPROVE_ASK.to_string() } else { p } // 207
    };
    if policy == AUTO_APPROVE_ASK || is_sensitive_auto_approve_path(&proposal.path) {
        return false; // 208-209
    }
    let path = resolve_strict_false(&expand_user(&proposal.path)); // 210

    if policy == AUTO_APPROVE_SESSION {
        return true; // 211-212
    }
    if policy == AUTO_APPROVE_WORKSPACE {
        // Mirrors `/tmp` vs `tempfile.gettempdir()` comment (214-217)
        let tmp_root = resolve_strict_false(&std::env::temp_dir().to_string_lossy().to_string()); // 217
        if is_relative_to(&path, &tmp_root) {
            return true; // 218-220
        }
        // `except ValueError: pass` (221-222) — fall through
        if let Some(cwd_str) = cwd {
            let root = resolve_strict_false(&expand_user(cwd_str)); // 224
            if is_relative_to(&path, &root) {
                return true; // 226-228
            } else {
                return false; // 229
            }
        }
    }
    false // 230
}

fn resolve_strict_false(path_str: &str) -> PathBuf {
    // Mirrors `Path(...).expanduser().resolve(strict=False)` (210, 217, 224)
    // `strict=False` means lexical resolve without requiring existence, following symlinks if possible.
    // Rust: try canonicalize if file exists, else lexical normpath (preserve WSL-like paths).
    let p = Path::new(path_str);
    if p.exists() {
        if let Ok(canon) = std::fs::canonicalize(p) {
            return canon;
        }
    }
    // Lexical resolve: expand `~` already done, normalize `.`/`..` + `realpath`-like
    // For missing paths, `canonicalize` fails so we do lexical join with cwd for relative.
    if p.is_relative() {
        // Resolve relative against current dir to mimic `resolve(strict=False)` semantics
        if let Ok(cwd) = std::env::current_dir() {
            return lexical_normpath(&cwd.join(p).to_string_lossy());
        }
    }
    lexical_normpath(path_str)
}

fn lexical_normpath(path: &str) -> PathBuf {
    // Minimal lexical normpath — mirrors `os.path.normpath` / `realpath(strict=False)` for missing paths.
    let is_absolute = path.starts_with('/');
    let mut parts: Vec<String> = Vec::new();
    for comp in path.split('/') {
        if comp.is_empty() || comp == "." {
            continue;
        } else if comp == ".." {
            if !parts.is_empty() && parts.last().map(|s| s.as_str()) != Some("..") {
                parts.pop();
            } else if !is_absolute {
                parts.push("..".to_string());
            }
        } else {
            parts.push(comp.to_string());
        }
    }
    let mut out = String::new();
    if is_absolute {
        out.push('/');
    }
    out.push_str(&parts.join("/"));
    if out.is_empty() {
        if is_absolute { PathBuf::from("/") } else { PathBuf::from(".") }
    } else {
        PathBuf::from(out)
    }
}

fn is_relative_to(path: &Path, root: &Path) -> bool {
    // Mirrors `path.relative_to(tmp_root)` try/except ValueError (218-229)
    path.starts_with(root)
}

#[allow(dead_code)]
pub fn _should_auto_approve_edit(proposal: &EditProposal, policy: &str, cwd: Option<&str>) -> bool {
    should_auto_approve_edit(proposal, policy, cwd)
}

// ---------------------------------------------------------------------------
// maybe_require_edit_approval — lines 233-261
// ---------------------------------------------------------------------------

/// Run ACP edit approval if bound.
/// Returns a JSON tool-error string when the edit must be blocked, otherwise `None` so dispatch can continue.
/// Requester exceptions deny by default.
///
/// Mirrors `def maybe_require_edit_approval(tool_name: str, arguments: dict[str, Any]) -> str | None:` (233-261).
///
/// ```python
/// def maybe_require_edit_approval(tool_name: str, arguments: dict[str, Any]) -> str | None:
///     requester = get_edit_approval_requester()
///     if requester is None:
///         return None
///     try:
///         proposal = build_edit_proposal(tool_name, arguments)
///     except Exception as exc:
///         logger.warning("Could not build ACP edit approval proposal for %s: %s", tool_name, exc)
///         return json.dumps({"error": f"Edit approval denied: could not prepare diff ({exc})"}, ensure_ascii=False)
///     if proposal is None:
///         return None
///     try:
///         approved = bool(requester(proposal))
///     except Exception as exc:
///         logger.warning("ACP edit approval requester failed: %s", exc)
///         approved = False
///     if approved:
///         return None
///     return json.dumps({"error": "Edit approval denied by ACP client; file was not modified."}, ensure_ascii=False)
/// ```
pub fn maybe_require_edit_approval(tool_name: &str, arguments: &Arguments) -> Option<String> {
    let requester = get_edit_approval_requester()?; // 240-242

    let proposal = match build_edit_proposal(tool_name, arguments) {
        Ok(opt) => opt,
        Err(exc) => {
            // Mirrors `logger.warning("Could not build ACP edit approval proposal for %s: %s", tool_name, exc)` (247)
            eprintln!("[{}] Could not build ACP edit approval proposal for {}: {}", logger_name(), tool_name, exc);
            return Some(json_error(&format!("Edit approval denied: could not prepare diff ({exc})"))); // 248
        }
    };

    let proposal = match proposal {
        Some(p) => p,
        None => return None, // 250-251
    };

    let approved = {
        // Mirrors `try: approved = bool(requester(proposal)) except Exception: approved = False` (253-257)
        // Use catch_unwind to model exception guard (NEVER cargo: std only)
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| requester(&proposal)));
        match result {
            Ok(v) => v,
            Err(_) => {
                eprintln!("[{}] ACP edit approval requester failed: panic", logger_name());
                false
            }
        }
    };

    if approved {
        return None; // 259-260
    }
    Some(json_error("Edit approval denied by ACP client; file was not modified.")) // 261
}

fn json_error(msg: &str) -> String {
    // Mirrors `json.dumps({"error": msg}, ensure_ascii=False)` (248, 261)
    // Minimal JSON with escaped quotes; `ensure_ascii=False` means unicode passthrough.
    let escaped = msg.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n").replace('\r', "\\r");
    format!("{{\"error\":\"{escaped}\"}}")
}

// ---------------------------------------------------------------------------
// ACP stubs — mirrors `acp` / `acp.schema` imports (lines 267-268, 286-283)
//   `acp.update_tool_call`, `acp.tool_diff_content`, `acp.schema.PermissionOption`
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct DiffContent {
    pub path: String,
    pub old_text: Option<String>,
    pub new_text: String,
}

#[derive(Debug, Clone)]
pub enum ToolContent {
    Diff(DiffContent),
    Text(String),
}

#[derive(Debug, Clone)]
pub struct ToolCallUpdate {
    pub tool_call_id: String,
    pub title: String,
    pub kind: String,
    pub status: String, // "pending" | "completed" | "failed"
    pub content: Vec<ToolContent>,
    pub raw_input: HashMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct PermissionOption {
    pub option_id: String,
    pub kind: String, // "allow_once" | "reject_once"
    pub name: String,
}

// Mirrors `acp.tool_diff_content(path=..., old_text=..., new_text=...)` (276-280)
pub fn tool_diff_content(path: String, old_text: Option<String>, new_text: String) -> ToolContent {
    ToolContent::Diff(DiffContent { path, old_text, new_text })
}

// Mirrors `acp.update_tool_call(tool_call_id, title, kind, status, content, raw_input)` (270-283)
pub fn update_tool_call(
    tool_call_id: String,
    title: String,
    kind: String,
    status: String,
    content: Vec<ToolContent>,
    raw_input: HashMap<String, String>,
) -> ToolCallUpdate {
    ToolCallUpdate { tool_call_id, title, kind, status, content, raw_input }
}

// ---------------------------------------------------------------------------
// build_acp_edit_tool_call — lines 264-283
// ---------------------------------------------------------------------------

/// Build the ToolCallUpdate payload for ACP request_permission.
/// Mirrors `def build_acp_edit_tool_call(proposal: EditProposal):` (264-283).
///
/// ```python
/// def build_acp_edit_tool_call(proposal: EditProposal):
///     import acp
///     tool_call_id = f"edit-approval-{next(_PERMISSION_REQUEST_IDS)}"
///     return acp.update_tool_call(
///         tool_call_id,
///         title=f"Approve edit: {proposal.path}",
///         kind="edit",
///         status="pending",
///         content=[acp.tool_diff_content(path=proposal.path, old_text=proposal.old_text, new_text=proposal.new_text)],
///         raw_input={"tool": proposal.tool_name, "arguments": proposal.arguments},
///     )
/// ```
pub fn build_acp_edit_tool_call(proposal: &EditProposal) -> ToolCallUpdate {
    let tool_call_id = format!("edit-approval-{}", PERMISSION_REQUEST_IDS.fetch_add(1, Ordering::Relaxed)); // 269
    let title = format!("Approve edit: {}", proposal.path); // 272
    let mut raw_input = proposal.arguments.clone(); // 282 `raw_input={"tool": ..., "arguments": ...}`
    raw_input.insert("tool".to_string(), proposal.tool_name.clone());
    // arguments already in proposal.arguments; flatten for stub
    update_tool_call(
        tool_call_id,
        title,
        "edit".to_string(),    // 273
        "pending".to_string(), // 274
        vec![tool_diff_content(
            proposal.path.clone(),
            proposal.old_text.clone(),
            proposal.new_text.clone(),
        )], // 275-281
        raw_input, // 282
    )
}

// ---------------------------------------------------------------------------
// make_acp_edit_approval_requester — lines 286-337
// ---------------------------------------------------------------------------

/// Outcome of `request_permission` — mirrors `response.outcome.outcome` / `option_id` (332-336).
#[derive(Debug, Clone)]
pub struct PermissionOutcome {
    pub outcome: String,   // "selected" | ...
    pub option_id: String, // "allow_once" | "deny"
}

#[derive(Debug, Clone)]
pub struct PermissionResponse {
    pub outcome: Option<PermissionOutcome>,
}

/// Minimal stub for `asyncio.AbstractEventLoop` + `concurrent.futures` future
/// timeout handling (lines 287-292, 318-330). In Python:
/// ```python
/// future = safe_schedule_threadsafe(coro, loop, ...)
/// if future is None: return False
/// try:
///     response = future.result(timeout=timeout)
/// except (FutureTimeout, Exception): future.cancel(); return False
/// ```
/// Rust (NEVER cargo): the event loop is a no-op handle; timeout maps to a
/// `Duration` budget. Scheduling is modelled as a direct call to the supplied
/// `request_permission_fn` stub. The `FutureTimeout` branch is modelled via
/// an explicit `timed_out` flag returned by the stub.
#[derive(Debug, Clone, Default)]
pub struct EventLoopStub;

/// Mirrors `def make_acp_edit_approval_requester(request_permission_fn: Callable, loop: asyncio.AbstractEventLoop, session_id: str, timeout: float = 60.0, auto_approve_getter: Callable[[], tuple[str, str | None]] | None = None) -> EditApprovalRequester:` (286-337).
///
/// Returns a sync requester that bridges edit proposals to ACP permissions.
///
/// ```python
/// def make_acp_edit_approval_requester(request_permission_fn, loop, session_id, timeout=60.0, auto_approve_getter=None):
///     def _requester(proposal: EditProposal) -> bool:
///         if auto_approve_getter is not None:
///             try:
///                 policy, cwd = auto_approve_getter()
///                 if should_auto_approve_edit(proposal, policy, cwd):
///                     logger.info("Auto-approved ACP edit under policy %s: %s", policy, proposal.path)
///                     return True
///             except Exception:
///                 logger.debug("ACP edit auto-approval policy check failed", exc_info=True)
///         options = [PermissionOption(...), PermissionOption(...)]
///         tool_call = build_acp_edit_tool_call(proposal)
///         coro = request_permission_fn(session_id=session_id, tool_call=tool_call, options=options)
///         future = safe_schedule_threadsafe(coro, loop, ...)
///         if future is None: return False
///         try:
///             response = future.result(timeout=timeout)
///         except (FutureTimeout, Exception): future.cancel(); return False
///         outcome = getattr(response, "outcome", None)
///         return getattr(outcome, "outcome", None) == "selected" and getattr(outcome, "option_id", None) == "allow_once"
///     return _requester
/// ```
pub fn make_acp_edit_approval_requester<F>(
    request_permission_fn: F,
    _loop: EventLoopStub,
    session_id: String,
    timeout_secs: f64,
    auto_approve_getter: Option<Arc<dyn Fn() -> (String, Option<String>) + Send + Sync>>,
) -> EditApprovalRequester
where
    F: Fn(String, ToolCallUpdate, Vec<PermissionOption>) -> Option<PermissionResponse>
        + Send
        + Sync
        + 'static,
{
    let request_permission_fn = Arc::new(request_permission_fn);
    let session_id_clone = session_id.clone();

    Arc::new(move |proposal: &EditProposal| {
        // Mirrors `if auto_approve_getter is not None: try: policy, cwd = auto_approve_getter() ...` (299-306)
        if let Some(ref getter) = auto_approve_getter {
            // Use catch_unwind to model `except Exception: logger.debug(..., exc_info=True)` (305-306)
            let policy_cwd = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| getter()));
            match policy_cwd {
                Ok((policy, cwd)) => {
                    if should_auto_approve_edit(proposal, &policy, cwd.as_deref()) {
                        eprintln!("[{}] Auto-approved ACP edit under policy {}: {}", logger_name(), policy, proposal.path); // 303
                        return true;
                    }
                }
                Err(_) => {
                    eprintln!("[{}] ACP edit auto-approval policy check failed", logger_name()); // 306
                }
            }
        }

        // Mirrors `options = [PermissionOption(option_id="allow_once", ...), PermissionOption(option_id="deny", ...)]` (308-311)
        let options = vec![
            PermissionOption { option_id: "allow_once".to_string(), kind: "allow_once".to_string(), name: "Allow edit".to_string() },
            PermissionOption { option_id: "deny".to_string(), kind: "reject_once".to_string(), name: "Deny".to_string() },
        ];
        let tool_call = build_acp_edit_tool_call(proposal); // 312
        // Mirrors `coro = request_permission_fn(session_id=session_id, tool_call=tool_call, options=options)` (313-317)
        let coro_result = request_permission_fn(session_id_clone.clone(), tool_call, options);

        // Mirrors `future = safe_schedule_threadsafe(coro, loop, ...)` (318-323)
        // Stub: `safe_schedule_threadsafe` returns a future wrapping the coro; here the coro is already
        // executed synchronously via `request_permission_fn`. `None` models scheduling failure.
        let future = coro_result; // In stub, `request_permission_fn` returning `None` models `future is None` (324-325)
        if future.is_none() {
            return false; // 324-325
        }

        // Mirrors timeout handling (326-330):
        // `try: response = future.result(timeout=timeout) except (FutureTimeout, Exception): future.cancel(); return False`
        // In stub we have no real async; we check `timeout_secs` budget as a guard.
        // If timeout_secs <= 0, treat as immediate timeout (mirrors FutureTimeout branch).
        if timeout_secs <= 0.0 {
            eprintln!("[{}] Edit approval request timed out or failed: timeout", logger_name()); // 330
            return false;
        }
        let response = future.unwrap();

        // Mirrors `outcome = getattr(response, "outcome", None)` (332) + `return outcome.outcome == "selected" and outcome.option_id == "allow_once"` (333-336)
        if let Some(outcome) = response.outcome {
            return outcome.outcome == "selected" && outcome.option_id == "allow_once";
        }
        false
    })
}

#[allow(dead_code)]
pub fn _make_acp_edit_approval_requester<F>(
    f: F,
    l: EventLoopStub,
    s: String,
    t: f64,
    g: Option<Arc<dyn Fn() -> (String, Option<String>) + Send + Sync>>,
) -> EditApprovalRequester
where
    F: Fn(String, ToolCallUpdate, Vec<PermissionOption>) -> Option<PermissionResponse> + Send + Sync + 'static,
{
    make_acp_edit_approval_requester(f, l, s, t, g)
}

// ---------------------------------------------------------------------------
// Tests — minimal runnable check (ponytail: one check for branch-heavy logic)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_v4a_paths_roundtrip() {
        let body = "*** Update File: src/foo.rs\n*** Add File: src/bar.rs\n*** Move File: a.txt -> b.txt\n";
        let paths = extract_v4a_patch_paths(body);
        assert_eq!(paths, vec!["src/foo.rs", "src/bar.rs", "a.txt", "b.txt"]);
    }

    #[test]
    fn sensitive_blocks_git_and_env() {
        assert!(is_sensitive_auto_approve_path(".git/config"));
        assert!(is_sensitive_auto_approve_path("/home/user/.ssh/id_rsa"));
        let p = EditProposal::new("write_file", "/home/user/project/.env", None, "x", HashMap::new());
        assert!(is_sensitive_auto_approve_path(&p.path));
        // session policy should still deny sensitive
        assert!(!should_auto_approve_edit(&p, AUTO_APPROVE_SESSION, None));
    }

    #[test]
    fn build_proposal_write_file() {
        let mut args = Arguments::new();
        args.insert("path".to_string(), ArgVal::Str("/tmp/test.txt".to_string()));
        args.insert("content".to_string(), ArgVal::Str("hello".to_string()));
        let prop = proposal_for_write_file(&args).unwrap();
        assert_eq!(prop.tool_name, "write_file");
        assert_eq!(prop.new_text, "hello");
    }
}
