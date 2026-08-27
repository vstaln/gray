//! Browserbase cloud browser provider — plugin form.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/plugins/browser/browserbase/provider.py` (300 LOC).
//! Subclasses `agent.browser_provider.BrowserProvider` (the plugin-facing
//! ABC introduced in PR #25214). The legacy in-tree module
//! `tools.browser_providers.browserbase` was removed in the same PR; this file
//! is now the canonical implementation.
//!
//! Browserbase requires direct `BROWSERBASE_API_KEY` and `BROWSERBASE_PROJECT_ID`
//! credentials. Managed Nous gateway support has been removed — the Nous
//! subscription now routes through Browser Use instead (see
//! `plugins/browser/browser_use/`).
//!
//! Config keys this provider responds to:
//! ```yaml
//! browser:
//!   cloud_provider: "browserbase"
//! ```
//! Auth env vars:
//! ```text
//! BROWSERBASE_API_KEY=...       # https://browserbase.com
//! BROWSERBASE_PROJECT_ID=...
//! ```
//! Optional feature knobs:
//! ```text
//! BROWSERBASE_BASE_URL=...      # default https://api.browserbase.com
//! BROWSERBASE_PROXIES=true      # default true
//! BROWSERBASE_ADVANCED_STEALTH=false
//! BROWSERBASE_KEEP_ALIVE=true   # default true
//! BROWSERBASE_SESSION_TIMEOUT=... (seconds, integer)
//! ```
//!
//! Python surface ported line-for-line:
//! - `class BrowserbaseBrowserProvider(BrowserProvider)` (lines 47-300): `name`,
//!   `display_name`, `is_available`, `_get_config_or_none`, `_get_config`,
//!   `create_session`, `close_session`, `emergency_cleanup`, `get_setup_schema`
//! - Direct credentials only — no managed gateway dispatch
//! - Optional knobs: proxies, advancedStealth, keepAlive, timeout
//! - 402 fallback: keepAlive → proxies, with warning logs
//! - `requests.post` with 30s/10s/5s timeouts, X-BB-API-Key header
//!
//! Rust notes:
//! - `requests` is modelled with `curl` fallback (`http_post`) so filtering
//!   and lifecycle semantics are byte-identical without `cargo`.
//!   Real I/O upgrade: `reqwest::Client::post(...).headers(...).json(...).send().await`.
//! - `uuid.uuid4().hex[:8]` → `generate_hex(8)`.
//! - `agent.secret_scope.get_secret` → `get_secret()` (HERMES_HOME/.env then os env).
//! - `os.environ.get("BROWSERBASE_*")` → `std::env::var` / `get_env_value` helpers.
//! - `serde_json` is already in workspace deps; no new Cargo.toml entry required (`NO CARGO`).

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// Constants — mirrors provider.py docstring defaults (lines 25-29)
// ---------------------------------------------------------------------------

/// Mirrors default `BROWSERBASE_BASE_URL` fallback (line 77).
pub const DEFAULT_BASE_URL: &str = "https://api.browserbase.com";

// ---------------------------------------------------------------------------
// HERMES_HOME + secret helpers — mirrors hermes_constants + agent.secret_scope
// ---------------------------------------------------------------------------

fn get_hermes_home() -> PathBuf {
    if let Ok(val) = std::env::var("HERMES_HOME") {
        let trimmed = val.trim().to_string();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".hermes");
    }
    PathBuf::from(".hermes")
}

fn get_env_value(name: &str) -> Option<String> {
    let home = get_hermes_home();
    let dotenv = home.join(".env");
    if let Ok(text) = fs::read_to_string(&dotenv) {
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let line = if line.starts_with("export ") {
                line["export ".len()..].trim()
            } else {
                line
            };
            if let Some((k, v)) = line.split_once('=') {
                if k.trim() == name {
                    let val = v.trim().trim_matches('"').trim_matches('\'').to_string();
                    if !val.is_empty() {
                        return Some(val);
                    }
                }
            }
        }
    }
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

/// Mirrors `agent.secret_scope.get_secret` — HERMES_HOME/.env then os env.
pub fn get_secret(name: &str) -> Option<String> {
    get_env_value(name).or_else(|| std::env::var(name).ok().filter(|v| !v.trim().is_empty()))
}

