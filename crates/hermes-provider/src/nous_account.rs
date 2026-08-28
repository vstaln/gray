//! Normalized Nous Portal account entitlement helpers.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/hermes_cli/nous_account.py` (814 lines).
//!
//! Entitlement model: `NousPortalAccountInfo` normalizes three sources —
//! live JWT claims (`jwt`), fresh `/api/oauth/account` payload (`account_api`),
//! opaque inference key fallback (`inference_key`), plus `none`/`error` sentinels.
//! Free tool-pool coverage (`tool_access`) mirrors the Portal's TOOL_COVERAGE
//! categories; `paid_service_access` drives the paid gate.
//!
//! T0034 — 1:1 port, no cargo (NEVER cargo).
//!
//! Notes on fidelity vs. Rust idioms:
//! - Python `Literal["jwt", ...]` ↔ `NousAccountInfoSource` enum with `as_str()`.
//! - Python `tuple[str, ...]` TOOL_COVERAGE_CATEGORIES ↔ `&[&str]` const slice.
//! - Python `threading.Lock` + global tuple cache ↔ `OnceLock<Mutex<Option<...>>>` + `Instant`.
//! - Python `dataclass(frozen=True)` ↔ `#[derive(Debug, Clone)]` structs (immutable by convention).
//! - Python `Optional[datetime]` ↔ `Option<SystemTime>`; `timezone.utc` anchor is `UNIX_EPOCH`.
//! - Python `urllib.request` + `json.loads` ↔ `curl`-backed stub (std-only) + hand-rolled JSON helpers.
//! - Python `hashlib.sha256` ↔ shell `sha256sum` probe + FNV fallback (preserves never-log-token property).
//! - Python `urllib.parse.quote` ↔ minimal percent-encode (unreserved set `A-Za-z0-9-._~`).
//! - Python `Any` payloads ↔ `Value` enum (std-only `serde_json::Value` stand-in).
//! - Python `get_provider_auth_state` / `resolve_nous_access_token` / `_decode_jwt_claims`
//!   / `load_pool("nous")` ↔ stubs reading env/pool files; canonical impls live in `hermes-cli` crate.
//! - `hermes_cli.auth.DEFAULT_NOUS_PORTAL_URL` fallback ↔ `DEFAULT_NOUS_PORTAL_URL` const.

#![allow(dead_code, unused_imports, unused_variables, clippy::all)]

use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// Literal + constants — mirrors ll.15-33
// ---------------------------------------------------------------------------

/// Mirrors `NousAccountInfoSource = Literal["jwt", "account_api", "inference_key", "none", "error"]` (l.15).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NousAccountInfoSource {
    Jwt,
    AccountApi,
    InferenceKey,
    None,
    Error,
}

impl NousAccountInfoSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            NousAccountInfoSource::Jwt => "jwt",
            NousAccountInfoSource::AccountApi => "account_api",
            NousAccountInfoSource::InferenceKey => "inference_key",
            NousAccountInfoSource::None => "none",
            NousAccountInfoSource::Error => "error",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "jwt" => Some(Self::Jwt),
            "account_api" => Some(Self::AccountApi),
            "inference_key" => Some(Self::InferenceKey),
            "none" => Some(Self::None),
            "error" => Some(Self::Error),
            _ => None,
        }
    }
}

impl std::fmt::Display for NousAccountInfoSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Mirrors `TOOL_COVERAGE_CATEGORIES` (ll.22-29). Kept byte-for-byte aligned with Portal.
pub const TOOL_COVERAGE_CATEGORIES: &[&str] = &[
    "firecrawl",
    "fal",
    "fal-video",
    "openai-audio",
    "browser-use",
    "modal",
];

/// Mirrors `_ACCOUNT_INFO_CACHE_TTL = 60` (l.31) — seconds.
pub const ACCOUNT_INFO_CACHE_TTL_SECS: f64 = 60.0;
pub const ACCOUNT_INFO_CACHE_TTL: Duration = Duration::from_secs(60);

const DEFAULT_NOUS_PORTAL_URL: &str = "https://portal.nousresearch.com";

// Global short-lived cache — mirrors `_account_info_cache` + `_ACCOUNT_INFO_CACHE_LOCK` (ll.32-33).
static ACCOUNT_INFO_CACHE: OnceLock<Mutex<Option<(String, Instant, NousPortalAccountInfo)>>> =
    OnceLock::new();

fn account_info_cache() -> &'static Mutex<Option<(String, Instant, NousPortalAccountInfo)>> {
    ACCOUNT_INFO_CACHE.get_or_init(|| Mutex::new(None))
}

fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

fn now_instant() -> Instant {
    Instant::now()
}

// ---------------------------------------------------------------------------
// Minimal Value — mirrors `Any` dict payloads for 1:1 coercion (std-only)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Number(f64),
    Int(i64),
    String(String),
    Array(Vec<Value>),
    Object(HashMap<String, Value>),
}

impl Value {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s.as_str()),
            _ => None,
        }
    }
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Number(n) => Some(*n),
            Value::Int(i) => Some(*i as f64),
            _ => None,
        }
    }
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Int(i) => Some(*i),
            Value::Number(n) => Some(*n as i64),
            _ => None,
        }
    }
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }
}

// ---------------------------------------------------------------------------
// Data classes — mirrors ll.36-131
// ---------------------------------------------------------------------------

/// Mirrors `NousPortalSubscriptionInfo` (ll.36-44).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NousPortalSubscriptionInfo {
    pub plan: Option<String>,
    pub tier: Option<i64>,
    pub monthly_charge: Option<f64>,
    pub monthly_credits: Option<f64>,
    pub current_period_end: Option<String>,
    pub credits_remaining: Option<f64>,
    pub rollover_credits: Option<f64>,
}

/// Mirrors `NousPaidServiceAccessInfo` (ll.47-64).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NousPaidServiceAccessInfo {
    pub allowed: Option<bool>,
    pub paid_access: Option<bool>,
    pub reason: Option<String>,
    pub organisation_id: Option<String>,
    pub effective_at_ms: Option<i64>,
    pub has_active_subscription: Option<bool>,
    pub active_subscription_is_paid: Option<bool>,
    pub subscription_tier: Option<i64>,
    pub subscription_monthly_charge: Option<f64>,
    pub subscription_credits_remaining: Option<f64>,
    pub purchased_credits_remaining: Option<f64>,
    pub total_usable_credits: Option<f64>,
    pub member_spend_cap_exceeded: Option<bool>,
    pub member_spend_cap_usd: Option<f64>,
    pub member_spend_usd: Option<f64>,
    pub member_spend_cap_remaining_usd: Option<f64>,
}

/// Mirrors `NousToolAccessInfo` (ll.67-77).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NousToolAccessInfo {
    pub enabled: bool,
    pub coverage: HashMap<String, bool>,
}

impl NousToolAccessInfo {
    pub fn new(enabled: bool, coverage: HashMap<String, bool>) -> Self {
        Self { enabled, coverage }
    }
}

/// Mirrors `NousPortalAccountInfo` (ll.81-131).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NousPortalAccountInfo {
    pub logged_in: bool,
    pub source: NousAccountInfoSource,
    pub fresh: bool,
    pub user_id: Option<String>,
    pub org_id: Option<String>,
    pub org_slug: Option<String>,
    pub org_name: Option<String>,
    pub client_id: Option<String>,
    pub product_id: Option<String>,
    pub nous_client: Option<String>,
    pub portal_base_url: Option<String>,
    pub inference_base_url: Option<String>,
    pub inference_credential_present: bool,
    pub credential_source: Option<String>,
    pub expires_at: Option<SystemTime>,
    pub email: Option<String>,
    pub privy_did: Option<String>,
    pub subscription: Option<NousPortalSubscriptionInfo>,
    pub paid_service_access: Option<bool>,
    pub paid_service_access_info: Option<NousPaidServiceAccessInfo>,
    pub tool_access: Option<NousToolAccessInfo>,
    pub raw_claims: Option<HashMap<String, Value>>,
    pub raw_account: Option<HashMap<String, Value>>,
    pub error: Option<String>,
}

impl NousPortalAccountInfo {
    /// Mirrors `is_paid` property (ll.108-109): `paid_service_access is True`.
    pub fn is_paid(&self) -> bool {
        self.paid_service_access == Some(true)
    }

