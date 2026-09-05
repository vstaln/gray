//! xAI (Grok) OAuth sign-in implementation.
//!
//! Handles PKCE authorization code flow, endpoint discovery, local callback
//! listener on loopback port 56121, secure permissioned auth storage (`~/.gray/auth.json`),
//! and token refresh.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use percent_encoding::{percent_decode_str, utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

pub const XAI_CLIENT_ID: &str = "b1a00492-073a-47ea-816f-4c329264a828";
pub const XAI_ISSUER: &str = "https://auth.x.ai";
pub const XAI_API_BASE: &str = "https://api.x.ai/v1";
pub const XAI_DEFAULT_MODEL: &str = "grok-4";

pub const CODEX_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const CODEX_AUTHORIZE_URL: &str = "https://auth.openai.com/oauth/authorize";
const CODEX_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const OPENAI_SCOPE: &str = "openid profile email offline_access";
pub const CODEX_API_BASE: &str = "https://api.openai.com/v1";
pub const CODEX_DEFAULT_MODEL: &str = "gpt-5.3-codex";
const CODEX_PORT: u16 = 1455;
const CODEX_CALLBACK_PATH: &str = "/auth/callback";
const CODEX_REDIRECT_URI: &str = "http://localhost:1455/auth/callback";

const DISCOVERY_URL: &str = "https://auth.x.ai/.well-known/openid-configuration";
const AUTHORIZE_FALLBACK: &str = "https://auth.x.ai/oauth2/authorize";
const TOKEN_FALLBACK: &str = "https://auth.x.ai/oauth2/token";
const XAI_SCOPE: &str = "openid profile email offline_access grok-cli:access api:access";
const LOOPBACK_PORT: u16 = 56121;
const CALLBACK_PATH: &str = "/callback";
const REDIRECT_URI: &str = "http://127.0.0.1:56121/callback";
const VERIFIER_BYTES: usize = 96;
const REFRESH_LEAD_SECS: i64 = 300;
const AUTH_TIMEOUT: Duration = Duration::from_secs(300);
const URI_ENCODE_SET: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

// ---- Crypto helpers (stdlib crates) -----------------------------------------

fn sha256(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
}

fn b64url_encode(data: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(data)
}

fn b64url_decode(s: &str) -> anyhow::Result<Vec<u8>> {
    Ok(URL_SAFE_NO_PAD.decode(s.trim_end_matches('='))?)
}

fn random_bytes(n: usize) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(n);
    while bytes.len() < n { bytes.extend_from_slice(Uuid::new_v4().as_bytes()); }
    bytes.truncate(n);
    bytes
}

/// Generates a PKCE code verifier (128 chars base64url).
pub fn generate_code_verifier() -> String {
    b64url_encode(&random_bytes(VERIFIER_BYTES))
}

/// Generates a PKCE S256 code challenge for a verifier.
pub fn generate_code_challenge(verifier: &str) -> String {
    b64url_encode(&sha256(verifier.as_bytes()))
}

