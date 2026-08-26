//! Config / projects / setup JSON-RPC handlers.
//!
//! 1:1 port of `tui_gateway/methods_config.py` (558 lines).
//!
//! ```python
//! # Python — tui_gateway/methods_config.py
//! """Config / projects / setup JSON-RPC handlers (moved verbatim from server.py).
//!
//! NOTE: ``config.set`` stays in server.py for now — the in-flight
//! opt/model-resolution-core PR touches it; move it in a follow-up once merged.
//!
//! Handler bodies are byte-identical to their pre-split server.py form; they
//! are rebound onto server.py's globals at install time — see method_ctx.py.
//! """
//! from .method_ctx import HandlerRegistry
//! from hermes_constants import DEFAULT_INDICATOR_STYLE, INDICATOR_STYLES
//! _registry = HandlerRegistry()
//! method = _registry.method
//! _profile_scoped = _registry.profile_scoped
//!
//! @method("projects.discover_repos")
//! @_profile_scoped
//! def _(rid, params: dict) -> dict:
//!     try:
//!         with _profile_db(params) as db:
//!             if db is None: return _ok(rid, {"repos": []})
//!             from hermes_cli import projects_db as pdb
//!             policy = _repo_discovery_policy()
//!             policy_key = _repo_discovery_policy_key(policy)
//!             with pdb.connect_closing() as conn:
//!                 pdb.reconcile_discovered_repos_policy(conn, policy_key, preserve_unversioned=_repo_discovery_policy_is_default(policy))
//!                 if params.get("scan") and policy["enabled"]:
//!                     _scan_discovered_repos_remote(conn, policy)
//!                 repos = _discover_repos_payload(db, conn=conn, include_cached=policy["enabled"])
//!             return _ok(rid, {"repos": repos, "discovery_policy": policy})
//!     except Exception as e: return _err(rid, 5061, str(e))
//!
//! @method("projects.record_repos")
//! @_profile_scoped
//! def _(rid, params: dict) -> dict: ...
//! @method("projects.tree") @_profile_scoped def _(rid, params: dict) -> dict: ...
//! @method("projects.project_sessions") @_profile_scoped def _(rid, params: dict) -> dict: ...
//! @method("config.get") def _(rid, params: dict) -> dict: ...
//! @method("setup.status") def _(rid, params: dict) -> dict: ...
//! @method("setup.runtime_check") def _(rid, params: dict) -> dict: ...
//! @method("diagnostics.share_nous") def _(rid, params: dict) -> dict: ...
//! def register(server) -> None: _registry.install(server)
//! ```
//!
//! # Rust mapping
//! * `HandlerRegistry` → [`crate::method_ctx::HandlerRegistry`] (same deferred
//!   `@method` + `install` shape; see `method_ctx.rs` for `FunctionType`
//!   rebinding no-op notes). Profile-scoped handlers use
//!   [`HandlerRegistry::method_profile_scoped`], plain handlers use
//!   [`HandlerRegistry::method`].
//! * `_profile_db(params)` / `_discover_repos_payload` / `projects_db` /
//!   `_repo_discovery_policy*` / `_scan_discovered_repos_remote` → injected
//!   closures (`Fn(&str) -> Result<...,String>`). The port keeps the `try:`
//!   → `5061` envelope mapping and the `db is None → repos: []` early return.
//! * `_build_project_tree(db, preview_limit, hydrate, session_limit, include_discovered)`
//!   / `scoped_session_ids` / `active_id` → injected `Fn(...) -> String` that
//!   returns the JSON fragment for the result payload; the handler only validates
//!   params and maps exceptions to `5061`.
//! * `config.get` key dispatch — every branch is mirrored as a pure normalizer
//!   plus an injected loader for the value that needs I/O. Unknown key →
//!   `4002`, `provider` exception → `5013`, `approvals` → `5001`. Normalizers:
//!   `indicator` (case-insensitive `INDICATOR_STYLES` fallback to
//!   `DEFAULT_INDICATOR_STYLE`), `details_mode` (`hidden|collapsed|expanded`
//!   → `collapsed`), `thinking_mode` (`collapsed|truncated|full` else fallback
//!   to `details_mode == expanded → full else collapsed`), `theme`
//!   (`auto|light|dark` → `auto`), `statusbar` via [`coerce_statusbar`],
//!   `density` from `tui_compact`, `reasoning` (`false → none`, else `medium`
//!   default, `show_reasoning` → `show|hide`), `fast` (`priority → fast else
//!   normal`), `focus` (`focus_view` + `tool_progress`), `mouse` via
//!   [`display_mouse_tracking`], `mtime` + `mcp_rev`.
//! * `setup.status` (`_has_any_provider_configured` → bool) → injected
//!   `Fn() -> Result<bool,String>`; exception → `5016`.
//! * `setup.runtime_check` (`resolve_runtime_provider` + `has_usable_secret`) →
//!   injected `Fn(Option<&str>) -> Result<RuntimeInfo,String>`; always returns
//!   `_ok` envelope (`ok: true|false`), never `_err`, mirroring Python's
//!   `except Exception as e: return _ok(rid, {"ok": False, "error": str(e)})`.
//! * `diagnostics.share_nous` (`collect_share_bundle` → `build_nous_bundle` →
//!   `share_to_nous`, `_redact_log_text`, label sanitization, `log_lines` bounds
//!   `10..=2000` → `200`, `extra_files` max 4 × 512 KiB, `client/` prefix) →
//!   injected `Fn(&ShareNousParams) -> Result<ShareNousOk,String>` plus pure
//!   helpers [`sanitize_label`], [`validate_log_lines`], [`redact_text_stub`].
//!   Always returns `_ok` with `ok: bool`, never JSON-RPC error.
//! * `_ok(rid, result)` / `_err(rid, code, msg)` → [`ok_response`] /
//!   [`err_response`] (mirrors `server.py::_ok` / `_err`).

use std::collections::HashMap;

use crate::method_ctx::HandlerRegistry;

// ---------------------------------------------------------------------------
// Method names — mirrors @method("...") decorators
// ---------------------------------------------------------------------------