    /// Mirrors `is_free_tier` property (ll.112-113): `paid_service_access is False`.
    pub fn is_free_tier(&self) -> bool {
        self.paid_service_access == Some(false)
    }

    /// Mirrors `tool_gateway_entitled` property (ll.117-122).
    pub fn tool_gateway_entitled(&self) -> bool {
        if self.paid_service_access == Some(true) {
            return true;
        }
        match &self.tool_access {
            Some(ta) => ta.enabled,
            None => false,
        }
    }

    /// Mirrors `tool_gateway_entitled_for` (ll.124-131).
    pub fn tool_gateway_entitled_for(&self, category: &str) -> bool {
        if self.paid_service_access == Some(true) {
            return true;
        }
        match &self.tool_access {
            Some(ta) if ta.enabled => ta.coverage.get(category) == Some(&true),
            _ => false,
        }
    }
}

impl Default for NousAccountInfoSource {
    fn default() -> Self {
        NousAccountInfoSource::None
    }
}

// ---------------------------------------------------------------------------
// Billing / topup URL helpers — mirrors ll.134-169
// ---------------------------------------------------------------------------

/// Mirrors `nous_portal_billing_url` (ll.134-146).
pub fn nous_portal_billing_url(account_info: Option<&NousPortalAccountInfo>) -> String {
    let base: Option<String> = account_info.and_then(|a| a.portal_base_url.clone());
    let base = match base {
        Some(s) if !s.trim().is_empty() => s.trim().to_string(),
        _ => DEFAULT_NOUS_PORTAL_URL.to_string(),
    };
    format!("{}/billing", base.trim_end_matches('/'))
}

/// Mirrors `nous_portal_topup_url` (ll.149-169).
pub fn nous_portal_topup_url(account_info: Option<&NousPortalAccountInfo>) -> String {
    let base_billing = nous_portal_billing_url(account_info); // {base}/billing
    let base = if base_billing.ends_with("/billing") {
        base_billing[..base_billing.len() - "/billing".len()].to_string()
    } else {
        base_billing
    };
    let slug = account_info.and_then(|a| a.org_slug.as_deref());
    if let Some(s) = slug {
        let trimmed = s.trim();
        if !trimmed.is_empty() {
            return format!(
                "{}/orgs/{}/billing?topup=open",
                base.trim_end_matches('/'),
                percent_encode(trimmed)
            );
        }
    }
    format!("{}/billing?topup=open", base.trim_end_matches('/'))
}

