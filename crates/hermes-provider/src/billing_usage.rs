//! Shared dollar-denominated usage model for the billing/subscription surfaces.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/agent/billing_usage.py` (323 lines).
//!
//! The single source of truth behind the `/usage` and `/subscription` usage
//! bars (TUI + CLI). Terminal surfaces show **dollars**, never "credits", and
//! every bar makes the monthly subscription allowance and separately-purchased
//! top-up dollars distinctly visible.
//!
//! Data source: the NAS account-info fetch (`NousPortalAccountInfo`), whose
//! `paid_service_access_info` carries the three dollar magnitudes (despite the
//! legacy `*_credits` field names, these are USD floats):
//!   - `subscription_credits_remaining`  -> plan dollars left this month
//!   - `purchased_credits_remaining`     -> top-up dollars left (rolls over)
//!   - `total_usable_credits`            -> total spendable
//! plus `subscription.monthly_credits` and `current_period_end`.
//!
//! Design: two SEPARATE bars rather than one crammed three-segment bar — at
//! terminal widths three same-glyph density segments are unreadable. Fail-open
//! everywhere: any missing/non-finite field degrades to fewer bars; logged-out
//! yields `available=False`.
//!
//! T0043 — 1:1 port, no cargo (NEVER cargo).
//!
//! Notes on fidelity vs. Rust idioms:
//! - Python `dataclass(frozen=True)` ↔ `#[derive(Debug, Clone)]` structs (immutable by convention).
//! - Python `Optional[float]` ↔ `Option<f64>`; `Optional[str]` ↔ `Option<String>`.
//! - Python `Any` payloads + `getattr(obj, "field", None)` ↔ `Option` fields on typed structs;
//!   dynamic `Any` path ↔ `Value` enum (std-only `serde_json::Value` stand-in).
//! - Python `math.isfinite` ↔ `f64::is_finite()`; `isinstance(bool)` guard is implicit (Rust `bool` ≠ `f64`).
//! - Python `datetime.fromisoformat` + `strptime("%Y-%m-%d")` + `strftime('%b')` ↔ manual ISO-8601 parser + month table.
//! - Python `logger.debug(..., exc_info=True)` ↔ `// log::debug!` elided (fail-open is silent).
//! - Python `os.getenv("HERMES_DEV_CREDITS_FIXTURE")` ↔ `env::var` same key.
//! - Python `get_provider_auth_state("nous")` / `get_nous_portal_account_info` ↔ stubs reading env/file (std-only).
//! - Python `concurrent.futures.ThreadPoolExecutor(max_workers=1).result(timeout)` ↔ `std::thread` + `mpsc::sync_channel` + `recv_timeout`.
//! - Crate stays `std`-only — no `chrono`, `serde`, or `reqwest` deps.

#![allow(dead_code, unused_imports, unused_variables, clippy::all)]

use std::collections::HashMap;
use std::env;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// Constants — mirrors ll.42-45
// ---------------------------------------------------------------------------

/// Below this TOTAL spendable ($), a paid account is flagged "low".
/// Mirrors `LOW_BALANCE_THRESHOLD_USD = 5.0` (l.45).
pub const LOW_BALANCE_THRESHOLD_USD: f64 = 5.0;

// ---------------------------------------------------------------------------
// Minimal Value — mirrors `Any` for 1:1 coercion (std-only)
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
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }
}

// ---------------------------------------------------------------------------
// Helpers — mirrors `_finite` (ll.48-53), `_fmt_usd` (ll.56-58), `format_renews` (ll.61-85)
// ---------------------------------------------------------------------------

/// Mirrors `def _finite(value: Any) -> Optional[float]` (ll.48-53).
/// Return value as `f64` iff it's a real finite number (not bool/NaN/Inf).
pub fn finite_value(value: Option<&Value>) -> Option<f64> {
    let v = value?;
    match v {
        Value::Bool(_) => None,
        Value::Int(i) => Some(*i as f64),
        Value::Number(n) if n.is_finite() => Some(*n),
        _ => None,
    }
}

#[allow(dead_code)]
fn _finite(value: Option<&Value>) -> Option<f64> {
    finite_value(value)
}

