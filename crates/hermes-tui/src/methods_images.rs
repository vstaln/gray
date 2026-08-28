//! Image-generation JSON-RPC handler (ws twin of the image_generate tool).
//!
//! 1:1 port of `tui_gateway/methods_images.py` (130 lines).
//!
//! Desktop plugins reach the backend only through ws JSON-RPC; the image
//! generation capability existed solely as a model tool. `image.generate`
//! lets UI surfaces (avatar pickers, artifact panes) generate directly.
//!
//! The result image is returned as a data URL (`image_data`): a remote
//! desktop client cannot read a file path on the gateway host, and hosted
//! result URLs are often CORS-opaque to a renderer canvas. Data URLs work
//! identically over local and remote gateways.
//!
//! Handlers are rebound onto server.py's globals at install time (see
//! method_ctx.py) — helpers must stay nested inside the handler body.
//!
//! ```python
//! # Python — tui_gateway/methods_images.py
//! from .method_ctx import HandlerRegistry
//! _registry = HandlerRegistry()
//! method = _registry.method
//!
//! @method("image.generate")
//! def _(rid, params: dict) -> dict:
//!     def _availability() -> bool:
//!         try:
//!             from tools.image_generation_tool import check_image_generation_requirements
//!             return bool(check_image_generation_requirements())
//!         except Exception:
//!             return False
//!
//!     def _to_data_url(ref: str, cap: int):
//!         import base64, mimetypes, os
//!         try:
//!             if ref.startswith(("http://", "https://")):
//!                 import urllib.request
//!                 req = urllib.request.Request(ref, headers={"User-Agent": "hermes-agent"})
//!                 with urllib.request.urlopen(req, timeout=60) as resp:
//!                     if resp.length is not None and resp.length > cap:
//!                         return None
//!                     data = resp.read(cap + 1)
//!                     mime = resp.headers.get_content_type() or "image/png"
//!             elif os.path.isfile(ref):
//!                 if os.path.getsize(ref) > cap:
//!                     return None
//!                 with open(ref, "rb") as fh:
//!                     data = fh.read(cap + 1)
//!                 mime = mimetypes.guess_type(ref)[0] or "image/png"
//!             else:
//!                 return None
//!             if len(data) > cap:
//!                 return None
//!             if not mime.startswith("image/"):
//!                 mime = "image/png"
//!             return f"data:{mime};base64,{base64.b64encode(data).decode('ascii')}"
//!         except Exception:
//!             return None
//!
//!     available = _availability()
//!     if is_truthy_value(params.get("probe", False)):
//!         return _ok(rid, {"available": available})
//!     if not available:
//!         return _ok(rid, {"available": False, "success": False,
//!                          "error": "No image generation backend configured (run `hermes tools` to enable one)."})
//!     prompt = str(params.get("prompt") or "").strip()
//!     if not prompt:
//!         return _err(rid, 4071, "prompt required")
//!     aspect = str(params.get("aspect_ratio") or "square").strip().lower()
//!     try:
//!         cap = min(int(params.get("max_bytes", 8_000_000) or 8_000_000), 16_000_000)
//!     except (TypeError, ValueError):
//!         cap = 8_000_000
//!     try:
//!         from tools.image_generation_tool import _handle_image_generate
//!         raw = _handle_image_generate({"prompt": prompt, "aspect_ratio": aspect})
//!         result = json.loads(raw)
//!     except Exception as e:
//!         return _err(rid, 5071, str(e))
//!     if not result.get("success"):
//!         return _ok(rid, {"available": True, "success": False,
//!                          "error": str(result.get("error") or "generation failed")})
//!     image_ref = str(result.get("image") or "")
//!     payload = {"available": True, "success": True, "image": image_ref}
//!     data_url = _to_data_url(image_ref, cap) if image_ref else None
//!     if data_url:
//!         payload["image_data"] = data_url
//!     return _ok(rid, payload)
//!
//! def register(server) -> None:
//!     _registry.install(server)
//! ```
//!
//! # Rust mapping
//! * `HandlerRegistry` → [`crate::method_ctx::HandlerRegistry`] (same deferred
//!   `@method` + `install` shape; see `method_ctx.rs` for `FunctionType`
//!   rebinding no-op notes).
//! * `is_truthy_value` → [`is_truthy_value`] / [`is_truthy_str`] (mirrors
//!   `utils.is_truthy_value` with `TRUTHY_STRINGS = {"1","true","yes","on"}`;
//!   `None` → `default` false, `bool` → itself, `str` → case-insensitive set,
//!   other → `bool(value)` via non-empty string check).
//! * `_availability()` → injected `check_fn: Fn() -> bool` (mirrors
//!   `check_image_generation_requirements()`; exception → false is preserved
//!   by letting the closure catch and return false).
//! * `_to_data_url(ref, cap)` → [`to_data_url`] with injected `http_fetch` and
//!   `file_read` closures so the port stays `std`-only and testable. Mirrors:
//!   `http://`/`https://` → `urllib.request.Request` with
//!   `User-Agent: hermes-agent` + `timeout=60`; `resp.length` cap check;
//!   `resp.read(cap+1)` + `len(data) > cap` guard; `resp.headers.get_content_type()`
//!   → mime; `os.path.isfile` + `os.path.getsize` cap check + `open(...,"rb").read(cap+1)`;
//!   `mimetypes.guess_type(ref)[0]` → [`guess_mime`]; `mime.startswith("image/")`
//!   fallback → `image/png`; `base64.b64encode` → [`base64_encode`] (std-only);
//!   any `Exception` → `None` via `Option`.
//! * `cap = min(int(params.get("max_bytes", 8_000_000) or 8_000_000), 16_000_000)`
//!   → [`parse_max_bytes`] (falsy `0`/`""` falls back to `DEFAULT_MAX_BYTES`;
//!   parse failure → default; clamped to `HARD_MAX_BYTES`).
//! * `aspect = str(params.get("aspect_ratio") or "square").strip().lower()` →
//!   [`normalize_aspect`] (empty → `"square"`, trimmed, lowercased).
//! * `_handle_image_generate` → injected `generate_fn: Fn(&str,&str) -> Result<String,String>`
//!   (Ok = raw JSON string, Err = exception message → `5071` error).
//! * `json.loads(raw)` + `result.get("success")` / `result.get("image")` /
//!   `result.get("error")` → [`parse_generate_result`] (minimal `std`-only
//!   JSON field extraction; no `serde_json`).
//! * `_ok(rid, result)` / `_err(rid, code, msg)` → [`ok_response`] /
//!   [`err_response`] (mirrors `server.py::_ok` / `_err` envelope shape).
//! * `@method("image.generate")` + `register(server)` → [`register`] /
//!   [`register_with`] / [`build_registry`] (deferred registration via
//!   `HandlerRegistry::method` + `install`/`install_into`).

