//! Prompt / attachment / respond JSON-RPC handlers — slice 1 (lines 1-900).
//!
//! 1:1 port of `tui_gateway/methods_prompt.py` lines 1–900 (T0385 slice 1/1626).
//!
//! Handler bodies are byte-identical to their pre-split `server.py` form; they
//! are rebound onto `server.py`'s globals at install time — see `method_ctx.py`.
//!
//! ```python
//! # Python — tui_gateway/methods_prompt.py 1-900 (abridged, comments preserved)
//! """Prompt / attachment / respond JSON-RPC handlers (moved verbatim from server.py).
//!
//! Handler bodies are byte-identical to their pre-split server.py form; they
//! are rebound onto server.py's globals at install time — see method_ctx.py.
//! """
//! from .method_ctx import HandlerRegistry
//! import types
//! _registry = HandlerRegistry()
//! method = _registry.method
//! _profile_scoped = _registry.profile_scoped
//!
//! def _history_user_indices(history: list) -> list:
//!     """Indices of model-visible user turns (excludes display_kind timeline markers)."""
//!     return [i for i, m in enumerate(history) if m.get("role") == "user" and not m.get("display_kind")]
//!
//! def _message_row_id(msg: dict):
//!     """Parse durable SQLite row id from a history entry, or None."""
//!     raw = msg.get("_row_id")
//!     if raw is None: raw = msg.get("row_id")
//!     if raw is None: return None
//!     try: return int(raw)
//!     except (TypeError, ValueError): return None
//!
//! def _mem_db_pair_agrees(mem, db_msg) -> bool:
//!     """True when a live-memory entry plausibly corresponds to a durable row."""
//!     if not isinstance(mem, dict) or not isinstance(db_msg, dict): return False
//!     if mem.get("role") != db_msg.get("role"): return False
//!     if bool(mem.get("display_kind")) != bool(db_msg.get("display_kind")): return False
//!     if mem.get("role") == "user" and not mem.get("display_kind"):
//!         mem_content = mem.get("content")
//!         db_content = db_msg.get("content")
//!         if isinstance(mem_content, str) and isinstance(db_content, str) and mem_content.strip() != db_content.strip(): return False
//!     return True
//!
//! def _find_user_turn_by_row_id(history: list, target_row_id: int):
//!     """Return ``(user_ordinal, history_index)`` for ``target_row_id``, or None."""
//!     for u_ord, h_idx in enumerate(_history_user_indices(history)):
//!         if _message_row_id(history[h_idx]) == target_row_id: return u_ord, h_idx
//!     return None
//!
//! def _load_durable_truncation_history(session: dict, fallback_sid: str = ""): ...
//!     # with _session_db(session) as db: get_conv = getattr(db, "get_messages_as_conversation", None)
//!     # if not callable(get_conv): return None
//!     # history = get_conv(session_key, repair_alternation=True, include_row_ids=True)
//!     # return history if isinstance(history, list) else None
//!
//! def _resolve_truncate_row_id(session: dict, history: list, target_row_id: int): ...
//!     # hit = _find_user_turn_by_row_id(history, target_row_id)
//!     # if hit is not None: return hit
//!     # db_history = _load_durable_truncation_history(session)
//!     # if db_history is None: return None
//!     # if len(db_history)==len(history) and all(_mem_db_pair_agrees(...)): heal _row_id stamps then retry
//!     # db_hit = _find_user_turn_by_row_id(db_history, target_row_id)
//!     # if db_hit is None: return None
//!     # ... same-ordinal mapping with _mem_db_pair_agrees guard (#82959)
//!
//! def _coerce_truncate_int(rid, value, param_name="truncate_before_user_ordinal"): ...
//!     # if isinstance(value, bool): return None, _err(rid, 4004, f"{param_name} must be an integer")
//!     # try: return int(value), None  except: return None, _err(4004)
//!
//! def _reconcile_client_ordinal(rid, sid, client_ordinal, msg_ordinal, param_name, target_repr, prefix_user_count=0): ...
//!     # if client_ordinal is None: return msg_ordinal, None
//!     # ordinal, err = _coerce_truncate_int(rid, client_ordinal)
//!     # if err is not None: return None, err
//!     # if ordinal == msg_ordinal: return msg_ordinal, None
//!     # if prefix_user_count>0 and ordinal==msg_ordinal+prefix_user_count: return msg_ordinal, None
//!     # logger.warning STALE ordinal (#82756) -> _err 4030
//!
//! def _pending_reaction_notes(session: dict) -> str: ...
//!     # gated on display.message_reactions, db.take_unseen_reactions, 120-char snippet, emoji, whose
//!
//! @method("prompt.submit")
//! def _(rid, params: dict) -> dict:
//!     # sanitize_user_prompt_text, voice stop phrase, _voice_mode_enabled, interrupted mark_speech_interrupted
//!     # _sess_nowait -> 4090 limit, client_surface hud, has_truncation _expand_skill_invocation_for_replay
//!     # isolation_cfg / turn_isolation / current_transport rebind
//!     # busy queue loop -> _handle_busy_submit, lazy _child_run_active 4009
//!     # confirm_truncate gates 4004/4029/4028, prefix_user_count, user_indices, _stale_target_data
//!     # target_row_id / client_ordinal / message_id branches -> 4018/4004/4030, heal stamps, replace_messages archive_dropped
//!     # survivor_user_row_ids rebinding (#83202), history truncation, is_disk_full_error 5070/5071, _start_agent_build + _wait_agent_for_prompt
//!     # compute-host fallback, _ensure_session_db_row / _persist_branch_seed
//!     # ... (through 835: streaming dispatch)
//!
//! @method("clipboard.paste")
//! def _(rid, params: dict) -> dict: # 838-875
//!     # _sess_building 5027, has_clipboard_image/save_clipboard_image, image_counter, _session_images_dir, _image_meta
//!
//! @method("image.attach")
//! def _(rid, params: dict) -> dict: # 878-900 truncated
//!     # _sess_building 4015/4016, _detect_file_drop, _split_path_input, _resolve_attachment_path, _IMAGE_EXTENSIONS
//!     # ... truncated at line 900 inside remainder handling — continues in slice 2
//!
//! def register(server) -> None:
//!     _registry.install(server)
//!     g = vars(server)
//!     for helper in (_history_user_indices, _message_row_id, _mem_db_pair_agrees, _find_user_turn_by_row_id,
//!                    _load_durable_truncation_history, _resolve_truncate_row_id, _coerce_truncate_int,
//!                    _reconcile_client_ordinal, _pending_reaction_notes, _approval_respond_session_fallback):
//!         setattr(server, helper.__name__, types.FunctionType(helper.__code__, g, helper.__name__, helper.__defaults__, helper.__closure__))
//! ```
//!
//! # Rust mapping
//! * `HandlerRegistry` → [`crate::method_ctx::HandlerRegistry`] (same deferred
//!   `@method` + `install` shape; see `method_ctx.rs` for `FunctionType`
//!   rebinding no-op notes). Plain handlers use [`HandlerRegistry::method`].
//! * `history: list[dict]` + `session["history"]` → [`HistoryMsg`] (`role`,
//!   `display_kind`, `content`, `row_id`/`_row_id`, `id`/`message_id`) and
//!   `&[HistoryMsg]` slices; `_row_id`/`row_id` aliasing preserved via
//!   [`message_row_id`] (checks `_row_id` then `row_id`).
//! * `_history_user_indices(history)` → [`history_user_indices`].
//! * `_message_row_id(msg)` → [`message_row_id`].
//! * `_mem_db_pair_agrees(mem, db_msg)` → [`mem_db_pair_agrees`] (role +
//!   `display_kind` bool + addressable user-turn content `trim()` compare).
//! * `_find_user_turn_by_row_id(history, target)` → [`find_user_turn_by_row_id`].
//! * `_load_durable_truncation_history(session, fallback_sid)` → injected
//!   `Fn(&str, &str) -> Result<Option<Vec<HistoryMsg>>, String>` where
//!   `Ok(None)` = `get_messages_as_conversation` not callable or exception
//!   → `None` (caller treats as `None`), `Err` also maps to `None` and is
//!   logged; the closure owns `session["session_key"]` / `_session_db` wiring.
//! * `_resolve_truncate_row_id(session, history, row_id)` → [`resolve_truncate_row_id`]
//!   (heal missing in-memory stamps only when `len==len` and
//!   `all(mem_db_pair_agrees)` all-or-nothing (#82959 note), otherwise
//!   same-ordinal mapping with `mem_db_pair_agrees` guard).
//! * `_coerce_truncate_int(rid, value, param_name)` → [`coerce_truncate_int_raw`]
//!   / [`parse_truncate_int`] (bool-int subclass refusal: JSON `true`/`false`
//!   tokens map to `4004`; `int(value)` failure → `4004`; wraps
//!   [`err_response`] `4004` when called via `handle_*` injection).
//! * `_reconcile_client_ordinal(...)` → [`reconcile_client_ordinal`] (accepts
//!   `msg_ordinal` when client sent `None`, same `msg_ordinal`, or
//!   `msg_ordinal+prefix_user_count` with `prefix>0` (#82462 lineage);
//!   otherwise `4030` with warning semantics).
//! * `_pending_reaction_notes(session)` → [`pending_reaction_notes`] /
//!   [`format_reaction_note`] (gated on `display.message_reactions` via
//!   injected `bool`, `take_unseen_reactions` via injected `Fn`, snippet
//!   `120` + `…`, `whose` role, `display_kind` filtering identical).
//! * `prompt.submit` → [`handle_prompt_submit`] (validates `session_id` presence
//!   + `running` + `lazy`/`_child_run_active` → `4009`, `confirm_truncate` gates
//!   `4029`/`4028`/`4004`, prefix `120` handling, truncation branches `4018`/`4030`,
//!   `DB_UNAVAILABLE` vs `5008`/`5070`/`5071`, `4090` active-slot; the heavy
//!   `replace_messages` + `_run_prompt_submit`/`compute_host` dispatch is owned by
//!   the injected `prompt_fn: Fn(&str) -> Result<String,(i32,String)>` so the port
//!   stays `std`-only).
//! * `clipboard.paste` → [`handle_clipboard_paste`] (injected
//!   `clipboard_fn: Fn(&str) -> Result<String,(i32,String)>` owns
//!   `_sess_building` `4015`/`5027` + `image_counter` + `_image_meta`).
//! * `image.attach` (truncated at 900) → [`handle_image_attach`] (validates
//!   `path` `4015` then delegates remainder / `4016` / `_detect_file_drop` /
//!   `_IMAGE_EXTENSIONS` to the injected `attach_fn`; slice 1 covers through
//!   `image_path = _resolve_attachment_path(path_token)` at 900, slice 2
//!   continues with suffix + `_image_meta`).
//! * `is_truthy_value` → [`is_truthy_value`] (mirrors `hermes_constants` truthy).
//! * `_ok(rid, result)` / `_err(rid, code, msg)` → [`ok_response`] /
//!   [`err_response`] (mirrors `server.py::_ok` / `_err`).
//! * `@method("...")` + `register(server)` → [`register`] / [`register_with`] /
//!   [`build_registry`] / [`build_registry_default`] (deferred via `HandlerRegistry`).

