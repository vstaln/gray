//! Bot-relay JSON-RPC handlers — the gateway side of cross-connection A2A.
//!
//! 1:1 port of `tui_gateway/methods_bot_relay.py` (210 lines).
//!
//! Connections ARE the peer set: every gateway the Desktop holds a socket to
//! (local, remote URL, SSH, Hermes Cloud, docker) must be able to find every
//! other connection's agents and message them. The Desktop is the relay — it
//! owns every socket — and these four methods are the door it uses on EACH
//! connected gateway:
//!
//! - `bot_relay.roster.sync`  — Desktop pushes the union roster of agents on
//!   the OTHER connections into this gateway's `bot_relay/roster.json`, so
//!   `message_agent` can resolve cross-connection targets and Bot Chat
//!   prompts list them (capability-epoch refresh picks up changes).
//! - `bot_relay.outbox.drain` — Desktop collects envelopes queued here by
//!   `message_agent` for targets on other connections.
//! - `bot_relay.deliver`      — Desktop hands an envelope to the TARGET
//!   gateway; this method runs the same one-turn Bot Chat delivery local DMs
//!   use and returns the reply text.
//! - `bot_relay.reply`        — Desktop writes the reply (or a delivery
//!   error) back on the SENDER gateway; the waiter spawned at send time picks
//!   it up and wakes the sending agent via the standard completion path.
//!
//! Storage/validation plumbing lives in `tools/bot_relay.py`. Handlers are
//! rebound onto server.py's globals at install time (see method_ctx.py) and may
//! reference server module globals (`_ok`, `_err`) not imported here.
//!
//! ```python
//! # Python — tui_gateway/methods_bot_relay.py
//! from .method_ctx import HandlerRegistry
//!
//! _registry = HandlerRegistry()
//! method = _registry.method
//!
//! @method("bot_relay.roster.sync")
//! def _(rid, params: dict) -> dict:
//!     try:
//!         import os
//!         from pathlib import Path
//!         from tools.bot_relay import write_remote_roster
//!         home = Path(os.getenv("HERMES_HOME") or os.path.expanduser("~/.hermes"))
//!         root = home.parent.parent if home.parent.name == "profiles" else home
//!         count = write_remote_roster(root, params.get("agents"))
//!         return _ok(rid, {"count": count})
//!     except Exception as e:
//!         return _err(rid, 5090, str(e))
//!
//! @method("bot_relay.outbox.drain")
//! def _(rid, params: dict) -> dict:
//!     try:
//!         import os
//!         from pathlib import Path
//!         from tools.bot_relay import claim_pending_envelopes
//!         home = Path(os.getenv("HERMES_HOME") or os.path.expanduser("~/.hermes"))
//!         root = home.parent.parent if home.parent.name == "profiles" else home
//!         return _ok(rid, {"envelopes": claim_pending_envelopes(root)})
//!     except Exception as e:
//!         return _err(rid, 5091, str(e))
//!
//! @method("bot_relay.deliver")
//! def _(rid, params: dict) -> dict:
//!     import os, subprocess, tempfile
//!     from pathlib import Path
//!     profile = str(params.get("profile") or "").strip()
//!     message = str(params.get("message") or "").strip()
//!     if not profile or not message:
//!         return _err(rid, 4090, "profile and message required")
//!     try:
//!         from tools.bot_mode_dm import MESSAGE_MAX_CHARS
//!         from tools.bot_relay import acquire_turn_lock, local_delivery_command
//!         if len(message) > MESSAGE_MAX_CHARS + 200:
//!             return _err(rid, 4091, "message too long")
//!         home = Path(os.getenv("HERMES_HOME") or os.path.expanduser("~/.hermes"))
//!         root = home.parent.parent if home.parent.name == "profiles" else home
//!         known = {"default"}
//!         profiles_dir = root / "profiles"
//!         if profiles_dir.is_dir():
//!             known.update(c.name for c in profiles_dir.iterdir() if c.is_dir())
//!         resolved = "default" if profile.lower() == "hermes" else profile
//!         if resolved not in known:
//!             return _err(rid, 4092, f"no profile '{profile}' on this gateway")
//!         fd, tmp = tempfile.mkstemp(prefix="hermes-relay-dm-", suffix=".txt", text=True)
//!         try:
//!             with os.fdopen(fd, "w", encoding="utf-8") as f:
//!                 f.write(message)
//!             with acquire_turn_lock(root, resolved):
//!                 proc = subprocess.run(local_delivery_command(resolved, tmp), capture_output=True, text=True, timeout=600)
//!                 if proc.returncode != 0:
//!                     from tools.bot_failure_reasons import RETRY_NONE, classify_agent_error, retry_action
//!                     first_detail = (proc.stderr or proc.stdout or "").strip()[-500:]
//!                     if retry_action(classify_agent_error(first_detail)) != RETRY_NONE:
//!                         proc = subprocess.run(local_delivery_command(resolved, tmp), capture_output=True, text=True, timeout=600)
//!         finally:
//!             try: os.unlink(tmp)
//!             except OSError: pass
//!         if proc.returncode != 0:
//!             from tools.bot_failure_reasons import classify_agent_error
//!             detail = (proc.stderr or proc.stdout or "").strip()[-500:]
//!             return _err(rid, 5092, f"delivery turn failed: {detail or proc.returncode}", data={"reason": classify_agent_error(detail)})
//!         return _ok(rid, {"reply": (proc.stdout or "").strip()})
//!     except subprocess.TimeoutExpired:
//!         return _err(rid, 5093, "delivery turn timed out")
//!     except Exception as e:
//!         if getattr(e, "reason", "") == "target_busy":
//!             return _err(rid, 5096, str(e))
//!         return _err(rid, 5094, str(e))
//!
//! @method("bot_relay.reply")
//! def _(rid, params: dict) -> dict:
//!     envelope_id = str(params.get("id") or "").strip()
//!     if not envelope_id:
//!         return _err(rid, 4093, "id required")
//!     try:
//!         import os
//!         from pathlib import Path
//!         from tools.bot_relay import write_reply
//!         home = Path(os.getenv("HERMES_HOME") or os.path.expanduser("~/.hermes"))
//!         root = home.parent.parent if home.parent.name == "profiles" else home
//!         write_reply(root, envelope_id, reply=str(params.get("reply") or ""), error=str(params.get("error") or ""), reason=str(params.get("reason") or ""))
//!         return _ok(rid, {"ok": True})
//!     except ValueError as e:
//!         return _err(rid, 4094, str(e))
//!     except Exception as e:
//!         return _err(rid, 5095, str(e))
//!
//! def register(server) -> None:
//!     _registry.install(server)
//! ```
//!
//! # Rust mapping
//! * `HandlerRegistry` → [`crate::method_ctx::HandlerRegistry`] (same deferred
//!   `@method` + `install` shape; see `method_ctx.rs` for `FunctionType`
//!   rebinding no-op notes).
//! * `os.getenv("HERMES_HOME") or os.path.expanduser("~/.hermes")` + parent
//!   check → [`hermes_root`] (mirrors `Path` parent logic: `home.parent.name ==
//!   "profiles"` → `home.parent.parent`, else `home`).
//! * `write_remote_roster` / `claim_pending_envelopes` / `write_reply` →
//!   injected `Fn(&str, ...) -> Result<...,String>` closures so the port stays
//!   `std`-only and testable (Python's `from tools.bot_relay import ...` is
//!   lazy inside the handler body).
//! * `MESSAGE_MAX_CHARS` (16000 from `tools/bot_mode_dm.py`) + 200 headroom →
//!   [`MESSAGE_MAX_CHARS`] + [`ATTRIBUTION_HEADROOM`] (=16200 cap). Mirrors
//!   `len(message) > MESSAGE_MAX_CHARS + 200`.
//! * `known = {"default"} | {c.name for c in (root/"profiles").iterdir()}` →
//!   injected `list_profiles: Fn(&str) -> Vec<String>` (so tests don't touch
//!   the filesystem; production closure reads `root/profiles`).
//! * `resolved = "default" if profile.lower() == "hermes" else profile` →
//!   [`resolve_profile`].
//! * `tempfile.mkstemp` + `os.fdopen` + `os.unlink` → caller-provided
//!   `run_delivery: Fn(&str,&str) -> Result<String,String>` that encapsulates
//!   the temp-file + `acquire_turn_lock` + `subprocess.run(..., timeout=600)` +
//!   retry loop. The handler only validates and delegates; the closure owns the
//!   OS interaction, matching Python's `try/finally: os.unlink(tmp)`.
//! * Retry policy: `classify_agent_error` + `retry_action != RETRY_NONE` →
//!   injected `should_retry: Fn(&str)->bool` (where `&str` is the truncated
//!   500-char detail). Default stub returns `false`.
//! * `subprocess.TimeoutExpired` → `Err` with code [`ERR_DELIVER_TIMEOUT`] (=5093)
//!   via the `run_delivery` closure returning a sentinel timeout error.
//! * `getattr(e, "reason", "") == "target_busy"` → `Err` string containing
//!   `"target_busy"` is mapped to [`ERR_TARGET_BUSY`] (=5096) before generic
//!   [`ERR_DELIVER_FAILED`] (=5094).
//! * `_ok(rid, result)` / `_err(rid, code, msg, data=None)` → [`ok_response`] /
//!   [`err_response`] / [`err_response_with_data`] (mirrors `server.py::_ok` /
//!   `_err` envelope shape; `data` → `{"reason": ...}` for 5092).
//! * `@method("...")` + `register(server)` → [`register`] / [`register_with`] /
//!   [`build_registry`] (deferred registration via `HandlerRegistry::method` +
//!   `install`/`install_into`).

