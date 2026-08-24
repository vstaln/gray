//! xAI (Grok) OAuth sign-in implementation.
//!
//! Handles PKCE authorization code flow, endpoint discovery, local callback
//! listener on loopback port 56121, secure permissioned auth storage (`~/.gray/auth.json`),
//! and token refresh. Hand-rolled SHA-256 and base64url encoding to avoid external crypto deps.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const XAI_CLIENT_ID: &str = "b1a00492-073a-47ea-816f-4c329264a828";
pub const XAI_ISSUER: &str = "https://auth.x.ai";
pub const XAI_API_BASE: &str = "https://api.x.ai/v1";
pub const XAI_DEFAULT_MODEL: &str = "grok-4";

const DISCOVERY_URL: &str = "https://auth.x.ai/.well-known/openid-configuration";
const AUTHORIZE_FALLBACK: &str = "https://auth.x.ai/oauth2/authorize";
const TOKEN_FALLBACK: &str = "https://auth.x.ai/oauth2/token";
const SCOPE: &str = "openid profile email offline_access grok-cli:access api:access";
const LOOPBACK_PORT: u16 = 56121;
const CALLBACK_PATH: &str = "/callback";
const REDIRECT_URI: &str = "http://127.0.0.1:56121/callback";
const VERIFIER_BYTES: usize = 96;
const REFRESH_LEAD_SECS: i64 = 300;
const AUTH_TIMEOUT: Duration = Duration::from_secs(300);

const B64URL_CHARS: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

// ---- Crypto helpers --------------------------------------------------------

/// Standard FIPS 180-4 SHA-256 implementation.
fn sha256(data: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
        0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
        0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
        0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
        0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
        0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
        0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
    ];

    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a,
        0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
    ];

    let bit_len = (data.len() as u64) * 8;
    let mut msg = data.to_vec();
    msg.push(0x80);
    while (msg.len() % 64) != 56 {
        msg.push(0x00);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in msg.chunks_exact(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                chunk[i * 4],
                chunk[i * 4 + 1],
                chunk[i * 4 + 2],
                chunk[i * 4 + 3],
            ]);
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
        let mut mut_h = h[7];

        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = mut_h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);

            mut_h = g;
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
        h[7] = h[7].wrapping_add(mut_h);
    }

    let mut out = [0u8; 32];
    for (i, val) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&val.to_be_bytes());
    }
    out
}

/// RFC 4648 base64url encoding without padding.
fn b64url_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    let chunks = data.chunks_exact(3);
    let remainder = chunks.remainder();

    for chunk in chunks {
        let b0 = chunk[0] as usize;
        let b1 = chunk[1] as usize;
        let b2 = chunk[2] as usize;
        out.push(B64URL_CHARS[b0 >> 2] as char);
        out.push(B64URL_CHARS[((b0 & 0x03) << 4) | (b1 >> 4)] as char);
        out.push(B64URL_CHARS[((b1 & 0x0f) << 2) | (b2 >> 6)] as char);
        out.push(B64URL_CHARS[b2 & 0x3f] as char);
    }

    match remainder.len() {
        1 => {
            let b0 = remainder[0] as usize;
            out.push(B64URL_CHARS[b0 >> 2] as char);
            out.push(B64URL_CHARS[(b0 & 0x03) << 4] as char);
        }
        2 => {
            let b0 = remainder[0] as usize;
            let b1 = remainder[1] as usize;
            out.push(B64URL_CHARS[b0 >> 2] as char);
            out.push(B64URL_CHARS[((b0 & 0x03) << 4) | (b1 >> 4)] as char);
            out.push(B64URL_CHARS[(b1 & 0x0f) << 2] as char);
        }
        _ => {}
    }

    out
}

fn b64url_char_val(c: char) -> anyhow::Result<u8> {
    match c {
        'A'..='Z' => Ok(c as u8 - b'A'),
        'a'..='z' => Ok(c as u8 - b'a' + 26),
        '0'..='9' => Ok(c as u8 - b'0' + 52),
        '-' => Ok(62),
        '_' => Ok(63),
        _ => anyhow::bail!("invalid base64url character: {c}"),
    }
}

