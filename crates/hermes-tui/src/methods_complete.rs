//! Completion / model-key / paste JSON-RPC handlers.
//!
//! 1:1 port of `tui_gateway/methods_complete.py` (626 lines).
//!
//! ```python
//! # Python — tui_gateway/methods_complete.py
//! """Completion / model-key / paste JSON-RPC handlers (moved verbatim from server.py).
//!
//! Handler bodies are byte-identical to their pre-split server.py form; they
//! are rebound onto server.py's globals at install time — see method_ctx.py.
//! """
//! from .method_ctx import HandlerRegistry
//! _registry = HandlerRegistry()
//! method = _registry.method
//!
//! @method("paste.collapse")
//! def _(rid, params: dict) -> dict:
//!     text = params.get("text", "")
//!     if not text: return _err(rid, 4004, "empty paste")
//!     _paste_counter += 1
//!     line_count = text.count("\n") + 1
//!     paste_dir = _hermes_home / "pastes"; paste_dir.mkdir(parents=True, exist_ok=True)
//!     paste_file = paste_dir / f"paste_{_paste_counter}_{datetime.now().strftime('%H%M%S')}.txt"
//!     paste_file.write_text(text, encoding="utf-8")
//!     placeholder = f"[Pasted text #{_paste_counter}: {line_count} lines \u2192 {paste_file}]"
//!     return _ok(rid, {"placeholder": placeholder, "path": str(paste_file), "lines": line_count})
//!
//! @method("complete.path")
//! def _(rid, params: dict) -> dict:
//!     word = params.get("word", "")
//!     if not word: return _ok(rid, {"items": []})
//!     # ... profile mentions, @diff/@staged/@file:/@folder:/@url:/@git:, plugin
//!     # context references, fuzzy basename search, directory listing, slash fallback
//!     try: root = _completion_cwd(params); is_context = word.startswith("@"); query = word[1:] if is_context else word
//!          ... # see file for full 200+ line @ handling + fuzzy + dir list
//!     except Exception as e: return _err(rid, 5021, str(e))
//!     return _ok(rid, {"items": items})
//!
//! @method("complete.slash")
//! def _(rid, params: dict) -> dict:
//!     text = params.get("text", "")
//!     if not text.startswith("/"): return _ok(rid, {"items": []})
//!     try: completer = SlashCommandCompleter(...); items = [... for c in completer.get_completions(Document(text,len(text)), None)]
//!          if text.rsplit(" ", 1)[-1].startswith("/"):  # rank + fuzzy + extras
//!          return _ok(rid, {"items": items, "replace_from": ...})
//!     except Exception as e: return _err(rid, 5020, str(e))
//!
//! @method("model.options")
//! def _(rid, params: dict) -> dict:
//!     try: payload = build_model_options_payload(ctx, explicit_only=..., include_unconfigured=..., refresh=...)
//!          return _ok(rid, payload)
//!     except Exception as e: return _err(rid, 5033, str(e))
//!
//! @method("model.save_key")
//! def _(rid, params: dict) -> dict:
//!     slug = (params.get("slug") or "").strip(); api_key = (params.get("api_key") or "").strip()
//!     if not slug or not api_key: return _err(rid, 4001, "slug and api_key are required")
//!     if is_managed(): return _err(rid, 4006, "managed install — credentials are read-only")
//!     pconfig = PROVIDER_REGISTRY.get(slug)
//!     if not pconfig: return _err(rid, 4002, f"unknown provider: {slug}")
//!     if pconfig.auth_type != "api_key": return _err(rid, 4003, f"{pconfig.name} uses {pconfig.auth_type} auth — run `hermes model` to configure")
//!     if not pconfig.api_key_env_vars: return _err(rid, 4004, f"no env var defined for {pconfig.name}")
//!     save_provider_env_credential(env_var, api_key); os.environ[env_var]=api_key
//!     payload = build_models_payload(ctx, picker_hints=True, max_models=50)
//!     provider_data = next((p for p in payload["providers"] if p["slug"]==slug), None) or {...}
//!     provider_data["authenticated"]=True
//!     return _ok(rid, {"provider": provider_data})
//!     except Exception as e: return _err(rid, 5034, str(e))
//!
//! @method("model.disconnect")
//! def _(rid, params: dict) -> dict:
//!     slug = (params.get("slug") or "").strip()
//!     if not slug: return _err(rid, 4001, "slug is required")
//!     # remove env + clear_provider_auth; if neither cleared: 4005
//!     return _ok(rid, {"slug":slug,"name":..., "disconnected": True})
//!     except Exception as e: return _err(rid, 5035, str(e))
//!
//! def register(server) -> None: _registry.install(server)
//! ```
//!
//! # Rust mapping
//! * `HandlerRegistry` → [`crate::method_ctx::HandlerRegistry`] (same deferred
//!   `@method` + `install` shape; see `method_ctx.rs` for `FunctionType` rebinding
//!   no-op notes).
//! * `_paste_counter` / `_hermes_home` / `paste_dir` / `write_text` → injected
//!   `save: Fn(&str) -> Result<(placeholder, path, lines), String>` so the port
//!   stays `std`-only and the line-count / placeholder shape is testable via
//!   [`count_lines`] + [`paste_placeholder`]. Empty `text` → `4004` is handled
//!   in Rust before the closure is called.
//! * `_completion_cwd` / `_list_repo_files` / `_fuzzy_basename_rank` /
//!   `_normalize_completion_path` / `_abs_completion_prefix_exists` / `list_profiles`
//!   / `get_context_reference_providers` → injected `complete_path_op: Fn(&str)->Result<String,String>`
//!   (params_json → `{"items":[...]} ` fragment). `word` empty → `items:[]`
//!   early-return and `5021` envelope mapping are handled in Rust; the heavy
//!   `@`/fuzzy/dir-list/profile/plugin branches live in the injected backend.
//! * `SlashCommandCompleter` / `skill_commands` / `skill_bundles` /
//!   `fuzzy_rank_slash_items` / `_rank_slash_completions` / `_details_completions`
//!   → injected `complete_slash_op: Fn(&str)->Result<String,String>`.
//!   `!startswith("/")` → `items:[]` and `5020` mapping stay in Rust.
//! * `build_model_options_payload` / `build_models_payload` /
//!   `PROVIDER_REGISTRY` / `is_managed` / `save_provider_env_credential` /
//!   `remove_provider_env_credential` / `clear_provider_auth` → injected
//!   `Fn(...) -> Result<String,String>` / `Fn()->bool` style closures.
//!   Validation that is pure string trimming (`slug`/`api_key` present, `slug`
//!   present for disconnect) plus error-code mapping (`4001`/`4006`/`4005`)
//!   is preserved in Rust; provider-specific `4002`/`4003`/`4004` branches are
//!   delegated to the closure via `Err((code,msg))`.
//! * `_ok(rid, result)` / `_err(rid, code, msg)` → [`ok_response`] /
//!   [`err_response`] (mirrors `server.py::_ok` / `_err`).
//! * `@method("...")` + `register(server)` → [`register`] / [`register_with`] /
//!   [`build_registry`] / [`build_registry_default`] (deferred via
//!   `HandlerRegistry::method` + `install`/`install_into`).