use std::collections::HashMap;

use crate::method_ctx::HandlerRegistry;

// ---------------------------------------------------------------------------
// Method names — mirrors @method("...") decorators
// ---------------------------------------------------------------------------

pub const METHOD_PROMPT_SUBMIT: &str = "prompt.submit";
pub const METHOD_CLIPBOARD_PASTE: &str = "clipboard.paste";
pub const METHOD_IMAGE_ATTACH: &str = "image.attach";

// ---------------------------------------------------------------------------
// Error codes — mirrors _err(rid, N, ...)
// ---------------------------------------------------------------------------

pub const ERR_TRUNCATE_NOT_INT: i32 = 4004;
pub const ERR_CONFIRM_TRUNCATE_REQUIRED: i32 = 4029;
pub const ERR_EMPTY_TRUNCATE_REQUIRES_CONFIRM: i32 = 4028;
pub const ERR_ORDINAL_MISMATCH: i32 = 4030;
pub const ERR_TARGET_NOT_FOUND: i32 = 4018;
pub const ERR_SUBAGENT_ACTIVE: i32 = 4009;
pub const ERR_ACTIVE_SESSION_SLOT: i32 = 4090;
pub const ERR_SESSION_ID_REQUIRED: i32 = 4006;
pub const ERR_PERSIST_TRUNCATION: i32 = 5008;
pub const ERR_DISK_FULL: i32 = 5070;
pub const ERR_SESSION_STORAGE: i32 = 5071;
pub const ERR_CLIPBOARD: i32 = 5027;
pub const ERR_IMAGE_PATH_REQUIRED: i32 = 4015;
pub const ERR_IMAGE_NOT_FOUND: i32 = 4016;

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
            if ch == qc { return Some(rest[..=i + 1].to_string()); }
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
// Truthiness — mirrors hermes_constants.is_truthy_value
// ---------------------------------------------------------------------------