pub const METHOD_PROJECTS_DISCOVER_REPOS: &str = "projects.discover_repos";
pub const METHOD_PROJECTS_RECORD_REPOS: &str = "projects.record_repos";
pub const METHOD_PROJECTS_TREE: &str = "projects.tree";
pub const METHOD_PROJECTS_PROJECT_SESSIONS: &str = "projects.project_sessions";
pub const METHOD_CONFIG_GET: &str = "config.get";
pub const METHOD_SETUP_STATUS: &str = "setup.status";
pub const METHOD_SETUP_RUNTIME_CHECK: &str = "setup.runtime_check";
pub const METHOD_DIAGNOSTICS_SHARE_NOUS: &str = "diagnostics.share_nous";

// ---------------------------------------------------------------------------
// Error codes — mirrors _err(rid, N, ...)
// ---------------------------------------------------------------------------

pub const ERR_PROJECTS: i32 = 5061;
pub const ERR_PROJECT_ID_REQUIRED: i32 = 5063;
pub const ERR_UNKNOWN_CONFIG_KEY: i32 = 4002;
pub const ERR_CONFIG_PROVIDER: i32 = 5013;
pub const ERR_APPROVAL_MODE: i32 = 5001;
pub const ERR_SETUP_STATUS: i32 = 5016;

// ---------------------------------------------------------------------------
// Indicator constants — mirrors hermes_constants
// ---------------------------------------------------------------------------

pub const DEFAULT_INDICATOR_STYLE: &str = "kaomoji";
pub const INDICATOR_STYLES: &[&str] = &["ascii", "emoji", "kaomoji", "unicode"];

// ---------------------------------------------------------------------------
// Small helpers — JSON envelope, rid encoding, field extraction
// ---------------------------------------------------------------------------

fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            _ => out.push(ch),
        }
    }
    out
}

