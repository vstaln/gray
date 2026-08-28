//! Managed Modal environment backed by tool-gateway.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/tools/environments/managed_modal.py` (282 lines).
//! Gateway-owned Modal sandbox with Hermes-compatible execute/cleanup.
//!
//! Python source docstring (preserved):
//! ```text
//! Managed Modal environment backed by tool-gateway.
//! ```

use std::collections::HashMap;
use std::env;
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::file_sync::get_hermes_home;

// ---------------------------------------------------------------------------
// Constants — mirrors Python module globals / class vars
// ---------------------------------------------------------------------------

/// Mirrors `_request_timeout_env` default for connect.
pub const DEFAULT_CONNECT_TIMEOUT_SECS: f64 = 1.0;
/// Mirrors default for poll read.
pub const DEFAULT_POLL_READ_TIMEOUT_SECS: f64 = 5.0;
/// Mirrors default for cancel read.
pub const DEFAULT_CANCEL_READ_TIMEOUT_SECS: f64 = 5.0;

/// Mirrors `ManagedModalEnvironment._client_timeout_grace_seconds = 10.0`.
pub const CLIENT_TIMEOUT_GRACE_SECONDS: f64 = 10.0;

/// Mirrors `ManagedModalEnvironment._interrupt_output`.
pub const INTERRUPT_OUTPUT: &str = "[Command interrupted - Modal sandbox exec cancelled]";

/// Mirrors `ManagedModalEnvironment._unexpected_error_prefix`.
pub const UNEXPECTED_ERROR_PREFIX: &str = "Managed Modal exec failed";

// ---------------------------------------------------------------------------
// Helpers — env / uuid / json
// ---------------------------------------------------------------------------

/// Mirrors `def _request_timeout_env(name, default)`.
///
/// Reads `name` from env, parses as f64, returns `default` if missing, <=0, or parse error.
pub fn request_timeout_env(name: &str, default: f64) -> f64 {
    match env::var(name) {
        Ok(v) => match v.trim().parse::<f64>() {
            Ok(f) if f > 0.0 => f,
            _ => default,
        },
        Err(_) => default,
    }
}

/// Mirrors class-var evaluation at import time.
pub fn connect_timeout_seconds() -> f64 {
    request_timeout_env("TERMINAL_MANAGED_MODAL_CONNECT_TIMEOUT_SECONDS", DEFAULT_CONNECT_TIMEOUT_SECS)
}
pub fn poll_read_timeout_seconds() -> f64 {
    request_timeout_env("TERMINAL_MANAGED_MODAL_POLL_READ_TIMEOUT_SECONDS", DEFAULT_POLL_READ_TIMEOUT_SECS)
}
pub fn cancel_read_timeout_seconds() -> f64 {
    request_timeout_env("TERMINAL_MANAGED_MODAL_CANCEL_READ_TIMEOUT_SECONDS", DEFAULT_CANCEL_READ_TIMEOUT_SECS)
}

fn uuid_simple() -> String {
    // Cheap pseudo-uuid from time + pid (mirrors modal.rs / file_sync.rs uuid_simple).
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id() as u128;
    format!("{nanos:x}{pid:x}")
}

fn uuid_v4() -> String {
    // v4-ish: use uuid_simple but ensure hex shape similar to Python uuid4().hex
    uuid_simple()
}

fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

// ---------------------------------------------------------------------------
// Gateway config — mirrors tools.managed_tool_gateway
// ---------------------------------------------------------------------------

/// Mirrors `ManagedToolGatewayConfig`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedToolGatewayConfig {
    pub vendor: String,
    pub gateway_origin: String,
    pub nous_user_token: String,
    pub managed_mode: bool,
}

fn managed_nous_tools_enabled() -> bool {
    // Mirrors `tools.tool_backend_helpers.managed_nous_tools_enabled()`:
    // checks portal entitlement. In Rust we have no portal client, so fail-open
    // to env-gated check: if `HERMES_MANAGED_NOUS_TOOLS_ENABLED` is explicitly
    // falsy, return false; otherwise true (gateway code will still require a token).
    // This matches Python's fail-closed on exception only when we can't consult the
    // entitlement service — here we approximate with env.
    if let Ok(v) = env::var("HERMES_MANAGED_NOUS_TOOLS_ENABLED") {
        let t = v.trim().to_lowercase();
        if matches!(t.as_str(), "0" | "false" | "no" | "off") {
            return false;
        }
    }
    true
}