/// Decodes base64url string, accepting unpadded or padded input and rejecting invalid chars.
fn b64url_decode(s: &str) -> anyhow::Result<Vec<u8>> {
    let clean = s.trim_end_matches('=');
    if clean.is_empty() {
        return Ok(Vec::new());
    }
    let chars: Vec<char> = clean.chars().collect();
    let mut out = Vec::with_capacity(chars.len() * 3 / 4);

    let chunks = chars.chunks_exact(4);
    let remainder = chunks.remainder();

    for chunk in chunks {
        let v0 = b64url_char_val(chunk[0])? as usize;
        let v1 = b64url_char_val(chunk[1])? as usize;
        let v2 = b64url_char_val(chunk[2])? as usize;
        let v3 = b64url_char_val(chunk[3])? as usize;

        out.push(((v0 << 2) | (v1 >> 4)) as u8);
        out.push((((v1 & 0x0f) << 4) | (v2 >> 2)) as u8);
        out.push((((v2 & 0x03) << 6) | v3) as u8);
    }

    match remainder.len() {
        0 => {}
        2 => {
            let v0 = b64url_char_val(remainder[0])? as usize;
            let v1 = b64url_char_val(remainder[1])? as usize;
            out.push(((v0 << 2) | (v1 >> 4)) as u8);
        }
        3 => {
            let v0 = b64url_char_val(remainder[0])? as usize;
            let v1 = b64url_char_val(remainder[1])? as usize;
            let v2 = b64url_char_val(remainder[2])? as usize;
            out.push(((v0 << 2) | (v1 >> 4)) as u8);
            out.push((((v1 & 0x0f) << 4) | (v2 >> 2)) as u8);
        }
        _ => anyhow::bail!("invalid base64url length"),
    }

    Ok(out)
}

/// Generates `n` random bytes by concatenating UUID v4 bytes.
fn random_bytes(n: usize) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(n);
    while bytes.len() < n {
        bytes.extend_from_slice(Uuid::new_v4().as_bytes());
    }
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
    let mut bytes = Vec::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '+' => bytes.push(b' '),
            '%' => {
                let h1 = chars.next().ok_or_else(|| anyhow::anyhow!("incomplete percent escape"))?;
                let h2 = chars.next().ok_or_else(|| anyhow::anyhow!("incomplete percent escape"))?;
                let hex_str = format!("{h1}{h2}");
                let byte = u8::from_str_radix(&hex_str, 16)
                    .map_err(|_| anyhow::anyhow!("invalid hex in percent escape: %{hex_str}"))?;
                bytes.push(byte);
            }
            _ => {
                let mut buf = [0u8; 4];
                bytes.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            }
        }
    }
    String::from_utf8(bytes).map_err(|e| anyhow::anyhow!("invalid utf-8 in percent decode: {e}"))
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

pub fn save_auth_at(path: &Path, auth: &StoredAuth) -> anyhow::Result<()> {
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

    let json = serde_json::to_string_pretty(auth)?;
    file.write_all(json.as_bytes())?;
    file.flush()?;
    Ok(())
}

pub fn save_auth(auth: &StoredAuth) -> anyhow::Result<()> {
    save_auth_at(&auth_path()?, auth)
}

pub fn load_auth_at(path: &Path, provider: &str) -> anyhow::Result<StoredAuth> {
    let body = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            anyhow::bail!("not signed in to {provider}");
        }
        Err(e) => return Err(e.into()),
    };

    let auth: StoredAuth = serde_json::from_str(&body)?;
    if auth.provider != provider {
        anyhow::bail!("not signed in to {provider}");
    }
    Ok(auth)
}

pub fn load_auth(provider: &str) -> anyhow::Result<StoredAuth> {
    load_auth_at(&auth_path()?, provider)
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

async fn exchange_code(token_url: &str, code: &str, verifier: &str) -> anyhow::Result<TokenResponse> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?;
    let params = [
        ("grant_type", "authorization_code"),
        ("client_id", XAI_CLIENT_ID),
        ("code", code),
        ("redirect_uri", REDIRECT_URI),
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
        anyhow::bail!("xAI token exchange failed ({status}): {body}");
    }

    let tokens: TokenResponse = resp.json().await?;
    Ok(tokens)
}

async fn refresh_with(provider: &str, refresh_token: &str) -> anyhow::Result<StoredAuth> {
    let endpoints = discover_endpoints().await?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?;
    let params = [
        ("grant_type", "refresh_token"),
        ("client_id", XAI_CLIENT_ID),
        ("refresh_token", refresh_token),
    ];
    let resp = client
        .post(&endpoints.token_url)
        .header("Accept", "application/json")
        .form(&params)
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("xAI token refresh failed ({status}): {body}");
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
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => {
                out.push_str(&format!("%{b:02X}"));
            }
        }
    }
    out
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
    let encoded_redirect = percent_encode_uri(redirect_uri);
    let encoded_scope = scope.replace(' ', "%20");
    format!(
        "{authorize_url}?response_type=code&client_id={client_id}&redirect_uri={encoded_redirect}&scope={encoded_scope}&code_challenge={code_challenge}&code_challenge_method=S256&state={state}&nonce={nonce}&plan=generic&referrer=cli-proxy-api"
    )
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

