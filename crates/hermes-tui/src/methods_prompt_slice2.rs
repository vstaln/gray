//! Prompt / attachment / respond JSON-RPC handlers — slice 2 (lines 900-1626).
//!
//! 1:1 port of `tui_gateway/methods_prompt.py` lines 900–1626 (T0385 slice 2/1626).
//!
//! Handler bodies are byte-identical to their pre-split `server.py` form; they
//! are rebound onto `server.py`'s globals at install time — see `method_ctx.py`.
//!
//! ```python
//! # Python — tui_gateway/methods_prompt.py 900-1626 (abridged, comments preserved)
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
//! # ── image.attach tail (878-920, suffix from 900) ─────────────────────────
//! # Inside @method("image.attach") def _(rid, params: dict) -> dict:
//! #     # ... slice 1 through image_path = _resolve_attachment_path(path_token) at 900
//! #     if image_path.suffix.lower() not in _IMAGE_EXTENSIONS:
//! #         return _err(rid, 4016, f"unsupported image: {image_path.name}")
//! #     session.setdefault("attached_images", []).append(str(image_path))
//! #     return _ok(rid, {"attached": True, "path": str(image_path), "count": len(session["attached_images"]), "remainder": remainder, "text": remainder or f"[User attached image: {image_path.name}]", **_image_meta(image_path)})
//!
//! @method("image.attach_bytes")
//! def _(rid, params: dict) -> dict:
//!     session, err = _sess_building(params, rid)
//!     if err: return err
//!     raw_b64 = str(params.get("content_base64") or params.get("data") or "").strip()
//!     if not raw_b64: return _err(rid, 4015, "content_base64 required")
//!     img_bytes = _decode_attach_base64(raw_b64, mime_prefix="image/")
//!     if img_bytes is None: return _err(rid, 4017, "data is not valid base64")
//!     if not img_bytes: return _err(rid, 4017, "image is empty")
//!     if len(img_bytes) > _ATTACH_BYTES_MAX_BYTES: return _err(rid, 4018, f"image too large ({len(img_bytes)} bytes; cap is {mb} MB)")
//!     filename = str(params.get("filename", "") or ""); ext_hint = str(params.get("ext", "") or "").strip().lower()
//!     if ext_hint and not ext_hint.startswith("."): ext_hint = "." + ext_hint
//!     ext = _sniff_image_ext(img_bytes, filename or (f"x{ext_hint}" if ext_hint else ""))
//!     if ext not in _allowed_image_extensions(): return _err(rid, 4016, f"unsupported image extension: {ext}")
//!     try: img_path = _queue_attached_image(session, img_bytes, ext, prefix="upload")
//!     except Exception as e: return _err(rid, 5027, f"write failed: {e}")
//!     return _ok(rid, {"attached": True, "path": str(img_path), "count": len(session["attached_images"]), "remainder": "", "text": f"[User attached image: {img_path.name}]", "bytes": len(img_bytes), **_image_meta(img_path)})
//!
//! @method("pdf.attach")
//! def _(rid, params: dict) -> dict:
//!     import shutil, subprocess, tempfile
//!     session, err = _sess_building(params, rid)
//!     if err: return err
//!     if shutil.which("pdftoppm") is None: return _err(rid, 5028, "pdftoppm not installed (poppler-utils package required)")
//!     raw_path = str(params.get("path", "") or "").strip(); raw_b64 = str(params.get("content_base64") or params.get("data") or "").strip()
//!     if not raw_path and not raw_b64: return _err(rid, 4015, "path or content_base64 required")
//!     with tempfile.TemporaryDirectory(prefix="pdf_attach_") as td:
//!         # ... decode / resolve / first_page/last_page 1-indexed, default last=first+MAX-1, 4015/4019 caps, pdftoppm -png -r 150 -f/-l with windows_hide_flags, capture_output text True errors replace timeout 120, rendered sorted glob page-*.png, _queue_attached_image per page prefix pdf_p{num} + _image_meta, return pages_attached ...
//!     # error branches: 4017 base64, 4017 empty, 4018 too large, 4017 missing %PDF- magic, 4016 not found / not .pdf, 4015 first_page/last_page must be integers / >=1 / last>=first, 4019 range exceeds 25, 5028 pdftoppm missing/timeout/failed/no pages
//!
//! @method("file.attach")
//! def _(rid, params: dict) -> dict:
//!     session, err = _sess_building(params, rid)
//!     if err: return err
//!     raw = str(params.get("path", "") or "").strip(); data_url = str(params.get("data_url", "") or "").strip(); name = str(params.get("name", "") or "").strip()
//!     if not raw and not data_url: return _err(rid, 4015, "path or data_url required")
//!     try: stored_path, uploaded = _stage_session_file_attachment(session, raw_path=raw, data_url=data_url, name=name); ref_path = _attachment_ref_path(session, stored_path); return _ok(rid, {"attached": True, "name": stored_path.name, "path": str(stored_path), "ref_path": ref_path, "ref_text": f"@file:{_format_ref_value(ref_path)}", "uploaded": uploaded})
//!     except Exception as e: return _err(rid, 5028, str(e))
//!
//! @method("image.detach")
//! def _(rid, params: dict) -> dict:
//!     session, err = _sess_building(params, rid)
//!     if err: return err
//!     raw = str(params.get("path", "") or "").strip()
//!     if not raw: return _err(rid, 4015, "path required")
//!     images = session.setdefault("attached_images", []); before = len(images); session["attached_images"] = [path for path in images if path != raw]; return _ok(rid, {"detached": len(session["attached_images"]) != before, "count": len(session["attached_images"])})
//!
//! @method("input.detect_drop")
//! def _(rid, params: dict) -> dict:
//!     session, err = _sess_nowait(params, rid)
//!     if err: return err
//!     try: from cli import _detect_file_drop; raw = str(params.get("text", "") or ""); dropped = _detect_file_drop(raw); if not dropped: return _ok(rid, {"matched": False}); drop_path = dropped["path"]; remainder = dropped["remainder"]; if dropped["is_image"]: session.setdefault("attached_images", []).append(str(drop_path)); return _ok(rid, {"matched": True, "is_image": True, "path": str(drop_path), "count": len(session["attached_images"]), "text": remainder or f"[User attached image: {drop_path.name}]", **_image_meta(drop_path)}); return _ok(rid, {"matched": True, "is_image": False, "path": str(drop_path), "name": drop_path.name, "text": f"[User attached file: {drop_path}]" + (f"\n{remainder}" if remainder else "")})
//!     except Exception as e: return _err(rid, 5027, str(e))
//!
//! @method("prompt.background")
//! def _(rid, params: dict) -> dict:
//!     session, err = _sess(params, rid)
//!     if err: return err
//!     text, parent = params.get("text", ""), params.get("session_id", "")
//!     if not text: return _err(rid, 4012, "text required")
//!     task_id = f"bg_{uuid.uuid4().hex[:6]}"
//!     def run(): session_tokens = _set_session_context(task_id, cwd=_session_cwd(session)); try: from run_agent import AIAgent; _profile_home_str = session.get("profile_home"); home_token = set_hermes_home_override(_profile_home_str) if _profile_home_str else None; try: result = AIAgent(**_background_agent_kwargs(session["agent"], task_id)).run_conversation(user_message=text, task_id=task_id)
//!                finally: if home_token is not None: reset_hermes_home_override(home_token); _emit("background.complete", parent, {"task_id": task_id, "text": result.get("final_response", str(result)) if isinstance(result, dict) else str(result)})
//!              except Exception as e: _emit("background.complete", parent, {"task_id": task_id, "text": f"error: {e}"})
//!              finally: _clear_session_context(session_tokens)
//!     threading.Thread(target=run, daemon=True).start(); return _ok(rid, {"task_id": task_id})
//!
//! @method("preview.restart")
//! def _(rid, params: dict) -> dict:
//!     session, err = _sess(params, rid)
//!     if err: return err
//!     url = str(params.get("url") or "").strip(); cwd = str(params.get("cwd") or "").strip(); context = str(params.get("context") or "").strip()
//!     if not url: return _err(rid, 4012, "url required")
//!     task_id = f"preview_{uuid.uuid4().hex[:6]}"; parent = params.get("session_id", ""); parent_history = _preview_restart_history(session)
//!     # ... prompt join lines: Preview URL, CWD hint, console, history-aware recovery, port owner, MIME module-script fix, detached server, etc. preview_cwd = abspath(expanduser(cwd)) if cwd else "" with isdir guard
//!     # def run(): _set_session_context(task_id, cwd=(preview_cwd or _session_cwd(session))); register_task_env_overrides if preview_cwd; _emit preview.restart.progress; AIAgent(**_ephemeral_preview_agent_kwargs(...), **_preview_restart_callbacks(parent, task_id)).run_conversation(user_message=prompt, task_id=task_id, conversation_history=parent_history or None) with home_token; _emit preview.restart.complete; clear_task_env_overrides
//!     threading.Thread(target=run, daemon=True).start(); return _ok(rid, {"task_id": task_id})
//!
//! @method("clarify.respond")
//! def _(rid, params: dict) -> dict: return _respond(rid, params, "answer", allow_expired=True)
//! @method("terminal.read.respond")
//! def _(rid, params: dict) -> dict: return _respond(rid, params, "text", allow_expired=True)
//! @method("preview.read.respond")
//! def _(rid, params: dict) -> dict: return _respond(rid, params, "text", allow_expired=True)
//! @method("preview.act.respond")
//! def _(rid, params: dict) -> dict: return _respond(rid, params, "text", allow_expired=True)
//! @method("window.read.respond")
//! def _(rid, params: dict) -> dict: return _respond(rid, params, "text", allow_expired=True)
//! @method("tour.respond")
//! def _(rid, params: dict) -> dict: return _respond(rid, params, "text", allow_expired=True)
//! @method("mcp.setup.respond")
//! def _(rid, params: dict) -> dict: return _respond(rid, params, "result", allow_expired=True)
//! @method("sudo.respond")
//! def _(rid, params: dict) -> dict: return _respond(rid, params, "password", allow_expired=True)
//! @method("secret.respond")
//! def _(rid, params: dict) -> dict: return _respond(rid, params, "value", allow_expired=True)
//!
//! @method("approval.pending")
//! def _(rid, params: dict) -> dict:
//!     session, err = _sess(params, rid)
//!     if err: return err
//!     try: from tools.approval import list_gateway_approvals; return _ok(rid, {"approvals": list_gateway_approvals(session["session_key"])})
//!     except Exception as e: return _err(rid, 5004, str(e))
//!
//! @method("approval.received")
//! def _(rid, params: dict) -> dict:
//!     session, err = _sess(params, rid)
//!     if err: return err
//!     request_id = params.get("request_id")
//!     if not isinstance(request_id, str) or not request_id: return _err(rid, 4006, "request_id required")
//!     try: from tools.approval import ack_gateway_approval; return _ok(rid, {"acknowledged": ack_gateway_approval(session["session_key"], request_id)})
//!     except Exception as e: return _err(rid, 5004, str(e))
//!
//! def _approval_respond_session_fallback(params: dict):
//!     request_id = str(params.get("request_id") or "")
//!     if request_id:
//!         try: from tools.approval import list_gateway_approvals; with _sessions_lock: live = list(_sessions.items())
//!              for sid, session in live: key = str(session.get("session_key") or ""); if not key: continue; for pending in list_gateway_approvals(key): if str(pending.get("request_id") or "") == request_id: return session
//!         except Exception: logger.debug("approval.respond request_id fallback failed", exc_info=True)
//!     target = str(params.get("session_id") or "")
//!     if target:
//!         try: live = _find_live_session_by_key(target)
//!              if live is not None: return live[1]
//!         except Exception: logger.debug("approval.respond stored-id fallback failed", exc_info=True)
//!     return None
//!
//! @method("approval.respond")
//! def _(rid, params: dict) -> dict:
//!     session, err = _sess(params, rid)
//!     if err:
//!         code = (err.get("error") or {}).get("code")
//!         if code != 4001: return err
//!         session = _approval_respond_session_fallback(params)
//!         if session is None: return err
//!     try: from tools.approval import resolve_gateway_approval; return _ok(rid, {"resolved": resolve_gateway_approval(session["session_key"], params.get("choice", "deny"), resolve_all=params.get("all", False), request_id=params.get("request_id"))})
//!     except Exception as e: return _err(rid, 5004, str(e))
//!
//! def register(server) -> None:
//!     _registry.install(server)
//!     g = vars(server)
//!     for helper in (_history_user_indices, _message_row_id, _mem_db_pair_agrees, _find_user_turn_by_row_id, _load_durable_truncation_history, _resolve_truncate_row_id, _coerce_truncate_int, _reconcile_client_ordinal, _pending_reaction_notes, _approval_respond_session_fallback):
//!         setattr(server, helper.__name__, types.FunctionType(helper.__code__, g, helper.__name__, helper.__defaults__, helper.__closure__))
//! ```
//!
//! # Rust mapping
//! * `HandlerRegistry` → [`crate::method_ctx::HandlerRegistry`] (same deferred
//!   `@method` + `install` shape; see `method_ctx.rs` for `FunctionType`
//!   rebinding no-op notes).
//! * `image.attach` tail (900-914) — `image_path.suffix.lower() not in _IMAGE_EXTENSIONS`
//!   `4016` + `attached_images` + `remainder`/`text` + `_image_meta` →
//!   documented as tail completion of the `image.attach` method registered in
//!   slice 1 (decorator lives at 878). Slice 2 only contains its Continuation
//!   lines 900-920; routing + `4015 path required` stay in slice 1 and are not
//!   re-registered here. Tests assert `is_supported_image_ext` / `format_image_attach_text`.
//! * `image.attach_bytes` — `content_base64`/`data` required `4015`, `_decode_attach_base64`
//!   `4017` (invalid/empty), `> _ATTACH_BYTES_MAX_BYTES` `4018` (`25 MB`),
//!   `filename`/`ext` hint + `_sniff_image_ext` + `_allowed_image_extensions` `4016`,
//!   `_queue_attached_image` `5027`, `attached_images` + `bytes` + `_image_meta` →
//!   [`handle_image_attach_bytes`] (validates `4015` handler-side for missing b64,
//!   delegates `4017`/`4018`/`4016`/`5027` to injected `Fn(&str)->Result<String,(i32,String)>`
//!   that owns `profile_home`/`images/` + `image_counter` + `mime_prefix image/` branching).
//! * `pdf.attach` — `path`/`content_base64` required `4015`, `which pdftoppm` `5028`,
//!   `data` base64 `4017`, `PDF too large` `4018` (`50 MB`), `missing %PDF-` `4017`,
//!   `.pdf` suffix + `PDF not found` `4016`, `first_page`/`last_page` ints `4015` +
//!   `>=1`/`last>=first`/`range>25` `4019`, `pdftoppm -png -r 150 -f/-l` with `windows_hide_flags`
//!   `capture_output` `errors replace` `timeout 120` → `5028` (missing/timeout/failed/no pages),
//!   `page-*.png` + `_queue_attached_image` prefix `pdf_p{num}` + `_image_meta` per page →
//!   [`handle_pdf_attach`] (validates `4015` missing `path`+`b64` handler-side, delegates
//!   `5028`/`4017`/`4018`/`4019`/`4016` to the injected closure that owns `Path` + `subprocess`).
//! * `file.attach` — `path`/`data_url` required `4015`, `_stage_session_file_attachment`
//!   + `_attachment_ref_path` + `_format_ref_value` `@file:` → [`handle_file_attach`]
//!   (validates `4015` handler-side, delegates `5028`).
//! * `image.detach` — `path` required `4015`, `attached_images` filter + `detached` bool + `count` →
//!   [`handle_image_detach`] (validates `4015` handler-side, delegates `5007` etc.).
//! * `input.detect_drop` — `_sess_nowait` + `_detect_file_drop(raw)` → `matched/is_image/path/count/text`
//!   + `_image_meta` vs file branch, `5027` on exception → [`handle_input_detect_drop`]
//!   (delegates; `_sess` + `detect` + extension `4016` owned by closure).
//! * `prompt.background` — `text` required `4012`, `bg_{uuid}` task_id, `_set_session_context` +
//!   `HERMES_HOME` override + `AIAgent(**_background_agent_kwargs)` + `background.complete` emit →
//!   [`handle_prompt_background`] (validates `4012` handler-side, delegates `5007`/threading).
//! * `preview.restart` — `url` required `4012`, `preview_{uuid}` task_id, history-aware prompt
//!   + `preview_cwd` `abspath(expanduser)` + `isdir` guard + `register_task_env_overrides` +
//!   `preview.restart.progress` + `AIAgent(**_ephemeral_preview_agent_kwargs, **_preview_restart_callbacks)`
//!   → [`handle_preview_restart`] (validates `4012` handler-side for `url`, delegates the rest).
//! * `clarify.respond` / `terminal.read.respond` / `preview.read.respond` / `preview.act.respond` /
//!   `window.read.respond` / `tour.respond` / `mcp.setup.respond` / `sudo.respond` / `secret.respond`
//!   — all `return _respond(rid, params, key, allow_expired=True)` where `key` is `answer`/`text`/`result`/
//!   `password`/`value`, `expired` → `{"status":"expired"}` vs `4009`/`4002` →
//!   [`handle_respond_generic`] family + thin wrappers
//!   [`handle_clarify_respond`] etc. (handler-side no validation; closure owns
//!   `_pending`/`_answers`/`_batch_clarify` map + `request_id`/`question_id` branching).
//! * `approval.pending` — `_sess` + `list_gateway_approvals` → `5004` → [`handle_approval_pending`].
//! * `approval.received` — `request_id` required `4006` + `ack_gateway_approval` `5004` →
//!   [`handle_approval_received`] (validates `4006` handler-side).
//! * `_approval_respond_session_fallback(params)` — `request_id` scan of `list(_sessions.items())`
//!   + `list_gateway_approvals(key)` match, plus `session_id` → `_find_live_session_by_key` tip →
//!   [`approval_fallback_by_request_id`] / [`approval_fallback_by_session_id`] (pure `std`-only
//!   helpers; the injected closure owns `_sessions_lock` + `list_gateway_approvals` wiring, tests
//!   exercise `resolve_approval_session_fallback`).
//! * `approval.respond` — `_sess` `4001` + durable fallback `4001` → `resolve_gateway_approval` `5004` →
//!   [`handle_approval_respond`] (delegates; fallback `4001` path tested via handler, closure owns `choice`/`all`/`request_id`).
//! * `is_truthy_value` → [`is_truthy_value`] (mirrors `hermes_constants` truthy).
//! * `_ok(rid, result)` / `_err(rid, code, msg)` → [`ok_response`] / [`err_response`] (mirrors `server.py::_ok` / `_err`).
//! * `@method("...")` + `register(server)` → [`register`] / [`register_with`] / [`build_registry`] / [`build_registry_default`] (deferred via `HandlerRegistry`).
//!