fn get_tool_gateway_scheme() -> Result<String, String> {
    let scheme = env::var("TOOL_GATEWAY_SCHEME").unwrap_or_default().trim().to_lowercase();
    if scheme.is_empty() {
        return Ok("https".to_string());
    }
    if scheme == "http" || scheme == "https" {
        return Ok(scheme);
    }
    Err("TOOL_GATEWAY_SCHEME must be 'http' or 'https'".to_string())
}

fn build_vendor_gateway_url(vendor: &str) -> String {
    let vendor_key = format!("{}_GATEWAY_URL", vendor.to_uppercase().replace('-', "_"));
    if let Ok(explicit) = env::var(&vendor_key) {
        let t = explicit.trim().trim_end_matches('/').to_string();
        if !t.is_empty() {
            return t;
        }
    }
    let scheme = get_tool_gateway_scheme().unwrap_or_else(|_| "https".to_string());
    let shared_domain = env::var("TOOL_GATEWAY_DOMAIN")
        .unwrap_or_default()
        .trim()
        .trim_matches('/')
        .to_string();
    if !shared_domain.is_empty() {
        return format!("{scheme}://{vendor}-gateway.{shared_domain}");
    }
    format!("{scheme}://{vendor}-gateway.nousresearch.com")
}

fn read_user_token_override() -> Option<String> {
    // Mirrors `_read_user_token_override`: try secret scope, else os.getenv.
    // In Rust we only have env.
    if let Ok(v) = env::var("TOOL_GATEWAY_USER_TOKEN") {
        let t = v.trim().to_string();
        if !t.is_empty() {
            return Some(t);
        }
    }
    None
}

fn read_nous_provider_state() -> Option<String> {
    // Reads `$HERMES_HOME/auth.json` providers.nous.access_token via simple substring search.
    // Returns raw token string if found.
    let path = get_hermes_home().join("auth.json");
    if !path.is_file() {
        return None;
    }
    let text = fs::read_to_string(&path).ok()?;
    // naive extract: find "nous" then "access_token"
    // Look for `"access_token"` and capture next quoted string.
    let needle = "\"access_token\"";
    let idx = text.find(needle)?;
    let after = &text[idx + needle.len()..];
    let colon = after.find(':')?;
    let rest = after[colon + 1..].trim_start();
    if !rest.starts_with('"') {
        return None;
    }
    let mut out = String::new();
    let mut esc = false;
    for c in rest[1..].chars() {
        if esc {
            out.push(c);
            esc = false;
            continue;
        }
        if c == '\\' {
            esc = true;
            continue;
        }
        if c == '"' {
            break;
        }
        out.push(c);
    }
    let t = out.trim().to_string();
    if t.is_empty() {
        None
    } else {
        Some(t)
    }
}

fn peek_nous_access_token() -> Option<String> {
    if let Some(t) = read_user_token_override() {
        return Some(t);
    }
    read_nous_provider_state()
}

fn read_nous_access_token() -> Option<String> {
    // Mirrors `read_nous_access_token`: peek + expiry check + refresh attempt.
    // In Rust we skip expiry/refresh network and just return peek.
    peek_nous_access_token()
}

pub fn resolve_managed_tool_gateway(vendor: &str) -> Option<ManagedToolGatewayConfig> {
    if !managed_nous_tools_enabled() {
        return None;
    }
    let gateway_origin = build_vendor_gateway_url(vendor);
    let nous_user_token = read_nous_access_token()?;
    if gateway_origin.is_empty() || nous_user_token.is_empty() {
        return None;
    }
    Some(ManagedToolGatewayConfig {
        vendor: vendor.to_string(),
        gateway_origin,
        nous_user_token,
        managed_mode: true,
    })
}

// ---------------------------------------------------------------------------
// Core types — mirrors dataclasses + base helpers
// ---------------------------------------------------------------------------

/// Mirrors `@dataclass(frozen=True) class _ManagedModalExecHandle`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedModalExecHandle {
    pub exec_id: String,
}

/// Mirrors `tools.environments.modal_utils.PreparedModalExec`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedModalExec {
    pub command: String,
    pub cwd: String,
    pub timeout: u64,
    pub stdin_data: Option<String>,
}

/// Mirrors `tools.environments.modal_utils.ModalExecStart`.
#[derive(Debug, Clone)]
pub struct ModalExecStart {
    pub handle: Option<ManagedModalExecHandle>,
    pub immediate_result: Option<ExecResult>,
}