// ---- Public API ------------------------------------------------------------

/// Runs interactive xAI OAuth browser sign-in flow.
pub async fn run_xai_signin() -> anyhow::Result<StoredAuth> {
    let endpoints = discover_endpoints().await?;
    let listener = TcpListener::bind(("127.0.0.1", LOOPBACK_PORT)).map_err(|e| {
        if e.kind() == std::io::ErrorKind::AddrInUse {
            anyhow::anyhow!("port 56121 busy — close whatever uses it and retry")
        } else {
            e.into()
        }
    })?;

    let verifier = generate_code_verifier();
    let challenge = generate_code_challenge(&verifier);
    let state = generate_state();
    let nonce = generate_state();
    let auth_url = build_auth_url(
        &endpoints.authorize_url,
        XAI_CLIENT_ID,
        REDIRECT_URI,
        SCOPE,
        &challenge,
        &state,
        &nonce,
    );

    println!("\nSign in with your x.ai account.");
    println!("Open this URL in your browser:\n{auth_url}\n");
    println!("(waiting up to 5 minutes…)");

    listener.set_nonblocking(true)?;
    let start = Instant::now();
    let (mut stream, _) = loop {
        match listener.accept() {
            Ok(conn) => break conn,
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if start.elapsed() >= AUTH_TIMEOUT {
                    anyhow::bail!("Authentication timeout (5 minutes)");
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
            Err(e) => return Err(e.into()),
        }
    };

    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;

    let mut buf = [0u8; 1024];
    let mut raw = Vec::new();
    while raw.len() < 8192 {
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                raw.extend_from_slice(&buf[..n]);
                if raw.windows(4).any(|w| w == b"\r\n\r\n") || raw.windows(2).any(|w| w == b"\n\n") {
                    break;
                }
            }
            Err(ref e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                break
            }
            Err(e) => return Err(e.into()),
        }
    }
    let head = String::from_utf8_lossy(&raw).into_owned();

    let is_callback = head
        .lines()
        .next()
        .map(|line| {
            let parts: Vec<&str> = line.split_whitespace().collect();
            parts.len() >= 2 && parts[1].starts_with(CALLBACK_PATH)
        })
        .unwrap_or(false);

    if is_callback {
        let body = "<!DOCTYPE html><html><body>&#10003; Signed in — you can close this tab and return to your terminal.</body></html>";
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(resp.as_bytes());
    } else {
        let resp = "HTTP/1.1 404 Not Found\r\nContent-Length: 9\r\nConnection: close\r\n\r\nNot found";
        let _ = stream.write_all(resp.as_bytes());
    }
    let _ = stream.flush();
    drop(stream);

    let params = parse_callback_line(&head)?;
    if let Some(err) = params.error {
        let desc = params.error_description.unwrap_or(err);
        anyhow::bail!("{desc}");
    }
    let code = params
        .code
        .ok_or_else(|| anyhow::anyhow!("No authorization code received"))?;
    if params.state.as_deref() != Some(&state) {
        anyhow::bail!("Invalid state parameter");
    }

    let tokens = exchange_code(&endpoints.token_url, &code, &verifier).await?;
    let email = tokens.id_token.as_deref().and_then(decode_id_token_email);
    let stored = StoredAuth {
        provider: "xai".to_string(),
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
    if saved.auth_mode.as_deref() == Some("oauth")
        && config.api_key.is_none()
        && let Ok(token) = ensure_access_token("xai").await
    {
        config.api_key = Some(token);
    }
}