use std::collections::HashMap;
use std::path::Path;

use crate::method_ctx::HandlerRegistry;

// ---------------------------------------------------------------------------
// Constants — mirrors methods_bot_relay.py literals and server.py helpers
// ---------------------------------------------------------------------------

/// `bot_relay.roster.sync` method name.
pub const METHOD_ROSTER_SYNC: &str = "bot_relay.roster.sync";
/// `bot_relay.outbox.drain` method name.
pub const METHOD_OUTBOX_DRAIN: &str = "bot_relay.outbox.drain";
/// `bot_relay.deliver` method name.
pub const METHOD_DELIVER: &str = "bot_relay.deliver";
/// `bot_relay.reply` method name.
pub const METHOD_REPLY: &str = "bot_relay.reply";

/// Mirrors `tools/bot_mode_dm.py::MESSAGE_MAX_CHARS` (16000).
pub const MESSAGE_MAX_CHARS: usize = 16_000;
/// Attribution headroom added to the max. Mirrors `+ 200`.
pub const ATTRIBUTION_HEADROOM: usize = 200;
/// Effective cap for `bot_relay.deliver` message. Mirrors `MESSAGE_MAX_CHARS + 200`.
pub const MESSAGE_MAX_WITH_HEADROOM: usize = MESSAGE_MAX_CHARS + ATTRIBUTION_HEADROOM;