fn percent_encode(input: &str) -> String {
    // Mirrors `urllib.parse.quote(slug.strip(), safe='')` (l.168) — encode all except unreserved.
    let mut out = String::new();
    for b in input.bytes() {
        let c = b as char;
        if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' || c == '~' {
            out.push(c);
        } else {
            out.push_str(&format!("%{:02X}", b));
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Entitlement messaging — mirrors ll.172-334
// ---------------------------------------------------------------------------

/// Mirrors `format_nous_portal_entitlement_message` (ll.172-260).
pub fn format_nous_portal_entitlement_message(
    account_info: Option<&NousPortalAccountInfo>,
    capability: &str,
    include_refresh_hint: bool,
    coverage_category: Option<&str>,
) -> Option<String> {
    let capability = if capability.trim().is_empty() {
        "this feature"
    } else {
        capability
    };
    let billing_url = nous_portal_billing_url(account_info);

    if let Some(info) = account_info {
        if let Some(cat) = coverage_category {
            if info.tool_gateway_entitled_for(cat) {
                return None;
            }
            if info.tool_gateway_entitled() {
                return Some(format!(
                    "{} isn't included with your current Nous Portal access. Add credits or a subscription to enable it at {}.",
                    capability, billing_url
                ));
            }
        } else if info.tool_gateway_entitled() {
            return None;
        }
    }

    if account_info.is_none() {
        return Some(format!(
            "Hermes could not verify your Nous Portal entitlement, so {} is unavailable. Run `hermes model` to refresh your login, or check billing at {}.",
            capability, billing_url
        ));
    }

    // At this point account_info is Some.
    let info = account_info.unwrap();

    if !info.logged_in {
        if info.inference_credential_present {
            return Some(format!(
                "Nous inference credentials are configured, but Hermes cannot verify your Nous Portal paid access for {}. Log in with `hermes model` to enable Portal-managed features. Billing and credits are managed at {}.",
                capability, billing_url
            ));
        }
        return Some(format!(
            "Log in to Nous Portal to use {}: run `hermes model`. Billing and credits are managed at {}.",
            capability, billing_url
        ));
    }

    if info.paid_service_access.is_none() {
        let mut detail = format!(
            "Hermes could not verify your Nous Portal paid access, so {} is unavailable.",
            capability
        );
        if let Some(err) = &info.error {
            if !err.trim().is_empty() {
                detail.push_str(&format!(" Account lookup failed: {}.", err));
            }
        }
        if include_refresh_hint {
            detail.push_str(" Run `hermes model` to refresh your session.");
        }
        detail.push_str(&format!(" Check billing at {}.", billing_url));
        return Some(detail);
    }

    let access = info.paid_service_access_info.as_ref();
    let reason = access.and_then(|a| a.reason.as_deref());

    if reason == Some("account_missing") {
        return Some(format!(
            "Hermes could not find a Nous Portal account or organisation for this login, so {} is unavailable. Run `hermes model` to authenticate again; if the problem persists, contact Nous support.",
            capability
        ));
    }

    if reason == Some("no_usable_credits") || info.paid_service_access == Some(false) {
        let mut message = no_paid_access_message(info, capability, &billing_url);
        if include_refresh_hint && !info.fresh {
            message.push_str(" If you recently bought credits, run `hermes model` to refresh Hermes.");
        }
        return Some(message);
    }

    Some(format!(
        "Your Nous Portal account does not currently have paid service access, so {} is unavailable. Add credits or update billing at {}.",
        capability, billing_url
    ))
}

/// Mirrors `_no_paid_access_message` (ll.263-317).
pub fn no_paid_access_message(
    account_info: &NousPortalAccountInfo,
    capability: &str,
    billing_url: &str,
) -> String {
    let access = account_info.paid_service_access_info.as_ref();
    let has_active_subscription = access.and_then(|a| a.has_active_subscription);
    let active_subscription_is_paid = access.and_then(|a| a.active_subscription_is_paid);
    let total_usable = access.and_then(|a| a.total_usable_credits);
    let subscription_credits = access.and_then(|a| a.subscription_credits_remaining);
    let purchased_credits = access.and_then(|a| a.purchased_credits_remaining);

    if let Some(a) = access {
        if a.member_spend_cap_exceeded == Some(true) {
            let cap = a.member_spend_cap_usd;
            let spent = a.member_spend_usd;
            let credit_detail = credit_detail(total_usable, subscription_credits, purchased_credits);
            let cap_detail = match (cap, spent) {
                (Some(c), Some(s)) if c.is_finite() && s.is_finite() => {
                    format!(
                        " Your organisation's per-member spend cap is ${:.2} and you've spent ${:.2} of it.",
                        c, s
                    )
                }
                (Some(c), _) if c.is_finite() => {
                    format!(" Your organisation's per-member spend cap is ${:.2}.", c)
                }
                _ => String::new(),
            };
            return format!(
                "Your Nous Portal access is paused because you've exceeded the per-member spend cap set by your organisation.{}{} Ask your organisation admin to raise the member spend cap at {}, then run `hermes model` to refresh.",
                cap_detail, credit_detail, billing_url
            );
        }
    }

    if has_active_subscription == Some(true) && active_subscription_is_paid == Some(true) {
        let credit_detail = credit_detail(total_usable, subscription_credits, purchased_credits);
        return format!(
            "Your Nous Portal credits are exhausted{}, so {} is unavailable. Top up or renew credits at {}.",
            credit_detail, capability, billing_url
        );
    }

    if has_active_subscription == Some(true) && active_subscription_is_paid == Some(false) {
        return format!(
            "Your current Nous Portal plan does not include paid service access, so {} is unavailable. Upgrade or add credits at {}.",
            capability, billing_url
        );
    }

    if has_active_subscription == Some(false) {
        let credit_detail = credit_detail(total_usable, subscription_credits, purchased_credits);
        return format!(
            "Your Nous Portal account has no active subscription or usable credits{}, so {} is unavailable. Subscribe or add credits at {}.",
            credit_detail, capability, billing_url
        );
    }

    let credit_detail = credit_detail(total_usable, subscription_credits, purchased_credits);
    format!(
        "Your Nous Portal account has no usable paid credits{}, so {} is unavailable. Add credits or update billing at {}.",
        credit_detail, capability, billing_url
    )
}

#[allow(dead_code)]
fn _no_paid_access_message(
    account_info: &NousPortalAccountInfo,
    capability: &str,
    billing_url: &str,
) -> String {
    no_paid_access_message(account_info, capability, billing_url)
}

/// Mirrors `_credit_detail` (ll.320-334).
pub fn credit_detail(
    total_usable: Option<f64>,
    subscription_credits: Option<f64>,
    purchased_credits: Option<f64>,
) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(v) = total_usable {
        if v.is_finite() {
            parts.push(format!("usable ${:.2}", v));
        }
    }
    if let Some(v) = subscription_credits {
        if v.is_finite() {
            parts.push(format!("subscription ${:.2}", v));
        }
    }
    if let Some(v) = purchased_credits {
        if v.is_finite() {
            parts.push(format!("purchased ${:.2}", v));
        }
    }
    if parts.is_empty() {
        return String::new();
    }
    format!(" ({})", parts.join(", "))
}

#[allow(dead_code)]
fn _credit_detail(
    total_usable: Option<f64>,
    subscription_credits: Option<f64>,
    purchased_credits: Option<f64>,
) -> String {
    credit_detail(total_usable, subscription_credits, purchased_credits)
}

// ---------------------------------------------------------------------------
// Cache reset — mirrors ll.337-340
// ---------------------------------------------------------------------------

/// Mirrors `reset_nous_portal_account_info_cache` (ll.337-340).
pub fn reset_nous_portal_account_info_cache() {
    if let Ok(mut guard) = account_info_cache().lock() {
        *guard = None;
    }
}

// ---------------------------------------------------------------------------
// Auth state helpers — mirrors `hermes_cli.auth` imports + state handling
// ---------------------------------------------------------------------------

/// Minimal auth state mirror — `get_provider_auth_state("nous") or {}` (l.358).
/// Real impl lives in `hermes_cli.auth`; this stub reads env / file so the slice
/// compiles std-only and preserves line-level audit.
pub type AuthState = HashMap<String, Value>;

fn value_string(v: &Value) -> Option<String> {
    match v {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        _ => None,
    }
}

fn auth_state_get_str(state: &AuthState, key: &str) -> Option<String> {
    state.get(key).and_then(|v| match v {
        Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
        _ => None,
    })
}

/// Stub: mirrors `hermes_cli.auth.get_provider_auth_state("nous")` (ll.356-358).
/// Returns `Some(state)` when `NOUS_ACCESS_TOKEN` or file-backed auth exists; else `None`.
/// For 1:1 audit we probe env vars that the real store would populate.
pub fn get_provider_auth_state(provider: &str) -> Option<AuthState> {
    if provider != "nous" {
        return None;
    }
    // Probe env for a minimal state so tests can inject via env without touching hermes home.
    // Real impl reads `~/.hermes/auth.json` / `hermes_constants.get_hermes_home()`.
    let mut state: AuthState = HashMap::new();
    let mut has_any = false;
    for (env_key, state_key) in [
        ("NOUS_ACCESS_TOKEN", "access_token"),
        ("NOUS_PORTAL_BASE_URL", "portal_base_url"),
        ("NOUS_CLIENT_ID", "client_id"),
        ("NOUS_INFERENCE_BASE_URL", "inference_base_url"),
        ("NOUS_AGENT_KEY", "agent_key"),
        ("NOUS_CREDENTIAL_SOURCE", "credential_source"),
    ] {
        if let Ok(v) = env::var(env_key) {
            let t = v.trim().to_string();
            if !t.is_empty() {
                state.insert(state_key.to_string(), Value::String(t));
                has_any = true;
            }
        }
    }
    // Also try reading HERMES_HOME/auth.json if present (best-effort, no error)
    if !has_any {
        if let Ok(home) = env::var("HERMES_HOME") {
            let p = Path::new(&home).join("auth.json");
            if let Ok(text) = fs::read_to_string(&p) {
                // Very minimal extraction — look for "access_token" field
                if let Some(tok) = extract_json_string(&text, "access_token") {
                    if !tok.trim().is_empty() {
                        state.insert("access_token".to_string(), Value::String(tok));
                        has_any = true;
                    }
                }
            }
        }
    }
    if has_any { Some(state) } else { None }
}

/// Stub: mirrors `hermes_cli.auth.resolve_nous_access_token()` (l.408).
/// Returns the current access token string or empty.
pub fn resolve_nous_access_token() -> String {
    if let Some(state) = get_provider_auth_state("nous") {
        if let Some(tok) = auth_state_get_str(&state, "access_token") {
            return tok;
        }
    }
    env::var("NOUS_ACCESS_TOKEN").unwrap_or_default().trim().to_string()
}

// ---------------------------------------------------------------------------
// Main entry — mirrors ll.343-396
// ---------------------------------------------------------------------------

/// Mirrors `get_nous_portal_account_info(*, force_fresh, min_jwt_ttl_seconds)` (ll.343-396).
pub fn get_nous_portal_account_info(force_fresh: bool, min_jwt_ttl_seconds: i64) -> NousPortalAccountInfo {
    let state = match get_provider_auth_state("nous") {
        Some(s) => s,
        None => {
            // Mirrors `except Exception as exc: return _error_info(error=exc, logged_in=False)` (ll.359-360)
            return error_info("no_auth_state", false, None, None);
        }
    };
    // For the pure std-only slice we handle the Result-like path via Option.
    // Real `get_provider_auth_state` may raise; we map None to error above.

    let access_token = auth_state_get_str(&state, "access_token").unwrap_or_default();
    let portal_base_url = portal_base_url_from_state(&state);

    if access_token.trim().is_empty() {
        if let Some(info) = info_from_oauth_pool(force_fresh, min_jwt_ttl_seconds, portal_base_url.clone()) {
            return info;
        }
        if let Some(info) = info_from_inference_key_pool(portal_base_url.clone()) {
            return info;
        }
        return NousPortalAccountInfo {
            logged_in: false,
            source: NousAccountInfoSource::None,
            fresh: false,
            portal_base_url,
            ..Default::default()
        };
    }

    if !force_fresh {
        if let Some(jwt_info) = info_from_valid_jwt(
            &access_token,
            &state,
            portal_base_url.clone(),
            min_jwt_ttl_seconds,
        ) {
            return jwt_info;
        }
    }

    fresh_account_info(&state, force_fresh, portal_base_url)
}

// ---------------------------------------------------------------------------
// Fresh fetch with cache — mirrors ll.399-449
// ---------------------------------------------------------------------------

/// Mirrors `_fresh_account_info` (ll.399-449).
pub fn fresh_account_info(
    state: &AuthState,
    force_fresh: bool,
    portal_base_url: Option<String>,
) -> NousPortalAccountInfo {
    // In the Python version this re-resolves token + state (ll.408-413) and checks cache (ll.415-419)
    // before fetching. We replicate the shape.

    let access_token = resolve_nous_access_token();
    // Re-read state to pick up refreshed values (best-effort); fall back to passed state.
    let refreshed_state = get_provider_auth_state("nous").unwrap_or_else(|| state.clone());
    let portal_base_url = portal_base_url_from_state(&refreshed_state).or(portal_base_url);
    let cache_key = cache_key_str(&access_token, portal_base_url.as_deref());

    if !force_fresh {
        if let Ok(guard) = account_info_cache().lock() {
            if let Some((cached_key, cached_at, cached_info)) = guard.as_ref() {
                if cached_key == &cache_key && cached_at.elapsed() < ACCOUNT_INFO_CACHE_TTL {
                    return cached_info.clone();
                }
            }
        }
    }

    let payload = match fetch_nous_account_info(&access_token, portal_base_url.as_deref()) {
        Ok(p) => p,
        Err(exc) => {
            return error_info(exc, !access_token.trim().is_empty() && !resolve_nous_access_token().is_empty() || !auth_state_get_str(state, "access_token").unwrap_or_default().is_empty(), portal_base_url.clone(), None);
        }
    };

    if payload.is_empty() {
        return error_info(
            "empty_account_response",
            true,
            portal_base_url,
            None,
        );
    }
    if let Some(Value::String(err)) = payload.get("error") {
        if !err.trim().is_empty() {
            return error_info(
                err.clone(),
                true,
                portal_base_url,
                Some(payload.clone()),
            );
        }
    }
    // Also handle plain string error under "error" key parsed as string
    if let Some(err) = payload.get("error").and_then(|v| match v {
        Value::String(s) if !s.trim().is_empty() => Some(s.clone()),
        _ => None,
    }) {
        return error_info(err, true, portal_base_url, Some(payload));
    }

    let info = info_from_account_payload(&payload, &refreshed_state, portal_base_url.clone());
    if let Ok(mut guard) = account_info_cache().lock() {
        *guard = Some((cache_key, now_instant(), info.clone()));
    }
    info
}

#[allow(dead_code)]
fn _fresh_account_info(
    state: &AuthState,
    force_fresh: bool,
    portal_base_url: Option<String>,
) -> NousPortalAccountInfo {
    fresh_account_info(state, force_fresh, portal_base_url)
}

// ---------------------------------------------------------------------------
// Pool fallbacks — mirrors ll.452-553
// ---------------------------------------------------------------------------

/// Minimal pool entry mirror for `agent.credential_pool.load_pool("nous")` (ll.556-572).
#[derive(Debug, Clone)]
pub struct PoolEntry {
    pub access_token: Option<String>,
    pub runtime_api_key: Option<String>,
    pub portal_base_url: Option<String>,
    pub inference_base_url: Option<String>,
    pub runtime_base_url: Option<String>,
    pub base_url: Option<String>,
    pub label: Option<String>,
    pub priority: i64,
    pub agent_key_expires_at: Option<String>,
    pub expires_at: Option<String>,
    pub auth_type: Option<String>,
    pub refresh_token: Option<String>,
    pub client_id: Option<String>,
    pub agent_key: Option<String>,
}

/// Stub: mirrors `agent.credential_pool.load_pool("nous")` (ll.558).
/// Real impl loads `~/.hermes/credentials/` pool files; this stub scans env for
/// `HERMES_NOUS_POOL_*` hints and otherwise returns empty so callers fail-open to `None`.
fn load_nous_pool() -> Vec<PoolEntry> {
    // Best-effort env-driven pool for tests: HERMES_NOUS_POOL_JSON contains a JSON array.
    if let Ok(json) = env::var("HERMES_NOUS_POOL_JSON") {
        if let Some(entries) = parse_pool_json(&json) {
            return entries;
        }
    }
    Vec::new()
}

fn parse_pool_json(_json: &str) -> Option<Vec<PoolEntry>> {
    // Minimal stub — real crate uses `serde_json`. For 1:1 audit we keep shape but stay std-only.
    // Returning None keeps the caller in the `None` → `info_from_inference_key_pool` returns None path.
    None
}

/// Mirrors `_info_from_inference_key_pool` (ll.452-482).
pub fn info_from_inference_key_pool(portal_base_url: Option<String>) -> Option<NousPortalAccountInfo> {
    let entry = select_nous_pool_entry()?;
    let runtime_key = entry
        .runtime_api_key
        .as_deref()
        .or(entry.access_token.as_deref())
        .unwrap_or("")
        .trim()
        .to_string();
    if runtime_key.is_empty() {
        return None;
    }

    Some(NousPortalAccountInfo {
        logged_in: false,
        source: NousAccountInfoSource::InferenceKey,
        fresh: false,
        portal_base_url: entry.portal_base_url.clone().or(portal_base_url),
        inference_base_url: entry
            .inference_base_url
            .clone()
            .or_else(|| entry.runtime_base_url.clone())
            .or_else(|| entry.base_url.clone()),
        inference_credential_present: true,
        credential_source: Some(format!("pool:{}", entry.label.as_deref().unwrap_or("unknown"))),
        error: Some("portal_oauth_missing".to_string()),
        ..Default::default()
    })
}

#[allow(dead_code)]
fn _info_from_inference_key_pool(portal_base_url: Option<String>) -> Option<NousPortalAccountInfo> {
    info_from_inference_key_pool(portal_base_url)
}

/// Mirrors `_info_from_oauth_pool` (ll.485-553).
pub fn info_from_oauth_pool(
    force_fresh: bool,
    min_jwt_ttl_seconds: i64,
    portal_base_url: Option<String>,
) -> Option<NousPortalAccountInfo> {
    let entry = select_nous_pool_entry()?;
    if !pool_entry_is_portal_oauth(&entry) {
        return None;
    }

    let access_token = entry.access_token.as_deref()?.trim().to_string();
    if access_token.is_empty() {
        return None;
    }

    let entry_portal_url = entry.portal_base_url.clone().or(portal_base_url);

    let mut state: AuthState = HashMap::new();
    state.insert("access_token".to_string(), Value::String(access_token.clone()));
    if let Some(cid) = &entry.client_id {
        if !cid.trim().is_empty() {
            state.insert("client_id".to_string(), Value::String(cid.clone()));
        }
    }
    let inference_base = entry
        .inference_base_url
        .clone()
        .or_else(|| entry.runtime_base_url.clone())
        .or_else(|| entry.base_url.clone());
    if let Some(ib) = inference_base {
        if !ib.trim().is_empty() {
            state.insert("inference_base_url".to_string(), Value::String(ib));
        }
    }
    if let Some(ak) = &entry.agent_key {
        if !ak.trim().is_empty() {
            state.insert("agent_key".to_string(), Value::String(ak.clone()));
        }
    }
    state.insert(
        "credential_source".to_string(),
        Value::String(format!("pool:{}", entry.label.as_deref().unwrap_or("unknown"))),
    );

    if !force_fresh {
        if let Some(jwt_info) = info_from_valid_jwt(
            &access_token,
            &state,
            entry_portal_url.clone(),
            min_jwt_ttl_seconds,
        ) {
            return Some(jwt_info);
        }
    }

    let payload = match fetch_nous_account_info(&access_token, entry_portal_url.as_deref()) {
        Ok(p) => p,
        Err(exc) => {
            return Some(error_info(exc, true, entry_portal_url, None));
        }
    };

    if payload.is_empty() {
        return Some(error_info(
            "empty_account_response",
            true,
            entry_portal_url,
            None,
        ));
    }
    if let Some(Value::String(err)) = payload.get("error") {
        if !err.trim().is_empty() {
            return Some(error_info(
                err.clone(),
                true,
                entry_portal_url,
                Some(payload),
            ));
        }
    }

    Some(info_from_account_payload(
        &payload,
        &state,
        entry_portal_url,
    ))
}

#[allow(dead_code)]
fn _info_from_oauth_pool(
    force_fresh: bool,
    min_jwt_ttl_seconds: i64,
    portal_base_url: Option<String>,
) -> Option<NousPortalAccountInfo> {
    info_from_oauth_pool(force_fresh, min_jwt_ttl_seconds, portal_base_url)
}

// ---------------------------------------------------------------------------
// Pool selection — mirrors ll.556-582
// ---------------------------------------------------------------------------

/// Mirrors `_select_nous_pool_entry` (ll.556-572).
pub fn select_nous_pool_entry() -> Option<PoolEntry> {
    let entries = load_nous_pool();
    if entries.is_empty() {
        return None;
    }
    // Mirrors `_entry_sort_key` (ll.566-570): `max(entries, key=lambda e: (agent_exp, access_exp, -priority))`
    // Real impl parses ISO timestamps; stub uses `parse_iso_timestamp` helper.
    let mut best: Option<&PoolEntry> = None;
    let mut best_key: (f64, f64, i64) = (f64::NEG_INFINITY, f64::NEG_INFINITY, i64::MIN);
    for entry in &entries {
        let agent_exp = parse_iso_timestamp(entry.agent_key_expires_at.as_deref()).unwrap_or(0.0);
        let access_exp = parse_iso_timestamp(entry.expires_at.as_deref()).unwrap_or(0.0);
        let priority = entry.priority;
        let key = (agent_exp, access_exp, -priority);
        let is_better = match best {
            None => true,
            Some(_) => {
                key.0 > best_key.0
                    || (key.0 == best_key.0 && key.1 > best_key.1)
                    || (key.0 == best_key.0 && key.1 == best_key.1 && key.2 > best_key.2)
            }
        };
        if is_better {
            best = Some(entry);
            best_key = key;
        }
    }
    best.cloned()
}

#[allow(dead_code)]
fn _select_nous_pool_entry() -> Option<PoolEntry> {
    select_nous_pool_entry()
}

/// Mirrors `_pool_entry_is_portal_oauth` (ll.575-582).
pub fn pool_entry_is_portal_oauth(entry: &PoolEntry) -> bool {
    let at = entry.access_token.as_deref().map(|s| s.trim()).unwrap_or("");
    if at.is_empty() {
        return false;
    }
    let auth_type = entry.auth_type.as_deref().unwrap_or("").trim().to_lowercase();
    let has_refresh = entry
        .refresh_token
        .as_deref()
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false);
    auth_type.starts_with("oauth") || has_refresh
}