/// Generates a 32-character lowercase hex state string.
pub fn generate_state() -> String {
    let bytes = random_bytes(16);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

// ---- Discovery -------------------------------------------------------------

struct Endpoints {
    authorize_url: String,
    token_url: String,
}

#[derive(Deserialize)]
struct DiscoveryDoc {
    authorization_endpoint: Option<String>,
    token_endpoint: Option<String>,
}

fn validate_endpoint(raw: &str, field: &str) -> anyhow::Result<String> {
    let rest = raw
        .strip_prefix("https://")
        .ok_or_else(|| anyhow::anyhow!("xai discovery {field} must use https: {raw}"))?;
    let host_and_port = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = host_and_port.split(':').next().unwrap_or("");
    if host.is_empty() || !(host == "x.ai" || host.ends_with(".x.ai")) {
        anyhow::bail!("xai discovery {field} invalid host: {raw}");
    }
    Ok(raw.to_string())
}

async fn discover_endpoints() -> anyhow::Result<Endpoints> {
    let result: anyhow::Result<Endpoints> = async {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()?;
        let resp = client
            .get(DISCOVERY_URL)
            .header("Accept", "application/json")
            .send()
            .await?;
        if !resp.status().is_success() {
            anyhow::bail!("non-200 status: {}", resp.status());
        }
        let doc: DiscoveryDoc = resp.json().await?;
        let auth_raw = doc
            .authorization_endpoint
            .ok_or_else(|| anyhow::anyhow!("missing authorization_endpoint"))?;
        let token_raw = doc
            .token_endpoint
            .ok_or_else(|| anyhow::anyhow!("missing token_endpoint"))?;
        let authorize_url = validate_endpoint(&auth_raw, "authorization_endpoint")?;
        let token_url = validate_endpoint(&token_raw, "token_endpoint")?;
        Ok(Endpoints {
            authorize_url,
            token_url,
        })
    }
    .await;

    Ok(result.unwrap_or_else(|_| Endpoints {
        authorize_url: AUTHORIZE_FALLBACK.to_string(),
        token_url: TOKEN_FALLBACK.to_string(),
    }))
}

// ---- ID Token decoding -----------------------------------------------------

/// Extracts email, preferred_username, or sub from an OpenID Connect id_token.
pub fn decode_id_token_email(id_token: &str) -> Option<String> {
    let parts: Vec<&str> = id_token.split('.').collect();
    if parts.len() != 3 {
        return None;
    }
    let payload_bytes = b64url_decode(parts[1]).ok()?;
    let payload_json: serde_json::Value = serde_json::from_slice(&payload_bytes).ok()?;

    for key in ["email", "preferred_username", "sub"] {
        if let Some(val) = payload_json.get(key).and_then(|v| v.as_str()).filter(|v| !v.is_empty()) {
            return Some(val.to_string());
        }
    }
    None
}

// ---- Callback parsing ------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CallbackParams {
    pub code: Option<String>,
    pub state: Option<String>,
    pub error: Option<String>,
    pub error_description: Option<String>,
}

fn percent_decode(s: &str) -> anyhow::Result<String> {
    let with_spaces = s.replace('+', " ");
    let b = with_spaces.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            if i + 2 >= b.len() {
                anyhow::bail!("incomplete percent escape");
            }
            let hex = &with_spaces[i + 1..i + 3];
            u8::from_str_radix(hex, 16)
                .map_err(|_| anyhow::anyhow!("invalid hex in percent escape: %{hex}"))?;
            i += 3;
        } else {
            i += 1;
        }
    }
    Ok(percent_decode_str(&with_spaces).decode_utf8()?.into_owned())
}

/// Parses the query parameters from the HTTP request line.
pub fn parse_callback_line(request_head: &str) -> anyhow::Result<CallbackParams> {
    let first_line = request_head
        .lines()
        .next()
        .ok_or_else(|| anyhow::anyhow!("empty request head"))?;
    let parts: Vec<&str> = first_line.split_whitespace().collect();
    if parts.len() < 2 {
        anyhow::bail!("malformed HTTP request line: {first_line}");
    }
    let target = parts[1];
    let query = target
        .split_once('?')
        .map(|(_, q)| q)
        .ok_or_else(|| anyhow::anyhow!("no query string in request: {target}"))?;

    let mut code = None;
    let mut state = None;
    let mut error = None;
    let mut error_description = None;

    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (key, val) = pair.split_once('=').unwrap_or((pair, ""));
        let key = percent_decode(key)?;
        let val = percent_decode(val)?;
        match key.as_str() {
            "code" => code = Some(val),
            "state" => state = Some(val),
            "error" => error = Some(val),
            "error_description" => error_description = Some(val),
            _ => {}
        }
    }

    Ok(CallbackParams {
        code,
        state,
        error,
        error_description,
    })
}

// ---- Storage ---------------------------------------------------------------

/// Persisted OAuth credential store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredAuth {
    pub provider: String,
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: String,
    pub expires_at: i64,
    #[serde(default)]
    pub email: Option<String>,
}