use std::collections::HashMap;

use crate::method_ctx::HandlerRegistry;

// ---------------------------------------------------------------------------
// Method names — mirrors @method("...") decorators for 900-1626
// ---------------------------------------------------------------------------

pub const METHOD_IMAGE_ATTACH_BYTES: &str = "image.attach_bytes";
pub const METHOD_PDF_ATTACH: &str = "pdf.attach";
pub const METHOD_FILE_ATTACH: &str = "file.attach";
pub const METHOD_IMAGE_DETACH: &str = "image.detach";
pub const METHOD_INPUT_DETECT_DROP: &str = "input.detect_drop";
pub const METHOD_PROMPT_BACKGROUND: &str = "prompt.background";
pub const METHOD_PREVIEW_RESTART: &str = "preview.restart";
pub const METHOD_CLARIFY_RESPOND: &str = "clarify.respond";
pub const METHOD_TERMINAL_READ_RESPOND: &str = "terminal.read.respond";
pub const METHOD_PREVIEW_READ_RESPOND: &str = "preview.read.respond";
pub const METHOD_PREVIEW_ACT_RESPOND: &str = "preview.act.respond";
pub const METHOD_WINDOW_READ_RESPOND: &str = "window.read.respond";
pub const METHOD_TOUR_RESPOND: &str = "tour.respond";
pub const METHOD_MCP_SETUP_RESPOND: &str = "mcp.setup.respond";
pub const METHOD_SUDO_RESPOND: &str = "sudo.respond";
pub const METHOD_SECRET_RESPOND: &str = "secret.respond";
pub const METHOD_APPROVAL_PENDING: &str = "approval.pending";
pub const METHOD_APPROVAL_RECEIVED: &str = "approval.received";
pub const METHOD_APPROVAL_RESPOND: &str = "approval.respond";

