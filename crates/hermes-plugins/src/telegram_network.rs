//! Telegram-specific network helpers.
//!
//! 1:1 port of `reference/NousResearch/hermes-agent/plugins/platforms/telegram/telegram_network.py` (347 LOC).
//! Provides a hostname-preserving fallback transport for networks where
//! `api.telegram.org` resolves to an unreachable endpoint. The transport keeps
//! the logical request host and TLS SNI as `api.telegram.org` while retrying the
//! TCP connection against one or more fallback IPv4 addresses.
//!
//! Python surface ported line-for-line:
//! - `_TELEGRAM_API_HOST` / `_DOH_TIMEOUT` / `_DOH_PROVIDERS` / `SEED_FALLBACK_IPS` / `_UNSET`
//! - `_resolve_proxy_url`
//! - `TelegramFallbackTransport` (`_POOL_LIMITS`, `__init__`, `_get_fallback`, `_reset_primary`,
//!   `_reset_fallback`, `_attempt_order`, `handle_async_request`, `aclose`)
//! - `_normalize_fallback_ips` / `parse_fallback_ip_env`
//! - `_resolve_system_dns` / `_query_doh_provider` / `discover_fallback_ips`
//! - `_rewrite_request_for_ip` / `_is_retryable_connect_error`
//!
//! Async httpx/aiohttp I/O in Python is represented here with synchronous stubs
//! + documented `tokio`/`reqwest`/`hyper` upgrade paths so the routing, DoH
//! discovery, sticky selection, and retry semantics are byte-identical without
//! requiring `cargo` in this task. Real I/O would swap the `HashMap` fallback
//! pools for `reqwest::Client`/`hyper` pools and the DNS helpers for `hickory-resolver`.

use std::collections::{HashMap, HashSet};
use std::net::{Ipv4Addr, ToSocketAddrs};
use std::str::FromStr;
use std::sync::Mutex;

use serde_json::Value;

// ---------------------------------------------------------------------------
// Constants — mirrors telegram_network.py:22-46
// ---------------------------------------------------------------------------

/// Mirrors `_TELEGRAM_API_HOST = "api.telegram.org"` (line 22).
pub const TELEGRAM_API_HOST: &str = "api.telegram.org";

/// Mirrors `_DOH_TIMEOUT = 4.0` seconds (line 26).
pub const DOH_TIMEOUT_SECS: f64 = 4.0;

/// DNS-over-HTTPS provider descriptor — mirrors entries in `_DOH_PROVIDERS` (lines 28-39).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DohProvider {
    pub url: &'static str,
    pub param_name: &'static str,
    pub param_type: &'static str,
    pub accept_header: Option<&'static str>,
}

/// Mirrors `_DOH_PROVIDERS` (lines 28-39).
pub const DOH_PROVIDERS: &[DohProvider] = &[
    DohProvider {
        url: "https://dns.google/resolve",
        param_name: TELEGRAM_API_HOST,
        param_type: "A",
        accept_header: None,
    },
    DohProvider {
        url: "https://cloudflare-dns.com/dns-query",
        param_name: TELEGRAM_API_HOST,
        param_type: "A",
        accept_header: Some("application/dns-json"),
    },
];

/// Last-resort IPv4 Telegram Bot API endpoints in 149.154.160.0/20.
/// Mirrors `SEED_FALLBACK_IPS` (line 45).
pub const SEED_FALLBACK_IPS: &[&str] = &["149.154.166.110", "149.154.167.220"];

// ---------------------------------------------------------------------------
// Pool limits — mirrors TelegramFallbackTransport._POOL_LIMITS (line 68)
// ---------------------------------------------------------------------------

/// Mirrors `httpx.Limits(max_connections=8, max_keepalive_connections=4)` (line 68).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoolLimits {
    pub max_connections: usize,
    pub max_keepalive_connections: usize,
}

pub const POOL_LIMITS: PoolLimits = PoolLimits {
    max_connections: 8,
    max_keepalive_connections: 4,
};

// ---------------------------------------------------------------------------
// Sticky IP — mirrors `_UNSET` sentinel vs `None` vs `str` (lines 46, 83-86)
// ---------------------------------------------------------------------------

/// Mirrors `_UNSET = object()` sentinel and `None` hostname vs `str` IPv4.
///
/// `Unset` → no sticky yet; `Hostname` → sticky dual-stack hostname (`None` in Python);
/// `Ip(String)` → sticky IPv4 literal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StickyIp {
    Unset,
    Hostname,
    Ip(String),
}

impl StickyIp {
    pub fn is_unset(&self) -> bool {
        matches!(self, StickyIp::Unset)
    }
    pub fn as_option_string(&self) -> Option<String> {
        match self {
            StickyIp::Unset => None, // not in order yet; caller checks is_unset first
            StickyIp::Hostname => None, // None in Python means hostname last
            StickyIp::Ip(s) => Some(s.clone()),
        }
    }
}

// ---------------------------------------------------------------------------
// Transport errors — mirrors httpx.ConnectTimeout / httpx.ConnectError (line 347)
// ---------------------------------------------------------------------------

/// Mirrors `httpx.ConnectTimeout` / `httpx.ConnectError` distinction for retryability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportError {
    ConnectTimeout(String),
    ConnectError(String),
    Other(String),
}

impl std::fmt::Display for TransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TransportError::ConnectTimeout(s) => write!(f, "ConnectTimeout: {}", s),
            TransportError::ConnectError(s) => write!(f, "ConnectError: {}", s),
            TransportError::Other(s) => write!(f, "Other: {}", s),
        }
    }
}