fn auth_path() -> anyhow::Result<PathBuf> {
    Ok(crate::setup::gray_home()?.join("auth.json"))
}

fn load_store(path: &Path) -> BTreeMap<String, StoredAuth> {
    load_mixed_store(path)
        .into_iter()
        .filter_map(|(k, v)| match v {
            AuthEntry::OAuth(a) => Some((k, a)),
            AuthEntry::Key(_) => None,
        })
        .collect()
}

/// One `auth.json` entry: a plaintext API key or an OAuth credential. The
/// file is a mixed map `{pid: String | StoredAuth}` (plus a legacy
/// single-object form); key helpers and OAuth saves share it so neither
/// writer clobbers the other's shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub(crate) enum AuthEntry {
    Key(String),
    OAuth(StoredAuth),
}

pub(crate) fn load_mixed_store(path: &Path) -> BTreeMap<String, AuthEntry> {
    let Ok(body) = std::fs::read_to_string(path) else {
        return BTreeMap::new();
    };
    if let Ok(single) = serde_json::from_str::<StoredAuth>(&body) {
        let mut map = BTreeMap::new();
        map.insert(single.provider.clone(), AuthEntry::OAuth(single));
        return map;
    }
    serde_json::from_str::<BTreeMap<String, AuthEntry>>(&body).unwrap_or_default()
}

pub(crate) fn save_mixed_store(path: &Path, store: &BTreeMap<String, AuthEntry>) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }

    let json = serde_json::to_string_pretty(&store)?;
    file.write_all(json.as_bytes())?;
    file.flush()?;
    Ok(())
}

pub fn save_auth_at(path: &Path, auth: &StoredAuth) -> anyhow::Result<()> {
    let mut store = load_mixed_store(path);
    store.insert(auth.provider.clone(), AuthEntry::OAuth(auth.clone()));
    save_mixed_store(path, &store)
}

pub fn save_auth(auth: &StoredAuth) -> anyhow::Result<()> {
    save_auth_at(&auth_path()?, auth)
}

pub fn load_auth_at(path: &Path, provider: &str) -> anyhow::Result<StoredAuth> {
    let store = load_store(path);
    store
        .get(provider)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("not signed in to {provider}"))
}

pub fn load_auth(provider: &str) -> anyhow::Result<StoredAuth> {
    load_auth_at(&auth_path()?, provider)
}

/// Maps a connect-modal provider id to its OAuth credential key.
/// `None` = key-only provider.
pub(crate) fn oauth_provider_for_connect_id(id: &str) -> Option<&'static str> {
    match id {
        "xai" => Some("xai"),
        "openai" | "codex" => Some("codex"),
        _ => None,
    }
}

/// True when usable OAuth credentials exist for the provider.
pub(crate) fn has_oauth(provider: &str) -> bool {
    load_auth(provider).is_ok()
}

// ---- Token HTTP ------------------------------------------------------------

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<i64>,
    #[serde(default)]
    id_token: Option<String>,
}

async fn exchange_code(
    token_url: &str,
    client_id: &str,
    code: &str,
    verifier: &str,
    redirect_uri: &str,
) -> anyhow::Result<TokenResponse> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?;
    let params = [
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", redirect_uri),
        ("client_id", client_id),
        ("code_verifier", verifier),
    ];
    let resp = client
        .post(token_url)
        .header("Accept", "application/json")
        .form(&params)
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("token exchange failed ({status}): {body}");
    }

    let tokens: TokenResponse = resp.json().await?;
    Ok(tokens)
}