/// Delivery turn timeout in seconds. Mirrors `timeout=600`.
pub const DELIVERY_TIMEOUT_SECS: u64 = 600;

/// Error codes — mirrors `_err(rid, N, ...)` in each handler.
pub const ERR_ROSTER_SYNC: i32 = 5090;
pub const ERR_OUTBOX_DRAIN: i32 = 5091;
pub const ERR_DELIVER_MISSING: i32 = 4090;
pub const ERR_DELIVER_TOO_LONG: i32 = 4091;
pub const ERR_DELIVER_NO_PROFILE: i32 = 4092;
pub const ERR_DELIVER_TURN_FAILED: i32 = 5092;
pub const ERR_DELIVER_TIMEOUT: i32 = 5093;
pub const ERR_DELIVER_FAILED: i32 = 5094;
pub const ERR_TARGET_BUSY: i32 = 5096;
pub const ERR_REPLY_MISSING_ID: i32 = 4093;
pub const ERR_REPLY_INVALID_ID: i32 = 4094;
pub const ERR_REPLY_FAILED: i32 = 5095;

// ---------------------------------------------------------------------------
// Small helpers — JSON envelope, hermes root, profile resolve
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

/// Mirrors `server.py::_ok(rid, result)` → `{"jsonrpc":"2.0","id":rid,"result":result}`.
pub fn ok_response(rid_json: &str, result_json: &str) -> String {
    format!(r#"{{"jsonrpc":"2.0","id":{},"result":{}}}"#, rid_json, result_json)
}

/// Mirrors `server.py::_err(rid, code, msg)` without `data`.
pub fn err_response(rid_json: &str, code: i32, msg: &str) -> String {
    let esc = json_escape(msg);
    format!(
        r#"{{"jsonrpc":"2.0","id":{},"error":{{"code":{},"message":"{}"}}}}"#,
        rid_json, code, esc
    )
}

/// Mirrors `server.py::_err(rid, code, msg, data={"reason":...})`.
pub fn err_response_with_data(rid_json: &str, code: i32, msg: &str, data_json: &str) -> String {
    let esc = json_escape(msg);
    format!(
        r#"{{"jsonrpc":"2.0","id":{},"error":{{"code":{},"message":"{}","data":{}}}}}"#,
        rid_json, code, esc, data_json
    )
}

/// Encode `rid` as JSON — string rid → quoted, numeric/null/bool → raw, empty → null.
/// Mirrors Python's opaque `rid` passthrough.
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

/// Resolve HERMES root from `HERMES_HOME` env or `~/.hermes`.
///
/// Mirrors:
/// ```python
/// home = Path(os.getenv("HERMES_HOME") or os.path.expanduser("~/.hermes"))
/// root = home.parent.parent if home.parent.name == "profiles" else home
/// ```
/// `hermes_home` is `os.getenv("HERMES_HOME")` (None when absent/empty);
/// `home_dir` is the OS home dir (for `~` expansion, e.g. `/home/alice`).
pub fn hermes_root(hermes_home: Option<&str>, home_dir: &str) -> String {
    let home_str = match hermes_home {
        Some(v) if !v.trim().is_empty() => v.trim().to_string(),
        _ => {
            let h = home_dir.trim_end_matches('/');
            if h.is_empty() {
                "/tmp/.hermes".to_string()
            } else {
                format!("{}/.hermes", h)
            }
        }
    };
    let p = Path::new(&home_str);
    // Check parent name == "profiles"
    if let Some(parent) = p.parent() {
        if let Some(name) = parent.file_name().and_then(|n| n.to_str()) {
            if name == "profiles" {
                if let Some(grand) = parent.parent() {
                    return grand.to_string_lossy().to_string();
                }
            }
        }
    }
    home_str
}

/// Resolve `profile` alias: `"hermes"` (case-insensitive) → `"default"`, else as-is.
/// Mirrors `resolved = "default" if profile.lower() == "hermes" else profile`.
pub fn resolve_profile(profile: &str) -> String {
    if profile.to_ascii_lowercase() == "hermes" {
        "default".to_string()
    } else {
        profile.to_string()
    }
}

/// Build `hermes -p <profile> chat ... --query-file <file>` argv.
/// Mirrors `tools/bot_relay.py::local_delivery_command`.
pub fn local_delivery_command(profile: &str, query_file: &str) -> Vec<String> {
    vec![
        "hermes".to_string(),
        "-p".to_string(),
        profile.to_string(),
        "chat".to_string(),
        "--in".to_string(),
        "~".to_string(),
        "-c".to_string(),
        "Bot Chat".to_string(),
        "--create-if-missing".to_string(),
        "-Q".to_string(),
        "--query-file".to_string(),
        query_file.to_string(),
    ]
}

/// Check if a delivery detail string should be retried.
/// Mirrors `retry_action(classify_agent_error(detail)) != RETRY_NONE`.
/// Default stub returns `false`; inject real classifier for production.
pub fn should_retry_default(_detail: &str) -> bool {
    false
}

// ---------------------------------------------------------------------------
// JSON param extraction — minimal std-only
// ---------------------------------------------------------------------------

/// Extract a quoted string field `field` from a flat JSON object string.
/// Returns `Some(value)` without surrounding quotes, `None` when absent/not a string.
/// Handles `null` / missing as `None`. Minimal — not a full JSON parser.
pub fn extract_string_field(json: &str, field: &str) -> Option<String> {
    let key = format!("\"{}\"", field);
    let pos = json.find(&key)?;
    let after = &json[pos + key.len()..];
    let colon = after.find(':')?;
    let mut val = after[colon + 1..].trim_start();
    if val.starts_with("null") {
        return None;
    }
    // Allow single-quoted keys/values as well
    if val.starts_with('\'') {
        let end = val[1..].find('\'')?;
        return Some(val[1..1 + end].to_string());
    }
    if !val.starts_with('"') {
        // Unquoted primitive — read until , or }
        let end = val.find(|c| c == ',' || c == '}').unwrap_or(val.len());
        let raw = val[..end].trim().trim_matches('"').trim_matches('\'');
        if raw.is_empty() {
            return None;
        }
        return Some(raw.to_string());
    }
    // Parse quoted string with escapes
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
                    // skip 4 hex digits, push placeholder
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

/// Extract raw JSON value for `field` (array/object/string/number) from `json`.
/// Returns the slice including brackets/quotes, or `None` when absent.
/// Handles balanced `[]` / `{}` / `""`.
pub fn extract_raw_value(json: &str, field: &str) -> Option<String> {
    let key = format!("\"{}\"", field);
    let pos = json.find(&key)?;
    let after = &json[pos + key.len()..];
    let colon = after.find(':')?;
    let mut rest = after[colon + 1..].trim_start();
    if rest.is_empty() {
        return None;
    }
    // If array or object, capture balanced
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
                // +1 for opening quote offset
                return Some(rest[..=i + 1].to_string());
            }
        }
        return None;
    }
    // Primitive: until , or }
    let end = rest.find(|c| c == ',' || c == '}').unwrap_or(rest.len());
    Some(rest[..end].trim().to_string())
}

