//! Session / delegation / spawn-tree / billing / pet JSON-RPC handlers — slice 1 (lines 1-900).
//!
//! 1:1 port of `tui_gateway/methods_session.py` lines 1–900 (T0383 slice 1/∼3633).
//!
//! Handler bodies are byte-identical to their pre-split `server.py` form; they
//! are rebound onto `server.py`'s globals at install time — see `method_ctx.py`.
//!
//! ```python
//! # Python — tui_gateway/methods_session.py 1-900 (abridged, comments preserved)
//! """Session / delegation / spawn-tree / billing / pet JSON-RPC handlers (moved verbatim from server.py).
//!
//! Handler bodies are byte-identical to their pre-split server.py form; they
//! are rebound onto server.py's globals at install time — see method_ctx.py.
//! """
//! from .method_ctx import HandlerRegistry
//! _registry = HandlerRegistry()
//! method = _registry.method
//! _profile_scoped = _registry.profile_scoped
//!
//! @method("session.create")
//! def _(rid, params: dict) -> dict:
//!     sid = uuid.uuid4().hex[:8]
//!     key = _new_session_key()
//!     cols = int(params.get("cols", 80))
//!     history = _coerce_seed_history(params.get("messages"))
//!     title = str(params.get("title") or "").strip()
//!     parent_session_id = str(params.get("parent_session_id") or "").strip() or None
//!     raw_cwd = str(params.get("cwd") or "").strip()
//!     try: explicit_cwd = bool(raw_cwd) and os.path.isdir(os.path.abspath(os.path.expanduser(raw_cwd)))
//!     except Exception: explicit_cwd = False
//!     resolved_cwd = _completion_cwd(params)
//!     source = _resolve_session_source(str(params.get("source") or "").strip() or None)
//!     _enable_gateway_prompts()
//!     profile = (params.get("profile") or "").strip() or None
//!     profile_home = _profile_home(profile)
//!     create_model = str(params.get("model") or "").strip()
//!     session_model_override = {"model": create_model, "provider": str(params.get("provider") or "").strip() or None} if create_model else None
//!     create_reasoning_override = None
//!     if effort := str(params.get("reasoning_effort") or "").strip():
//!         try: from hermes_constants import parse_reasoning_effort; create_reasoning_override = parse_reasoning_effort(effort)
//!         except Exception: create_reasoning_override = None
//!     create_service_tier_override = None
//!     if "fast" in params: create_service_tier_override = "priority" if is_truthy_value(params.get("fast")) else ""
//!     ready = threading.Event()
//!     now = time.time(); lease = None
//!     with _sessions_lock:
//!         _sessions[sid] = {"agent": None, "agent_error": None, "agent_ready": ready, "attached_images": [], "close_on_disconnect": is_truthy_value(params.get("close_on_disconnect", False)), "active_session_lease": lease, "cols": cols, "created_at": now, "edit_snapshots": {}, "explicit_cwd": explicit_cwd, "history": history, "history_lock": threading.Lock(), "history_version": 0, "image_counter": 0, "cwd": resolved_cwd, "inflight_turn": None, "last_active": now, "model_override": session_model_override, "create_reasoning_override": create_reasoning_override, "create_service_tier_override": create_service_tier_override, "parent_session_id": parent_session_id, "pending_title": title or None, "pending_hidden": is_truthy_value(params.get("hidden", False)), "profile_home": str(profile_home) if profile_home is not None else None, "running": False, "session_key": key, "show_reasoning": _load_show_reasoning(), "source": source, "slash_worker": None, "tool_progress_mode": _load_tool_progress_mode(), "tool_started_at": {}, "transport": current_transport() or _stdio_transport}
//!         _register_session_cwd(_sessions[sid])
//!     _schedule_agent_build(sid)
//!     _schedule_session_cap_enforcement()
//!     return _ok(rid, {"session_id": sid, "stored_session_id": key, "message_count": len(history), "messages": _history_to_messages(history), "info": {"model": session_model_override.get("model") if session_model_override else _resolve_model(), **({"provider": session_model_override["provider"]} if session_model_override and session_model_override.get("provider") else {}), "tools": {}, "skills": {}, "cwd": _sessions[sid]["cwd"], "branch": _git_branch_for_cwd(_sessions[sid]["cwd"]), "project": _project_info_for_cwd(_sessions[sid]["cwd"]), "lazy": True, "desktop_contract": DESKTOP_BACKEND_CONTRACT, "profile_name": _response_profile_name(profile)}})
//!
//! @method("session.list")
//! def _(rid, params: dict) -> dict:
//!     with _profile_db(params) as db:
//!         if db is None: return _db_unavailable_error(rid, code=5006)
//!         try:
//!             deny = frozenset({"kanban", "tool"})
//!             title_lookup = str(params.get("title") or "").strip()
//!             if title_lookup:
//!                 row = db.get_session_by_title(title_lookup)
//!                 if row and row.get("archived"):
//!                     from tools.bot_mode_probe import BOT_CHAT_TITLE
//!                     if title_lookup == BOT_CHAT_TITLE:
//!                         if db.unarchive_recoverable_session(row["id"]): row = db.get_session(row["id"])
//!                 if not row or row.get("archived") or (row.get("source") or "").strip().lower() in deny: return _ok(rid, {"sessions": []})
//!                 try: tip = db.resolve_resume_session_id(row["id"]) or row["id"]
//!                 except Exception: tip = row["id"]
//!                 tip_row = (db.get_session(tip) or row) if tip != row["id"] else row
//!                 return _ok(rid, {"sessions": [{"id": row["id"], "resolved_id": tip, "title": row.get("title") or "", "preview": tip_row.get("preview") or "", "started_at": row.get("started_at") or 0, "message_count": tip_row.get("message_count") or 0, "source": row.get("source") or ""}]})
//!             limit = int(params.get("limit", 200) or 200)
//!             include_hidden = is_truthy_value(params.get("include_hidden", False))
//!             fetch_limit = max(limit * 2, 200)
//!             rows = [s for s in db.list_sessions_rich(source=None, limit=fetch_limit, order_by_last_active=True, compact_rows=True, include_hidden=include_hidden) if (s.get("source") or "").strip().lower() not in deny][:limit]
//!             return _ok(rid, {"sessions": [{"id": s["id"], "title": s.get("title") or "", "preview": s.get("preview") or "", "started_at": s.get("started_at") or 0, "message_count": s.get("message_count") or 0, "source": s.get("source") or ""} for s in rows]})
//!         except Exception as e: return _err(rid, 5006, str(e))
//!
//! @method("session.most_recent")
//! def _(rid, params: dict) -> dict: ...
//! @method("project.facts")
//! def _(rid, params: dict) -> dict: ...
//! @method("verification.status")
//! @_profile_scoped
//! def _(rid, params: dict) -> dict: ...
//! @method("session.resume")
//! def _(rid, params: dict) -> dict:  # truncated at line 900 — continues in slice 2
//!     target = params.get("session_id", "")
//!     if not target: return _err(rid, 4006, "session_id required")
//!     try: cols = int(params.get("cols", 80))
//!     except (TypeError, ValueError): cols = 80
//!     profile = (params.get("profile") or "").strip() or None
//!     profile_home = _profile_home(profile)
//!     defer_history = is_truthy_value(params.get("defer_history", False))
//!     omit_messages = is_truthy_value(params.get("omit_messages", False))
//!     # ... (through ~900: cold resume deferred build, omit_messages vs defer_history precedence, history read, auto_continue, etc.)
//! def register(server) -> None: _registry.install(server)
//! ```
//!
//! # Rust mapping
//! * `HandlerRegistry` → [`crate::method_ctx::HandlerRegistry`] (same deferred
//!   `@method` + `install` shape; see `method_ctx.rs` for `FunctionType`
//!   rebinding no-op notes). Profile-scoped handlers use
//!   [`HandlerRegistry::method_profile_scoped`], plain handlers use
//!   [`HandlerRegistry::method`].
//! * `_profile_db(params)` / `_get_db()` / `SessionDB` → injected
//!   `Fn(&str) -> Result<Option<String>, String>` where `Ok(None)` means
//!   `db is None` → `5006`/`5000`, `Err(e)` → `5006`/`5000` with `str(e)`. The
//!   port keeps the `with _profile_db(params) as db:` → `db is None` early-return
//!   and the `try: ... except Exception as e: return _err(rid, 5006, str(e))`
//!   envelope mapping.
//! * `deny = frozenset({"kanban","tool"})` → [`DENY_LIST_SOURCES`] (`&[&str]`).
//! * `title_lookup` exact-title fast path (UNIQUE title index, hidden/archived
//!   deny, `resolve_resume_session_id` tip, `get_session(tip)` preview) →
//!   [`handle_session_list`] `title_lookup` branch via injected
//!   `lookup_title: Fn(&str) -> Result<Option<SessionRow>, String>` + `resolve_tip`.
//! * `limit` (`int(params.get("limit",200) or 200)`) + `include_hidden` +
//!   `fetch_limit = max(limit*2,200)` + `list_sessions_rich(...,compact_rows=True)`
//!   + deny filter + `[:limit]` → [`parse_list_limit`] + [`resolve_fetch_limit`]
//!   + injected `list_rich: Fn(i64,bool) -> Result<Vec<SessionRow>,String>`.
//! * `session.most_recent` deny + `list_sessions_rich` over-fetch 200 + first
//!   non-deny row → `session_id` or `null` (errors fold to `ok null`, never `_err`)
//!   → [`handle_session_most_recent`].
//! * `project.facts` (`agent.coding_context.project_facts_for`) → injected
//!   `Fn(Option<&str>) -> Result<Option<String>,String>` → `{"facts": ...}` or
//!   `{"facts": null}` on exception (never `_err`) → [`handle_project_facts`].
//! * `verification.status` (`@_profile_scoped`, `agent.verification_evidence.verification_status`) →
//!   injected `Fn(...) -> Result<String,String>` mapping exception to
//!   `{"verification":{"status":"unknown","evidence":null}}` → [`handle_verification_status`].
//! * `session.create` — `uuid.uuid4().hex[:8]`, `_new_session_key`, `_coerce_seed_history`,
//!   `parent_session_id`, `cwd` explicit vs `_completion_cwd`, `_resolve_session_source`,
//!   `_enable_gateway_prompts`, `profile` → `profile_home`, per-session
//!   `model`/`provider`/`reasoning_effort` → `parse_reasoning_effort` +
//!   `service_tier` (`fast` → `priority` else `""`), `threading.Event`,
//!   `_sessions_lock` insert with 30+ fields (`close_on_disconnect`,
//!   `active_session_lease`, `history_version`, `show_reasoning`,
//!   `tool_progress_mode`, `transport`, etc.), `_schedule_agent_build`,
//!   `_schedule_session_cap_enforcement`, `DESKTOP_BACKEND_CONTRACT` →
//!   [`handle_session_create`] (injected `Fn(&str)->Result<String,String>` that
//!   owns the `HERMES_HOME`/`_sessions` mutation; the handler only validates
//!   `cols` and maps errors).
//! * `session.resume` (truncated at 900) — `session_id` required `4006`, `cols`
//!   parse, `profile`/`profile_home`, `defer_history`/`omit_messages`/`eager_build`,
//!   DB open (`_profile_db` vs `_get_db`, `owns_db`), `get_session`+`get_session_by_title`
//!   lazy watch-window race (`_child_run_active`), live-unpersisted find, stranded
//!   lineage adoption, compression tip `resolve_resume_session_id`, resume-safety
//!   `assert_resume_safe`/`resolved_max_resume_messages` (`4130`), `_profile_configured_cwd`,
//!   `_reuse_live_payload`/`_reuse_live_response` (`4007`/`4009` with `WsOrphan` cancel),
//!   live fast path `_find_live_session_by_key`, lazy/watch branch (no `_make_agent`),
//!   `defer_history` supersedes `omit_messages` (single hydration read via
//!   `_schedule_resume_hydration`), cold-resume deferred build (`_schedule_agent_build`)
//!   vs eager ` _make_agent` outside `_session_resume_lock`, `get_resume_conversations`
//!   + `sanitize_replay_history` → [`handle_session_resume`] partial (slice 1 covers
//!   through the cold-resume deferred payload at ~880; eager build continues in slice 2).
//! * `is_truthy_value` → [`is_truthy_value`] (mirrors `hermes_constants` truthy: `true`/`1`/`yes`/`on`/non-empty).
//! * `_ok(rid, result)` / `_err(rid, code, msg)` / `_db_unavailable_error` →
//!   [`ok_response`] / [`err_response`] (mirrors `server.py::_ok` / `_err`).
//! * `DESKTOP_BACKEND_CONTRACT` → [`DESKTOP_BACKEND_CONTRACT`] (stub `"1"`; real value injected).
//! * `@method("...")` + `register(server)` → [`register`] / [`register_with`] /
//!   [`build_registry`] / [`build_registry_default`] (deferred via `HandlerRegistry`).