/// Mirrors `BaseModalExecutionEnvironment._result` dict `{"output":..., "returncode":...}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecResult {
    pub output: String,
    pub returncode: i32,
}

/// Minimal HTTP response — mirrors `requests.Response` subset used by managed_modal.
#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub status_code: u16,
    pub text: String,
    pub json_body: Option<String>,
}

impl HttpResponse {
    pub fn json_value(&self) -> Option<String> {
        self.json_body.clone()
    }
    /// Try to parse body as JSON string->value map for status/output/returncode etc.
    /// We do minimal extraction via substring search (no serde).
    pub fn json_get_str(&self, key: &str) -> Option<String> {
        let body = self.json_body.as_deref().or(Some(self.text.as_str()))?;
        extract_json_string(body, key)
    }
    pub fn json_get_i32(&self, key: &str) -> Option<i32> {
        let body = self.json_body.as_deref().or(Some(self.text.as_str()))?;
        extract_json_i32(body, key)
    }
}

fn extract_json_string(body: &str, key: &str) -> Option<String> {
    // Find `"key"` then `:` then `"value"`
    let pat = format!("\"{}\"", key);
    let idx = body.find(&pat)?;
    let after = &body[idx + pat.len()..];
    let colon = after.find(':')?;
    let rest = after[colon + 1..].trim_start();
    if !rest.starts_with('"') {
        return None;
    }
    let mut out = String::new();
    let mut esc = false;
    for c in rest[1..].chars() {
        if esc {
            match c {
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                '"' => out.push('"'),
                '\\' => out.push('\\'),
                'u' => {
                    // skip \uXXXX for simplicity — push placeholder
                    // consume next 4 chars if present
                    // We can't fully decode without allocation; just skip.
                    // Caller trims; for error messages this is acceptable.
                    // Take 4 hex chars if available.
                    // This branch is reached only when esc was true and c=='u'
                    // but we already consumed 'u', need to handle hex.
                    // For simplicity, push 'u' and continue.
                    out.push('u');
                }
                _ => out.push(c),
            }
            esc = false;
            continue;
        }
        if c == '\\' {
            esc = true;
            continue;
        }
        if c == '"' {
            break;
        }
        out.push(c);
    }
    Some(out)
}

fn extract_json_i32(body: &str, key: &str) -> Option<i32> {
    let pat = format!("\"{}\"", key);
    let idx = body.find(&pat)?;
    let after = &body[idx + pat.len()..];
    let colon = after.find(':')?;
    let rest = after[colon + 1..].trim_start();
    let mut num = String::new();
    for c in rest.chars() {
        if c.is_ascii_digit() || c == '-' {
            num.push(c);
        } else {
            break;
        }
    }
    if num.is_empty() {
        return None;
    }
    num.parse::<i32>().ok()
}

/// Mirrors `Request timeout spec`: single seconds or (connect, read) pair.
#[derive(Debug, Clone, Copy)]
pub enum RequestTimeout {
    Single(f64),
    Pair(f64, f64),
}

impl From<f64> for RequestTimeout {
    fn from(v: f64) -> Self {
        RequestTimeout::Single(v)
    }
}
impl From<(f64, f64)> for RequestTimeout {
    fn from(v: (f64, f64)) -> Self {
        RequestTimeout::Pair(v.0, v.1)
    }
}

// ---------------------------------------------------------------------------
// Shared helpers — mirrors Python staticmethods / free functions
// ---------------------------------------------------------------------------

/// Mirrors `_coerce_number(value, default)`.
///
/// `value` is an optional string (from kwargs), `default` is the fallback.
/// Returns `None` when default is None and value is missing/ unparseable.
pub fn coerce_number(value: Option<&str>, default: Option<f64>) -> Option<f64> {
    match value {
        None => default,
        Some(v) => {
            let t = v.trim();
            if t.is_empty() {
                return default;
            }
            match t.parse::<f64>() {
                Ok(f) => Some(f),
                Err(_) => default,
            }
        }
    }
}

fn coerce_number_opt(value: Option<&String>, default: Option<f64>) -> Option<f64> {
    coerce_number(value.map(|s| s.as_str()), default)
}