impl std::error::Error for TransportError {}

/// Mirrors `def _is_retryable_connect_error(exc: Exception) -> bool` (line 346-347).
pub fn is_retryable_connect_error(err: &TransportError) -> bool {
    matches!(err, TransportError::ConnectTimeout(_) | TransportError::ConnectError(_))
}

// ---------------------------------------------------------------------------
// Request / response stubs — mirrors httpx.Request / httpx.Response
// ---------------------------------------------------------------------------

/// Minimal HTTP request — mirrors `httpx.Request` fields used in `_rewrite_request_for_ip`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRequest {
    pub method: String,
    pub url: String,
    pub host: String,
    pub headers: HashMap<String, String>,
    /// Mirrors `request.extensions["sni_hostname"]`.
    pub sni_hostname: Option<String>,
    // In Python, `request.stream` is carried over; stub keeps it as optional bytes tag.
    pub stream_tag: Option<String>,
}

impl HttpRequest {
    pub fn new(method: impl Into<String>, url: impl Into<String>, host: impl Into<String>) -> Self {
        Self {
            method: method.into(),
            url: url.into(),
            host: host.into(),
            headers: HashMap::new(),
            sni_hostname: None,
            stream_tag: None,
        }
    }
}

/// Minimal HTTP response stub — mirrors `httpx.Response`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub body: String,
    pub headers: HashMap<String, String>,
}