use crate::method_ctx::HandlerRegistry;

// ---------------------------------------------------------------------------
// Method names — mirrors @method("...") decorators
// ---------------------------------------------------------------------------

pub const METHOD_PASTE_COLLAPSE: &str = "paste.collapse";
pub const METHOD_COMPLETE_PATH: &str = "complete.path";
pub const METHOD_COMPLETE_SLASH: &str = "complete.slash";
pub const METHOD_MODEL_OPTIONS: &str = "model.options";
pub const METHOD_MODEL_SAVE_KEY: &str = "model.save_key";
pub const METHOD_MODEL_DISCONNECT: &str = "model.disconnect";

// ---------------------------------------------------------------------------
// Error codes — mirrors _err(rid, N, ...)
// ---------------------------------------------------------------------------

pub const ERR_EMPTY_PASTE: i32 = 4004;
pub const ERR_SAVE_KEY_MISSING: i32 = 4001;
pub const ERR_DISCONNECT_MISSING: i32 = 4001;
pub const ERR_UNKNOWN_PROVIDER: i32 = 4002;
pub const ERR_AUTH_TYPE_MISMATCH: i32 = 4003;
pub const ERR_NO_ENV_VAR: i32 = 4004;
pub const ERR_NO_CREDENTIALS: i32 = 4005;
pub const ERR_MANAGED_READONLY: i32 = 4006;
pub const ERR_COMPLETE_SLASH: i32 = 5020;
pub const ERR_COMPLETE_PATH: i32 = 5021;
pub const ERR_MODEL_OPTIONS: i32 = 5033;
pub const ERR_MODEL_SAVE_KEY: i32 = 5034;
pub const ERR_MODEL_DISCONNECT: i32 = 5035;

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
    let rest = after[colon + 1..].trim_start();
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