// image.attach itself lives in slice 1 (decorator at 878); its suffix 900-914 is
// documented here for 1:1 traceability but not re-registered.

// ---------------------------------------------------------------------------
// Error codes — mirrors _err(rid, N, ...)
// ---------------------------------------------------------------------------

pub const ERR_TEXT_REQUIRED: i32 = 4012;
pub const ERR_URL_REQUIRED: i32 = 4012;
pub const ERR_PATH_REQUIRED: i32 = 4015;
pub const ERR_CONTENT_REQUIRED: i32 = 4015;
pub const ERR_PATH_OR_CONTENT_REQUIRED: i32 = 4015;
pub const ERR_PATH_OR_DATA_URL_REQUIRED: i32 = 4015;
pub const ERR_IMAGE_NOT_FOUND: i32 = 4016;
pub const ERR_UNSUPPORTED_IMAGE: i32 = 4016;
pub const ERR_PDF_NOT_FOUND: i32 = 4016;
pub const ERR_NOT_PDF: i32 = 4016;
pub const ERR_BASE64_INVALID: i32 = 4017;
pub const ERR_IMAGE_EMPTY: i32 = 4017;
pub const ERR_IMAGE_TOO_LARGE: i32 = 4018;
pub const ERR_PDF_TOO_LARGE: i32 = 4018;
pub const ERR_PAGE_RANGE_EXCEEDS: i32 = 4019;
pub const ERR_REQUEST_ID_REQUIRED: i32 = 4006;
pub const ERR_NO_PENDING: i32 = 4009;
pub const ERR_UNKNOWN_QUESTION: i32 = 4002;
pub const ERR_APPROVAL: i32 = 5004;
pub const ERR_WRITE_FAILED: i32 = 5027;
pub const ERR_PDFTOPPM: i32 = 5028;
pub const ERR_FILE_ATTACH: i32 = 5028;
pub const ERR_DETECT_DROP: i32 = 5027;

// ---------------------------------------------------------------------------
// Limits — mirrors server.py constants
// ---------------------------------------------------------------------------

pub const ATTACH_BYTES_MAX_BYTES: usize = 25 * 1024 * 1024;
pub const PDF_ATTACH_MAX_BYTES: usize = 50 * 1024 * 1024;
pub const PDF_ATTACH_MAX_PAGES: i64 = 25;

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
// Image / PDF / attach helpers — mirrors server.py constants + cli helpers
// ---------------------------------------------------------------------------

/// Mirrors `cli._IMAGE_EXTENSIONS` fallback — `frozenset({".png",".jpg",".jpeg",".gif",".webp",".bmp"})`.
pub const DEFAULT_IMAGE_EXTENSIONS: &[&str] = &[".png", ".jpg", ".jpeg", ".gif", ".webp", ".bmp"];

/// Whether `ext` (with dot, case-insensitive) is an allowed image extension.
///
/// Mirrors `_allowed_image_extensions()` → `frozenset(_IMAGE_EXTENSIONS)`.
pub fn is_supported_image_ext(ext: &str) -> bool {
    let lower = ext.trim().to_ascii_lowercase();
    let lower = if lower.starts_with('.') { lower } else { format!(".{}", lower) };
    DEFAULT_IMAGE_EXTENSIONS.contains(&lower.as_str())
}

/// Sniff extension from filename hint or magic bytes — mirrors `_sniff_image_ext`.
///
/// For `std`-only we only mirror the filename-hint branch (magic-byte fallback
/// is owned by the injected closure that has the raw bytes). Returns `.png` default.
pub fn sniff_image_ext_from_filename(filename: &str) -> String {
    let t = filename.trim();
    if t.is_empty() {
        return ".png".to_string();
    }
    if let Some(dot) = t.rfind('.') {
        let ext = t[dot..].to_ascii_lowercase();
        if is_supported_image_ext(&ext) {
            return ext;
        }
        // Even unsupported ext from filename is returned verbatim in Python (then rejected via allowed set).
        // Mirror that: return the suffix if present.
        if !ext.is_empty() && ext.contains('.') {
            return ext;
        }
    }
    ".png".to_string()
}

/// Check whether `s` is plausibly valid base64 (std-only cheap check).
///
/// Mirrors `_decode_attach_base64(..., validate=True)` success → `Some`, else `None`.
/// Here we just check chars and padding; the real decode is owned by the closure.
pub fn is_valid_base64_chars(s: &str) -> bool {
    let t = s.trim();
    if t.is_empty() { return false; }
    // strip data URL prefix if present
    let b64 = if let Some(comma) = t.find(',') {
        if t[..comma].contains("base64") { &t[comma+1..] } else { t }
    } else { t };
    let b64: String = b64.chars().filter(|c| !c.is_whitespace()).collect();
    if b64.is_empty() { return false; }
    // length must be multiple of 4 after padding (allow missing padding via closure, but we check chars)
    for ch in b64.chars() {
        if !ch.is_ascii_alphanumeric() && ch != '+' && ch != '/' && ch != '=' {
            return false;
        }
    }
    true
}

/// Format `image.attach` text — mirrors `remainder or f"[User attached image: {name}]"`.
pub fn format_image_attach_text(remainder: &str, image_name: &str) -> String {
    let r = remainder.trim();
    if r.is_empty() {
        format!("[User attached image: {}]", image_name)
    } else {
        r.to_string()
    }
}

/// Format `file.attach` ref_text — mirrors `f"@file:{_format_ref_value(ref_path)}"`.
pub fn format_file_ref_text(ref_path: &str) -> String {
    format!("@file:{}", format_ref_value(ref_path))
}

/// Quote a context-ref value when it contains whitespace or bracket chars.
///
/// Mirrors `_format_ref_value` / desktop `formatRefValue`.
pub fn format_ref_value(value: &str) -> String {
    let needs = value.chars().any(|c| c.is_whitespace() || c == '[' || c == ']' || c == '"' || c == '\'');
    if needs {
        format!("\"{}\"", value.replace('"', "\\\""))
    } else {
        value.to_string()
    }
}

/// Validate `first_page`/`last_page` for `pdf.attach` — mirrors Python checks.
///
/// Returns `Ok((first,last))` where `last` is resolved (default `first+MAX-1`),
/// or `Err((code,msg))` with `4015`/`4019`.
pub fn validate_pdf_page_range(first_raw: Option<&str>, last_raw: Option<&str>) -> Result<(i64, i64), (i32, String)> {
    let first: i64 = match first_raw {
        None => 1,
        Some(s) => {
            let t = s.trim().trim_matches('"');
            if t.is_empty() || t == "null" { 1 } else { t.parse::<i64>().map_err(|_| (ERR_PATH_REQUIRED, "first_page/last_page must be integers".to_string()))? }
        }
    };
    let last: i64 = match last_raw {
        None => first + PDF_ATTACH_MAX_PAGES - 1,
        Some(s) => {
            let t = s.trim().trim_matches('"');
            if t.is_empty() || t == "null" { first + PDF_ATTACH_MAX_PAGES - 1 } else { t.parse::<i64>().map_err(|_| (ERR_PATH_REQUIRED, "first_page/last_page must be integers".to_string()))? }
        }
    };
    if first < 1 {
        return Err((ERR_PATH_REQUIRED, "first_page must be >= 1".to_string()));
    }
    if last < first {
        return Err((ERR_PATH_REQUIRED, "last_page must be >= first_page".to_string()));
    }
    if last - first + 1 > PDF_ATTACH_MAX_PAGES {
        return Err((ERR_PAGE_RANGE_EXCEEDS, format!("page range exceeds cap of {} pages per attach call", PDF_ATTACH_MAX_PAGES)));
    }
    Ok((first, last))
}