/// Minimal JSON string escaper for error messages (shared with envelope helpers).
fn _json_escape_inner(s: &str) -> String {
    json_escape(s)
}

// ---------------------------------------------------------------------------
// Core handlers — mirrors each @method body, injected for std-only testing
// ---------------------------------------------------------------------------

/// Handle `bot_relay.roster.sync`.
///
/// `rid_json` is JSON-encoded request id; `params_json` is raw `params` JSON.
/// `hermes_home` / `home_dir` mirror `os.getenv("HERMES_HOME")` + `~` expansion.
/// `write_roster` mirrors `tools.bot_relay.write_remote_roster(root, agents)`:
/// takes `(root, agents_json)` where `agents_json` is the raw `agents` value
/// (e.g. `[{"profile":"..."}]` or `"null"` when absent), returns `Ok(count)` or
/// `Err(message)`. Returns a JSON-RPC envelope string.
pub fn handle_roster_sync<F>(
    rid_json: &str,
    params_json: &str,
    hermes_home: Option<&str>,
    home_dir: &str,
    write_roster: F,
) -> String
where
    F: Fn(&str, &str) -> Result<usize, String>,
{
    let root = hermes_root(hermes_home, home_dir);
    // params.get("agents") may be missing/non-list → write_remote_roster handles it;
    // we pass the raw JSON value or "null" to preserve the Python's `params.get("agents")`.
    let agents_json = extract_raw_value(params_json, "agents").unwrap_or_else(|| "null".to_string());
    match write_roster(&root, &agents_json) {
        Ok(count) => {
            let result = format!(r#"{{"count":{}}}"#, count);
            ok_response(rid_json, &result)
        }
        Err(e) => err_response(rid_json, ERR_ROSTER_SYNC, &e),
    }
}

/// Handle `bot_relay.outbox.drain`.
///
/// `claim` mirrors `tools.bot_relay.claim_pending_envelopes(root)`:
/// takes `root` and returns `Ok(envelopes_json_array)` (e.g. `"[{...}]"`) or
/// `Err(message)`. The `params` dict is ignored (mirrors Python's `params` unused).
pub fn handle_outbox_drain<F>(
    rid_json: &str,
    params_json: &str,
    hermes_home: Option<&str>,
    home_dir: &str,
    claim: F,
) -> String
where
    F: Fn(&str) -> Result<String, String>,
{
    let _ = params_json; // unused — mirrors Python's `params: dict` ignored
    let root = hermes_root(hermes_home, home_dir);
    match claim(&root) {
        Ok(arr_json) => {
            // arr_json is expected to be a JSON array string; we embed directly.
            // If caller returned raw objects, we still wrap as array field.
            let trimmed = arr_json.trim();
            let envelopes = if trimmed.is_empty() { "[]" } else { trimmed };
            let result = format!(r#"{{"envelopes":{}}}"#, envelopes);
            ok_response(rid_json, &result)
        }
        Err(e) => err_response(rid_json, ERR_OUTBOX_DRAIN, &e),
    }
}

/// Handle `bot_relay.deliver`.
///
/// Validation mirrors Python exactly; OS/subprocess/turn-lock is owned by
/// `run_delivery: Fn(profile, message, root) -> Result<reply, DeliverErr>`.
///
/// `run_delivery` is called with `(resolved_profile, message, root)`. On
/// success it returns `Ok(reply_text)`. On failure it returns
/// `Err(DeliverErr)` where `DeliverErr` encodes the Python exception branches:
/// - `Timeout` → 5093
/// - `TargetBusy(msg)` → 5096
/// - `TurnFailed { detail, reason }` → 5092 with `data={"reason":...}`
/// - `Other(msg)` → 5094
///
/// `list_profiles: Fn(root) -> Vec<String>` mirrors the `root/profiles` scan.
#[derive(Debug, Clone)]
pub enum DeliverErr {
    Timeout,
    TargetBusy(String),
    TurnFailed { detail: String, reason: String },
    Other(String),
}

pub fn handle_deliver<F, L>(
    rid_json: &str,
    params_json: &str,
    hermes_home: Option<&str>,
    home_dir: &str,
    list_profiles: L,
    run_delivery: F,
) -> String
where
    F: Fn(&str, &str, &str) -> Result<String, DeliverErr>,
    L: Fn(&str) -> Vec<String>,
{
    let profile_raw = extract_string_field(params_json, "profile")
        .unwrap_or_default()
        .trim()
        .to_string();
    let message_raw = extract_string_field(params_json, "message")
        .unwrap_or_default()
        .trim()
        .to_string();

    if profile_raw.is_empty() || message_raw.is_empty() {
        return err_response(rid_json, ERR_DELIVER_MISSING, "profile and message required");
    }
    // len(message) > MESSAGE_MAX_CHARS + 200 — Python uses char count, we use byte-aware
    // char count via .chars().count() to match Python's `len(str)`.
    if message_raw.chars().count() > MESSAGE_MAX_WITH_HEADROOM {
        return err_response(rid_json, ERR_DELIVER_TOO_LONG, "message too long");
    }

    let root = hermes_root(hermes_home, home_dir);
    let resolved = resolve_profile(&profile_raw);

    // known = {"default"} ∪ profiles_dir iter
    let mut known: std::collections::HashSet<String> = std::collections::HashSet::new();
    known.insert("default".to_string());
    for p in list_profiles(&root) {
        known.insert(p);
    }
    if !known.contains(&resolved) {
        let msg = format!("no profile '{}' on this gateway", profile_raw);
        return err_response(rid_json, ERR_DELIVER_NO_PROFILE, &msg);
    }

    match run_delivery(&resolved, &message_raw, &root) {
        Ok(reply) => {
            let esc = json_escape(reply.trim());
            let result = format!(r#"{{"reply":"{}"}}"#, esc);
            ok_response(rid_json, &result)
        }
        Err(DeliverErr::Timeout) => err_response(rid_json, ERR_DELIVER_TIMEOUT, "delivery turn timed out"),
        Err(DeliverErr::TargetBusy(msg)) => err_response(rid_json, ERR_TARGET_BUSY, &msg),
        Err(DeliverErr::TurnFailed { detail, reason }) => {
            let truncated = if detail.chars().count() > 500 {
                detail.chars().skip(detail.chars().count() - 500).collect::<String>()
            } else {
                detail
            };
            let msg = format!("delivery turn failed: {}", if truncated.is_empty() { "unknown".to_string() } else { truncated.clone() });
            // reason is classify_agent_error(detail) — preserve even when detail empty
            let reason_esc = json_escape(&reason);
            let data_json = format!(r#"{{"reason":"{}"}}"#, reason_esc);
            err_response_with_data(rid_json, ERR_DELIVER_TURN_FAILED, &msg, &data_json)
        }
        Err(DeliverErr::Other(msg)) => {
            // Check for target_busy sentinel inside generic error (mirrors getattr(e,"reason")=="target_busy")
            if msg.contains("target_busy") {
                return err_response(rid_json, ERR_TARGET_BUSY, &msg);
            }
            err_response(rid_json, ERR_DELIVER_FAILED, &msg)
        }
    }
}

/// Handle `bot_relay.reply`.
///
/// `write_reply_fn` mirrors `tools.bot_relay.write_reply(root, id, reply, error, reason)`:
/// takes `(root, envelope_id, reply, error, reason)` and returns `Ok(())` or
/// `Err(msg)` where `ValueError` (invalid id) should be returned as `Err` with
/// a message that the caller maps to 4094 (we sniff `"invalid envelope id"`).
pub fn handle_reply<F>(
    rid_json: &str,
    params_json: &str,
    hermes_home: Option<&str>,
    home_dir: &str,
    write_reply_fn: F,
) -> String
where
    F: Fn(&str, &str, &str, &str, &str) -> Result<(), String>,
{
    let envelope_id = extract_string_field(params_json, "id")
        .unwrap_or_default()
        .trim()
        .to_string();
    if envelope_id.is_empty() {
        return err_response(rid_json, ERR_REPLY_MISSING_ID, "id required");
    }
    let reply = extract_string_field(params_json, "reply").unwrap_or_default();
    let error = extract_string_field(params_json, "error").unwrap_or_default();
    let reason = extract_string_field(params_json, "reason").unwrap_or_default();
    let root = hermes_root(hermes_home, home_dir);
    match write_reply_fn(&root, &envelope_id, &reply, &error, &reason) {
        Ok(()) => ok_response(rid_json, r#"{"ok":true}"#),
        Err(e) => {
            // Python distinguishes ValueError (invalid envelope id) → 4094 vs generic → 5095
            let lower = e.to_ascii_lowercase();
            if lower.contains("invalid envelope id") || lower.contains("invalid envelope") {
                err_response(rid_json, ERR_REPLY_INVALID_ID, &e)
            } else if lower.contains("invalid") {
                err_response(rid_json, ERR_REPLY_INVALID_ID, &e)
            } else {
                err_response(rid_json, ERR_REPLY_FAILED, &e)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Registry wiring — mirrors _registry = HandlerRegistry() + register(server)
// ---------------------------------------------------------------------------

/// Build a fresh [`HandlerRegistry`] with all four bot_relay methods registered.
///
/// Each closure is `'static` and mirrors the lazy `from tools... import` inside
/// the Python handler body. For the default stub (no backend) use
/// [`build_registry_default`].
pub fn build_registry<W, C, D, R>(
    write_roster: W,
    claim_envelopes: C,
    deliver: D,
    write_reply: R,
) -> HandlerRegistry
where
    W: Fn(&str, &str) -> Result<usize, String> + Send + Sync + 'static,
    C: Fn(&str) -> Result<String, String> + Send + Sync + 'static,
    D: Fn(&str, &str, &str) -> Result<String, DeliverErr> + Send + Sync + 'static,
    R: Fn(&str, &str, &str, &str, &str) -> Result<(), String> + Send + Sync + 'static,
{
    let mut reg = HandlerRegistry::new();
    register_with(&mut reg, write_roster, claim_envelopes, deliver, write_reply);
    reg
}

/// Build a registry with default stubs (every operation returns an error).
///
/// Mirrors the import-failure path where `tools.bot_relay` is unavailable.
pub fn build_registry_default() -> HandlerRegistry {
    build_registry(
        |_, _| Err("no backend".to_string()),
        |_| Err("no backend".to_string()),
        |_, _, _| Err(DeliverErr::Other("no backend".to_string())),
        |_, _, _, _, _| Err("no backend".to_string()),
    )
}

/// Register all four bot_relay methods onto an existing registry.
///
/// Mirrors `register(server)` which calls `_registry.install(server)`.
/// This helper defers registration onto `registry` with the provided deps.
pub fn register_with<W, C, D, R>(
    registry: &mut HandlerRegistry,
    write_roster: W,
    claim_envelopes: C,
    deliver: D,
    write_reply: R,
) where
    W: Fn(&str, &str) -> Result<usize, String> + Send + Sync + 'static,
    C: Fn(&str) -> Result<String, String> + Send + Sync + 'static,
    D: Fn(&str, &str, &str) -> Result<String, DeliverErr> + Send + Sync + 'static,
    R: Fn(&str, &str, &str, &str, &str) -> Result<(), String> + Send + Sync + 'static,
{
    // roster.sync
    registry.method(METHOD_ROSTER_SYNC, move |rid, params_json| {
        let rid_json = encode_rid(&rid);
        // In the WS server, HERMES_HOME is read per-request from env; for the
        // tui_gateway std-only port we capture it via env at call time.
        // To keep the closure `'static`, read env inside the handler.
        let hermes_home = std::env::var("HERMES_HOME").ok();
        let home_dir = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        handle_roster_sync(&rid_json, &params_json, hermes_home.as_deref(), &home_dir, &write_roster)
    });

    // outbox.drain
    registry.method(METHOD_OUTBOX_DRAIN, move |rid, params_json| {
        let rid_json = encode_rid(&rid);
        let hermes_home = std::env::var("HERMES_HOME").ok();
        let home_dir = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        handle_outbox_drain(&rid_json, &params_json, hermes_home.as_deref(), &home_dir, &claim_envelopes)
    });

    // deliver
    registry.method(METHOD_DELIVER, move |rid, params_json| {
        let rid_json = encode_rid(&rid);
        let hermes_home = std::env::var("HERMES_HOME").ok();
        let home_dir = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        // profiles_dir scan stub — real filesystem listing
        let list_profiles = |root: &str| {
            let p = Path::new(root).join("profiles");
            let mut out = Vec::new();
            if let Ok(rd) = std::fs::read_dir(&p) {
                for e in rd.flatten() {
                    if let Ok(ft) = e.file_type() {
                        if ft.is_dir() {
                            if let Some(name) = e.file_name().to_str() {
                                out.push(name.to_string());
                            }
                        }
                    }
                }
            }
            out
        };
        handle_deliver(&rid_json, &params_json, hermes_home.as_deref(), &home_dir, list_profiles, &deliver)
    });

    // reply
    registry.method(METHOD_REPLY, move |rid, params_json| {
        let rid_json = encode_rid(&rid);
        let hermes_home = std::env::var("HERMES_HOME").ok();
        let home_dir = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        handle_reply(&rid_json, &params_json, hermes_home.as_deref(), &home_dir, &write_reply)
    });
}

/// Register with default stubs (mirrors Python's bare `register(server)` when
/// `tools.bot_relay` is importable but we have no injected impl).
pub fn register(registry: &mut HandlerRegistry) {
    register_with(
        registry,
        |_, _| Err("no backend".to_string()),
        |_| Err("no backend".to_string()),
        |_, _, _| Err(DeliverErr::Other("no backend".to_string())),
        |_, _, _, _, _| Err("no backend".to_string()),
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
    fn hermes_root_cases() {
        assert_eq!(hermes_root(Some("/home/alice/.hermes"), "/home/alice"), "/home/alice/.hermes");
        assert_eq!(
            hermes_root(Some("/home/alice/.hermes/profiles/bob"), "/home/alice"),
            "/home/alice/.hermes"
        );
        assert_eq!(hermes_root(None, "/home/alice"), "/home/alice/.hermes");
        assert_eq!(hermes_root(Some(""), "/home/alice"), "/home/alice/.hermes");
        // nested profiles check is only one level: home.parent.name == "profiles"
        assert_eq!(hermes_root(Some("/tmp/x/profiles"), "/tmp"), "/tmp/x/profiles");
    }

    #[test]
    fn resolve_profile_cases() {
        assert_eq!(resolve_profile("hermes"), "default");
        assert_eq!(resolve_profile("Hermes"), "default");
        assert_eq!(resolve_profile("HERMES"), "default");
        assert_eq!(resolve_profile("default"), "default");
        assert_eq!(resolve_profile("bob"), "bob");
    }

    #[test]
    fn local_delivery_command_shape() {
        let cmd = local_delivery_command("mybot", "/tmp/q.txt");
        assert_eq!(cmd[0], "hermes");
        assert_eq!(cmd[1], "-p");
        assert_eq!(cmd[2], "mybot");
        assert!(cmd.contains(&"Bot Chat".to_string()));
        assert!(cmd.contains(&"--query-file".to_string()));
        assert_eq!(cmd.last().unwrap(), "/tmp/q.txt");
    }

    #[test]
    fn roster_sync_success() {
        let rid = rid1();
        let params = r#"{"agents":[{"profile":"a","handle":"b","connection_id":"c"}]}"#;
        let out = handle_roster_sync(&rid, params, Some("/tmp/home/.hermes"), "/tmp/home", |root, agents| {
            assert_eq!(root, "/tmp/home/.hermes");
            assert!(agents.contains("a"));
            Ok(1)
        });
        assert!(out.contains(r#""count":1"#), "{}", out);
        assert!(out.contains(r#""result""#));
    }

    #[test]
    fn roster_sync_error_maps_to_5090() {
        let rid = rid1();
        let out = handle_roster_sync(&rid, r#"{"agents":[]}"#, None, "/home/u", |_, _| Err("boom".into()));
        assert!(out.contains(r#""code":5090"#));
        assert!(out.contains("boom"));
    }

    #[test]
    fn outbox_drain_success() {
        let rid = rid1();
        let out = handle_outbox_drain(&rid, "{}", Some("/tmp/h/.hermes"), "/tmp", |root| {
            assert_eq!(root, "/tmp/h/.hermes");
            Ok(r#"[{"id":"abc"}]"#.to_string())
        });
        assert!(out.contains(r#""envelopes""#));
        assert!(out.contains("abc"));
    }

    #[test]
    fn outbox_drain_empty() {
        let rid = rid1();
        let out = handle_outbox_drain(&rid, "{}", None, "/home/u", |_| Ok("[]".to_string()));
        assert!(out.contains(r#""envelopes":[]"#));
    }

    #[test]
    fn outbox_drain_error_5091() {
        let rid = rid1();
        let out = handle_outbox_drain(&rid, "{}", None, "/home/u", |_| Err("fail".into()));
        assert!(out.contains(r#""code":5091"#));
    }

    #[test]
    fn deliver_missing_fields_4090() {
        let rid = rid1();
        let cases = [r#"{}"#, r#"{"profile":"bob"}"#, r#"{"message":"hi"}"#, r#"{"profile":"","message":"hi"}"#];
        for c in cases {
            let out = handle_deliver(&rid, c, None, "/home/u", |_| vec!["default".into()], |_, _, _| Ok("hi".into()));
            assert!(out.contains(r#""code":4090"#), "case {} got {}", c, out);
        }
    }

    #[test]
    fn deliver_too_long_4091() {
        let rid = rid1();
        let long = "a".repeat(MESSAGE_MAX_WITH_HEADROOM + 1);
        let params = format!(r#"{{"profile":"default","message":"{}"}}"#, long);
        let out = handle_deliver(&rid, &params, None, "/home/u", |_| vec!["default".into()], |_, _, _| Ok("hi".into()));
        assert!(out.contains(r#""code":4091"#));
    }

    #[test]
    fn deliver_unknown_profile_4092() {
        let rid = rid1();
        let params = r#"{"profile":"ghost","message":"hi"}"#;
        let out = handle_deliver(&rid, params, Some("/tmp/h/.hermes"), "/tmp", |_| vec!["default".into()], |_, _, _| Ok("hi".into()));
        assert!(out.contains(r#""code":4092"#));
        assert!(out.contains("ghost"));
    }

    #[test]
    fn deliver_hermes_alias_resolves_to_default() {
        let rid = rid1();
        let params = r#"{"profile":"hermes","message":"hello"}"#;
        let out = handle_deliver(&rid, params, None, "/home/u", |_| vec!["default".into()], |profile, msg, _| {
            assert_eq!(profile, "default");
            assert_eq!(msg, "hello");
            Ok("reply text".to_string())
        });
        assert!(out.contains(r#""reply":"reply text""#));
    }

    #[test]
    fn deliver_success_trims_reply() {
        let rid = rid1();
        let params = r#"{"profile":"default","message":"hi"}"#;
        let out = handle_deliver(&rid, params, None, "/home/u", |_| vec!["default".into()], |_, _, _| Ok("  hello world  \n".to_string()));
        assert!(out.contains(r#""reply":"hello world""#));
    }

    #[test]
    fn deliver_timeout_5093() {
        let rid = rid1();
        let params = r#"{"profile":"default","message":"hi"}"#;
        let out = handle_deliver(&rid, params, None, "/home/u", |_| vec!["default".into()], |_, _, _| Err(DeliverErr::Timeout));
        assert!(out.contains(r#""code":5093"#));
    }

    #[test]
    fn deliver_target_busy_5096() {
        let rid = rid1();
        let params = r#"{"profile":"default","message":"hi"}"#;
        let out = handle_deliver(&rid, params, None, "/home/u", |_| vec!["default".into()], |_, _, _| Err(DeliverErr::TargetBusy("target_busy: busy".into())));
        assert!(out.contains(r#""code":5096"#));
    }

    #[test]
    fn deliver_turn_failed_5092_with_reason() {
        let rid = rid1();
        let params = r#"{"profile":"default","message":"hi"}"#;
        let out = handle_deliver(
            &rid,
            params,
            None,
            "/home/u",
            |_| vec!["default".into()],
            |_, _, _| Err(DeliverErr::TurnFailed { detail: "model not found".into(), reason: "model_unavailable".into() }),
        );
        assert!(out.contains(r#""code":5092"#));
        assert!(out.contains("delivery turn failed"));
        assert!(out.contains(r#""reason":"model_unavailable""#));
    }

    #[test]
    fn deliver_other_with_target_busy_text_maps_to_5096() {
        let rid = rid1();
        let params = r#"{"profile":"default","message":"hi"}"#;
        let out = handle_deliver(&rid, params, None, "/home/u", |_| vec!["default".into()], |_, _, _| Err(DeliverErr::Other("something target_busy inside".into())));
        assert!(out.contains(r#""code":5096"#));
    }

    #[test]
    fn deliver_other_5094() {
        let rid = rid1();
        let params = r#"{"profile":"default","message":"hi"}"#;
        let out = handle_deliver(&rid, params, None, "/home/u", |_| vec!["default".into()], |_, _, _| Err(DeliverErr::Other("kaboom".into())));
        assert!(out.contains(r#""code":5094"#));
        assert!(out.contains("kaboom"));
    }

    #[test]
    fn reply_missing_id_4093() {
        let rid = rid1();
        for c in [r#"{}"#, r#"{"id":""}"#, r#"{"id":"   "}"#] {
            let out = handle_reply(&rid, c, None, "/home/u", |_, _, _, _, _| Ok(()));
            assert!(out.contains(r#""code":4093"#), "case {} got {}", c, out);
        }
    }

    #[test]
    fn reply_success_ok_true() {
        let rid = rid1();
        let out = handle_reply(&rid, r#"{"id":"abc123abc123abc123abc123abc123ab","reply":"hi"}"#, Some("/tmp/h/.hermes"), "/tmp", |root, id, reply, err, reason| {
            assert_eq!(root, "/tmp/h/.hermes");
            assert_eq!(id, "abc123abc123abc123abc123abc123ab");
            assert_eq!(reply, "hi");
            assert_eq!(err, "");
            assert_eq!(reason, "");
            Ok(())
        });
        assert!(out.contains(r#""ok":true"#));
    }

    #[test]
    fn reply_invalid_id_4094() {
        let rid = rid1();
        let out = handle_reply(&rid, r#"{"id":"not-hex","reply":"x"}"#, None, "/home/u", |_, _, _, _, _| Err("invalid envelope id: 'not-hex'".into()));
        assert!(out.contains(r#""code":4094"#));
    }

    #[test]
    fn reply_generic_5095() {
        let rid = rid1();
        let out = handle_reply(&rid, r#"{"id":"abc123abc123abc123abc123abc123ab"}"#, None, "/home/u", |_, _, _, _, _| Err("disk full".into()));
        assert!(out.contains(r#""code":5095"#));
    }

    #[test]
    fn build_registry_installs_all_four() {
        let mut reg = build_registry_default();
        assert_eq!(reg.len(), 4);
        let mut names: Vec<_> = reg.pending_names().collect();
        names.sort();
        assert_eq!(names, vec!["bot_relay.deliver", "bot_relay.outbox.drain", "bot_relay.reply", "bot_relay.roster.sync"]);
        let mut map = HashMap::new();
        reg.install_into(&mut map);
        assert_eq!(map.len(), 4);
        // roster.sync stub should return 5090
        let out = map.get(METHOD_ROSTER_SYNC).unwrap()("1".to_string(), r#"{"agents":[]}"#.to_string());
        assert!(out.contains("5090"));
    }

    #[test]
    fn ok_err_envelope_shape() {
        let rid = encode_rid("42");
        let ok = ok_response(&rid, r#"{"count":2}"#);
        assert!(ok.contains(r#""count":2"#));
        assert!(ok.contains(r#""result""#));
        let err = err_response(&rid, 5090, "boom");
        assert!(err.contains(r#""code":5090"#));
        let err2 = err_response_with_data(&rid, 5092, "failed", r#"{"reason":"unknown"}"#);
        assert!(err2.contains(r#""data":{"reason":"unknown"}"#));
    }

    #[test]
    fn extract_helpers() {
        assert_eq!(extract_string_field(r#"{"profile":"bob","message":"hi"}"#, "profile").as_deref(), Some("bob"));
        assert_eq!(extract_string_field(r#"{"id":"abc"}"#, "id").as_deref(), Some("abc"));
        assert_eq!(extract_raw_value(r#"{"agents":[{"a":1},{"a":2}]}"#, "agents").unwrap(), r#"[{"a":1},{"a":2}]"#);
        assert_eq!(extract_raw_value(r#"{"agents":[]}"#, "agents").unwrap(), "[]");
        assert!(extract_raw_value(r#"{}"#, "agents").is_none());
    }
}