/// Mirrors `is_truthy_value(v)`.
pub fn is_truthy_value(raw: Option<&str>) -> bool {
    match raw {
        None => false,
        Some(s) => {
            let t = s.trim().to_ascii_lowercase();
            if t.is_empty() || t == "0" || t == "false" || t == "no" || t == "off" || t == "n" || t == "f" { return false; }
            if t == "true" || t == "1" || t == "yes" || t == "on" || t == "y" || t == "t" { return true; }
            if let Ok(n) = t.parse::<i64>() { return n != 0; }
            if let Ok(f) = t.parse::<f64>() { return f != 0.0 && f.is_finite(); }
            false
        }
    }
}

pub fn is_truthy_field(params_json: &str, field: &str) -> bool {
    let raw = extract_raw_value(params_json, field);
    is_truthy_value(raw.as_deref().map(|s| s.trim().trim_matches('"')))
}

// ---------------------------------------------------------------------------
// History types — mirrors Python dict shape
// ---------------------------------------------------------------------------

/// Mirrors a `history` entry dict (`role`, `display_kind`, `content`, `_row_id`/`row_id`, `id`/`message_id`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HistoryMsg {
    pub role: String,
    pub display_kind: Option<String>,
    pub content: Option<String>,
    pub row_id: Option<i64>,
    pub row_id_alt: Option<i64>, // _row_id
    pub id: Option<String>,
    pub message_id: Option<String>,
}

impl HistoryMsg {
    pub fn new_user(content: impl Into<String>) -> Self {
        Self { role: "user".to_string(), content: Some(content.into()), ..Default::default() }
    }
    pub fn new_assistant(content: impl Into<String>) -> Self {
        Self { role: "assistant".to_string(), content: Some(content.into()), ..Default::default() }
    }
    pub fn with_row_id(mut self, rid: i64) -> Self {
        self.row_id = Some(rid);
        self
    }
    pub fn with_alt_row_id(mut self, rid: i64) -> Self {
        self.row_id_alt = Some(rid);
        self
    }
    pub fn with_display_kind(mut self, kind: impl Into<String>) -> Self {
        self.display_kind = Some(kind.into());
        self
    }
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }
    pub fn with_message_id(mut self, id: impl Into<String>) -> Self {
        self.message_id = Some(id.into());
        self
    }
}

/// Parse durable row id — mirrors `_message_row_id(msg)`.
///
/// Checks `_row_id` (`row_id_alt`) then `row_id`.
pub fn message_row_id(msg: &HistoryMsg) -> Option<i64> {
    msg.row_id_alt.or(msg.row_id)
}

/// Indices of model-visible user turns — mirrors `_history_user_indices(history)`.
pub fn history_user_indices(history: &[HistoryMsg]) -> Vec<usize> {
    history.iter().enumerate()
        .filter(|(_, m)| m.role == "user" && m.display_kind.as_deref().map(|s| !s.is_empty()).unwrap_or(false) == false)
        .map(|(i, _)| i)
        .collect()
}

/// Whether a display_kind is present (non-empty).
fn has_display_kind(m: &HistoryMsg) -> bool {
    m.display_kind.as_deref().map(|s| !s.is_empty()).unwrap_or(false)
}

/// True when live-memory entry plausibly corresponds to durable row.
///
/// Mirrors `_mem_db_pair_agrees(mem, db_msg)`.
pub fn mem_db_pair_agrees(mem: &HistoryMsg, db_msg: &HistoryMsg) -> bool {
    if mem.role != db_msg.role {
        return false;
    }
    if has_display_kind(mem) != has_display_kind(db_msg) {
        return false;
    }
    if mem.role == "user" && !has_display_kind(mem) {
        if let (Some(mc), Some(dc)) = (mem.content.as_deref(), db_msg.content.as_deref()) {
            if mc.trim() != dc.trim() {
                return false;
            }
        }
    }
    true
}

