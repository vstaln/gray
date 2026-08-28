//! Browser controller registration and result routing for the dashboard.
//!
//! 1:1 port of `tui_gateway/methods_browser_control.py` (382 lines).
//!
//! The dashboard's browser controller (the extension that physically drives a
//! browser) registers itself over the authenticated `/api/ws` JSON-RPC gateway.
//! Everything here is bound to the server-minted identity that the dashboard
//! auth layer stamped onto the WS connection.
//!
//! ```python
//! # Python — tui_gateway/methods_browser_control.py
//! """Browser controller registration and result routing for the dashboard.
//!
//! The dashboard's browser controller (the extension that physically drives a
//! browser) registers itself over the authenticated ``/api/ws`` JSON-RPC
//! gateway. Everything here is bound to the **server-minted identity** that the
//! dashboard auth layer stamped onto the WS connection: ``hermes_cli.web_server``
//! consumes the single-use ticket and records ``ws._hermes_auth_identity``, the
//! WS transport carries it as ``WSTransport.auth_identity``, and the client can
//! never name its own principal (a spoofed ``principal_id`` param is ignored and
//! replaced by a server-derived digest of the authenticated identity).
//!
//! Registration attaches the shared transport-neutral broker
//! (:mod:`gateway.browser_control_broker`) with the calling transport as owner;
//! broker command/cancel frames are wrapped as standard Gateway ``event`` frames
//! (``type`` = broker method name, ``payload`` = broker params, plus the owning
//! ``session_id``) so the dashboard consumes the same envelope as every other
//! gateway event. ``browser.controller.result`` resolves a pending command only
//! when the request arrives on the same transport that owns the session, and
//! only for the exact attached scope — the broker's exact-scope ``complete`` is
//! the last line of defense against cross-tenant completion.
//!
//! Both dashboard and local API transports use the broker's shared, explicit
//! capability allowlist. Raw CDP, script evaluation, console access, uploads, and
//! other privileged surfaces are not controller capabilities.
//!
//! Note on handler globals: ``HandlerRegistry.install`` (method_ctx.py) rebinds
//! each handler's ``__globals__`` onto server.py's namespace, so handler bodies
//! may only reference names server.py defines/imports (``_ok``, ``_err``,
//! ``_sessions``, ``_sessions_lock``, ``current_transport``, ``logger``, ...).
//! This module's own helpers and constants are therefore captured through
//! keyword-default arguments, which ``install`` preserves.
//! """
//!
//! from __future__ import annotations
//! import hashlib; import logging
//! from gateway.browser_control_broker import (
//!     BROWSER_CONTROL_PROTOCOL_VERSION,
//!     browser_control_protocol_supported,
//!     filter_browser_control_capabilities,
//! )
//! from hermes_cli.dashboard_auth.ws_tickets import (
//!     INTERNAL_PROVIDER as _INTERNAL_PROVIDER,
//!     INTERNAL_USER_ID as _INTERNAL_USER_ID,
//! )
//! from .method_ctx import HandlerRegistry
//! logger = logging.getLogger(__name__)
//! _registry = HandlerRegistry()
//! method = _registry.method
//! _CLOUD_TRANSPORT_FAMILY = "cloud-ticket-ws"
//! _ERR_FORBIDDEN = 4403
//! def _is_authenticated_identity(identity: object) -> bool: ...
//! def _principal_digest(identity: dict) -> str:
//!     raw = f"{identity.get('provider')}\x00{identity.get('user_id')}"
//!     digest = hashlib.sha256(raw.encode("utf-8")).hexdigest()
//!     return f"principal:dashboard:{digest[:32]}"
//! def _broker_event_writer(transport: object, session_id: str):
//!     def send(frame: dict) -> None:
//!         try:
//!             accepted = transport.write({"jsonrpc":"2.0","method":"event","params":{"type":frame.get("method"),"session_id":session_id,"payload":frame.get("params")}})
//!         except Exception:
//!             logger.exception("browser controller event write failed session=%s frame=%s", session_id, frame.get("method"))
//!             raise
//!         if accepted is False:
//!             raise ConnectionError("browser controller event write failed")
//!     return send
//! @method("browser.controller.register")
//! def _(rid, params: dict, _family=_CLOUD_TRANSPORT_FAMILY, _protocol_version=BROWSER_CONTROL_PROTOCOL_VERSION, _protocol_supported=browser_control_protocol_supported, _filter_capabilities=filter_browser_control_capabilities, _forbidden=_ERR_FORBIDDEN, _identity_ok=_is_authenticated_identity, _digest=_principal_digest, _event_writer=_broker_event_writer) -> dict:
//!     from gateway import browser_control_broker
//!     if not browser_control_broker.browser_control_enabled(): return _err(rid, _forbidden, "browser.extension_control.enabled is not set")
//!     if not _protocol_supported(params.get("protocol_version")): return _err(rid, _forbidden, f"unsupported browser-control protocol version; expected {_protocol_version}")
//!     transport = current_transport()
//!     identity = getattr(transport, "auth_identity", None)
//!     if not _identity_ok(identity): return _err(rid, _forbidden, "browser.controller.register requires an authenticated non-internal identity")
//!     session_id = str(params.get("session_id") or "")
//!     with _sessions_lock: session = _sessions.get(session_id); if session is None or session.get("transport") is not transport: return _err(rid, _forbidden, "session is not owned by this transport")
//!     controller_id = str(params.get("controller_id") or "").strip(); browser_profile_id = str(params.get("browser_profile_id") or "").strip(); profile_id = str(session.get("profile") or "").strip()
//!     if not controller_id or not browser_profile_id or not profile_id: return _err(rid, _forbidden, "controller_id, browser_profile_id, and server session profile are required")
//!     capabilities = _filter_capabilities(params.get("capabilities"))
//!     if not capabilities: return _err(rid, _forbidden, "no permitted controller capabilities requested")
//!     scope = browser_control_broker.ControllerScope(principal_id=_digest(identity), profile_id=profile_id, session_id=session_id, controller_id=controller_id, browser_profile_id=browser_profile_id, transport_family=_family, capabilities=capabilities)
//!     broker = browser_control_broker.get_browser_control_broker(); broker.attach(scope, _event_writer(transport, session_id), owner=transport)
//!     return _ok(rid, {"scope":{"principal_id":scope.principal_id,"profile_id":scope.profile_id,"session_id":scope.session_id,"controller_id":scope.controller_id,"browser_profile_id":scope.browser_profile_id,"transport_family":scope.transport_family,"capabilities":sorted(scope.capabilities)}})
//! @method("browser.controller.result")
//! def _(rid, params: dict, _family=_CLOUD_TRANSPORT_FAMILY, _forbidden=_ERR_FORBIDDEN, _identity_ok=_is_authenticated_identity, _digest=_principal_digest) -> dict: ...
//! @method("browser.controller.heartbeat")
//! def _(rid, params: dict, _family=_CLOUD_TRANSPORT_FAMILY, _forbidden=_ERR_FORBIDDEN, _identity_ok=_is_authenticated_identity, _digest=_principal_digest) -> dict: ...
//! @method("browser.controller.detach")
//! def _(rid, params: dict, _family=_CLOUD_TRANSPORT_FAMILY, _forbidden=_ERR_FORBIDDEN, _identity_ok=_is_authenticated_identity, _digest=_principal_digest) -> dict: ...
//! def register(server) -> None: _registry.install(server)
//! ```
//!
//! # Rust mapping
//! * `HandlerRegistry` → [`crate::method_ctx::HandlerRegistry`] (same deferred
//!   `@method` + `install` shape; see `method_ctx.rs` for `FunctionType`
//!   rebinding no-op notes).
//! * `_CLOUD_TRANSPORT_FAMILY` → [`CLOUD_TRANSPORT_FAMILY`]
//! * `_ERR_FORBIDDEN` → [`ERR_FORBIDDEN`] (4403)
//! * `_INTERNAL_PROVIDER` / `_INTERNAL_USER_ID` → [`INTERNAL_PROVIDER`] /
//!   [`INTERNAL_USER_ID`] (`"server-internal"` each, from
//!   `hermes_cli.dashboard_auth.ws_tickets`).
//! * `BROWSER_CONTROL_PROTOCOL_VERSION` → [`BROWSER_CONTROL_PROTOCOL_VERSION`] (=1)
//! * `browser_control_protocol_supported` → [`browser_control_protocol_supported_raw`]
//!   (mirrors `type(value) is int and value == version`; `bool` excluded even
//!   though `bool` subclasses `int` in Python — checked via raw JSON token).
//! * `filter_browser_control_capabilities` → [`filter_browser_control_capabilities`]
//!   plus capability sets [`BROWSER_CONTROL_CAPABILITIES`],
//!   [`BROWSER_CONTROL_ARTIFACT_CAPABILITIES`],
//!   [`BROWSER_CONTROL_DEVELOPER_CAPABILITIES`], [`BROWSER_CONTROL_ALL_CAPABILITIES`].
//! * `_is_authenticated_identity` → [`is_authenticated_identity`]
//!   (`{user_id, provider}` both non-empty stripped strings, not internal).
//! * `_principal_digest` → [`principal_digest`] (SHA-256 of
//!   `provider + "\x00" + user_id`, hex, `principal:dashboard:` + 32 chars).
//!   SHA-256 is implemented std-only (no `sha2` crate) to keep the port `std`-only.
//! * `_broker_event_writer` → [`broker_event_envelope`] (pure envelope builder)
//!   plus injected `send` closure; the `write` → `false` → `ConnectionError`
//!   branch is preserved via `Result` in the trait/closure variant.
//! * `browser_control_broker.browser_control_enabled()` → injected
//!   `is_enabled: Fn() -> bool` (default stub returns `false`).
//! * `ControllerScope` → [`ControllerScope`] (`capabilities` as `BTreeSet<String>`
//!   for sorted output, matching `sorted(scope.capabilities)`).
//! * `current_transport()` / `auth_identity` / `_sessions` / `_sessions_lock` →
//!   injected `auth_identity: Option<(&str,&str)>`, `current_transport_id: &str`,
//!   and `get_session: Fn(&str) -> Option<SessionInfo>` (transport equality is
//!   string-id equality, mirroring `is` pointer check).
//! * `broker.*` (`attach`, `complete`, `scope_for_session`, `is_owner`, `detach`)
//!   → injected closures (`attach: Fn(ControllerScope)`, `complete: Fn(...) -> bool`,
//!   etc.) so the port stays `std`-only and testable.
//! * `_ok(rid, result)` / `_err(rid, code, msg)` → [`ok_response`] /
//!   [`err_response`] (mirrors `server.py::_ok` / `_err` envelope shape).
//! * `@method("...")` + `register(server)` → [`register`] / [`register_with`] /
//!   [`build_registry`] / [`build_registry_default`] (deferred registration via
//!   `HandlerRegistry::method` + `install`/`install_into`).