#[allow(dead_code)]
fn _pool_entry_is_portal_oauth(entry: &PoolEntry) -> bool {
    pool_entry_is_portal_oauth(entry)
}

// ---------------------------------------------------------------------------
// Network fetch — mirrors ll.584-598
// ---------------------------------------------------------------------------

/// Mirrors `_fetch_nous_account_info` (ll.584-598).
/// Real impl: `urllib.request.Request("{base}/api/oauth/account", headers={Bearer})` → `json.loads`.
/// Std-only stub shells to `curl -fsSL` so the crate stays without `reqwest` until provider wave.
pub fn fetch_nous_account_info(
    access_token: &str,
    portal_base_url: Option<&str>,
) -> Result<HashMap<String, Value>, String> {
    let base = portal_base_url
        .map(|s| s.trim().trim_end_matches('/').to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_NOUS_PORTAL_URL.to_string());
    let url = format!("{}/api/oauth/account", base.trim_end_matches('/'));

    // Try curl (mirrors `urllib.request.urlopen(..., timeout=8)`).
    // Timeout 8s is preserved as `curl --max-time 8`.
    let output = Command::new("curl")
        .args([
            "-fsSL",
            "--max-time",
            "8",
            "-H",
            &format!("Authorization: Bearer {}", access_token),
            "-H",
            "Accept: application/json",
            &url,
        ])
        .output();

    match output {
        Ok(out) if out.status.success() => {
            let text = String::from_utf8_lossy(&out.stdout).to_string();
            let payload = parse_json_object(&text);
            Ok(payload)
        }
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
            let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
            let detail = if !stderr.is_empty() {
                stderr
            } else if !stdout.is_empty() {
                stdout
            } else {
                format!("HTTP error fetching {}", url)
            };
            Err(detail)
        }
        Err(e) => Err(format!("failed to invoke curl for {}: {}", url, e)),
    }
}