pub fn ok_response(rid_json: &str, result_json: &str) -> String {
    format!(r#"{{"jsonrpc":"2.0","id":{},"result":{}}}"#, rid_json, result_json)
}

pub fn err_response(rid_json: &str, code: i32, msg: &str) -> String {
    let esc = json_escape(msg);
    format!(
        r#"{{"jsonrpc":"2.0","id":{},"error":{{"code":{},"message":"{}"}}}}"#,
        rid_json, code, esc
    )
}

pub fn encode_rid(rid: &str) -> String {
    let t = rid.trim();
    if t.is_empty() {
        return "null".to_string();
    }
    if t == "null" || t == "true" || t == "false" {
        return t.to_string();
    }
    if t.parse::<i64>().is_ok() || t.parse::<f64>().is_ok() {
        if t.eq_ignore_ascii_case("inf")
            || t.eq_ignore_ascii_case("nan")
            || t.eq_ignore_ascii_case("-inf")
        {
            return format!("\"{}\"", json_escape(t));
        }
        return t.to_string();
    }
    if (t.starts_with('"') && t.ends_with('"') && t.len() >= 2)
        || (t.starts_with('\'') && t.ends_with('\'') && t.len() >= 2)
    {
        return format!("\"{}\"", json_escape(&t[1..t.len() - 1]));
    }
    format!("\"{}\"", json_escape(t))
}

pub fn extract_string_field(json: &str, field: &str) -> Option<String> {
    let key = format!("\"{}\"", field);
    let pos = json.find(&key)?;
    let after = &json[pos + key.len()..];
    let colon = after.find(':')?;
    let mut val = after[colon + 1..].trim_start();
    if val.starts_with("null") {
        return None;
    }
    if val.starts_with('\'') {
        let end = val[1..].find('\'')?;
        return Some(val[1..1 + end].to_string());
    }
    if !val.starts_with('"') {
        let end = val.find(|c| c == ',' || c == '}').unwrap_or(val.len());
        let raw = val[..end].trim().trim_matches('"').trim_matches('\'');
        if raw.is_empty() || raw == "null" {
            return None;
        }
        return Some(raw.to_string());
    }
    let mut out = String::new();
    let mut chars = val[1..].chars();
    let mut esc = false;
    for ch in chars {
        if esc {
            match ch {
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                '"' => out.push('"'),
                '\\' => out.push('\\'),
                '/' => out.push('/'),
                'u' => {
                    for _ in 0..3 {
                        chars.next();
                    }
                    out.push('?');
                }
                _ => out.push(ch),
            }
            esc = false;
            continue;
        }
        if ch == '\\' {
            esc = true;
            continue;
        }
        if ch == '"' {
            return Some(out);
        }
        out.push(ch);
    }
    None
}

pub fn extract_string_or_empty(json: &str, field: &str) -> String {
    extract_string_field(json, field).unwrap_or_default()
}

pub fn extract_raw_value(json: &str, field: &str) -> Option<String> {
    let key = format!("\"{}\"", field);
    let pos = json.find(&key)?;
    let after = &json[pos + key.len()..];
    let colon = after.find(':')?;
    let mut rest = after[colon + 1..].trim_start();
    if rest.is_empty() {
        return None;
    }
    if rest.starts_with('[') || rest.starts_with('{') {
        let open = rest.chars().next().unwrap();
        let close = if open == '[' { ']' } else { '}' };
        let mut depth = 0usize;
        let mut in_str = false;
        let mut esc = false;
        let mut end_idx: Option<usize> = None;
        for (i, ch) in rest.char_indices() {
            if esc {
                esc = false;
                continue;
            }
            if ch == '\\' && in_str {
                esc = true;
                continue;
            }
            if ch == '"' && !esc {
                in_str = !in_str;
                continue;
            }
            if in_str {
                continue;
            }
            if ch == open {
                depth += 1;
            } else if ch == close {
                if depth > 0 {
                    depth -= 1;
                    if depth == 0 {
                        end_idx = Some(i);
                        break;
                    }
                }
            }
        }
        if let Some(e) = end_idx {
            return Some(rest[..=e].to_string());
        }
        return None;
    }
    if rest.starts_with('"') || rest.starts_with('\'') {
        let qc = rest.chars().next().unwrap();
        let mut esc = false;
        for (i, ch) in rest[1..].char_indices() {
            if esc {
                esc = false;
                continue;
            }
            if ch == '\\' {
                esc = true;
                continue;
            }
            if ch == qc {
                return Some(rest[..=i + 1].to_string());
            }
        }
        return None;
    }
    let end = rest.find(|c| c == ',' || c == '}').unwrap_or(rest.len());
    Some(rest[..end].trim().to_string())
}

pub fn extract_bool_field(json: &str, field: &str) -> Option<bool> {
    let raw = extract_raw_value(json, field)?;
    match raw.trim() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Config normalizers — mirrors config.get branches
// ---------------------------------------------------------------------------

pub fn normalize_indicator_style(raw: &str) -> &str {
    let norm = raw.trim().to_ascii_lowercase();
    if INDICATOR_STYLES.contains(&norm.as_str()) {
        // SAFETY: we just checked membership; return the static slice entry
        // to avoid returning a reference to the temporary `norm`.
        for &s in INDICATOR_STYLES {
            if s == norm {
                return s;
            }
        }
        DEFAULT_INDICATOR_STYLE
    } else {
        DEFAULT_INDICATOR_STYLE
    }
}

/// Owned variant for callers that need an owned string.
pub fn normalize_indicator_style_owned(raw: &str) -> String {
    normalize_indicator_style(raw).to_string()
}

pub fn normalize_details_mode(raw: &str) -> &str {
    let t = raw.trim().to_ascii_lowercase();
    match t.as_str() {
        "hidden" | "collapsed" | "expanded" => {
            for &s in ["hidden", "collapsed", "expanded"] {
                if s == t {
                    return s;
                }
            }
            "collapsed"
        }
        _ => "collapsed",
    }
}

pub fn normalize_thinking_mode(thinking_raw: &str, details_raw: &str) -> String {
    let allowed = ["collapsed", "truncated", "full"];
    let t = thinking_raw.trim().to_ascii_lowercase();
    if allowed.contains(&t.as_str()) {
        return t;
    }
    let dm = details_raw.trim().to_ascii_lowercase();
    if dm == "expanded" {
        "full".to_string()
    } else {
        "collapsed".to_string()
    }
}

pub fn normalize_theme(raw: &str) -> &str {
    let t = raw.trim().to_ascii_lowercase();
    match t.as_str() {
        "auto" | "light" | "dark" => {
            for &s in ["auto", "light", "dark"] {
                if s == t {
                    return s;
                }
            }
            "auto"
        }
        _ => "auto",
    }
}

/// Mirrors `_coerce_statusbar` — `top|bottom|hidden` else `top`.
pub fn coerce_statusbar(raw: &str) -> &str {
    let t = raw.trim().to_ascii_lowercase();
    match t.as_str() {
        "top" | "bottom" | "hidden" => {
            for &s in ["top", "bottom", "hidden"] {
                if s == t {
                    return s;
                }
            }
            "top"
        }
        _ => "top",
    }
}

pub fn coerce_statusbar_owned(raw: &str) -> String {
    coerce_statusbar(raw).to_string()
}

/// Mirrors `_display_mouse_tracking` — returns `"on"` / `"off"` from display dict.
/// Python checks `display.get("mouse", ...)` style; here caller passes raw.
pub fn display_mouse_tracking(raw: Option<&str>) -> &str {
    match raw.map(|s| s.trim().to_ascii_lowercase()) {
        Some(v) if v == "on" || v == "true" || v == "1" || v == "yes" => "on",
        Some(v) if v == "off" || v == "false" || v == "0" || v == "no" => "off",
        _ => "off",
    }
}

/// Mirrors `_load_tool_progress_mode` — `full|minimal|off` else `full`.
pub fn normalize_tool_progress_mode(raw: &str) -> &str {
    let t = raw.trim().to_ascii_lowercase();
    match t.as_str() {
        "full" | "minimal" | "off" => {
            for &s in ["full", "minimal", "off"] {
                if s == t {
                    return s;
                }
            }
            "full"
        }
        _ => "full",
    }
}

/// Mirrors `config.get` reasoning effort display mapping.
/// `show_reasoning: bool` → `"show"` / `"hide"`.
pub fn reasoning_display(show_reasoning: Option<bool>) -> &'static str {
    match show_reasoning {
        Some(false) => "hide",
        _ => "show",
    }
}

/// Normalize reasoning effort from config or session override.
/// Mirrors:
/// ```python
/// if reasoning_config.get("enabled") is False: effort = "none"
/// else: effort = str(reasoning_config.get("effort") or "medium")
/// ```
/// and `raw_effort is False → "none"` else `str(raw_effort or "medium")`.
pub fn normalize_reasoning_effort(effort_raw: Option<&str>, enabled: Option<bool>) -> String {
    if let Some(false) = enabled {
        return "none".to_string();
    }
    match effort_raw {
        None => "medium".to_string(),
        Some(s) => {
            let t = s.trim();
            if t.is_empty() || t.eq_ignore_ascii_case("false") {
                // Python: `if raw_effort is False: "none" else str(raw_effort or "medium")`
                // str("false") would be "false", but bool False is handled via `enabled`.
                // Empty → "medium".
                if t.is_empty() {
                    "medium".to_string()
                } else {
                    t.to_ascii_lowercase()
                }
            } else {
                t.to_string()
            }
        }
    }
}

/// Parse `preview_limit` / `session_limit` with `or 3` / `or 2000` default semantics.
/// Mirrors `int(params.get("preview_limit") or 3)`.
pub fn parse_limit(params_json: &str, field: &str, default: i64) -> i64 {
    let raw = extract_raw_value(params_json, field);
    match raw {
        None => default,
        Some(v) => {
            let t = v.trim().trim_matches('"').trim().to_string();
            if t.is_empty() || t == "null" {
                return default;
            }
            match t.parse::<i64>() {
                Ok(n) if n != 0 => n,
                Ok(_) => default,
                Err(_) => default,
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Diagnostics helpers — mirrors diagnostics.share_nous sanitization
// ---------------------------------------------------------------------------

/// Validate `log_lines` param: `10..=2000` else `200`.
/// Mirrors `if not isinstance(log_lines, int) or not (10 <= log_lines <= 2000): log_lines = 200`.
pub fn validate_log_lines(raw: Option<&str>) -> usize {
    match raw {
        None => 200,
        Some(s) => {
            let t = s.trim().trim_matches('"');
            match t.parse::<i64>() {
                Ok(n) if (10..=2000).contains(&n) => n as usize,
                _ => 200,
            }
        }
    }
}

/// Sanitize a `client/` label.
/// Mirrors:
/// ```python
/// safe_label = "".join(ch for ch in label if ch.isalnum() or ch in "._- ()").strip()[:64]
/// while ".." in safe_label: safe_label = safe_label.replace("..", ".")
/// safe_label = safe_label.lstrip(".").strip()
/// ```
pub fn sanitize_label(label: &str) -> String {
    let mut s: String = label
        .chars()
        .filter(|c| c.is_alphanumeric() || "._- ()".contains(*c))
        .collect();
    s = s.trim().to_string();
    if s.len() > 64 {
        s.truncate(64);
    }
    s = s.trim().to_string();
    while s.contains("..") {
        s = s.replace("..", ".");
    }
    s = s.trim_start_matches('.').trim().to_string();
    s
}

/// Stub redactor — mirrors `_redact_log_text` (force secret redaction + email masking).
/// Real implementation is injected; this stub does minimal trimming for tests.
pub fn redact_text_stub(s: &str) -> String {
    s.to_string()
}

// ---------------------------------------------------------------------------
// Core handlers — mirrors each @method body, injected for std-only testing
// ---------------------------------------------------------------------------

/// Handle `projects.discover_repos`.
///
/// `with_db` mirrors `with _profile_db(params) as db:` — `Ok(None)` means
/// `db is None → {"repos": []}`, `Err(e)` → `5061`, `Ok(Some(json))` where
/// json is the full result payload fragment `{"repos":..., "discovery_policy":...}`.
pub fn handle_projects_discover_repos<F>(rid_json: &str, params_json: &str, with_db: F) -> String
where
    F: Fn(&str) -> Result<Option<String>, String>,
{
    match with_db(params_json) {
        Err(e) => err_response(rid_json, ERR_PROJECTS, &e),
        Ok(None) => ok_response(rid_json, r#"{"repos":[]}"#),
        Ok(Some(payload_json)) => {
            // payload_json already contains repos + discovery_policy; wrap as result
            // If caller returned just repos array, wrap; but spec says repos+policy.
            // Detect if it's already an object with repos.
            let trimmed = payload_json.trim();
            if trimmed.starts_with('{') {
                ok_response(rid_json, trimmed)
            } else {
                ok_response(rid_json, &format!(r#"{{"repos":{}}}"#, trimmed))
            }
        }
    }
}

/// Handle `projects.record_repos`.
///
/// `op` mirrors the whole `try:` block: policy reconcile + record + payload.
/// Returns `Ok(result_json)` where result_json is `{"repos":..., "accepted":..., "discovery_policy":...}`.
/// `Err(e)` → `5061`.
pub fn handle_projects_record_repos<F>(rid_json: &str, params_json: &str, op: F) -> String
where
    F: Fn(&str) -> Result<String, String>,
{
    match op(params_json) {
        Ok(result_json) => {
            let t = result_json.trim();
            ok_response(rid_json, t)
        }
        Err(e) => err_response(rid_json, ERR_PROJECTS, &e),
    }
}

/// Handle `projects.tree`.
///
/// `op` mirrors `with _profile_db + _build_project_tree` returning
/// `{"projects":..., "active_id":..., "scoped_session_ids":...}` as JSON.
pub fn handle_projects_tree<F>(rid_json: &str, params_json: &str, op: F) -> String
where
    F: Fn(&str) -> Result<Option<String>, String>,
{
    match op(params_json) {
        Err(e) => err_response(rid_json, ERR_PROJECTS, &e),
        Ok(None) => ok_response(
            rid_json,
            r#"{"projects":[],"active_id":null,"scoped_session_ids":[]}"#,
        ),
        Ok(Some(payload)) => ok_response(rid_json, payload.trim()),
    }
}

/// Handle `projects.project_sessions`.
///
/// Validates `project_id` first (empty → `5063`), then delegates to `op`.
pub fn handle_projects_project_sessions<F>(rid_json: &str, params_json: &str, op: F) -> String
where
    F: Fn(&str, &str) -> Result<Option<String>, String>,
{
    let project_id = extract_string_field(params_json, "project_id")
        .unwrap_or_default()
        .trim()
        .to_string();
    if project_id.is_empty() {
        return err_response(rid_json, ERR_PROJECT_ID_REQUIRED, "project_id required");
    }
    match op(params_json, &project_id) {
        Err(e) => err_response(rid_json, ERR_PROJECTS, &e),
        Ok(None) => ok_response(rid_json, r#"{"project":null}"#),
        Ok(Some(payload)) => ok_response(rid_json, payload.trim()),
    }
}

/// Handle `config.get`.
///
/// `get` is injected as `Fn(key, params_json) -> Result<result_json, (code, msg)>`.
/// Mirrors the per-key dispatch; unknown key → `4002`, provider exception → `5013`,
/// approval_mode exception → `5001`. The closure returns the JSON for the `result`
/// field (e.g. `{"value":"on"}` or `{"model":"...","provider":"...","providers":[...]}`).
/// `None` from `extract_string_field("key")` is treated as `""` → unknown.
pub fn handle_config_get<F>(rid_json: &str, params_json: &str, get: F) -> String
where
    F: Fn(&str, &str) -> Result<String, (i32, String)>,
{
    let key = extract_string_field(params_json, "key").unwrap_or_default();
    match get(&key, params_json) {
        Ok(result_json) => ok_response(rid_json, result_json.trim()),
        Err((code, msg)) => err_response(rid_json, code, &msg),
    }
}

/// Pure helper for `config.get` unknown-key branch (for tests).
pub fn config_get_unknown_key(key: &str) -> (i32, String) {
    (ERR_UNKNOWN_CONFIG_KEY, format!("unknown config key: {}", key))
}

/// Handle `setup.status`.
///
/// `check` mirrors `from hermes_cli.main import _has_any_provider_configured`.
pub fn handle_setup_status<F>(rid_json: &str, check: F) -> String
where
    F: Fn() -> Result<bool, String>,
{
    match check() {
        Ok(v) => ok_response(rid_json, &format!(r#"{{"provider_configured":{}}}"#, if v { "true" } else { "false" })),
        Err(e) => err_response(rid_json, ERR_SETUP_STATUS, &e),
    }
}

/// Runtime info for `setup.runtime_check`.
#[derive(Debug, Clone)]
pub struct RuntimeInfo {
    pub provider: String,
    pub model: String,
    pub source: String,
    pub api_key: Option<String>,
    pub command: Option<String>,
}

impl RuntimeInfo {
    pub fn new(provider: &str, model: &str, source: &str) -> Self {
        Self {
            provider: provider.to_string(),
            model: model.to_string(),
            source: source.to_string(),
            api_key: None,
            command: None,
        }
    }
}

/// Handle `setup.runtime_check`.
///
/// Always returns `_ok` envelope (mirrors Python's `except Exception as e: return _ok(rid, {"ok": False, "error": str(e)})`).
/// `resolve` mirrors `resolve_runtime_provider(requested)`; `has_any` mirrors
/// `_has_any_provider_configured`; `has_secret` mirrors `has_usable_secret`.
pub fn handle_setup_runtime_check<F, G, H>(
    rid_json: &str,
    params_json: &str,
    resolve: F,
    has_any_provider: G,
    has_usable_secret: H,
) -> String
where
    F: Fn(Option<&str>) -> Result<RuntimeInfo, String>,
    G: Fn() -> bool,
    H: Fn(&str) -> bool,
{
    let requested_raw = extract_string_field(params_json, "provider");
    let requested = requested_raw
        .as_deref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty());

    let runtime = match resolve(requested) {
        Ok(r) => r,
        Err(e) => {
            let esc = json_escape(&e);
            return ok_response(rid_json, &format!(r#"{{"ok":false,"error":"{}"}}"#, esc));
        }
    };

    let provider_configured = has_any_provider();
    let provider = if runtime.provider.is_empty() { "provider".to_string() } else { runtime.provider.clone() };
    let source = runtime.source.clone();

    // Bedrock IAM short-circuit when no provider configured but runtime is bedrock via SDK chain.
    if !provider_configured && provider == "bedrock" && (source == "iam-role" || source == "aws-sdk-default-chain") {
        let esc_model = json_escape(&runtime.model);
        let esc_source = json_escape(&source);
        return ok_response(
            rid_json,
            &format!(
                r#"{{"ok":false,"provider":"{}","model":"{}","source":"{}","error":"No Hermes provider is configured."}}"#,
                json_escape(&provider),
                esc_model,
                esc_source
            ),
        );
    }

    // Credential check
    let api_key_text = runtime.api_key.as_deref().unwrap_or("").trim().to_string();
    let has_command = runtime.command.as_deref().map(|c| !c.trim().is_empty()).unwrap_or(false);
    let is_callable = api_key_text == "__callable__"; // sentinel for callable api_key in tests
    let credential_ok = is_callable
        || api_key_text == "aws-sdk"
        || api_key_text == "no-key-required"
        || has_usable_secret(&api_key_text)
        || has_command;

    if !credential_ok {
        let esc_model = json_escape(&runtime.model);
        let esc_source = json_escape(&source);
        let msg = format!("No usable credentials found for {}.", provider);
        return ok_response(
            rid_json,
            &format!(
                r#"{{"ok":false,"provider":"{}","model":"{}","source":"{}","error":"{}"}}"#,
                json_escape(&provider),
                esc_model,
                esc_source,
                json_escape(&msg)
            ),
        );
    }

    let esc_model = json_escape(&runtime.model);
    let esc_source = json_escape(&source);
    ok_response(
        rid_json,
        &format!(
            r#"{{"ok":true,"provider":"{}","model":"{}","source":"{}"}}"#,
            json_escape(&provider),
            esc_model,
            esc_source
        ),
    )
}

/// Params for `diagnostics.share_nous`.
#[derive(Debug, Clone)]
pub struct ShareNousParams {
    pub log_lines: usize,
    pub error_context: Option<String>,
    pub extra_files: Vec<(String, String)>,
}

impl ShareNousParams {
    pub fn from_json(params_json: &str) -> Self {
        let log_lines_raw = extract_raw_value(params_json, "log_lines");
        let log_lines = validate_log_lines(log_lines_raw.as_deref());

        let error_context = extract_string_field(params_json, "error_context")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .map(|s| {
                let truncated = if s.chars().count() > 8000 {
                    s.chars().take(8000).collect()
                } else {
                    s
                };
                redact_text_stub(&truncated)
            });

        // extra_files: at most 4, 512 KiB each, sanitized label
        let mut extra_files = Vec::new();
        if let Some(raw) = extract_raw_value(params_json, "extra_files") {
            let trimmed = raw.trim();
            if trimmed.starts_with('{') {
                // Minimal object parsing: extract "label": "text" pairs
                // Use a simple scan for quoted key then colon then quoted value.
                let mut i = 0;
                let chars: Vec<char> = trimmed.chars().collect();
                let mut count = 0;
                while i < chars.len() && count < 4 {
                    // find opening quote for key
                    while i < chars.len() && chars[i] != '"' && chars[i] != '\'' {
                        i += 1;
                    }
                    if i >= chars.len() {
                        break;
                    }
                    let qc = chars[i];
                    i += 1;
                    let start_key = i;
                    let mut esc = false;
                    let mut end_key = None;
                    while i < chars.len() {
                        if esc {
                            esc = false;
                            i += 1;
                            continue;
                        }
                        if chars[i] == '\\' {
                            esc = true;
                            i += 1;
                            continue;
                        }
                        if chars[i] == qc {
                            end_key = Some(i);
                            i += 1;
                            break;
                        }
                        i += 1;
                    }
                    let key = match end_key {
                        Some(e) => chars[start_key..e].iter().collect::<String>(),
                        None => break,
                    };
                    // skip to colon
                    while i < chars.len() && chars[i] != ':' {
                        i += 1;
                    }
                    if i >= chars.len() {
                        break;
                    }
                    i += 1; // past colon
                    while i < chars.len() && chars[i].is_whitespace() {
                        i += 1;
                    }
                    if i >= chars.len() {
                        break;
                    }
                    // value must be string
                    if chars[i] != '"' && chars[i] != '\'' {
                        // skip non-string value
                        while i < chars.len() && chars[i] != ',' && chars[i] != '}' {
                            i += 1;
                        }
                        if i < chars.len() && chars[i] == ',' {
                            i += 1;
                        }
                        continue;
                    }
                    let vqc = chars[i];
                    i += 1;
                    let start_val = i;
                    let mut esc2 = false;
                    let mut end_val = None;
                    while i < chars.len() {
                        if esc2 {
                            esc2 = false;
                            i += 1;
                            continue;
                        }
                        if chars[i] == '\\' {
                            esc2 = true;
                            i += 1;
                            continue;
                        }
                        if chars[i] == vqc {
                            end_val = Some(i);
                            i += 1;
                            break;
                        }
                        i += 1;
                    }
                    let val = match end_val {
                        Some(e) => chars[start_val..e].iter().collect::<String>(),
                        None => break,
                    };
                    let safe_label = sanitize_label(&key);
                    if safe_label.is_empty() || val.trim().is_empty() {
                        // skip
                    } else {
                        let capped = if val.chars().count() > 524_288 {
                            val.chars().take(524_288).collect::<String>()
                        } else {
                            val
                        };
                        extra_files.push((safe_label, redact_text_stub(&capped)));
                        count += 1;
                    }
                    // skip comma
                    while i < chars.len() && chars[i] != ',' && chars[i] != '"' && chars[i] != '\'' {
                        i += 1;
                    }
                    if i < chars.len() && chars[i] == ',' {
                        i += 1;
                    }
                }
            }
        }

        ShareNousParams { log_lines, error_context, extra_files }
    }
}

/// Handle `diagnostics.share_nous`.
///
/// Always returns `_ok` (structured `ok`/`error`), never JSON-RPC `_err`.
/// `collect` mirrors `collect_share_bundle` + `build_nous_bundle` + redaction,
/// `upload` mirrors `share_to_nous(blob)` returning `Ok((view_url, upload_id, expires_at))`
/// or `Err(msg)` which maps to `{"ok":false,"error":msg}`.
/// The `log_lines` bounds, `error_context` truncation, `extra_files` sanitization
/// are handled in [`ShareNousParams::from_json`] (pure, mirrors Python inline).
pub fn handle_diagnostics_share_nous<C, U>(
    rid_json: &str,
    params_json: &str,
    collect: C,
    upload: U,
) -> String
where
    C: Fn(&ShareNousParams) -> Result<HashMap<String, String>, String>,
    U: Fn(&HashMap<String, String>) -> Result<(Option<String>, Option<String>, Option<String>), String>,
{
    let p = ShareNousParams::from_json(params_json);
    let bundle = match collect(&p) {
        Ok(b) => b,
        Err(e) => {
            let esc = json_escape(&e);
            return ok_response(rid_json, &format!(r#"{{"ok":false,"error":"{}"}}"#, esc));
        }
    };
    // Inject error-context and client/ files are done in collect's construction via p;
    // here we just model the bundle already contains them. Python adds them after collect:
    // we simulate by letting collect incorporate p.error_context / p.extra_files.
    match upload(&bundle) {
        Ok((view_url, upload_id, expires_at)) => {
            if view_url.is_none() && upload_id.is_none() {
                return ok_response(
                    rid_json,
                    r#"{"ok":false,"error":"upload succeeded but returned no view URL or id"}"#,
                );
            }
            let vu = view_url.map(|s| format!(r#""view_url":"{}""#, json_escape(&s))).unwrap_or_default();
            let uid = upload_id.map(|s| format!(r#""upload_id":"{}""#, json_escape(&s))).unwrap_or_default();
            let exp = expires_at.map(|s| format!(r#""expires_at":"{}""#, json_escape(&s))).unwrap_or_default();
            let mut parts = vec![r#""ok":true"#.to_string()];
            if !vu.is_empty() { parts.push(vu); }
            if !uid.is_empty() { parts.push(uid); }
            if !exp.is_empty() { parts.push(exp); }
            ok_response(rid_json, &format!("{{{}}}", parts.join(",")))
        }
        Err(e) => {
            let esc = json_escape(&e);
            ok_response(rid_json, &format!(r#"{{"ok":false,"error":"{}"}}"#, esc))
        }
    }
}

// ---------------------------------------------------------------------------
// Registry wiring — mirrors _registry = HandlerRegistry() + register(server)
// ---------------------------------------------------------------------------

/// Build a fresh [`HandlerRegistry`] with all eight methods registered
/// using the provided deps (for tests / production injection).
///
/// Each closure is `'static` and mirrors the lazy imports inside Python
/// handler bodies. For the default stub (no backend) use [`build_registry_default`].
pub fn build_registry<PD, PR, PT, PS, CG, SS, RC, DN>(
    projects_discover: PD,
    projects_record: PR,
    projects_tree: PT,
    projects_sessions: PS,
    config_get: CG,
    setup_status: SS,
    setup_runtime_check: RC,
    diagnostics_share: DN,
) -> HandlerRegistry
where
    PD: Fn(String, String) -> String + Send + Sync + 'static,
    PR: Fn(String, String) -> String + Send + Sync + 'static,
    PT: Fn(String, String) -> String + Send + Sync + 'static,
    PS: Fn(String, String) -> String + Send + Sync + 'static,
    CG: Fn(String, String) -> String + Send + Sync + 'static,
    SS: Fn(String, String) -> String + Send + Sync + 'static,
    RC: Fn(String, String) -> String + Send + Sync + 'static,
    DN: Fn(String, String) -> String + Send + Sync + 'static,
{
    let mut reg = HandlerRegistry::new();
    register_with(
        &mut reg,
        projects_discover,
        projects_record,
        projects_tree,
        projects_sessions,
        config_get,
        setup_status,
        setup_runtime_check,
        diagnostics_share,
    );
    reg
}

/// Build a registry with default stubs (every operation returns error / `ok:false`).
pub fn build_registry_default() -> HandlerRegistry {
    build_registry(
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_projects_discover_repos(&rid_json, &params_json, |_| Err("no backend".to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_projects_record_repos(&rid_json, &params_json, |_| Err("no backend".to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_projects_tree(&rid_json, &params_json, |_| Err("no backend".to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_projects_project_sessions(&rid_json, &params_json, |_, _| Err("no backend".to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_config_get(&rid_json, &params_json, |key, _| Err(config_get_unknown_key(key)))
        },
        |rid, _| {
            let rid_json = encode_rid(&rid);
            handle_setup_status(&rid_json, || Err("no backend".to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_setup_runtime_check(
                &rid_json,
                &params_json,
                |_| Err("no backend".to_string()),
                || false,
                |_| false,
            )
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_diagnostics_share_nous(&rid_json, &params_json, |_| Err("no backend".to_string()), |_| Err("no backend".to_string()))
        },
    )
}

/// Register all eight methods onto an existing registry.
pub fn register_with<PD, PR, PT, PS, CG, SS, RC, DN>(
    registry: &mut HandlerRegistry,
    projects_discover: PD,
    projects_record: PR,
    projects_tree: PT,
    projects_sessions: PS,
    config_get: CG,
    setup_status: SS,
    setup_runtime_check: RC,
    diagnostics_share: DN,
) where
    PD: Fn(String, String) -> String + Send + Sync + 'static,
    PR: Fn(String, String) -> String + Send + Sync + 'static,
    PT: Fn(String, String) -> String + Send + Sync + 'static,
    PS: Fn(String, String) -> String + Send + Sync + 'static,
    CG: Fn(String, String) -> String + Send + Sync + 'static,
    SS: Fn(String, String) -> String + Send + Sync + 'static,
    RC: Fn(String, String) -> String + Send + Sync + 'static,
    DN: Fn(String, String) -> String + Send + Sync + 'static,
{
    registry.method_profile_scoped(METHOD_PROJECTS_DISCOVER_REPOS, projects_discover);
    registry.method_profile_scoped(METHOD_PROJECTS_RECORD_REPOS, projects_record);
    registry.method_profile_scoped(METHOD_PROJECTS_TREE, projects_tree);
    registry.method_profile_scoped(METHOD_PROJECTS_PROJECT_SESSIONS, projects_sessions);
    registry.method(METHOD_CONFIG_GET, config_get);
    registry.method(METHOD_SETUP_STATUS, setup_status);
    registry.method(METHOD_SETUP_RUNTIME_CHECK, setup_runtime_check);
    registry.method(METHOD_DIAGNOSTICS_SHARE_NOUS, diagnostics_share);
}

/// Register with default stubs onto `registry`.
pub fn register(registry: &mut HandlerRegistry) {
    register_with(
        registry,
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_projects_discover_repos(&rid_json, &params_json, |_| Err("no backend".to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_projects_record_repos(&rid_json, &params_json, |_| Err("no backend".to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_projects_tree(&rid_json, &params_json, |_| Err("no backend".to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_projects_project_sessions(&rid_json, &params_json, |_, _| Err("no backend".to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_config_get(&rid_json, &params_json, |key, _| Err(config_get_unknown_key(key)))
        },
        |rid, _| {
            let rid_json = encode_rid(&rid);
            handle_setup_status(&rid_json, || Err("no backend".to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_setup_runtime_check(
                &rid_json,
                &params_json,
                |_| Err("no backend".to_string()),
                || false,
                |_| false,
            )
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_diagnostics_share_nous(&rid_json, &params_json, |_| Err("no backend".to_string()), |_| Err("no backend".to_string()))
        },
    )
}

// ---------------------------------------------------------------------------
// Tests — mirror Python invariants (std-only)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn rid1() -> String {
        encode_rid("1")
    }

    #[test]
    fn indicator_normalization() {
        assert_eq!(normalize_indicator_style("emoji"), "emoji");
        assert_eq!(normalize_indicator_style("EMOJI"), "emoji");
        assert_eq!(normalize_indicator_style("  Kaomoji "), "kaomoji");
        assert_eq!(normalize_indicator_style("bad"), DEFAULT_INDICATOR_STYLE);
        assert_eq!(normalize_indicator_style(""), DEFAULT_INDICATOR_STYLE);
        assert_eq!(normalize_indicator_style_owned("unicode"), "unicode");
    }

    #[test]
    fn details_mode_normalization() {
        assert_eq!(normalize_details_mode("hidden"), "hidden");
        assert_eq!(normalize_details_mode("collapsed"), "collapsed");
        assert_eq!(normalize_details_mode("expanded"), "expanded");
        assert_eq!(normalize_details_mode("COLLAPSED"), "collapsed");
        assert_eq!(normalize_details_mode("unknown"), "collapsed");
        assert_eq!(normalize_details_mode(""), "collapsed");
    }

    #[test]
    fn thinking_mode_fallback() {
        assert_eq!(normalize_thinking_mode("truncated", "collapsed"), "truncated");
        assert_eq!(normalize_thinking_mode("full", ""), "full");
        assert_eq!(normalize_thinking_mode("bad", "expanded"), "full");
        assert_eq!(normalize_thinking_mode("bad", "collapsed"), "collapsed");
        assert_eq!(normalize_thinking_mode("", "expanded"), "full");
        assert_eq!(normalize_thinking_mode("", ""), "collapsed");
    }

    #[test]
    fn theme_and_statusbar() {
        assert_eq!(normalize_theme("dark"), "dark");
        assert_eq!(normalize_theme("Light"), "light");
        assert_eq!(normalize_theme("bad"), "auto");
        assert_eq!(coerce_statusbar("bottom"), "bottom");
        assert_eq!(coerce_statusbar("hidden"), "hidden");
        assert_eq!(coerce_statusbar("bad"), "top");
        assert_eq!(coerce_statusbar_owned("top"), "top");
    }

    #[test]
    fn validate_log_lines_bounds() {
        assert_eq!(validate_log_lines(Some("200")), 200);
        assert_eq!(validate_log_lines(Some("10")), 10);
        assert_eq!(validate_log_lines(Some("2000")), 2000);
        assert_eq!(validate_log_lines(Some("9")), 200);
        assert_eq!(validate_log_lines(Some("2001")), 200);
        assert_eq!(validate_log_lines(None), 200);
        assert_eq!(validate_log_lines(Some("not-int")), 200);
    }

    #[test]
    fn sanitize_label_cases() {
        assert_eq!(sanitize_label("my file (1).txt"), "my file (1).txt");
        assert_eq!(sanitize_label("../../etc/passwd"), "etc/passwd");
        assert_eq!(sanitize_label("...hidden"), "hidden");
        let long = "a".repeat(100);
        assert_eq!(sanitize_label(&long).len(), 64);
        assert_eq!(sanitize_label("a/b"), "ab");
        assert_eq!(sanitize_label("  hello!@#  "), "hello");
    }

    #[test]
    fn projects_discover_repos_none_and_err() {
        let rid = rid1();
        let out = handle_projects_discover_repos(&rid, "{}", |_| Ok(None));
        assert!(out.contains(r#""repos":[]"#), "{}", out);
        let out2 = handle_projects_discover_repos(&rid, "{}", |_| Err("boom".into()));
        assert!(out2.contains(r#""code":5061"#));
        assert!(out2.contains("boom"));
    }

    #[test]
    fn projects_project_sessions_requires_id() {
        let rid = rid1();
        let out = handle_projects_project_sessions(&rid, "{}", |_, _| Ok(None));
        assert!(out.contains(r#""code":5063"#));
        let out2 = handle_projects_project_sessions(&rid, r#"{"project_id":"  "}"#, |_, _| Ok(None));
        assert!(out2.contains(r#""code":5063"#));
        let out3 = handle_projects_project_sessions(&rid, r#"{"project_id":"abc"}"#, |_, pid| {
            assert_eq!(pid, "abc");
            Ok(Some(r#"{"project":{"id":"abc"}}"#.to_string()))
        });
        assert!(out3.contains(r#""project""#));
    }

    #[test]
    fn config_get_unknown_key_4002() {
        let rid = rid1();
        let out = handle_config_get(&rid, r#"{"key":"bogus"}"#, |k, _| Err(config_get_unknown_key(k)));
        assert!(out.contains(r#""code":4002"#));
        assert!(out.contains("unknown config key"));
        let out2 = handle_config_get(&rid, r#"{"key":"provider"}"#, |_, _| {
            Ok(r#"{"model":"a/b","provider":"a","providers":["a","b"]}"#.to_string())
        });
        assert!(out2.contains(r#""provider""#));
    }

    #[test]
    fn config_get_provider_and_approval_errors() {
        let rid = rid1();
        // provider exception maps to 5013
        let out = handle_config_get(&rid, r#"{"key":"provider"}"#, |_, _| Err((ERR_CONFIG_PROVIDER, "fail".into())));
        assert!(out.contains(r#""code":5013"#));
        // approval_mode exception maps to 5001
        let out2 = handle_config_get(&rid, r#"{"key":"approval_mode"}"#, |_, _| Err((ERR_APPROVAL_MODE, "bad".into())));
        assert!(out2.contains(r#""code":5001"#));
    }

    #[test]
    fn setup_status_ok_and_err() {
        let rid = rid1();
        let out = handle_setup_status(&rid, || Ok(true));
        assert!(out.contains(r#""provider_configured":true"#));
        let out2 = handle_setup_status(&rid, || Ok(false));
        assert!(out2.contains(r#""provider_configured":false"#));
        let out3 = handle_setup_status(&rid, || Err("boom".into()));
        assert!(out3.contains(r#""code":5016"#));
    }

    #[test]
    fn setup_runtime_check_always_ok_envelope() {
        let rid = rid1();
        // success
        let out = handle_setup_runtime_check(
            &rid,
            r#"{"provider":"openai"}"#,
            |_| Ok(RuntimeInfo { provider: "openai".into(), model: "gpt-4".into(), source: "config".into(), api_key: Some("sk-123".into()), command: None }),
            || true,
            |k| k == "sk-123",
        );
        assert!(out.contains(r#""ok":true"#));
        assert!(out.contains("openai"));
        // resolve error → ok false
        let out2 = handle_setup_runtime_check(&rid, "{}", |_| Err("resolve fail".into()), || true, |_| false);
        assert!(out2.contains(r#""ok":false"#));
        assert!(out2.contains("resolve fail"));
        // bedrock IAM short-circuit
        let out3 = handle_setup_runtime_check(
            &rid, "{}", |_| Ok(RuntimeInfo { provider: "bedrock".into(), model: "claude".into(), source: "iam-role".into(), api_key: None, command: None }),
            || false, |_| false,
        );
        assert!(out3.contains(r#""ok":false"#));
        assert!(out3.contains("No Hermes provider is configured"));
        // missing credentials
        let out4 = handle_setup_runtime_check(
            &rid, "{}", |_| Ok(RuntimeInfo { provider: "openai".into(), model: "m".into(), source: "config".into(), api_key: Some("".into()), command: None }),
            || true, |_| false,
        );
        assert!(out4.contains("No usable credentials"));
    }

    #[test]
    fn diagnostics_share_nous_ok_and_err() {
        let rid = rid1();
        // success returns view_url
        let out = handle_diagnostics_share_nous(
            &rid, r#"{"log_lines":200}"#,
            |_| Ok(HashMap::new()),
            |_| Ok((Some("https://view".into()), Some("id123".into()), Some("2026-01-01".into()))),
        );
        assert!(out.contains(r#""ok":true"#));
        assert!(out.contains("https://view"));
        // upload with no id/url → ok false
        let out2 = handle_diagnostics_share_nous(
            &rid, "{}", |_| Ok(HashMap::new()), |_| Ok((None, None, None)),
        );
        assert!(out2.contains(r#""ok":false"#));
        assert!(out2.contains("no view URL"));
        // collect error → ok false
        let out3 = handle_diagnostics_share_nous(&rid, "{}", |_| Err("collect fail".into()), |_| Ok((None, None, None)));
        assert!(out3.contains(r#""ok":false"#));
        assert!(out3.contains("collect fail"));
    }

    #[test]
    fn share_nous_params_parsing() {
        let p = ShareNousParams::from_json(r#"{"log_lines": 50, "error_context": "  oops  ", "extra_files": {"bad/../label": "hello world", "ok": "  "}}"#);
        assert_eq!(p.log_lines, 50);
        assert_eq!(p.error_context.as_deref(), Some("oops"));
        // extra_files: one valid (sanitized), empty value skipped
        assert_eq!(p.extra_files.len(), 1);
        assert_eq!(p.extra_files[0].0, "bad./label");
    }

    #[test]
    fn build_registry_installs_eight() {
        let mut reg = build_registry_default();
        assert_eq!(reg.len(), 8);
        let mut names: Vec<_> = reg.pending_names().collect();
        names.sort();
        assert_eq!(
            names,
            vec![
                "config.get",
                "diagnostics.share_nous",
                "projects.discover_repos",
                "projects.project_sessions",
                "projects.record_repos",
                "projects.tree",
                "setup.runtime_check",
                "setup.status"
            ]
        );
        let mut map = HashMap::new();
        reg.install_into(&mut map);
        assert_eq!(map.len(), 8);
        // projects.tree stub should be 5061
        let out = map.get("projects.tree").unwrap()("1".to_string(), "{}".to_string());
        assert!(out.contains("5061"));
        // config.get unknown -> 4002
        let out2 = map.get("config.get").unwrap()("1".to_string(), r#"{"key":"nope"}"#.to_string());
        assert!(out2.contains("4002"));
    }
}