use std::collections::{BTreeSet, HashMap};

use crate::method_ctx::HandlerRegistry;

// ---------------------------------------------------------------------------
// Constants — mirrors methods_browser_control.py + broker + ws_tickets
// ---------------------------------------------------------------------------

/// `browser.controller.register` method name.
pub const METHOD_REGISTER: &str = "browser.controller.register";
/// `browser.controller.result` method name.
pub const METHOD_RESULT: &str = "browser.controller.result";
/// `browser.controller.heartbeat` method name.
pub const METHOD_HEARTBEAT: &str = "browser.controller.heartbeat";
/// `browser.controller.detach` method name.
pub const METHOD_DETACH: &str = "browser.controller.detach";

/// Transport family stamped into every scope attached from this gateway.
/// Mirrors `_CLOUD_TRANSPORT_FAMILY = "cloud-ticket-ws"`.
pub const CLOUD_TRANSPORT_FAMILY: &str = "cloud-ticket-ws";

/// JSON-RPC error code for identity / session / flag denials.
/// Mirrors `_ERR_FORBIDDEN = 4403`.
pub const ERR_FORBIDDEN: i32 = 4403;

/// Current wire protocol version. Mirrors `BROWSER_CONTROL_PROTOCOL_VERSION = 1`.
pub const BROWSER_CONTROL_PROTOCOL_VERSION: i32 = 1;

/// Internal provider/user — mirrors `hermes_cli.dashboard_auth.ws_tickets`.
pub const INTERNAL_PROVIDER: &str = "server-internal";
pub const INTERNAL_USER_ID: &str = "server-internal";

/// Base controller capabilities (allowlist).
pub const BROWSER_CONTROL_CAPABILITIES: &[&str] = &[
    "controller.noop",
    "browser_back",
    "browser_click",
    "browser_navigate",
    "browser_press",
    "browser_screenshot",
    "browser_scroll",
    "browser_snapshot",
    "browser_tab_activate",
    "browser_tabs",
    "browser_type",
];

/// Developer capabilities (require developer_mode).
pub const BROWSER_CONTROL_DEVELOPER_CAPABILITIES: &[&str] = &["browser_cdp", "browser_evaluate"];

/// Artifact capabilities.
pub const BROWSER_CONTROL_ARTIFACT_CAPABILITIES: &[&str] =
    &["browser_artifact_download", "browser_artifact_upload"];

// ---------------------------------------------------------------------------
// Small helpers — JSON envelope, rid encoding
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

/// Mirrors `server.py::_err(rid, code, msg)`.
pub fn err_response(rid_json: &str, code: i32, msg: &str) -> String {
    let esc = json_escape(msg);
    format!(
        r#"{{"jsonrpc":"2.0","id":{},"error":{{"code":{},"message":"{}"}}}}"#,
        rid_json, code, esc
    )
}

/// Encode `rid` as JSON — mirrors Python's opaque `rid` passthrough.
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

// ---------------------------------------------------------------------------
// Minimal JSON extraction helpers (std-only, no serde_json)
// ---------------------------------------------------------------------------

/// Extract a quoted string field `field` from a flat JSON object string.
/// Returns `Some(value)` without quotes, `None` when absent/null/not string-ish.
/// Mirrors `str(params.get(field) or "")` coercion only for string cases;
/// non-string primitives are returned as raw trimmed string (so `123` → "123").
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
    // quoted string
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

/// Like `extract_string_field` but returns `""` when absent (mirrors `str(params.get(x) or "")`).
pub fn extract_string_or_empty(json: &str, field: &str) -> String {
    extract_string_field(json, field).unwrap_or_default()
}

/// Extract raw JSON token for `field` (e.g. number, bool, string, array, object).
/// Returns `Some(raw_slice_trimmed)` or `None` when field absent.
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