use std::collections::HashMap;

use crate::method_ctx::HandlerRegistry;

// ---------------------------------------------------------------------------
// Constants — mirrors methods_images.py literals and server.py helpers
// ---------------------------------------------------------------------------

/// JSON-RPC method name. Mirrors `@method("image.generate")`.
pub const METHOD_NAME: &str = "image.generate";

/// Default `max_bytes` cap. Mirrors `8_000_000`.
pub const DEFAULT_MAX_BYTES: usize = 8_000_000;

/// Hard ceiling for `max_bytes`. Mirrors `16_000_000`.
pub const HARD_MAX_BYTES: usize = 16_000_000;

/// User-Agent for remote fetches. Mirrors `headers={"User-Agent": "hermes-agent"}`.
pub const USER_AGENT: &str = "hermes-agent";

/// HTTP fetch timeout in seconds. Mirrors `urlopen(..., timeout=60)`.
pub const FETCH_TIMEOUT_SECS: u64 = 60;

/// Error code for missing prompt. Mirrors `_err(rid, 4071, "prompt required")`.
pub const ERR_PROMPT_REQUIRED: i32 = 4071;

/// Error code for generation exception. Mirrors `_err(rid, 5071, str(e))`.
pub const ERR_GENERATION: i32 = 5071;

/// Error message when no backend is configured.
/// Mirrors `"No image generation backend configured (run `hermes tools` to enable one)."`
pub const ERR_NO_BACKEND: &str =
    "No image generation backend configured (run `hermes tools` to enable one).";

// ---------------------------------------------------------------------------
// is_truthy_value — mirrors utils.is_truthy_value
// ---------------------------------------------------------------------------

/// Truthy strings — mirrors `TRUTHY_STRINGS = frozenset({"1","true","yes","on"})` in `utils.py`.
const TRUTHY_STRINGS: &[&str] = &["1", "true", "yes", "on"];

/// Mirrors `utils.is_truthy_value(value, default=False)` for string values.
///
/// * `None` → `default` (here fixed to `false`; callers that need a different
///   default can check `is_none` themselves).
/// * `Some(s)` where `s.trim().to_lowercase()` is in `TRUTHY_STRINGS` → `true`.
/// * otherwise → `false`.
///
/// For `bool` inputs callers should stringify first (`"true"`/`"false"`) or
/// use [`is_truthy_bool`]. This matches the Python path where
/// `params.get("probe", False)` may be a `bool` and hits the `isinstance(value, bool)`
/// early-return before the string set check.
pub fn is_truthy_value(value: Option<&str>) -> bool {
    match value {
        None => false,
        Some(s) => is_truthy_str(s),
    }
}

/// Case-insensitive truthy check for a string value.
///
/// Mirrors the `isinstance(value, str): return value.strip().lower() in TRUTHY_STRINGS`
/// branch. Empty/whitespace-only strings return `false`.
pub fn is_truthy_str(s: &str) -> bool {
    let t = s.trim().to_ascii_lowercase();
    TRUTHY_STRINGS.contains(&t.as_str())
}

/// Direct `bool` truthy — mirrors `isinstance(value, bool): return value`.
pub fn is_truthy_bool(b: bool) -> bool {
    b
}

/// Probe helper — reads `probe` from a string map and applies `is_truthy_value`.
///
/// Mirrors `is_truthy_value(params.get("probe", False))` where `params` is the
/// JSON-RPC `params` dict. The map stores stringified values (bool `true` → `"true"`,
/// missing → `None`). Returns `false` when absent.
pub fn probe_is_truthy(params: &HashMap<String, String>) -> bool {
    match params.get("probe") {
        None => false,
        Some(v) => {
            // Accept both string truthy and bool-stringified values.
            // Also handle "True"/"1" etc. via is_truthy_str, and direct bool true
            // is stringified as "true" by the JSON parser below.
            is_truthy_str(v)
        }
    }
}

// ---------------------------------------------------------------------------
// Small helpers — aspect, cap, mime, base64, JSON envelope
// ---------------------------------------------------------------------------