// ---------------------------------------------------------------------------
// Provider config — mirrors _get_config_or_none / _get_config return shape
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserbaseConfig {
    pub api_key: String,
    pub project_id: String,
    pub base_url: String,
}

// ---------------------------------------------------------------------------
// HTTP helpers — mirrors requests.post
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub status: u16,
    pub text: String,
}

/// Perform POST via curl (NO CARGO) — mirrors `requests.post(..., timeout=30/10/5)`.
fn http_post(url: &str, headers: &HashMap<String, String>, payload: &Value, timeout_secs: u64) -> Result<HttpResponse, String> {
    // Test injection: `BROWSERBASE_MOCK_POST` JSON like {"status":200,"body":{...}}
    if let Ok(mock) = std::env::var("BROWSERBASE_MOCK_POST") {
        if !mock.trim().is_empty() {
            if let Ok(v) = serde_json::from_str::<Value>(&mock) {
                let status = v.get("status").and_then(|x| x.as_u64()).unwrap_or(200) as u16;
                let body_val = v.get("body").cloned().unwrap_or(json!({}));
                let text = if body_val.is_string() {
                    body_val.as_str().unwrap().to_string()
                } else {
                    body_val.to_string()
                };
                return Ok(HttpResponse { status, text });
            }
        }
    }
    if let Ok(json_str) = std::env::var("BROWSERBASE_CREATE_JSON") {
        if !json_str.trim().is_empty() {
            let status = std::env::var("BROWSERBASE_CREATE_STATUS").ok().and_then(|s| s.parse::<u16>().ok()).unwrap_or(200);
            return Ok(HttpResponse { status, text: json_str });
        }
    }
    if let Ok(err) = std::env::var("BROWSERBASE_POST_ERROR") {
        return Err(err);
    }

    let body_str = serde_json::to_string(payload).unwrap_or_else(|_| "{}".to_string());
    try_curl_post(url, &body_str, headers, timeout_secs)
}

fn try_curl_post(url: &str, body: &str, headers: &HashMap<String, String>, timeout: u64) -> Result<HttpResponse, String> {
    let mut cmd = std::process::Command::new("curl");
    cmd.arg("-sS")
        .arg("-m")
        .arg(timeout.to_string())
        .arg("-X")
        .arg("POST")
        .arg("-w")
        .arg("\n%{http_code}")
        .arg("-D")
        .arg("-");
    for (k, v) in headers {
        cmd.arg("-H").arg(format!("{}: {}", k, v));
    }
    cmd.arg("-d").arg(body).arg(url);

    let output = cmd.output().map_err(|e| format!("curl failed: {}", e))?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    if !output.status.success() && stdout.trim().is_empty() {
        return Err(format!("curl error: {}", stderr.trim()));
    }
    parse_curl_output(&stdout)
}

fn parse_curl_output(stdout: &str) -> Result<HttpResponse, String> {
    let trimmed = stdout.trim_end();
    let last_newline = trimmed.rfind('\n');
    let (head_body, status_str) = match last_newline {
        Some(idx) => (&trimmed[..idx], trimmed[idx + 1..].trim()),
        None => ("", trimmed),
    };
    let status: u16 = status_str.parse().unwrap_or(0);
    let (_header_part, body) = if let Some(pos) = head_body.find("\r\n\r\n") {
        let (h, b) = head_body.split_at(pos + 4);
        (h, b.to_string())
    } else if let Some(pos) = head_body.find("\n\n") {
        let (h, b) = head_body.split_at(pos + 2);
        (h, b.to_string())
    } else {
        ("", head_body.to_string())
    };
    Ok(HttpResponse { status, text: body })
}

// ---------------------------------------------------------------------------
// Provider — mirrors class BrowserbaseBrowserProvider(BrowserProvider)
// ---------------------------------------------------------------------------

/// Browserbase (https://browserbase.com) cloud browser backend.
///
/// Direct credentials only — managed-Nous-gateway support lives on the
/// Browser Use provider now.
///
/// Mirrors `class BrowserbaseBrowserProvider(BrowserProvider)` (lines 47-300).
#[derive(Debug, Clone, Default)]
pub struct BrowserbaseBrowserProvider;

impl BrowserbaseBrowserProvider {
    pub fn new() -> Self {
        Self
    }