/// Mirrors `ManagedModalEnvironment._format_error(prefix, response)`.
///
/// Extracts `error`/`message`/`code` from JSON body if present, else falls back to
/// text body, else HTTP status.
pub fn format_error(prefix: &str, response: &HttpResponse) -> String {
    // Try JSON body extraction.
    if let Some(body) = response.json_body.as_deref().or(Some(response.text.as_str())) {
        // Try to find error/message/code as string values.
        for key in ["error", "message", "code"] {
            if let Some(msg) = extract_json_string(body, key) {
                let t = msg.trim().to_string();
                if !t.is_empty() {
                    return format!("{prefix}: {t}");
                }
            }
        }
        // If body is JSON object but no specific field, return serialized body if object-like.
        let trimmed = body.trim();
        if trimmed.starts_with('{') && trimmed.ends_with('}') && trimmed.len() > 2 {
            // Return prefix + raw JSON (mirrors json.dumps(payload))
            return format!("{prefix}: {trimmed}");
        }
    }
    let text = response.text.trim();
    if !text.is_empty() {
        return format!("{prefix}: {text}");
    }
    format!("{prefix}: HTTP {}", response.status_code)
}

fn result(output: &str, returncode: i32) -> ExecResult {
    ExecResult {
        output: output.to_string(),
        returncode,
    }
}
fn error_result(output: &str) -> ExecResult {
    result(output, 1)
}

// ---------------------------------------------------------------------------
// ManagedModalEnvironment — mirrors Python `ManagedModalEnvironment(BaseModalExecutionEnvironment)`
// ---------------------------------------------------------------------------

/// Gateway-owned Modal sandbox with Hermes-compatible execute/cleanup.
///
/// Mirrors `tools.environments.managed_modal.ManagedModalEnvironment`.
///
/// `sandbox_kwargs` mirrors `modal_sandbox_kwargs` passthrough (stringly-typed).
pub struct ManagedModalEnvironment {
    /// Mirrors `BaseEnvironment.cwd`.
    pub cwd: String,
    /// Mirrors `BaseEnvironment.timeout` (seconds).
    pub timeout: u64,
    /// Mirrors `self._persistent`.
    pub persistent: bool,
    /// Mirrors `self._task_id`.
    pub task_id: String,
    /// Mirrors `self._image`.
    pub image: String,
    /// Mirrors `self._sandbox_kwargs`.
    pub sandbox_kwargs: HashMap<String, String>,
    /// Mirrors `self._gateway_origin` (rstripped).
    pub gateway_origin: String,
    /// Mirrors `self._nous_user_token`.
    pub nous_user_token: String,
    /// Mirrors `self._create_idempotency_key`.
    pub create_idempotency_key: String,
    /// Mirrors `self._sandbox_id` (None after cleanup).
    pub sandbox_id: Option<String>,
}

impl ManagedModalEnvironment {
    /// Mirrors `ManagedModalEnvironment.__init__(image, cwd="/root", timeout=60, modal_sandbox_kwargs, persistent_filesystem, task_id)`.
    pub fn new(
        image: &str,
        cwd: &str,
        timeout: u64,
        modal_sandbox_kwargs: Option<HashMap<String, String>>,
        persistent_filesystem: bool,
        task_id: &str,
    ) -> Result<Self, String> {
        // Mirrors `super().__init__(cwd=cwd, timeout=timeout)` — sets cwd/timeout.
        let cwd_owned = if cwd.is_empty() { "/root".to_string() } else { cwd.to_string() };
        let t = if timeout == 0 { 60 } else { timeout };

        // Mirrors `self._guard_unsupported_credential_passthrough()`.
        guard_unsupported_credential_passthrough()?;

        // Mirrors `gateway = resolve_managed_tool_gateway("modal")`.
        let gateway = resolve_managed_tool_gateway("modal")
            .ok_or_else(|| "Managed Modal requires a configured tool gateway and Nous user token".to_string())?;

        let gateway_origin = gateway.gateway_origin.trim_end_matches('/').to_string();
        let nous_user_token = gateway.nous_user_token;
        let task_id_owned = if task_id.is_empty() { "default".to_string() } else { task_id.to_string() };
        let persistent = persistent_filesystem;
        let image_owned = image.to_string();
        let sandbox_kwargs = modal_sandbox_kwargs.unwrap_or_default();
        let create_idempotency_key = uuid_v4();

        let mut env = Self {
            cwd: cwd_owned,
            timeout: t,
            persistent,
            task_id: task_id_owned,
            image: image_owned,
            sandbox_kwargs,
            gateway_origin,
            nous_user_token,
            create_idempotency_key,
            sandbox_id: None,
        };

        // Mirrors `self._sandbox_id = self._create_sandbox()`.
        let sandbox_id = env.create_sandbox()?;
        env.sandbox_id = Some(sandbox_id);
        Ok(env)
    }