#[allow(dead_code)]
fn _fetch_nous_account_info(
    access_token: &str,
    portal_base_url: Option<&str>,
) -> Result<HashMap<String, Value>, String> {
    fetch_nous_account_info(access_token, portal_base_url)
}

// Minimal JSON object parser for `_fetch_nous_account_info` payload (std-only).
// Extracts top-level keys as `Value::String` / `Value::Object` shallow so `_info_from_account_payload`
// and error checks can run. Full parsing would use `serde_json::from_str`.
fn parse_json_object(text: &str) -> HashMap<String, Value> {
    let mut map: HashMap<String, Value> = HashMap::new();
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed == "{}" {
        return map;
    }
    // Preserve raw error string if top-level contains `"error": "..."`
    if let Some(err) = extract_json_string(trimmed, "error") {
        map.insert("error".to_string(), Value::String(err));
    }
    // Try to extract shallow objects: `user`, `organisation`, `subscription`, `paid_service_access`, `tool_access`
    for key in ["user", "organisation", "subscription", "paid_service_access", "tool_access"] {
        if let Some(obj_text) = extract_json_object_for_key(trimmed, key) {
            // Store as Object with stringified inner for coercion helpers to re-extract via string search.
            // For richer typed access, also store the raw object text under a synthetic key.
            // Hand-rolled inner parsing: extract string/number/bool fields shallow.
            let inner_map = parse_shallow_object(&obj_text);
            map.insert(key.to_string(), Value::Object(inner_map));
        }
    }
    // If nothing was extracted but text looks like JSON, keep empty (caller treats as empty → error_info).
    map
}

fn parse_shallow_object(obj_text: &str) -> HashMap<String, Value> {
    let mut out: HashMap<String, Value> = HashMap::new();
    // obj_text includes braces `{...}`. Naive split by commas at depth 0.
    let inner = obj_text.trim();
    let inner = inner.strip_prefix('{').unwrap_or(inner).strip_suffix('}').unwrap_or(inner);
    for pair in split_json_pairs(inner) {
        let p = pair.trim();
        if p.is_empty() {
            continue;
        }
        let colon = match p.find(':') {
            Some(c) => c,
            None => continue,
        };
        let k_raw = p[..colon].trim();
        let v_raw = p[colon + 1..].trim();
        let key = match parse_json_string(k_raw) {
            Some(k) => k,
            None => continue,
        };
        let value = if v_raw.starts_with('"') {
            match parse_json_string(v_raw) {
                Some(s) => Value::String(s),
                None => Value::Null,
            }
        } else if v_raw == "null" {
            Value::Null
        } else if v_raw == "true" {
            Value::Bool(true)
        } else if v_raw == "false" {
            Value::Bool(false)
        } else if v_raw.starts_with('{') {
            // Nested object — store as string placeholder; deeper parsing will use extract helpers on raw text.
            Value::String(v_raw.to_string())
        } else if v_raw.starts_with('[') {
            Value::String(v_raw.to_string())
        } else {
            // Number
            if let Ok(i) = v_raw.parse::<i64>() {
                // Check if it contains '.' or 'e' → float
                if v_raw.contains('.') || v_raw.contains('e') || v_raw.contains('E') {
                    if let Ok(f) = v_raw.parse::<f64>() {
                        Value::Number(f)
                    } else {
                        Value::Int(i)
                    }
                } else {
                    Value::Int(i)
                }
            } else if let Ok(f) = v_raw.parse::<f64>() {
                Value::Number(f)
            } else {
                Value::String(v_raw.to_string())
            }
        };
        out.insert(key, value);
    }
    out
}

// ---------------------------------------------------------------------------
// JWT path — mirrors ll.601-647
// ---------------------------------------------------------------------------

/// Stub: mirrors `hermes_cli.auth._decode_jwt_claims` (ll.608).
/// Decodes the JWT payload (base64url) and returns claims as `HashMap<String, Value>`.
/// Std-only: hand-rolled base64url decode + minimal JSON extraction.
pub fn decode_jwt_claims(token: &str) -> Option<HashMap<String, Value>> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() < 2 {
        return None;
    }
    let payload_b64 = parts[1];
    let decoded = base64url_decode(payload_b64)?;
    let text = String::from_utf8(decoded).ok()?;
    let trimmed = text.trim();
    if !trimmed.starts_with('{') {
        return None;
    }
    // Minimal extraction of known claim keys so `_info_from_valid_jwt` can coerce.
    let mut claims: HashMap<String, Value> = HashMap::new();
    for key in [
        "exp",
        "sub",
        "org_id",
        "client_id",
        "product_id",
        "nous_client",
        "paid_access",
        "subscription_tier",
        "tool_access",
    ] {
        // Try number first, then bool, then string, then object
        if let Some(n) = extract_json_number(trimmed, key) {
            // Distinguish int vs float by presence of '.'
            let raw = extract_json_raw_number_token(trimmed, key).unwrap_or_default();
            if raw.contains('.') || raw.contains('e') || raw.contains('E') {
                claims.insert(key.to_string(), Value::Number(n));
            } else {
                // Prefer Int when possible
                if let Ok(i) = raw.parse::<i64>() {
                    claims.insert(key.to_string(), Value::Int(i));
                } else {
                    claims.insert(key.to_string(), Value::Number(n));
                }
            }
            continue;
        }
        if let Some(b) = extract_json_bool(trimmed, key) {
            claims.insert(key.to_string(), Value::Bool(b));
            continue;
        }
        if let Some(s) = extract_json_string(trimmed, key) {
            claims.insert(key.to_string(), Value::String(s));
            continue;
        }
        if let Some(obj) = extract_json_object_for_key(trimmed, key) {
            let inner = parse_shallow_object(&obj);
            claims.insert(key.to_string(), Value::Object(inner));
        }
    }
    if claims.is_empty() {
        return None;
    }
    Some(claims)
}