// ---------------------------------------------------------------------------
// Paste helpers — mirrors paste.collapse block
// ---------------------------------------------------------------------------

/// Count lines as Python `text.count("\n") + 1` (no trimming).
pub fn count_lines(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }
    text.chars().filter(|&c| c == '\n').count() + 1
}

/// Build placeholder string.
/// Mirrors `f"[Pasted text #{counter}: {line_count} lines \u2192 {paste_file}]"`.
pub fn paste_placeholder(counter: u64, line_count: usize, path: &str) -> String {
    format!("[Pasted text #{}: {} lines \u2192 {}]", counter, line_count, path)
}

// ---------------------------------------------------------------------------
// Complete helpers — minimal pure bits for tests/docs
// ---------------------------------------------------------------------------

/// True when `word` is an `@` context query.
pub fn is_context_word(word: &str) -> bool {
    word.starts_with('@')
}

// ---------------------------------------------------------------------------
// Core handlers — mirrors each @method body, injected for std-only testing
// ---------------------------------------------------------------------------

/// Handle `paste.collapse`.
///
/// `save` mirrors the counter + `paste_dir.mkdir` + `write_text` + placeholder
/// build. It receives `text` and returns `Ok((placeholder, path, lines))` or
/// `Err(msg)` (maps to `5000`).
pub fn handle_paste_collapse<F>(rid_json: &str, params_json: &str, save: F) -> String
where
    F: Fn(&str) -> Result<(String, String, usize), String>,
{
    let text = extract_string_field(params_json, "text").unwrap_or_default();
    if text.is_empty() {
        return err_response(rid_json, ERR_EMPTY_PASTE, "empty paste");
    }
    match save(&text) {
        Ok((placeholder, path, lines)) => {
            let result = format!(
                r#"{{"placeholder":"{}","path":"{}","lines":{}}}"#,
                json_escape(&placeholder),
                json_escape(&path),
                lines
            );
            ok_response(rid_json, &result)
        }
        Err(e) => err_response(rid_json, 5000, &e),
    }
}