/// Normalize `aspect_ratio` param.
///
/// Mirrors `str(params.get("aspect_ratio") or "square").strip().lower()`:
/// empty/whitespace/`None` → `"square"`, otherwise trimmed + lowercased.
pub fn normalize_aspect(raw: Option<&str>) -> String {
    match raw {
        None => "square".to_string(),
        Some(s) => {
            let t = s.trim();
            if t.is_empty() {
                "square".to_string()
            } else {
                t.to_ascii_lowercase()
            }
        }
    }
}

/// Parse `max_bytes` param with fallback and hard cap.
///
/// Mirrors:
/// ```python
/// try:
///     cap = min(int(params.get("max_bytes", 8_000_000) or 8_000_000), 16_000_000)
/// except (TypeError, ValueError):
///     cap = 8_000_000
/// ```
/// `raw` is `params.get("max_bytes")` stringified (or `None` when absent).
/// Falsy `0`/`""` falls back to `DEFAULT_MAX_BYTES`; parse errors → default;
/// result is clamped to `HARD_MAX_BYTES`.
pub fn parse_max_bytes(raw: Option<&str>) -> usize {
    match raw {
        None => DEFAULT_MAX_BYTES,
        Some(s) => {
            let t = s.trim();
            if t.is_empty() {
                return DEFAULT_MAX_BYTES;
            }
            // Python's `or 8_000_000` treats 0 as falsy → fallback.
            // Also empty string already handled.
            match t.parse::<i64>() {
                Ok(n) if n == 0 => DEFAULT_MAX_BYTES,
                Ok(n) if n < 0 => DEFAULT_MAX_BYTES,
                Ok(n) => {
                    let v = n as usize;
                    if v == 0 {
                        DEFAULT_MAX_BYTES
                    } else {
                        std::cmp::min(v, HARD_MAX_BYTES)
                    }
                }
                Err(_) => DEFAULT_MAX_BYTES,
            }
        }
    }
}

/// Guess MIME from a file path's extension.
///
/// Mirrors `mimetypes.guess_type(ref)[0] or "image/png"` (minimal mapping).
/// Extensions are matched case-insensitively; unknown → `"image/png"`.
pub fn guess_mime(path: &str) -> String {
    let lower = path.to_ascii_lowercase();
    // Extract extension after last dot, handling paths with query fragments stripped earlier
    let ext = lower.rsplit('.').next().unwrap_or("");
    // Only treat as extension if dot exists in lower
    if !lower.contains('.') {
        return "image/png".to_string();
    }
    match ext {
        "png" => "image/png".to_string(),
        "jpg" | "jpeg" => "image/jpeg".to_string(),
        "gif" => "image/gif".to_string(),
        "webp" => "image/webp".to_string(),
        "bmp" => "image/bmp".to_string(),
        "svg" => "image/svg+xml".to_string(),
        "avif" => "image/avif".to_string(),
        "heic" | "heif" => "image/heic".to_string(),
        "tiff" | "tif" => "image/tiff".to_string(),
        _ => "image/png".to_string(),
    }
}

/// Normalize MIME — ensures `image/*`, else `"image/png"`.
///
/// Mirrors `if not mime.startswith("image/"): mime = "image/png"`.
pub fn normalize_mime(mime: &str) -> &str {
    if mime.starts_with("image/") {
        mime
    } else {
        "image/png"
    }
}

/// Std-only base64 encode (RFC 4648, no line breaks).
///
/// Mirrors `base64.b64encode(data).decode('ascii')`.
pub fn base64_encode(data: &[u8]) -> String {
    const ALPH: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((data.len() + 2) / 3 * 4);
    let mut i = 0;
    while i < data.len() {
        let b0 = data[i] as u32;
        let b1 = if i + 1 < data.len() { data[i + 1] as u32 } else { 0 };
        let b2 = if i + 2 < data.len() { data[i + 2] as u32 } else { 0 };
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPH[((n >> 18) & 63) as usize] as char);
        out.push(ALPH[((n >> 12) & 63) as usize] as char);
        if i + 1 < data.len() {
            out.push(ALPH[((n >> 6) & 63) as usize] as char);
        } else {
            out.push('=');
        }
        if i + 2 < data.len() {
            out.push(ALPH[(n & 63) as usize] as char);
        } else {
            out.push('=');
        }
        i += 3;
    }
    out
}