async fn refresh_with(provider: &str, refresh_token: &str) -> anyhow::Result<StoredAuth> {
    let (token_url, client_id, has_scope) = if provider == "codex" {
        (CODEX_TOKEN_URL.to_string(), CODEX_CLIENT_ID, true)
    } else {
        let endpoints = discover_endpoints().await?;
        (endpoints.token_url, XAI_CLIENT_ID, false)
    };

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?;
    let mut params = vec![
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id", client_id),
    ];
    if has_scope {
        params.push(("scope", OPENAI_SCOPE));
    }
    let resp = client
        .post(&token_url)
        .header("Accept", "application/json")
        .form(&params)
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("{provider} token refresh failed ({status}): {body}");
    }

    let tokens: TokenResponse = resp.json().await?;
    let old_auth = load_auth(provider).ok();
    let new_email = tokens.id_token.as_deref().and_then(decode_id_token_email);
    let email = new_email.or_else(|| old_auth.and_then(|a| a.email));

    let stored = StoredAuth {
        provider: provider.to_string(),
        access_token: tokens.access_token,
        refresh_token: tokens
            .refresh_token
            .unwrap_or_else(|| refresh_token.to_string()),
        expires_at: unix_now() + tokens.expires_in.unwrap_or(3600),
        email,
    };
    save_auth(&stored)?;
    Ok(stored)
}

fn percent_encode_uri(s: &str) -> String {
    utf8_percent_encode(s, URI_ENCODE_SET).to_string()
}

pub fn build_oauth_url(
    authorize_url: &str,
    client_id: &str,
    redirect_uri: &str,
    scope: &str,
    code_challenge: &str,
    state: &str,
    extras: &[(&str, &str)],
) -> String {
    let encoded_redirect = percent_encode_uri(redirect_uri);
    let encoded_scope = scope.replace(' ', "%20");
    let mut url = format!(
        "{authorize_url}?response_type=code&client_id={client_id}&redirect_uri={encoded_redirect}&scope={encoded_scope}&code_challenge={code_challenge}&code_challenge_method=S256&state={state}"
    );
    for &(k, v) in extras {
        url.push('&');
        url.push_str(k);
        url.push('=');
        url.push_str(&percent_encode_uri(v));
    }
    url
}