/// Handle `complete.path`.
///
/// `op` mirrors the entire `try:` block after the empty-word early return.
/// `Ok(payload)` is the `{"items":[...]}` fragment (already JSON); `Err(e)`
/// maps to `5021`. The empty `word` → `items:[]` early return is preserved
/// in Rust without calling `op`.
pub fn handle_complete_path<F>(rid_json: &str, params_json: &str, op: F) -> String
where
    F: Fn(&str) -> Result<String, String>,
{
    let word = extract_string_field(params_json, "word").unwrap_or_default();
    if word.is_empty() {
        return ok_response(rid_json, r#"{"items":[]}"#);
    }
    match op(params_json) {
        Ok(payload) => {
            let t = payload.trim();
            // Accept either raw fragment or full object; ensure result is object with items
            if t.starts_with('{') {
                ok_response(rid_json, t)
            } else {
                ok_response(rid_json, &format!(r#"{{"items":{}}}"#, t))
            }
        }
        Err(e) => err_response(rid_json, ERR_COMPLETE_PATH, &e),
    }
}

/// Handle `complete.slash`.
///
/// `op` mirrors the completer + ranking + extras block. Empty / non-`/` input
/// short-circuits to `items:[]`; exceptions map to `5020`.
pub fn handle_complete_slash<F>(rid_json: &str, params_json: &str, op: F) -> String
where
    F: Fn(&str) -> Result<String, String>,
{
    let text = extract_string_field(params_json, "text").unwrap_or_default();
    if !text.starts_with('/') {
        return ok_response(rid_json, r#"{"items":[]}"#);
    }
    match op(params_json) {
        Ok(payload) => {
            let t = payload.trim();
            ok_response(rid_json, t)
        }
        Err(e) => err_response(rid_json, ERR_COMPLETE_SLASH, &e),
    }
}

/// Handle `model.options`.
///
/// `op` mirrors `build_model_options_payload` returning the full result payload
/// JSON (e.g. `{"providers":[...]}` ). `Err(e)` → `5033`.
pub fn handle_model_options<F>(rid_json: &str, params_json: &str, op: F) -> String
where
    F: Fn(&str) -> Result<String, String>,
{
    match op(params_json) {
        Ok(payload) => ok_response(rid_json, payload.trim()),
        Err(e) => err_response(rid_json, ERR_MODEL_OPTIONS, &e),
    }
}

/// Handle `model.save_key`.
///
/// Validation `slug`+`api_key` present (`4001`) and managed guard (`4006`) are
/// handled in Rust; the remaining provider lookup + save + inventory build is
/// delegated to `op(slug, api_key)` where `Ok(provider_json)` is
/// `{"provider":{...}}` and `Err((code,msg))` carries the specific `4002`/
/// `4003`/`4004`/`5034` error.
pub fn handle_model_save_key<M, O>(rid_json: &str, params_json: &str, is_managed: M, op: O) -> String
where
    M: Fn() -> bool,
    O: Fn(&str, &str, &str) -> Result<String, (i32, String)>,
{
    let slug = extract_string_field(params_json, "slug")
        .unwrap_or_default()
        .trim()
        .to_string();
    let api_key = extract_string_field(params_json, "api_key")
        .unwrap_or_default()
        .trim()
        .to_string();
    if slug.is_empty() || api_key.is_empty() {
        return err_response(rid_json, ERR_SAVE_KEY_MISSING, "slug and api_key are required");
    }
    if is_managed() {
        return err_response(rid_json, ERR_MANAGED_READONLY, "managed install — credentials are read-only");
    }
    let session_id = extract_string_field(params_json, "session_id").unwrap_or_default();
    match op(&slug, &api_key, &session_id) {
        Ok(payload) => ok_response(rid_json, payload.trim()),
        Err((code, msg)) => err_response(rid_json, code, &msg),
    }
}

/// Handle `model.disconnect`.
///
/// Validates `slug` present (`4001`), then delegates `op(slug, session_id)`.
/// `Ok` is `{"slug":...,"name":...,"disconnected":true}`; `Err((code,msg))`
/// carries `4005` (no credentials) or `5035` (exception).
pub fn handle_model_disconnect<O>(rid_json: &str, params_json: &str, op: O) -> String
where
    O: Fn(&str, &str) -> Result<String, (i32, String)>,
{
    let slug = extract_string_field(params_json, "slug")
        .unwrap_or_default()
        .trim()
        .to_string();
    if slug.is_empty() {
        return err_response(rid_json, ERR_DISCONNECT_MISSING, "slug is required");
    }
    let session_id = extract_string_field(params_json, "session_id").unwrap_or_default();
    match op(&slug, &session_id) {
        Ok(payload) => ok_response(rid_json, payload.trim()),
        Err((code, msg)) => err_response(rid_json, code, &msg),
    }
}

// ---------------------------------------------------------------------------
// Registry wiring — mirrors _registry = HandlerRegistry() + register(server)
// ---------------------------------------------------------------------------

/// Build a fresh [`HandlerRegistry`] with all six methods registered
/// using the provided deps (for tests / production injection).
///
/// Each closure is `'static` and mirrors the lazy imports inside Python
/// handler bodies. For the default stub (no backend) use [`build_registry_default`].
pub fn build_registry<PA, CP, CS, MO, SK, DC>(
    paste_save: PA,
    complete_path: CP,
    complete_slash: CS,
    model_options: MO,
    model_save_key: SK,
    model_disconnect: DC,
) -> HandlerRegistry
where
    PA: Fn(String, String) -> String + Send + Sync + 'static,
    CP: Fn(String, String) -> String + Send + Sync + 'static,
    CS: Fn(String, String) -> String + Send + Sync + 'static,
    MO: Fn(String, String) -> String + Send + Sync + 'static,
    SK: Fn(String, String) -> String + Send + Sync + 'static,
    DC: Fn(String, String) -> String + Send + Sync + 'static,
{
    let mut reg = HandlerRegistry::new();
    register_with(
        &mut reg,
        paste_save,
        complete_path,
        complete_slash,
        model_options,
        model_save_key,
        model_disconnect,
    );
    reg
}

/// Build a registry with default stubs (no backend / no file I/O).
pub fn build_registry_default() -> HandlerRegistry {
    build_registry(
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_paste_collapse(&rid_json, &params_json, |_| Err("no backend".to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_complete_path(&rid_json, &params_json, |_| Err("no backend".to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_complete_slash(&rid_json, &params_json, |_| Err("no backend".to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_model_options(&rid_json, &params_json, |_| Err("no backend".to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_model_save_key(&rid_json, &params_json, || false, |slug, _key, _sid| {
                Err((ERR_UNKNOWN_PROVIDER, format!("unknown provider: {}", slug)))
            })
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_model_disconnect(&rid_json, &params_json, |slug, _sid| {
                Err((ERR_NO_CREDENTIALS, format!("no credentials found for {}", slug)))
            })
        },
    )
}

/// Register all six methods onto an existing registry.
pub fn register_with<PA, CP, CS, MO, SK, DC>(
    registry: &mut HandlerRegistry,
    paste_save: PA,
    complete_path: CP,
    complete_slash: CS,
    model_options: MO,
    model_save_key: SK,
    model_disconnect: DC,
) where
    PA: Fn(String, String) -> String + Send + Sync + 'static,
    CP: Fn(String, String) -> String + Send + Sync + 'static,
    CS: Fn(String, String) -> String + Send + Sync + 'static,
    MO: Fn(String, String) -> String + Send + Sync + 'static,
    SK: Fn(String, String) -> String + Send + Sync + 'static,
    DC: Fn(String, String) -> String + Send + Sync + 'static,
{
    registry.method(METHOD_PASTE_COLLAPSE, paste_save);
    registry.method(METHOD_COMPLETE_PATH, complete_path);
    registry.method(METHOD_COMPLETE_SLASH, complete_slash);
    registry.method(METHOD_MODEL_OPTIONS, model_options);
    registry.method(METHOD_MODEL_SAVE_KEY, model_save_key);
    registry.method(METHOD_MODEL_DISCONNECT, model_disconnect);
}

/// Register with default stubs onto `registry`.
pub fn register(registry: &mut HandlerRegistry) {
    register_with(
        registry,
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_paste_collapse(&rid_json, &params_json, |_| Err("no backend".to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_complete_path(&rid_json, &params_json, |_| Err("no backend".to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_complete_slash(&rid_json, &params_json, |_| Err("no backend".to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_model_options(&rid_json, &params_json, |_| Err("no backend".to_string()))
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_model_save_key(&rid_json, &params_json, || false, |slug, _key, _sid| {
                Err((ERR_UNKNOWN_PROVIDER, format!("unknown provider: {}", slug)))
            })
        },
        |rid, params_json| {
            let rid_json = encode_rid(&rid);
            handle_model_disconnect(&rid_json, &params_json, |slug, _sid| {
                Err((ERR_NO_CREDENTIALS, format!("no credentials found for {}", slug)))
            })
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
    fn count_lines_and_placeholder() {
        assert_eq!(count_lines("a\nb\nc"), 3);
        assert_eq!(count_lines("single"), 1);
        assert_eq!(count_lines(""), 0);
        assert_eq!(
            paste_placeholder(2, 3, "/tmp/pastes/paste_2_123.txt"),
            "[Pasted text #2: 3 lines \u{2192} /tmp/pastes/paste_2_123.txt]"
        );
    }

    #[test]
    fn paste_empty_and_ok() {
        let rid = rid1();
        let out = handle_paste_collapse(&rid, r#"{"text":""}"#, |_| Ok(("ph".into(), "/p".into(), 1)));
        assert!(out.contains(r#""code":4004"#), "{}", out);
        let out2 = handle_paste_collapse(&rid, r#"{"text":"hi\nthere"}"#, |t| {
            assert_eq!(t, "hi\nthere");
            let lines = count_lines(t);
            let ph = paste_placeholder(1, lines, "/tmp/pastes/paste_1_010101.txt");
            Ok((ph.clone(), "/tmp/pastes/paste_1_010101.txt".into(), lines))
        });
        assert!(out2.contains("Pasted text #1"), "{}", out2);
        assert!(out2.contains(r#""lines":2"#), "{}", out2);
        let out3 = handle_paste_collapse(&rid, r#"{"text":"x"}"#, |_| Err("disk full".into()));
        assert!(out3.contains(r#""code":5000"#), "{}", out3);
    }

    #[test]
    fn complete_path_empty_and_err() {
        let rid = rid1();
        let out = handle_complete_path(&rid, r#"{"word":""}"#, |_| panic!("should not call"));
        assert!(out.contains(r#""items":[]"#), "{}", out);
        let out2 = handle_complete_path(&rid, r#"{"word":"@foo"}"#, |_| Ok(r#"{"items":[{"text":"@file:foo","display":"foo","meta":""}]}"#.into()));
        assert!(out2.contains("@file:foo"), "{}", out2);
        let out3 = handle_complete_path(&rid, r#"{"word":"a"}"#, |_| Err("boom".into()));
        assert!(out3.contains(r#""code":5021"#), "{}", out3);
    }

    #[test]
    fn complete_slash_prefix_and_err() {
        let rid = rid1();
        let out = handle_complete_slash(&rid, r#"{"text":"hello"}"#, |_| panic!("should not call"));
        assert!(out.contains(r#""items":[]"#), "{}", out);
        let out2 = handle_complete_slash(&rid, r#"{"text":""}"#, |_| panic!("empty not slash"));
        assert!(out2.contains(r#""items":[]"#), "{}", out2);
        let out3 = handle_complete_slash(&rid, r#"{"text":"/mod"}"#, |_| Ok(r#"{"items":[{"text":"/model","display":"/model","meta":""}]}"#.into()));
        assert!(out3.contains("/model"), "{}", out3);
        let out4 = handle_complete_slash(&rid, r#"{"text":"/x"}"#, |_| Err("fail".into()));
        assert!(out4.contains(r#""code":5020"#), "{}", out4);
    }

    #[test]
    fn model_options_ok_and_err() {
        let rid = rid1();
        let out = handle_model_options(&rid, "{}", |_| Ok(r#"{"providers":[]}"#.into()));
        assert!(out.contains("providers"), "{}", out);
        let out2 = handle_model_options(&rid, "{}", |_| Err("boom".into()));
        assert!(out2.contains(r#""code":5033"#), "{}", out2);
    }

    #[test]
    fn save_key_missing_and_managed() {
        let rid = rid1();
        let out = handle_model_save_key(&rid, r#"{"slug":"","api_key":""}"#, || false, |_,_,_| Ok(r#"{"provider":{"slug":"x"}}"#.into()));
        assert!(out.contains(r#""code":4001"#), "{}", out);
        let out2 = handle_model_save_key(&rid, r#"{"slug":"openai","api_key":"sk-1"}"#, || true, |_,_,_| Ok(r#"{"provider":{}}"#.into()));
        assert!(out2.contains(r#""code":4006"#), "{}", out2);
    }

    #[test]
    fn save_key_provider_errors() {
        let rid = rid1();
        // unknown provider 4002
        let out = handle_model_save_key(&rid, r#"{"slug":"bad","api_key":"k"}"#, || false, |slug,_,_| Err((ERR_UNKNOWN_PROVIDER, format!("unknown provider: {}", slug))));
        assert!(out.contains(r#""code":4002"#), "{}", out);
        // auth type 4003
        let out2 = handle_model_save_key(&rid, r#"{"slug":"azure","api_key":"k"}"#, || false, |_,_,_| Err((ERR_AUTH_TYPE_MISMATCH, "azure uses oauth auth — run `hermes model` to configure".into())));
        assert!(out2.contains(r#""code":4003"#), "{}", out2);
        // no env var 4004
        let out3 = handle_model_save_key(&rid, r#"{"slug":"x","api_key":"k"}"#, || false, |_,_,_| Err((ERR_NO_ENV_VAR, "no env var defined for X".into())));
        assert!(out3.contains(r#""code":4004"#), "{}", out3);
        // exception 5034
        let out4 = handle_model_save_key(&rid, r#"{"slug":"x","api_key":"k"}"#, || false, |_,_,_| Err((ERR_MODEL_SAVE_KEY, "boom".into())));
        assert!(out4.contains(r#""code":5034"#), "{}", out4);
        // success
        let out5 = handle_model_save_key(&rid, r#"{"slug":"openai","api_key":"sk-123","session_id":"s1"}"#, || false, |slug,key,sid| {
            assert_eq!(slug, "openai");
            assert_eq!(key, "sk-123");
            assert_eq!(sid, "s1");
            Ok(r#"{"provider":{"slug":"openai","authenticated":true}}"#.into())
        });
        assert!(out5.contains(r#""authenticated":true"#), "{}", out5);
    }

    #[test]
    fn disconnect_missing_and_no_creds() {
        let rid = rid1();
        let out = handle_model_disconnect(&rid, r#"{"slug":""}"#, |_,_| Ok(r#"{"disconnected":true}"#.into()));
        assert!(out.contains(r#""code":4001"#), "{}", out);
        let out2 = handle_model_disconnect(&rid, r#"{"slug":"deepseek"}"#, |slug,_| Err((ERR_NO_CREDENTIALS, format!("no credentials found for {}", slug))));
        assert!(out2.contains(r#""code":4005"#), "{}", out2);
        let out3 = handle_model_disconnect(&rid, r#"{"slug":"x"}"#, |_,_| Err((ERR_MODEL_DISCONNECT, "boom".into())));
        assert!(out3.contains(r#""code":5035"#), "{}", out3);
        let out4 = handle_model_disconnect(&rid, r#"{"slug":"openai","session_id":""}"#, |slug,sid| {
            assert_eq!(slug, "openai");
            assert_eq!(sid, "");
            Ok(r#"{"slug":"openai","name":"OpenAI","disconnected":true}"#.into())
        });
        assert!(out4.contains(r#""disconnected":true"#), "{}", out4);
    }

    #[test]
    fn build_registry_installs() {
        let mut reg = build_registry_default();
        assert_eq!(reg.len(), 6);
        let names: Vec<_> = reg.pending_names().collect();
        assert!(names.contains(&"paste.collapse"));
        assert!(names.contains(&"complete.path"));
        assert!(names.contains(&"complete.slash"));
        assert!(names.contains(&"model.options"));
        assert!(names.contains(&"model.save_key"));
        assert!(names.contains(&"model.disconnect"));
        let mut map = std::collections::HashMap::new();
        reg.install_into(&mut map);
        assert!(map.contains_key("paste.collapse"));
        let out = map.get("paste.collapse").unwrap()("1".to_string(), r#"{"text":""}"#.to_string());
        assert!(out.contains(r#""code":4004"#));
    }
}