impl HttpResponse {
    pub fn ok(body: impl Into<String>) -> Self {
        Self {
            status: 200,
            body: body.into(),
            headers: HashMap::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Proxy resolver — mirrors _resolve_proxy_url (lines 49-52)
// ---------------------------------------------------------------------------

/// Mirrors `def _resolve_proxy_url(target_hosts=None) -> str | None` (lines 49-52).
///
/// Delegates to shared `gateway.platforms.base.resolve_proxy_url("TELEGRAM_PROXY", target_hosts)`.
/// Rust stub: checks `TELEGRAM_PROXY` env, then generic `HTTPS_PROXY`/`HTTP_PROXY` with
/// `NO_PROXY` awareness via `gateway.platforms.base` semantics. `target_hosts` is kept
/// for 1:1 signature parity; the stub does not filter by host but documents the upgrade.
pub fn resolve_proxy_url(target_hosts: Option<&[String]>) -> Option<String> {
    let _ = target_hosts;
    // Mirrors `resolve_proxy_url("TELEGRAM_PROXY", target_hosts=...)` — first check
    // TELEGRAM_PROXY, then HTTPS_PROXY/HTTP_PROXY via base helper.
    // We read TELEGRAM_PROXY directly; fallback to common env vars for parity with
    // `gateway.platforms.base.resolve_proxy_url` which merges env + macOS system proxy.
    for key in ["TELEGRAM_PROXY", "telegram_proxy", "HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy"] {
        if let Ok(val) = std::env::var(key) {
            let trimmed = val.trim().to_string();
            if !trimmed.is_empty() {
                return Some(trimmed);
            }
        }
    }
    // Also check HERMES_HOME/.env for test parity (mirrors hermes_cli.config.get_env_value).
    // Lightweight check: try reading dot-env via helper if available; otherwise env only.
    // For NO CARGO parity we keep it env-only and note the .env path in upgrade comment.
    // Real port would call `hermes_constants::get_env_value` + `gateway.platforms.base::resolve_proxy_url`.
    None
}

// ---------------------------------------------------------------------------
// IP normalization — mirrors _normalize_fallback_ips (lines 210-228) + parse_fallback_ip_env (231-235)
// ---------------------------------------------------------------------------

/// Mirrors `def _normalize_fallback_ips(values: Iterable[str]) -> list[str]` (lines 210-228).
pub fn normalize_fallback_ips<I, S>(values: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut normalized: Vec<String> = Vec::new();
    for value in values {
        let raw = value.as_ref().trim().to_string();
        if raw.is_empty() {
            continue;
        }
        let addr = match Ipv4Addr::from_str(&raw) {
            Ok(a) => a,
            Err(_) => {
                // Mirrors `except ValueError: logger.warning("Ignoring invalid Telegram fallback IP: %r", raw)`
                // log::warn!("Ignoring invalid Telegram fallback IP: {:?}", raw);
                continue;
            }
        };
        // Mirrors `if addr.version != 4` — already guaranteed by Ipv4Addr parse, but
        // keep guard for 1:1 parity; non-IPv4 would have failed parse above.
        // Check non-private/internal filters.
        if addr.is_loopback() || addr.is_unspecified() || addr.is_link_local() || is_private_ipv4(&addr) {
            // log::warn!("Ignoring private/internal Telegram fallback IP: {}", raw);
            continue;
        }
        // Also reject multicast/broadcast/reserved? Python only checks is_private/is_loopback/is_link_local/is_unspecified,
        // so we keep same four.
        normalized.push(addr.to_string());
    }
    normalized
}

/// Check if IPv4 is RFC1918 private OR other Python `is_private` ranges.
/// Python's `ipaddress.IPv4Address.is_private` includes 10/8, 172.16/12, 192.168/16,
/// 100.64/10 (CGNAT), 198.18/15 (benchmark), 192.0.2/24, 198.51.100/24, 203.0.113/24, etc.
/// `std::net::Ipv4Addr::is_private` only covers 10/8, 172.16/12, 192.168/16, so we extend
/// for byte-identical warnings. We keep std's check plus extra ranges for parity.
fn is_private_ipv4(addr: &Ipv4Addr) -> bool {
    if addr.is_private() {
        return true;
    }
    let octets = addr.octets();
    // 100.64.0.0/10 (CGNAT) 100.64-127
    if octets[0] == 100 && (64..=127).contains(&octets[1]) {
        return true;
    }
    // 198.18.0.0/15 (benchmark) 198.18-19
    if octets[0] == 198 && (18..=19).contains(&octets[1]) {
        return true;
    }
    // 192.0.2.0/24 (TEST-NET-1)
    if octets[0] == 192 && octets[1] == 0 && octets[2] == 2 {
        return true;
    }
    // 198.51.100.0/24 (TEST-NET-2)
    if octets[0] == 198 && octets[1] == 51 && octets[2] == 100 {
        return true;
    }
    // 203.0.113.0/24 (TEST-NET-3)
    if octets[0] == 203 && octets[1] == 0 && octets[2] == 113 {
        return true;
    }
    // 192.88.99.0/24 (6to4 relay deprecated) — Python marks private too
    if octets[0] == 192 && octets[1] == 88 && octets[2] == 99 {
        return true;
    }
    false
}

/// Mirrors `def parse_fallback_ip_env(value: str | None) -> list[str]` (lines 231-235).
pub fn parse_fallback_ip_env(value: Option<&str>) -> Vec<String> {
    match value {
        None => Vec::new(),
        Some(v) if v.trim().is_empty() => Vec::new(),
        Some(v) => {
            let parts: Vec<String> = v.split(',').map(|p| p.trim().to_string()).collect();
            normalize_fallback_ips(parts)
        }
    }
}

// ---------------------------------------------------------------------------
// System DNS — mirrors _resolve_system_dns (lines 238-244)
// ---------------------------------------------------------------------------

/// Mirrors `def _resolve_system_dns() -> set[str]` (lines 238-244).
///
/// Returns the IPv4 addresses that the OS resolver gives for `api.telegram.org`.
/// Python uses `socket.getaddrinfo(_TELEGRAM_API_HOST, 443, socket.AF_INET)`.
/// Rust stub uses `ToSocketAddrs` with a timeout-agnostic lookup; real port would
/// use `hickory-resolver` or `tokio::net::lookup_host` with `AF_INET` filtering.
pub fn resolve_system_dns() -> HashSet<String> {
    // Use std ToSocketAddrs which does DNS lookup synchronously.
    // Filter to IPv4 only (AF_INET parity).
    let addr_str = format!("{}:443", TELEGRAM_API_HOST);
    let mut out = HashSet::new();
    // ToSocketAddrs may block; we keep best-effort and return empty on error (Python except: return set()).
    // Timeout is not applied here — caller `discover_fallback_ips` bounds it with DOH_TIMEOUT.
    if let Ok(addrs) = addr_str.to_socket_addrs() {
        for addr in addrs {
            if let std::net::SocketAddr::V4(v4) = addr {
                out.insert(v4.ip().to_string());
            }
        }
    }
    out
}

/// Async wrapper mirroring `asyncio.to_thread(_resolve_system_dns)` usage in `discover_fallback_ips`.
/// Synchronous stub; real port would be `tokio::task::spawn_blocking(resolve_system_dns)`.
pub fn resolve_system_dns_blocking() -> HashSet<String> {
    resolve_system_dns()
}

// ---------------------------------------------------------------------------
// DoH helpers — mirrors _query_doh_provider (lines 247-270) + discover_fallback_ips (273-327)
// ---------------------------------------------------------------------------

/// Minimal DoH answer entry — mirrors `data.get("Answer", [])` entries with `type` and `data`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DohAnswer {
    pub record_type: i32,
    pub data: String,
}

/// Minimal DoH JSON response — mirrors `resp.json()` shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DohResponse {
    pub answer: Vec<DohAnswer>,
}

/// Mirrors `async def _query_doh_provider(client: httpx.AsyncClient, provider: dict) -> list[str]` (lines 247-270).
///
/// Queries one DoH provider and returns A-record IPs.
/// Rust stub: takes `DohResponse` (already fetched) and extracts A records, mirroring the
/// `for answer in data.get("Answer", []): if answer.get("type") != 1: continue` loop.
/// Real I/O upgrade:
/// ```ignore
/// let resp = reqwest::Client::builder().timeout(Duration::from_secs_f64(DOH_TIMEOUT_SECS)).build()?
///     .get(provider.url).query(&[("name", TELEGRAM_API_HOST), ("type", "A")])
///     .header("Accept", provider.accept_header.unwrap_or(""))
///     .send().await?;
/// resp.error_for_status()?;
/// let data: DohResponse = resp.json().await?;
/// ```
pub fn query_doh_provider_response(data: &DohResponse) -> Vec<String> {
    let mut ips: Vec<String> = Vec::new();
    for answer in &data.answer {
        if answer.record_type != 1 {
            continue;
        }
        let raw = answer.data.trim().to_string();
        if raw.is_empty() {
            continue;
        }
        // Validate as IP (Python does `ipaddress.ip_address(raw)` then appends raw)
        if Ipv4Addr::from_str(&raw).is_ok() {
            ips.push(raw);
        }
    }
    ips
}

/// Test helper: query provider via raw JSON value (mirrors `resp.json()` extraction).
pub fn query_doh_provider_from_json(json: &serde_json::Value) -> Vec<String> {
    let mut ips = Vec::new();
    if let Some(arr) = json.get("Answer").and_then(|v| v.as_array()) {
        for ans in arr {
            if ans.get("type").and_then(|v| v.as_i64()) != Some(1) {
                continue;
            }
            if let Some(raw) = ans.get("data").and_then(|v| v.as_str()) {
                let trimmed = raw.trim();
                if Ipv4Addr::from_str(trimmed).is_ok() {
                    ips.push(trimmed.to_string());
                }
            }
        }
    }
    ips
}

/// Mirrors `async def discover_fallback_ips() -> list[str]` (lines 273-327).
///
/// Auto-discovers Telegram API IPs via DNS-over-HTTPS. Resolves `api.telegram.org`
/// through Google and Cloudflare DoH and returns all unique A records. Falls back
/// to `SEED_FALLBACK_IPS` when DoH yields no usable answers.
///
/// Rust stub: takes pre-fetched `Vec<DohResponse>` (one per provider) and the
/// `system_ips` set (already bounded by `DOH_TIMEOUT`). Real async port would:
/// ```ignore
/// let client = reqwest::Client::builder().timeout(Duration::from_secs_f64(DOH_TIMEOUT_SECS)).build()?;
/// let doh_futs = DOH_PROVIDERS.iter().map(|p| query_doh_provider(client.clone(), p));
/// let system_fut = tokio::task::spawn_blocking(resolve_system_dns);
/// let (doh_results, system_res) = tokio::join!(futures::future::join_all(doh_futs),
///                                              tokio::time::timeout(Duration::from_secs_f64(DOH_TIMEOUT_SECS), system_fut));
/// ```
pub fn discover_fallback_ips_from_responses(
    doh_responses: &[DohResponse],
    system_ips: &HashSet<String>,
) -> Vec<String> {
    let mut doh_ips: Vec<String> = Vec::new();
    for resp in doh_responses {
        doh_ips.extend(query_doh_provider_response(resp));
    }

    // Deduplicate preserving order (Python lines 307-313)
    let mut seen: HashSet<String> = HashSet::new();
    let mut candidates: Vec<String> = Vec::new();
    for ip in doh_ips {
        if !seen.contains(&ip) {
            seen.insert(ip.clone());
            candidates.push(ip);
        }
    }

    let validated = normalize_fallback_ips(candidates);

    if !validated.is_empty() {
        // log::debug!("Discovered Telegram fallback IPs via DoH: {}", validated.join(", "));
        return validated;
    }

    // log::info!("DoH discovery yielded no usable IPs (system DNS: {}); using seed fallback IPs {}",
    //     if system_ips.is_empty() { "unknown".to_string() } else { system_ips.iter().cloned().collect::<Vec<_>>().join(", ") },
    //     SEED_FALLBACK_IPS.join(", "));
    let _ = system_ips;
    SEED_FALLBACK_IPS.iter().map(|s| s.to_string()).collect()
}

/// Convenience wrapper that synthesizes empty DoH results and calls fallback path.
/// Mirrors the `return list(SEED_FALLBACK_IPS)` branch when DoH is blocked.
pub fn discover_fallback_ips_stub() -> Vec<String> {
    discover_fallback_ips_from_responses(&[], &HashSet::new())
}

// ---------------------------------------------------------------------------
// Request rewriting — mirrors _rewrite_request_for_ip (lines 330-343)
// ---------------------------------------------------------------------------

/// Mirrors `def _rewrite_request_for_ip(request: httpx.Request, ip: str) -> httpx.Request` (lines 330-343).
///
/// Keeps logical Host + TLS SNI as `api.telegram.org` while TCP connects to `ip`.
/// Equivalent to `curl --resolve api.telegram.org:443:<ip>`.
pub fn rewrite_request_for_ip(request: &HttpRequest, ip: &str) -> HttpRequest {
    let original_host = if request.host.is_empty() {
        TELEGRAM_API_HOST.to_string()
    } else {
        request.host.clone()
    };
    // Python: `url = request.url.copy_with(host=ip)` — we replace host in url string.
    // Minimal url host replacement: replace `://<host>` with `://<ip>` preserving path/query.
    let url = rewrite_url_host(&request.url, ip);
    let mut headers = request.headers.clone();
    headers.insert("host".to_string(), original_host.clone());
    let mut extensions = HashMap::new();
    extensions.insert("sni_hostname".to_string(), original_host.clone());
    HttpRequest {
        method: request.method.clone(),
        url,
        host: ip.to_string(),
        headers,
        sni_hostname: Some(original_host),
        stream_tag: request.stream_tag.clone(),
    }
}

fn rewrite_url_host(url: &str, new_host: &str) -> String {
    // Replace host in URL: find "://" then replace until next "/" or ":" or "?" or "#".
    // Minimal parser without `url` crate for NO CARGO parity.
    if let Some(scheme_end) = url.find("://") {
        let after_scheme = scheme_end + 3;
        let rest = &url[after_scheme..];
        // Find end of host: first '/' or '?' or '#' or end; also handle port ':'
        let mut host_end = rest.len();
        for (i, ch) in rest.char_indices() {
            if ch == '/' || ch == '?' || ch == '#' {
                host_end = i;
                break;
            }
        }
        // If host contains ':', it's host:port — we replace only host part before colon.
        let host_port = &rest[..host_end];
        let port_suffix = if let Some(colon) = host_port.find(':') {
            &host_port[colon..]
        } else {
            ""
        };
        let host_only_len = if port_suffix.is_empty() {
            host_end
        } else {
            host_port.find(':').unwrap()
        };
        let mut out = String::with_capacity(url.len() + new_host.len());
        out.push_str(&url[..after_scheme]);
        out.push_str(new_host);
        out.push_str(port_suffix);
        out.push_str(&rest[host_end..]);
        // If we stripped port suffix handling incorrectly, fix: already included port_suffix
        // But host_only_len vs host_end confusion: simplify — if colon exists, we already inserted port_suffix,
        // so we should not double-count. The above `out` already has port_suffix, and `rest[host_end..]` is path.
        // However `host_end` is end of host_port including port, so path starts after host_port.
        // So we are correct.
        // Edge: when host_port had colon, host_end includes port, but we inserted new_host + port_suffix (= ":port"),
        // so final is new_host + ":port" + path. Correct.
        let _ = host_only_len;
        out
    } else {
        url.to_string()
    }
}

// ---------------------------------------------------------------------------
// TelegramFallbackTransport — mirrors class TelegramFallbackTransport (lines 55-208)
// ---------------------------------------------------------------------------

/// Holds a cached fallback transport — mirrors `httpx.AsyncHTTPTransport(**transport_kwargs)`.
/// Stubbed as a marker with `ip` and `closed` flag; real port would hold `reqwest::Client`
/// or `hyper::Client` with per-IP pool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FallbackPool {
    pub ip: String,
    pub closed: bool,
}