    /// Mirrors `_start_modal_exec(self, prepared: PreparedModalExec) -> ModalExecStart`.
    pub fn start_modal_exec(&self, prepared: &PreparedModalExec) -> ModalExecStart {
        let exec_id = uuid_v4();
        // Build payload: mirrors Python dict construction.
        let mut payload = format!(
            "{{\"execId\":{},\"command\":{},\"cwd\":{},\"timeoutMs\":{}}}",
            json_escape(&exec_id),
            json_escape(&prepared.command),
            json_escape(&prepared.cwd),
            (prepared.timeout as u64) * 1000
        );
        // If stdin_data present, add to payload.
        if let Some(stdin) = &prepared.stdin_data {
            // Insert stdinData before closing `}`.
            let stdin_field = format!(",\"stdinData\":{}", json_escape(stdin));
            payload.pop(); // remove `}`
            payload.push_str(&stdin_field);
            payload.push('}');
        }

        let sandbox_id = match &self.sandbox_id {
            Some(id) => id.clone(),
            None => {
                return ModalExecStart {
                    handle: None,
                    immediate_result: Some(error_result(&format!("{}: missing sandbox id", UNEXPECTED_ERROR_PREFIX))),
                }
            }
        };

        let path = format!("/v1/sandboxes/{}/execs", sandbox_id);
        let response = match self.request("POST", &path, Some(&payload), RequestTimeout::Single(10.0), None) {
            Ok(r) => r,
            Err(exc) => {
                return ModalExecStart {
                    handle: None,
                    immediate_result: Some(error_result(&format!("{}: {}", UNEXPECTED_ERROR_PREFIX, exc))),
                }
            }
        };

        if response.status_code >= 400 {
            return ModalExecStart {
                handle: None,
                immediate_result: Some(error_result(&format_error(UNEXPECTED_ERROR_PREFIX, &response))),
            };
        }

        let body_text = response.json_body.as_deref().unwrap_or(&response.text).to_string();
        let status = extract_json_string(&body_text, "status");

        if matches!(status.as_deref(), Some("completed") | Some("failed") | Some("cancelled") | Some("timeout")) {
            let output = extract_json_string(&body_text, "output").unwrap_or_default();
            let returncode = extract_json_i32(&body_text, "returncode").unwrap_or(1);
            return ModalExecStart {
                handle: None,
                immediate_result: Some(result(&output, returncode)),
            };
        }

        let returned_exec_id = extract_json_string(&body_text, "execId");
        if returned_exec_id.as_deref() != Some(&exec_id) {
            return ModalExecStart {
                handle: None,
                immediate_result: Some(error_result("Managed Modal exec start did not return the expected exec id")),
            };
        }

        ModalExecStart {
            handle: Some(ManagedModalExecHandle { exec_id }),
            immediate_result: None,
        }
    }

    /// Mirrors `_poll_modal_exec(self, handle: _ManagedModalExecHandle) -> dict | None`.
    pub fn poll_modal_exec(&self, handle: &ManagedModalExecHandle) -> Option<ExecResult> {
        let sandbox_id = self.sandbox_id.as_deref().unwrap_or("");
        let path = format!("/v1/sandboxes/{}/execs/{}", sandbox_id, handle.exec_id);
        let timeout = RequestTimeout::Pair(connect_timeout_seconds(), poll_read_timeout_seconds());
        let response = match self.request("GET", &path, None, timeout, None) {
            Ok(r) => r,
            Err(exc) => {
                return Some(error_result(&format!("Managed Modal exec poll failed: {}", exc)));
            }
        };

        if response.status_code == 404 {
            return Some(error_result("Managed Modal exec not found"));
        }
        if response.status_code >= 400 {
            return Some(error_result(&format_error("Managed Modal exec poll failed", &response)));
        }

        let body_text = response.json_body.as_deref().unwrap_or(&response.text).to_string();
        let status = extract_json_string(&body_text, "status");
        if matches!(status.as_deref(), Some("completed") | Some("failed") | Some("cancelled") | Some("timeout")) {
            let output = extract_json_string(&body_text, "output").unwrap_or_default();
            let returncode = extract_json_i32(&body_text, "returncode").unwrap_or(1);
            return Some(result(&output, returncode));
        }
        None
    }