/// Mirrors the `f64` finite check used when value is already `Option<f64>`.
/// Handles `None`, `NaN`, `Inf` → `None`; rejects non-finite.
pub fn finite_f64(value: Option<f64>) -> Option<f64> {
    match value {
        Some(f) if f.is_finite() => Some(f),
        _ => None,
    }
}

#[allow(dead_code)]
fn _finite_f64(value: Option<f64>) -> Option<f64> {
    finite_f64(value)
}

/// Mirrors `def _fmt_usd(value: Optional[float]) -> str` (ll.56-58).
/// `$X.YY` for display. `None` -> `$0.00`.
pub fn fmt_usd(value: Option<f64>) -> String {
    let v = value.unwrap_or(0.0);
    // Clamp non-finite to 0.0 for display parity (Python would format `nan`/`inf` but we gate on finite elsewhere)
    let v = if v.is_finite() { v } else { 0.0 };
    format_usd(v)
}

#[allow(dead_code)]
fn _fmt_usd(value: Option<f64>) -> String {
    fmt_usd(value)
}

fn format_usd(v: f64) -> String {
    let sign = if v < 0.0 { "-" } else { "" };
    let abs = v.abs();
    // Format to 2 decimals
    let s = format!("{:.2}", abs);
    let parts: Vec<&str> = s.split('.').collect();
    let int_part = parts[0];
    let frac = parts.get(1).copied().unwrap_or("00");
    let mut with_commas = String::new();
    let mut count = 0;
    for c in int_part.chars().rev() {
        if count != 0 && count % 3 == 0 {
            with_commas.push(',');
        }
        with_commas.push(c);
        count += 1;
    }
    let int_comma: String = with_commas.chars().rev().collect();
    format!("{}${}.{}", sign, int_comma, frac)
}

/// Mirrors `def format_renews(value: Optional[str]) -> Optional[str]` (ll.61-85).
/// Format an ISO date/timestamp as `Jul 24, 2026`. Returns raw string if unparsable, `None` for empty.
pub fn format_renews(value: Option<&str>) -> Option<String> {
    let raw = value?;
    let text = raw.trim();
    if text.is_empty() {
        return None;
    }
    // Handle trailing Z -> +00:00 (l.75)
    let iso = if text.ends_with('Z') {
        format!("{}+00:00", &text[..text.len() - 1])
    } else {
        text.to_string()
    };
    // Try datetime.fromisoformat(iso) — mirrors ll.77-78
    if let Some(dt) = parse_iso8601(&iso) {
        return Some(format_ymd(dt.0, dt.1, dt.2));
    }
    if let Some(dt) = parse_iso8601(text) {
        return Some(format_ymd(dt.0, dt.1, dt.2));
    }
    // Fall back to bare date prefix YYYY-MM-DD (ll.79-82)
    if text.len() >= 10 {
        let prefix = &text[..10];
        if let Some((y, m, d)) = parse_ymd(prefix) {
            // Validate via datetime.strptime
            return Some(format_ymd(y, m, d));
        }
    }
    // Return raw text unchanged (never raises) — l.83
    Some(text.to_string())
}

#[allow(dead_code)]
fn _format_renews(value: Option<&str>) -> Option<String> {
    format_renews(value)
}

fn format_ymd(year: i32, month: u32, day: u32) -> String {
    let mon = match month {
        1 => "Jan",
        2 => "Feb",
        3 => "Mar",
        4 => "Apr",
        5 => "May",
        6 => "Jun",
        7 => "Jul",
        8 => "Aug",
        9 => "Sep",
        10 => "Oct",
        11 => "Nov",
        12 => "Dec",
        _ => "Jan",
    };
    // `%-d` not portable — build day without leading zero (l.84-85)
    format!("{} {}, {}", mon, day, year)
}

fn parse_ymd(s: &str) -> Option<(i32, u32, u32)> {
    if s.len() < 10 {
        return None;
    }
    if s.as_bytes()[4] != b'-' || s.as_bytes()[7] != b'-' {
        return None;
    }
    let y: i32 = s[0..4].parse().ok()?;
    let m: u32 = s[5..7].parse().ok()?;
    let d: u32 = s[8..10].parse().ok()?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    Some((y, m, d))
}