fn base64url_decode(input: &str) -> Option<Vec<u8>> {
    // Replace URL-safe chars, add padding
    let mut s = input.replace('-', "+").replace('_', "/");
    while s.len() % 4 != 0 {
        s.push('=');
    }
    b64d(&s).ok()
}

fn b64d(text: &str) -> Result<Vec<u8>, String> {
    let s = text.trim();
    if s.is_empty() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    let mut buf: u32 = 0;
    let mut bits: u32 = 0;
    for c in s.chars() {
        if c == '=' {
            break;
        }
        let val = match c {
            'A'..='Z' => c as u32 - 'A' as u32,
            'a'..='z' => c as u32 - 'a' as u32 + 26,
            '0'..='9' => c as u32 - '0' as u32 + 52,
            '+' => 62,
            '/' => 63,
            _ => return Err(format!("invalid base64 char {:?}", c)),
        };
        buf = (buf << 6) | val;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((buf >> bits) & 0xFF) as u8);
        }
    }
    Ok(out)
}

/// Mirrors `_info_from_valid_jwt` (ll.600-647).
pub fn info_from_valid_jwt(
    token: &str,
    state: &AuthState,
    portal_base_url: Option<String>,
    min_jwt_ttl_seconds: i64,
) -> Option<NousPortalAccountInfo> {
    let claims = decode_jwt_claims(token)?;
    if claims.is_empty() {
        return None;
    }

    let exp = coerce_float(claims.get("exp"))?;
    let min_ttl = min_jwt_ttl_seconds.max(0) as f64;
    if exp <= now_secs() + min_ttl {
        return None;
    }

    let paid_access = coerce_bool(claims.get("paid_access"));
    let subscription_tier = coerce_int(claims.get("subscription_tier"));

    let access_info = NousPaidServiceAccessInfo {
        allowed: paid_access,
        paid_access,
        organisation_id: coerce_str(claims.get("org_id")),
        subscription_tier,
        ..Default::default()
    };

    let credential_source = auth_state_get_str(state, "credential_source")
        .or_else(|| Some("auth_store".to_string()));

    // portal and inference base urls as strings
    let inference_base_url = auth_state_get_str(state, "inference_base_url");

    Some(NousPortalAccountInfo {
        logged_in: true,
        source: NousAccountInfoSource::Jwt,
        fresh: false,
        user_id: coerce_str(claims.get("sub")),
        org_id: coerce_str(claims.get("org_id")),
        client_id: coerce_str(claims.get("client_id")).or_else(|| auth_state_get_str(state, "client_id")),
        product_id: coerce_str(claims.get("product_id")),
        nous_client: coerce_str(claims.get("nous_client")),
        portal_base_url,
        inference_base_url,
        inference_credential_present: true,
        credential_source,
        expires_at: Some(UNIX_EPOCH + Duration::from_secs_f64(exp.max(0.0))),
        paid_service_access: paid_access,
        paid_service_access_info: Some(access_info),
        tool_access: claims.get("tool_access").and_then(|v| tool_access_from_value(Some(v))),
        raw_claims: Some(claims),
        ..Default::default()
    })
}

#[allow(dead_code)]
fn _info_from_valid_jwt(
    token: &str,
    state: &AuthState,
    portal_base_url: Option<String>,
    min_jwt_ttl_seconds: i64,
) -> Option<NousPortalAccountInfo> {
    info_from_valid_jwt(token, state, portal_base_url, min_jwt_ttl_seconds)
}

// ---------------------------------------------------------------------------
// Account payload path — mirrors ll.650-702
// ---------------------------------------------------------------------------

/// Mirrors `_info_from_account_payload` (ll.650-685).
pub fn info_from_account_payload(
    payload: &HashMap<String, Value>,
    state: &AuthState,
    portal_base_url: Option<String>,
) -> NousPortalAccountInfo {
    let user_map: HashMap<String, Value> = match payload.get("user") {
        Some(Value::Object(m)) => m.clone(),
        _ => HashMap::new(),
    };
    let org_map: HashMap<String, Value> = match payload.get("organisation") {
        Some(Value::Object(m)) => m.clone(),
        _ => HashMap::new(),
    };

    let subscription = subscription_from_payload(payload.get("subscription"));
    let access = paid_service_access_from_payload(payload.get("paid_service_access"));
    let mut paid_access = access.as_ref().and_then(|a| a.allowed);
    if paid_access.is_none() {
        if let Some(a) = &access {
            paid_access = a.paid_access;
        }
    }

    let org_id = coerce_str(org_map.get("id")).or_else(|| access.as_ref().and_then(|a| a.organisation_id.clone()));
    let credential_source =
        auth_state_get_str(state, "credential_source").or_else(|| Some("auth_store".to_string()));

    let inference_present = auth_state_get_str(state, "access_token").is_some()
        || auth_state_get_str(state, "agent_key").is_some();

    NousPortalAccountInfo {
        logged_in: true,
        source: NousAccountInfoSource::AccountApi,
        fresh: true,
        org_id,
        org_slug: coerce_str(org_map.get("slug")),
        org_name: coerce_str(org_map.get("name")),
        client_id: auth_state_get_str(state, "client_id"),
        portal_base_url,
        inference_base_url: auth_state_get_str(state, "inference_base_url"),
        inference_credential_present: inference_present,
        credential_source,
        email: coerce_str(user_map.get("email")),
        privy_did: coerce_str(user_map.get("privy_did")),
        subscription,
        paid_service_access: paid_access,
        paid_service_access_info: access,
        tool_access: tool_access_from_value(payload.get("tool_access")),
        raw_account: Some(payload.clone()),
        ..Default::default()
    }
}

#[allow(dead_code)]
fn _info_from_account_payload(
    payload: &HashMap<String, Value>,
    state: &AuthState,
    portal_base_url: Option<String>,
) -> NousPortalAccountInfo {
    info_from_account_payload(payload, state, portal_base_url)
}

/// Mirrors `_tool_access_from_value` (ll.688-702).
pub fn tool_access_from_value(value: Option<&Value>) -> Option<NousToolAccessInfo> {
    let map = match value? {
        Value::Object(m) => m,
        _ => return None,
    };
    let enabled = coerce_bool(map.get("enabled")) == Some(true);
    let mut coverage: HashMap<String, bool> = HashMap::new();
    if let Some(Value::Object(cov)) = map.get("coverage") {
        for (k, v) in cov {
            // Mirrors `coverage[key] = val is True` (l.701) — only literal true counts
            coverage.insert(k.clone(), v == &Value::Bool(true));
        }
    } else if let Some(Value::String(_)) = map.get("coverage") {
        // non-dict → leave empty (Python checks isinstance(raw_coverage, dict))
    }
    Some(NousToolAccessInfo { enabled, coverage })
}

#[allow(dead_code)]
fn _tool_access_from_value(value: Option<&Value>) -> Option<NousToolAccessInfo> {
    tool_access_from_value(value)
}

/// Mirrors `_subscription_from_payload` (ll.705-716).
pub fn subscription_from_payload(value: Option<&Value>) -> Option<NousPortalSubscriptionInfo> {
    let map = match value? {
        Value::Object(m) => m,
        _ => return None,
    };
    Some(NousPortalSubscriptionInfo {
        plan: coerce_str(map.get("plan")),
        tier: coerce_int(map.get("tier")),
        monthly_charge: coerce_float(map.get("monthly_charge")),
        monthly_credits: coerce_float(map.get("monthly_credits")),
        current_period_end: coerce_str(map.get("current_period_end")),
        credits_remaining: coerce_float(map.get("credits_remaining")),
        rollover_credits: coerce_float(map.get("rollover_credits")),
    })
}

#[allow(dead_code)]
fn _subscription_from_payload(value: Option<&Value>) -> Option<NousPortalSubscriptionInfo> {
    subscription_from_payload(value)
}