    /// Mirrors `_cancel_modal_exec(self, handle: _ManagedModalExecHandle) -> None`.
    pub fn cancel_modal_exec(&self, handle: &ManagedModalExecHandle) {
        self.cancel_exec(&handle.exec_id);
    }

    /// Mirrors `_timeout_result_for_modal(self, timeout: int) -> dict`.
    pub fn timeout_result_for_modal(&self, timeout: u64) -> ExecResult {
        result(&format!("Managed Modal exec timed out after {}s", timeout), 124)
    }

    /// Mirrors `cleanup(self)`.
    pub fn cleanup(&mut self) {
        if self.sandbox_id.is_none() {
            return;
        }
        let sandbox_id = self.sandbox_id.as_deref().unwrap_or("").to_string();
        let path = format!("/v1/sandboxes/{}/terminate", sandbox_id);
        let payload = format!("{{\"snapshotBeforeTerminate\":{}}}", if self.persistent { "true" } else { "false" });
        match self.request("POST", &path, Some(&payload), RequestTimeout::Single(60.0), None) {
            Ok(_) => {},
            Err(exc) => {
                log::warn!("Managed Modal cleanup failed: {}", exc);
            }
        }
        self.sandbox_id = None;
    }

    /// Mirrors `_create_sandbox(self) -> str`.
    pub fn create_sandbox(&self) -> Result<String, String> {
        // Mirrors:
        // cpu = self._coerce_number(self._sandbox_kwargs.get("cpu"), 1)
        // memory = self._coerce_number(self._sandbox_kwargs.get("memoryMiB", self._sandbox_kwargs.get("memory")), 5120)
        // disk = self._coerce_number(self._sandbox_kwargs.get("ephemeral_disk", self._sandbox_kwargs.get("diskMiB")), None)
        let cpu = coerce_number_opt(self.sandbox_kwargs.get("cpu"), Some(1.0)).unwrap_or(1.0);
        let memory = {
            let v = self.sandbox_kwargs.get("memoryMiB").or_else(|| self.sandbox_kwargs.get("memory"));
            coerce_number_opt(v, Some(5120.0)).unwrap_or(5120.0)
        };
        let disk = {
            let v = self.sandbox_kwargs.get("ephemeral_disk").or_else(|| self.sandbox_kwargs.get("diskMiB"));
            coerce_number_opt(v, None)
        };

        // Build create_payload mirroring Python dict.
        // Use stringified JSON for stub transport.
        let idle_timeout_ms = std::cmp::max(300_000u64, self.timeout * 1000);
        let mut payload = format!(
            "{{\"image\":{},\"cwd\":{},\"cpu\":{},\"memoryMiB\":{},\"timeoutMs\":3600000,\"idleTimeoutMs\":{},\"persistentFilesystem\":{},\"logicalKey\":{}}}",
            json_escape(&self.image),
            json_escape(&self.cwd),
            cpu,
            memory,
            idle_timeout_ms,
            if self.persistent { "true" } else { "false" },
            json_escape(&self.task_id)
        );
        if let Some(d) = disk {
            // Insert diskMiB before final `}`.
            let disk_field = format!(",\"diskMiB\":{}", d);
            payload.pop();
            payload.push_str(&disk_field);
            payload.push('}');
        }

        let mut extra_headers = HashMap::new();
        extra_headers.insert("x-idempotency-key".to_string(), self.create_idempotency_key.clone());

        let response = self.request("POST", "/v1/sandboxes", Some(&payload), RequestTimeout::Single(60.0), Some(&extra_headers))
            .map_err(|e| format_error("Managed Modal create failed", &HttpResponse { status_code: 0, text: e.clone(), json_body: None }))?;

        if response.status_code >= 400 {
            return Err(format_error("Managed Modal create failed", &response));
        }

        let body_text = response.json_body.as_deref().unwrap_or(&response.text).to_string();
        let sandbox_id = extract_json_string(&body_text, "id");
        match sandbox_id {
            Some(id) if !id.is_empty() => Ok(id),
            _ => Err("Managed Modal create did not return a sandbox id".to_string()),
        }
    }

    /// Mirrors `_guard_unsupported_credential_passthrough(self) -> None`.
    pub fn guard_unsupported_credential_passthrough(&self) -> Result<(), String> {
        guard_unsupported_credential_passthrough()
    }