pub fn build_auth_url(
    authorize_url: &str,
    client_id: &str,
    redirect_uri: &str,
    scope: &str,
    code_challenge: &str,
    state: &str,
    nonce: &str,
) -> String {
    build_oauth_url(
        authorize_url,
        client_id,
        redirect_uri,
        scope,
        code_challenge,
        state,
        &[
            ("nonce", nonce),
            ("plan", "generic"),
            ("referrer", "cli-proxy-api"),
        ],
    )
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

// ---- Public API ------------------------------------------------------------

async fn await_callback_head(port: u16, callback_path: &str) -> anyhow::Result<String> {
    let listener = TcpListener::bind(("127.0.0.1", port)).map_err(|e| {
        if e.kind() == std::io::ErrorKind::AddrInUse {
            anyhow::anyhow!("port {port} busy — close whatever uses it and retry")
        } else {
            e.into()
        }
    })?;

    listener.set_nonblocking(true)?;
    let start = Instant::now();
    // Loop until a request actually targets callback_path: browsers open
    // speculative connections (favicon/prefetch) that must not consume the
    // handshake. Everything else gets a 404 and we keep listening.
    loop {
        let (mut stream, _) = match listener.accept() {
            Ok(conn) => conn,
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if start.elapsed() >= AUTH_TIMEOUT {
                    anyhow::bail!("Authentication timeout (5 minutes)");
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
                continue;
            }
            Err(e) => return Err(e.into()),
        };
        stream.set_nonblocking(false)?;
        let _ = stream.set_read_timeout(Some(Duration::from_secs(10)))?;

        let mut buf = [0u8; 1024];
        let mut raw = Vec::new();
        while raw.len() < 8192 {
            match stream.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    raw.extend_from_slice(&buf[..n]);
                    if raw.windows(4).any(|w| w == b"\r\n\r\n")
                        || raw.windows(2).any(|w| w == b"\n\n")
                    {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        let head = String::from_utf8_lossy(&raw).into_owned();

        let is_callback = head
            .lines()
            .next()
            .map(|line| {
                let parts: Vec<&str> = line.split_whitespace().collect();
                parts.len() >= 2 && parts[1].starts_with(callback_path)
            })
            .unwrap_or(false);

        if is_callback {
            let body = "<!DOCTYPE html><html><body>&#10003; Signed in — you can close this tab and return to your terminal.</body></html>";
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(resp.as_bytes());
            let _ = stream.flush();
            return Ok(head);
        }
        let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 9\r\nConnection: close\r\n\r\nNot found");
        let _ = stream.flush();
        if start.elapsed() >= AUTH_TIMEOUT {
            anyhow::bail!("Authentication timeout (5 minutes)");
        }
    }
}

async fn finish_signin(
    head: &str,
    state: &str,
    verifier: &str,
    provider: &str,
    client_id: &str,
    token_url: &str,
) -> anyhow::Result<StoredAuth> {
    let params = parse_callback_line(head)?;
    if let Some(err) = params.error {
        let desc = params.error_description.unwrap_or(err);
        anyhow::bail!("{desc}");
    }
    let code = params
        .code
        .ok_or_else(|| anyhow::anyhow!("No authorization code received"))?;
    if params.state.as_deref() != Some(state) {
        anyhow::bail!("Invalid state parameter");
    }

    let redirect_uri = if provider == "codex" {
        CODEX_REDIRECT_URI
    } else {
        REDIRECT_URI
    };
    let tokens = exchange_code(token_url, client_id, &code, verifier, redirect_uri).await?;
    let email = tokens.id_token.as_deref().and_then(decode_id_token_email);
    let stored = StoredAuth {
        provider: provider.to_string(),
        access_token: tokens.access_token,
        refresh_token: tokens.refresh_token.unwrap_or_default(),
        expires_at: unix_now() + tokens.expires_in.unwrap_or(3600),
        email: email.clone(),
    };
    save_auth(&stored)?;

    if let Some(ref em) = email {
        println!("Signed in as {em}.");
    } else {
        println!("Signed in.");
    }
    Ok(stored)
}

/// Runs interactive xAI OAuth browser sign-in flow.
pub async fn run_xai_signin() -> anyhow::Result<StoredAuth> {
    let endpoints = discover_endpoints().await?;
    let verifier = generate_code_verifier();
    let challenge = generate_code_challenge(&verifier);
    let state = generate_state();
    let nonce = generate_state();
    let auth_url = build_auth_url(
        &endpoints.authorize_url,
        XAI_CLIENT_ID,
        REDIRECT_URI,
        XAI_SCOPE,
        &challenge,
        &state,
        &nonce,
    );

    println!("\nSign in with your x.ai account.");
    println!("Open this URL in your browser:\n{auth_url}\n");
    println!("(waiting up to 5 minutes…)");

    let head = await_callback_head(LOOPBACK_PORT, CALLBACK_PATH).await?;
    finish_signin(
        &head,
        &state,
        &verifier,
        "xai",
        XAI_CLIENT_ID,
        &endpoints.token_url,
    )
    .await
}

/// Runs interactive Codex OAuth browser sign-in flow.
pub async fn run_codex_signin() -> anyhow::Result<StoredAuth> {
    let verifier = generate_code_verifier();
    let challenge = generate_code_challenge(&verifier);
    let state = generate_state();
    let auth_url = build_oauth_url(
        CODEX_AUTHORIZE_URL,
        CODEX_CLIENT_ID,
        CODEX_REDIRECT_URI,
        OPENAI_SCOPE,
        &challenge,
        &state,
        &[
            ("id_token_add_organizations", "true"),
            ("codex_cli_simplified_flow", "true"),
            ("originator", "codex_cli_rs"),
        ],
    );

    println!("\nSign in with your ChatGPT account.");
    println!("Open this URL in your browser:\n{auth_url}\n");
    println!("(waiting up to 5 minutes…)");

    let head = await_callback_head(CODEX_PORT, CODEX_CALLBACK_PATH).await?;
    finish_signin(
        &head,
        &state,
        &verifier,
        "codex",
        CODEX_CLIENT_ID,
        CODEX_TOKEN_URL,
    )
    .await
}

/// Refreshes saved OAuth credentials for the given provider.
pub async fn refresh(provider: &str) -> anyhow::Result<StoredAuth> {
    let auth = load_auth(provider)?;
    if auth.refresh_token.trim().is_empty() {
        anyhow::bail!("session cannot be refreshed — sign in again");
    }
    refresh_with(provider, &auth.refresh_token).await
}

/// Retrieves a valid access token, refreshing automatically if expired or nearing expiration.
pub async fn ensure_access_token(provider: &str) -> anyhow::Result<String> {
    let auth = load_auth(provider)?;
    let now = unix_now();
    if now < auth.expires_at - REFRESH_LEAD_SECS && !auth.access_token.trim().is_empty() {
        Ok(auth.access_token)
    } else {
        let refreshed = refresh(provider).await?;
        Ok(refreshed.access_token)
    }
}

/// Silently applies cached OAuth token to Config if auth_mode is oauth and api_key is not set.
pub async fn apply_saved_oauth(config: &mut crate::config::Config) {
    let Ok(path) = crate::setup::saved_config_path() else {
        return;
    };
    let saved = crate::setup::load_saved_config_at(&path);
    let provider = if saved
        .base_url
        .as_deref()
        .map(|u| u.contains("api.openai.com"))
        .unwrap_or(false)
    {
        "codex"
    } else {
        "xai"
    };
    if saved.auth_mode.as_deref() == Some("oauth")
        && config.api_key.is_none()
        && let Ok(token) = ensure_access_token(provider).await
    {
        config.api_key = Some(token);
    }
}

// ---- Unit tests ------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn stored_xai() -> StoredAuth {
        StoredAuth {
            provider: "xai".to_string(),
            access_token: "tok123".to_string(),
            refresh_token: "ref123".to_string(),
            expires_at: 9_999_999_999,
            email: None,
        }
    }

    fn write(path: &Path, body: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(path, body).expect("write fixture");
    }

    #[test]
    fn mixed_store_keeps_key_strings_and_oauth_objects() {
        let dir = tempfile::tempdir().expect("tmp");
        let path = dir.path().join("auth.json");
        write(&path, r#"{"openrouter": "sk-or-123", "xai": {"provider": "xai", "access_token": "tok123", "refresh_token": "ref123", "expires_at": 9999999999}}"#);
        let store = load_mixed_store(&path);
        assert!(matches!(store.get("openrouter"), Some(AuthEntry::Key(k)) if k == "sk-or-123"), "{store:?}");
        assert!(matches!(store.get("xai"), Some(AuthEntry::OAuth(a)) if a.access_token == "tok123"), "{store:?}");
    }

    #[test]
    fn saving_oauth_preserves_existing_key_strings() {
        // Regression: oauth save rewrote the file as objects-only, wiping keys.
        let dir = tempfile::tempdir().expect("tmp");
        let path = dir.path().join("auth.json");
        write(&path, r#"{"openrouter": "sk-or-123"}"#);
        save_auth_at(&path, &stored_xai()).expect("save");
        let store = load_mixed_store(&path);
        assert!(matches!(store.get("openrouter"), Some(AuthEntry::Key(_))), "key survived: {store:?}");
        assert!(matches!(store.get("xai"), Some(AuthEntry::OAuth(_))), "oauth saved: {store:?}");
    }

    #[test]
    fn oauth_provider_mapping_covers_dual_providers() {
        assert_eq!(oauth_provider_for_connect_id("xai"), Some("xai"));
        assert_eq!(oauth_provider_for_connect_id("openai"), Some("codex"));
        assert_eq!(oauth_provider_for_connect_id("codex"), Some("codex"));
        assert_eq!(oauth_provider_for_connect_id("anthropic"), None);
        assert_eq!(oauth_provider_for_connect_id("ollama"), None);
    }

    #[test]
    fn legacy_single_object_still_loads() {
        let dir = tempfile::tempdir().expect("tmp");
        let path = dir.path().join("auth.json");
        write(&path, r#"{"provider": "codex", "access_token": "tok", "expires_at": 9999999999}"#);
        let store = load_mixed_store(&path);
        assert!(matches!(store.get("codex"), Some(AuthEntry::OAuth(_))), "{store:?}");
    }
}