fn parse_iso8601(text: &str) -> Option<(i32, u32, u32)> {
    let s = text.trim();
    if s.is_empty() {
        return None;
    }
    // Use manual parser that extracts Y-M-D ignoring time/tz.
    // First try full ISO with T and tz, then fallback to plain date.
    // This mirrors Python's datetime.fromisoformat + strptime fallback.
    // Extract date part before T or space.
    let date_part = if let Some(t_pos) = s.find('T').or_else(|| s.find(' ')) {
        &s[..t_pos]
    } else if s.len() >= 10 {
        &s[..10]
    } else {
        s
    };
    parse_ymd(date_part)
}

// ---------------------------------------------------------------------------
// UsageBar — mirrors `@dataclass(frozen=True) class UsageBar` (ll.88-113)
// ---------------------------------------------------------------------------

/// Mirrors `UsageBar` (ll.88-113).
/// One full-resolution bar: `spent` of `total`, plus a remaining figure.
/// `kind` is `"plan"` (monthly allowance, shows % used) or `"topup"`.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageBar {
    pub kind: String,
    pub remaining_usd: f64,
    pub total_usd: f64,
    pub spent_usd: f64,
}

impl UsageBar {
    pub fn new(kind: impl Into<String>, remaining_usd: f64, total_usd: f64, spent_usd: f64) -> Self {
        Self {
            kind: kind.into(),
            remaining_usd,
            total_usd,
            spent_usd,
        }
    }

    /// Mirrors `pct_used` property (ll.102-106).
    pub fn pct_used(&self) -> Option<i32> {
        if self.kind != "plan" || self.total_usd <= 0.0 {
            return None;
        }
        let raw = self.spent_usd / self.total_usd * 100.0;
        let rounded = raw.round() as i32;
        Some(rounded.clamp(0, 100))
    }

    /// Mirrors `fill_fraction` property (ll.108-113).
    /// Fraction of the bar that should read as 'remaining' (filled).
    pub fn fill_fraction(&self) -> f64 {
        if self.total_usd <= 0.0 {
            return 0.0;
        }
        (self.remaining_usd / self.total_usd).clamp(0.0, 1.0)
    }
}

// ---------------------------------------------------------------------------
// UsageModel — mirrors `@dataclass(frozen=True) class UsageModel` (ll.116-140)
// ---------------------------------------------------------------------------

/// Mirrors `UsageModel` (ll.116-140).
/// Surface-agnostic dollar usage model shared by `/usage` and `/subscription`.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageModel {
    pub available: bool,
    pub status: String,
    pub plan_name: Option<String>,
    pub renews_at: Option<String>,
    pub renews_display: Option<String>,
    pub subscription_remaining_usd: Option<f64>,
    pub topup_remaining_usd: Option<f64>,
    pub total_spendable_usd: Option<f64>,
    pub plan_bar: Option<UsageBar>,
    pub topup_bar: Option<UsageBar>,
}

impl UsageModel {
    pub fn new(available: bool) -> Self {
        Self {
            available,
            status: "free".to_string(),
            plan_name: None,
            renews_at: None,
            renews_display: None,
            subscription_remaining_usd: None,
            topup_remaining_usd: None,
            total_spendable_usd: None,
            plan_bar: None,
            topup_bar: None,
        }
    }

    /// Mirrors `has_topup` property (ll.138-140).
    pub fn has_topup(&self) -> bool {
        match self.topup_remaining_usd {
            Some(v) if v > 0.0 && v.is_finite() => true,
            _ => false,
        }
    }
}