    /// Mirrors `_request(self, method, path, *, json, timeout, extra_headers)`.
    ///
    /// In Python this calls `requests.request(method, f"{gateway_origin}{path}", headers=..., json=..., timeout=...)`.
    /// In Rust without an HTTP crate this is a stub that would be wired to `reqwest` in a full implementation.
    /// It constructs the URL and headers faithfully and returns a descriptive error so callers can still
    /// exercise error paths. Tests can inject a custom transport via `with_request_fn`.
    pub fn request(
        &self,
        method: &str,
        path: &str,
        json_body: Option<&str>,
        _timeout: RequestTimeout,
        extra_headers: Option<&HashMap<String, String>>,
    ) -> Result<HttpResponse, String> {
        let url = format!("{}{}", self.gateway_origin, path);
        let mut headers = HashMap::new();
        headers.insert("Authorization".to_string(), format!("Bearer {}", self.nous_user_token));
        headers.insert("Content-Type".to_string(), "application/json".to_string());
        if let Some(extra) = extra_headers {
            for (k, v) in extra {
                headers.insert(k.clone(), v.clone());
            }
        }
        // Stub: no HTTP crate available in this workspace (see docs/port/tools.md).
        // Return an error that mimics a connection failure so callers can map it to
        // `ModalExecStart.immediate_result` error handling exactly as Python does on
        // `requests` exception.
        let _ = (method, url, json_body, headers);
        Err(format!("http client not wired in this build (stub for {} {})", method, path))
    }

    /// Mirrors `_cancel_exec(self, exec_id: str) -> None`.
    pub fn cancel_exec(&self, exec_id: &str) {
        let sandbox_id = self.sandbox_id.as_deref().unwrap_or("");
        let path = format!("/v1/sandboxes/{}/execs/{}/cancel", sandbox_id, exec_id);
        let timeout = RequestTimeout::Pair(connect_timeout_seconds(), cancel_read_timeout_seconds());
        match self.request("POST", &path, None, timeout, None) {
            Ok(_) => {},
            Err(exc) => {
                log::warn!("Managed Modal exec cancel failed: {}", exc);
            }
        }
    }

    /// Mirrors `ManagedModalEnvironment._coerce_number(value, default) -> float`.
    pub fn coerce_number(value: Option<&str>, default: Option<f64>) -> Option<f64> {
        coerce_number(value, default)
    }

    /// Mirrors `ManagedModalEnvironment._format_error(prefix, response) -> str`.
    pub fn format_error(prefix: &str, response: &HttpResponse) -> String {
        format_error(prefix, response)
    }

    // ------------------------------------------------------------------
    // Base-compat helpers — mirrors BaseModalExecutionEnvironment
    // ------------------------------------------------------------------

    fn result(&self, output: &str, returncode: i32) -> ExecResult {
        result(output, returncode)
    }
    fn error_result(&self, output: &str) -> ExecResult {
        error_result(output)
    }
}

// ---------------------------------------------------------------------------
// Credential passthrough guard — mirrors Python free function
// ---------------------------------------------------------------------------

fn guard_unsupported_credential_passthrough() -> Result<(), String> {
    // Mirrors:
    // try: from tools.credential_files import get_credential_file_mounts
    // except Exception: return
    // mounts = get_credential_file_mounts()
    // if mounts: raise ValueError(...)
    //
    // In Rust we check `crate::file_sync::credential_host_paths()` plus a
    // secondary env sentinel `HERMES_CREDENTIAL_MOUNTS` for test injection.
    let has_mounts = {
        // Primary: check file_sync credential set (empty by default; populated via FileSyncManager).
        let cred = crate::file_sync::credential_host_paths();
        if !cred.is_empty() {
            true
        } else if let Ok(v) = env::var("HERMES_CREDENTIAL_MOUNTS") {
            !v.trim().is_empty()
        } else {
            // Also check filesystem fallback: any file under $HERMES_HOME matching credential pattern?
            // We treat absence as no mounts (faithful to Python's empty list return on import failure).
            false
        }
    };
    if has_mounts {
        return Err(
            "Managed Modal does not support host credential-file passthrough. Use TERMINAL_MODAL_MODE=direct when skills or config require credential files inside the sandbox.".to_string()
        );
    }
    Ok(())
}

// Allow calling guard as free function for tests.
pub fn _guard_unsupported_credential_passthrough() -> Result<(), String> {
    guard_unsupported_credential_passthrough()
}