// ---- Unit tests ------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_known_vectors() {
        let empty = sha256(b"");
        let empty_hex: String = empty.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(
            empty_hex,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );

        let abc = sha256(b"abc");
        let abc_hex: String = abc.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(
            abc_hex,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn challenge_matches_rfc7636_appendix_b() {
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let challenge = generate_code_challenge(verifier);
        assert_eq!(challenge, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }

    #[test]
    fn b64url_fixed_vectors() {
        assert_eq!(b64url_encode(b""), "");
        assert_eq!(b64url_encode(b"f"), "Zg");
        assert_eq!(b64url_encode(b"fo"), "Zm8");
        assert_eq!(b64url_encode(b"foo"), "Zm9v");

        let pattern: Vec<u8> = (0..96).map(|i| (i * 7 + 13) as u8).collect();
        let encoded = b64url_encode(&pattern);
        let decoded = b64url_decode(&encoded).expect("decode should succeed");
        assert_eq!(decoded, pattern);

        assert!(b64url_decode("abc+").is_err());
        assert!(b64url_decode("abc/").is_err());

        let with_pad = format!("{encoded}==");
        let decoded_pad = b64url_decode(&with_pad).expect("padding should be ignored");
        assert_eq!(decoded_pad, pattern);
    }

    #[test]
    fn verifier_shape() {
        let v1 = generate_code_verifier();
        let v2 = generate_code_verifier();
        assert_eq!(v1.len(), 128);
        assert_eq!(v2.len(), 128);
        assert!(v1.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
        assert!(v2.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
        assert_ne!(v1, v2);
    }

    #[test]
    fn state_shape() {
        let s1 = generate_state();
        let s2 = generate_state();
        assert_eq!(s1.len(), 32);
        assert_eq!(s2.len(), 32);
        assert!(s1.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        assert!(s2.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        assert_ne!(s1, s2);
    }

    #[test]
    fn parse_callback_line_cases() {
        let line1 = "GET /callback?code=abc&state=xyz HTTP/1.1";
        let p1 = parse_callback_line(line1).unwrap();
        assert_eq!(p1.code.as_deref(), Some("abc"));
        assert_eq!(p1.state.as_deref(), Some("xyz"));
        assert!(p1.error.is_none());

        let line2 = "GET /callback?error=access_denied&error_description=User%20denied HTTP/1.1";
        let p2 = parse_callback_line(line2).unwrap();
        assert_eq!(p2.error.as_deref(), Some("access_denied"));
        assert_eq!(p2.error_description.as_deref(), Some("User denied"));
        assert!(p2.code.is_none());

        let line3 = "GET /callback?state=xyz HTTP/1.1";
        let p3 = parse_callback_line(line3).unwrap();
        assert!(p3.code.is_none());

        let line4 = "GET /callback?code=a%20b+c HTTP/1.1";
        let p4 = parse_callback_line(line4).unwrap();
        assert_eq!(p4.code.as_deref(), Some("a b c"));
    }

    #[test]
    fn id_token_email_variants() {
        let header = b64url_encode(b"{\"alg\":\"none\"}");
        let sig = b64url_encode(b"sig");

        let payload1 = b64url_encode(b"{\"email\":\"a@b.c\"}");
        let jwt1 = format!("{header}.{payload1}.{sig}");
        assert_eq!(decode_id_token_email(&jwt1), Some("a@b.c".to_string()));

        let payload2 = b64url_encode(b"{\"preferred_username\":\"u\"}");
        let jwt2 = format!("{header}.{payload2}.{sig}");
        assert_eq!(decode_id_token_email(&jwt2), Some("u".to_string()));

        let payload3 = b64url_encode(b"{\"sub\":\"s1\"}");
        let jwt3 = format!("{header}.{payload3}.{sig}");
        assert_eq!(decode_id_token_email(&jwt3), Some("s1".to_string()));

        assert_eq!(decode_id_token_email("not-a-jwt"), None);
    }

    #[test]
    fn build_auth_url_encodes_scope() {
        let url = build_auth_url(
            AUTHORIZE_FALLBACK,
            XAI_CLIENT_ID,
            REDIRECT_URI,
            SCOPE,
            "challenge123",
            "state123",
            "nonce123",
        );
        assert!(url.contains("scope=openid%20profile%20email%20offline_access%20grok-cli:access%20api:access"));
        assert!(url.contains("&code_challenge_method=S256"));
        assert!(url.contains("response_type=code"));
    }

    #[test]
    fn auth_json_round_trip_and_perms() {
        let dir = std::env::temp_dir().join(format!("gray-oauth-test-{}", Uuid::new_v4()));
        let path = dir.join("auth.json");

        let auth = StoredAuth {
            provider: "xai".to_string(),
            access_token: "access_token_123".to_string(),
            refresh_token: "refresh_token_123".to_string(),
            expires_at: 1234567890,
            email: Some("test@x.ai".to_string()),
        };

        save_auth_at(&path, &auth).unwrap();
        assert!(path.exists());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::metadata(&path).unwrap().permissions();
            assert_eq!(perms.mode() & 0o777, 0o600);
        }

        let loaded = load_auth_at(&path, "xai").unwrap();
        assert_eq!(loaded.provider, auth.provider);
        assert_eq!(loaded.access_token, auth.access_token);
        assert_eq!(loaded.refresh_token, auth.refresh_token);
        assert_eq!(loaded.expires_at, auth.expires_at);
        assert_eq!(loaded.email, auth.email);

        assert!(load_auth_at(&path, "openai").is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn endpoint_validation() {
        assert!(validate_endpoint("http://auth.x.ai/oauth2/token", "token_endpoint").is_err());
        assert!(validate_endpoint("https://evil.com/oauth2/token", "token_endpoint").is_err());
        assert!(validate_endpoint("https://auth.x.ai/oauth2/token", "token_endpoint").is_ok());
    }
}