    /// Mirrors `@property def name(self) -> str: return "browserbase"` (lines 54-56).
    pub fn name(&self) -> &'static str {
        "browserbase"
    }

    /// Mirrors `@property def display_name(self) -> str: return "Browserbase"` (lines 58-60).
    pub fn display_name(&self) -> &'static str {
        "Browserbase"
    }

    /// Mirrors `def is_available(self) -> bool: return self._get_config_or_none() is not None` (lines 62-63).
    pub fn is_available(&self) -> bool {
        self.get_config_or_none().is_some()
    }

    /// Backward-compat alias — mirrors `def is_configured(self) -> bool`.
    pub fn is_configured(&self) -> bool {
        self.is_available()
    }

    /// Backward-compat alias — mirrors `def provider_name(self) -> str`.
    pub fn provider_name(&self) -> String {
        self.display_name().to_string()
    }

    // -- Config resolution --------------------------------------------------

    /// Mirrors `def _get_config_or_none(self) -> Optional[Dict[str, Any]]` (lines 69-80).
    pub fn get_config_or_none(&self) -> Option<BrowserbaseConfig> {
        let api_key = get_secret("BROWSERBASE_API_KEY")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())?;
        let project_id = get_secret("BROWSERBASE_PROJECT_ID")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())?;
        let base_url = std::env::var("BROWSERBASE_BASE_URL")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .or_else(|| get_env_value("BROWSERBASE_BASE_URL"))
            .map(|s| s.trim().trim_end_matches('/').to_string())
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
        Some(BrowserbaseConfig {
            api_key,
            project_id,
            base_url: base_url.trim_end_matches('/').to_string(),
        })
    }

    /// Mirrors `def _get_config(self) -> Dict[str, Any]` (lines 82-89).
    pub fn get_config(&self) -> Result<BrowserbaseConfig, String> {
        self.get_config_or_none().ok_or_else(|| {
            "Browserbase requires BROWSERBASE_API_KEY and BROWSERBASE_PROJECT_ID environment variables.".to_string()
        })
    }

    // -- Session lifecycle --------------------------------------------------

    /// Mirrors `def create_session(self, task_id: str) -> Dict[str, object]` (lines 95-216).
    pub fn create_session(&self, task_id: &str) -> Result<Value, String> {
        let config = self.get_config().map_err(|e| e.to_string())?;

        // Optional env-var knobs — mirrors lines 99-106
        let enable_proxies = std::env::var("BROWSERBASE_PROXIES")
            .or_else(|_| get_env_value("BROWSERBASE_PROXIES").ok_or(std::env::VarError::NotPresent))
            .map(|v| v.trim().to_ascii_lowercase() != "false")
            .unwrap_or(true);
        let enable_advanced_stealth = std::env::var("BROWSERBASE_ADVANCED_STEALTH")
            .or_else(|_| get_env_value("BROWSERBASE_ADVANCED_STEALTH").ok_or(std::env::VarError::NotPresent))
            .map(|v| v.trim().to_ascii_lowercase() == "true")
            .unwrap_or(false);
        let enable_keep_alive = std::env::var("BROWSERBASE_KEEP_ALIVE")
            .or_else(|_| get_env_value("BROWSERBASE_KEEP_ALIVE").ok_or(std::env::VarError::NotPresent))
            .map(|v| v.trim().to_ascii_lowercase() != "false")
            .unwrap_or(true);
        let custom_timeout_ms = std::env::var("BROWSERBASE_SESSION_TIMEOUT")
            .or_else(|_| get_env_value("BROWSERBASE_SESSION_TIMEOUT").ok_or(std::env::VarError::NotPresent))
            .ok()
            .filter(|v| !v.trim().is_empty());

        let mut features_enabled = json!({
            "basic_stealth": true,
            "proxies": false,
            "advanced_stealth": false,
            "keep_alive": false,
            "custom_timeout": false,
        });

        let mut session_config = json!({"projectId": config.project_id});

        if enable_keep_alive {
            session_config["keepAlive"] = json!(true);
        }

        if let Some(ref timeout_str) = custom_timeout_ms {
            match timeout_str.trim().parse::<i64>() {
                Ok(timeout_val) if timeout_val > 0 => {
                    session_config["timeout"] = json!(timeout_val);
                }
                Ok(_) => {}
                Err(_) => {
                    eprintln!("Invalid BROWSERBASE_SESSION_TIMEOUT value: {}", timeout_str);
                }
            }
        }

        if enable_proxies {
            session_config["proxies"] = json!(true);
        }

        if enable_advanced_stealth {
            session_config["browserSettings"] = json!({"advancedStealth": true});
        }

        // --- Create session via API ---
        let headers = {
            let mut h = HashMap::new();
            h.insert("Content-Type".to_string(), "application/json".to_string());
            h.insert("X-BB-API-Key".to_string(), config.api_key.clone());
            h
        };

        let url = format!("{}/v1/sessions", config.base_url.trim_end_matches('/'));

        let mut proxies_fallback = false;
        let mut keepalive_fallback = false;

        let mut response = match http_post(&url, &headers, &session_config, 30) {
            Ok(r) => r,
            Err(exc) => return Err(format!("Browserbase API connection failed: {}", exc)),
        };

        // Handle 402 — paid features unavailable — mirrors lines 155-182
        if response.status == 402 {
            if enable_keep_alive {
                keepalive_fallback = true;
                eprintln!(
                    "keepAlive may require paid plan (402), retrying without it. Sessions may timeout during long operations."
                );
                if let Some(obj) = session_config.as_object_mut() {
                    obj.remove("keepAlive");
                }
                response = match http_post(&url, &headers, &session_config, 30) {
                    Ok(r) => r,
                    Err(exc) => return Err(format!("Browserbase API connection failed: {}", exc)),
                };
            }

            if response.status == 402 && enable_proxies {
                proxies_fallback = true;
                eprintln!(
                    "Proxies unavailable (402), retrying without proxies. Bot detection may be less effective."
                );
                if let Some(obj) = session_config.as_object_mut() {
                    obj.remove("proxies");
                }
                response = match http_post(&url, &headers, &session_config, 30) {
                    Ok(r) => r,
                    Err(exc) => return Err(format!("Browserbase API connection failed: {}", exc)),
                };
            }
        }

        if !(200..300).contains(&response.status) {
            return Err(format!(
                "Failed to create Browserbase session: {} {}",
                response.status, response.text
            ));
        }

        let session_data: Value = serde_json::from_str(&response.text)
            .map_err(|e| format!("Failed to parse Browserbase session response: {}", e))?;

        let session_name = format!("hermes_{}_{}", task_id, generate_hex(8));

        if enable_proxies && !proxies_fallback {
            features_enabled["proxies"] = json!(true);
        }
        if enable_advanced_stealth {
            features_enabled["advanced_stealth"] = json!(true);
        }
        if enable_keep_alive && !keepalive_fallback {
            features_enabled["keep_alive"] = json!(true);
        }
        if custom_timeout_ms.is_some() && session_config.get("timeout").is_some() {
            features_enabled["custom_timeout"] = json!(true);
        }

        let feature_str = features_enabled
            .as_object()
            .map(|obj| {
                obj.iter()
                    .filter(|(_, v)| v.as_bool().unwrap_or(false))
                    .map(|(k, _)| k.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();
        eprintln!("Created Browserbase session {} with features: {}", session_name, feature_str);

        let bb_session_id = session_data
            .get("id")
            .map(|v| {
                if let Some(s) = v.as_str() {
                    s.to_string()
                } else {
                    v.to_string().trim_matches('"').to_string()
                }
            })
            .unwrap_or_default();

        let cdp_url = session_data
            .get("connectUrl")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        Ok(json!({
            "session_name": session_name,
            "bb_session_id": bb_session_id,
            "cdp_url": cdp_url,
            "features": features_enabled,
        }))
    }

    /// Mirrors `def close_session(self, session_id: str) -> bool` (lines 218-253).
    pub fn close_session(&self, session_id: &str) -> bool {
        let config = match self.get_config() {
            Ok(c) => c,
            Err(_) => {
                eprintln!("Cannot close Browserbase session {} — missing credentials", session_id);
                return false;
            }
        };

        let url = format!("{}/v1/sessions/{}", config.base_url.trim_end_matches('/'), session_id);
        let headers = {
            let mut h = HashMap::new();
            h.insert("X-BB-API-Key".to_string(), config.api_key.clone());
            h.insert("Content-Type".to_string(), "application/json".to_string());
            h
        };
        let payload = json!({
            "projectId": config.project_id,
            "status": "REQUEST_RELEASE",
        });

        match http_post(&url, &headers, &payload, 10) {
            Ok(resp) => {
                if matches!(resp.status, 200 | 201 | 204) {
                    eprintln!("Successfully closed Browserbase session {}", session_id);
                    true
                } else {
                    eprintln!(
                        "Failed to close session {}: HTTP {} - {}",
                        session_id,
                        resp.status,
                        resp.text.chars().take(200).collect::<String>()
                    );
                    false
                }
            }
            Err(e) => {
                eprintln!("Exception closing Browserbase session {}: {}", session_id, e);
                false
            }
        }
    }

    /// Mirrors `def emergency_cleanup(self, session_id: str) -> None` (lines 255-279).
    pub fn emergency_cleanup(&self, session_id: &str) {
        let config = match self.get_config_or_none() {
            Some(c) => c,
            None => {
                eprintln!(
                    "Cannot emergency-cleanup Browserbase session {} — missing credentials",
                    session_id
                );
                return;
            }
        };
        let url = format!("{}/v1/sessions/{}", config.base_url.trim_end_matches('/'), session_id);
        let headers = {
            let mut h = HashMap::new();
            h.insert("X-BB-API-Key".to_string(), config.api_key.clone());
            h.insert("Content-Type".to_string(), "application/json".to_string());
            h
        };
        let payload = json!({
            "projectId": config.project_id,
            "status": "REQUEST_RELEASE",
        });
        match http_post(&url, &headers, &payload, 5) {
            Ok(_) => {},
            Err(e) => {
                eprintln!(
                    "Emergency cleanup failed for Browserbase session {}: {}",
                    session_id, e
                );
            }
        }
    }

    /// Mirrors `def get_setup_schema(self) -> Dict[str, Any]` (lines 281-300).
    pub fn get_setup_schema() -> Value {
        json!({
            "name": "Browserbase",
            "badge": "paid",
            "tag": "Cloud browser with stealth and proxies",
            "env_vars": [
                {
                    "key": "BROWSERBASE_API_KEY",
                    "prompt": "Browserbase API key",
                    "url": "https://browserbase.com",
                },
                {
                    "key": "BROWSERBASE_PROJECT_ID",
                    "prompt": "Browserbase project ID",
                },
            ],
            "post_setup": "browserbase",
        })
    }

    /// Instance wrapper for get_setup_schema (object-safe).
    pub fn setup_schema(&self) -> Value {
        Self::get_setup_schema()
    }
}

// ---------------------------------------------------------------------------
// Helpers: uuid hex, etc.
// ---------------------------------------------------------------------------

fn generate_hex(len: usize) -> String {
    const HEX: &[u8] = b"0123456789abcdef";
    if let Ok(uuid_str) = fs::read_to_string("/proc/sys/kernel/random/uuid") {
        let hex: String = uuid_str.chars().filter(|c| c.is_ascii_hexdigit()).collect();
        if hex.len() >= len {
            return hex[..len].to_ascii_lowercase();
        }
    }
    if let Ok(bytes) = fs::read("/dev/urandom") {
        if bytes.len() >= len.div_ceil(2) {
            let mut out = String::with_capacity(len);
            for b in bytes.iter().take(len.div_ceil(2)) {
                out.push(HEX[(b >> 4) as usize] as char);
                if out.len() < len {
                    out.push(HEX[(b & 0xf) as usize] as char);
                }
            }
            out.truncate(len);
            return out;
        }
    }
    let mut seed = {
        use std::time::{SystemTime, UNIX_EPOCH};
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(std::time::Duration::from_secs(0))
            .as_nanos() as u64;
        now.wrapping_add(std::process::id() as u64)
            .wrapping_add(0x9e3779b97f4a7c15)
    };
    let mut out = String::with_capacity(len);
    for _ in 0..len {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        out.push(HEX[((seed >> 33) & 0xf) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    fn with_env<F: FnOnce()>(vars: &[(&str, Option<&str>)], f: F) {
        let prev: Vec<(&str, Option<String>)> = vars.iter().map(|(k, _)| (*k, env::var(k).ok())).collect();
        for (k, v) in vars {
            match v {
                Some(val) => env::set_var(k, val),
                None => env::remove_var(k),
            }
        }
        f();
        for (k, prev_val) in prev {
            match prev_val {
                Some(v) => env::set_var(k, v),
                None => env::remove_var(k),
            }
        }
    }

    #[test]
    fn constants_match_python() {
        assert_eq!(DEFAULT_BASE_URL, "https://api.browserbase.com");
    }

    #[test]
    fn provider_names() {
        let p = BrowserbaseBrowserProvider::new();
        assert_eq!(p.name(), "browserbase");
        assert_eq!(p.display_name(), "Browserbase");
    }

    #[test]
    fn is_available_requires_both_keys() {
        with_env(
            &[("BROWSERBASE_API_KEY", None), ("BROWSERBASE_PROJECT_ID", None)],
            || {
                let p = BrowserbaseBrowserProvider::new();
                assert!(!p.is_available());
            },
        );
        with_env(
            &[("BROWSERBASE_API_KEY", Some("key123")), ("BROWSERBASE_PROJECT_ID", None)],
            || {
                let p = BrowserbaseBrowserProvider::new();
                assert!(!p.is_available());
            },
        );
        with_env(
            &[("BROWSERBASE_API_KEY", Some("key123")), ("BROWSERBASE_PROJECT_ID", Some("proj123"))],
            || {
                let p = BrowserbaseBrowserProvider::new();
                assert!(p.is_available());
                let cfg = p.get_config().unwrap();
                assert_eq!(cfg.api_key, "key123");
                assert_eq!(cfg.project_id, "proj123");
                assert_eq!(cfg.base_url, DEFAULT_BASE_URL);
            },
        );
    }

    #[test]
    fn base_url_override() {
        with_env(
            &[
                ("BROWSERBASE_API_KEY", Some("k")),
                ("BROWSERBASE_PROJECT_ID", Some("p")),
                ("BROWSERBASE_BASE_URL", Some("https://custom.example.com/")),
            ],
            || {
                let p = BrowserbaseBrowserProvider::new();
                let cfg = p.get_config().unwrap();
                assert_eq!(cfg.base_url, "https://custom.example.com");
            },
        );
    }

    #[test]
    fn get_config_err_when_missing() {
        with_env(
            &[("BROWSERBASE_API_KEY", None), ("BROWSERBASE_PROJECT_ID", None)],
            || {
                let p = BrowserbaseBrowserProvider::new();
                assert!(p.get_config().is_err());
                assert!(p.get_config().unwrap_err().contains("BROWSERBASE_API_KEY"));
            },
        );
    }

    #[test]
    fn setup_schema_shape() {
        let schema = BrowserbaseBrowserProvider::get_setup_schema();
        assert_eq!(schema["name"], "Browserbase");
        assert_eq!(schema["badge"], "paid");
        assert_eq!(schema["post_setup"], "browserbase");
        let env_vars = schema["env_vars"].as_array().unwrap();
        assert_eq!(env_vars.len(), 2);
        assert_eq!(env_vars[0]["key"], "BROWSERBASE_API_KEY");
        assert_eq!(env_vars[1]["key"], "BROWSERBASE_PROJECT_ID");
    }

    #[test]
    fn create_session_mock() {
        with_env(
            &[
                ("BROWSERBASE_API_KEY", Some("k")),
                ("BROWSERBASE_PROJECT_ID", Some("p")),
                ("BROWSERBASE_CREATE_JSON", Some(r#"{"id":"sess-123","connectUrl":"wss://connect.browserbase.com/abc"}"#)),
            ],
            || {
                let p = BrowserbaseBrowserProvider::new();
                let result = p.create_session("task-1").unwrap();
                assert_eq!(result["bb_session_id"], "sess-123");
                assert_eq!(result["cdp_url"], "wss://connect.browserbase.com/abc");
                assert!(result["session_name"].as_str().unwrap().starts_with("hermes_task-1_"));
                assert_eq!(result["features"]["basic_stealth"], true);
            },
        );
    }

    #[test]
    fn close_session_mock_success() {
        with_env(
            &[
                ("BROWSERBASE_API_KEY", Some("k")),
                ("BROWSERBASE_PROJECT_ID", Some("p")),
                ("BROWSERBASE_MOCK_POST", Some(r#"{"status":204,"body":{}}"#)),
            ],
            || {
                let p = BrowserbaseBrowserProvider::new();
                assert!(p.close_session("sess-123"));
            },
        );
    }
}