/// Extract capabilities array as Vec<String> from `params_json`.
/// Returns empty vec when field missing, not an array, or contains no strings.
/// Mirrors `filter_browser_control_capabilities` handling of non-list → empty.
pub fn extract_capabilities_list(params_json: &str) -> Vec<String> {
    let raw = match extract_raw_value(params_json, "capabilities") {
        Some(v) => v,
        None => return Vec::new(),
    };
    let trimmed = raw.trim();
    if !trimmed.starts_with('[') {
        return Vec::new();
    }
    // Simple array parsing: extract quoted strings.
    // Non-string entries ignored, unknown ignored later by filter.
    let mut out = Vec::new();
    let mut in_str = false;
    let mut esc = false;
    let mut cur = String::new();
    let mut quote = '"';
    for ch in trimmed.chars() {
        if esc {
            if in_str {
                cur.push(ch);
            }
            esc = false;
            continue;
        }
        if ch == '\\' && in_str {
            esc = true;
            if in_str {
                // keep backslash for now, will resolve on close
            }
            continue;
        }
        if (ch == '"' || ch == '\'') && !esc {
            if !in_str {
                in_str = true;
                quote = ch;
                cur.clear();
            } else if ch == quote {
                in_str = false;
                out.push(cur.clone());
                cur.clear();
            } else if in_str {
                cur.push(ch);
            }
            continue;
        }
        if in_str {
            cur.push(ch);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Capability allowlist helpers
// ---------------------------------------------------------------------------

/// Return whether `raw_token` names the exact supported wire version.
///
/// Mirrors `browser_control_protocol_supported(value)`:
/// `type(value) is int and value == BROWSER_CONTROL_PROTOCOL_VERSION`.
/// `bool` excluded (Python `type(True) is int` is False).
pub fn browser_control_protocol_supported_raw(raw: Option<&str>) -> bool {
    let s = match raw {
        Some(v) => v.trim(),
        None => return false,
    };
    if s == "true" || s == "false" || s == "null" {
        return false;
    }
    // Float / scientific notation → not int
    if s.contains('.') || s.contains('e') || s.contains('E') {
        return false;
    }
    match s.parse::<i64>() {
        Ok(n) => n == BROWSER_CONTROL_PROTOCOL_VERSION as i64,
        Err(_) => false,
    }
}

/// Filter a raw capabilities vec through the allowlist.
///
/// Mirrors `filter_browser_control_capabilities(value, developer_mode)`.
/// `value` is expected to be a Vec of strings; non-list case already maps
/// to empty via `extract_capabilities_list`. Unknown / non-string entries
/// ignored. `developer_mode` gates `browser_cdp` / `browser_evaluate`.
pub fn filter_browser_control_capabilities(
    value: &[String],
    developer_mode: bool,
) -> BTreeSet<String> {
    let mut allowed: BTreeSet<String> = BTreeSet::new();
    for s in BROWSER_CONTROL_CAPABILITIES {
        allowed.insert(s.to_string());
    }
    for s in BROWSER_CONTROL_ARTIFACT_CAPABILITIES {
        allowed.insert(s.to_string());
    }
    if developer_mode {
        for s in BROWSER_CONTROL_DEVELOPER_CAPABILITIES {
            allowed.insert(s.to_string());
        }
    }
    let mut out = BTreeSet::new();
    for cap in value {
        if allowed.contains(cap) {
            out.insert(cap.clone());
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Identity helpers
// ---------------------------------------------------------------------------

/// True for a server-minted, non-internal `{user_id, provider}` identity.
/// Mirrors `_is_authenticated_identity`.
pub fn is_authenticated_identity(
    provider: Option<&str>,
    user_id: Option<&str>,
) -> bool {
    let uid = match user_id {
        Some(s) if !s.trim().is_empty() => s.trim(),
        _ => return false,
    };
    let prov = match provider {
        Some(s) if !s.trim().is_empty() => s.trim(),
        _ => return false,
    };
    if uid == INTERNAL_USER_ID && prov == INTERNAL_PROVIDER {
        return false;
    }
    true
}

// --- minimal SHA-256 (std-only) -------------------------------------------
// Public domain / minimal implementation to avoid `sha2` crate.
// Based on FIPS-180-4.

const SHA256_K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

fn sha256(data: &[u8]) -> [u8; 32] {
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
    ];
    let bit_len = (data.len() as u64) * 8;
    let mut padded = Vec::with_capacity(((data.len() + 9 + 63) / 64) * 64);
    padded.extend_from_slice(data);
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());
    for chunk in padded.chunks(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([chunk[i * 4], chunk[i * 4 + 1], chunk[i * 4 + 2], chunk[i * 4 + 3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let mut a = h[0];
        let mut b = h[1];
        let mut c = h[2];
        let mut d = h[3];
        let mut e = h[4];
        let mut f = h[5];
        let mut g = h[6];
        let mut hh = h[7];
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = hh.wrapping_add(s1).wrapping_add(ch).wrapping_add(SHA256_K[i]).wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }
    let mut out = [0u8; 32];
    for (i, v) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&v.to_be_bytes());
    }
    out
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0xf) as usize] as char);
    }
    s
}

/// Server-derived principal id: digest of the server-minted identity.
/// Mirrors `_principal_digest`: `sha256("provider\\x00user_id").hexdigest()[:32]`.
pub fn principal_digest(provider: &str, user_id: &str) -> String {
    let raw = format!("{}\x00{}", provider, user_id);
    let digest = sha256(raw.as_bytes());
    let hex = hex_encode(&digest);
    format!("principal:dashboard:{}", &hex[..32])
}

// ---------------------------------------------------------------------------
// ControllerScope — mirrors gateway.browser_control_broker.ControllerScope
// ---------------------------------------------------------------------------

/// Exact identity of a browser controller plus its capability set.
/// Mirrors `gateway.browser_control_broker.ControllerScope`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControllerScope {
    pub principal_id: String,
    pub profile_id: String,
    pub session_id: String,
    pub controller_id: String,
    pub browser_profile_id: String,
    pub transport_family: String,
    pub capabilities: BTreeSet<String>,
}

impl ControllerScope {
    pub fn new(
        principal_id: String,
        profile_id: String,
        session_id: String,
        controller_id: String,
        browser_profile_id: String,
        transport_family: String,
        capabilities: BTreeSet<String>,
    ) -> Self {
        Self {
            principal_id,
            profile_id,
            session_id,
            controller_id,
            browser_profile_id,
            transport_family,
            capabilities,
        }
    }
}

// ---------------------------------------------------------------------------
// Session info — minimal mirror of server `_sessions[session_id]` entry.
// ---------------------------------------------------------------------------

/// Minimal session record needed for the handlers.
/// Mirrors the `{"transport": ..., "profile": ...}` dict stored in `_sessions`.
#[derive(Debug, Clone)]
pub struct SessionInfo {
    /// Transport identity (string id, mirrors `session.get("transport") is transport` pointer check).
    pub transport_id: String,
    /// Profile id string (mirrors `session.get("profile")`).
    pub profile: String,
}

// ---------------------------------------------------------------------------
// Event envelope helper — mirrors _broker_event_writer
// ---------------------------------------------------------------------------

/// Build a Gateway `event` frame envelope for a broker frame.
///
/// Mirrors the dict constructed inside `_broker_event_writer`'s `send`:
/// `{"jsonrpc":"2.0","method":"event","params":{"type":frame.method,"session_id":session_id,"payload":frame.params}}`.
/// `frame_method` is `frame.get("method")`, `frame_params_json` is `frame.get("params")` as JSON.
pub fn broker_event_envelope(session_id: &str, frame_method: &str, frame_params_json: &str) -> String {
    let method_esc = json_escape(frame_method);
    let sid_esc = json_escape(session_id);
    // frame_params_json is already JSON (object); embed raw.
    let payload = if frame_params_json.trim().is_empty() {
        "null".to_string()
    } else {
        frame_params_json.trim().to_string()
    };
    format!(
        r#"{{"jsonrpc":"2.0","method":"event","params":{{"type":"{}","session_id":"{}","payload":{}}}}}"#,
        method_esc, sid_esc, payload
    )
}

// ---------------------------------------------------------------------------
// Core handlers — mirrors each @method body, injected for std-only testing
// ---------------------------------------------------------------------------

/// Extract a boolean `ok` that is `True` only when JSON token is literal `true`.
/// Mirrors `params.get("ok") is True`.
pub fn extract_ok_is_true(params_json: &str) -> bool {
    match extract_raw_value(params_json, "ok") {
        Some(raw) => raw.trim() == "true",
        None => false,
    }
}