impl Default for UsageModel {
    fn default() -> Self {
        Self {
            available: false,
            status: "free".to_string(),
            plan_name: None,
            renews_at: None,
            renews_display: None,
            subscription_remaining_usd: None,
            topup_remaining_usd: None,
            total_spendable_usd: None,
            plan_bar: None,
            topup_bar: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Account-info structs — mirrors `NousPortalAccountInfo` shape (ll.143-164)
// ---------------------------------------------------------------------------

/// Minimal mirror of `NousPortalAccountInfo` fields read by `usage_model_from_account`.
/// Mirrors `reference/NousResearch/hermes-agent/hermes_cli/nous_account.py` normalized shape.
#[derive(Debug, Clone, Default)]
pub struct PaidServiceAccessInfo {
    pub subscription_credits_remaining: Option<f64>,
    pub purchased_credits_remaining: Option<f64>,
    pub total_usable_credits: Option<f64>,
}

#[derive(Debug, Clone, Default)]
pub struct SubscriptionInfo {
    pub plan: Option<String>,
    pub monthly_credits: Option<f64>,
    pub current_period_end: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct AccountInfo {
    pub logged_in: bool,
    pub paid_service_access_info: Option<PaidServiceAccessInfo>,
    pub subscription: Option<SubscriptionInfo>,
    /// `None` → field absent; `Some(false)` → explicitly false (depleted).
    pub paid_service_access: Option<bool>,
}

// ---------------------------------------------------------------------------
// usage_model_from_account — mirrors `def usage_model_from_account` (ll.143-223)
// ---------------------------------------------------------------------------

/// Mirrors `def usage_model_from_account(account_info: Any) -> UsageModel` (ll.143-223).
/// Build a `UsageModel` from a `NousPortalAccountInfo`. Fail-open — never panics.
pub fn usage_model_from_account(account_info: Option<&AccountInfo>) -> UsageModel {
    // Mirrors try/except with `logger.debug(..., exc_info=True)` → fail-open to available=False (ll.221-223)
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        usage_model_from_account_inner(account_info)
    }));
    match result {
        Ok(m) => m,
        Err(_) => UsageModel { available: false, ..Default::default() },
    }
}

fn usage_model_from_account_inner(account_info: Option<&AccountInfo>) -> UsageModel {
    // l.150-151: if account_info is None or not logged_in → available=False
    let account_info = match account_info {
        Some(a) if a.logged_in => a,
        _ => return UsageModel { available: false, ..Default::default() },
    };

    let access = account_info.paid_service_access_info.as_ref();
    let sub = account_info.subscription.as_ref();
    let paid = account_info.paid_service_access;

    // ll.157-159: _finite(getattr(access, "subscription_credits_remaining", None)) if access else None
    let sub_remaining = access
        .and_then(|a| finite_f64(a.subscription_credits_remaining));
    let topup_remaining = access
        .and_then(|a| finite_f64(a.purchased_credits_remaining));
    let total_usable = access
        .and_then(|a| finite_f64(a.total_usable_credits));

    // ll.161-163: plan_name, renews_at, monthly
    let plan_name = sub.and_then(|s| s.plan.clone()).filter(|s| !s.trim().is_empty());
    let renews_at = sub.and_then(|s| s.current_period_end.clone()).filter(|s| !s.trim().is_empty());
    let monthly = sub.and_then(|s| finite_f64(s.monthly_credits));

    // l.165: has_subscription = bool(plan_name) or (monthly is not None and monthly > 0)
    let has_subscription = plan_name.is_some() || matches!(monthly, Some(m) if m > 0.0);

    // ll.167-172: Total spendable — prefer server's total; else sum parts
    let total_spendable = if let Some(t) = total_usable {
        Some(t)
    } else {
        let mut parts: Vec<f64> = Vec::new();
        if let Some(v) = sub_remaining { parts.push(v); }
        if let Some(v) = topup_remaining { parts.push(v); }
        if parts.is_empty() { None } else { Some(parts.iter().sum()) }
    };

    // ll.174-183: Status classification
    let status = if paid == Some(false) {
        "depleted".to_string()
    } else if !has_subscription && !matches!(topup_remaining, Some(v) if v > 0.0) {
        "free".to_string()
    } else if let Some(total) = total_spendable {
        if total < LOW_BALANCE_THRESHOLD_USD {
            "low".to_string()
        } else {
            "healthy".to_string()
        }
    } else {
        "healthy".to_string()
    };

    // ll.185-196: Plan bar — only with positive monthly AND remaining
    let plan_bar = if let (Some(monthly_val), Some(sub_rem)) = (monthly, sub_remaining) {
        if monthly_val > 0.0 {
            let remaining = sub_rem.clamp(0.0, monthly_val).max(0.0);
            // Python: spent = max(0.0, monthly - sub_remaining) — note uses original sub_remaining, not clamped remaining
            let spent = (monthly_val - sub_rem).max(0.0);
            Some(UsageBar {
                kind: "plan".to_string(),
                remaining_usd: remaining,
                total_usd: monthly_val,
                spent_usd: spent,
            })
        } else {
            None
        }
    } else {
        None
    };

    // ll.198-207: Top-up bar — only when purchased dollars > 0
    let topup_bar = if let Some(topup) = topup_remaining {
        if topup > 0.0 {
            Some(UsageBar {
                kind: "topup".to_string(),
                remaining_usd: topup,
                total_usd: topup,
                spent_usd: 0.0,
            })
        } else {
            None
        }
    } else {
        None
    };

    // ll.209-220
    let renews_display = format_renews(renews_at.as_deref());

    UsageModel {
        available: true,
        status,
        plan_name,
        renews_at,
        renews_display,
        subscription_remaining_usd: sub_remaining,
        topup_remaining_usd: topup_remaining,
        total_spendable_usd: total_spendable,
        plan_bar,
        topup_bar,
    }
}

#[allow(dead_code)]
fn _usage_model_from_account(account_info: Option<&AccountInfo>) -> UsageModel {
    usage_model_from_account(account_info)
}

// ---------------------------------------------------------------------------
// build_usage_model + dev fixtures — mirrors ll.226-323
// ---------------------------------------------------------------------------

/// Mirrors `def build_usage_model(*, timeout: float = 10.0) -> UsageModel` (ll.226-256).
/// Fetch account-info and build the shared usage model. Fail-open.
pub fn build_usage_model(timeout_secs: f64) -> UsageModel {
    // l.233-235: fixture short-circuit
    if let Some(fixture) = dev_fixture_usage_model() {
        return fixture;
    }

    // ll.237-244: try get_provider_auth_state("nous")
    let token = match get_provider_auth_state_token() {
        Some(t) if !t.trim().is_empty() => t,
        _ => return UsageModel { available: false, ..Default::default() },
    };
    let _ = token; // keep parity — token presence gates portal fetch

    // ll.246-253: ThreadPoolExecutor(max_workers=1) with timeout
    let account = fetch_account_with_timeout(timeout_secs);
    match account {
        Some(acct) => usage_model_from_account(acct.as_ref()),
        None => {
            // l.255: logger.debug fail-open
            UsageModel { available: false, ..Default::default() }
        }
    }
}

#[allow(dead_code)]
fn _build_usage_model(timeout_secs: f64) -> UsageModel {
    build_usage_model(timeout_secs)
}

fn get_provider_auth_state_token() -> Option<String> {
    // Mirrors `hermes_cli.auth.get_provider_auth_state("nous")` → `access_token`
    // Std-only stub: env vars that the real store would populate.
    for key in ["NOUS_ACCESS_TOKEN", "NOUS_API_KEY", "HERMES_NOUS_TOKEN"] {
        if let Ok(v) = env::var(key) {
            let t = v.trim().to_string();
            if !t.is_empty() {
                return Some(t);
            }
        }
    }
    // Also try HERMES_HOME/auth.json if present (best-effort)
    if let Ok(home) = env::var("HERMES_HOME") {
        let p = std::path::Path::new(&home).join("auth.json");
        if let Ok(text) = std::fs::read_to_string(&p) {
            if let Some(tok) = extract_json_string(&text, "access_token") {
                if !tok.trim().is_empty() {
                    return Some(tok);
                }
            }
        }
    }
    None
}

fn fetch_account_with_timeout(timeout_secs: f64) -> Option<Option<AccountInfo>> {
    // Mirrors `ThreadPoolExecutor(max_workers=1).submit(get_nous_portal_account_info, force_fresh=True).result(timeout=timeout)`
    let timeout = Duration::from_secs_f64(timeout_secs.max(0.1).min(60.0));
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let res = get_nous_portal_account_info(true);
        let _ = tx.send(res);
    });
    match rx.recv_timeout(timeout) {
        Ok(v) => Some(v),
        Err(_) => None,
    }
}

