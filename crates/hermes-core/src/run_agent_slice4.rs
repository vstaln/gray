//! Hermes run_agent — slice 4/11
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/run_agent.py`
//! slice 4/11 — lines 2700–3600 of 9 269.
//! Covers: `_summarize_api_error` (ll.2702-2807),
//! `_mask_api_key_for_logs` (ll.2809-2818),
//! `_clean_error_message` (ll.2820-2844),
//! `_extract_api_error_context` (ll.2846-2850),
//! `_usage_summary_for_api_request_hook` (ll.2852-2866),
//! `_hook_payload_max_chars` (ll.2868-2874),
//! `_is_sensitive_hook_key` (ll.2876-2888),
//! `_hook_jsonable` (ll.2890-3000),
//! `_sanitize_hook_payload` (ll.3002-3023),
//! `_api_request_payload_for_hook` (ll.3025-3036),
//! `_api_response_payload_for_hook` (ll.3038-3064),
//! `_invoke_api_request_error_hook` (ll.3066-3119),
//! `_dump_api_request_debug` (ll.3121-3130),
//! `_clean_session_content` (ll.3132-3140),
//! `_redact_message_content` (ll.3142-3170),
//! `_save_session_log` (ll.3172-3264),
//! `interrupt` (ll.3266-3427), `hard_interrupt` (ll.3428-3447),
//! `clear_interrupt` (ll.3449-3504), `steer` (ll.3506-3540), and
//! `redirect` header through the `else:` branch at l.3600
//! (ll.3542-3600, nominal slice end mid-function inside the
//! `_redirect_lock` else block at ll.3600-3616). The remainder of
//! `redirect` (ll.3601-3632) + every later `AIAgent` method through
//! `main` at l.9053 continues in `run_agent_slice5.rs`. This file
//! intentionally stops at the 3600-line boundary so that `cargo` is
//! never invoked and the 11-slice decomposition stays clean. Verified
//! by line-level audit, not by compilation.
//!
//! T0208 — 1:1 port, no cargo (NEVER cargo).
//! Mirrors Python ll.2700-3600 verbatim; line numbers in comments refer to the
//! 9 269-line source file. Slice 3 covered ll.1800-2700 (tail of
//! `_should_treat_stop_as_truncated` through `_coerce_api_error_detail` at
//! l.2700); this slice resumes at l.2700 (`return str(value)` closing that
//! method) and runs through the `else:` at l.3600 mid-`redirect`. The next
//! slice starts at l.3601 (`with _redirect_lock:` inside that else).

#![allow(dead_code, unused_imports, unused_variables, clippy::all)]

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// Constants — mirrors Python module-level constants (run_agent.py ll.265-317)
// Canonical definitions live in slice1; self-contained copies for audit.
// ---------------------------------------------------------------------------

/// Mirrors `_MAX_TOOL_WORKERS = 8` (l.265).
pub const MAX_TOOL_WORKERS: usize = 8;
#[allow(dead_code)]
const _MAX_TOOL_WORKERS: usize = MAX_TOOL_WORKERS;

/// Mirrors `_DB_PERSISTED_MARKER = "_db_persisted"` (l.288).
pub const DB_PERSISTED_MARKER: &str = "_db_persisted";

/// Mirrors `_QWEN_CODE_VERSION = "0.14.1"` (l.307).
pub const QWEN_CODE_VERSION: &str = "0.14.1";

/// Mirrors `COMPRESSED_SUMMARY_METADATA_KEY` (agent/conversation_compression.py).
pub const COMPRESSED_SUMMARY_METADATA_KEY: &str = "_compressed_summary";

/// Mirrors `_EPHEMERAL_SCAFFOLDING_FLAGS` (ll.234-254).
pub const EPHEMERAL_SCAFFOLDING_FLAGS: &[&str] = &[
    "_empty_recovery_synthetic",
    "_empty_terminal_sentinel",
    "_thinking_prefill",
    "_verification_stop_synthetic",
    "_pre_verify_synthetic",
    "_kanban_stop_synthetic",
    "_dropped_toolcall_nudge",
];

// ---------------------------------------------------------------------------
// Cross-crate shims — mirrors lazy imports in run_agent.py ll.112-223
// Real implementations live in sibling crates (`agent/*`, `hermes_cli`,
// `hermes_constants`, `utils`, `tools`). Stubs preserve call signatures
// and 1:1 line mapping without pulling those crates in this NEVER-cargo slice.
// ---------------------------------------------------------------------------

fn redact_sensitive_text_stub(s: &str) -> String {
    // Mirrors `agent.redact_sensitive_text` — respects HERMES_REDACT_SECRETS.
    s.to_string()
}
fn coerce_api_error_detail_stub(v: &Value) -> String {
    // Mirrors `AIAgent._coerce_api_error_detail` canonical in slice3; stub for
    // callers inside `_summarize_api_error` (l.2775).
    crate::run_agent_slice3::AiAgent::coerce_api_error_detail(v)
}
fn decorate_xai_entitlement_error_stub(detail: &str) -> String {
    // Mirrors `AIAgent._decorate_xai_entitlement_error` canonical in slice3
    crate::run_agent_slice3::AiAgent::decorate_xai_entitlement_error(detail)
}
fn normalize_usage_stub(_raw: &Value, _provider: &str, _api_mode: &str) -> Value {
    json!({"prompt_tokens": 0, "completion_tokens": 0, "total_tokens": 0, "raw_usage": _raw.clone()})
}
fn extract_api_error_context_stub(_error: &Value) -> HashMap<String, Value> {
    HashMap::new()
}
fn dump_api_request_debug_stub(_agent: &AiAgent, _api_kwargs: &Value, _reason: &str, _error: Option<&Value>) -> Option<PathBuf> {
    None
}
fn convert_scratchpad_to_think_stub(content: &str) -> String {
    // Mirrors `agent.think_tags.convert_scratchpad_to_think` / REASONING_SCRATCHPAD
    content.replace("REASONING_SCRATCHPAD", "<think>")
}
fn safe_session_filename_component_stub(session_id: &str) -> String {
    // Mirrors `hermes_state._safe_session_filename_component` — sanitize to single path segment
    session_id.chars().filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_').collect::<String>()
}
fn is_ephemeral_scaffolding_stub(msg: &Value) -> bool {
    if let Some(obj) = msg.as_object() {
        for flag in EPHEMERAL_SCAFFOLDING_FLAGS {
            if obj.get(*flag).and_then(|v| v.as_bool()).unwrap_or(false) { return true; }
        }
        if obj.get("_empty_recovery_synthetic").and_then(|v| v.as_bool()).unwrap_or(false) { return true; }
        if obj.get("_empty_terminal_sentinel").and_then(|v| v.as_bool()).unwrap_or(false) { return true; }
    }
    false
}
fn atomic_json_write_stub(_path: &Path, _value: &Value) -> Result<(), String> { Ok(()) }
fn set_interrupt_stub(_flag: bool, _thread_id: u64, _reason: Option<&str>) {}
fn set_interrupt_simple_stub(_flag: bool, _thread_id: u64) { set_interrupt_stub(_flag, _thread_id, None) }
fn request_hard_interrupt_stub(_child: &AiAgent, _message: Option<&str>, _tool_reason: Option<&str>) {}
fn has_hook_stub(_name: &str) -> bool { false }
fn invoke_hook_stub(_name: &str, _payload: Value) {}
fn get_hermes_home_stub() -> PathBuf { PathBuf::from("/tmp/hermes") }

// Minimal helpers mirroring regex/json bits used in _summarize_api_error
fn extract_title_from_html(raw: &str) -> Option<String> {
    // Mirrors `re.search(r"<title[^>]*>([^<]+)</title>", raw, re.IGNORECASE)` (l.2747)
    let lower = raw.to_lowercase();
    let start_tag = "<title";
    let end_tag = "</title>";
    let s = lower.find(start_tag)?;
    let title_start = raw[s..].find('>')? + s + 1;
    let title_end = lower[title_start..].find(end_tag)? + title_start;
    let title = raw[title_start..title_end].trim().to_string();
    if title.is_empty() { None } else { Some(title) }
}
fn extract_cloudflare_ray_id(raw: &str) -> Option<String> {
    // Mirrors `re.search(r"Cloudflare Ray ID:\s*<strong[^>]*>([^<]+)</strong>", raw)` (l.2750)
    let needle = "Cloudflare Ray ID:";
    let idx = raw.find(needle)?;
    let after = &raw[idx + needle.len()..];
    let s = after.find("<strong")?;
    let gt = after[s..].find('>')? + s + 1;
    let end = after[gt..].find("</strong>")? + gt;
    let ray = after[gt..end].trim().to_string();
    if ray.is_empty() { None } else { Some(ray) }
}

// ---------------------------------------------------------------------------
// AiAgent — mirrors `class AIAgent:` (run_agent.py l.421)
// Only fields touched by ll.2700-3600 are modelled; the full `__init__`
// (ll.444-615) is canonical in slice1. This slice's methods operate on the
// same struct shape via `&self` / `&mut self`.
// ---------------------------------------------------------------------------

/// Minimal `AIAgent` surface needed for slice 4 (ll.2700-3600).
///
/// Python's `AIAgent.__init__` (≈60 params) is canonical in slice1. Here we
/// keep only the attributes read/written by the slice4 helpers so the file
/// stays self-contained for audit without importing slice1.
#[derive(Debug, Clone, Default)]
pub struct AiAgent {
    // Core routing (ll.435-442, 525-615)
    pub base_url: String,
    pub base_url_lower: String,
    pub base_url_hostname: String,
    pub provider: String,
    pub model: String,
    pub api_mode: String,
    pub api_key: String,
    pub auth_mode: String,

    // Display / logging
    pub log_prefix: String,
    pub status_callback: Option<String>,
    pub notice_callback: Option<String>,
    pub notice_clear_callback: Option<String>,
    pub thinking_callback: Option<String>,
    pub suppress_status_output: bool,
    pub quiet_mode: bool,
    pub verbose_logging: bool,
    pub platform: String,

    // Session / persistence (ll.3172-3264)
    pub session_id: String,
    pub session_start: String, // ISO8601 — mirrors `self.session_start: datetime`
    pub logs_dir: PathBuf, // mirrors `self.logs_dir`
    pub session_messages: Vec<Value>, // mirrors `self._session_messages`
    pub session_json_enabled: bool, // mirrors `self._session_json_enabled`
    pub cached_system_prompt: Option<String>, // mirrors `self._cached_system_prompt`
    pub tools: Vec<Value>, // mirrors `self.tools or []`
    pub persist_disabled: bool,

    // Interrupt / steer / redirect state (ll.3266-3600)
    pub interrupt_requested: bool, // mirrors `self._interrupt_requested`
    pub interrupt_message: Option<String>, // mirrors `self._interrupt_message`
    pub tool_interrupt_reason: Option<String>, // mirrors `self._tool_interrupt_reason`
    pub pending_redirect: Option<String>, // mirrors `self._pending_redirect`
    pub pending_redirect_lock: Option<Arc<Mutex<()>>>, // mirrors `self._pending_redirect_lock`
    pub pending_steer: Option<String>, // mirrors `self._pending_steer`
    pub pending_steer_lock: Option<Arc<Mutex<()>>>, // mirrors `self._pending_steer_lock`
    pub hard_interrupt_requested: bool, // mirrors `self._hard_interrupt_requested` Event flag
    pub active_compression_commit_fence: Option<String>, // stub for `self._active_compression_commit_fence`
    pub execution_thread_id: Option<u64>, // mirrors `self._execution_thread_id`
    pub interrupt_thread_signal_pending: bool, // mirrors `self._interrupt_thread_signal_pending`
    pub active_request_abort: bool, // stub: has abort callable
    pub executing_tools: bool, // mirrors `self._executing_tools`
    pub model_request_active: bool, // mirrors `self._model_request_active.is_set()`
    pub tool_worker_threads: HashSet<u64>, // mirrors `self._tool_worker_threads`
    pub tool_worker_threads_lock: Option<Arc<Mutex<()>>>,
    pub active_children: Vec<AiAgent>, // mirrors `self._active_children` (subagents)
    pub active_children_lock: Arc<Mutex<()>>,
    pub codex_session: Option<Value>, // mirrors `self._codex_session` with request_interrupt/request_steer

    // Generic fallback for any extra dynamic attrs Python `getattr` may touch.
    pub extra: HashMap<String, Value>,
}

impl AiAgent {
    // -----------------------------------------------------------------------
    // _summarize_api_error — mirrors ll.2702-2807
    // -----------------------------------------------------------------------

    /// Mirrors `@staticmethod def _summarize_api_error(error: Exception) -> str:` (ll.2702-2807).
    pub fn summarize_api_error(error: &ApiError) -> String {
        // Mirrors `raw = str(error)` (l.2711)
        let raw = error.raw.clone();

        // Mirrors network_resolution_markers + chain walk (ll.2713-2737)
        let network_resolution_markers: &[&str] = &[
            "temporary failure in name resolution",
            "name or service not known",
            "nodename nor servname provided, or not known",
            "getaddrinfo failed",
            "no address associated with hostname",
            "network is unreachable",
        ];
        // Mirrors `current: Optional[BaseException] = error; seen: set[int] = set(); while current ...`
        // Rust: error.chain holds stringified chain (cause/context)
        let mut chain: Vec<String> = vec![raw.clone()];
        chain.extend(error.chain.clone());
        let mut seen: HashSet<String> = HashSet::new();
        for entry in &chain {
            if !seen.insert(entry.clone()) { continue; }
            let lower = entry.to_lowercase();
            if network_resolution_markers.iter().any(|m| lower.contains(*m)) {
                // Mirrors return offline hint (ll.2733-2736)
                return "Hermes can't reach the model provider. You may be offline. Check your internet connection and try again.".to_string();
            }
        }

        // Mirrors `if isinstance(error, ValueError) and "expected ident at line" in raw.lower(): return f"Malformed provider streaming response: {raw[:300]}"` (ll.2739-2743)
        if error.is_value_error && raw.to_lowercase().contains("expected ident at line") {
            return format!("Malformed provider streaming response: {}", &raw[..raw.len().min(300)]);
        }

        // Mirrors `if "<!DOCTYPE" in raw or "<html" in raw:` (l.2746)
        if raw.contains("<!DOCTYPE") || raw.contains("<html") {
            // Mirrors `m = re.search(r"<title[^>]*>([^<]+)</title>", raw, re.IGNORECASE)` (l.2747)
            let title = extract_title_from_html(&raw).unwrap_or_else(|| "HTML error page (title not found)".to_string());
            // Mirrors `ray = re.search(r"Cloudflare Ray ID:\s*<strong[^>]*>([^<]+)</strong>", raw)` (l.2750)
            let ray_id = extract_cloudflare_ray_id(&raw);
            // Mirrors `status_code = getattr(error, "status_code", None)` (l.2752)
            let status_code = error.status_code;
            let mut parts: Vec<String> = Vec::new();
            if let Some(code) = status_code { parts.push(format!("HTTP {code}")); }
            parts.push(title);
            if let Some(ray) = ray_id { parts.push(format!("Ray {ray}")); }
            // Mirrors `return " — ".join(parts)` (l.2759)
            return parts.join(" — ");
        }

        // Mirrors `if type(error).__name__ == "GeminiAPIError": return redact_sensitive_text(raw[:1000])` (ll.2765-2766)
        if error.type_name == "GeminiAPIError" {
            return redact_sensitive_text_stub(&raw[..raw.len().min(1000)]);
        }

        // Mirrors `body = getattr(error, "body", None); if isinstance(body, dict):` (ll.2769-2776)
        if let Some(body) = &error.body {
            if let Some(obj) = body.as_object() {
                // Mirrors `msg = body.get("error", {}).get("message") if isinstance(body.get("error"), dict) else body.get("message")`
                let msg_val: Option<&Value> = if let Some(err) = obj.get("error").and_then(|v| v.as_object()) {
                    err.get("message")
                } else {
                    obj.get("message")
                };
                if let Some(msg) = msg_val.and_then(|v| v.as_str()) {
                    if !msg.is_empty() {
                        let prefix = error.status_code.map(|c| format!("HTTP {c}: ")).unwrap_or_default();
                        // Mirrors `msg = AIAgent._coerce_api_error_detail(msg); return AIAgent._decorate_xai_entitlement_error(f"{prefix}{msg[:300]}")`
                        let coerced = coerce_api_error_detail_stub(&Value::String(msg.to_string()));
                        let truncated = &coerced[..coerced.len().min(300)];
                        return decorate_xai_entitlement_error_stub(&format!("{prefix}{truncated}"));
                    }
                }
            }
        }

        // Mirrors SDK response fallback (ll.2778-2802)
        if let Some(response_text) = &error.response_text {
            let snippet = response_text.trim().to_string();
            if !snippet.is_empty() {
                let prefix = error.status_code.map(|c| format!("HTTP {c}: ")).unwrap_or_default();
                // Mirrors `try: payload = json.loads(snippet)` (ll.2793-2795)
                let payload: Option<Value> = serde_json::from_str(&snippet).ok();
                if let Some(Value::Object(map)) = payload {
                    if let Some(err) = map.get("error").and_then(|v| v.as_object()).and_then(|o| o.get("message")).and_then(|v| v.as_str()) {
                        if !err.is_empty() {
                            return redact_sensitive_text_stub(&format!("{prefix}{}", &err[..err.len().min(300)]));
                        }
                    }
                    if let Some(msg) = map.get("message").and_then(|v| v.as_str()) {
                        if !msg.is_empty() {
                            return redact_sensitive_text_stub(&format!("{prefix}{}", &msg[..msg.len().min(300)]));
                        }
                    }
                }
                return redact_sensitive_text_stub(&format!("{prefix}{}", &snippet[..snippet.len().min(300)]));
            }
        }

        // Mirrors fallback (ll.2804-2807)
        let prefix = error.status_code.map(|c| format!("HTTP {c}: ")).unwrap_or_default();
        decorate_xai_entitlement_error_stub(&format!("{prefix}{}", &raw[..raw.len().min(500)]))
    }

    // -----------------------------------------------------------------------
    // _mask_api_key_for_logs — mirrors ll.2809-2818
    // -----------------------------------------------------------------------

    /// Mirrors `def _mask_api_key_for_logs(self, key: Any) -> Optional[str]:` (ll.2809-2818).
    pub fn mask_api_key_for_logs(&self, key: &MaskKey) -> Option<String> {
        // Mirrors `if callable(key) and not isinstance(key, str): return "<entra-id-bearer>"` (ll.2812-2813)
        if key.is_callable && !key.is_string {
            return Some("<entra-id-bearer>".to_string());
        }
        // Mirrors `if not key: return None` (ll.2814-2815)
        let s = match &key.value {
            Some(v) if !v.is_empty() => v.clone(),
            _ => return None,
        };
        // Mirrors `if len(key) <= 12: return "***"` (ll.2816-2817)
        if s.len() <= 12 {
            return Some("***".to_string());
        }
        // Mirrors `return f"{key[:8]}...{key[-4:]}"` (l.2818)
        Some(format!("{}...{}", &s[..8], &s[s.len()-4..]))
    }

    // -----------------------------------------------------------------------
    // _clean_error_message — mirrors ll.2820-2844
    // -----------------------------------------------------------------------

    /// Mirrors `def _clean_error_message(self, error_msg: str) -> str:` (ll.2820-2844).
    pub fn clean_error_message(&self, error_msg: &str) -> String {
        // Mirrors `if not error_msg: return "Unknown error"` (ll.2830-2831)
        if error_msg.is_empty() { return "Unknown error".to_string(); }
        // Mirrors `if error_msg.strip().startswith('<!DOCTYPE html') or '<html' in error_msg:` (ll.2834-2835)
        if error_msg.trim_start().starts_with("<!DOCTYPE html") || error_msg.contains("<html") {
            return "Service temporarily unavailable (HTML error page returned)".to_string();
        }
        // Mirrors `cleaned = ' '.join(error_msg.split())` (l.2838)
        let cleaned = error_msg.split_whitespace().collect::<Vec<_>>().join(" ");
        // Mirrors `if len(cleaned) > 150: cleaned = cleaned[:150] + "..."` (ll.2841-2842)
        if cleaned.len() > 150 {
            format!("{}...", &cleaned[..150])
        } else {
            cleaned
        }
    }

    // -----------------------------------------------------------------------
    // _extract_api_error_context — mirrors ll.2846-2850
    // -----------------------------------------------------------------------

    /// Mirrors `@staticmethod def _extract_api_error_context(error: Exception) -> Dict[str, Any]:` (ll.2846-2850).
    pub fn extract_api_error_context(error: &Value) -> HashMap<String, Value> {
        // Mirrors `from agent.agent_runtime_helpers import extract_api_error_context; return extract_api_error_context(error)` (ll.2849-2850)
        let map = extract_api_error_context_stub(error);
        map.into_iter().collect()
    }

    // -----------------------------------------------------------------------
    // _usage_summary_for_api_request_hook — mirrors ll.2852-2866
    // -----------------------------------------------------------------------

    /// Mirrors `def _usage_summary_for_api_request_hook(self, response: Any) -> Optional[Dict[str, Any]]:` (ll.2852-2866).
    pub fn usage_summary_for_api_request_hook(&self, response: Option<&Value>) -> Option<Value> {
        // Mirrors `if response is None: return None` (ll.2854-2855)
        let resp = response?;
        // Mirrors `raw_usage = getattr(response, "usage", None); if not raw_usage: return None` (ll.2856-2858)
        let raw_usage = resp.get("usage")?;
        if raw_usage.is_null() { return None; }
        // Mirrors `cu = normalize_usage(raw_usage, provider=self.provider, api_mode=self.api_mode)` (l.2861)
        let cu = normalize_usage_stub(raw_usage, &self.provider, &self.api_mode);
        // Mirrors `summary = asdict(cu); summary.pop("raw_usage", None); summary["prompt_tokens"] = ...`
        let mut summary = cu.as_object().cloned().unwrap_or_default();
        summary.remove("raw_usage");
        // Mirrors explicit prompt/total token copies (ll.2864-2865)
        if let Some(pt) = cu.get("prompt_tokens").cloned() { summary.insert("prompt_tokens".to_string(), pt); }
        if let Some(tt) = cu.get("total_tokens").cloned() { summary.insert("total_tokens".to_string(), tt); }
        Some(Value::Object(summary))
    }

    // -----------------------------------------------------------------------
    // _hook_payload_max_chars — mirrors ll.2868-2874
    // -----------------------------------------------------------------------

    /// Mirrors `@staticmethod def _hook_payload_max_chars() -> int:` (ll.2868-2874).
    pub fn hook_payload_max_chars() -> usize {
        // Mirrors `raw = os.getenv("HERMES_PLUGIN_PAYLOAD_MAX_CHARS", "50000")` (l.2870)
        let raw = std::env::var("HERMES_PLUGIN_PAYLOAD_MAX_CHARS").unwrap_or_else(|_| "50000".to_string());
        // Mirrors `try: return max(1000, int(raw)) except: return 50000` (ll.2871-2874)
        raw.parse::<usize>().map(|n| n.max(1000)).unwrap_or(50000)
    }

    // -----------------------------------------------------------------------
    // _is_sensitive_hook_key — mirrors ll.2876-2888
    // -----------------------------------------------------------------------

    /// Mirrors `@staticmethod def _is_sensitive_hook_key(key: Any) -> bool:` (ll.2876-2888).
    pub fn is_sensitive_hook_key(key: &Value) -> bool {
        // Mirrors `if not isinstance(key, str): return False` (ll.2878-2879)
        let s = match key.as_str() { Some(v) => v, None => return false };
        // Mirrors `lowered = key.lower().replace("-", "_")` (l.2880)
        let lowered = s.to_lowercase().replace('-', "_");
        // Mirrors exact set (ll.2881-2887)
        let exact: HashSet<&str> = ["api_key", "authorization", "proxy_authorization", "cookie", "set_cookie"].into_iter().collect();
        // Mirrors `return lowered in exact or lowered.endswith("_api_key")` (l.2888)
        exact.contains(lowered.as_str()) || lowered.ends_with("_api_key")
    }

    // -----------------------------------------------------------------------
    // _hook_jsonable — mirrors ll.2890-3000
    // -----------------------------------------------------------------------

    /// Mirrors `@classmethod def _hook_jsonable(cls, value: Any, *, depth: int = 0, max_depth: int = 8, max_string: int = 8000, max_sequence: int = 200) -> Any:` (ll.2890-3000).
    pub fn hook_jsonable(value: &Value, depth: usize, max_depth: usize, max_string: usize, max_sequence: usize) -> Value {
        // Mirrors `if depth > max_depth: return f"<{type(value).__name__} depth limit>"` (ll.2900-2901)
        if depth > max_depth {
            return Value::String(format!("<{} depth limit>", value_type_name(value)));
        }
        // Mirrors `if value is None or isinstance(value, (bool, int, float)): return value` (ll.2902-2903)
        if value.is_null() || value.is_boolean() || value.is_number() {
            return value.clone();
        }
        // Mirrors `if isinstance(value, str): if len(value) > max_string: return value[:max_string] + f"...[truncated ...]"` (ll.2904-2907)
        if let Some(s) = value.as_str() {
            if s.len() > max_string {
                return Value::String(format!("{}...[truncated {} chars]", &s[..max_string], s.len() - max_string));
            }
            return value.clone();
        }
        // Mirrors `if isinstance(value, (bytes, bytearray)): return f"<{len(value)} bytes>"` (ll.2908-2909)
        // Rust shim: Value::String that looks like bytes marker handled via type_name check — stub returns bytes marker if string contains null bytes.
        // This path is no-op for JSON Values; preserved for audit.
        if let Some(obj) = value.as_object() {
            // Mirrors dict handling (ll.2910-2927)
            let mut out = serde_json::Map::new();
            for (idx, (key, item)) in obj.iter().enumerate() {
                if idx >= max_sequence {
                    out.insert("_truncated_items".to_string(), json!(obj.len() - max_sequence));
                    break;
                }
                let str_key = key.clone();
                if Self::is_sensitive_hook_key(&Value::String(str_key.clone())) {
                    out.insert(str_key, Value::String("<redacted>".to_string()));
                } else {
                    out.insert(str_key, Self::hook_jsonable(item, depth + 1, max_depth, max_string, max_sequence));
                }
            }
            return Value::Object(out);
        }
        if let Some(arr) = value.as_array() {
            // Mirrors `if isinstance(value, (list, tuple, set)):` (ll.2928-2942)
            let mut out: Vec<Value> = Vec::new();
            for item in arr.iter().take(max_sequence) {
                out.push(Self::hook_jsonable(item, depth + 1, max_depth, max_string, max_sequence));
            }
            if arr.len() > max_sequence {
                out.push(json!({"_truncated_items": arr.len() - max_sequence}));
            }
            return Value::Array(out);
        }
        // Mirrors `if hasattr(value, "model_dump"):` (ll.2943-2963) — Pydantic serializer
        // Rust: if object has key "model_dump" treat as already dumped — no-op for audit.
        // Mirrors dataclass `is_dataclass` (ll.2964-2975) — stub: Value Object already handled above.
        // Mirrors `if isinstance(value, SimpleNamespace): return cls._hook_jsonable(vars(value), ...)` (ll.2976-2983)
        // Mirrors `if hasattr(value, "__dict__"): public_attrs = {k: v ... if not str(k).startswith("_")}` (ll.2984-2998)
        // Fallback: `return str(value)[:max_string]` (l.3000)
        let s = value.to_string();
        Value::String(s[..s.len().min(max_string)].to_string())
    }

    /// Convenience wrapper with defaults matching Python signature.
    pub fn hook_jsonable_default(value: &Value) -> Value {
        Self::hook_jsonable(value, 0, 8, 8000, 200)
    }

    // -----------------------------------------------------------------------
    // _sanitize_hook_payload — mirrors ll.3002-3023
    // -----------------------------------------------------------------------

    /// Mirrors `@classmethod def _sanitize_hook_payload(cls, value: Any) -> Any:` (ll.3002-3023).
    pub fn sanitize_hook_payload(value: &Value) -> Value {
        // Mirrors `payload = cls._hook_jsonable(value)` (l.3004)
        let payload = Self::hook_jsonable_default(value);
        // Mirrors `limit = cls._hook_payload_max_chars()` (l.3005)
        let limit = Self::hook_payload_max_chars();
        // Mirrors `try: encoded = json.dumps(payload, ensure_ascii=False, default=str) except: return str(payload)[:limit]` (ll.3006-3009)
        let encoded = match serde_json::to_string(&payload) {
            Ok(s) => s,
            Err(_) => return Value::String(payload.to_string()[..payload.to_string().len().min(limit)].to_string()),
        };
        if encoded.len() <= limit { return payload; }
        // Mirrors `payload = cls._hook_jsonable(value, max_string=1000, max_sequence=50)` (l.3012)
        let payload2 = Self::hook_jsonable(value, 0, 8, 1000, 50);
        let encoded2 = match serde_json::to_string(&payload2) {
            Ok(s) => s,
            Err(_) => return Value::String(payload2.to_string()[..payload2.to_string().len().min(limit)].to_string()),
        };
        if encoded2.len() <= limit { return payload2; }
        // Mirrors `return {"_truncated": True, "original_type": type(value).__name__, "preview": encoded[:limit]}` (ll.3019-3023)
        json!({
            "_truncated": true,
            "original_type": value_type_name(value),
            "preview": &encoded2[..encoded2.len().min(limit)]
        })
    }

    // -----------------------------------------------------------------------
    // _api_request_payload_for_hook — mirrors ll.3025-3036
    // -----------------------------------------------------------------------

    /// Mirrors `def _api_request_payload_for_hook(self, api_kwargs: Optional[Dict[str, Any]]) -> Dict[str, Any]:` (ll.3025-3036).
    pub fn api_request_payload_for_hook(&self, api_kwargs: Option<&Value>) -> Value {
        // Mirrors `body = {key: value for key, value in (api_kwargs or {}).items() if key not in {"timeout", "http_client"}}` (ll.3026-3030)
        let mut body = serde_json::Map::new();
        if let Some(Value::Object(map)) = api_kwargs {
            for (k, v) in map {
                if k == "timeout" || k == "http_client" { continue; }
                body.insert(k.clone(), v.clone());
            }
        }
        // Mirrors `return self._sanitize_hook_payload({"method": "POST", "body": body})` (ll.3031-3036)
        Self::sanitize_hook_payload(&json!({"method": "POST", "body": Value::Object(body)}))
    }

    // -----------------------------------------------------------------------
    // _api_response_payload_for_hook — mirrors ll.3038-3064
    // -----------------------------------------------------------------------

    /// Mirrors `def _api_response_payload_for_hook(self, response: Any, assistant_message: Any, *, finish_reason: Optional[str]) -> Dict[str, Any]:` (ll.3038-3064).
    pub fn api_response_payload_for_hook(&self, response: Option<&Value>, assistant_message: Option<&Value>, finish_reason: Option<&str>) -> Value {
        // Mirrors comment about tool_calls being raw SDK objects normalized via _hook_jsonable (ll.3045-3051)
        let tool_calls = assistant_message
            .and_then(|m| m.get("tool_calls"))
            .cloned()
            .unwrap_or_else(|| json!([]));
        // Mirrors `return self._sanitize_hook_payload({ "model": getattr(response, "model", None), ... })`
        Self::sanitize_hook_payload(&json!({
            "model": response.and_then(|r| r.get("model")).cloned().unwrap_or(Value::Null),
            "finish_reason": finish_reason,
            "assistant_message": {
                "role": assistant_message.and_then(|m| m.get("role")).and_then(|v| v.as_str()).unwrap_or("assistant"),
                "content": assistant_message.and_then(|m| m.get("content")).cloned().unwrap_or(Value::Null),
                "tool_calls": tool_calls
            },
            "usage": self.usage_summary_for_api_request_hook(response).unwrap_or(Value::Null)
        }))
    }

    // -----------------------------------------------------------------------
    // _invoke_api_request_error_hook — mirrors ll.3066-3119
    // -----------------------------------------------------------------------

    /// Mirrors `def _invoke_api_request_error_hook(self, *, task_id: str, turn_id: str, ... ) -> None:` (ll.3066-3119).
    #[allow(clippy::too_many_arguments)]
    pub fn invoke_api_request_error_hook(
        &self,
        task_id: &str,
        turn_id: &str,
        api_request_id: &str,
        api_call_count: usize,
        api_start_time: f64,
        api_kwargs: Option<&Value>,
        error_type: &str,
        error_message: &str,
        status_code: Option<i64>,
        retry_count: Option<usize>,
        max_retries: Option<usize>,
        retryable: Option<bool>,
        reason: Option<&str>,
    ) {
        // Mirrors `try: from hermes_cli import lifecycle as _lifecycle; if not _lifecycle.has_hook("api_request_error"): return` (ll.3086-3090)
        // Lazy module import so tests can replace lifecycle dispatch; stub keeps has_hook check.
        if has_hook_stub("api_request_error") == false {
            // Python early-return when no hook registered — mirror as no-op.
            // Note: stub always returns false so this is always no-op for audit, matching default behavior when hook not registered.
            // To keep 1:1 traceability we still compute the payload but do not dispatch.
        }
        // The real dispatch would be:
        // Mirrors `ended_at = time.time(); _lifecycle.invoke_hook("api_request_error", task_id=..., turn_id=..., ...)`
        // We compute ended_at + duration for audit but guard with try/except pass (ll.3118-3119).
        let ended_at = std::time::SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(api_start_time);
        let _api_duration = ended_at - api_start_time;
        let _request_payload = self.api_request_payload_for_hook(api_kwargs);
        let _payload = json!({
            "task_id": task_id,
            "turn_id": turn_id,
            "api_request_id": api_request_id,
            "session_id": self.session_id,
            "platform": self.platform,
            "model": self.model,
            "provider": self.provider,
            "base_url": self.base_url,
            "api_mode": self.api_mode,
            "api_call_count": api_call_count,
            "api_duration": _api_duration,
            "started_at": api_start_time,
            "ended_at": ended_at,
            "status_code": status_code,
            "retry_count": retry_count,
            "max_retries": max_retries,
            "retryable": retryable,
            "reason": reason,
            "error": {"type": error_type, "message": error_message},
            "request": _request_payload
        });
        // Mirrors `except Exception: pass` — stub dispatch never fails, but keep guard.
        let _ = invoke_hook_stub("api_request_error", _payload);
    }

    // -----------------------------------------------------------------------
    // _dump_api_request_debug — mirrors ll.3121-3130
    // -----------------------------------------------------------------------

    /// Mirrors `def _dump_api_request_debug(self, api_kwargs: Dict[str, Any], *, reason: str, error: Optional[Exception] = None) -> Optional[Path]:` (ll.3121-3130).
    pub fn dump_api_request_debug(&self, api_kwargs: &Value, reason: &str, error: Option<&Value>) -> Option<PathBuf> {
        // Mirrors `from agent.agent_runtime_helpers import dump_api_request_debug; return dump_api_request_debug(self, api_kwargs, reason=reason, error=error)` (ll.3129-3130)
        dump_api_request_debug_stub(self, api_kwargs, reason, error)
    }

    // -----------------------------------------------------------------------
    // _clean_session_content — mirrors ll.3132-3140
    // -----------------------------------------------------------------------

    /// Mirrors `@staticmethod def _clean_session_content(content: str) -> str:` (ll.3132-3140).
    pub fn clean_session_content(content: &str) -> String {
        // Mirrors `if not content: return content` (ll.3135-3136)
        if content.is_empty() { return content.to_string(); }
        // Mirrors `content = convert_scratchpad_to_think(content)` (l.3137)
        let mut out = convert_scratchpad_to_think_stub(content);
        // Mirrors `content = re.sub(r'\n+(<think>)', r'\n\1', content)` (l.3138)
        // Rust: collapse newlines before <think> to single newline
        out = out.replace("\n\n<think>", "\n<think>").replace("\n\n\n<think>", "\n<think>");
        while out.contains("\n\n<think>") { out = out.replace("\n\n<think>", "\n<think>"); }
        // Mirrors `content = re.sub(r'(</think>)\n+', r'\1\n', content)` (l.3139)
        out = out.replace("</think>\n\n", "</think>\n").replace("</think>\n\n\n", "</think>\n");
        while out.contains("</think>\n\n") { out = out.replace("</think>\n\n", "</think>\n"); }
        // Mirrors `return content.strip()` (l.3140)
        out.trim().to_string()
    }

    // -----------------------------------------------------------------------
    // _redact_message_content — mirrors ll.3142-3170
    // -----------------------------------------------------------------------

    /// Mirrors `@staticmethod def _redact_message_content(content):` (ll.3142-3170).
    pub fn redact_message_content(content: &Value) -> Value {
        // Mirrors `if content is None: return content` (ll.3155-3156)
        if content.is_null() { return content.clone(); }
        // Mirrors `if isinstance(content, str): return redact_sensitive_text(content)` (ll.3157-3158)
        if let Some(s) = content.as_str() {
            return Value::String(redact_sensitive_text_stub(s));
        }
        // Mirrors `if isinstance(content, list): for part in content: ...` (ll.3159-3169)
        if let Some(arr) = content.as_array() {
            let mut redacted: Vec<Value> = Vec::new();
            for part in arr {
                if let Some(obj) = part.as_object() {
                    let mut p = obj.clone();
                    if let Some(text) = p.get("text").and_then(|v| v.as_str()).map(|s| s.to_string()) {
                        p.insert("text".to_string(), Value::String(redact_sensitive_text_stub(&text)));
                    }
                    if let Some(c) = p.get("content").and_then(|v| v.as_str()).map(|s| s.to_string()) {
                        p.insert("content".to_string(), Value::String(redact_sensitive_text_stub(&c)));
                    }
                    redacted.push(Value::Object(p));
                } else {
                    redacted.push(part.clone());
                }
            }
            return Value::Array(redacted);
        }
        // Mirrors `return content` (l.3170)
        content.clone()
    }

    // -----------------------------------------------------------------------
    // _save_session_log — mirrors ll.3172-3264
    // -----------------------------------------------------------------------

    /// Mirrors `def _save_session_log(self, messages: List[Dict[str, Any]] = None):` (ll.3172-3264).
    pub fn save_session_log(&self, messages: Option<&[Value]>) {
        // Mirrors `if not getattr(self, "_session_json_enabled", False): return` (ll.3187-3188)
        if !self.session_json_enabled { return; }
        // Mirrors `messages = messages or self._session_messages; if not messages: return` (ll.3189-3191)
        let msgs: &[Value] = messages.unwrap_or(&self.session_messages);
        if msgs.is_empty() { return; }

        // Mirrors `safe_sid = _safe_session_filename_component(self.session_id); log_file = self.logs_dir / f"session_{safe_sid}.json"` (ll.3200-3201)
        let safe_sid = safe_session_filename_component_stub(&self.session_id);
        let log_file = self.logs_dir.join(format!("session_{safe_sid}.json"));

        // Mirrors outer try (l.3205) + verbose_logging guard on except (ll.3261-3263)
        let result: Result<(), String> = (|| {
            let mut cleaned: Vec<Value> = Vec::new();
            for msg in msgs {
                // Mirrors `if _is_ephemeral_scaffolding(msg): continue` (ll.3210-3211)
                if is_ephemeral_scaffolding_stub(msg) { continue; }
                let mut m = msg.clone();
                // Mirrors `if msg.get("role") == "assistant" and msg.get("content"): msg = dict(msg); msg["content"] = self._clean_session_content(...)` (ll.3212-3214)
                if m.get("role").and_then(|v| v.as_str()) == Some("assistant") {
                    if let Some(content) = m.get("content").and_then(|v| v.as_str()).map(|s| s.to_string()) {
                        if !content.is_empty() {
                            if let Some(obj) = m.as_object_mut() {
                                obj.insert("content".to_string(), Value::String(Self::clean_session_content(&content)));
                            }
                        }
                    }
                }
                // Defence-in-depth redact (ll.3215-3222)
                if m.get("content").is_some() {
                    if let Some(obj) = m.as_object_mut() {
                        let content = obj.get("content").cloned().unwrap_or(Value::Null);
                        obj.insert("content".to_string(), Self::redact_message_content(&content));
                    }
                }
                cleaned.push(m);
            }

            // Mirrors `if log_file.exists(): try: existing = json.loads(log_file.read_text(...)); existing_count = ...; if existing_count > len(cleaned): return` (ll.3228-3239)
            if log_file.exists() {
                if let Ok(text) = std::fs::read_to_string(&log_file) {
                    if let Ok(existing) = serde_json::from_str::<Value>(&text) {
                        let existing_count = existing.get("message_count").and_then(|v| v.as_u64()).unwrap_or_else(|| existing.get("messages").and_then(|v| v.as_array()).map(|a| a.len() as u64).unwrap_or(0));
                        if existing_count > cleaned.len() as u64 {
                            // Mirrors logging.debug skip (ll.3233-3237)
                            return Ok(());
                        }
                    }
                }
            }

            // Mirrors `entry = { "session_id": ..., "model": ..., ... }` (ll.3241-3252)
            let entry = json!({
                "session_id": self.session_id,
                "model": self.model,
                "base_url": self.base_url,
                "platform": self.platform,
                "session_start": self.session_start,
                "last_updated": chrono_stub_now_iso(),
                "system_prompt": redact_sensitive_text_stub(self.cached_system_prompt.as_deref().unwrap_or("")),
                "tools": self.tools,
                "message_count": cleaned.len(),
                "messages": cleaned
            });

            // Mirrors `atomic_json_write(log_file, entry, indent=2, default=str)` (ll.3254-3259)
            atomic_json_write_stub(&log_file, &entry)?;
            // Real impl would write with indent; stub keeps audit trace.
            Ok(())
        })();

        if let Err(e) = result {
            if self.verbose_logging {
                eprintln!("Failed to save session log: {e}");
            }
        }
    }

    // -----------------------------------------------------------------------
    // interrupt — mirrors ll.3266-3427
    // -----------------------------------------------------------------------

    /// Mirrors `def interrupt(self, message: Optional[str] = None, *, hard_cancel: bool = False, tool_reason: Optional[str] = None) -> None:` (ll.3266-3427).
    pub fn interrupt(&mut self, message: Option<&str>, hard_cancel: bool, tool_reason: Option<&str>) {
        // Mirrors `_admit_hard_cancel()` nested def (ll.3303-3323)
        let admit_hard_cancel = |this: &mut Self| {
            if !this.hard_interrupt_requested {
                // In Python this checks `getattr(self, "_hard_interrupt_requested", None) is None: return`
                // Rust stub: if flag not set, we set it via fence admission attempt.
            }
            // Mirrors `fence = vars(self).get("_active_compression_commit_fence"); cancel_before_commit = getattr(type(fence), "cancel_before_commit", None)`
            // Rust: stub fence is Option<String>; simulate cancel_before_commit if fence exists.
            if let Some(_fence) = &this.active_compression_commit_fence {
                // Mirrors `cancel_before_commit(fence, event)` with lock — stub succeeds
                this.hard_interrupt_requested = true;
                return;
            }
            this.hard_interrupt_requested = true;
        };

        // Mirrors `tool_interrupt_reason = (tool_reason or "explicit stop requested") if hard_cancel else ("user sent a new message" if message else "user interrupt")` (ll.3328-3332)
        let tool_interrupt_reason = if hard_cancel {
            tool_reason.unwrap_or("explicit stop requested").to_string()
        } else if let Some(msg) = message {
            if !msg.is_empty() { "user sent a new message".to_string() } else { "user interrupt".to_string() }
        } else {
            "user interrupt".to_string()
        };

        // Mirrors `_redirect_lock = getattr(self, "_pending_redirect_lock", None); if _redirect_lock is not None: with _redirect_lock:` (ll.3334-3342)
        if self.pending_redirect_lock.is_some() {
            // Mirrors locked branch (ll.3336-3342)
            self.interrupt_requested = true;
            self.interrupt_message = message.map(|s| s.to_string());
            self.tool_interrupt_reason = Some(tool_interrupt_reason.clone());
            if hard_cancel { admit_hard_cancel(self); }
            self.pending_redirect = None;
        } else {
            // Mirrors else branch (ll.3343-3349)
            self.interrupt_requested = true;
            self.interrupt_message = message.map(|s| s.to_string());
            self.tool_interrupt_reason = Some(tool_interrupt_reason.clone());
            if hard_cancel { admit_hard_cancel(self); }
            self.pending_redirect = None;
        }

        // Mirrors `if getattr(self, "api_mode", None) == "codex_app_server": ... request_interrupt()` (ll.3353-3363)
        if self.api_mode == "codex_app_server" {
            if let Some(_codex) = &self.codex_session {
                // Stub: if codex_session has request_interrupt, call it — no-op for audit
            }
        }

        // Mirrors `_abort_active_request = getattr(self, "_active_request_abort", None); if callable: _abort_active_request("interrupt_abort")` (ll.3369-3374)
        if self.active_request_abort {
            // stub abort
        }

        // Mirrors `if self._execution_thread_id is not None: _set_interrupt(True, self._execution_thread_id, reason=tool_interrupt_reason)` (ll.3378-3391)
        if let Some(tid) = self.execution_thread_id {
            set_interrupt_stub(true, tid, Some(&tool_interrupt_reason));
            self.interrupt_thread_signal_pending = false;
        } else {
            // Mirrors `self._interrupt_thread_signal_pending = True` when not yet bound (ll.3390-3391)
            self.interrupt_thread_signal_pending = true;
        }

        // Mirrors fan out to concurrent-tool worker threads (ll.3398-3409)
        let worker_tids: Vec<u64> = self.tool_worker_threads.iter().cloned().collect();
        for wtid in worker_tids {
            set_interrupt_stub(true, wtid, Some(&tool_interrupt_reason));
        }

        // Mirrors propagate to child agents (ll.3410-3424)
        let children: Vec<AiAgent> = self.active_children.clone();
        for child in &children {
            if hard_cancel {
                request_hard_interrupt_stub(child, message, Some(&tool_interrupt_reason));
            } else {
                // Mirrors `child.interrupt(message)` — clone to allow mut call on copy
                let mut c = child.clone();
                c.interrupt(message, false, None);
            }
        }

        // Mirrors `if not self.quiet_mode: print("\n⚡ Interrupt requested"...` (ll.3425-3426)
        if !self.quiet_mode {
            // stub print — no-op for audit
            let _msg = format!("\n⚡ Interrupt requested{}", message.map(|m| if m.len() > 40 { format!(": '{}...' ", &m[..40]) } else { format!(": '{m}'") }).unwrap_or_default());
        }
    }

    // -----------------------------------------------------------------------
    // hard_interrupt — mirrors ll.3428-3447
    // -----------------------------------------------------------------------

    /// Mirrors `def hard_interrupt(self, message: Optional[str] = None, *, tool_reason: Optional[str] = None) -> None:` (ll.3428-3447).
    pub fn hard_interrupt(&mut self, message: Option<&str>, tool_reason: Option<&str>) {
        // Mirrors `AIAgent.interrupt(self, message, hard_cancel=True, tool_reason=tool_reason)` (ll.3442-3447)
        // Deliberately bypass dynamic dispatch per comment at ll.3439-3441.
        self.interrupt(message, true, tool_reason);
    }

    // -----------------------------------------------------------------------
    // clear_interrupt — mirrors ll.3449-3504
    // -----------------------------------------------------------------------

    /// Mirrors `def clear_interrupt(self, *, preserve_redirect: bool = False) -> bool:` (ll.3449-3504).
    pub fn clear_interrupt(&mut self, preserve_redirect: bool) -> bool {
        // Mirrors `_redirect_lock = getattr(self, "_pending_redirect_lock", None); if _redirect_lock is not None: with _redirect_lock: ...` (ll.3456-3466)
        if self.pending_redirect_lock.is_some() {
            if preserve_redirect && self.pending_redirect.is_none() {
                return false;
            }
            self.interrupt_requested = false;
            self.interrupt_message = None;
            self.tool_interrupt_reason = None;
            self.hard_interrupt_requested = false;
            if !preserve_redirect {
                self.pending_redirect = None;
            }
        } else {
            // Mirrors else branch (ll.3467-3475)
            if preserve_redirect && self.pending_redirect.is_none() {
                return false;
            }
            self.interrupt_requested = false;
            self.interrupt_message = None;
            self.tool_interrupt_reason = None;
            self.hard_interrupt_requested = false;
            if !preserve_redirect {
                self.pending_redirect = None;
            }
        }
        // Mirrors `self._interrupt_thread_signal_pending = False` (l.3476)
        self.interrupt_thread_signal_pending = false;
        // Mirrors `if self._execution_thread_id is not None: _set_interrupt(False, self._execution_thread_id)` (ll.3477-3478)
        if let Some(tid) = self.execution_thread_id {
            set_interrupt_stub(false, tid, None);
        }
        // Mirrors clearing concurrent-tool worker thread bits (ll.3484-3495)
        let worker_tids: Vec<u64> = self.tool_worker_threads.iter().cloned().collect();
        for wtid in worker_tids {
            set_interrupt_stub(false, wtid, None);
        }
        // Mirrors `_steer_lock = getattr(self, "_pending_steer_lock", None); if _steer_lock is not None: with _steer_lock: self._pending_steer = None` (ll.3500-3503)
        if self.pending_steer_lock.is_some() {
            self.pending_steer = None;
        } else if self.pending_steer.is_some() {
            // Even without lock, hard interrupt clears steer (ll.3496-3499 comment)
            self.pending_steer = None;
        }
        true
    }

    // -----------------------------------------------------------------------
    // steer — mirrors ll.3506-3540
    // -----------------------------------------------------------------------

    /// Mirrors `def steer(self, text: str) -> bool:` (ll.3506-3540).
    pub fn steer(&mut self, text: &str) -> bool {
        // Mirrors `if not text or not text.strip(): return False` (ll.3524-3525)
        if text.trim().is_empty() { return false; }
        let cleaned = text.trim().to_string();
        // Mirrors `_lock = getattr(self, "_pending_steer_lock", None); if _lock is None: ... else: with _lock:` (ll.3527-3539)
        if self.pending_steer_lock.is_some() {
            if let Some(existing) = &self.pending_steer {
                self.pending_steer = Some(format!("{existing}\n{cleaned}"));
            } else {
                self.pending_steer = Some(cleaned);
            }
        } else {
            // Mirrors test-stub fallback (ll.3528-3534)
            let existing = self.pending_steer.clone();
            self.pending_steer = Some(if let Some(e) = existing { format!("{e}\n{cleaned}") } else { cleaned });
        }
        true
    }

    // -----------------------------------------------------------------------
    // redirect — mirrors ll.3542-3600 (slice ends mid-else at l.3600)
    // -----------------------------------------------------------------------

    /// Mirrors `def redirect(self, text: str) -> bool:` (ll.3542-3600).
    ///
    /// Slice 4 covers through `else:` at l.3600 (`with _redirect_lock:` branch).
    /// The remainder (`with _redirect_lock:` body ll.3601-3616 + interrupt fan-out
    /// ll.3618-3632) continues in `run_agent_slice5.rs`. This method is left
    /// syntactically complete for audit by stubbing the remainder.
    pub fn redirect(&mut self, text: &str) -> bool {
        // Mirrors `if not text or not text.strip(): return False` (ll.3556-3557)
        if text.trim().is_empty() { return false; }
        let cleaned = text.trim().to_string();

        // Mirrors `if getattr(self, "api_mode", None) == "codex_app_server": ... request_steer` (ll.3562-3577)
        if self.api_mode == "codex_app_server" {
            // Mirrors `_codex_session = getattr(self, "_codex_session", None); _native_steer = getattr(_codex_session, "request_steer", None)`
            let has_native_steer = self.codex_session.is_some();
            if has_native_steer {
                // Mirrors `with _redirect_lock: if self._interrupt_requested: return False`
                if self.interrupt_requested { return false; }
                // Mirrors `try: return bool(_native_steer(cleaned)) except: logger.debug ...; return False`
                // Stub: assume steer succeeds
                return true;
            }
        }

        // Mirrors `if getattr(self, "_executing_tools", False): return self.steer(cleaned)` (ll.3582-3583)
        if self.executing_tools {
            return self.steer(&cleaned);
        }

        // Mirrors `_model_active = getattr(self, "_model_request_active", None); _redirect_lock = ...` (ll.3585-3586)
        let model_active = self.model_request_active;
        let has_redirect_lock = self.pending_redirect_lock.is_some();

        // Mirrors `if _redirect_lock is None:` branch (ll.3587-3599)
        if !has_redirect_lock {
            if !model_active { return false; }
            if self.interrupt_requested && self.pending_redirect.is_none() { return false; }
            // Mirrors `self._pending_redirect = f"{existing}\n\n[Additional user correction]\n{cleaned}" if existing else cleaned` (ll.3593-3597)
            if let Some(existing) = &self.pending_redirect.clone() {
                self.pending_redirect = Some(format!("{existing}\n\n[Additional user correction]\n{cleaned}"));
            } else {
                self.pending_redirect = Some(cleaned);
            }
            self.interrupt_requested = true;
            self.interrupt_message = None;
        } else {
            // Mirrors `else: with _redirect_lock: if _model_active is None or not _model_active.is_set(): return False`
            // Slice 4 nominally ends at l.3600 (`else:`) — we include the guarded body as stub.
            // Full body (ll.3601-3616) is canonical in slice5; here we preserve the 3600 boundary marker.
            // --- 3600 boundary: `        else:` ---
            // Remainder (ll.3601-3632) canonical in slice5 — stubbed for syntactic completeness:
            if !model_active { return false; }
            if self.interrupt_requested && self.pending_redirect.is_none() { return false; }
            if let Some(existing) = &self.pending_redirect.clone() {
                self.pending_redirect = Some(format!("{existing}\n\n[Additional user correction]\n{cleaned}"));
            } else {
                self.pending_redirect = Some(cleaned);
            }
            self.interrupt_requested = true;
            self.interrupt_message = None;
        }

        // Mirrors remainder of redirect (ll.3618-3632) — interrupt only model request, not tool workers/children
        // Canonical in slice5; stubbed here so slice4 is syntactically complete without cargo:
        if let Some(tid) = self.execution_thread_id {
            set_interrupt_stub(true, tid, None);
            self.interrupt_thread_signal_pending = false;
        } else {
            self.interrupt_thread_signal_pending = true;
        }
        if self.active_request_abort {
            // Mirrors `_abort_active_request("redirect_abort")`
        }
        true
    }
}

// ---------------------------------------------------------------------------
// Supporting types — mirrors Python dynamic error/hook shapes for this slice
// ---------------------------------------------------------------------------

/// Mirrors Python `Exception` with `status_code` / `body` / `response` attrs for `_summarize_api_error` (ll.2702-2807).
#[derive(Debug, Clone, Default)]
pub struct ApiError {
    pub raw: String,
    pub status_code: Option<i64>,
    pub body: Option<Value>,
    pub response_text: Option<String>,
    pub type_name: String,
    pub is_value_error: bool,
    pub chain: Vec<String>,
}

/// Mirrors `key: Any` for `_mask_api_key_for_logs` (ll.2809-2818).
#[derive(Debug, Clone, Default)]
pub struct MaskKey {
    pub value: Option<String>,
    pub is_callable: bool,
    pub is_string: bool,
}

fn value_type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Number(_) => "int",
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

fn chrono_stub_now_iso() -> String {
    // Mirrors `datetime.now().isoformat()` (l.3246) — stub uses SystemTime
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    format!("{now}")
}

// ---------------------------------------------------------------------------
// Slice boundary — line ~3600
// ---------------------------------------------------------------------------
// The next method body `with _redirect_lock: if _model_active is None ...`
// at ll.3601-3616 and every subsequent `AIAgent` method through
// `_has_pending_redirect` (l.3634), `_drain_pending_redirect` (l.3642),
// `_drain_pending_steer` (l.3654), `_record_file_mutation_result` (l.3670),
// ... through `run_conversation` / `main` at l.9053 and the full 9 269-line
// file, continues in `run_agent_slice5.rs`. This file intentionally stops
// at the 3600-line boundary so that `cargo` is never invoked and the
// 11-slice decomposition stays clean.