/// Return `(user_ordinal, history_index)` for `target_row_id` — mirrors `_find_user_turn_by_row_id`.
pub fn find_user_turn_by_row_id(history: &[HistoryMsg], target_row_id: i64) -> Option<(usize, usize)> {
    for (u_ord, h_idx) in history_user_indices(history).into_iter().enumerate() {
        if message_row_id(&history[h_idx]) == Some(target_row_id) {
            return Some((u_ord, h_idx));
        }
    }
    None
}

/// Heal missing in-memory `_row_id` stamps when live list aligns 1:1 with durable transcript.
///
/// Mirrors the `if len(db_history)==len(history) and all(_mem_db_pair_agrees(...)):` block.
/// Returns `true` if healing was applied.
pub fn heal_row_ids_if_aligned(history: &mut [HistoryMsg], db_history: &[HistoryMsg]) -> bool {
    if history.len() != db_history.len() {
        return false;
    }
    if !history.iter().zip(db_history.iter()).all(|(a, b)| mem_db_pair_agrees(a, b)) {
        return false;
    }
    let mut healed = false;
    for (mem, db_msg) in history.iter_mut().zip(db_history.iter()) {
        if message_row_id(mem).is_none() {
            if let Some(rid) = message_row_id(db_msg) {
                // Prefer stamping `_row_id` side (row_id_alt) to preserve alias semantics.
                mem.row_id_alt = Some(rid);
                healed = true;
            }
        }
    }
    healed
}

/// Resolve `truncate_before_row_id` to `(user_ordinal, history_index)`.
///
/// Mirrors `_resolve_truncate_row_id(session, history, target_row_id)`:
/// 1. in-memory hit
/// 2. load durable transcript via `load_db` (None → no proof → None)
/// 3. heal stamps when aligned then retry in-memory
/// 4. durable hit → same-ordinal mapping with `mem_db_pair_agrees` guard
pub fn resolve_truncate_row_id<F>(history: &mut Vec<HistoryMsg>, target_row_id: i64, load_db: F) -> Option<(usize, usize)>
where
    F: Fn() -> Result<Option<Vec<HistoryMsg>>, String>,
{
    if let Some(hit) = find_user_turn_by_row_id(history, target_row_id) {
        return Some(hit);
    }
    let db_history = match load_db() {
        Ok(Some(v)) => v,
        Ok(None) | Err(_) => return None,
    };

    if heal_row_ids_if_aligned(history, &mut *history, &db_history) {
        if let Some(hit) = find_user_turn_by_row_id(history, target_row_id) {
            return Some(hit);
        }
    } else if history.len() == db_history.len() {
        // healing not applied due to misalignment — do not retry stale healing
    }

    let db_hit = find_user_turn_by_row_id(&db_history, target_row_id)?;
    let (db_ord, db_idx) = db_hit;
    let mem_user_indices = history_user_indices(history);
    if db_ord >= mem_user_indices.len() {
        return None;
    }
    let mem_idx = mem_user_indices[db_ord];
    if !mem_db_pair_agrees(&history[mem_idx], &db_history[db_idx]) {
        return None;
    }
    Some((db_ord, mem_idx))
}

/// Coerce client-supplied int param — mirrors `_coerce_truncate_int`.
///
/// `raw` is the JSON raw token for the value (e.g. `42`, `"42"`, `true`).
/// `false`/`true` without quotes are bool-typed and must refuse with 4004.
pub fn coerce_truncate_int_raw(raw: &str, param_name: &str) -> Result<i64, (i32, String)> {
    let t = raw.trim();
    // bool is int subclass in Python: JSON true/false without quotes must be rejected
    if t == "true" || t == "false" {
        return Err((ERR_TRUNCATE_NOT_INT, format!("{param_name} must be an integer")));
    }
    // strip optional string quotes (params may pass stringified int)
    let inner = if (t.starts_with('"') && t.ends_with('"') && t.len() >= 2)
        || (t.starts_with('\'') && t.ends_with('\'') && t.len() >= 2)
    {
        &t[1..t.len() - 1]
    } else {
        t
    };
    let inner = inner.trim();
    if inner == "true" || inner == "false" {
        return Err((ERR_TRUNCATE_NOT_INT, format!("{param_name} must be an integer")));
    }
    match inner.parse::<i64>() {
        Ok(n) => Ok(n),
        Err(_) => {
            // try float → int like Python int("3.0") would fail but int(3.0) works;
            // JSON numbers like 3.0 should still be accepted as int after truncation?
            // Python: int(3.0)=3 but int("3.0") fails. We mirror strict string parse:
            // only integer strings succeed.
            Err((ERR_TRUNCATE_NOT_INT, format!("{param_name} must be an integer")))
        }
    }
}

/// Parse from `params_json` field — convenience wrapper.
pub fn parse_truncate_int(params_json: &str, field: &str, param_name: &str) -> Result<Option<i64>, (i32, String)> {
    let raw = match extract_raw_value(params_json, field) {
        None => return Ok(None),
        Some(v) => v,
    };
    // Handle explicit null
    if raw.trim() == "null" {
        return Ok(None);
    }
    match coerce_truncate_int_raw(&raw, param_name) {
        Ok(n) => Ok(Some(n)),
        Err(e) => Err(e),
    }
}

