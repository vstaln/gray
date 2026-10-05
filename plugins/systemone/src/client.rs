//! Synchronous HTTP client for the `/v1/systemone` wire. One client shape
//! covers every backend: base URL is the only thing that changes.

use std::time::Duration;

use serde_json::Value;

use crate::config::{Config, host_of, is_local_host};

const TIMEOUT: Duration = Duration::from_secs(20);

fn agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .http_status_as_error(false)
        .build()
        .into()
}

/// Map a transport-level failure to a user-facing message. On a local
/// base the likely story is "no daemon running", so say how to fix it.
fn transport_error(cfg: &Config, err: &ureq::Error) -> String {
    if matches!(err, ureq::Error::Timeout(_)) {
        format!("systemone request to {} timed out", cfg.base)
    } else if is_local_host(&host_of(&cfg.base)) {
        format!(
            "no System One server at {} — start Ollaya (`ollaya serve`) or set \
             SYSTEMONE_BASE_URL (+ SYSTEMONE_API_KEY for TypeSafe/OpenRouter)",
            cfg.base
        )
    } else {
        format!("systemone request to {} failed: {err}", cfg.base)
    }
}

fn check_status(status: u16, body: &str) -> Result<(), String> {
    if (200..300).contains(&status) {
        return Ok(());
    }
    let truncated: String = body.chars().take(300).collect();
    Err(format!("systemone HTTP {status}: {truncated}"))
}

/// POST the judge/semantic_find request body to `<base>/v1/systemone`.
/// `timeout` bounds this request: semantic_find passes the remainder of
/// its time budget so a late batch cannot overrun the host's 30s TTL.
pub fn post_systemone(cfg: &Config, body: &Value, timeout: Duration) -> Result<Value, String> {
    let mut req = agent(timeout).post(cfg.systemone_url());
    if let Some(key) = &cfg.key {
        req = req.header("Authorization", &format!("Bearer {key}"));
    }
    let mut resp = match req.send_json(body) {
        Ok(r) => r,
        Err(e) => return Err(transport_error(cfg, &e)),
    };
    let status = resp.status().as_u16();
    let text = resp
        .body_mut()
        .read_to_string()
        .map_err(|e| format!("systemone: could not read response body: {e}"))?;
    check_status(status, &text)?;
    serde_json::from_str(&text).map_err(|e| format!("systemone: invalid JSON response: {e}"))
}

/// GET `<base>/v1/models`; returns the raw JSON (`{"data":[{"id":…}]}`).
pub fn get_models(cfg: &Config) -> Result<Value, String> {
    let mut req = agent(TIMEOUT).get(cfg.models_url());
    if let Some(key) = &cfg.key {
        req = req.header("Authorization", &format!("Bearer {key}"));
    }
    let mut resp = match req.call() {
        Ok(r) => r,
        Err(e) => return Err(transport_error(cfg, &e)),
    };
    let status = resp.status().as_u16();
    let text = resp
        .body_mut()
        .read_to_string()
        .map_err(|e| format!("systemone: could not read response body: {e}"))?;
    check_status(status, &text)?;
    serde_json::from_str(&text).map_err(|e| format!("systemone: invalid JSON response: {e}"))
}