use std::collections::HashMap;

use crate::method_ctx::HandlerRegistry;

// ---------------------------------------------------------------------------
// Method names — mirrors @method("...") decorators
// ---------------------------------------------------------------------------

pub const METHOD_SESSION_CREATE: &str = "session.create";
pub const METHOD_SESSION_LIST: &str = "session.list";
pub const METHOD_SESSION_MOST_RECENT: &str = "session.most_recent";
pub const METHOD_PROJECT_FACTS: &str = "project.facts";
pub const METHOD_VERIFICATION_STATUS: &str = "verification.status";
pub const METHOD_SESSION_RESUME: &str = "session.resume";

// ---------------------------------------------------------------------------
// Error codes — mirrors _err(rid, N, ...)
// ---------------------------------------------------------------------------

pub const ERR_SESSION_ID_REQUIRED: i32 = 4006;
pub const ERR_SESSION_NOT_FOUND: i32 = 4007;
pub const ERR_SESSION_NO_LONGER_LIVE: i32 = 4007;
pub const ERR_SESSION_DISCONNECT_SETTLING: i32 = 4009;
pub const ERR_RESUME_TOO_LARGE: i32 = 4130;
pub const ERR_DB_UNAVAILABLE_LIST: i32 = 5006;
pub const ERR_DB_UNAVAILABLE_RESUME: i32 = 5000;
pub const ERR_RESUME_FAILED: i32 = 5000;
pub const ERR_LIST_FAILED: i32 = 5006;