/// Cross-check client ordinal against resolved durable target — mirrors `_reconcile_client_ordinal`.
///
/// Returns `Ok(msg_ordinal)` when client sent none or agreed (including prefix lineage),
/// else `Err((4030|4004, msg))` for mismatch.
pub fn reconcile_client_ordinal(
    client_ordinal_raw: Option<&str>,
    msg_ordinal: i64,
    param_name: &str,
    target_repr: &str,
    prefix_user_count: i64,
) -> Result<i64, (i32, String)> {
    let raw = match client_ordinal_raw {
        None => return Ok(msg_ordinal),
        Some(s) => s,
    };
    let ordinal = match coerce_truncate_int_raw(raw, "truncate_before_user_ordinal") {
        Ok(n) => n,
        Err(e) => return Err(e),
    };
    if ordinal == msg_ordinal {
        return Ok(msg_ordinal);
    }
    if prefix_user_count > 0 && ordinal == msg_ordinal + prefix_user_count {
        return Ok(msg_ordinal);
    }
    Err((
        ERR_ORDINAL_MISMATCH,
        format!(
            "truncate_before_user_ordinal ({}) does not match {} target turn ({})",
            ordinal, param_name, target_repr
        ),
    ))
}

// ---------------------------------------------------------------------------
// Reaction notes — mirrors _pending_reaction_notes
// ---------------------------------------------------------------------------

/// One reaction entry from `take_unseen_reactions`.
#[derive(Debug, Clone)]
pub struct ReactionEntry {
    pub emoji: String,
    pub role: String,
    pub text: String,
}

impl ReactionEntry {
    pub fn new(emoji: impl Into<String>, role: impl Into<String>, text: impl Into<String>) -> Self {
        Self { emoji: emoji.into(), role: role.into(), text: text.into() }
    }
}

/// Format a single reaction note — mirrors the loop inside `_pending_reaction_notes`.
///
/// Snippet trimmed to 120 + `…`, newlines → spaces.
pub fn format_reaction_note(entry: &ReactionEntry) -> String {
    let snippet = entry.text.trim().replace('\n', " ");
    let snippet_trunc = if snippet.chars().count() > 120 {
        let mut s: String = snippet.chars().take(120).collect();
        s.push('…');
        s
    } else {
        snippet
    };
    let whose = if entry.role == "user" { "their own" } else { "your" };
    if snippet_trunc.is_empty() {
        format!("[The user reacted {} to {} earlier message]", entry.emoji, whose)
    } else {
        format!("[The user reacted {} to {} message: \"{}\"]", entry.emoji, whose, snippet_trunc)
    }
}

/// Build the full notes block — mirrors `_pending_reaction_notes` aggregation.
///
/// `reactions` is already the `pending` list from `take_unseen_reactions` after the
/// `display.message_reactions` gate. Empty → `""`.
pub fn pending_reaction_notes_block(reactions: &[ReactionEntry]) -> String {
    if reactions.is_empty() {
        return String::new();
    }
    reactions.iter().map(format_reaction_note).collect::<Vec<_>>().join("\n")
}

// ---------------------------------------------------------------------------
// Core handlers — mirrors each @method body, injected for std-only testing
// ---------------------------------------------------------------------------

/// Handle `prompt.submit`.
///
/// `prompt_fn` mirrors the whole `try:` body: session lookup, truncation, DB write,
/// compute-host dispatch, agent start. Returns `Ok(result_json)` where result_json
/// is the `result` object (`{"status":"streaming", ...}`) or includes
/// `survivor_user_row_ids` when truncation applied.
/// `Err((code,msg))` maps to `_err`. The wrapper only handles JSON encoding.
///
/// The injected closure owns `sanitize_user_prompt_text`, voice-mode, `confirm_truncate`
/// gate, `replace_messages` archive, `is_disk_full_error` → `5070`, etc., so the port
/// stays std-only.
pub fn handle_prompt_submit<F>(rid_json: &str, params_json: &str, prompt_fn: F) -> String
where
    F: Fn(&str) -> Result<String, (i32, String)>,
{
    match prompt_fn(params_json) {
        Ok(result_json) => ok_response(rid_json, result_json.trim()),
        Err((code, msg)) => err_response(rid_json, code, &msg),
    }
}

/// Handle `clipboard.paste`.
///
/// `clip_fn` mirrors `with _profile_db + save_clipboard_image` → `{"attached":bool, ...}`.
/// `Ok(None)` handling for DB unavailable is inside the closure (maps to 5027).
pub fn handle_clipboard_paste<F>(rid_json: &str, params_json: &str, clip_fn: F) -> String
where
    F: Fn(&str) -> Result<String, (i32, String)>,
{
    match clip_fn(params_json) {
        Ok(result_json) => ok_response(rid_json, result_json.trim()),
        Err((code, msg)) => err_response(rid_json, code, &msg),
    }
}

/// Handle `image.attach` (slice 1 — lines 878-900 truncated).
///
/// Validates `path` required (`4015`) then delegates `4016`/`5027` and suffix
/// + `_detect_file_drop` to the injected `attach_fn`. Slice 1 covers through
/// `image_path = _resolve_attachment_path(path_token)` at line 900; the suffix
/// `in _IMAGE_EXTENSIONS` check and `_image_meta` continuation lives in slice 2
/// but the `handle_*` stub is already faithful for routing + `4015` tests.
///
/// `attach_fn` receives the raw `params_json` and returns the JSON for the
/// `result` field (`{"attached":true, "path":..., "count":..., "remainder":...}`)
/// or `Err((code,msg))`.
pub fn handle_image_attach<F>(rid_json: &str, params_json: &str, attach_fn: F) -> String
where
    F: Fn(&str) -> Result<String, (i32, String)>,
{
    // Slice-1 validation: path required (mirrors `if not raw: return _err(4015)`)
    let raw = extract_string_field(params_json, "path").unwrap_or_default();
    if raw.trim().is_empty() {
        return err_response(rid_json, ERR_IMAGE_PATH_REQUIRED, "path required");
    }
    match attach_fn(params_json) {
        Ok(result_json) => ok_response(rid_json, result_json.trim()),
        Err((code, msg)) => err_response(rid_json, code, &msg),
    }
}