/// Mirrors `_paid_service_access_from_payload` (ll.719-741).
pub fn paid_service_access_from_payload(value: Option<&Value>) -> Option<NousPaidServiceAccessInfo> {
    let map = match value? {
        Value::Object(m) => m,
        _ => return None,
    };
    Some(NousPaidServiceAccessInfo {
        allowed: coerce_bool(map.get("allowed")),
        paid_access: coerce_bool(map.get("paid_access")),
        reason: coerce_str(map.get("reason")),
        organisation_id: coerce_str(map.get("organisation_id")),
        effective_at_ms: coerce_int(map.get("effective_at_ms")),
        has_active_subscription: coerce_bool(map.get("has_active_subscription")),
        active_subscription_is_paid: coerce_bool(map.get("active_subscription_is_paid")),
        subscription_tier: coerce_int(map.get("subscription_tier")),
        subscription_monthly_charge: coerce_float(map.get("subscription_monthly_charge")),
        subscription_credits_remaining: coerce_float(map.get("subscription_credits_remaining")),
        purchased_credits_remaining: coerce_float(map.get("purchased_credits_remaining")),
        total_usable_credits: coerce_float(map.get("total_usable_credits")),
        member_spend_cap_exceeded: coerce_bool(map.get("member_spend_cap_exceeded")),
        member_spend_cap_usd: coerce_float(map.get("member_spend_cap_usd")),
        member_spend_usd: coerce_float(map.get("member_spend_usd")),
        member_spend_cap_remaining_usd: coerce_float(map.get("member_spend_cap_remaining_usd")),
    })
}

#[allow(dead_code)]
fn _paid_service_access_from_payload(value: Option<&Value>) -> Option<NousPaidServiceAccessInfo> {
    paid_service_access_from_payload(value)
}

// ---------------------------------------------------------------------------
// Error / small helpers — mirrors ll.744-814
// ---------------------------------------------------------------------------

/// Mirrors `_error_info` (ll.744-758).
pub fn error_info(
    error: impl ToString,
    logged_in: bool,
    portal_base_url: Option<String>,
    raw_account: Option<HashMap<String, Value>>,
) -> NousPortalAccountInfo {
    NousPortalAccountInfo {
        logged_in,
        source: NousAccountInfoSource::Error,
        fresh: false,
        portal_base_url,
        raw_account,
        error: Some(error.to_string()),
        ..Default::default()
    }
}

#[allow(dead_code)]
fn _error_info(
    error: impl ToString,
    logged_in: bool,
    portal_base_url: Option<String>,
    raw_account: Option<HashMap<String, Value>>,
) -> NousPortalAccountInfo {
    error_info(error, logged_in, portal_base_url, raw_account)
}

/// Mirrors `_portal_base_url` (ll.761-766).
pub fn portal_base_url_from_state(state: &AuthState) -> Option<String> {
    let v = state.get("portal_base_url")?;
    match v {
        Value::String(s) if !s.trim().is_empty() => Some(s.trim().trim_end_matches('/').to_string()),
        _ => None,
    }
}

#[allow(dead_code)]
fn _portal_base_url(state: &AuthState) -> Option<String> {
    portal_base_url_from_state(state)
}

/// Mirrors `_cache_key` (ll.768-770): `f"{portal_base_url or ''}:{sha256(token).hexdigest()}"`.
pub fn cache_key_str(access_token: &str, portal_base_url: Option<&str>) -> String {
    let digest = sha256_hex(access_token);
    format!("{}:{}", portal_base_url.unwrap_or(""), digest)
}

#[allow(dead_code)]
fn _cache_key(access_token: &str, portal_base_url: Option<&str>) -> String {
    cache_key_str(access_token, portal_base_url)
}

fn sha256_hex(input: &str) -> String {
    // Try `sha256sum` / `shasum` so we stay std-only; fallback to FNV hex (mirrors bitwarden.rs).
    // Real impl would use `sha2::Sha256`.
    let probe = Command::new("sh")
        .args(["-c", &format!("printf %s {} | sha256sum 2>/dev/null | cut -d' ' -f1", shell_escape(input))])
        .output();
    if let Ok(out) = probe {
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit()) {
                return s;
            }
        }
    }
    // macOS shasum fallback
    if let Ok(out) = Command::new("sh")
        .args(["-c", &format!("printf %s {} | shasum -a 256 2>/dev/null | cut -d' ' -f1", shell_escape(input))])
        .output()
    {
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit()) {
                return s;
            }
        }
    }
    // Fallback FNV-1a expanded to 64 hex chars (deterministic, never equal to real SHA in prod verification)
    let mut h: u64 = 0xcbf29ce484222325;
    for b in input.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    // Expand to 64 hex by repeating hash with rotation
    let mut acc = String::new();
    let mut cur = h;
    while acc.len() < 64 {
        acc.push_str(&format!("{:016x}", cur));
        cur = cur.wrapping_mul(0x100000001b3) ^ (cur >> 32);
    }
    acc[..64].to_string()
}

fn shell_escape(s: &str) -> String {
    // Safe single-quote escaping for `printf %s '...'` payload
    let escaped = s.replace('\'', "'\\''");
    format!("'{}'", escaped)
}

/// Mirrors `_parse_iso_timestamp` (ll.773-783).
pub fn parse_iso_timestamp(value: Option<&str>) -> Option<f64> {
    let raw = value?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut text = trimmed.to_string();
    if text.ends_with('Z') {
        text = format!("{}+00:00", &text[..text.len() - 1]);
    }
    // Try `datetime.fromisoformat` semantics via our ISO parser (handles `+00:00` suffix)
    // Reuse the ISO-8601 manual parser from `account_usage.rs` semantics.
    // First try parsing as float epoch (not in original _parse_iso_timestamp, but keep parity with _coerce handling)
    // The Python version only does `datetime.fromisoformat` after Z rewrite.
    parse_iso8601_to_timestamp(&text)
}

fn parse_iso8601_to_timestamp(text: &str) -> Option<f64> {
    // Handles `YYYY-MM-DDTHH:MM:SS[.frac][+HH:MM]` or naive → UTC.
    // Minimal std-only, mirrors Python's `datetime.fromisoformat(...).timestamp()`.
    let s = text.trim();
    if s.is_empty() {
        return None;
    }
    // Strip timezone offset
    let (dt_part, tz_offset_secs) = split_tz_for_iso(s)?;
    let t_pos = dt_part.find('T').or_else(|| dt_part.find(' '))?;
    let date_str = &dt_part[..t_pos];
    let time_str = &dt_part[t_pos + 1..];

    let date_parts: Vec<&str> = date_str.split('-').collect();
    if date_parts.len() != 3 {
        return None;
    }
    let year: i32 = date_parts[0].parse().ok()?;
    let month: u32 = date_parts[1].parse().ok()?;
    let day: u32 = date_parts[2].parse().ok()?;

    let time_parts: Vec<&str> = time_str.split(':').collect();
    if time_parts.len() < 2 {
        return None;
    }
    let hour: u32 = time_parts[0].parse().ok()?;
    let minute: u32 = time_parts[1].parse().ok()?;
    let sec_frac = time_parts[2];
    let second: u32 = if let Some(dot) = sec_frac.find('.') {
        sec_frac[..dot].parse().ok()?
    } else if let Some(plus) = sec_frac.find('+') {
        sec_frac[..plus].parse().ok()?
    } else if let Some(minus) = sec_frac.find('-') {
        sec_frac[..minus].parse().ok()?
    } else {
        sec_frac.parse().ok()?
    };

    let days = days_since_epoch(year, month, day)?;
    let secs_of_day = (hour as i64) * 3600 + (minute as i64) * 60 + (second as i64);
    let epoch_secs = days * 86400 + secs_of_day - tz_offset_secs;
    Some(epoch_secs as f64)
}

fn split_tz_for_iso(s: &str) -> Option<(String, i64)> {
    // Find timezone offset after 'T', else naive (0)
    if let Some(t_pos) = s.find('T').or_else(|| s.find(' ')) {
        let after_t = &s[t_pos + 1..];
        if let Some(plus) = after_t.rfind('+') {
            if plus >= 5 {
                let tz = &after_t[plus..];
                if tz.contains(':') {
                    let off = parse_tz_offset(tz)?;
                    let dt = s[..t_pos + 1 + plus].to_string();
                    return Some((dt, off));
                }
            }
        }
        if let Some(minus) = after_t.rfind('-') {
            if minus >= 5 {
                let tz = &after_t[minus..];
                if tz.contains(':') {
                    let off = parse_tz_offset(tz)?;
                    let dt = s[..t_pos + 1 + minus].to_string();
                    return Some((dt, off));
                }
            }
        }
    }
    Some((s.to_string(), 0))
}