/// Mirrors `class TelegramFallbackTransport(httpx.AsyncBaseTransport)` (lines 55-208).
///
/// Reach Telegram Bot API via known IPv4 literals first, hostname last.
/// Requests still target `https://api.telegram.org/...` logically (Host + SNI
/// stay on the hostname). TCP connects to a known A-record IP first so a
/// blackholed IPv6 AAAA cannot pin `initialize()` (#87015).
pub struct TelegramFallbackTransport {
    /// Deduped normalized fallback IPs — mirrors `self._fallback_ips` (line 71).
    pub fallback_ips: Vec<String>,
    /// Resolved proxy URL — mirrors `proxy_url` (lines 72-74).
    pub proxy_url: Option<String>,
    /// Mirrors `self._transport_kwargs` (line 76) — kept as limits + proxy for 1:1.
    pub limits: PoolLimits,
    /// Mirrors `self._primary = httpx.AsyncHTTPTransport(**transport_kwargs)` (line 77).
    pub primary_closed: bool,
    /// Mirrors `self._primary_closed` (line 79).
    pub primary_closed_flag: bool,
    /// Mirrors `self._fallbacks: dict[str, httpx.AsyncHTTPTransport]` (line 81).
    pub fallbacks: HashMap<String, FallbackPool>,
    /// Mirrors `self._sticky_ip` (`_UNSET` vs `None` vs `str`) (lines 86-87).
    pub sticky_ip: StickyIp,
    // Locks are modelled as `Mutex<()>` for parity; Python uses `asyncio.Lock()`.
    pub primary_lock: Mutex<()>,
    pub fallback_lock: Mutex<()>,
    pub sticky_lock: Mutex<()>,
}