// ---------------------------------------------------------------------------
// Approval fallback helpers — mirrors _approval_respond_session_fallback
// ---------------------------------------------------------------------------

/// Extract `request_id` from params_json.
pub fn extract_request_id(params_json: &str) -> Option<String> {
    let s = extract_string_field(params_json, "request_id")?.trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

/// Extract `session_id` target for fallback.
pub fn extract_target_session_id(params_json: &str) -> Option<String> {
    let s = extract_string_field(params_json, "session_id")?.trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

/// Pure helper: find session index by `request_id` in a list of `(session_key, Vec<request_id>)`.
///
/// Mirrors the `for sid, session in live: for pending in list_gateway_approvals(key): if request_id matches → session` loop.
/// Returns the matched `session_key` if any.
pub fn find_session_by_approval_request_id<'a>(live: &'a [(String, Vec<String>)], target_rid: &str) -> Option<&'a String> {
    for (key, pending_ids) in live {
        if key.is_empty() { continue; }
        for pid in pending_ids {
            if pid == target_rid {
                return Some(key);
            }
        }
    }
    None
}

/// Resolve approval session fallback — mirrors `_approval_respond_session_fallback`.
///
/// `list_fn`: `Fn(session_key) -> Vec<request_id>` (mirrors `list_gateway_approvals`),
/// `live_sessions`: `Vec<(sid, session_key)>` snapshot (mirrors `list(_sessions.items())`),
/// `find_by_key`: `Fn(target_sid) -> Option<session_key>` (mirrors `_find_live_session_by_key`).
///
/// Returns `Some(matched_session_key)` or `None`.
pub fn resolve_approval_session_fallback<F1, F2>(
    params_json: &str,
    live_sessions: &[(String, String)],
    mut list_fn: F1,
    mut find_by_key: F2,
) -> Option<String>
where
    F1: FnMut(&str) -> Vec<String>,
    F2: FnMut(&str) -> Option<String>,
{
    if let Some(req_id) = extract_request_id(params_json) {
        // Build (key, pending_ids) for pure scan
        let mut with_pending: Vec<(String, Vec<String>)> = Vec::new();
        for (_, key) in live_sessions {
            if key.is_empty() { continue; }
            let pending = list_fn(key);
            with_pending.push((key.clone(), pending));
        }
        if let Some(hit) = find_session_by_approval_request_id(&with_pending, &req_id) {
            return Some(hit.clone());
        }
    }
    if let Some(target) = extract_target_session_id(params_json) {
        if let Some(hit) = find_by_key(&target) {
            return Some(hit);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Core handlers — mirrors each @method body, injected for std-only testing
// ---------------------------------------------------------------------------

/// Handle `image.attach_bytes`.
///
/// Validates `content_base64`/`data` required (`4015`) handler-side, then delegates
/// `4017`/`4018`/`4016`/`5027` to the injected `op`.
pub fn handle_image_attach_bytes<F>(rid_json: &str, params_json: &str, op: F) -> String
where
    F: Fn(&str) -> Result<String, (i32, String)>,
{
    let raw_b64 = extract_string_field(params_json, "content_base64")
        .or_else(|| extract_string_field(params_json, "data"))
        .unwrap_or_default();
    if raw_b64.trim().is_empty() {
        return err_response(rid_json, ERR_CONTENT_REQUIRED, "content_base64 required");
    }
    match op(params_json) {
        Ok(result_json) => ok_response(rid_json, result_json.trim()),
        Err((code, msg)) => err_response(rid_json, code, &msg),
    }
}

/// Handle `pdf.attach`.
///
/// Validates `path or content_base64` required (`4015`) handler-side, then delegates.
pub fn handle_pdf_attach<F>(rid_json: &str, params_json: &str, op: F) -> String
where
    F: Fn(&str) -> Result<String, (i32, String)>,
{
    let raw_path = extract_string_field(params_json, "path").unwrap_or_default().trim().to_string();
    let raw_b64 = extract_string_field(params_json, "content_base64")
        .or_else(|| extract_string_field(params_json, "data"))
        .unwrap_or_default().trim().to_string();
    if raw_path.is_empty() && raw_b64.is_empty() {
        return err_response(rid_json, ERR_PATH_OR_CONTENT_REQUIRED, "path or content_base64 required");
    }
    // first_page/last_page validation is optionally done handler-side for 4015/4019 caps
    // when params include them as integers; but to preserve `windows_hide_flags` + `pdftoppm`
    // subprocess error `5028` fidelity we let closure own full validation and just pass through.
    // We keep page-range helper exposed for unit tests: callers can assert `validate_pdf_page_range`.
    match op(params_json) {
        Ok(result_json) => ok_response(rid_json, result_json.trim()),
        Err((code, msg)) => err_response(rid_json, code, &msg),
    }
}

/// Handle `file.attach`.
///
/// Validates `path or data_url` required (`4015`) handler-side.
pub fn handle_file_attach<F>(rid_json: &str, params_json: &str, op: F) -> String
where
    F: Fn(&str) -> Result<String, (i32, String)>,
{
    let raw = extract_string_field(params_json, "path").unwrap_or_default().trim().to_string();
    let data_url = extract_string_field(params_json, "data_url").unwrap_or_default().trim().to_string();
    if raw.is_empty() && data_url.is_empty() {
        return err_response(rid_json, ERR_PATH_OR_DATA_URL_REQUIRED, "path or data_url required");
    }
    match op(params_json) {
        Ok(result_json) => ok_response(rid_json, result_json.trim()),
        Err((code, msg)) => err_response(rid_json, code, &msg),
    }
}

/// Handle `image.detach`.
pub fn handle_image_detach<F>(rid_json: &str, params_json: &str, op: F) -> String
where
    F: Fn(&str) -> Result<String, (i32, String)>,
{
    let raw = extract_string_field(params_json, "path").unwrap_or_default().trim().to_string();
    if raw.is_empty() {
        return err_response(rid_json, ERR_PATH_REQUIRED, "path required");
    }
    match op(params_json) {
        Ok(result_json) => ok_response(rid_json, result_json.trim()),
        Err((code, msg)) => err_response(rid_json, code, &msg),
    }
}

/// Handle `input.detect_drop`.
///
/// Delegates all matching (`_detect_file_drop`) + `is_image` branching to `op`.
pub fn handle_input_detect_drop<F>(rid_json: &str, params_json: &str, op: F) -> String
where
    F: Fn(&str) -> Result<String, (i32, String)>,
{
    match op(params_json) {
        Ok(result_json) => ok_response(rid_json, result_json.trim()),
        Err((code, msg)) => err_response(rid_json, code, &msg),
    }
}

/// Handle `prompt.background`.
pub fn handle_prompt_background<F>(rid_json: &str, params_json: &str, op: F) -> String
where
    F: Fn(&str) -> Result<String, (i32, String)>,
{
    let text = extract_string_field(params_json, "text").unwrap_or_default().trim().to_string();
    if text.is_empty() {
        return err_response(rid_json, ERR_TEXT_REQUIRED, "text required");
    }
    match op(params_json) {
        Ok(result_json) => ok_response(rid_json, result_json.trim()),
        Err((code, msg)) => err_response(rid_json, code, &msg),
    }
}

/// Handle `preview.restart`.
///
/// Validates `url` required (`4012`) handler-side, delegates the rest.
pub fn handle_preview_restart<F>(rid_json: &str, params_json: &str, op: F) -> String
where
    F: Fn(&str) -> Result<String, (i32, String)>,
{
    let url = extract_string_field(params_json, "url").unwrap_or_default().trim().to_string();
    if url.is_empty() {
        return err_response(rid_json, ERR_URL_REQUIRED, "url required");
    }
    match op(params_json) {
        Ok(result_json) => ok_response(rid_json, result_json.trim()),
        Err((code, msg)) => err_response(rid_json, code, &msg),
    }
}

/// Generic `_respond` wrapper — `allow_expired=True` mirrors Python.
///
/// `key` is the `params.get(key,"")` field stored into `_answers` (e.g. `answer`,`text`,`password`).
pub fn handle_respond_generic<F>(rid_json: &str, params_json: &str, op: F) -> String
where
    F: Fn(&str) -> Result<String, (i32, String)>,
{
    match op(params_json) {
        Ok(result_json) => ok_response(rid_json, result_json.trim()),
        Err((code, msg)) => err_response(rid_json, code, &msg),
    }
}

/// Handle `clarify.respond` (`answer`, allow_expired)
pub fn handle_clarify_respond<F>(rid_json: &str, params_json: &str, op: F) -> String where F: Fn(&str) -> Result<String,(i32,String)> { handle_respond_generic(rid_json, params_json, op) }
/// Handle `terminal.read.respond` (`text`, allow_expired)
pub fn handle_terminal_read_respond<F>(rid_json: &str, params_json: &str, op: F) -> String where F: Fn(&str) -> Result<String,(i32,String)> { handle_respond_generic(rid_json, params_json, op) }
/// Handle `preview.read.respond` (`text`)
pub fn handle_preview_read_respond<F>(rid_json: &str, params_json: &str, op: F) -> String where F: Fn(&str) -> Result<String,(i32,String)> { handle_respond_generic(rid_json, params_json, op) }
/// Handle `preview.act.respond` (`text`)
pub fn handle_preview_act_respond<F>(rid_json: &str, params_json: &str, op: F) -> String where F: Fn(&str) -> Result<String,(i32,String)> { handle_respond_generic(rid_json, params_json, op) }
/// Handle `window.read.respond` (`text`)
pub fn handle_window_read_respond<F>(rid_json: &str, params_json: &str, op: F) -> String where F: Fn(&str) -> Result<String,(i32,String)> { handle_respond_generic(rid_json, params_json, op) }
/// Handle `tour.respond` (`text`)
pub fn handle_tour_respond<F>(rid_json: &str, params_json: &str, op: F) -> String where F: Fn(&str) -> Result<String,(i32,String)> { handle_respond_generic(rid_json, params_json, op) }
/// Handle `mcp.setup.respond` (`result`)
pub fn handle_mcp_setup_respond<F>(rid_json: &str, params_json: &str, op: F) -> String where F: Fn(&str) -> Result<String,(i32,String)> { handle_respond_generic(rid_json, params_json, op) }
/// Handle `sudo.respond` (`password`)
pub fn handle_sudo_respond<F>(rid_json: &str, params_json: &str, op: F) -> String where F: Fn(&str) -> Result<String,(i32,String)> { handle_respond_generic(rid_json, params_json, op) }
/// Handle `secret.respond` (`value`)
pub fn handle_secret_respond<F>(rid_json: &str, params_json: &str, op: F) -> String where F: Fn(&str) -> Result<String,(i32,String)> { handle_respond_generic(rid_json, params_json, op) }

/// Handle `approval.pending`.
pub fn handle_approval_pending<F>(rid_json: &str, params_json: &str, op: F) -> String
where
    F: Fn(&str) -> Result<String, (i32, String)>,
{
    match op(params_json) {
        Ok(result_json) => ok_response(rid_json, result_json.trim()),
        Err((code, msg)) => err_response(rid_json, code, &msg),
    }
}

/// Handle `approval.received`.
pub fn handle_approval_received<F>(rid_json: &str, params_json: &str, op: F) -> String
where
    F: Fn(&str) -> Result<String, (i32, String)>,
{
    let request_id = extract_string_field(params_json, "request_id").unwrap_or_default().trim().to_string();
    if request_id.is_empty() {
        return err_response(rid_json, ERR_REQUEST_ID_REQUIRED, "request_id required");
    }
    match op(params_json) {
        Ok(result_json) => ok_response(rid_json, result_json.trim()),
        Err((code, msg)) => err_response(rid_json, code, &msg),
    }
}

/// Handle `approval.respond`.
///
/// Session lookup `_sess` `4001` + fallback is owned by `op` so the `4001`-only
/// durable fallback semantics are preserved; this wrapper only delegates.
pub fn handle_approval_respond<F>(rid_json: &str, params_json: &str, op: F) -> String
where
    F: Fn(&str) -> Result<String, (i32, String)>,
{
    match op(params_json) {
        Ok(result_json) => ok_response(rid_json, result_json.trim()),
        Err((code, msg)) => err_response(rid_json, code, &msg),
    }
}

// ---------------------------------------------------------------------------
// Registry wiring — mirrors _registry = HandlerRegistry() + register(server)
// ---------------------------------------------------------------------------

/// Build a fresh [`HandlerRegistry`] with the 19 slice-2 methods registered
/// using the provided deps (for tests / production injection).
///
/// Each closure is `'static` and mirrors the lazy imports inside Python
/// handler bodies. For the default stub (no backend) use [`build_registry_default`].
#[allow(clippy::too_many_arguments)]
pub fn build_registry<IAB, PA, FA, ID, DD, BG, PR, CR, TR, PRR, PA2, WR, TO, MS, SU, SE, AP, AR, AS_>(
    image_attach_bytes: IAB,
    pdf_attach: PA,
    file_attach: FA,
    image_detach: ID,
    input_detect_drop: DD,
    prompt_background: BG,
    preview_restart: PR,
    clarify_respond: CR,
    terminal_read_respond: TR,
    preview_read_respond: PRR,
    preview_act_respond: PA2,
    window_read_respond: WR,
    tour_respond: TO,
    mcp_setup_respond: MS,
    sudo_respond: SU,
    secret_respond: SE,
    approval_pending: AP,
    approval_received: AR,
    approval_respond: AS_,
) -> HandlerRegistry
where
    IAB: Fn(String, String) -> String + Send + Sync + 'static,
    PA: Fn(String, String) -> String + Send + Sync + 'static,
    FA: Fn(String, String) -> String + Send + Sync + 'static,
    ID: Fn(String, String) -> String + Send + Sync + 'static,
    DD: Fn(String, String) -> String + Send + Sync + 'static,
    BG: Fn(String, String) -> String + Send + Sync + 'static,
    PR: Fn(String, String) -> String + Send + Sync + 'static,
    CR: Fn(String, String) -> String + Send + Sync + 'static,
    TR: Fn(String, String) -> String + Send + Sync + 'static,
    PRR: Fn(String, String) -> String + Send + Sync + 'static,
    PA2: Fn(String, String) -> String + Send + Sync + 'static,
    WR: Fn(String, String) -> String + Send + Sync + 'static,
    TO: Fn(String, String) -> String + Send + Sync + 'static,
    MS: Fn(String, String) -> String + Send + Sync + 'static,
    SU: Fn(String, String) -> String + Send + Sync + 'static,
    SE: Fn(String, String) -> String + Send + Sync + 'static,
    AP: Fn(String, String) -> String + Send + Sync + 'static,
    AR: Fn(String, String) -> String + Send + Sync + 'static,
    AS_: Fn(String, String) -> String + Send + Sync + 'static,
{
    let mut reg = HandlerRegistry::new();
    register_with(
        &mut reg,
        image_attach_bytes,
        pdf_attach,
        file_attach,
        image_detach,
        input_detect_drop,
        prompt_background,
        preview_restart,
        clarify_respond,
        terminal_read_respond,
        preview_read_respond,
        preview_act_respond,
        window_read_respond,
        tour_respond,
        mcp_setup_respond,
        sudo_respond,
        secret_respond,
        approval_pending,
        approval_received,
        approval_respond,
    );
    reg
}

/// Build a registry with default stubs (every operation returns error / `expired` via 4009).
pub fn build_registry_default() -> HandlerRegistry {
    build_registry(
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_image_attach_bytes(&rid_json, &params_json, |_| Err((ERR_WRITE_FAILED, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_pdf_attach(&rid_json, &params_json, |_| Err((ERR_PDFTOPPM, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_file_attach(&rid_json, &params_json, |_| Err((ERR_FILE_ATTACH, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_image_detach(&rid_json, &params_json, |_| Err((ERR_DETECT_DROP, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_input_detect_drop(&rid_json, &params_json, |_| Err((ERR_DETECT_DROP, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_prompt_background(&rid_json, &params_json, |_| Err((ERR_APPROVAL, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_preview_restart(&rid_json, &params_json, |_| Err((ERR_APPROVAL, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_clarify_respond(&rid_json, &params_json, |_| Err((ERR_NO_PENDING, "no pending answer request".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_terminal_read_respond(&rid_json, &params_json, |_| Err((ERR_NO_PENDING, "no pending text request".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_preview_read_respond(&rid_json, &params_json, |_| Err((ERR_NO_PENDING, "no pending text request".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_preview_act_respond(&rid_json, &params_json, |_| Err((ERR_NO_PENDING, "no pending text request".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_window_read_respond(&rid_json, &params_json, |_| Err((ERR_NO_PENDING, "no pending text request".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_tour_respond(&rid_json, &params_json, |_| Err((ERR_NO_PENDING, "no pending text request".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_mcp_setup_respond(&rid_json, &params_json, |_| Err((ERR_NO_PENDING, "no pending result request".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_sudo_respond(&rid_json, &params_json, |_| Err((ERR_NO_PENDING, "no pending password request".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_secret_respond(&rid_json, &params_json, |_| Err((ERR_NO_PENDING, "no pending value request".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_approval_pending(&rid_json, &params_json, |_| Err((ERR_APPROVAL, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_approval_received(&rid_json, &params_json, |_| Err((ERR_APPROVAL, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_approval_respond(&rid_json, &params_json, |_| Err((ERR_APPROVAL, "no backend".to_string())))
        },
    )
}

/// Register all 19 slice-2 methods onto an existing registry.
#[allow(clippy::too_many_arguments)]
pub fn register_with<IAB, PA, FA, ID, DD, BG, PR, CR, TR, PRR, PA2, WR, TO, MS, SU, SE, AP, AR, AS_>(
    registry: &mut HandlerRegistry,
    image_attach_bytes: IAB,
    pdf_attach: PA,
    file_attach: FA,
    image_detach: ID,
    input_detect_drop: DD,
    prompt_background: BG,
    preview_restart: PR,
    clarify_respond: CR,
    terminal_read_respond: TR,
    preview_read_respond: PRR,
    preview_act_respond: PA2,
    window_read_respond: WR,
    tour_respond: TO,
    mcp_setup_respond: MS,
    sudo_respond: SU,
    secret_respond: SE,
    approval_pending: AP,
    approval_received: AR,
    approval_respond: AS_,
) where
    IAB: Fn(String, String) -> String + Send + Sync + 'static,
    PA: Fn(String, String) -> String + Send + Sync + 'static,
    FA: Fn(String, String) -> String + Send + Sync + 'static,
    ID: Fn(String, String) -> String + Send + Sync + 'static,
    DD: Fn(String, String) -> String + Send + Sync + 'static,
    BG: Fn(String, String) -> String + Send + Sync + 'static,
    PR: Fn(String, String) -> String + Send + Sync + 'static,
    CR: Fn(String, String) -> String + Send + Sync + 'static,
    TR: Fn(String, String) -> String + Send + Sync + 'static,
    PRR: Fn(String, String) -> String + Send + Sync + 'static,
    PA2: Fn(String, String) -> String + Send + Sync + 'static,
    WR: Fn(String, String) -> String + Send + Sync + 'static,
    TO: Fn(String, String) -> String + Send + Sync + 'static,
    MS: Fn(String, String) -> String + Send + Sync + 'static,
    SU: Fn(String, String) -> String + Send + Sync + 'static,
    SE: Fn(String, String) -> String + Send + Sync + 'static,
    AP: Fn(String, String) -> String + Send + Sync + 'static,
    AR: Fn(String, String) -> String + Send + Sync + 'static,
    AS_: Fn(String, String) -> String + Send + Sync + 'static,
{
    registry.method(METHOD_IMAGE_ATTACH_BYTES, image_attach_bytes);
    registry.method(METHOD_PDF_ATTACH, pdf_attach);
    registry.method(METHOD_FILE_ATTACH, file_attach);
    registry.method(METHOD_IMAGE_DETACH, image_detach);
    registry.method(METHOD_INPUT_DETECT_DROP, input_detect_drop);
    registry.method(METHOD_PROMPT_BACKGROUND, prompt_background);
    registry.method(METHOD_PREVIEW_RESTART, preview_restart);
    registry.method(METHOD_CLARIFY_RESPOND, clarify_respond);
    registry.method(METHOD_TERMINAL_READ_RESPOND, terminal_read_respond);
    registry.method(METHOD_PREVIEW_READ_RESPOND, preview_read_respond);
    registry.method(METHOD_PREVIEW_ACT_RESPOND, preview_act_respond);
    registry.method(METHOD_WINDOW_READ_RESPOND, window_read_respond);
    registry.method(METHOD_TOUR_RESPOND, tour_respond);
    registry.method(METHOD_MCP_SETUP_RESPOND, mcp_setup_respond);
    registry.method(METHOD_SUDO_RESPOND, sudo_respond);
    registry.method(METHOD_SECRET_RESPOND, secret_respond);
    registry.method(METHOD_APPROVAL_PENDING, approval_pending);
    registry.method(METHOD_APPROVAL_RECEIVED, approval_received);
    registry.method(METHOD_APPROVAL_RESPOND, approval_respond);
}

/// Register with default stubs onto `registry`.
pub fn register(registry: &mut HandlerRegistry) {
    register_with(
        registry,
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_image_attach_bytes(&rid_json, &params_json, |_| Err((ERR_WRITE_FAILED, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_pdf_attach(&rid_json, &params_json, |_| Err((ERR_PDFTOPPM, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_file_attach(&rid_json, &params_json, |_| Err((ERR_FILE_ATTACH, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_image_detach(&rid_json, &params_json, |_| Err((ERR_DETECT_DROP, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_input_detect_drop(&rid_json, &params_json, |_| Err((ERR_DETECT_DROP, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_prompt_background(&rid_json, &params_json, |_| Err((ERR_APPROVAL, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_preview_restart(&rid_json, &params_json, |_| Err((ERR_APPROVAL, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_clarify_respond(&rid_json, &params_json, |_| Err((ERR_NO_PENDING, "no pending answer request".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_terminal_read_respond(&rid_json, &params_json, |_| Err((ERR_NO_PENDING, "no pending text request".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_preview_read_respond(&rid_json, &params_json, |_| Err((ERR_NO_PENDING, "no pending text request".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_preview_act_respond(&rid_json, &params_json, |_| Err((ERR_NO_PENDING, "no pending text request".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_window_read_respond(&rid_json, &params_json, |_| Err((ERR_NO_PENDING, "no pending text request".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_tour_respond(&rid_json, &params_json, |_| Err((ERR_NO_PENDING, "no pending text request".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_mcp_setup_respond(&rid_json, &params_json, |_| Err((ERR_NO_PENDING, "no pending result request".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_sudo_respond(&rid_json, &params_json, |_| Err((ERR_NO_PENDING, "no pending password request".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_secret_respond(&rid_json, &params_json, |_| Err((ERR_NO_PENDING, "no pending value request".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_approval_pending(&rid_json, &params_json, |_| Err((ERR_APPROVAL, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_approval_received(&rid_json, &params_json, |_| Err((ERR_APPROVAL, "no backend".to_string())))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_approval_respond(&rid_json, &params_json, |_| Err((ERR_APPROVAL, "no backend".to_string())))
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
    fn image_attach_tail_helpers() {
        assert!(is_supported_image_ext(".png"));
        assert!(is_supported_image_ext(".PNG"));
        assert!(is_supported_image_ext("jpg"));
        assert!(!is_supported_image_ext(".txt"));
        assert!(!is_supported_image_ext(".pdf"));
        assert_eq!(sniff_image_ext_from_filename("photo.jpg"), ".jpg");
        assert_eq!(sniff_image_ext_from_filename("noext"), ".png");
        assert_eq!(sniff_image_ext_from_filename(""), ".png");
        assert_eq!(format_image_attach_text("", "a.png"), "[User attached image: a.png]");
        assert_eq!(format_image_attach_text("hello", "a.png"), "hello");
        assert_eq!(format_image_attach_text("  ", "b.jpg"), "[User attached image: b.jpg]");
        // limits
        assert_eq!(ATTACH_BYTES_MAX_BYTES, 25 * 1024 * 1024);
        assert_eq!(PDF_ATTACH_MAX_BYTES, 50 * 1024 * 1024);
        assert_eq!(PDF_ATTACH_MAX_PAGES, 25);
    }

    #[test]
    fn base64_chars_check() {
        assert!(is_valid_base64_chars("aGVsbG8="));
        assert!(is_valid_base64_chars(" data:image/png;base64,aGVsbG8= "));
        assert!(!is_valid_base64_chars(""));
        assert!(!is_valid_base64_chars("not*base64!"));
        assert!(is_valid_base64_chars("YWJjZA=="));
    }

    #[test]
    fn pdf_page_range() {
        assert_eq!(validate_pdf_page_range(None, None).unwrap(), (1, 25));
        assert_eq!(validate_pdf_page_range(Some("1"), Some("5")).unwrap(), (1,5));
        assert_eq!(validate_pdf_page_range(Some("3"), None).unwrap(), (3, 27));
        assert!(validate_pdf_page_range(Some("0"), None).is_err());
        assert_eq!(validate_pdf_page_range(Some("0"), None).unwrap_err().0, ERR_PATH_REQUIRED);
        assert!(validate_pdf_page_range(Some("5"), Some("2")).is_err());
        assert!(validate_pdf_page_range(Some("1"), Some("30")).is_err());
        assert_eq!(validate_pdf_page_range(Some("1"), Some("30")).unwrap_err().0, ERR_PAGE_RANGE_EXCEEDS);
        assert!(validate_pdf_page_range(Some("bad"), None).is_err());
        assert_eq!(format_ref_value("simple/path.png"), "simple/path.png");
        assert_eq!(format_ref_value("a b"), "\"a b\"");
        assert_eq!(format_file_ref_text("a b/c.png"), "@file:\"a b/c.png\"");
        assert_eq!(format_file_ref_text("a.png"), "@file:a.png");
    }

    #[test]
    fn image_attach_bytes_requires_b64() {
        let rid = rid1();
        let err = handle_image_attach_bytes(&rid, r#"{}"#, |_| Ok(r#"{"attached":true}"#.to_string()));
        assert!(err.contains(r#""code":4015"#), "{}", err);
        assert!(err.contains("content_base64 required"));
        let err2 = handle_image_attach_bytes(&rid, r#"{"data":""}"#, |_| Ok(r#"{}"#.to_string()));
        assert!(err2.contains(r#""code":4015"#));
        let ok = handle_image_attach_bytes(&rid, r#"{"content_base64":"aGVsbG8="}"#, |_| Ok(r#"{"attached":true,"path":"/tmp/x.png","count":1,"bytes":5}"#.to_string()));
        assert!(ok.contains(r#""attached":true"#), "{}", ok);
        assert!(ok.contains(r#""bytes":5"#));
        let err3 = handle_image_attach_bytes(&rid, r#"{"content_base64":"aGVsbG8="}"#, |_| Err((ERR_BASE64_INVALID, "data is not valid base64".into())));
        assert!(err3.contains(r#""code":4017"#));
        let err4 = handle_image_attach_bytes(&rid, r#"{"content_base64":"aGVsbG8="}"#, |_| Err((ERR_IMAGE_TOO_LARGE, "image too large".into())));
        assert!(err4.contains(r#""code":4018"#));
        let err5 = handle_image_attach_bytes(&rid, r#"{"content_base64":"aGVsbG8="}"#, |_| Err((ERR_UNSUPPORTED_IMAGE, "unsupported image extension: .xyz".into())));
        assert!(err5.contains(r#""code":4016"#));
    }

    #[test]
    fn pdf_attach_requires_path_or_b64() {
        let rid = rid1();
        let err = handle_pdf_attach(&rid, r#"{}"#, |_| Ok("{}".to_string()));
        assert!(err.contains(r#""code":4015"#), "{}", err);
        assert!(err.contains("path or content_base64 required"));
        let ok = handle_pdf_attach(&rid, r#"{"path":"/tmp/a.pdf"}"#, |_| Ok(r#"{"attached":true,"filename":"a.pdf","pages_attached":2,"pages":[]}"#.to_string()));
        assert!(ok.contains(r#""attached":true"#), "{}", ok);
        let ok2 = handle_pdf_attach(&rid, r#"{"content_base64":"JVBERi0="}"#, |_| Ok(r#"{"attached":true,"filename":"uploaded.pdf","pages_attached":1}"#.into()));
        assert!(ok2.contains("pages_attached"), "{}", ok2);
        let err2 = handle_pdf_attach(&rid, r#"{"path":"/tmp/a.pdf"}"#, |_| Err((ERR_PDFTOPPM, "pdftoppm not installed".into())));
        assert!(err2.contains(r#""code":5028"#));
        let err3 = handle_pdf_attach(&rid, r#"{"path":"/tmp/a.pdf","first_page":"bad"}"#, |_| Err((ERR_PATH_REQUIRED, "first_page/last_page must be integers".into())));
        assert!(err3.contains(r#""code":4015"#) || err3.contains("4015"));
        let err4 = handle_pdf_attach(&rid, r#"{"path":"/tmp/a.pdf"}"#, |_| Err((ERR_PAGE_RANGE_EXCEEDS, "page range exceeds cap".into())));
        assert!(err4.contains(r#""code":4019"#));
    }

    #[test]
    fn file_attach_requires_path_or_url() {
        let rid = rid1();
        let err = handle_file_attach(&rid, r#"{}"#, |_| Ok("{}".into()));
        assert!(err.contains(r#""code":4015"#), "{}", err);
        let err2 = handle_file_attach(&rid, r#"{"path":""}"#, |_| Ok("{}".into()));
        assert!(err2.contains(r#""code":4015"#));
        let ok = handle_file_attach(&rid, r#"{"path":"/tmp/foo.txt"}"#, |_| Ok(r#"{"attached":true,"name":"foo.txt","path":"/tmp/gray/files/foo.txt","ref_path":"files/foo.txt","ref_text":"@file:files/foo.txt","uploaded":false}"#.into()));
        assert!(ok.contains(r#""attached":true"#), "{}", ok);
        assert!(ok.contains("@file:"));
        let ok2 = handle_file_attach(&rid, r#"{"data_url":"data:text/plain;base64,aGVsbG8="}"#, |_| Ok(r#"{"attached":true,"name":"hello.txt","uploaded":true}"#.into()));
        assert!(ok2.contains("uploaded"), "{}", ok2);
        let err3 = handle_file_attach(&rid, r#"{"path":"/tmp/a.txt"}"#, |_| Err((ERR_FILE_ATTACH, "stage failed".into())));
        assert!(err3.contains(r#""code":5028"#));
    }

    #[test]
    fn image_detach_requires_path() {
        let rid = rid1();
        let err = handle_image_detach(&rid, r#"{}"#, |_| Ok(r#"{"detached":true,"count":0}"#.into()));
        assert!(err.contains(r#""code":4015"#), "{}", err);
        let err2 = handle_image_detach(&rid, r#"{"path":""}"#, |_| Ok("{}".into()));
        assert!(err2.contains(r#""code":4015"#));
        let ok = handle_image_detach(&rid, r#"{"path":"/tmp/a.png"}"#, |_| Ok(r#"{"detached":true,"count":0}"#.into()));
        assert!(ok.contains(r#""detached":true"#), "{}", ok);
        let ok2 = handle_image_detach(&rid, r#"{"path":"/tmp/missing.png"}"#, |_| Ok(r#"{"detached":false,"count":1}"#.into()));
        assert!(ok2.contains(r#""detached":false"#));
    }

    #[test]
    fn input_detect_drop_ok_and_err() {
        let rid = rid1();
        let ok = handle_input_detect_drop(&rid, r#"{"text":"hello /tmp/a.png"}"#, |_| Ok(r#"{"matched":true,"is_image":true,"path":"/tmp/a.png","count":1,"text":"hello"}"#.into()));
        assert!(ok.contains(r#""matched":true"#), "{}", ok);
        let ok2 = handle_input_detect_drop(&rid, r#"{"text":"no drop"}"#, |_| Ok(r#"{"matched":false}"#.into()));
        assert!(ok2.contains(r#""matched":false"#));
        let err = handle_input_detect_drop(&rid, "{}", |_| Err((ERR_DETECT_DROP, "boom".into())));
        assert!(err.contains(r#""code":5027"#));
    }

    #[test]
    fn background_requires_text() {
        let rid = rid1();
        let err = handle_prompt_background(&rid, r#"{"session_id":"abc"}"#, |_| Ok("{}".into()));
        assert!(err.contains(r#""code":4012"#), "{}", err);
        assert!(err.contains("text required"));
        let err2 = handle_prompt_background(&rid, r#"{"text":""}"#, |_| Ok("{}".into()));
        assert!(err2.contains(r#""code":4012"#));
        let ok = handle_prompt_background(&rid, r#"{"session_id":"abc","text":"hello"}"#, |_| Ok(r#"{"task_id":"bg_abcdef"}"#.into()));
        assert!(ok.contains("task_id"), "{}", ok);
    }

    #[test]
    fn preview_restart_requires_url() {
        let rid = rid1();
        let err = handle_preview_restart(&rid, r#"{"session_id":"a"}"#, |_| Ok("{}".into()));
        assert!(err.contains(r#""code":4012"#), "{}", err);
        assert!(err.contains("url required"));
        let err2 = handle_preview_restart(&rid, r#"{"url":""}"#, |_| Ok("{}".into()));
        assert!(err2.contains(r#""code":4012"#));
        let ok = handle_preview_restart(&rid, r#"{"session_id":"a","url":"http://localhost:3000"}"#, |_| Ok(r#"{"task_id":"preview_abc"}"#.into()));
        assert!(ok.contains("preview_abc"), "{}", ok);
    }

    #[test]
    fn respond_generic_expired_and_ok() {
        let rid = rid1();
        // clarify.respond success
        let ok = handle_clarify_respond(&rid, r#"{"request_id":"r1","answer":"yes"}"#, |_| Ok(r#"{"status":"ok","remaining":[]}"#.into()));
        assert!(ok.contains(r#""status":"ok""#), "{}", ok);
        let expired = handle_clarify_respond(&rid, r#"{"request_id":"r1","answer":"late"}"#, |_| Ok(r#"{"status":"expired"}"#.into()));
        assert!(expired.contains("expired"), "{}", expired);
        let err = handle_clarify_respond(&rid, r#"{"request_id":"bad"}"#, |_| Err((ERR_NO_PENDING, "no pending answer request".into())));
        assert!(err.contains(r#""code":4009"#));
        // terminal.read
        let ok2 = handle_terminal_read_respond(&rid, r#"{"request_id":"r1","text":"buf"}"#, |_| Ok(r#"{"status":"ok"}"#.into()));
        assert!(ok2.contains(r#""status":"ok""#));
        // preview.read
        let ok3 = handle_preview_read_respond(&rid, r#"{"request_id":"r1","text":"{}"}"#, |_| Ok(r#"{"status":"ok"}"#.into()));
        assert!(ok3.contains("ok"));
        // preview.act
        let ok4 = handle_preview_act_respond(&rid, r#"{"request_id":"r1","text":"{}"}"#, |_| Ok(r#"{"status":"ok"}"#.into()));
        assert!(ok4.contains("ok"));
        // window.read
        let ok5 = handle_window_read_respond(&rid, r#"{"request_id":"r1","text":"{}"}"#, |_| Ok(r#"{"status":"ok"}"#.into()));
        assert!(ok5.contains("ok"));
        // tour
        let ok6 = handle_tour_respond(&rid, r#"{"request_id":"r1","text":"{}"}"#, |_| Ok(r#"{"status":"ok"}"#.into()));
        assert!(ok6.contains("ok"));
        // mcp.setup
        let ok7 = handle_mcp_setup_respond(&rid, r#"{"request_id":"r1","result":"{}"}"#, |_| Ok(r#"{"status":"ok"}"#.into()));
        assert!(ok7.contains("ok"));
        // sudo
        let ok8 = handle_sudo_respond(&rid, r#"{"request_id":"r1","password":"hunter2"}"#, |_| Ok(r#"{"status":"ok"}"#.into()));
        assert!(ok8.contains("ok"));
        let err_sudo = handle_sudo_respond(&rid, r#"{"request_id":"bad"}"#, |_| Err((ERR_NO_PENDING, "no pending password request".into())));
        assert!(err_sudo.contains(r#""code":4009"#));
        // secret
        let ok9 = handle_secret_respond(&rid, r#"{"request_id":"r1","value":"s3cr3t"}"#, |_| Ok(r#"{"status":"ok"}"#.into()));
        assert!(ok9.contains("ok"));
        // unknown question_id branch (4002) via clarify batch
        let err2 = handle_clarify_respond(&rid, r#"{"request_id":"r1","question_id":"q9","answer":"a"}"#, |_| Err((ERR_UNKNOWN_QUESTION, "unknown question_id 'q9'".into())));
        assert!(err2.contains(r#""code":4002"#));
    }

    #[test]
    fn approval_pending_and_received() {
        let rid = rid1();
        let ok = handle_approval_pending(&rid, r#"{"session_id":"s1"}"#, |_| Ok(r#"{"approvals":[]}"#.into()));
        assert!(ok.contains("approvals"), "{}", ok);
        let ok2 = handle_approval_pending(&rid, r#"{"session_id":"s1"}"#, |_| Ok(r#"{"approvals":[{"request_id":"r1","command":"rm"}]}"#.into()));
        assert!(ok2.contains("r1"), "{}", ok2);
        let err = handle_approval_pending(&rid, "{}", |_| Err((ERR_APPROVAL, "boom".into())));
        assert!(err.contains(r#""code":5004"#));

        let err2 = handle_approval_received(&rid, r#"{"session_id":"s1"}"#, |_| Ok("{}".into()));
        assert!(err2.contains(r#""code":4006"#), "{}", err2);
        let err3 = handle_approval_received(&rid, r#"{"request_id":""}"#, |_| Ok("{}".into()));
        assert!(err3.contains(r#""code":4006"#));
        let ok3 = handle_approval_received(&rid, r#"{"session_id":"s1","request_id":"r1"}"#, |_| Ok(r#"{"acknowledged":true}"#.into()));
        assert!(ok3.contains("acknowledged"), "{}", ok3);
        let ok4 = handle_approval_received(&rid, r#"{"session_id":"s1","request_id":"r1"}"#, |_| Ok(r#"{"acknowledged":false}"#.into()));
        assert!(ok4.contains("false"));
    }

    #[test]
    fn approval_fallback_helpers() {
        // pure helper scan
        let live = vec![("s1".to_string(), vec!["r1".to_string(), "r2".to_string()]), ("s2".to_string(), vec!["r3".to_string()])];
        assert_eq!(find_session_by_approval_request_id(&live, "r1"), Some(&"s1".to_string()));
        assert_eq!(find_session_by_approval_request_id(&live, "r3"), Some(&"s2".to_string()));
        assert_eq!(find_session_by_approval_request_id(&live, "missing"), None);
        // empty key skipped
        let live2 = vec![("".to_string(), vec!["r1".to_string()]), ("s1".to_string(), vec!["r1".to_string()])];
        assert_eq!(find_session_by_approval_request_id(&live2, "r1"), Some(&"s1".to_string()));

        // resolve helper via request_id
        let live_sessions = vec![("live1".to_string(), "key1".to_string()), ("live2".to_string(), "key2".to_string())];
        let hit = resolve_approval_session_fallback(r#"{"request_id":"r1"}"#, &live_sessions,
            |k| if k=="key1" { vec!["r1".into()] } else { vec![] },
            |_t| None);
        assert_eq!(hit, Some("key1".to_string()));
        // fallback via session_id
        let hit2 = resolve_approval_session_fallback(r#"{"session_id":"live2"}"#, &live_sessions,
            |_| vec![],
            |t| if t=="live2" { Some("key2".to_string()) } else { None });
        assert_eq!(hit2, Some("key2".to_string()));
        // none
        let miss = resolve_approval_session_fallback(r#"{"session_id":"missing"}"#, &live_sessions, |_| vec![], |_| None);
        assert_eq!(miss, None);
        // request_id takes precedence over session_id
        let hit3 = resolve_approval_session_fallback(r#"{"request_id":"r1","session_id":"live2"}"#, &live_sessions,
            |k| if k=="key1" { vec!["r1".into()] } else { vec![] },
            |t| if t=="live2" { Some("key2".into()) } else { None });
        assert_eq!(hit3, Some("key1".into()));
    }

    #[test]
    fn approval_respond_delegates() {
        let rid = rid1();
        let ok = handle_approval_respond(&rid, r#"{"session_id":"s1","choice":"allow"}"#, |_| Ok(r#"{"resolved":true}"#.into()));
        assert!(ok.contains("resolved"), "{}", ok);
        let ok2 = handle_approval_respond(&rid, r#"{"session_id":"s1","choice":"deny","all":true}"#, |_| Ok(r#"{"resolved":2}"#.into()));
        assert!(ok2.contains("2"));
        let err = handle_approval_respond(&rid, "{}", |_| Err((ERR_APPROVAL, "boom".into())));
        assert!(err.contains(r#""code":5004"#));
        // fallback 4001 path is delegated: closure returns 4001 -> handler wraps
        let err2 = handle_approval_respond(&rid, r#"{"session_id":"stale"}"#, |_| Err((4001, "session not found".into())));
        assert!(err2.contains(r#""code":4001"#));
    }

    #[test]
    fn registry_installs_nineteen() {
        let mut reg = build_registry_default();
        assert_eq!(reg.len(), 19);
        let mut names: Vec<_> = reg.pending_names().collect();
        names.sort();
        assert_eq!(names, vec!["approval.pending","approval.received","approval.respond","clarify.respond","file.attach","image.attach_bytes","image.detach","input.detect_drop","mcp.setup.respond","pdf.attach","preview.act.respond","preview.read.respond","preview.restart","prompt.background","secret.respond","sudo.respond","terminal.read.respond","tour.respond","window.read.respond"]);
        let mut map = HashMap::new();
        reg.install_into(&mut map);
        assert_eq!(map.len(), 19);
        // image.attach_bytes without b64 should be 4015 even with default stub
        let out = map.get(METHOD_IMAGE_ATTACH_BYTES).unwrap()("1".to_string(), "{}".to_string());
        assert!(out.contains(r#""code":4015"#), "{}", out);
        // pdf.attach without path/b64 should be 4015
        let out2 = map.get(METHOD_PDF_ATTACH).unwrap()("1".to_string(), "{}".to_string());
        assert!(out2.contains(r#""code":4015"#), "{}", out2);
        // file.attach without path/data_url → 4015
        let out3 = map.get(METHOD_FILE_ATTACH).unwrap()("1".to_string(), "{}".to_string());
        assert!(out3.contains(r#""code":4015"#), "{}", out3);
        // image.detach without path → 4015
        let out4 = map.get(METHOD_IMAGE_DETACH).unwrap()("1".to_string(), "{}".to_string());
        assert!(out4.contains(r#""code":4015"#), "{}", out4);
        // prompt.background without text → 4012
        let out5 = map.get(METHOD_PROMPT_BACKGROUND).unwrap()("1".to_string(), "{}".to_string());
        assert!(out5.contains(r#""code":4012"#), "{}", out5);
        // preview.restart without url → 4012
        let out6 = map.get(METHOD_PREVIEW_RESTART).unwrap()("1".to_string(), "{}".to_string());
        assert!(out6.contains(r#""code":4012"#), "{}", out6);
        // approval.received without request_id → 4006
        let out7 = map.get(METHOD_APPROVAL_RECEIVED).unwrap()("1".to_string(), "{}".to_string());
        assert!(out7.contains(r#""code":4006"#), "{}", out7);
        // clarify.respond with default stub → 4009
        let out8 = map.get(METHOD_CLARIFY_RESPOND).unwrap()("1".to_string(), r#"{"request_id":"r1"}"#.to_string());
        assert!(out8.contains(r#""code":4009"#), "{}", out8);
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
    fn is_truthy_and_extract() {
        assert!(is_truthy_value(Some("true")));
        assert!(is_truthy_value(Some("1")));
        assert!(!is_truthy_value(Some("false")));
        assert!(!is_truthy_value(None));
        assert_eq!(extract_string_field(r#"{"path":"/tmp/a.png"}"#, "path").as_deref(), Some("/tmp/a.png"));
        assert_eq!(extract_string_field(r#"{"request_id":"r1"}"#, "request_id").as_deref(), Some("r1"));
        assert_eq!(extract_string_field(r#"{"confirm_truncate":true}"#, "confirm_truncate"), None);
        assert!(is_truthy_field(r#"{"confirm":true}"#, "confirm"));
        assert!(!is_truthy_field(r#"{"confirm":false}"#, "confirm"));
    }
}