fn parse_tz_offset(tz: &str) -> Option<i64> {
    let sign = if tz.starts_with('+') { 1 } else if tz.starts_with('-') { -1 } else { return None };
    let rest = &tz[1..];
    let parts: Vec<&str> = rest.split(':').collect();
    if parts.len() != 2 {
        return None;
    }
    let hh: i64 = parts[0].parse().ok()?;
    let mm: i64 = parts[1].parse().ok()?;
    Some(sign * (hh * 3600 + mm * 60))
}

fn days_since_epoch(year: i32, month: u32, day: u32) -> Option<i64> {
    let mut y = year as i64;
    let mut m = month as i64;
    let d = day as i64;
    if m <= 2 {
        y -= 1;
        m += 12;
    }
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400;
    let doy = (153 * (m - 3) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146097 + doe - 719468)
}

#[allow(dead_code)]
fn _parse_iso_timestamp(value: Option<&str>) -> Option<f64> {
    parse_iso_timestamp(value)
}

// ---------------------------------------------------------------------------
// Coercion helpers — mirrors ll.786-814
// ---------------------------------------------------------------------------

/// Mirrors `_coerce_str` (ll.785-788): `value if isinstance(value, str) and value else None`.
pub fn coerce_str(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        _ => None,
    }
}

#[allow(dead_code)]
fn _coerce_str(value: Option<&Value>) -> Option<String> {
    coerce_str(value)
}

/// Mirrors `_coerce_bool` (ll.791-792): `value if isinstance(value, bool) else None`.
pub fn coerce_bool(value: Option<&Value>) -> Option<bool> {
    match value? {
        Value::Bool(b) => Some(*b),
        _ => None,
    }
}

#[allow(dead_code)]
fn _coerce_bool(value: Option<&Value>) -> Option<bool> {
    coerce_bool(value)
}

/// Mirrors `_coerce_int` (ll.795-803): `int(value)` with bool guard.
pub fn coerce_int(value: Option<&Value>) -> Option<i64> {
    match value? {
        Value::Bool(_) => None,
        Value::Int(i) => Some(*i),
        Value::Number(n) => Some(*n as i64),
        Value::String(s) => s.trim().parse::<i64>().ok().or_else(|| s.trim().parse::<f64>().ok().map(|f| f as i64)),
        _ => None,
    }
}

#[allow(dead_code)]
fn _coerce_int(value: Option<&Value>) -> Option<i64> {
    coerce_int(value)
}

/// Mirrors `_coerce_float` (ll.806-814): `float(value)` with bool guard.
pub fn coerce_float(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::Bool(_) => None,
        Value::Number(n) if n.is_finite() => Some(*n),
        Value::Int(i) => Some(*i as f64),
        Value::String(s) => s.trim().parse::<f64>().ok().filter(|f| f.is_finite()),
        Value::Number(n) => {
            // Already handled finite case; non-finite → None mirrors `math.isfinite` via no return
            if n.is_finite() { Some(*n) } else { None }
        }
        _ => None,
    }
}

#[allow(dead_code)]
fn _coerce_float(value: Option<&Value>) -> Option<f64> {
    coerce_float(value)
}

// ---------------------------------------------------------------------------
// Minimal JSON extraction helpers — mirrors `response.json()` dict indexing
// Std-only, hand-rolled (maps to real `serde_json` at merge)
// ---------------------------------------------------------------------------

fn extract_json_string(json: &str, field: &str) -> Option<String> {
    let needle = format!("\"{}\"", field);
    let idx = json.find(&needle)?;
    let after = &json[idx + needle.len()..];
    let colon = after.find(':')?;
    let rest = after[colon + 1..].trim_start();
    if !rest.starts_with('"') {
        return None;
    }
    parse_json_string(rest)
}

fn extract_json_number(json: &str, field: &str) -> Option<f64> {
    let needle = format!("\"{}\"", field);
    let idx = json.find(&needle)?;
    let after = &json[idx + needle.len()..];
    let colon = after.find(':')?;
    let rest = after[colon + 1..].trim_start();
    if rest.starts_with("null") || rest.starts_with("true") || rest.starts_with("false") || rest.starts_with('"') {
        return None;
    }
    let end = rest
        .find(|c: char| c == ',' || c == '}' || c == ']' || c.is_whitespace())
        .unwrap_or(rest.len());
    rest[..end].trim().parse::<f64>().ok()
}

fn extract_json_raw_number_token(json: &str, field: &str) -> Option<String> {
    let needle = format!("\"{}\"", field);
    let idx = json.find(&needle)?;
    let after = &json[idx + needle.len()..];
    let colon = after.find(':')?;
    let rest = after[colon + 1..].trim_start();
    let end = rest
        .find(|c: char| c == ',' || c == '}' || c == ']' || c.is_whitespace())
        .unwrap_or(rest.len());
    Some(rest[..end].trim().to_string())
}

fn extract_json_bool(json: &str, field: &str) -> Option<bool> {
    let needle = format!("\"{}\"", field);
    let idx = json.find(&needle)?;
    let after = &json[idx + needle.len()..];
    let colon = after.find(':')?;
    let rest = after[colon + 1..].trim_start();
    if rest.starts_with("true") {
        Some(true)
    } else if rest.starts_with("false") {
        Some(false)
    } else {
        None
    }
}

fn extract_json_object_for_key(json: &str, key: &str) -> Option<String> {
    let needle = format!("\"{}\"", key);
    let idx = json.find(&needle)?;
    let after = &json[idx + needle.len()..];
    let colon = after.find(':')?;
    let rest = after[colon + 1..].trim_start();
    if !rest.starts_with('{') {
        return None;
    }
    let end = find_matching_brace(rest)?;
    Some(rest[..=end].to_string())
}

fn find_matching_brace(s: &str) -> Option<usize> {
    let mut depth = 0usize;
    let mut in_str = false;
    let mut escape = false;
    for (i, c) in s.char_indices() {
        if in_str {
            if escape {
                escape = false;
            } else if c == '\\' {
                escape = true;
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        match c {
            '"' => in_str = true,
            '{' => depth += 1,
            '}' => {
                if depth == 0 {
                    return None;
                }
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

fn split_json_pairs(inner: &str) -> Vec<String> {
    let mut pairs = Vec::new();
    let mut cur = String::new();
    let mut depth = 0usize;
    let mut in_str = false;
    let mut escape = false;
    for c in inner.chars() {
        if in_str {
            cur.push(c);
            if escape {
                escape = false;
            } else if c == '\\' {
                escape = true;
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        match c {
            '"' => {
                in_str = true;
                cur.push(c);
            }
            '{' | '[' => {
                depth += 1;
                cur.push(c);
            }
            '}' | ']' => {
                depth -= 1;
                cur.push(c);
            }
            ',' if depth == 0 => {
                pairs.push(cur.clone());
                cur.clear();
            }
            _ => cur.push(c),
        }
    }
    if !cur.trim().is_empty() {
        pairs.push(cur);
    }
    pairs
}

fn parse_json_string(s: &str) -> Option<String> {
    let s = s.trim();
    if !s.starts_with('"') {
        return None;
    }
    let mut out = String::new();
    let mut chars = s[1..].chars();
    let mut escape = false;
    while let Some(c) = chars.next() {
        if escape {
            match c {
                '"' => out.push('"'),
                '\\' => out.push('\\'),
                '/' => out.push('/'),
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                'u' => {
                    let hex: String = chars.by_ref().take(4).collect();
                    if let Ok(code) = u32::from_str_radix(&hex, 16) {
                        if let Some(ch) = char::from_u32(code) {
                            out.push(ch);
                        }
                    }
                }
                _ => out.push(c),
            }
            escape = false;
        } else if c == '\\' {
            escape = true;
        } else if c == '"' {
            return Some(out);
        } else {
            out.push(c);
        }
    }
    None
}