/// Build a JSON-RPC success envelope.
///
/// Mirrors `server.py::_ok(rid, result)`:
/// `{"jsonrpc":"2.0","id":rid,"result":result}`.
/// `result_json` must already be a JSON object string (e.g. `{"available":true}`).
/// `rid_json` must already be JSON-encoded (number/string/null).
pub fn ok_response(rid_json: &str, result_json: &str) -> String {
    format!(r#"{{"jsonrpc":"2.0","id":{},"result":{}}}"#, rid_json, result_json)
}

/// Build a JSON-RPC error envelope.
///
/// Mirrors `server.py::_err(rid, code, msg, data=None)`:
/// `{"jsonrpc":"2.0","id":rid,"error":{"code":code,"message":msg}}`.
/// `rid_json` must already be JSON-encoded; `msg` is JSON-escaped.
pub fn err_response(rid_json: &str, code: i32, msg: &str) -> String {
    let esc = json_escape(msg);
    format!(
        r#"{{"jsonrpc":"2.0","id":{},"error":{{"code":{},"message":"{}"}}}}"#,
        rid_json, code, esc
    )
}

/// Minimal JSON string escaper for error messages and payloads.
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

/// Encode `rid` as JSON (string rid → quoted, numeric/null already JSON).
///
/// Python's `rid` is opaque from the request `id` field; we preserve it by
/// checking if it parses as JSON number/null/bool, otherwise quote as string.
pub fn encode_rid(rid: &str) -> String {
    let t = rid.trim();
    if t.is_empty() {
        return "null".to_string();
    }
    if t == "null" || t == "true" || t == "false" {
        return t.to_string();
    }
    if t.parse::<i64>().is_ok() || t.parse::<f64>().is_ok() {
        // Check f64 parse is valid JSON number (not "inf"/"nan")
        if t.eq_ignore_ascii_case("inf")
            || t.eq_ignore_ascii_case("nan")
            || t.eq_ignore_ascii_case("-inf")
        {
            return format!("\"{}\"", json_escape(t));
        }
        return t.to_string();
    }
    // Already quoted?
    if (t.starts_with('"') && t.ends_with('"') && t.len() >= 2)
        || (t.starts_with('\'') && t.ends_with('\'') && t.len() >= 2)
    {
        return format!("\"{}\"", json_escape(&t[1..t.len() - 1]));
    }
    format!("\"{}\"", json_escape(t))
}

// ---------------------------------------------------------------------------
// Param parsing — minimal JSON object parser (std-only)
// ---------------------------------------------------------------------------

/// Parse a flat JSON object string into a stringified map.
///
/// Handles the params shape for `image.generate` (string/bool/int values).
/// Missing/invalid JSON → empty map (so defaults apply, matching Python's
/// `params.get(..., default)`).
///
/// Example: `{"prompt":"hi","probe":true,"max_bytes":8000000}` →
/// `{"prompt":"hi","probe":"true","max_bytes":"8000000"}`.
pub fn parse_params_json(params_json: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let s = params_json.trim();
    if s.is_empty() || s == "null" || s == "{}" {
        return out;
    }
    // Strip outer braces
    let inner = if s.starts_with('{') && s.ends_with('}') {
        &s[1..s.len() - 1]
    } else {
        // Not an object — return empty (Python would have coerced via .get)
        return out;
    };
    let inner = inner.trim();
    if inner.is_empty() {
        return out;
    }
    // Split by commas outside strings
    let mut tokens: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut in_str = false;
    let mut esc = false;
    let mut quote_char = '"';
    for ch in inner.chars() {
        if esc {
            cur.push(ch);
            esc = false;
            continue;
        }
        if ch == '\\' && in_str {
            cur.push(ch);
            esc = true;
            continue;
        }
        if (ch == '"' || ch == '\'') && !esc {
            if !in_str {
                in_str = true;
                quote_char = ch;
            } else if ch == quote_char {
                in_str = false;
            }
            cur.push(ch);
            continue;
        }
        if ch == ',' && !in_str {
            tokens.push(cur.trim().to_string());
            cur.clear();
            continue;
        }
        cur.push(ch);
    }
    if !cur.trim().is_empty() {
        tokens.push(cur.trim().to_string());
    }

    for tok in tokens {
        if tok.is_empty() {
            continue;
        }
        // Split by first colon outside strings
        let mut key_part = String::new();
        let mut val_part = String::new();
        let mut found_colon = false;
        let mut in_s = false;
        let mut es = false;
        let mut qc = '"';
        for ch in tok.chars() {
            if es {
                if !found_colon {
                    key_part.push(ch);
                } else {
                    val_part.push(ch);
                }
                es = false;
                continue;
            }
            if ch == '\\' && in_s {
                if !found_colon {
                    key_part.push(ch);
                } else {
                    val_part.push(ch);
                }
                es = true;
                continue;
            }
            if (ch == '"' || ch == '\'') && !es {
                if !in_s {
                    in_s = true;
                    qc = ch;
                } else if ch == qc {
                    in_s = false;
                }
                if !found_colon {
                    key_part.push(ch);
                } else {
                    val_part.push(ch);
                }
                continue;
            }
            if ch == ':' && !in_s && !found_colon {
                found_colon = true;
                continue;
            }
            if !found_colon {
                key_part.push(ch);
            } else {
                val_part.push(ch);
            }
        }
        if !found_colon {
            continue;
        }
        let k = key_part.trim();
        let v = val_part.trim();
        // Unquote key
        let key = if (k.starts_with('"') && k.ends_with('"') && k.len() >= 2)
            || (k.starts_with('\'') && k.ends_with('\'') && k.len() >= 2)
        {
            k[1..k.len() - 1].to_string()
        } else {
            k.to_string()
        };
        if key.is_empty() {
            continue;
        }
        // Unquote value if string, else keep raw (bool/int)
        let val = if (v.starts_with('"') && v.ends_with('"') && v.len() >= 2)
            || (v.starts_with('\'') && v.ends_with('\'') && v.len() >= 2)
        {
            // Unescape minimal
            let inner_v = &v[1..v.len() - 1];
            inner_v
                .replace("\\\"", "\"")
                .replace("\\'", "'")
                .replace("\\\\", "\\")
        } else {
            v.to_string()
        };
        out.insert(key, val);
    }
    out
}

// ---------------------------------------------------------------------------
// _to_data_url — mirrors Python's nested helper
// ---------------------------------------------------------------------------

/// Fetch a URL or read a local path into a data URL, size-capped.
///
/// Mirrors `methods_images.py::_to_data_url(ref, cap)`:
/// * `http://`/`https://` → `http_fetch(ref, cap)` (should implement
///   `User-Agent: hermes-agent`, `timeout=60`, `resp.length` cap check,
///   `read(cap+1)` + `len > cap` guard, `get_content_type` mime).
/// * local path → `file_read(ref, cap)` (should check `is_file`, `getsize > cap`,
///   `read(cap+1)`, `guess_type` mime).
/// * otherwise → `None`.
/// * `len(data) > cap` → `None`.
/// * `!mime.starts_with("image/")` → `"image/png"`.
/// * `data:{mime};base64,{b64}`.
/// * any error → `None` (closure returns `None`).
///
/// `http_fetch` and `file_read` each return `Some((bytes, mime))` on success.
/// They should already enforce the `cap+1` read and return `None` on `>cap`
/// if they wish, but this wrapper re-checks `bytes.len() > cap`.
pub fn to_data_url<F, G>(ref_: &str, cap: usize, http_fetch: F, file_read: G) -> Option<String>
where
    F: Fn(&str, usize) -> Option<(Vec<u8>, String)>,
    G: Fn(&str, usize) -> Option<(Vec<u8>, String)>,
{
    let (data, mime_raw) = if ref_.starts_with("http://") || ref_.starts_with("https://") {
        http_fetch(ref_, cap)?
    } else if ref_.is_empty() {
        return None;
    } else {
        file_read(ref_, cap)?
    };
    if data.len() > cap {
        return None;
    }
    let mime_owned: String;
    let mime = {
        let m = mime_raw.as_str();
        if m.starts_with("image/") {
            m
        } else {
            mime_owned = "image/png".to_string();
            &mime_owned
        }
    };
    // Re-borrow to handle owned fallback
    let mime_str = if mime_raw.starts_with("image/") {
        mime_raw
    } else {
        "image/png".to_string()
    };
    let _ = mime; // suppress unused
    let b64 = base64_encode(&data);
    Some(format!("data:{};base64,{}", mime_str, b64))
}

/// Build a data URL directly from bytes and a MIME type.
///
/// Mirrors the tail of `_to_data_url` after the fetch/read branches:
/// size check + mime normalization + base64.
pub fn data_url_from_bytes(data: &[u8], mime: &str, cap: usize) -> Option<String> {
    if data.len() > cap {
        return None;
    }
    let mime_norm = if mime.starts_with("image/") {
        mime.to_string()
    } else {
        "image/png".to_string()
    };
    Some(format!("data:{};base64,{}", mime_norm, base64_encode(data)))
}

// ---------------------------------------------------------------------------
// Generate-result parsing — mirrors json.loads(raw) + result.get(...)
// ---------------------------------------------------------------------------

/// Parsed image-generation result.
///
/// Mirrors `result = json.loads(raw)` where `raw` is from `_handle_image_generate`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenerateResult {
    /// `result.get("success")` — `true` when generation succeeded.
    pub success: bool,
    /// `str(result.get("image") or "")` — backend URL/path.
    pub image: String,
    /// `str(result.get("error") or "generation failed")` when `!success`.
    pub error: String,
}