/// Stub: mirrors `hermes_cli.nous_account.get_nous_portal_account_info(force_fresh=True)` (l.249).
/// Real impl does a portal HTTP fetch; this stub returns `None` (fail-open).
/// Tests call `usage_model_from_account` directly with a constructed `AccountInfo`.
pub fn get_nous_portal_account_info(_force_fresh: bool) -> Option<AccountInfo> {
    // Would do bearer-auth GET to portal; std-only slice has no network.
    None
}

// ---------------------------------------------------------------------------
// Dev fixtures — mirrors `def _dev_fixture_usage_model` (ll.264-323)
// ---------------------------------------------------------------------------

/// Mirrors `def _dev_fixture_usage_model() -> Optional[UsageModel]` (ll.264-323).
pub fn dev_fixture_usage_model() -> Option<UsageModel> {
    let name = env::var("HERMES_DEV_CREDITS_FIXTURE")
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    if name.is_empty() {
        return None;
    }

    if name == "free" {
        return Some(UsageModel {
            available: true,
            status: "free".to_string(),
            plan_name: None,
            ..Default::default()
        });
    }

    if name == "healthy" || name == "mid" {
        return Some(UsageModel {
            available: true,
            status: "healthy".to_string(),
            plan_name: Some("Plus".to_string()),
            renews_at: Some("2026-07-01".to_string()),
            renews_display: format_renews(Some("2026-07-01")),
            subscription_remaining_usd: Some(14.0),
            total_spendable_usd: Some(14.0),
            plan_bar: Some(UsageBar {
                kind: "plan".to_string(),
                remaining_usd: 14.0,
                total_usd: 20.0,
                spent_usd: 6.0,
            }),
            ..Default::default()
        });
    }

    if name == "topup" || name == "top-up" {
        return Some(UsageModel {
            available: true,
            status: "healthy".to_string(),
            plan_name: Some("Plus".to_string()),
            renews_at: Some("2026-07-01".to_string()),
            renews_display: format_renews(Some("2026-07-01")),
            subscription_remaining_usd: Some(14.0),
            topup_remaining_usd: Some(12.0),
            total_spendable_usd: Some(26.0),
            plan_bar: Some(UsageBar {
                kind: "plan".to_string(),
                remaining_usd: 14.0,
                total_usd: 20.0,
                spent_usd: 6.0,
            }),
            topup_bar: Some(UsageBar {
                kind: "topup".to_string(),
                remaining_usd: 12.0,
                total_usd: 12.0,
                spent_usd: 0.0,
            }),
        });
    }

    if name == "low" {
        return Some(UsageModel {
            available: true,
            status: "low".to_string(),
            plan_name: Some("Plus".to_string()),
            renews_at: Some("2026-07-01".to_string()),
            renews_display: format_renews(Some("2026-07-01")),
            subscription_remaining_usd: Some(3.4),
            total_spendable_usd: Some(3.4),
            plan_bar: Some(UsageBar {
                kind: "plan".to_string(),
                remaining_usd: 3.4,
                total_usd: 20.0,
                spent_usd: 16.6,
            }),
            ..Default::default()
        });
    }

    if name == "depleted" {
        return Some(UsageModel {
            available: true,
            status: "depleted".to_string(),
            plan_name: Some("Plus".to_string()),
            renews_at: Some("2026-07-01".to_string()),
            renews_display: format_renews(Some("2026-07-01")),
            subscription_remaining_usd: Some(0.0),
            total_spendable_usd: Some(0.0),
            plan_bar: Some(UsageBar {
                kind: "plan".to_string(),
                remaining_usd: 0.0,
                total_usd: 20.0,
                spent_usd: 20.0,
            }),
            ..Default::default()
        });
    }

    // Unknown fixture name → server returns None via real path? For parity, return None (original returns None)
    // But Python's `_dev_fixture_usage_model` returns None for unknown names (l.323).
    // Keep that — caller will proceed to real portal path which fail-opens to available=False.
    // However our stub already short-circuits only on Some, so returning None is correct.
    None
}

#[allow(dead_code)]
fn _dev_fixture_usage_model() -> Option<UsageModel> {
    dev_fixture_usage_model()
}

// ---------------------------------------------------------------------------
// Small JSON helpers — mirrors `response.json()` extraction for token stub
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