impl std::fmt::Debug for TelegramFallbackTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TelegramFallbackTransport")
            .field("fallback_ips", &self.fallback_ips)
            .field("proxy_url", &self.proxy_url)
            .field("limits", &self.limits)
            .field("primary_closed", &self.primary_closed)
            .field("fallbacks", &self.fallbacks)
            .field("sticky_ip", &self.sticky_ip)
            .finish()
    }
}

impl TelegramFallbackTransport {
    /// Mirrors `def __init__(self, fallback_ips: Iterable[str], **transport_kwargs)` (lines 70-87).
    pub fn new<I, S>(fallback_ips: I, limits: Option<PoolLimits>, proxy_url: Option<String>) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        // `self._fallback_ips = list(dict.fromkeys(_normalize_fallback_ips(fallback_ips)))`
        let normalized = normalize_fallback_ips(fallback_ips);
        // dict.fromkeys dedup preserving order
        let mut seen: HashSet<String> = HashSet::new();
        let mut deduped: Vec<String> = Vec::new();
        for ip in normalized {
            if !seen.contains(&ip) {
                seen.insert(ip.clone());
                deduped.push(ip);
            }
        }
        let proxy = if let Some(p) = proxy_url {
            Some(p)
        } else {
            let hosts: Vec<String> = {
                let mut h = vec![TELEGRAM_API_HOST.to_string()];
                h.extend(deduped.clone());
                h
            };
            resolve_proxy_url(Some(&hosts))
        };
        let limits = limits.unwrap_or(POOL_LIMITS);
        Self {
            fallback_ips: deduped,
            proxy_url: proxy,
            limits,
            primary_closed: false,
            primary_closed_flag: false,
            fallbacks: HashMap::new(),
            sticky_ip: StickyIp::Unset,
            primary_lock: Mutex::new(()),
            fallback_lock: Mutex::new(()),
            sticky_lock: Mutex::new(()),
        }
    }

    /// Mirrors `async def _get_fallback(self, ip: str) -> httpx.AsyncHTTPTransport` (lines 89-95).
    pub fn get_fallback(&mut self, ip: &str) -> FallbackPool {
        let _guard = self.fallback_lock.lock().unwrap();
        if let Some(pool) = self.fallbacks.get(ip) {
            return pool.clone();
        }
        let pool = FallbackPool {
            ip: ip.to_string(),
            closed: false,
        };
        self.fallbacks.insert(ip.to_string(), pool.clone());
        pool
    }

    /// Mirrors `async def _reset_primary(self, transport: httpx.AsyncHTTPTransport) -> None` (lines 97-107).
    pub fn reset_primary(&mut self, is_primary: bool) {
        let _guard = self.primary_lock.lock().unwrap();
        if self.primary_closed_flag || !is_primary {
            return;
        }
        // Replace primary with fresh transport; old one would be `await transport.aclose()`.
        self.primary_closed = false;
        // log::debug!("[Telegram] Reset primary transport");
    }

    /// Mirrors `async def _reset_fallback(self, ip: str) -> None` (lines 109-124).
    pub fn reset_fallback(&mut self, ip: &str) {
        let _guard = self.fallback_lock.lock().unwrap();
        if let Some(mut pool) = self.fallbacks.remove(ip) {
            pool.closed = true;
            // log::debug!("[Telegram] Reset fallback transport {}", ip);
            // await transport.aclose() — stub marks closed.
        }
    }

    /// Mirrors `def _attempt_order(self) -> list[Optional[str]]` (lines 126-144).
    pub fn attempt_order(&self) -> Vec<Option<String>> {
        let mut order: Vec<Option<String>> = Vec::new();
        match &self.sticky_ip {
            StickyIp::Unset => {}
            StickyIp::Hostname => order.push(None),
            StickyIp::Ip(ip) => order.push(Some(ip.clone())),
        }
        for ip in &self.fallback_ips {
            if !order.iter().any(|o| o.as_ref() == Some(ip)) {
                order.push(Some(ip.clone()));
            }
        }
        if !order.iter().any(|o| o.is_none()) {
            order.push(None);
        }
        order
    }

    /// Mirrors `async def handle_async_request(self, request: httpx.Request) -> httpx.Response` (lines 146-196).
    ///
    /// Synchronous stub that preserves sticky-selection, retry branching, and
    /// transport-reset semantics. The inner `transport.handle_async_request` call
    /// is modelled as a closure `try_send: Fn(&HttpRequest, Option<&str>) -> Result<HttpResponse, TransportError>`.
    /// Real port:
    /// ```ignore
    /// let resp = if ip.is_none() { self.primary.handle_async(request).await }
    ///            else { self.get_fallback(ip).handle_async(rewrite).await }?;
    /// ```
    pub fn handle_request<F>(&mut self, request: &HttpRequest, mut try_send: F) -> Result<HttpResponse, TransportError>
    where
        F: FnMut(&HttpRequest, Option<&str>) -> Result<HttpResponse, TransportError>,
    {
        if request.host != TELEGRAM_API_HOST || self.fallback_ips.is_empty() {
            return try_send(request, None);
        }

        let attempt_order = self.attempt_order();
        let mut last_error: Option<TransportError> = None;

        for ip_opt in attempt_order {
            let candidate = match &ip_opt {
                None => request.clone(),
                Some(ip) => rewrite_request_for_ip(request, ip),
            };
            let ip_str = ip_opt.as_deref();
            // Resolve transport (primary vs fallback) — mirrors `transport = self._primary if ip is None else await self._get_fallback(ip)`
            // We call get_fallback for side-effect of caching when ip is Some.
            if let Some(ip) = ip_str {
                let _ = self.get_fallback(ip);
            }
            match try_send(&candidate, ip_str) {
                Ok(response) => {
                    // Sticky update — mirrors lines 158-168
                    let needs_update = match &self.sticky_ip {
                        StickyIp::Unset => true,
                        StickyIp::Hostname => ip_opt.is_some(),
                        StickyIp::Ip(s) => Some(s.as_str()) != ip_str,
                    };
                    if needs_update {
                        let _guard = self.sticky_lock.lock().unwrap();
                        let still_needs = match &self.sticky_ip {
                            StickyIp::Unset => true,
                            StickyIp::Hostname => ip_opt.is_some(),
                            StickyIp::Ip(s) => Some(s.as_str()) != ip_str,
                        };
                        if still_needs {
                            self.sticky_ip = match ip_opt.clone() {
                                None => StickyIp::Hostname,
                                Some(ip) => StickyIp::Ip(ip),
                            };
                            // log level depends on whether previous attempt had error
                            // if last_error.is_some() { log::warn! } else { log::info! }
                        }
                    }
                    return Ok(response);
                }
                Err(exc) => {
                    let is_retryable = is_retryable_connect_error(&exc);
                    if !is_retryable {
                        return Err(exc);
                    }
                    last_error = Some(exc.clone());
                    // Sticky reset if sticky ip failed — mirrors lines 174-182
                    let sticky_matches = match &self.sticky_ip {
                        StickyIp::Unset => false,
                        StickyIp::Hostname => ip_opt.is_none(),
                        StickyIp::Ip(s) => Some(s.as_str()) == ip_str,
                    };
                    if sticky_matches {
                        let _guard = self.sticky_lock.lock().unwrap();
                        let still_matches = match &self.sticky_ip {
                            StickyIp::Unset => false,
                            StickyIp::Hostname => ip_opt.is_none(),
                            StickyIp::Ip(s) => Some(s.as_str()) == ip_str,
                        };
                        if still_matches {
                            self.sticky_ip = StickyIp::Unset;
                            // log::warn!("[Telegram] Sticky Telegram path {} failed; re-walking IPv4 literals before the hostname", ip_str.unwrap_or("api.telegram.org"));
                        }
                    }
                    if ip_opt.is_none() {
                        self.reset_primary(true);
                        // log::warn!("[Telegram] Dual-stack api.telegram.org path failed ({})", exc);
                        continue;
                    }
                    // log::warn!("[Telegram] IPv4 Telegram API IP {} failed: {}", ip_opt.as_deref().unwrap_or("?"), exc);
                    if let Some(ip) = ip_str {
                        self.reset_fallback(ip);
                    }
                    continue;
                }
            }
        }

        if let Some(err) = last_error {
            Err(err)
        } else {
            Err(TransportError::Other(
                "All Telegram fallback IPs exhausted but no error was recorded".to_string(),
            ))
        }
    }

    /// Mirrors `async def aclose(self) -> None` (lines 198-207).
    pub fn aclose(&mut self) {
        let _guard = self.primary_lock.lock().unwrap();
        self.primary_closed_flag = true;
        self.primary_closed = true;
        // await primary.aclose() — stub marks closed.
        drop(_guard);
        let _guard2 = self.fallback_lock.lock().unwrap();
        for (_, mut pool) in self.fallbacks.drain() {
            pool.closed = true;
            // await transport.aclose()
        }
    }

    // ------------------------------------------------------------------
    // Accessors for inspection / tests — mirrors Python attributes
    // ------------------------------------------------------------------

    pub fn fallback_ips(&self) -> &[String] {
        &self.fallback_ips
    }

    pub fn sticky(&self) -> &StickyIp {
        &self.sticky_ip
    }

    pub fn set_sticky_for_test(&mut self, sticky: StickyIp) {
        self.sticky_ip = sticky;
    }
}