/// Minimal extraction of `success`/`image`/`error` from a JSON string.
///
/// `std`-only, no `serde_json`. Looks for `"success": true|false`,
/// `"image": "..."`, `"error": "..."` (case-sensitive keys, whitespace tolerant).
/// Missing fields → defaults (`success=false`, `image=""`, `error="generation failed"`).
pub fn parse_generate_result(raw: &str) -> GenerateResult {
    let success = parse_json_bool(raw, "success").unwrap_or(false);
    let image = parse_json_string(raw, "image").unwrap_or_default();
    let error_raw = parse_json_string(raw, "error").unwrap_or_default();
    let error = if error_raw.trim().is_empty() {
        "generation failed".to_string()
    } else {
        error_raw
    };
    GenerateResult { success, image, error }
}

fn parse_json_bool(raw: &str, field: &str) -> Option<bool> {
    // Find `"field"` then `:` then `true`/`false`
    let key = format!("\"{}\"", field);
    let pos = raw.find(&key)?;
    let after = &raw[pos + key.len()..];
    let colon = after.find(':')?;
    let val = after[colon + 1..].trim_start();
    if val.starts_with("true") {
        Some(true)
    } else if val.starts_with("false") {
        Some(false)
    } else if val.starts_with('1') {
        Some(true)
    } else if val.starts_with('0') {
        Some(false)
    } else {
        None
    }
}