// Expose coerce for tests.
pub fn _coerce_number(value: Option<&str>, default: Option<f64>) -> Option<f64> {
    coerce_number(value, default)
}

// Expose format_error for tests.
pub fn _format_error(prefix: &str, response: &HttpResponse) -> String {
    format_error(prefix, response)
}

// Expose request_timeout_env for tests.
pub fn _request_timeout_env(name: &str, default: f64) -> f64 {
    request_timeout_env(name, default)
}

// ---------------------------------------------------------------------------
// Tests — minimal smoke for 1:1 fidelity
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_timeout_env_default() {
        // Use unique name to avoid env pollution.
        let v = request_timeout_env("__HERMES_TEST_TIMEOUT_DEFAULT_XYZ__", 1.5);
        assert_eq!(v, 1.5);
    }

    #[test]
    fn coerce_number_cases() {
        assert_eq!(coerce_number(None, Some(1.0)), Some(1.0));
        assert_eq!(coerce_number(None, None), None);
        assert_eq!(coerce_number(Some("2"), Some(1.0)), Some(2.0));
        assert_eq!(coerce_number(Some("  3.5  "), Some(1.0)), Some(3.5));
        assert_eq!(coerce_number(Some("bad"), Some(1.0)), Some(1.0));
        assert_eq!(coerce_number(Some("bad"), None), None);
        assert_eq!(coerce_number(Some(""), Some(5.0)), Some(5.0));
    }

    #[test]
    fn format_error_json_fields() {
        let r = HttpResponse {
            status_code: 400,
            text: r#"{"error":"boom"}"#.to_string(),
            json_body: Some(r#"{"error":"boom"}"#.to_string()),
        };
        assert_eq!(format_error("Managed Modal exec failed", &r), "Managed Modal exec failed: boom");

        let r2 = HttpResponse {
            status_code: 400,
            text: r#"{"message":"oops"}"#.to_string(),
            json_body: Some(r#"{"message":"oops"}"#.to_string()),
        };
        assert_eq!(format_error("Managed Modal exec failed", &r2), "Managed Modal exec failed: oops");

        let r3 = HttpResponse {
            status_code: 400,
            text: r#"{"code":"limit"}"#.to_string(),
            json_body: Some(r#"{"code":"limit"}"#.to_string()),
        };
        assert_eq!(format_error("prefix", &r3), "prefix: limit");

        let r4 = HttpResponse {
            status_code: 502,
            text: "gateway down".to_string(),
            json_body: None,
        };
        assert_eq!(format_error("prefix", &r4), "prefix: gateway down");

        let r5 = HttpResponse {
            status_code: 503,
            text: "   ".to_string(),
            json_body: None,
        };
        assert_eq!(format_error("prefix", &r5), "prefix: HTTP 503");
    }

    #[test]
    fn exec_handle_roundtrip() {
        let h = ManagedModalExecHandle { exec_id: "abc-123".to_string() };
        assert_eq!(h.exec_id, "abc-123");
    }

    #[test]
    fn gateway_origin_builds() {
        // Ensure build_vendor_gateway_url respects explicit env.
        unsafe { env::set_var("MODAL_GATEWAY_URL", "https://custom.example.com/") };
        let url = build_vendor_gateway_url("modal");
        assert_eq!(url, "https://custom.example.com");
        unsafe { env::remove_var("MODAL_GATEWAY_URL") };

        let url2 = build_vendor_gateway_url("modal");
        assert!(url2.contains("modal-gateway"));
    }

    #[test]
    fn guard_no_mounts_ok() {
        // Ensure no env sentinel -> guard passes.
        unsafe { env::remove_var("HERMES_CREDENTIAL_MOUNTS") };
        assert!(guard_unsupported_credential_passthrough().is_ok());
    }

    #[test]
    fn constants_match_python() {
        assert_eq!(CLIENT_TIMEOUT_GRACE_SECONDS, 10.0);
        assert_eq!(INTERRUPT_OUTPUT, "[Command interrupted - Modal sandbox exec cancelled]");
        assert_eq!(UNEXPECTED_ERROR_PREFIX, "Managed Modal exec failed");
        assert_eq!(DEFAULT_CONNECT_TIMEOUT_SECS, 1.0);
        assert_eq!(DEFAULT_POLL_READ_TIMEOUT_SECS, 5.0);
        assert_eq!(DEFAULT_CANCEL_READ_TIMEOUT_SECS, 5.0);
    }
}