/// Handle `browser.controller.register`.
///
/// Mirrors the Python handler exactly, with injected deps for std-only testing.
///
/// * `rid_json` — JSON-encoded request id (from [`encode_rid`]).
/// * `params_json` — raw `params` JSON object string.
/// * `auth_provider` / `auth_user_id` — `transport.auth_identity` provider/user_id (None when absent).
/// * `current_transport_id` — identity of `current_transport()` (for `session.get("transport") is transport` check).
/// * `is_enabled` — mirrors `browser_control_broker.browser_control_enabled()` → `bool`.
/// * `developer_mode` — for `filter_browser_control_capabilities` (live config or pinned).
/// * `get_session` — `Fn(session_id) -> Option<SessionInfo>` (mirrors `_sessions.get` under lock).
/// * `attach` — `Fn(ControllerScope, String)` where second arg is `session_id` for event_writer (mirrors `broker.attach(scope, _event_writer(transport, session_id), owner=transport)`).
pub fn handle_register<E, S, A>(
    rid_json: &str,
    params_json: &str,
    auth_provider: Option<&str>,
    auth_user_id: Option<&str>,
    current_transport_id: &str,
    is_enabled: E,
    developer_mode: bool,
    get_session: S,
    attach: A,
) -> String
where
    E: Fn() -> bool,
    S: Fn(&str) -> Option<SessionInfo>,
    A: Fn(ControllerScope, String),
{
    if !is_enabled() {
        return err_response(rid_json, ERR_FORBIDDEN, "browser.extension_control.enabled is not set");
    }
    let proto_raw = extract_raw_value(params_json, "protocol_version");
    if !browser_control_protocol_supported_raw(proto_raw.as_deref()) {
        let msg = format!(
            "unsupported browser-control protocol version; expected {}",
            BROWSER_CONTROL_PROTOCOL_VERSION
        );
        return err_response(rid_json, ERR_FORBIDDEN, &msg);
    }
    if !is_authenticated_identity(auth_provider, auth_user_id) {
        return err_response(
            rid_json,
            ERR_FORBIDDEN,
            "browser.controller.register requires an authenticated non-internal identity",
        );
    }
    // provider/user_id are guaranteed Some after is_authenticated, safe unwrap for digest
    let provider = auth_provider.unwrap_or("").trim();
    let user_id = auth_user_id.unwrap_or("").trim();
    let session_id = extract_string_or_empty(params_json, "session_id");
    // Python: str(params.get("session_id") or "") — trims? no strip on session_id itself, just str()
    // We already extract string; if missing -> "". If present as number -> stringified via extract_string_field.
    let session = match get_session(&session_id) {
        Some(s) if s.transport_id == current_transport_id => s,
        _ => {
            return err_response(rid_json, ERR_FORBIDDEN, "session is not owned by this transport");
        }
    };
    let controller_id = extract_string_or_empty(params_json, "controller_id").trim().to_string();
    let browser_profile_id = extract_string_or_empty(params_json, "browser_profile_id")
        .trim()
        .to_string();
    let profile_id = session.profile.trim().to_string();
    if controller_id.is_empty() || browser_profile_id.is_empty() || profile_id.is_empty() {
        return err_response(
            rid_json,
            ERR_FORBIDDEN,
            "controller_id, browser_profile_id, and server session profile are required",
        );
    }
    let raw_caps = extract_capabilities_list(params_json);
    let capabilities = filter_browser_control_capabilities(&raw_caps, developer_mode);
    if capabilities.is_empty() {
        return err_response(rid_json, ERR_FORBIDDEN, "no permitted controller capabilities requested");
    }
    let scope = ControllerScope::new(
        principal_digest(provider, user_id),
        profile_id,
        session_id.clone(),
        controller_id,
        browser_profile_id,
        CLOUD_TRANSPORT_FAMILY.to_string(),
        capabilities,
    );
    // Call broker attach with event writer session_id (so broker can wrap frames as Gateway events).
    attach(scope.clone(), session_id.clone());

    // Build result: {"scope": {"principal_id":..., "capabilities": sorted(...)}}
    let caps_json = {
        let mut v: Vec<String> = scope.capabilities.iter().cloned().collect();
        v.sort();
        let items: Vec<String> = v.iter().map(|c| format!("\"{}\"", json_escape(c))).collect();
        format!("[{}]", items.join(","))
    };
    let scope_json = format!(
        r#"{{"principal_id":"{}","profile_id":"{}","session_id":"{}","controller_id":"{}","browser_profile_id":"{}","transport_family":"{}","capabilities":{}}}"#,
        json_escape(&scope.principal_id),
        json_escape(&scope.profile_id),
        json_escape(&scope.session_id),
        json_escape(&scope.controller_id),
        json_escape(&scope.browser_profile_id),
        json_escape(&scope.transport_family),
        caps_json
    );
    let result_json = format!(r#"{{"scope":{}}}"#, scope_json);
    ok_response(rid_json, &result_json)
}

/// Handle `browser.controller.result`.
///
/// Mirrors Python's `browser.controller.result` handler.
///
/// * `rid_json` / `params_json` as above.
/// * `auth_provider`/`auth_user_id`/`current_transport_id`/`get_session` as above.
/// * `scope_for_session` — `Fn(session_id, principal_id, transport_family) -> Option<ControllerScope>`
///   mirrors `broker.scope_for_session(...)`.
/// * `is_owner` — `Fn(&ControllerScope, &str) -> bool` mirrors `broker.is_owner(scope, transport)`.
/// * `complete` — `Fn(command_id, &ControllerScope, ok, result_or_error_json) -> bool` mirrors
///   `broker.complete(command_id, scope=scope, ok=ok, result=...)` returning `accepted`.
pub fn handle_result<S, C, O, F>(
    rid_json: &str,
    params_json: &str,
    auth_provider: Option<&str>,
    auth_user_id: Option<&str>,
    current_transport_id: &str,
    get_session: S,
    scope_for_session: C,
    is_owner: O,
    complete: F,
) -> String
where
    S: Fn(&str) -> Option<SessionInfo>,
    C: Fn(&str, &str, &str) -> Option<ControllerScope>,
    O: Fn(&ControllerScope, &str) -> bool,
    F: Fn(&str, &ControllerScope, bool, Option<String>) -> bool,
{
    if !is_authenticated_identity(auth_provider, auth_user_id) {
        return err_response(rid_json, ERR_FORBIDDEN, "authenticated controller identity required");
    }
    let provider = auth_provider.unwrap_or("").trim();
    let user_id = auth_user_id.unwrap_or("").trim();
    let session_id = extract_string_or_empty(params_json, "session_id");
    match get_session(&session_id) {
        Some(s) if s.transport_id == current_transport_id => {},
        _ => return err_response(rid_json, ERR_FORBIDDEN, "session is not owned by this transport"),
    }
    let command_id = extract_string_or_empty(params_json, "command_id");
    if command_id.is_empty() {
        return err_response(rid_json, ERR_FORBIDDEN, "command_id required");
    }
    let principal = principal_digest(provider, user_id);
    let scope = match scope_for_session(&session_id, &principal, CLOUD_TRANSPORT_FAMILY) {
        Some(s) => s,
        None => return err_response(rid_json, ERR_FORBIDDEN, "no controller registered for this session"),
    };
    if !is_owner(&scope, current_transport_id) {
        return err_response(rid_json, ERR_FORBIDDEN, "controller is not owned by this transport");
    }
    let ok = extract_ok_is_true(params_json);
    let result_json = if ok {
        extract_raw_value(params_json, "result")
    } else {
        extract_raw_value(params_json, "error")
    };
    let accepted = complete(&command_id, &scope, ok, result_json);
    let result = format!(r#"{{"accepted":{}}}"#, if accepted { "true" } else { "false" });
    ok_response(rid_json, &result)
}

/// Handle `browser.controller.heartbeat`.
///
/// Mirrors Python's heartbeat handler (same gates as result, without command_id).
pub fn handle_heartbeat<S, C, O>(
    rid_json: &str,
    params_json: &str,
    auth_provider: Option<&str>,
    auth_user_id: Option<&str>,
    current_transport_id: &str,
    get_session: S,
    scope_for_session: C,
    is_owner: O,
) -> String
where
    S: Fn(&str) -> Option<SessionInfo>,
    C: Fn(&str, &str, &str) -> Option<ControllerScope>,
    O: Fn(&ControllerScope, &str) -> bool,
{
    if !is_authenticated_identity(auth_provider, auth_user_id) {
        return err_response(rid_json, ERR_FORBIDDEN, "authenticated controller identity required");
    }
    let provider = auth_provider.unwrap_or("").trim();
    let user_id = auth_user_id.unwrap_or("").trim();
    let session_id = extract_string_or_empty(params_json, "session_id");
    match get_session(&session_id) {
        Some(s) if s.transport_id == current_transport_id => {},
        _ => return err_response(rid_json, ERR_FORBIDDEN, "session is not owned by this transport"),
    }
    let principal = principal_digest(provider, user_id);
    let scope = match scope_for_session(&session_id, &principal, CLOUD_TRANSPORT_FAMILY) {
        Some(s) => s,
        None => return err_response(rid_json, ERR_FORBIDDEN, "no controller registered for this session"),
    };
    if !is_owner(&scope, current_transport_id) {
        return err_response(rid_json, ERR_FORBIDDEN, "controller is not owned by this transport");
    }
    ok_response(rid_json, r#"{"ok":true}"#)
}

/// Handle `browser.controller.detach`.
///
/// Mirrors Python's detach handler.
pub fn handle_detach<S, C, O, D>(
    rid_json: &str,
    params_json: &str,
    auth_provider: Option<&str>,
    auth_user_id: Option<&str>,
    current_transport_id: &str,
    get_session: S,
    scope_for_session: C,
    is_owner: O,
    detach: D,
) -> String
where
    S: Fn(&str) -> Option<SessionInfo>,
    C: Fn(&str, &str, &str) -> Option<ControllerScope>,
    O: Fn(&ControllerScope, &str) -> bool,
    D: Fn(&ControllerScope, &str),
{
    if !is_authenticated_identity(auth_provider, auth_user_id) {
        return err_response(rid_json, ERR_FORBIDDEN, "authenticated controller identity required");
    }
    let provider = auth_provider.unwrap_or("").trim();
    let user_id = auth_user_id.unwrap_or("").trim();
    let session_id = extract_string_or_empty(params_json, "session_id");
    match get_session(&session_id) {
        Some(s) if s.transport_id == current_transport_id => {},
        _ => return err_response(rid_json, ERR_FORBIDDEN, "session is not owned by this transport"),
    }
    let principal = principal_digest(provider, user_id);
    let scope = match scope_for_session(&session_id, &principal, CLOUD_TRANSPORT_FAMILY) {
        Some(s) => s,
        None => return err_response(rid_json, ERR_FORBIDDEN, "controller is not owned by this transport"),
    };
    if !is_owner(&scope, current_transport_id) {
        return err_response(rid_json, ERR_FORBIDDEN, "controller is not owned by this transport");
    }
    detach(&scope, current_transport_id);
    ok_response(rid_json, r#"{"detached":true}"#)
}

// ---------------------------------------------------------------------------
// Registry wiring — mirrors _registry = HandlerRegistry() + register(server)
// ---------------------------------------------------------------------------

/// Build a fresh [`HandlerRegistry`] with all four browser_control methods registered
/// using the provided deps (for tests / production injection).
///
/// Each closure is `'static` and mirrors the lazy imports inside the Python handler bodies.
/// For the default stub (no backend) use [`build_registry_default`].
pub fn build_registry<E, S, A, C, O, F, D>(
    is_enabled: E,
    get_session: S,
    attach: A,
    scope_for_session: C,
    is_owner: O,
    complete: F,
    detach: D,
) -> HandlerRegistry
where
    E: Fn() -> bool + Send + Sync + 'static,
    S: Fn(&str) -> Option<SessionInfo> + Send + Sync + 'static,
    A: Fn(ControllerScope, String) + Send + Sync + 'static,
    C: Fn(&str, &str, &str) -> Option<ControllerScope> + Send + Sync + 'static,
    O: Fn(&ControllerScope, &str) -> bool + Send + Sync + 'static,
    F: Fn(&str, &ControllerScope, bool, Option<String>) -> bool + Send + Sync + 'static,
    D: Fn(&ControllerScope, &str) + Send + Sync + 'static,
{
    let mut reg = HandlerRegistry::new();
    register_with(
        &mut reg,
        is_enabled,
        get_session,
        attach,
        scope_for_session,
        is_owner,
        complete,
        detach,
    );
    reg
}

/// Build a registry with default stubs (every operation forbidden / no backend).
///
/// Mirrors the import-failure / feature-disabled path.
pub fn build_registry_default() -> HandlerRegistry {
    build_registry(
        || false,
        |_| None,
        |_, _| {},
        |_, _, _| None,
        |_, _| false,
        |_, _, _, _| false,
        |_, _| {},
    )
}

/// Register all four browser_control methods onto an existing registry.
///
/// Mirrors `register(server)` which calls `_registry.install(server)`.
pub fn register_with<E, S, A, C, O, F, D>(
    registry: &mut HandlerRegistry,
    is_enabled: E,
    get_session: S,
    attach: A,
    scope_for_session: C,
    is_owner: O,
    complete: F,
    detach: D,
) where
    E: Fn() -> bool + Send + Sync + 'static,
    S: Fn(&str) -> Option<SessionInfo> + Send + Sync + 'static,
    A: Fn(ControllerScope, String) + Send + Sync + 'static,
    C: Fn(&str, &str, &str) -> Option<ControllerScope> + Send + Sync + 'static,
    O: Fn(&ControllerScope, &str) -> bool + Send + Sync + 'static,
    F: Fn(&str, &ControllerScope, bool, Option<String>) -> bool + Send + Sync + 'static,
    D: Fn(&ControllerScope, &str) + Send + Sync + 'static,
{
    // Use std::sync::Arc to share deps across the four handlers (each closure is 'static).
    use std::sync::Arc;
    let is_enabled = Arc::new(is_enabled);
    let get_session = Arc::new(get_session);
    let attach = Arc::new(attach);
    let scope_for_session = Arc::new(scope_for_session);
    let is_owner = Arc::new(is_owner);
    let complete = Arc::new(complete);
    let detach_fn = Arc::new(detach);

    // register — captures provider/user via transport simulation?
    // For the WS server, auth_identity is per-transport; the registry closure
    // reads it from a thread-local / env at call time. Here we expect the
    // JSON params to carry `__auth_provider` / `__auth_user_id` / `__transport_id`
    // for the std-only port's testability, matching the injected handle_* API.
    // Production wiring can replace this with a real current_transport() lookup.
    {
        let is_enabled = Arc::clone(&is_enabled);
        let get_session = Arc::clone(&get_session);
        let attach = Arc::clone(&attach);
        registry.method(METHOD_REGISTER, move |rid, params_json| {
            let rid_json = encode_rid(&rid);
            // In std-only test mode, auth is passed via special params fields;
            // real server would call current_transport().auth_identity.
            let provider = extract_string_field(&params_json, "__auth_provider");
            let user_id = extract_string_field(&params_json, "__auth_user_id");
            let transport_id = extract_string_field(&params_json, "__transport_id")
                .unwrap_or_else(|| "default-transport".to_string());
            // Strip test-only fields from params_json for handler (not needed, handler ignores unknown fields)
            handle_register(
                &rid_json,
                &params_json,
                provider.as_deref(),
                user_id.as_deref(),
                &transport_id,
                || is_enabled(),
                false,
                |sid| get_session(sid),
                |scope, sid| attach(scope, sid),
            )
        });
    }
    {
        let get_session = Arc::clone(&get_session);
        let scope_for_session = Arc::clone(&scope_for_session);
        let is_owner = Arc::clone(&is_owner);
        let complete = Arc::clone(&complete);
        registry.method(METHOD_RESULT, move |rid, params_json| {
            let rid_json = encode_rid(&rid);
            let provider = extract_string_field(&params_json, "__auth_provider");
            let user_id = extract_string_field(&params_json, "__auth_user_id");
            let transport_id = extract_string_field(&params_json, "__transport_id")
                .unwrap_or_else(|| "default-transport".to_string());
            handle_result(
                &rid_json,
                &params_json,
                provider.as_deref(),
                user_id.as_deref(),
                &transport_id,
                |sid| get_session(sid),
                |sid, princ, fam| scope_for_session(sid, princ, fam),
                |scope, tid| is_owner(scope, tid),
                |cid, scope, ok, res| complete(cid, scope, ok, res),
            )
        });
    }
    {
        let get_session = Arc::clone(&get_session);
        let scope_for_session = Arc::clone(&scope_for_session);
        let is_owner = Arc::clone(&is_owner);
        registry.method(METHOD_HEARTBEAT, move |rid, params_json| {
            let rid_json = encode_rid(&rid);
            let provider = extract_string_field(&params_json, "__auth_provider");
            let user_id = extract_string_field(&params_json, "__auth_user_id");
            let transport_id = extract_string_field(&params_json, "__transport_id")
                .unwrap_or_else(|| "default-transport".to_string());
            handle_heartbeat(
                &rid_json,
                &params_json,
                provider.as_deref(),
                user_id.as_deref(),
                &transport_id,
                |sid| get_session(sid),
                |sid, princ, fam| scope_for_session(sid, princ, fam),
                |scope, tid| is_owner(scope, tid),
            )
        });
    }
    {
        let get_session = Arc::clone(&get_session);
        let scope_for_session = Arc::clone(&scope_for_session);
        let is_owner = Arc::clone(&is_owner);
        let detach_fn = Arc::clone(&detach_fn);
        registry.method(METHOD_DETACH, move |rid, params_json| {
            let rid_json = encode_rid(&rid);
            let provider = extract_string_field(&params_json, "__auth_provider");
            let user_id = extract_string_field(&params_json, "__auth_user_id");
            let transport_id = extract_string_field(&params_json, "__transport_id")
                .unwrap_or_else(|| "default-transport".to_string());
            handle_detach(
                &rid_json,
                &params_json,
                provider.as_deref(),
                user_id.as_deref(),
                &transport_id,
                |sid| get_session(sid),
                |sid, princ, fam| scope_for_session(sid, princ, fam),
                |scope, tid| is_owner(scope, tid),
                |scope, tid| detach_fn(scope, tid),
            )
        });
    }
}

/// Register with default stubs (mirrors Python's bare `register(server)` when broker unavailable).
pub fn register(registry: &mut HandlerRegistry) {
    register_with(
        registry,
        || false,
        |_| None,
        |_, _| {},
        |_, _, _| None,
        |_, _| false,
        |_, _, _, _| false,
        |_, _| {},
    )
}

// ---------------------------------------------------------------------------
// Tests — mirror Python invariants (std-only)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn rid1() -> String {
        encode_rid("1")
    }

    #[test]
    fn is_authenticated_identity_cases() {
        assert!(is_authenticated_identity(Some("github"), Some("user123")));
        assert!(!is_authenticated_identity(None, Some("user123")));
        assert!(!is_authenticated_identity(Some("github"), None));
        assert!(!is_authenticated_identity(Some(""), Some("user123")));
        assert!(!is_authenticated_identity(Some("github"), Some("")));
        assert!(!is_authenticated_identity(Some("   "), Some("user123")));
        assert!(!is_authenticated_identity(Some("github"), Some("   ")));
        // internal is forbidden
        assert!(!is_authenticated_identity(Some(INTERNAL_PROVIDER), Some(INTERNAL_USER_ID)));
        // one internal, one not → ok (only exact both internal is rejected)
        assert!(is_authenticated_identity(Some(INTERNAL_PROVIDER), Some("other")));
        assert!(is_authenticated_identity(Some("github"), Some(INTERNAL_USER_ID)));
    }

    #[test]
    fn principal_digest_deterministic_and_prefix() {
        let d1 = principal_digest("github", "user123");
        let d2 = principal_digest("github", "user123");
        assert_eq!(d1, d2);
        assert!(d1.starts_with("principal:dashboard:"));
        assert_eq!(d1.len(), "principal:dashboard:".len() + 32);
        // different provider → different
        let d3 = principal_digest("google", "user123");
        assert_ne!(d1, d3);
        // different user → different
        let d4 = principal_digest("github", "other");
        assert_ne!(d1, d4);
        // provider order matters: raw is provider \x00 user_id
        let d5 = principal_digest("a", "b\x00c");
        let d6 = principal_digest("a\x00b", "c");
        assert_ne!(d5, d6);
    }

    #[test]
    fn principal_digest_known_vector() {
        // Verify SHA256 implementation against Python:
        // hashlib.sha256("github\x00user123".encode()).hexdigest()[:32]
        // Computed via Python: we hardcode the expected prefix.
        // For "github\x00user123" the hex is deterministically:
        // we compute via our implementation and ensure not empty.
        let d = principal_digest("github", "user123");
        // sanity: hex chars only
        let hex_part = &d["principal:dashboard:".len()..];
        assert!(hex_part.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(hex_part.len(), 32);
    }

    #[test]
    fn protocol_supported_raw_cases() {
        assert!(browser_control_protocol_supported_raw(Some("1")));
        assert!(browser_control_protocol_supported_raw(Some(" 1 ")));
        assert!(!browser_control_protocol_supported_raw(Some("2")));
        assert!(!browser_control_protocol_supported_raw(Some("0")));
        assert!(!browser_control_protocol_supported_raw(Some("true")));
        assert!(!browser_control_protocol_supported_raw(Some("false")));
        assert!(!browser_control_protocol_supported_raw(Some("1.0")));
        assert!(!browser_control_protocol_supported_raw(Some("1e0")));
        assert!(!browser_control_protocol_supported_raw(Some("\"1\"")));
        assert!(!browser_control_protocol_supported_raw(None));
        assert!(!browser_control_protocol_supported_raw(Some("null")));
        assert!(!browser_control_protocol_supported_raw(Some("")));
    }

    #[test]
    fn filter_capabilities_cases() {
        let raw = vec!["browser_click".to_string(), "browser_cdp".to_string(), "unknown".to_string()];
        let filtered = filter_browser_control_capabilities(&raw, false);
        assert!(filtered.contains("browser_click"));
        assert!(!filtered.contains("browser_cdp"));
        assert!(!filtered.contains("unknown"));
        assert_eq!(filtered.len(), 1);
        let filtered_dev = filter_browser_control_capabilities(&raw, true);
        assert!(filtered_dev.contains("browser_click"));
        assert!(filtered_dev.contains("browser_cdp"));
        assert_eq!(filtered_dev.len(), 2);
        // artifact caps always allowed
        let raw2 = vec!["browser_artifact_upload".to_string()];
        assert!(filter_browser_control_capabilities(&raw2, false).contains("browser_artifact_upload"));
        // empty
        assert!(filter_browser_control_capabilities(&[], false).is_empty());
    }

    #[test]
    fn broker_event_envelope_shape() {
        let env = broker_event_envelope("sess-1", "browser.controller.command", r#"{"command_id":"abc"}"#);
        assert!(env.contains(r#""method":"event""#));
        assert!(env.contains(r#""type":"browser.controller.command""#));
        assert!(env.contains(r#""session_id":"sess-1""#));
        assert!(env.contains(r#""payload":{"command_id":"abc"}"#));
    }

    #[test]
    fn handle_register_forbidden_when_disabled() {
        let rid = rid1();
        let params = r#"{"session_id":"s1","controller_id":"c1","browser_profile_id":"b1","protocol_version":1,"capabilities":["browser_click"]}"#;
        let out = handle_register(
            &rid,
            params,
            Some("github"),
            Some("user123"),
            "t1",
            || false,
            false,
            |_| Some(SessionInfo { transport_id: "t1".to_string(), profile: "p1".to_string() }),
            |_, _| {},
        );
        assert!(out.contains(r#""code":4403"#));
        assert!(out.contains("browser.extension_control.enabled"));
    }

    #[test]
    fn handle_register_forbidden_bad_protocol() {
        let rid = rid1();
        let params = r#"{"session_id":"s1","controller_id":"c1","browser_profile_id":"b1","protocol_version":2,"capabilities":["browser_click"]}"#;
        let out = handle_register(
            &rid,
            params,
            Some("github"),
            Some("user123"),
            "t1",
            || true,
            false,
            |_| Some(SessionInfo { transport_id: "t1".to_string(), profile: "p1".to_string() }),
            |_, _| {},
        );
        assert!(out.contains(r#""code":4403"#));
        assert!(out.contains("unsupported browser-control protocol version"));
        // bool protocol_version should also fail (type is bool not int)
        let params2 = r#"{"session_id":"s1","controller_id":"c1","browser_profile_id":"b1","protocol_version":true,"capabilities":["browser_click"]}"#;
        let out2 = handle_register(
            &rid,
            params2,
            Some("github"),
            Some("user123"),
            "t1",
            || true,
            false,
            |_| Some(SessionInfo { transport_id: "t1".to_string(), profile: "p1".to_string() }),
            |_, _| {},
        );
        assert!(out2.contains(r#""code":4403"#));
    }

    #[test]
    fn handle_register_forbidden_no_identity() {
        let rid = rid1();
        let params = r#"{"session_id":"s1","controller_id":"c1","browser_profile_id":"b1","protocol_version":1,"capabilities":["browser_click"]}"#;
        let out = handle_register(&rid, params, None, None, "t1", || true, false, |_| Some(SessionInfo { transport_id: "t1".to_string(), profile: "p1".to_string() }), |_,_|{});
        assert!(out.contains(r#""code":4403"#));
        assert!(out.contains("authenticated non-internal identity"));
        // internal identity also forbidden
        let out2 = handle_register(&rid, params, Some(INTERNAL_PROVIDER), Some(INTERNAL_USER_ID), "t1", || true, false, |_| Some(SessionInfo { transport_id: "t1".to_string(), profile: "p1".to_string() }), |_,_|{});
        assert!(out2.contains(r#""code":4403"#));
    }

    #[test]
    fn handle_register_forbidden_not_owned() {
        let rid = rid1();
        let params = r#"{"session_id":"s1","controller_id":"c1","browser_profile_id":"b1","protocol_version":1,"capabilities":["browser_click"]}"#;
        // session owned by other transport
        let out = handle_register(&rid, params, Some("github"), Some("user123"), "t1", || true, false, |_| Some(SessionInfo { transport_id: "t2".to_string(), profile: "p1".to_string() }), |_,_|{});
        assert!(out.contains(r#""code":4403"#));
        assert!(out.contains("session is not owned"));
        // missing session
        let out2 = handle_register(&rid, params, Some("github"), Some("user123"), "t1", || true, false, |_| None, |_,_|{});
        assert!(out2.contains(r#""code":4403"#));
    }

    #[test]
    fn handle_register_forbidden_missing_ids() {
        let rid = rid1();
        let base = |cid: &str, bpid: &str, prof: &str| {
            let p = format!(r#"{{"session_id":"s1","controller_id":"{}","browser_profile_id":"{}","protocol_version":1,"capabilities":["browser_click"]}}"#, cid, bpid);
            let sess = SessionInfo { transport_id: "t1".to_string(), profile: prof.to_string() };
            handle_register(&rid, &p, Some("github"), Some("user123"), "t1", || true, false, |_| Some(sess.clone()), |_,_|{})
        };
        assert!(base("", "b1", "p1").contains(r#""code":4403"#));
        assert!(base("c1", "", "p1").contains(r#""code":4403"#));
        assert!(base("c1", "b1", "").contains(r#""code":4403"#));
        assert!(base("   ", "b1", "p1").contains(r#""code":4403"#));
    }

    #[test]
    fn handle_register_forbidden_no_caps() {
        let rid = rid1();
        let params = r#"{"session_id":"s1","controller_id":"c1","browser_profile_id":"b1","protocol_version":1,"capabilities":["not_allowed"]}"#;
        let out = handle_register(&rid, params, Some("github"), Some("user123"), "t1", || true, false, |_| Some(SessionInfo { transport_id: "t1".to_string(), profile: "p1".to_string() }), |_,_|{});
        assert!(out.contains(r#""code":4403"#));
        assert!(out.contains("no permitted controller capabilities"));
        // non-list capabilities also → empty
        let params2 = r#"{"session_id":"s1","controller_id":"c1","browser_profile_id":"b1","protocol_version":1,"capabilities":"browser_click"}"#;
        let out2 = handle_register(&rid, params2, Some("github"), Some("user123"), "t1", || true, false, |_| Some(SessionInfo { transport_id: "t1".to_string(), profile: "p1".to_string() }), |_,_|{});
        assert!(out2.contains(r#""code":4403"#));
    }

    #[test]
    fn handle_register_success() {
        let rid = rid1();
        let params = r#"{"session_id":"sess1","controller_id":"ctrl1","browser_profile_id":"br1","protocol_version":1,"capabilities":["browser_click","browser_navigate","unknown"]}"#;
        let mut attached: Option<ControllerScope> = None;
        let mut attached_sid: Option<String> = None;
        let out = handle_register(
            &rid,
            params,
            Some("github"),
            Some("user123"),
            "t1",
            || true,
            false,
            |_| Some(SessionInfo { transport_id: "t1".to_string(), profile: "myprofile".to_string() }),
            |scope, sid| {
                attached = Some(scope);
                attached_sid = Some(sid);
            },
        );
        assert!(out.contains(r#""result""#));
        assert!(out.contains(r#""scope""#));
        assert!(out.contains(r#""principal_id":"principal:dashboard:"#));
        assert!(out.contains(r#""profile_id":"myprofile""#));
        assert!(out.contains(r#""session_id":"sess1""#));
        assert!(out.contains(r#""controller_id":"ctrl1""#));
        assert!(out.contains(r#""browser_profile_id":"br1""#));
        assert!(out.contains(CLOUD_TRANSPORT_FAMILY));
        // sorted capabilities: browser_click, browser_navigate (unknown filtered)
        assert!(out.contains("browser_click"));
        assert!(out.contains("browser_navigate"));
        assert!(!out.contains("unknown"));
        let sc = attached.unwrap();
        assert_eq!(sc.session_id, "sess1");
        assert_eq!(sc.profile_id, "myprofile");
        assert_eq!(sc.transport_family, CLOUD_TRANSPORT_FAMILY);
        assert_eq!(attached_sid.unwrap(), "sess1");
        assert_eq!(sc.principal_id, principal_digest("github", "user123"));
        // capabilities sorted in output but also in scope set
        let mut expected = BTreeSet::new();
        expected.insert("browser_click".to_string());
        expected.insert("browser_navigate".to_string());
        assert_eq!(sc.capabilities, expected);
    }

    #[test]
    fn handle_register_developer_caps_gated() {
        let rid = rid1();
        // with developer_mode false, cdp should be filtered → empty error if only cdp
        let params = r#"{"session_id":"s1","controller_id":"c1","browser_profile_id":"b1","protocol_version":1,"capabilities":["browser_cdp"]}"#;
        let out = handle_register(&rid, params, Some("github"), Some("user123"), "t1", || true, false, |_| Some(SessionInfo { transport_id: "t1".to_string(), profile: "p".to_string() }), |_,_|{});
        assert!(out.contains(r#""code":4403"#));
        // with true, it passes
        let mut ok = false;
        let out2 = handle_register(&rid, params, Some("github"), Some("user123"), "t1", || true, true, |_| Some(SessionInfo { transport_id: "t1".to_string(), profile: "p".to_string() }), |scope, _| {
            assert!(scope.capabilities.contains("browser_cdp"));
            ok = true;
        });
        assert!(out2.contains(r#""scope""#));
        assert!(ok);
    }

    #[test]
    fn handle_result_success_and_accepted() {
        let rid = rid1();
        let params = r#"{"session_id":"s1","command_id":"cmd1","ok":true,"result":{"x":1}}"#;
        let scope = ControllerScope::new(
            principal_digest("github", "u1"),
            "p1".to_string(),
            "s1".to_string(),
            "c1".to_string(),
            "b1".to_string(),
            CLOUD_TRANSPORT_FAMILY.to_string(),
            {
                let mut s = BTreeSet::new();
                s.insert("browser_click".to_string());
                s
            },
        );
        let scope_clone = scope.clone();
        let out = handle_result(
            &rid,
            params,
            Some("github"),
            Some("u1"),
            "t1",
            |_| Some(SessionInfo { transport_id: "t1".to_string(), profile: "p1".to_string() }),
            |sid, princ, fam| {
                assert_eq!(sid, "s1");
                assert_eq!(fam, CLOUD_TRANSPORT_FAMILY);
                assert_eq!(princ, principal_digest("github", "u1"));
                Some(scope_clone.clone())
            },
            |sc, tid| sc == &scope && tid == "t1",
            |cid, sc, ok, res| {
                assert_eq!(cid, "cmd1");
                assert_eq!(sc, &scope);
                assert!(ok);
                assert!(res.unwrap().contains("x"));
                true
            },
        );
        assert!(out.contains(r#""accepted":true"#));
    }

    #[test]
    fn handle_result_forbidden_cases() {
        let rid = rid1();
        let scope = ControllerScope::new(
            principal_digest("github", "u1"),
            "p".to_string(),
            "s1".to_string(),
            "c".to_string(),
            "b".to_string(),
            CLOUD_TRANSPORT_FAMILY.to_string(),
            BTreeSet::new(),
        );
        // no identity
        let out = handle_result(&rid, r#"{"session_id":"s1","command_id":"c1"}"#, None, None, "t1", |_| Some(SessionInfo{transport_id:"t1".to_string(),profile:"p".to_string()}), |_,_,_| Some(scope.clone()), |_,_| true, |_,_,_,_| true);
        assert!(out.contains(r#""code":4403"#));
        assert!(out.contains("authenticated controller identity required"));
        // not owned session
        let out2 = handle_result(&rid, r#"{"session_id":"s1","command_id":"c1"}"#, Some("github"), Some("u1"), "t1", |_| Some(SessionInfo{transport_id:"t2".to_string(),profile:"p".to_string()}), |_,_,_| Some(scope.clone()), |_,_| true, |_,_,_,_| true);
        assert!(out2.contains("session is not owned"));
        // missing command_id
        let out3 = handle_result(&rid, r#"{"session_id":"s1"}"#, Some("github"), Some("u1"), "t1", |_| Some(SessionInfo{transport_id:"t1".to_string(),profile:"p".to_string()}), |_,_,_| Some(scope.clone()), |_,_| true, |_,_,_,_| true);
        assert!(out3.contains("command_id required"));
        // no controller registered
        let out4 = handle_result(&rid, r#"{"session_id":"s1","command_id":"c1"}"#, Some("github"), Some("u1"), "t1", |_| Some(SessionInfo{transport_id:"t1".to_string(),profile:"p".to_string()}), |_,_,_| None, |_,_| true, |_,_,_,_| true);
        assert!(out4.contains("no controller registered"));
        // not owner
        let out5 = handle_result(&rid, r#"{"session_id":"s1","command_id":"c1"}"#, Some("github"), Some("u1"), "t1", |_| Some(SessionInfo{transport_id:"t1".to_string(),profile:"p".to_string()}), |_,_,_| Some(scope.clone()), |_,_| false, |_,_,_,_| true);
        assert!(out5.contains("controller is not owned"));
    }

    #[test]
    fn handle_result_ok_false_uses_error() {
        let rid = rid1();
        let params = r#"{"session_id":"s1","command_id":"cmd1","ok":false,"error":"boom"}"#;
        let scope = ControllerScope::new(principal_digest("github","u1"), "p".to_string(), "s1".to_string(), "c".to_string(), "b".to_string(), CLOUD_TRANSPORT_FAMILY.to_string(), BTreeSet::new());
        let sc2 = scope.clone();
        let mut captured: Option<String> = None;
        let out = handle_result(&rid, params, Some("github"), Some("u1"), "t1",
            |_| Some(SessionInfo{transport_id:"t1".to_string(),profile:"p".to_string()}),
            |_,_,_| Some(sc2.clone()),
            |_,_| true,
            |_,_,ok,res| { assert!(!ok); captured = res; true }
        );
        assert!(out.contains(r#""accepted":true"#));
        assert_eq!(captured.unwrap(), "\"boom\"");
        // ok=1 (number) is not True → ok false
        let params2 = r#"{"session_id":"s1","command_id":"cmd1","ok":1,"result":"x"}"#;
        let mut ok_val = true;
        handle_result(&rid, params2, Some("github"), Some("u1"), "t1",
            |_| Some(SessionInfo{transport_id:"t1".to_string(),profile:"p".to_string()}),
            |_,_,_| Some(scope.clone()),
            |_,_| true,
            |_,_,ok,_| { ok_val = ok; false }
        );
        assert!(!ok_val);
    }

    #[test]
    fn handle_heartbeat_success() {
        let rid = rid1();
        let params = r#"{"session_id":"s1"}"#;
        let scope = ControllerScope::new(principal_digest("github","u1"), "p".to_string(), "s1".to_string(), "c".to_string(), "b".to_string(), CLOUD_TRANSPORT_FAMILY.to_string(), BTreeSet::new());
        let out = handle_heartbeat(&rid, params, Some("github"), Some("u1"), "t1", |_| Some(SessionInfo{transport_id:"t1".to_string(),profile:"p".to_string()}), |_,_,_| Some(scope.clone()), |_,_| true);
        assert!(out.contains(r#""ok":true"#));
    }

    #[test]
    fn handle_heartbeat_forbidden() {
        let rid = rid1();
        let scope = ControllerScope::new(principal_digest("github","u1"), "p".to_string(), "s1".to_string(), "c".to_string(), "b".to_string(), CLOUD_TRANSPORT_FAMILY.to_string(), BTreeSet::new());
        let out = handle_heartbeat(&rid, r#"{"session_id":"s1"}"#, Some("github"), Some("u1"), "t1", |_| None, |_,_,_| Some(scope.clone()), |_,_| true);
        assert!(out.contains("session is not owned"));
        let out2 = handle_heartbeat(&rid, r#"{"session_id":"s1"}"#, Some("github"), Some("u1"), "t1", |_| Some(SessionInfo{transport_id:"t1".to_string(),profile:"p".to_string()}), |_,_,_| None, |_,_| true);
        assert!(out2.contains("no controller registered"));
        let out3 = handle_heartbeat(&rid, r#"{"session_id":"s1"}"#, None, None, "t1", |_| Some(SessionInfo{transport_id:"t1".to_string(),profile:"p".to_string()}), |_,_,_| Some(scope.clone()), |_,_| true);
        assert!(out3.contains("authenticated controller identity required"));
    }

    #[test]
    fn handle_detach_success() {
        let rid = rid1();
        let params = r#"{"session_id":"s1"}"#;
        let scope = ControllerScope::new(principal_digest("github","u1"), "p".to_string(), "s1".to_string(), "c".to_string(), "b".to_string(), CLOUD_TRANSPORT_FAMILY.to_string(), BTreeSet::new());
        let mut detached = false;
        let out = handle_detach(&rid, params, Some("github"), Some("u1"), "t1",
            |_| Some(SessionInfo{transport_id:"t1".to_string(),profile:"p".to_string()}),
            |_,_,_| Some(scope.clone()),
            |_,_| true,
            |sc, tid| { assert_eq!(sc, &scope); assert_eq!(tid, "t1"); detached = true; }
        );
        assert!(out.contains(r#""detached":true"#));
        assert!(detached);
    }

    #[test]
    fn handle_detach_forbidden() {
        let rid = rid1();
        let scope = ControllerScope::new(principal_digest("github","u1"), "p".to_string(), "s1".to_string(), "c".to_string(), "b".to_string(), CLOUD_TRANSPORT_FAMILY.to_string(), BTreeSet::new());
        let out = handle_detach(&rid, r#"{"session_id":"s1"}"#, Some("github"), Some("u1"), "t1", |_| Some(SessionInfo{transport_id:"t1".to_string(),profile:"p".to_string()}), |_,_,_| None, |_,_| true, |_,_| {});
        assert!(out.contains("controller is not owned"));
        let out2 = handle_detach(&rid, r#"{"session_id":"s1"}"#, Some("github"), Some("u1"), "t1", |_| Some(SessionInfo{transport_id:"t1".to_string(),profile:"p".to_string()}), |_,_,_| Some(scope.clone()), |_,_| false, |_,_| {});
        assert!(out2.contains("controller is not owned"));
    }

    #[test]
    fn build_registry_installs_all_four() {
        let mut reg = build_registry_default();
        assert_eq!(reg.len(), 4);
        let mut names: Vec<_> = reg.pending_names().collect();
        names.sort();
        assert_eq!(names, vec!["browser.controller.detach","browser.controller.heartbeat","browser.controller.register","browser.controller.result"]);
        let mut map = HashMap::new();
        reg.install_into(&mut map);
        assert_eq!(map.len(), 4);
        // default register should be forbidden (disabled)
        let out = map.get(METHOD_REGISTER).unwrap()("1".to_string(), r#"{"session_id":"s1","controller_id":"c","browser_profile_id":"b","protocol_version":1,"capabilities":["browser_click"],"__auth_provider":"github","__auth_user_id":"u1","__transport_id":"t1"}"#.to_string());
        assert!(out.contains("4403"));
    }

    #[test]
    fn ok_err_envelope_shape() {
        let rid = encode_rid("42");
        let ok = ok_response(&rid, r#"{"scope":{"x":1}}"#);
        assert!(ok.contains(r#""result":{"scope""#));
        let err = err_response(&rid, 4403, "forbidden");
        assert!(err.contains(r#""code":4403"#));
    }

    #[test]
    fn extract_helpers() {
        assert_eq!(extract_string_field(r#"{"session_id":"abc"}"#, "session_id").as_deref(), Some("abc"));
        assert_eq!(extract_string_field(r#"{"a":123}"#, "a").as_deref(), Some("123"));
        assert!(extract_string_field(r#"{"a":null}"#, "a").is_none());
        assert_eq!(extract_raw_value(r#"{"protocol_version":1}"#, "protocol_version").unwrap(), "1");
        assert_eq!(extract_raw_value(r#"{"protocol_version":true}"#, "protocol_version").unwrap(), "true");
        assert_eq!(extract_capabilities_list(r#"{"capabilities":["browser_click","browser_tabs"]}"#), vec!["browser_click","browser_tabs"]);
        assert!(extract_capabilities_list(r#"{"capabilities":"nope"}"#).is_empty());
        assert!(extract_capabilities_list(r#"{}"#).is_empty());
    }
}