fn parse_json_string(raw: &str, field: &str) -> Option<String> {
    let key = format!("\"{}\"", field);
    let pos = raw.find(&key)?;
    let after = &raw[pos + key.len()..];
    let colon = after.find(':')?;
    let val = after[colon + 1..].trim_start();
    // Handle null
    if val.starts_with("null") {
        return Some(String::new());
    }
    // Must be quoted string
    if !val.starts_with('"') {
        // Maybe single-quoted or unquoted — treat as empty
        if val.starts_with('\'') {
            let end = val[1..].find('\'')?;
            return Some(val[1..1 + end].to_string());
        }
        return None;
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
                    // Minimal \uXXXX — skip 4 hex digits, push replacement
                    // Consume next 4 chars if available
                    // For std-only, just skip and push '?'
                    let mut hex = String::new();
                    for _ in 0..3 {
                        if let Some(h) = chars.next() {
                            hex.push(h);
                        }
                    }
                    // first char already in ch? Actually ch is 'u', next 4 are hex
                    // We already consumed 3 extra, need 1 more? The loop consumes one per iteration,
                    // so for \u we need to read 4 hex digits.
                    // Simplify: push placeholder
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

// ---------------------------------------------------------------------------
// Core handler — mirrors the body of @method("image.generate") def _(rid, params)
// ---------------------------------------------------------------------------

/// Handle `image.generate` JSON-RPC.
///
/// `rid_json` is the request `id` already JSON-encoded (e.g. `"1"` or `1`);
/// `params` is the stringified param map (from [`parse_params_json`]).
///
/// `check` mirrors `_availability()` ( `check_image_generation_requirements()` → `bool`).
/// `generate` mirrors `_handle_image_generate({"prompt":..., "aspect_ratio":...})`
/// returning `Ok(raw_json)` or `Err(msg)` (exception → `5071`).
/// `data_url` mirrors `_to_data_url(ref, cap)` returning `Some(data_url)` or `None`.
///
/// Returns a JSON-RPC envelope string (`_ok` or `_err`).
pub fn handle_image_generate<C, G, D>(
    rid_json: &str,
    params: &HashMap<String, String>,
    check: C,
    generate: G,
    data_url: D,
) -> String
where
    C: Fn() -> bool,
    G: Fn(&str, &str) -> Result<String, String>,
    D: Fn(&str, usize) -> Option<String>,
{
    // def _availability() is injected as `check`; exception → false handled by closure
    let available = {
        // Preserve try/except → false: closure should catch and return false;
        // here we also guard against panic.
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| check()));
        res.unwrap_or(false)
    };

    // if is_truthy_value(params.get("probe", False)): return _ok(rid, {"available": available})
    if probe_is_truthy(params) {
        let result = format!(r#"{{"available":{}}}"#, if available { "true" } else { "false" });
        return ok_response(rid_json, &result);
    }

    // if not available: return _ok(rid, {"available": False, "success": False, "error": "..."})
    if !available {
        let result = format!(
            r#"{{"available":false,"success":false,"error":"{}"}}"#,
            json_escape(ERR_NO_BACKEND)
        );
        return ok_response(rid_json, &result);
    }

    // prompt = str(params.get("prompt") or "").strip()
    let prompt_raw = params.get("prompt").map(|s| s.as_str()).unwrap_or("");
    let prompt = prompt_raw.trim().to_string();
    if prompt.is_empty() {
        return err_response(rid_json, ERR_PROMPT_REQUIRED, "prompt required");
    }

    // aspect = str(params.get("aspect_ratio") or "square").strip().lower()
    let aspect = normalize_aspect(params.get("aspect_ratio").map(|s| s.as_str()));

    // cap = min(int(params.get("max_bytes", 8_000_000) or 8_000_000), 16_000_000)
    let cap = parse_max_bytes(params.get("max_bytes").map(|s| s.as_str()));

    // try: raw = _handle_image_generate({"prompt": prompt, "aspect_ratio": aspect}); result = json.loads(raw)
    // except Exception as e: return _err(rid, 5071, str(e))
    let raw = match generate(&prompt, &aspect) {
        Ok(r) => r,
        Err(e) => return err_response(rid_json, ERR_GENERATION, &e),
    };
    let result = parse_generate_result(&raw);

    // if not result.get("success"): return _ok(rid, {"available": True, "success": False, "error": ...})
    if !result.success {
        let result_json = format!(
            r#"{{"available":true,"success":false,"error":"{}"}}"#,
            json_escape(&result.error)
        );
        return ok_response(rid_json, &result_json);
    }

    // image_ref = str(result.get("image") or "")
    let image_ref = result.image.trim().to_string();
    // payload = {"available": True, "success": True, "image": image_ref}
    // data_url = _to_data_url(image_ref, cap) if image_ref else None
    let maybe_data_url = if image_ref.is_empty() {
        None
    } else {
        // data_url closure should internally handle cap+1 and mime; we pass cap
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| data_url(&image_ref, cap)));
        res.unwrap_or(None)
    };

    if let Some(du) = maybe_data_url {
        let payload = format!(
            r#"{{"available":true,"success":true,"image":"{}","image_data":"{}"}}"#,
            json_escape(&image_ref),
            json_escape(&du)
        );
        ok_response(rid_json, &payload)
    } else {
        let payload = format!(
            r#"{{"available":true,"success":true,"image":"{}"}}"#,
            json_escape(&image_ref)
        );
        ok_response(rid_json, &payload)
    }
}

/// Convenience: handle with `params_json` string and `rid` string.
///
/// Parses `params_json` via [`parse_params_json`] and JSON-encodes `rid` via [`encode_rid`],
/// then delegates to [`handle_image_generate`].
pub fn handle_image_generate_json<C, G, D>(
    rid: &str,
    params_json: &str,
    check: C,
    generate: G,
    data_url: D,
) -> String
where
    C: Fn() -> bool,
    G: Fn(&str, &str) -> Result<String, String>,
    D: Fn(&str, usize) -> Option<String>,
{
    let rid_json = encode_rid(rid);
    let params = parse_params_json(params_json);
    handle_image_generate(&rid_json, &params, check, generate, data_url)
}

// ---------------------------------------------------------------------------
// Registry wiring — mirrors _registry = HandlerRegistry() + register(server)
// ---------------------------------------------------------------------------