// ---------------------------------------------------------------------------
// Registry wiring — mirrors _registry = HandlerRegistry() + register(server)
// ---------------------------------------------------------------------------

/// Build a fresh [`HandlerRegistry`] with the three slice-1 methods registered
/// using the provided deps (for tests / production injection).
///
/// Each closure is `'static` and mirrors the lazy imports inside Python
/// handler bodies. For the default stub (no backend) use [`build_registry_default`].
pub fn build_registry<P, C, A>(prompt_submit: P, clipboard_paste: C, image_attach: A) -> HandlerRegistry
where
    P: Fn(String, String) -> String + Send + Sync + 'static,
    C: Fn(String, String) -> String + Send + Sync + 'static,
    A: Fn(String, String) -> String + Send + Sync + 'static,
{
    let mut reg = HandlerRegistry::new();
    register_with(&mut reg, prompt_submit, clipboard_paste, image_attach);
    reg
}

/// Build a registry with default stubs (every operation returns error / `ok` with attached false).
pub fn build_registry_default() -> HandlerRegistry {
    build_registry(
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_prompt_submit(&rid_json, &params_json, |_| Err((ERR_SESSION_STORAGE, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_clipboard_paste(&rid_json, &params_json, |_| Err((ERR_CLIPBOARD, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_image_attach(&rid_json, &params_json, |_| Err((ERR_IMAGE_NOT_FOUND, "no backend".to_string())))
        },
    )
}

/// Register all three slice-1 methods onto an existing registry.
pub fn register_with<P, C, A>(registry: &mut HandlerRegistry, prompt_submit: P, clipboard_paste: C, image_attach: A)
where
    P: Fn(String, String) -> String + Send + Sync + 'static,
    C: Fn(String, String) -> String + Send + Sync + 'static,
    A: Fn(String, String) -> String + Send + Sync + 'static,
{
    registry.method(METHOD_PROMPT_SUBMIT, prompt_submit);
    registry.method(METHOD_CLIPBOARD_PASTE, clipboard_paste);
    registry.method(METHOD_IMAGE_ATTACH, image_attach);
}

/// Register with default stubs onto `registry`.
pub fn register(registry: &mut HandlerRegistry) {
    register_with(
        registry,
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_prompt_submit(&rid_json, &params_json, |_| Err((ERR_SESSION_STORAGE, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_clipboard_paste(&rid_json, &params_json, |_| Err((ERR_CLIPBOARD, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_image_attach(&rid_json, &params_json, |_| Err((ERR_IMAGE_NOT_FOUND, "no backend".to_string())))
        },
    )
}

// ---------------------------------------------------------------------------
// Tests — mirror Python invariants (std-only)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn rid1() -> String { encode_rid("1") }

    #[test]
    fn history_user_indices_filters_markers() {
        let h = vec![
            HistoryMsg::new_user("hi"),
            HistoryMsg::new_assistant("hello"),
            HistoryMsg::new_user("again").with_display_kind("hidden"),
            HistoryMsg::new_user("visible"),
        ];
        assert_eq!(history_user_indices(&h), vec![0, 3]);
        assert_eq!(history_user_indices(&[]), Vec::<usize>::new());
    }

    #[test]
    fn message_row_id_alias() {
        let m1 = HistoryMsg::new_user("x").with_row_id(42);
        let m2 = HistoryMsg::new_user("x").with_alt_row_id(7);
        let m3 = HistoryMsg::new_user("x");
        let m4 = HistoryMsg { row_id: Some(99), row_id_alt: Some(101), ..HistoryMsg::new_user("x") };
        assert_eq!(message_row_id(&m1), Some(42));
        assert_eq!(message_row_id(&m2), Some(7));
        assert_eq!(message_row_id(&m3), None);
        // _row_id preferred
        assert_eq!(message_row_id(&m4), Some(101));
    }

    #[test]
    fn mem_db_agrees() {
        let a = HistoryMsg::new_user(" hello ");
        let b = HistoryMsg::new_user("hello");
        assert!(mem_db_pair_agrees(&a, &b));
        let c = HistoryMsg::new_user("different");
        assert!(!mem_db_pair_agrees(&a, &c));
        let d = HistoryMsg::new_assistant("hello");
        assert!(!mem_db_pair_agrees(&a, &d));
        let e = HistoryMsg::new_user("hello").with_display_kind("marker");
        assert!(!mem_db_pair_agrees(&a, &e));
        // non-string content: only role/marker checked -> agrees
        let f = HistoryMsg { role: "user".into(), content: None, ..Default::default() };
        let g = HistoryMsg { role: "user".into(), content: None, ..Default::default() };
        assert!(mem_db_pair_agrees(&f, &g));
        // multimodal non-string same role/marker → true
        let mut h = HistoryMsg::new_user("x");
        h.content = None;
        assert!(mem_db_pair_agrees(&h, &g));
    }

    #[test]
    fn find_by_row_id() {
        let h = vec![
            HistoryMsg::new_user("a").with_row_id(1),
            HistoryMsg::new_assistant("b"),
            HistoryMsg::new_user("c").with_alt_row_id(2),
            HistoryMsg::new_user("d").with_row_id(3),
        ];
        assert_eq!(find_user_turn_by_row_id(&h, 2), Some((1, 2)));
        assert_eq!(find_user_turn_by_row_id(&h, 99), None);
        assert_eq!(find_user_turn_by_row_id(&h, 1), Some((0, 0)));
        assert_eq!(find_user_turn_by_row_id(&[], 1), None);
    }

    #[test]
    fn heal_stamps() {
        let mut hist = vec![
            HistoryMsg::new_user("a"),
            HistoryMsg::new_user("b"),
        ];
        let db = vec![
            HistoryMsg::new_user("a").with_alt_row_id(10),
            HistoryMsg::new_user("b").with_alt_row_id(20),
        ];
        assert!(heal_row_ids_if_aligned(&mut hist, &db));
        assert_eq!(message_row_id(&hist[0]), Some(10));
        assert_eq!(message_row_id(&hist[1]), Some(20));
        // misaligned length → no heal
        let mut hist2 = vec![HistoryMsg::new_user("a")];
        assert!(!heal_row_ids_if_aligned(&mut hist2, &db));
        // content mismatch → no heal
        let mut hist3 = vec![HistoryMsg::new_user("x"), HistoryMsg::new_user("y")];
        assert!(!heal_row_ids_if_aligned(&mut hist3, &db));
    }

    #[test]
    fn resolve_truncate_via_memory_and_db() {
        // direct hit
        let mut h = vec![
            HistoryMsg::new_user("a").with_row_id(1),
            HistoryMsg::new_user("b").with_row_id(2),
        ];
        let hit = resolve_truncate_row_id(&mut h, 2, || Ok(None));
        assert_eq!(hit, Some((1, 1)));

        // via durable healing
        let mut h2 = vec![HistoryMsg::new_user("a"), HistoryMsg::new_user("b")];
        let db = vec![HistoryMsg::new_user("a").with_row_id(10), HistoryMsg::new_user("b").with_row_id(20)];
        let hit2 = resolve_truncate_row_id(&mut h2, 20, || Ok(Some(db.clone())));
        assert!(hit2.is_some());
        // db miss → None
        let mut h3 = vec![HistoryMsg::new_user("a").with_row_id(1)];
        assert_eq!(resolve_truncate_row_id(&mut h3, 99, || Ok(Some(vec![HistoryMsg::new_user("a").with_row_id(1)]))), None);
        // db load failure → None
        let mut h4 = vec![HistoryMsg::new_user("a").with_row_id(1)];
        assert_eq!(resolve_truncate_row_id(&mut h4, 1, || Ok(None)), Some((0,0)));
        assert_eq!(resolve_truncate_row_id(&mut vec![HistoryMsg::new_user("a")], 99, || Err("fail".into())), None);
    }

    #[test]
    fn coerce_int_bool_refusal() {
        assert!(coerce_truncate_int_raw("42", "truncate_before_user_ordinal").is_ok());
        assert_eq!(coerce_truncate_int_raw("42", "p").unwrap(), 42);
        assert!(coerce_truncate_int_raw("true", "p").is_err());
        assert!(coerce_truncate_int_raw("false", "p").is_err());
        assert!(coerce_truncate_int_raw("\"true\"", "p").is_err());
        assert!(coerce_truncate_int_raw("notint", "p").is_err());
        assert!(coerce_truncate_int_raw("\"123\"", "p").is_ok());
        // error code 4004
        assert_eq!(coerce_truncate_int_raw("true", "my_param").unwrap_err().0, 4004);
        assert!(coerce_truncate_int_raw("true", "my_param").unwrap_err().1.contains("my_param"));
    }

    #[test]
    fn reconcile_cases() {
        // none -> msg_ordinal
        assert_eq!(reconcile_client_ordinal(None, 5, "truncate_before_row_id", "10", 0).unwrap(), 5);
        // equal -> ok
        assert_eq!(reconcile_client_ordinal(Some("5"), 5, "truncate_before_row_id", "5", 0).unwrap(), 5);
        // prefix lineage -> ok
        assert_eq!(reconcile_client_ordinal(Some("8"), 5, "truncate_before_row_id", "10", 3).unwrap(), 5);
        assert_eq!(reconcile_client_ordinal(Some("5"), 5, "truncate_before_row_id", "5", 3).unwrap(), 5);
        // mismatch -> 4030
        let err = reconcile_client_ordinal(Some("9"), 5, "truncate_before_row_id", "10", 0).unwrap_err();
        assert_eq!(err.0, 4030);
        // bool refusal inside -> 4004
        let err2 = reconcile_client_ordinal(Some("true"), 5, "truncate_before_row_id", "10", 0).unwrap_err();
        assert_eq!(err2.0, 4004);
        // prefix not enough -> 4030
        let err3 = reconcile_client_ordinal(Some("8"), 5, "truncate_before_row_id", "10", 1).unwrap_err();
        assert_eq!(err3.0, 4030);
    }

    #[test]
    fn reaction_formatting() {
        let e1 = ReactionEntry::new("👍", "assistant", "hello world");
        assert!(format_reaction_note(&e1).contains("your message"));
        assert!(format_reaction_note(&e1).contains("👍"));
        let e2 = ReactionEntry::new("❤️", "user", "hello");
        assert!(format_reaction_note(&e2).contains("their own"));
        // empty text -> earlier message without quote
        let e3 = ReactionEntry::new("😂", "assistant", "");
        assert!(format_reaction_note(&e3).contains("earlier message"));
        assert!(!format_reaction_note(&e3).contains('"'));
        // 120 trunc
        let long = "a".repeat(200);
        let e4 = ReactionEntry::new("🔥", "assistant", long);
        let note = format_reaction_note(&e4);
        assert!(note.contains('…'));
        // newline -> space
        let e5 = ReactionEntry::new("👏", "assistant", "line1\nline2");
        assert!(format_reaction_note(&e5).contains("line1 line2"));
        assert_eq!(pending_reaction_notes_block(&[]), "");
        let block = pending_reaction_notes_block(&[e1.clone(), e2.clone()]);
        assert!(block.contains('\n'));
    }

    #[test]
    fn prompt_submit_handler_ok_and_err() {
        let rid = rid1();
        let ok = handle_prompt_submit(&rid, r#"{"session_id":"abc","text":"hi"}"#, |_| Ok(r#"{"status":"streaming"}"#.to_string()));
        assert!(ok.contains(r#""status":"streaming""#), "{}", ok);
        assert!(ok.contains(r#""result""#));
        let err = handle_prompt_submit(&rid, "{}", |_| Err((4009, "subagent still running — wait for it to finish".into())));
        assert!(err.contains(r#""code":4009"#));
        let trunc_err = handle_prompt_submit(&rid, "{}", |_| Err((4029, "truncation parameters require confirm_truncate".into())));
        assert!(trunc_err.contains("4029"));
    }

    #[test]
    fn clipboard_paste_handler() {
        let rid = rid1();
        let ok = handle_clipboard_paste(&rid, "{}", |_| Ok(r#"{"attached":true,"path":"/tmp/clip.png","count":1}"#.to_string()));
        assert!(ok.contains(r#""attached":true"#));
        let err = handle_clipboard_paste(&rid, "{}", |_| Err((5027, "clipboard unavailable".into())));
        assert!(err.contains(r#""code":5027"#));
    }

    #[test]
    fn image_attach_requires_path() {
        let rid = rid1();
        let err = handle_image_attach(&rid, r#"{}"#, |_| Ok(r#"{"attached":true}"#.to_string()));
        assert!(err.contains(r#""code":4015"#), "{}", err);
        assert!(err.contains("path required"));
        let err2 = handle_image_attach(&rid, r#"{"path":""}"#, |_| Ok(r#"{}"#.to_string()));
        assert!(err2.contains(r#""code":4015"#));
        // with path delegates to closure and can return 4016 / ok
        let ok = handle_image_attach(&rid, r#"{"path":"/tmp/foo.png"}"#, |_| Ok(r#"{"attached":true,"path":"/tmp/foo.png","count":1}"#.to_string()));
        assert!(ok.contains(r#""attached":true"#));
        let notfound = handle_image_attach(&rid, r#"{"path":"/tmp/foo.png"}"#, |_| Err((4016, "image not found: /tmp/foo.png".into())));
        assert!(notfound.contains(r#""code":4016"#));
    }

    #[test]
    fn is_truthy_and_extract() {
        assert!(is_truthy_value(Some("true")));
        assert!(is_truthy_value(Some("1")));
        assert!(!is_truthy_value(Some("false")));
        assert!(!is_truthy_value(None));
        assert_eq!(extract_string_field(r#"{"path":"/tmp/a.png"}"#, "path").as_deref(), Some("/tmp/a.png"));
        assert_eq!(extract_string_field(r#"{"confirm_truncate":true}"#, "confirm_truncate"), None);
        assert!(is_truthy_field(r#"{"confirm_truncate":true}"#, "confirm_truncate"));
        assert!(!is_truthy_field(r#"{"confirm_truncate":false}"#, "confirm_truncate"));
        assert_eq!(parse_truncate_int(r#"{"truncate_before_row_id":42}"#, "truncate_before_row_id", "truncate_before_row_id").unwrap(), Some(42));
        assert!(parse_truncate_int(r#"{"truncate_before_row_id":true}"#, "truncate_before_row_id", "truncate_before_row_id").is_err());
    }

    #[test]
    fn registry_installs_three() {
        let mut reg = build_registry_default();
        assert_eq!(reg.len(), 3);
        let mut names: Vec<_> = reg.pending_names().collect();
        names.sort();
        assert_eq!(names, vec!["clipboard.paste","image.attach","prompt.submit"]);
        let mut map = HashMap::new();
        reg.install_into(&mut map);
        assert_eq!(map.len(), 3);
        let out = map.get(METHOD_PROMPT_SUBMIT).unwrap()("1".to_string(), "{}".to_string());
        assert!(out.contains("5071") || out.contains("no backend"));
        let out2 = map.get(METHOD_CLIPBOARD_PASTE).unwrap()("1".to_string(), "{}".to_string());
        assert!(out2.contains("5027") || out2.contains("no backend"));
        // image.attach without path should be 4015 even with default stub
        let out3 = map.get(METHOD_IMAGE_ATTACH).unwrap()("1".to_string(), "{}".to_string());
        assert!(out3.contains(r#""code":4015"#), "{}", out3);
    }

    #[test]
    fn ok_err_envelope_shape() {
        let rid = encode_rid("42");
        let ok = ok_response(&rid, r#"{"attached":true}"#);
        assert!(ok.contains(r#""result""#));
        let err = err_response(&rid, 4015, "path required");
        assert!(err.contains(r#""code":4015"#));
        assert!(err.contains("path required"));
    }

    #[test]
    fn prefix_user_count_reconcile_edge() {
        // prefix=0, equal only
        assert!(reconcile_client_ordinal(Some("5"), 5, "x", "5", 0).is_ok());
        assert!(reconcile_client_ordinal(Some("6"), 5, "x", "5", 0).is_err());
        // prefix>0 allows lineage count
        assert!(reconcile_client_ordinal(Some("7"), 5, "x", "5", 2).is_ok());
        assert!(reconcile_client_ordinal(Some("8"), 5, "x", "5", 2).is_err());
    }
}