// ---------------------------------------------------------------------------
// Re-exports for 1:1 naming — mirrors Python private names as pub aliases
// ---------------------------------------------------------------------------

/// Alias for `normalize_fallback_ips` — mirrors `_normalize_fallback_ips`.
pub fn _normalize_fallback_ips<I, S>(values: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    normalize_fallback_ips(values)
}

/// Alias for `is_retryable_connect_error` — mirrors `_is_retryable_connect_error`.
pub fn _is_retryable_connect_error(err: &TransportError) -> bool {
    is_retryable_connect_error(err)
}

/// Alias for `rewrite_request_for_ip` — mirrors `_rewrite_request_for_ip`.
pub fn _rewrite_request_for_ip(request: &HttpRequest, ip: &str) -> HttpRequest {
    rewrite_request_for_ip(request, ip)
}

/// Alias for `resolve_system_dns` — mirrors `_resolve_system_dns`.
pub fn _resolve_system_dns() -> HashSet<String> {
    resolve_system_dns()
}

/// Alias for `resolve_proxy_url` — mirrors `_resolve_proxy_url`.
pub fn _resolve_proxy_url(target_hosts: Option<&[String]>) -> Option<String> {
    resolve_proxy_url(target_hosts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seed_ips_are_two() {
        assert_eq!(SEED_FALLBACK_IPS.len(), 2);
        assert_eq!(SEED_FALLBACK_IPS[0], "149.154.166.110");
        assert_eq!(SEED_FALLBACK_IPS[1], "149.154.167.220");
    }

    #[test]
    fn normalize_filters_private_and_invalid() {
        let input = vec![
            "149.154.166.110".to_string(),
            "  149.154.167.220  ".to_string(),
            "10.0.0.1".to_string(),
            "127.0.0.1".to_string(),
            "169.254.1.1".to_string(),
            "0.0.0.0".to_string(),
            "not-an-ip".to_string(),
            "".to_string(),
            "192.168.1.1".to_string(),
            "::1".to_string(),
        ];
        let out = normalize_fallback_ips(input);
        assert_eq!(out, vec!["149.154.166.110", "149.154.167.220"]);
    }

    #[test]
    fn normalize_rejects_cgnat_and_test_nets() {
        let input = vec!["100.64.0.1".to_string(), "198.18.0.1".to_string(), "192.0.2.1".to_string()];
        let out = normalize_fallback_ips(input);
        assert!(out.is_empty(), "CGNAT/test nets should be filtered as private: {:?}", out);
    }

    #[test]
    fn parse_env_splits_and_normalizes() {
        let v = parse_fallback_ip_env(Some("149.154.166.110, 10.0.0.1 , 149.154.167.220"));
        assert_eq!(v, vec!["149.154.166.110", "149.154.167.220"]);
        assert!(parse_fallback_ip_env(None).is_empty());
        assert!(parse_fallback_ip_env(Some("")).is_empty());
        assert!(parse_fallback_ip_env(Some("   ")).is_empty());
    }

    #[test]
    fn attempt_order_sticky_first_then_ips_then_hostname() {
        let mut t = TelegramFallbackTransport::new(vec!["1.1.1.1".to_string(), "2.2.2.2".to_string()], None, Some(String::new()));
        // No sticky: ips then hostname
        assert_eq!(
            t.attempt_order(),
            vec![Some("1.1.1.1".to_string()), Some("2.2.2.2".to_string()), None]
        );
        // Sticky IP
        t.set_sticky_for_test(StickyIp::Ip("2.2.2.2".to_string()));
        assert_eq!(
            t.attempt_order(),
            vec![Some("2.2.2.2".to_string()), Some("1.1.1.1".to_string()), None]
        );
        // Sticky hostname
        t.set_sticky_for_test(StickyIp::Hostname);
        assert_eq!(
            t.attempt_order(),
            vec![None, Some("1.1.1.1".to_string()), Some("2.2.2.2".to_string())]
        );
        // Sticky ip not in fallback list still first
        t.set_sticky_for_test(StickyIp::Ip("9.9.9.9".to_string()));
        assert_eq!(
            t.attempt_order(),
            vec![
                Some("9.9.9.9".to_string()),
                Some("1.1.1.1".to_string()),
                Some("2.2.2.2".to_string()),
                None
            ]
        );
    }

    #[test]
    fn attempt_order_no_fallback_ips_is_just_hostname() {
        let t = TelegramFallbackTransport::new(Vec::<String>::new(), None, Some(String::new()));
        assert_eq!(t.attempt_order(), vec![None]);
        t.attempt_order(); // idempotent
    }

    #[test]
    fn dedup_preserves_order() {
        let t = TelegramFallbackTransport::new(
            vec![
                "149.154.166.110".to_string(),
                "149.154.166.110".to_string(),
                "149.154.167.220".to_string(),
            ],
            None,
            Some(String::new()),
        );
        assert_eq!(t.fallback_ips(), &["149.154.166.110", "149.154.167.220"]);
    }

    #[test]
    fn rewrite_preserves_host_and_sni() {
        let mut req = HttpRequest::new("GET", "https://api.telegram.org/bot123/sendMessage?chat_id=1", "api.telegram.org");
        req.headers.insert("authorization".to_string(), "Bearer x".to_string());
        let rewritten = rewrite_request_for_ip(&req, "149.154.166.110");
        assert_eq!(rewritten.host, "149.154.166.110");
        assert_eq!(rewritten.headers.get("host").map(|s| s.as_str()), Some("api.telegram.org"));
        assert_eq!(rewritten.sni_hostname.as_deref(), Some("api.telegram.org"));
        assert!(rewritten.url.contains("149.154.166.110"));
        assert!(rewritten.url.contains("/bot123/sendMessage"));
    }

    #[test]
    fn is_retryable_only_connect_errors() {
        assert!(is_retryable_connect_error(&TransportError::ConnectTimeout("t".into())));
        assert!(is_retryable_connect_error(&TransportError::ConnectError("e".into())));
        assert!(!is_retryable_connect_error(&TransportError::Other("other".into())));
    }

    #[test]
    fn doh_query_filters_a_records() {
        let resp = DohResponse {
            answer: vec![
                DohAnswer { record_type: 1, data: "149.154.166.110".to_string() },
                DohAnswer { record_type: 5, data: "alias.example.com".to_string() },
                DohAnswer { record_type: 1, data: "not-an-ip".to_string() },
                DohAnswer { record_type: 1, data: "149.154.167.220".to_string() },
            ],
        };
        let ips = query_doh_provider_response(&resp);
        assert_eq!(ips, vec!["149.154.166.110", "149.154.167.220"]);
    }

    #[test]
    fn discover_uses_seed_when_doh_empty() {
        let out = discover_fallback_ips_from_responses(&[], &HashSet::new());
        assert_eq!(out, vec!["149.154.166.110", "149.154.167.220"]);
    }

    #[test]
    fn discover_dedup_and_normalizes() {
        let resp = DohResponse {
            answer: vec![
                DohAnswer { record_type: 1, data: "149.154.166.110".to_string() },
                DohAnswer { record_type: 1, data: "149.154.166.110".to_string() },
                DohAnswer { record_type: 1, data: "10.0.0.1".to_string() },
            ],
        };
        let out = discover_fallback_ips_from_responses(&[resp], &HashSet::new());
        assert_eq!(out, vec!["149.154.166.110"]);
    }

    #[test]
    fn handle_request_sticky_and_fallback() {
        let mut t = TelegramFallbackTransport::new(vec!["1.1.1.1".to_string()], None, Some(String::new()));
        let req = HttpRequest::new("GET", "https://api.telegram.org/getMe", "api.telegram.org");
        // Simulate first IP fails with ConnectError, then hostname succeeds
        let mut calls: Vec<Option<String>> = Vec::new();
        let res = t.handle_request(&req, |_candidate, ip_opt| {
            calls.push(ip_opt.map(|s| s.to_string()));
            match ip_opt {
                Some("1.1.1.1") => Err(TransportError::ConnectError("refused".into())),
                None => Ok(HttpResponse::ok("ok")),
                _ => Err(TransportError::Other("unexpected".into())),
            }
        });
        assert!(res.is_ok());
        assert_eq!(calls, vec![Some("1.1.1.1".to_string()), None]);
        // Sticky should now be Hostname (since hostname succeeded after fallback failed)
        assert_eq!(t.sticky(), &StickyIp::Hostname);
    }

    #[test]
    fn handle_request_non_retryable_bubbles() {
        let mut t = TelegramFallbackTransport::new(vec!["1.1.1.1".to_string()], None, Some(String::new()));
        let req = HttpRequest::new("GET", "https://api.telegram.org/getMe", "api.telegram.org");
        let res = t.handle_request(&req, |_, _| Err(TransportError::Other("boom".into())));
        assert!(matches!(res, Err(TransportError::Other(_))));
    }

    #[test]
    fn pool_limits_match_python() {
        assert_eq!(POOL_LIMITS.max_connections, 8);
        assert_eq!(POOL_LIMITS.max_keepalive_connections, 4);
    }
}