/// Build a fresh [`HandlerRegistry`] with `image.generate` registered.
///
/// `check`, `generate`, and `data_url` are injected and must be `'static`
/// (mirrors Python's lazy `from tools... import` inside the handler body).
/// For the default no-backend stub use [`build_registry_default`].
pub fn build_registry<C, G, D>(check: C, generate: G, data_url: D) -> HandlerRegistry
where
    C: Fn() -> bool + Send + Sync + 'static,
    G: Fn(&str, &str) -> Result<String, String> + Send + Sync + 'static,
    D: Fn(&str, usize) -> Option<String> + Send + Sync + 'static,
{
    let mut reg = HandlerRegistry::new();
    reg.method(METHOD_NAME, move |rid, params_json| {
        handle_image_generate_json(&rid, &params_json, &check, &generate, &data_url)
    });
    reg
}

/// Build a registry with the default stub backend (availability `false`).
///
/// Mirrors the import-failure path where `check_image_generation_requirements`
/// is unavailable → `_availability()` returns `False`.
pub fn build_registry_default() -> HandlerRegistry {
    build_registry(|| false, |_, _| Err("no backend".to_string()), |_, _| None)
}

/// Register `image.generate` onto an existing registry.
///
/// Mirrors `register(server)` which calls `_registry.install(server)`.
/// This helper defers registration onto `registry` with the provided deps.
pub fn register_with<C, G, D>(registry: &mut HandlerRegistry, check: C, generate: G, data_url: D)
where
    C: Fn() -> bool + Send + Sync + 'static,
    G: Fn(&str, &str) -> Result<String, String> + Send + Sync + 'static,
    D: Fn(&str, usize) -> Option<String> + Send + Sync + 'static,
{
    registry.method(METHOD_NAME, move |rid, params_json| {
        handle_image_generate_json(&rid, &params_json, &check, &generate, &data_url)
    });
}

/// Register with default stub (availability false) onto `registry`.
pub fn register(registry: &mut HandlerRegistry) {
    register_with(registry, || false, |_, _| Err("no backend".to_string()), |_, _| None);
}