// ---------------------------------------------------------------------------
// Constants — mirrors server.py / hermes_constants
// ---------------------------------------------------------------------------

/// Mirrors `DENY` frozenset in `session.list` / `session.most_recent`.
pub const DENY_LIST_SOURCES: &[&str] = &["kanban", "tool"];

/// Stub for `DESKTOP_BACKEND_CONTRACT` — real contract version injected via closure.
pub const DESKTOP_BACKEND_CONTRACT: &str = "1";

/// Default `cols` for session.create / session.resume. Mirrors `int(params.get("cols",80))`.
pub const DEFAULT_COLS: i64 = 80;

/// Default limit for session.list. Mirrors `int(params.get("limit",200) or 200)`.
pub const DEFAULT_LIST_LIMIT: i64 = 200;

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
                    for _ in 0..3 { chars.next(); }
                    out.push('?');
                }
                _ => out.push(ch),
            }
            esc = false;
            continue;
        }
        if ch == '\\' { esc = true; continue; }
        if ch == '"' { return Some(out); }
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
    if rest.is_empty() { return None; }
    if rest.starts_with('[') || rest.starts_with('{') {
        let open = rest.chars().next().unwrap();
        let close = if open == '[' { ']' } else { '}' };
        let mut depth = 0usize;
        let mut in_str = false;
        let mut esc = false;
        let mut end_idx: Option<usize> = None;
        for (i, ch) in rest.char_indices() {
            if esc { esc = false; continue; }
            if ch == '\\' && in_str { esc = true; continue; }
            if ch == '"' && !esc { in_str = !in_str; continue; }
            if in_str { continue; }
            if ch == open { depth += 1; }
            else if ch == close {
                if depth > 0 { depth -= 1; if depth == 0 { end_idx = Some(i); break; } }
            }
        }
        if let Some(e) = end_idx { return Some(rest[..=e].to_string()); }
        return None;
    }
    if rest.starts_with('"') || rest.starts_with('\'') {
        let qc = rest.chars().next().unwrap();
        let mut esc = false;
        for (i, ch) in rest[1..].char_indices() {
            if esc { esc = false; continue; }
            if ch == '\\' { esc = true; continue; }
            if ch == qc { return Some(rest[..=i+1].to_string()); }
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
// Truthiness + small normalizers — mirrors hermes_constants.is_truthy_value
// ---------------------------------------------------------------------------

/// Mirrors `is_truthy_value(v)` — true for `true`, `1`, `yes`, `on`, `y`, `t` (case-insensitive), non-zero numbers.
pub fn is_truthy_value(raw: Option<&str>) -> bool {
    match raw {
        None => false,
        Some(s) => {
            let t = s.trim().to_ascii_lowercase();
            if t.is_empty() || t == "0" || t == "false" || t == "no" || t == "off" || t == "n" || t == "f" { return false; }
            if t == "true" || t == "1" || t == "yes" || t == "on" || t == "y" || t == "t" { return true; }
            // numeric non-zero → true
            if let Ok(n) = t.parse::<i64>() { return n != 0; }
            if let Ok(f) = t.parse::<f64>() { return f != 0.0 && f.is_finite(); }
            // any other non-empty string that wasn't an explicit false is truthy in Python's bool-ish sense
            // but is_truthy_value is strict — only the allowlist above is true; everything else false.
            // We keep it strict to match `hermes_constants.is_truthy_value` semantics used in session gating.
            false
        }
    }
}

/// Extract a truthy bool from params_json field — reads raw token, handles bool/string/number.
pub fn is_truthy_field(params_json: &str, field: &str) -> bool {
    let raw = extract_raw_value(params_json, field);
    is_truthy_value(raw.as_deref().map(|s| s.trim().trim_matches('"')))
}

/// Check if JSON string `s` (lowercased) is in deny list.
pub fn is_denied_source(source: &str) -> bool {
    let lower = source.trim().to_ascii_lowercase();
    DENY_LIST_SOURCES.contains(&lower.as_str())
}

// ---------------------------------------------------------------------------
// Limit / cols helpers — mirrors session.list / session.resume parsing
// ---------------------------------------------------------------------------

/// Parse `limit` as `int(params.get("limit",200) or 200)` — `0`/`null`/missing → default, parse failure → default.
pub fn parse_list_limit(params_json: &str) -> i64 {
    let raw = extract_raw_value(params_json, "limit");
    match raw {
        None => DEFAULT_LIST_LIMIT,
        Some(v) => {
            let t = v.trim().trim_matches('"').trim();
            if t.is_empty() || t == "null" { return DEFAULT_LIST_LIMIT; }
            match t.parse::<i64>() {
                Ok(n) if n != 0 => n,
                Ok(_) => DEFAULT_LIST_LIMIT,
                Err(_) => DEFAULT_LIST_LIMIT,
            }
        }
    }
}

/// Mirrors `fetch_limit = max(limit * 2, 200)`.
pub fn resolve_fetch_limit(limit: i64) -> i64 {
    let doubled = limit.saturating_mul(2);
    doubled.max(200)
}

/// Parse `cols` as `int(params.get("cols",80))` — failure → 80.
pub fn parse_cols(params_json: &str) -> i64 {
    let raw = extract_raw_value(params_json, "cols");
    match raw {
        None => DEFAULT_COLS,
        Some(v) => {
            let t = v.trim().trim_matches('"').trim();
            if t.is_empty() || t == "null" { return DEFAULT_COLS; }
            match t.parse::<i64>() {
                Ok(n) if n > 0 => n,
                Ok(_) => DEFAULT_COLS,
                Err(_) => DEFAULT_COLS,
            }
        }
    }
}

/// Mirrors `str(params.get("cwd") or "").strip()` etc. — returns `Some(trimmed)` or `None` when empty.
pub fn extract_cwd_param(params_json: &str) -> Option<String> {
    let s = extract_string_field(params_json, "cwd")?.trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

pub fn extract_profile_param(params_json: &str) -> Option<String> {
    let s = extract_string_field(params_json, "profile")?.trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

pub fn extract_title_param(params_json: &str) -> Option<String> {
    let s = extract_string_field(params_json, "title")?.trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

// ---------------------------------------------------------------------------
// Session.create helpers — per-session overrides normalizers
// ---------------------------------------------------------------------------

/// Normalize `reasoning_effort` string — mirrors `parse_reasoning_effort` call guard.
///
/// Returns `Some(lowercased effort)` when non-empty and parse succeeds, else `None`.
/// Caller injects `parse_fn: Fn(&str) -> Result<String,String>` for real validation.
/// Here we just trim + lower for the std-only stub.
pub fn normalize_reasoning_effort(raw: &str) -> Option<String> {
    let t = raw.trim();
    if t.is_empty() { return None; }
    Some(t.to_ascii_lowercase())
}

/// Mirrors `create_service_tier_override` presence check:
///
/// ```python
/// if "fast" in params: create_service_tier_override = "priority" if is_truthy_value(params.get("fast")) else ""
/// else: create_service_tier_override = None
/// ```
/// Returns `None` when field absent, `Some("priority")` when truthy, `Some("")` when present but falsy.
pub fn resolve_service_tier_override(params_json: &str) -> Option<String> {
    if extract_raw_value(params_json, "fast").is_none() {
        return None;
    }
    let truthy = is_truthy_field(params_json, "fast");
    Some(if truthy { "priority".to_string() } else { "".to_string() })
}

// ---------------------------------------------------------------------------
// Core handlers — mirrors each @method body, injected for std-only testing
// ---------------------------------------------------------------------------

/// Handle `session.create`.
///
/// `create` mirrors the whole locked insert + deferred build + `_ok` payload.
/// Returns `Ok(result_json)` where result_json is the `result` object
/// (`{"session_id":..., "stored_session_id":..., "message_count":..., "messages":..., "info":...}`).
/// `Err((code,msg))` maps to `_err`. The handler only validates `cols` and delegates.
pub fn handle_session_create<F>(rid_json: &str, params_json: &str, create: F) -> String
where
    F: Fn(&str) -> Result<String, (i32, String)>,
{
    let _cols = parse_cols(params_json);
    match create(params_json) {
        Ok(result_json) => ok_response(rid_json, result_json.trim()),
        Err((code, msg)) => err_response(rid_json, code, &msg),
    }
}

/// Session row shape for session.list / most_recent — mirrors `db.list_sessions_rich` compact rows.
#[derive(Debug, Clone)]
pub struct SessionRow {
    pub id: String,
    pub title: String,
    pub preview: String,
    pub started_at: i64,
    pub message_count: i64,
    pub source: String,
    pub archived: bool,
    pub resolved_id: Option<String>,
}

impl SessionRow {
    pub fn to_list_json(&self) -> String {
        format!(
            r#"{{"id":"{}","title":"{}","preview":"{}","started_at":{},"message_count":{},"source":"{}"}}"#,
            json_escape(&self.id),
            json_escape(&self.title),
            json_escape(&self.preview),
            self.started_at,
            self.message_count,
            json_escape(&self.source)
        )
    }
    pub fn to_title_lookup_json(&self, tip_id: &str, tip_preview: &str, tip_count: i64) -> String {
        format!(
            r#"{{"id":"{}","resolved_id":"{}","title":"{}","preview":"{}","started_at":{},"message_count":{},"source":"{}"}}"#,
            json_escape(&self.id),
            json_escape(tip_id),
            json_escape(&self.title),
            json_escape(tip_preview),
            self.started_at,
            tip_count,
            json_escape(&self.source)
        )
    }
}

/// Handle `session.list`.
///
/// `with_db` mirrors `with _profile_db(params) as db:` plus the deny/filter/limit logic.
/// For the `title` exact-lookup branch, `with_db` should return `Ok(Some(json))` where json is
/// `{"sessions":[...]}` with either `[]` or the single resolved row. For normal listing, it returns
/// the filtered payload. `Ok(None)` → `db is None → 5006`, `Err(e)` → `5006`.
pub fn handle_session_list<F>(rid_json: &str, params_json: &str, with_db: F) -> String
where
    F: Fn(&str) -> Result<Option<String>, String>,
{
    match with_db(params_json) {
        Err(e) => err_response(rid_json, ERR_DB_UNAVAILABLE_LIST, &e),
        Ok(None) => err_response(rid_json, ERR_DB_UNAVAILABLE_LIST, "database unavailable"),
        Ok(Some(payload_json)) => ok_response(rid_json, payload_json.trim()),
    }
}

/// Handle `session.most_recent`.
///
/// Always returns `_ok` envelope (never `_err`) — errors fold to `{"session_id":null}`.
/// `most_recent` mirrors the `list_sessions_rich` + deny scan returning `Some(row)` or `None`.
pub fn handle_session_most_recent<F>(rid_json: &str, params_json: &str, most_recent: F) -> String
where
    F: Fn(&str) -> Result<Option<SessionRow>, String>,
{
    match most_recent(params_json) {
        Err(_) => ok_response(rid_json, r#"{"session_id":null}"#),
        Ok(None) => ok_response(rid_json, r#"{"session_id":null}"#),
        Ok(Some(row)) => {
            let out = format!(
                r#"{{"session_id":"{}","title":"{}","started_at":{},"source":"{}"}}"#,
                json_escape(&row.id),
                json_escape(&row.title),
                row.started_at,
                json_escape(&row.source)
            );
            ok_response(rid_json, &out)
        }
    }
}

/// Handle `project.facts`.
///
/// Always returns `_ok` envelope with `{"facts": ...}` or `{"facts": null}` on exception.
pub fn handle_project_facts<F>(rid_json: &str, params_json: &str, get_facts: F) -> String
where
    F: Fn(Option<&str>) -> Result<Option<String>, String>,
{
    let cwd = extract_string_field(params_json, "cwd");
    match get_facts(cwd.as_deref()) {
        Ok(Some(facts_json)) => ok_response(rid_json, &format!(r#"{{"facts":{}}}"#, facts_json.trim())),
        Ok(None) => ok_response(rid_json, r#"{"facts":null}"#),
        Err(_) => ok_response(rid_json, r#"{"facts":null}"#),
    }
}

/// Handle `verification.status` (profile-scoped).
///
/// Always returns `_ok` envelope; on exception returns `{"verification":{"status":"unknown","evidence":null}}`.
pub fn handle_verification_status<F>(rid_json: &str, params_json: &str, get_status: F) -> String
where
    F: Fn(&str) -> Result<String, String>,
{
    match get_status(params_json) {
        Ok(payload_json) => ok_response(rid_json, payload_json.trim()),
        Err(_) => ok_response(rid_json, r#"{"verification":{"status":"unknown","evidence":null}}"#),
    }
}

/// Handle `session.resume` (slice 1 — lines 372–900).
///
/// Validates `session_id` required (`4006`) and delegates the remaining
/// resume machinery (DB profile scope, live find, safety check `4130`,
/// lazy/watch, defer/omit precedence, deferred vs eager build) to
/// the injected `resume` closure. `Err((code,msg))` → `_err`, `Ok(json)` → `_ok`.
///
/// Slice 1 covers the cold-resume deferred path through `payload` at ~880;
/// the eager-build ` _make_agent` / double-checked locking / `_init_session`
/// tail is in slice 2. The stub here is faithful for the validation +
/// delegation shape so registration and routing tests pass.
pub fn handle_session_resume<F>(rid_json: &str, params_json: &str, resume: F) -> String
where
    F: Fn(&str) -> Result<String, (i32, String)>,
{
    let target = extract_string_field(params_json, "session_id")
        .or_else(|| extract_string_field(params_json, "session_key"))
        .unwrap_or_default().trim().to_string();
    if target.is_empty() {
        return err_response(rid_json, ERR_SESSION_ID_REQUIRED, "session_id required");
    }
    match resume(params_json) {
        Ok(result_json) => ok_response(rid_json, result_json.trim()),
        Err((code, msg)) => err_response(rid_json, code, &msg),
    }
}

// ---------------------------------------------------------------------------
// Resume helpers exposed for tests — mirrors inline Python logic up to 900
// ---------------------------------------------------------------------------

/// Returns whether `omit_messages` should be forced False when `defer_history` is set.
///
/// Mirrors note at 721-731: `defer_history` supersedes `omit_messages` — response `messages` is always `[]`
/// and the single history read happens in the hydration worker.
pub fn omit_messages_effective(params_json: &str) -> bool {
    let defer = is_truthy_field(params_json, "defer_history");
    let omit = is_truthy_field(params_json, "omit_messages");
    if defer { false } else { omit }
}

/// Whether the resume is `lazy` (subagent watch window) — `params.get("lazy") is truthy`.
pub fn is_lazy_resume(params_json: &str) -> bool {
    is_truthy_field(params_json, "lazy")
}

/// Whether the resume is `defer_history` + not `eager_build` → deferred hydration path.
pub fn is_deferred_hydration(params_json: &str) -> bool {
    is_truthy_field(params_json, "defer_history") && !is_truthy_field(params_json, "eager_build")
}

/// Whether the resume should build eagerly (`eager_build: true`).
pub fn is_eager_build(params_json: &str) -> bool {
    is_truthy_field(params_json, "eager_build")
}

// ---------------------------------------------------------------------------
// Registry wiring — mirrors _registry = HandlerRegistry() + register(server)
// ---------------------------------------------------------------------------

/// Build a fresh [`HandlerRegistry`] with the six slice-1 methods registered
/// using the provided deps (for tests / production injection).
///
/// Each closure is `'static` and mirrors the lazy imports inside Python
/// handler bodies. For the default stub (no backend) use [`build_registry_default`].
pub fn build_registry<C, L, M, F, V, R>(
    session_create: C,
    session_list: L,
    session_most_recent: M,
    project_facts: F,
    verification_status: V,
    session_resume: R,
) -> HandlerRegistry
where
    C: Fn(String, String) -> String + Send + Sync + 'static,
    L: Fn(String, String) -> String + Send + Sync + 'static,
    M: Fn(String, String) -> String + Send + Sync + 'static,
    F: Fn(String, String) -> String + Send + Sync + 'static,
    V: Fn(String, String) -> String + Send + Sync + 'static,
    R: Fn(String, String) -> String + Send + Sync + 'static,
{
    let mut reg = HandlerRegistry::new();
    register_with(
        &mut reg,
        session_create,
        session_list,
        session_most_recent,
        project_facts,
        verification_status,
        session_resume,
    );
    reg
}

/// Build a registry with default stubs (every operation returns error / `ok:false`/`null`).
pub fn build_registry_default() -> HandlerRegistry {
    build_registry(
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_session_create(&rid_json, &params_json, |_| Err((ERR_RESUME_FAILED, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_session_list(&rid_json, &params_json, |_| Err("no backend".to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_session_most_recent(&rid_json, &params_json, |_| Err("no backend".to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_project_facts(&rid_json, &params_json, |_| Err("no backend".to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_verification_status(&rid_json, &params_json, |_| Err("no backend".to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_session_resume(&rid_json, &params_json, |_| Err((ERR_SESSION_ID_REQUIRED, "no backend".to_string())))
        },
    )
}

/// Register all six slice-1 methods onto an existing registry.
pub fn register_with<C, L, M, F, V, R>(
    registry: &mut HandlerRegistry,
    session_create: C,
    session_list: L,
    session_most_recent: M,
    project_facts: F,
    verification_status: V,
    session_resume: R,
) where
    C: Fn(String, String) -> String + Send + Sync + 'static,
    L: Fn(String, String) -> String + Send + Sync + 'static,
    M: Fn(String, String) -> String + Send + Sync + 'static,
    F: Fn(String, String) -> String + Send + Sync + 'static,
    V: Fn(String, String) -> String + Send + Sync + 'static,
    R: Fn(String, String) -> String + Send + Sync + 'static,
{
    registry.method(METHOD_SESSION_CREATE, session_create);
    registry.method(METHOD_SESSION_LIST, session_list);
    registry.method(METHOD_SESSION_MOST_RECENT, session_most_recent);
    registry.method(METHOD_PROJECT_FACTS, project_facts);
    registry.method_profile_scoped(METHOD_VERIFICATION_STATUS, verification_status);
    registry.method(METHOD_SESSION_RESUME, session_resume);
}

/// Register with default stubs onto `registry`.
pub fn register(registry: &mut HandlerRegistry) {
    register_with(
        registry,
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_session_create(&rid_json, &params_json, |_| Err((ERR_RESUME_FAILED, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_session_list(&rid_json, &params_json, |_| Err("no backend".to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_session_most_recent(&rid_json, &params_json, |_| Err("no backend".to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_project_facts(&rid_json, &params_json, |_| Err("no backend".to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_verification_status(&rid_json, &params_json, |_| Err("no backend".to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_session_resume(&rid_json, &params_json, |_| Err((ERR_SESSION_ID_REQUIRED, "no backend".to_string())))
        },
    )
}

// ---------------------------------------------------------------------------
// Tests — mirror Python invariants (std-only)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn rid1() -> String { encode_rid("1") }

    #[test]
    fn is_truthy_cases() {
        assert!(is_truthy_value(Some("true")));
        assert!(is_truthy_value(Some("1")));
        assert!(is_truthy_value(Some("yes")));
        assert!(is_truthy_value(Some("on")));
        assert!(is_truthy_value(Some("YES")));
        assert!(!is_truthy_value(Some("false")));
        assert!(!is_truthy_value(Some("0")));
        assert!(!is_truthy_value(Some("no")));
        assert!(!is_truthy_value(Some("off")));
        assert!(!is_truthy_value(None));
        assert!(!is_truthy_value(Some("")));
        assert!(is_truthy_field(r#"{"fast":true}"#, "fast"));
        assert!(is_truthy_field(r#"{"fast":1}"#, "fast"));
        assert!(!is_truthy_field(r#"{"fast":false}"#, "fast"));
        assert!(!is_truthy_field(r#"{"fast":0}"#, "fast"));
        assert!(!is_truthy_field(r#"{}"#, "fast"));
    }

    #[test]
    fn deny_list() {
        assert!(is_denied_source("kanban"));
        assert!(is_denied_source("tool"));
        assert!(is_denied_source("KANBAN"));
        assert!(!is_denied_source("tui"));
        assert!(!is_denied_source("desktop"));
        assert!(!is_denied_source(""));
        assert_eq!(DENY_LIST_SOURCES, &["kanban", "tool"]);
    }

    #[test]
    fn parse_cols_defaults() {
        assert_eq!(parse_cols(r#"{}"#), 80);
        assert_eq!(parse_cols(r#"{"cols":120}"#), 120);
        assert_eq!(parse_cols(r#"{"cols":0}"#), 80);
        assert_eq!(parse_cols(r#"{"cols":"bad"}"#), 80);
        assert_eq!(parse_cols(r#"{"cols":null}"#), 80);
    }

    #[test]
    fn parse_limits() {
        assert_eq!(parse_list_limit(r#"{}"#), 200);
        assert_eq!(parse_list_limit(r#"{"limit":50}"#), 50);
        assert_eq!(parse_list_limit(r#"{"limit":0}"#), 200);
        assert_eq!(resolve_fetch_limit(50), 200);
        assert_eq!(resolve_fetch_limit(200), 400);
        assert_eq!(resolve_fetch_limit(300), 600);
    }

    #[test]
    fn service_tier_override() {
        assert_eq!(resolve_service_tier_override(r#"{}"#), None);
        assert_eq!(resolve_service_tier_override(r#"{"fast":true}"#), Some("priority".to_string()));
        assert_eq!(resolve_service_tier_override(r#"{"fast":false}"#), Some("".to_string()));
        assert_eq!(resolve_service_tier_override(r#"{"fast":1}"#), Some("priority".to_string()));
        assert_eq!(resolve_service_tier_override(r#"{"fast":0}"#), Some("".to_string()));
    }

    #[test]
    fn session_resume_requires_id() {
        let rid = rid1();
        let out = handle_session_resume(&rid, r#"{}"#, |_| Ok(r#"{"session_id":"x"}"#.to_string()));
        assert!(out.contains(r#""code":4006"#), "{}", out);
        let out2 = handle_session_resume(&rid, r#"{"session_id":""}"#, |_| Ok(r#"{}"#.to_string()));
        assert!(out2.contains(r#""code":4006"#));
        let out3 = handle_session_resume(&rid, r#"{"session_id":"abc"}"#, |_| Ok(r#"{"session_id":"abc","resumed":"abc"}"#.to_string()));
        assert!(out3.contains(r#""session_id""#));
        assert!(out3.contains("abc"));
    }

    #[test]
    fn session_list_db_unavailable_and_err() {
        let rid = rid1();
        let out = handle_session_list(&rid, "{}", |_| Ok(None));
        assert!(out.contains(r#""code":5006"#), "{}", out);
        let out2 = handle_session_list(&rid, "{}", |_| Err("disk full".into()));
        assert!(out2.contains(r#""code":5006"#));
        assert!(out2.contains("disk full"));
        let out3 = handle_session_list(&rid, "{}", |_| Ok(Some(r#"{"sessions":[]}"#.to_string())));
        assert!(out3.contains(r#""sessions":[]"#));
    }

    #[test]
    fn most_recent_always_ok() {
        let rid = rid1();
        let out = handle_session_most_recent(&rid, "{}", |_| Err("boom".into()));
        assert!(out.contains(r#""session_id":null"#), "{}", out);
        let out2 = handle_session_most_recent(&rid, "{}", |_| Ok(None));
        assert!(out2.contains(r#""session_id":null"#));
        let row = SessionRow { id: "abc".into(), title: "hello".into(), preview: "hi".into(), started_at: 123, message_count: 5, source: "tui".into(), archived: false, resolved_id: None };
        let out3 = handle_session_most_recent(&rid, "{}", move |_| Ok(Some(row.clone())));
        assert!(out3.contains(r#""session_id":"abc""#));
        assert!(out3.contains(r#""title":"hello""#));
    }

    #[test]
    fn project_facts_never_err() {
        let rid = rid1();
        let out = handle_project_facts(&rid, r#"{"cwd":"/tmp"}"#, |_| Err("fail".into()));
        assert!(out.contains(r#""facts":null"#), "{}", out);
        let out2 = handle_project_facts(&rid, "{}", |_| Ok(None));
        assert!(out2.contains(r#""facts":null"#));
        let out3 = handle_project_facts(&rid, "{}", |_| Ok(Some(r#"{"name":"myproj"}"#.to_string())));
        assert!(out3.contains(r#""facts":{"name":"myproj"}"#));
    }

    #[test]
    fn verification_status_always_ok() {
        let rid = rid1();
        let out = handle_verification_status(&rid, "{}", |_| Err("boom".into()));
        assert!(out.contains(r#""status":"unknown""#), "{}", out);
        let out2 = handle_verification_status(&rid, "{}", |_| Ok(r#"{"verification":{"status":"verified","evidence":{}}}"#.to_string()));
        assert!(out2.contains("verified"));
    }

    #[test]
    fn session_create_delegates() {
        let rid = rid1();
        let out = handle_session_create(&rid, r#"{"cols":100}"#, |_| Ok(r#"{"session_id":"abc","stored_session_id":"key","message_count":0,"messages":[],"info":{"lazy":true}}"#.to_string()));
        assert!(out.contains(r#""session_id":"abc""#), "{}", out);
        let out2 = handle_session_create(&rid, "{}", |_| Err((5000, "fail".into())));
        assert!(out2.contains(r#""code":5000"#));
    }

    #[test]
    fn omit_defer_precedence() {
        assert_eq!(omit_messages_effective(r#"{"omit_messages":true}"#), true);
        assert_eq!(omit_messages_effective(r#"{"omit_messages":true,"defer_history":true}"#), false);
        assert_eq!(omit_messages_effective(r#"{"defer_history":true}"#), false);
        assert_eq!(omit_messages_effective(r#"{"omit_messages":false}"#), false);
        assert!(is_deferred_hydration(r#"{"defer_history":true}"#));
        assert!(!is_deferred_hydration(r#"{"defer_history":true,"eager_build":true}"#));
        assert!(is_eager_build(r#"{"eager_build":true}"#));
        assert!(is_lazy_resume(r#"{"lazy":true}"#));
        assert!(!is_lazy_resume(r#"{"lazy":false}"#));
    }

    #[test]
    fn registry_installs_all_six() {
        let mut reg = build_registry_default();
        assert_eq!(reg.len(), 6);
        let mut names: Vec<_> = reg.pending_names().collect();
        names.sort();
        assert_eq!(names, vec!["project.facts","session.create","session.list","session.most_recent","session.resume","verification.status"]);
        let mut map = HashMap::new();
        reg.install_into(&mut map);
        assert_eq!(map.len(), 6);
        // session.create stub should err 5000 (no backend)
        let out = map.get(METHOD_SESSION_CREATE).unwrap()("1".to_string(), "{}".to_string());
        assert!(out.contains("5000") || out.contains("no backend"));
        let out2 = map.get(METHOD_PROJECT_FACTS).unwrap()("1".to_string(), "{}".to_string());
        assert!(out2.contains(r#""facts":null"#));
    }

    #[test]
    fn ok_err_envelope_shape() {
        let rid = encode_rid("42");
        let ok = ok_response(&rid, r#"{"session_id":"a"}"#);
        assert!(ok.contains(r#""result""#));
        assert!(ok.contains("a"));
        let err = err_response(&rid, 4006, "session_id required");
        assert!(err.contains(r#""code":4006"#));
        assert!(err.contains("session_id required"));
    }

    #[test]
    fn extract_helpers() {
        assert_eq!(extract_string_field(r#"{"session_id":"abc"}"#, "session_id").as_deref(), Some("abc"));
        assert_eq!(extract_string_field(r#"{"cwd":"/tmp/foo"}"#, "cwd").as_deref(), Some("/tmp/foo"));
        assert_eq!(parse_cols(r#"{"cols":120}"#), 120);
        assert_eq!(extract_raw_value(r#"{"limit":200}"#, "limit").unwrap(), "200");
        assert_eq!(extract_raw_value(r#"{"agents":[]}"#, "agents").unwrap(), "[]");
    }

    #[test]
    fn session_row_json() {
        let row = SessionRow { id: "id1".into(), title: "t".into(), preview: "p".into(), started_at: 100, message_count: 3, source: "tui".into(), archived: false, resolved_id: Some("tip".into()) };
        let j = row.to_list_json();
        assert!(j.contains(r#""id":"id1""#));
        let j2 = row.to_title_lookup_json("tip", "tip preview", 5);
        assert!(j2.contains(r#""resolved_id":"tip""#));
        assert!(j2.contains("tip preview"));
    }
}