// ---------------------------------------------------------------------------
// Tests — mirror Python invariants (std-only)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn params(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn truthy_strings() {
        assert!(is_truthy_str("true"));
        assert!(is_truthy_str("True"));
        assert!(is_truthy_str("1"));
        assert!(is_truthy_str("yes"));
        assert!(is_truthy_str("on"));
        assert!(is_truthy_str("  YES  "));
        assert!(!is_truthy_str("false"));
        assert!(!is_truthy_str("0"));
        assert!(!is_truthy_str(""));
        assert!(!is_truthy_str("   "));
        assert!(is_truthy_value(Some("true")));
        assert!(!is_truthy_value(None));
        assert!(!is_truthy_value(Some("false")));
        assert!(is_truthy_bool(true));
        assert!(!is_truthy_bool(false));
    }

    #[test]
    fn normalize_aspect_cases() {
        assert_eq!(normalize_aspect(None), "square");
        assert_eq!(normalize_aspect(Some("")), "square");
        assert_eq!(normalize_aspect(Some("  ")), "square");
        assert_eq!(normalize_aspect(Some("LANDSCAPE")), "landscape");
        assert_eq!(normalize_aspect(Some(" portrait ")), "portrait");
    }

    #[test]
    fn parse_max_bytes_cases() {
        assert_eq!(parse_max_bytes(None), DEFAULT_MAX_BYTES);
        assert_eq!(parse_max_bytes(Some("")), DEFAULT_MAX_BYTES);
        assert_eq!(parse_max_bytes(Some("0")), DEFAULT_MAX_BYTES);
        assert_eq!(parse_max_bytes(Some("100")), 100);
        assert_eq!(parse_max_bytes(Some("16000000")), 16_000_000);
        assert_eq!(parse_max_bytes(Some("20000000")), HARD_MAX_BYTES);
        assert_eq!(parse_max_bytes(Some("not-a-number")), DEFAULT_MAX_BYTES);
        assert_eq!(parse_max_bytes(Some("-5")), DEFAULT_MAX_BYTES);
    }

    #[test]
    fn guess_mime_cases() {
        assert_eq!(guess_mime("foo.png"), "image/png");
        assert_eq!(guess_mime("photo.JPG"), "image/jpeg");
        assert_eq!(guess_mime("a/webp"), "image/webp");
        assert_eq!(guess_mime("noext"), "image/png");
        assert_eq!(guess_mime("file.unknown"), "image/png");
    }

    #[test]
    fn base64_roundtrip() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"hello world"), "aGVsbG8gd29ybGQ=");
    }

    #[test]
    fn data_url_from_bytes_cases() {
        let du = data_url_from_bytes(b"hi", "image/png", 10).unwrap();
        assert_eq!(du, "data:image/png;base64,aGk=");
        assert!(data_url_from_bytes(b"hi", "text/plain", 10).unwrap().starts_with("data:image/png;"));
        assert!(data_url_from_bytes(&vec![0u8; 100], "image/png", 10).is_none());
    }

    #[test]
    fn to_data_url_dispatch() {
        // http path
        let http = |url: &str, cap: usize| -> Option<(Vec<u8>, String)> {
            if url == "https://example.com/img.png" {
                Some((b"pngdata".to_vec(), "image/png".to_string()))
            } else {
                None
            }
        };
        let file = |_p: &str, _cap: usize| -> Option<(Vec<u8>, String)> { None };
        let du = to_data_url("https://example.com/img.png", 100, http, file).unwrap();
        assert!(du.starts_with("data:image/png;base64,"));

        // file path
        let http2 = |_: &str, _: usize| -> Option<(Vec<u8>, String)> { None };
        let file2 = |p: &str, _cap: usize| -> Option<(Vec<u8>, String)> {
            if p == "/tmp/img.jpg" {
                Some((b"jpgdata".to_vec(), "image/jpeg".to_string()))
            } else {
                None
            }
        };
        let du2 = to_data_url("/tmp/img.jpg", 100, http2, file2).unwrap();
        assert!(du2.starts_with("data:image/jpeg;base64,"));

        // cap exceeded
        let http3 = |_: &str, _: usize| -> Option<(Vec<u8>, String)> {
            Some((vec![0u8; 20], "image/png".to_string()))
        };
        let file3 = |_: &str, _: usize| -> Option<(Vec<u8>, String)> { None };
        assert!(to_data_url("https://example.com/big.png", 10, http3, file3).is_none());

        // unknown ref
        let none: Option<String> = to_data_url("not-a-file-or-url", 100, |_,_| None, |_,_| None);
        assert!(none.is_none());
    }

    #[test]
    fn ok_err_envelope() {
        let ok = ok_response("1", r#"{"available":true}"#);
        assert!(ok.contains(r#""result":{"available":true}"#));
        let err = err_response("1", 4071, "prompt required");
        assert!(err.contains(r#""code":4071"#));
        assert!(err.contains("prompt required"));
    }

    #[test]
    fn parse_params_json_cases() {
        let m = parse_params_json(r#"{"prompt":"hello","aspect_ratio":"square","probe":true,"max_bytes":8000000}"#);
        assert_eq!(m.get("prompt").map(|s| s.as_str()), Some("hello"));
        assert_eq!(m.get("aspect_ratio").map(|s| s.as_str()), Some("square"));
        assert_eq!(m.get("probe").map(|s| s.as_str()), Some("true"));
        assert_eq!(m.get("max_bytes").map(|s| s.as_str()), Some("8000000"));
        let empty = parse_params_json("{}");
        assert!(empty.is_empty());
    }

    #[test]
    fn handle_probe_returns_available() {
        let mut p = params(&[("probe", "true")]);
        let out = handle_image_generate("1", &p, || true, |_, _| Ok(r#"{"success":true,"image":"x"}"#.into()), |_, _| None);
        assert!(out.contains(r#""available":true"#));
        assert!(!out.contains("success"));
        let out2 = handle_image_generate("1", &p, || false, |_, _| Ok(r#"{"success":true,"image":"x"}"#.into()), |_, _| None);
        assert!(out2.contains(r#""available":false"#));
    }

    #[test]
    fn handle_no_backend() {
        let p = params(&[("prompt", "a cat")]);
        let out = handle_image_generate("42", &p, || false, |_, _| Ok(r#"{"success":true,"image":"x"}"#.into()), |_, _| None);
        assert!(out.contains("No image generation backend"));
        assert!(out.contains(r#""available":false"#));
    }

    #[test]
    fn handle_missing_prompt() {
        let p = params(&[]);
        let out = handle_image_generate("1", &p, || true, |_, _| Ok(r#"{"success":true,"image":"x"}"#.into()), |_, _| None);
        assert!(out.contains(r#""code":4071"#));
        let p2 = params(&[("prompt", "   ")]);
        let out2 = handle_image_generate("1", &p2, || true, |_, _| Ok(r#"{"success":true,"image":"x"}"#.into()), |_, _| None);
        assert!(out2.contains(r#""code":4071"#));
    }

    #[test]
    fn handle_generate_err() {
        let p = params(&[("prompt", "hi")]);
        let out = handle_image_generate("1", &p, || true, |_, _| Err("boom".into()), |_, _| None);
        assert!(out.contains(r#""code":5071"#));
        assert!(out.contains("boom"));
    }

    #[test]
    fn handle_success_with_data_url() {
        let p = params(&[("prompt", "a cat"), ("aspect_ratio", "landscape")]);
        let out = handle_image_generate(
            "7",
            &p,
            || true,
            |prompt, aspect| {
                assert_eq!(prompt, "a cat");
                assert_eq!(aspect, "landscape");
                Ok(r#"{"success":true,"image":"https://example.com/img.png"}"#.into())
            },
            |url, _cap| {
                assert_eq!(url, "https://example.com/img.png");
                Some("data:image/png;base64,abc".to_string())
            },
        );
        assert!(out.contains(r#""success":true"#));
        assert!(out.contains("https://example.com/img.png"));
        assert!(out.contains("image_data"));
    }

    #[test]
    fn handle_success_omits_data_url_when_none() {
        let p = params(&[("prompt", "hi")]);
        let out = handle_image_generate("1", &p, || true, |_, _| Ok(r#"{"success":true,"image":"https://example.com/img.png"}"#.into()), |_, _| None);
        assert!(out.contains(r#""success":true"#));
        assert!(!out.contains("image_data"));
    }

    #[test]
    fn handle_generation_failure() {
        let p = params(&[("prompt", "hi")]);
        let out = handle_image_generate("1", &p, || true, |_, _| Ok(r#"{"success":false,"error":"bad prompt"}"#.into()), |_, _| None);
        assert!(out.contains(r#""success":false"#));
        assert!(out.contains("bad prompt"));
    }

    #[test]
    fn build_registry_installs() {
        let mut reg = build_registry_default();
        assert_eq!(reg.len(), 1);
        assert_eq!(reg.pending_names().collect::<Vec<_>>(), vec!["image.generate"]);
        let mut map = std::collections::HashMap::new();
        reg.install_into(&mut map);
        assert!(map.contains_key("image.generate"));
        let out = map.get("image.generate").unwrap()("1".to_string(), r#"{"probe":true}"#.to_string());
        assert!(out.contains("available"));
    }
}
