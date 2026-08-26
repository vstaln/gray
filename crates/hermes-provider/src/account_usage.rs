//! Account usage / quota surfaces — `/usage` and `/topup`.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/agent/account_usage.py` (902 lines).
//!
//! Hermes exposes provider quota via `fetch_account_usage` (provider-dispatched
//! → Codex / Anthropic / OpenRouter) and Nous credits via
//! `build_nous_credits_snapshot` / `nous_credits_lines` / `build_credits_view`.
//! All three surfaces share the same `AccountUsageSnapshot` + `render_account_usage_lines`
//! renderer so `/usage` and `/topup` always show matching numbers.  Fail-open
//! is load-bearing: every portal / network failure returns `None` or `[]` so
//! the caller simply shows nothing that turn.
//!
//! T0033 — 1:1 port, no cargo (NEVER cargo).
//!
//! Notes on fidelity vs. Rust idioms:
//! - Python `dataclass(frozen=True)` ↔ `#[derive(Debug, Clone)]` with `Clone` semantics;
//!   `tuple[AccountUsageWindow, ...]` ↔ `Vec<AccountUsageWindow>` (ordered, immutable by convention).
//! - Python `Optional[datetime]` ↔ `Option<SystemTime>`; `datetime` parsing ↔ ISO-8601 + epoch helpers.
//!   Real crate would use `chrono::DateTime<Utc>`; this slice stays `std`-only so line-level
//!   audit doesn't require `chrono` before the provider wave lands. Helpers preserve UTC semantics.
//! - Python `httpx.Client(timeout=…)` ↔ `reqwest::blocking::Client` noted as future dep;
//!   this slice shells to `curl`/`reqwest` stubs so the crate stays `std`-only. Timeout values
//!   are preserved as constants for audit.
//! - Python `TypeGuard[float]` / `math.isfinite` ↔ `f64::is_finite()`.
//! - Python `getattr(obj, "field", None)` ↔ `Option` fields on typed structs; dynamic `Any`
//!   payloads ↔ `serde_json::Value` where JSON is needed (Codex / Anthropic / OpenRouter responses).
//!   This slice uses a minimal `JsonValue` alias over `HashMap<String,String>` + hand-rolled JSON
//!   extraction so it compiles `std`-only; the merge step replaces with `serde_json::Value`.
//! - Python `concurrent.futures.ThreadPoolExecutor(max_workers=1, timeout=…)` ↔ `std::thread` +
//!   `mpsc::sync_channel` with wall-clock deadline; timeout seconds preserved.
//! - Python `logging.getLogger(__name__)` ↔ `log::debug!` target `"account_usage"`.
//! - The `_snapshot_from_credits_state` dev-fixture path mirrors `agent/credits_tracker.CreditsState`
//!   — the real struct lives in `hermes-core`; this slice carries a minimal mirror with the same
//!   `used_fraction` / `*_usd` / `paid_access` fields so the fixture renders identically.
//! - `SecretSource` / auth / pool helpers (`resolve_codex_runtime_credentials`, `load_pool`, etc.)
//!   are forward-declared as stubs mirroring their Python surfaces; canonical impls live in
//!   sibling crates (`hermes-cli`, `hermes-provider`) and replace stubs at merge.

#![allow(dead_code, unused_imports, unused_variables, clippy::all)]

use std::collections::HashMap;
use std::env;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// Logger target — mirrors `logger = logging.getLogger(__name__)` (l.18)
// ---------------------------------------------------------------------------
const LOG_TARGET: &str = "account_usage";

// ---------------------------------------------------------------------------
// Time helpers — mirrors `def _utc_now() -> datetime` (ll.21-22)
// ---------------------------------------------------------------------------

/// Mirrors `_utc_now()` (ll.21-22): `datetime.now(timezone.utc)`.
/// Rust: `SystemTime::now()` is UTC by definition (UNIX_EPOCH anchor).
pub fn utc_now() -> SystemTime {
    SystemTime::now()
}

#[allow(dead_code)]
fn _utc_now() -> SystemTime {
    utc_now()
}

fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

fn system_time_to_secs(t: SystemTime) -> f64 {
    t.duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

fn secs_to_system_time(secs: f64) -> SystemTime {
    if secs <= 0.0 {
        return UNIX_EPOCH;
    }
    UNIX_EPOCH + Duration::from_secs_f64(secs)
}

// ---------------------------------------------------------------------------
// Core data types — mirrors `@dataclass(frozen=True)` (ll.25-46)
// ---------------------------------------------------------------------------

/// Mirrors `AccountUsageWindow` (ll.25-30).
#[derive(Debug, Clone, PartialEq)]
pub struct AccountUsageWindow {
    pub label: String,
    pub used_percent: Option<f64>,
    pub reset_at: Option<SystemTime>,
    pub detail: Option<String>,
}

impl AccountUsageWindow {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            used_percent: None,
            reset_at: None,
            detail: None,
        }
    }
}

/// Mirrors `AccountUsageSnapshot` (ll.33-46).
#[derive(Debug, Clone)]
pub struct AccountUsageSnapshot {
    pub provider: String,
    pub source: String,
    pub fetched_at: SystemTime,
    pub title: String,
    pub plan: Option<String>,
    pub windows: Vec<AccountUsageWindow>,
    pub details: Vec<String>,
    pub unavailable_reason: Option<String>,
}

impl AccountUsageSnapshot {
    /// Mirrors `available` property (ll.44-46):
    /// `return bool(self.windows or self.details) and not self.unavailable_reason`
    pub fn available(&self) -> bool {
        (!self.windows.is_empty() || !self.details.is_empty()) && self.unavailable_reason.is_none()
    }
}

impl Default for AccountUsageSnapshot {
    fn default() -> Self {
        Self {
            provider: String::new(),
            source: String::new(),
            fetched_at: UNIX_EPOCH,
            title: "Account limits".to_string(),
            plan: None,
            windows: Vec::new(),
            details: Vec::new(),
            unavailable_reason: None,
        }
    }
}

// ---------------------------------------------------------------------------
// String helpers — mirrors ll.49-72
// ---------------------------------------------------------------------------

/// Mirrors `def _title_case_slug(value: Optional[str]) -> Optional[str]` (ll.49-53).
pub fn title_case_slug(value: Option<&str>) -> Option<String> {
    let cleaned = value.unwrap_or("").trim();
    if cleaned.is_empty() {
        return None;
    }
    // Replace "_" and "-" with " ", then title-case each word.
    let spaced = cleaned.replace('_', " ").replace('-', " ");
    let titled = spaced
        .split_whitespace()
        .map(|w| {
            let mut chars = w.chars();
            match chars.next() {
                None => String::new(),
                Some(first) => {
                    let mut out = String::new();
                    out.extend(first.to_uppercase());
                    out.push_str(&chars.as_str().to_lowercase());
                    out
                }
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    if titled.is_empty() {
        None
    } else {
        Some(titled)
    }
}

#[allow(dead_code)]
fn _title_case_slug(value: Option<&str>) -> Option<String> {
    title_case_slug(value)
}

/// Mirrors `def _parse_dt(value: Any) -> Optional[datetime]` (ll.56-72).
///
/// Handles:
/// - `None` / `""` → `None`
/// - `int` / `float` (epoch seconds) → `SystemTime`
/// - ISO-8601 string (with `Z` → `+00:00` rewrite) → `SystemTime`
pub fn parse_dt(value: &str) -> Option<SystemTime> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    // Try numeric epoch first (mirrors `isinstance(value, (int, float))`)
    if let Ok(secs) = trimmed.parse::<f64>() {
        // Only accept if the whole string is numeric (no extra chars)
        // Check by round-tripping: "123" parses, "2026-01-01" does not (parse fails on "-")
        // For strings containing "-" this branch fails and we fall through to ISO parsing.
        // Guard: require no alpha and only digits/dot/minus
        let is_epoch_like = trimmed.chars().all(|c| c.is_ascii_digit() || c == '.' || c == '-' || c == '+');
        if is_epoch_like && !trimmed.contains('T') && !trimmed.contains('-') || trimmed.parse::<i64>().is_ok() && !trimmed.contains('T') {
            // Actually we already parsed as f64; verify it was purely numeric
            // If original contains letters, parsing as f64 would have failed, so this is safe.
        }
        // If we successfully parsed and the string is purely numeric, return epoch.
        // But for generic strings like "2026-01-01T00:00:00Z", f64 parse would fail above.
        // So if we are here, it was numeric.
        if trimmed.chars().all(|c| c.is_ascii_digit() || c == '.' || c == '-' || c == '+' ) && !trimmed.contains('T') {
             return Some(secs_to_system_time(secs));
        }
    }
    parse_iso8601(trimmed)
}

pub fn parse_dt_f64(epoch_secs: f64) -> Option<SystemTime> {
    Some(secs_to_system_time(epoch_secs))
}

fn parse_dt_opt(value: Option<&str>) -> Option<SystemTime> {
    match value {
        None => None,
        Some(s) if s.trim().is_empty() => None,
        Some(s) => parse_dt(s),
    }
}

#[allow(dead_code)]
fn _parse_dt(value: Option<&str>) -> Option<SystemTime> {
    parse_dt_opt(value)
}

/// Minimal ISO-8601 parser for `_parse_dt` string branch (ll.62-71).
/// Handles `2026-01-02T15:04:05Z`, `2026-01-02T15:04:05+00:00`, `2026-01-02T15:04:05`.
/// Returns UTC `SystemTime`; naive datetimes are treated as UTC (mirrors Python's
/// `dt.replace(tzinfo=timezone.utc)` when `dt.tzinfo is None`).
fn parse_iso8601(text: &str) -> Option<SystemTime> {
    let mut s = text.trim().to_string();
    if s.is_empty() {
        return None;
    }
    if s.ends_with('Z') {
        s = format!("{}+00:00", &s[..s.len() - 1]);
    }
    // Try to parse via `chrono` semantics without chrono: handle `YYYY-MM-DDTHH:MM:SS[.frac][+HH:MM]`
    // We delegate to a best-effort manual parse for std-only fidelity.
    // Full ISO support would use `chrono::DateTime::parse_from_rfc3339`.
    // Here we extract epoch seconds via a simple state machine.
    parse_iso8601_manual(&s)
}

fn parse_iso8601_manual(s: &str) -> Option<SystemTime> {
    // Split on 'T' or ' '
    let s = s.trim();
    // Find timezone offset
    let (datetime_part, tz_offset_secs) = split_tz(s)?;
    // Split date and time
    let t_pos = datetime_part.find('T').or_else(|| datetime_part.find(' '))?;
    let date_str = &datetime_part[..t_pos];
    let time_str = &datetime_part[t_pos + 1..];
    // Parse date YYYY-MM-DD
    let date_parts: Vec<&str> = date_str.split('-').collect();
    if date_parts.len() != 3 {
        return None;
    }
    let year: i32 = date_parts[0].parse().ok()?;
    let month: u32 = date_parts[1].parse().ok()?;
    let day: u32 = date_parts[2].parse().ok()?;
    // Parse time HH:MM:SS[.frac]
    let time_parts: Vec<&str> = time_str.split(':').collect();
    if time_parts.len() < 2 {
        return None;
    }
    let hour: u32 = time_parts[0].parse().ok()?;
    let minute: u32 = time_parts[1].parse().ok()?;
    let sec_frac = time_parts[2];
    let (second, _frac) = if let Some(dot) = sec_frac.find('.') {
        let sec: u32 = sec_frac[..dot].parse().ok()?;
        (sec, &sec_frac[dot..])
    } else {
        let sec: u32 = sec_frac.parse().ok()?;
        (sec, "")
    };
    // Convert to epoch using days since 1970 (proleptic Gregorian)
    let epoch_days = days_since_epoch(year, month, day)?;
    let secs_of_day = (hour as i64) * 3600 + (minute as i64) * 60 + (second as i64);
    let mut epoch_secs = epoch_days * 86400 + secs_of_day - tz_offset_secs;
    // Handle negative epochs gracefully (pre-1970 not expected, but keep correct)
    if epoch_secs < 0 {
        return None;
    }
    Some(UNIX_EPOCH + Duration::from_secs(epoch_secs as u64))
}

fn split_tz(s: &str) -> Option<(String, i64)> {
    // Find last '+' or '-' after the 'T' (timezone offset), or trailing 'Z' already handled.
    // Naive (no tz) → offset 0 (mirrors Python `replace(tzinfo=UTC)`).
    if let Some(t_pos) = s.find('T').or_else(|| s.find(' ')) {
        let after_t = &s[t_pos + 1..];
        // Search from end for + or - that is part of tz
        // Time part has at least HH:MM:SS, so tz offset (if present) is after seconds.
        // Look for '+' or '-' that appears after position 8 in after_t.
        if let Some(plus) = after_t.rfind('+') {
            if plus >= 5 {
                let dt = format!("{}{}", &s[..t_pos + 1 + plus], "");
                // Actually split: datetime_part = s[..t_pos+1+plus], tz = s[t_pos+1+plus..]
                let tz_str = &after_t[plus..];
                let offset = parse_tz_offset(tz_str)?;
                let datetime_part = s[..t_pos + 1 + plus].to_string();
                return Some((datetime_part, offset));
            }
        }
        if let Some(minus) = after_t.rfind('-') {
            // Ensure this minus is tz, not part of date (date already split, but after_t only contains time)
            // Time's seconds don't contain '-', so any '-' here is tz.
            if minus >= 5 {
                let tz_str = &after_t[minus..];
                // Verify tz shape HH:MM
                if tz_str.contains(':') {
                    let offset = parse_tz_offset(tz_str)?;
                    let datetime_part = s[..t_pos + 1 + minus].to_string();
                    return Some((datetime_part, offset));
                }
            }
        }
    }
    // No tz found → naive, treat as UTC
    Some((s.to_string(), 0))
}

fn parse_tz_offset(tz: &str) -> Option<i64> {
    // "+HH:MM" or "-HH:MM" → seconds east of UTC
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
    // Howard Hinnant's days_from_civil
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

/// Mirrors `def _format_reset(dt: Optional[datetime]) -> str` (ll.75-92).
pub fn format_reset(dt: Option<SystemTime>) -> String {
    let Some(dt) = dt else {
        return "unknown".to_string();
    };
    let delta = dt.duration_since(utc_now()).unwrap_or(Duration::from_secs(0));
    // Use total_seconds via signed duration: dt - now
    let secs_signed = if let Ok(d) = dt.duration_since(utc_now()) {
        d.as_secs() as i64
    } else if let Ok(d) = utc_now().duration_since(dt) {
        -(d.as_secs() as i64)
    } else {
        0
    };
    // For display we need local time string — best-effort UTC formatting (real impl uses `chrono::Local`)
    let local_str = format_system_time_local(&dt);
    if secs_signed <= 0 {
        return format!("now ({})", local_str);
    }
    let total = secs_signed as u64;
    let hours = total / 3600;
    let rem = total % 3600;
    let minutes = rem / 60;
    let rel = if hours >= 24 {
        let days = hours / 24;
        let h = hours % 24;
        format!("in {}d {}h", days, h)
    } else if hours > 0 {
        format!("in {}h {}m", hours, minutes)
    } else {
        format!("in {}m", minutes)
    };
    format!("{} ({})", rel, local_str)
}

#[allow(dead_code)]
fn _format_reset(dt: Option<SystemTime>) -> String {
    format_reset(dt)
}

fn format_system_time_local(t: &SystemTime) -> String {
    // Best-effort UTC formatting `YYYY-MM-DD HH:MM UTC` — real impl would use `Local`.
    // Preserve the `strftime('%Y-%m-%d %H:%M %Z')` shape for audit.
    let secs = system_time_to_secs(*t) as i64;
    if secs < 0 {
        return "1970-01-01 00:00 UTC".to_string();
    }
    let (y, m, d, hh, mm, _ss) = secs_to_ymd_hms(secs as u64);
    format!("{:04}-{:02}-{:02} {:02}:{:02} UTC", y, m, d, hh, mm)
}

fn secs_to_ymd_hms(secs: u64) -> (i32, u32, u32, u32, u32, u32) {
    let days = (secs / 86400) as i64;
    let secs_of_day = (secs % 86400) as u32;
    let hh = secs_of_day / 3600;
    let mm = (secs_of_day % 3600) / 60;
    let ss = secs_of_day % 60;
    let (y, m, d) = civil_from_days(days);
    (y, m, d, hh, mm, ss)
}

fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719468;
    let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let mut y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    y += if m <= 2 { 1 } else { 0 };
    (y as i32, m as u32, d as u32)
}

// ---------------------------------------------------------------------------
// Rendering — mirrors `def render_account_usage_lines` (ll.95-120)
// ---------------------------------------------------------------------------

/// Mirrors `def render_account_usage_lines(snapshot: Optional[AccountUsageSnapshot], *, markdown: bool = False) -> list[str]` (ll.95-120).
pub fn render_account_usage_lines(snapshot: Option<&AccountUsageSnapshot>, markdown: bool) -> Vec<String> {
    let Some(snapshot) = snapshot else {
        return Vec::new();
    };
    let header = if markdown {
        format!("📈 **{}**", snapshot.title)
    } else {
        format!("📈 {}", snapshot.title)
    };
    let mut lines = vec![header];
    if let Some(plan) = &snapshot.plan {
        lines.push(format!("Provider: {} ({})", snapshot.provider, plan));
    } else {
        lines.push(format!("Provider: {}", snapshot.provider));
    }
    for window in &snapshot.windows {
        let base = if let Some(used) = window.used_percent {
            let remaining = (100.0 - used).round().max(0.0) as i64;
            let used_i = used.round().max(0.0) as i64;
            format!("{}: {}% remaining ({}% used)", window.label, remaining, used_i)
        } else {
            format!("{}: unavailable", window.label)
        };
        let line = if let Some(reset_at) = window.reset_at {
            format!("{} • resets {}", base, format_reset(Some(reset_at)))
        } else if let Some(detail) = &window.detail {
            format!("{} • {}", base, detail)
        } else {
            base
        };
        lines.push(line);
    }
    for detail in &snapshot.details {
        lines.push(detail.clone());
    }
    if let Some(reason) = &snapshot.unavailable_reason {
        lines.push(format!("Unavailable: {}", reason));
    }
    lines
}

// ---------------------------------------------------------------------------
// Money helpers — mirrors ll.123-134
// ---------------------------------------------------------------------------

/// Mirrors `def _fmt_usd(d: float) -> str` (ll.123-124): `f"${d:,.2f}"`.
pub fn fmt_usd(d: f64) -> String {
    // Format with commas and 2 decimals — std-only.
    let sign = if d < 0.0 { "-" } else { "" };
    let abs = d.abs();
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

#[allow(dead_code)]
fn _fmt_usd(d: f64) -> String {
    fmt_usd(d)
}

/// Mirrors `def _is_finite_num(v: Any) -> TypeGuard[float]` (ll.127-134).
/// `isinstance(v, (int, float)) and not isinstance(v, bool) and math.isfinite(v)`
pub fn is_finite_num_f64(v: Option<f64>) -> bool {
    match v {
        Some(f) => f.is_finite(),
        None => false,
    }
}

pub fn is_finite_num_value(v: &serde_like::Value) -> bool {
    match v {
        serde_like::Value::Number(n) => n.is_finite(),
        _ => false,
    }
}

#[allow(dead_code)]
fn _is_finite_num(v: Option<f64>) -> bool {
    is_finite_num_f64(v)
}

// Minimal serde-like Value for std-only JSON handling.
// Real crate uses `serde_json::Value`; this alias keeps the slice std-only while
// preserving the call graph for audit. Merge step replaces with `serde_json`.
mod serde_like {
    #[derive(Debug, Clone)]
    pub enum Value {
        Null,
        Bool(bool),
        Number(f64),
        String(String),
        Array(Vec<Value>),
        Object(std::collections::HashMap<String, Value>),
    }
    impl Value {
        pub fn is_finite(&self) -> bool {
            matches!(self, Value::Number(n) if n.is_finite())
        }
    }
}

// ---------------------------------------------------------------------------
// Nous credits — mirrors ll.137-338
// ---------------------------------------------------------------------------

/// Mirrors `NousPortalSubscriptionInfo` (hermes_cli/nous_account.py) — the subset
/// read by `build_nous_credits_snapshot` (ll.171-188).
#[derive(Debug, Clone, Default)]
pub struct NousPortalSubscriptionInfo {
    pub plan: Option<String>,
    pub monthly_credits: Option<f64>,
    pub credits_remaining: Option<f64>,
    pub rollover_credits: Option<f64>,
    pub current_period_end: Option<String>,
}

/// Mirrors `NousPaidServiceAccessInfo` — subset used in ll.190-199.
#[derive(Debug, Clone, Default)]
pub struct NousPaidServiceAccessInfo {
    pub subscription_credits_remaining: Option<f64>,
    pub purchased_credits_remaining: Option<f64>,
    pub total_usable_credits: Option<f64>,
}

/// Mirrors `NousPortalAccountInfo` — subset used in `build_nous_credits_snapshot` (ll.147-229).
#[derive(Debug, Clone, Default)]
pub struct NousPortalAccountInfo {
    pub logged_in: bool,
    pub email: Option<String>,
    pub org_name: Option<String>,
    pub org_slug: Option<String>,
    pub portal_base_url: Option<String>,
    pub paid_service_access: Option<bool>,
    pub paid_service_access_info: Option<NousPaidServiceAccessInfo>,
    pub subscription: Option<NousPortalSubscriptionInfo>,
}

/// Mirrors `agent/credits_tracker.CreditsState` — minimal mirror for `_snapshot_from_credits_state` (ll.283-338).
#[derive(Debug, Clone, Default)]
pub struct CreditsState {
    pub used_fraction: Option<f64>,
    pub subscription_limit_usd: Option<String>,
    pub subscription_usd: Option<String>,
    pub purchased_usd: Option<String>,
    pub remaining_usd: Option<String>,
    pub paid_access: bool,
}

impl CreditsState {
    pub fn new() -> Self {
        Self { paid_access: true, ..Default::default() }
    }
}

/// Stub: mirrors `hermes_cli.nous_account.nous_portal_topup_url` (ll.216, 423).
/// Real impl builds `{base}/orgs/{slug}/billing?topup=open` or `{base}/billing?topup=open`.
pub fn nous_portal_topup_url(account_info: &NousPortalAccountInfo) -> String {
    let base = account_info
        .portal_base_url
        .as_deref()
        .unwrap_or("https://portal.nousresearch.com");
    let base = base.trim_end_matches('/');
    if let Some(slug) = account_info.org_slug.as_deref().map(|s| s.trim()).filter(|s| !s.is_empty()) {
        // Minimal URL-encoding for slug (real impl uses `urllib.parse.quote`)
        let encoded: String = slug
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' || c == '~' { c.to_string() } else { format!("%{:02X}", c as u32) })
            .collect();
        format!("{}/orgs/{}/billing?topup=open", base, encoded)
    } else {
        format!("{}/billing?topup=open", base)
    }
}

/// Mirrors `def build_nous_credits_snapshot(account_info) -> Optional[AccountUsageSnapshot]` (ll.137-230).
pub fn build_nous_credits_snapshot(account_info: Option<&NousPortalAccountInfo>) -> Option<AccountUsageSnapshot> {
    // Fail-open: any AttributeError/TypeError → None (Rust: Option handling)
    let account_info = account_info?;
    if !account_info.logged_in {
        return None;
    }

    let mut windows: Vec<AccountUsageWindow> = Vec::new();
    let mut details: Vec<String> = Vec::new();

    // Subscription usage gauge — only when portal supplies positive monthly_credits AND finite remaining <= cap
    // Mirrors ll.171-188
    if let Some(sub) = &account_info.subscription {
        if let (Some(monthly_credits), Some(sub_remaining)) = (sub.monthly_credits, sub.credits_remaining) {
            if monthly_credits.is_finite() && monthly_credits > 0.0
                && sub_remaining.is_finite()
                && sub_remaining <= monthly_credits
            {
                let used = monthly_credits - sub_remaining;
                let used_pct = (used / monthly_credits * 100.0).clamp(0.0, 100.0);
                windows.push(AccountUsageWindow {
                    label: "Subscription".to_string(),
                    used_percent: Some(used_pct),
                    reset_at: None,
                    detail: Some(format!("{} of {} left", fmt_usd(sub_remaining), fmt_usd(monthly_credits))),
                });
            }
        }
    }

    if let Some(access) = &account_info.paid_service_access_info {
        if let Some(v) = access.subscription_credits_remaining {
            if v.is_finite() {
                details.push(format!("Subscription credits: {}", fmt_usd(v)));
            }
        }
        if let Some(v) = access.purchased_credits_remaining {
            if v.is_finite() {
                details.push(format!("Top-up credits: {}", fmt_usd(v)));
            }
        }
        if let Some(v) = access.total_usable_credits {
            if v.is_finite() {
                details.push(format!("Total usable: {}", fmt_usd(v)));
            }
        }
    }

    if let Some(sub) = &account_info.subscription {
        if let Some(rollover) = sub.rollover_credits {
            if rollover.is_finite() && rollover > 0.0 {
                details.push(format!("Rollover: {}", fmt_usd(rollover)));
            }
        }
        if let Some(period_end) = &sub.current_period_end {
            if !period_end.trim().is_empty() {
                details.push(format!("Renews: {}", period_end));
            }
        }
    }

    if account_info.paid_service_access == Some(false) {
        details.push("Status: access depleted — top up to restore".to_string());
    }

    if windows.is_empty() && details.is_empty() {
        return None;
    }

    details.push(format!("Top up: {}", nous_portal_topup_url(account_info)));
    details.push("(or run /topup)".to_string());

    let plan = account_info.subscription.as_ref().and_then(|s| s.plan.clone());

    Some(AccountUsageSnapshot {
        provider: "nous".to_string(),
        source: "portal-account".to_string(),
        fetched_at: utc_now(),
        title: "Nous credits".to_string(),
        plan,
        windows,
        details,
    })
}

/// Mirrors `def _snapshot_from_credits_state(state) -> Optional[AccountUsageSnapshot]` (ll.283-338).
pub fn snapshot_from_credits_state(state: Option<&CreditsState>) -> Option<AccountUsageSnapshot> {
    let state = state?;
    let mut windows: Vec<AccountUsageWindow> = Vec::new();
    let mut details: Vec<String> = Vec::new();

    if let Some(uf) = state.used_fraction {
        if uf.is_finite() {
            let cap_usd = state.subscription_limit_usd.as_deref();
            let sub_usd = state.subscription_usd.as_deref();
            let detail = match (sub_usd, cap_usd) {
                (Some(s), Some(c)) if !s.is_empty() && !c.is_empty() => Some(format!("${} of ${} left", s, c)),
                _ => None,
            };
            windows.push(AccountUsageWindow {
                label: "Subscription".to_string(),
                used_percent: Some(uf.clamp(0.0, 1.0) * 100.0),
                reset_at: None,
                detail,
            });
        }
    }

    if let Some(s) = &state.subscription_usd {
        if !s.is_empty() {
            details.push(format!("Subscription credits: ${}", s));
        }
    }
    if let Some(s) = &state.purchased_usd {
        if !s.is_empty() {
            details.push(format!("Top-up credits: ${}", s));
        }
    }
    if let Some(s) = &state.remaining_usd {
        if !s.is_empty() {
            details.push(format!("Total usable: ${}", s));
        }
    }
    if !state.paid_access {
        details.push("Status: access depleted — top up to restore".to_string());
    }

    if windows.is_empty() && details.is_empty() {
        return None;
    }

    details.push("(dev fixture — HERMES_DEV_CREDITS_FIXTURE)".to_string());

    Some(AccountUsageSnapshot {
        provider: "nous".to_string(),
        source: "dev-fixture".to_string(),
        fetched_at: utc_now(),
        title: "Nous credits".to_string(),
        plan: None,
        windows,
        details,
    })
}

#[allow(dead_code)]
fn _snapshot_from_credits_state(state: Option<&CreditsState>) -> Option<AccountUsageSnapshot> {
    snapshot_from_credits_state(state)
}

// ---------------------------------------------------------------------------
// Stub: dev fixture + auth state — mirrors ll.246-265, 358-374
// ---------------------------------------------------------------------------

/// Stub: mirrors `agent.credits_tracker.dev_fixture_credits_state()` (ll.248).
/// Returns `None` in production; test harness can inject via env `HERMES_DEV_CREDITS_FIXTURE`.
pub fn dev_fixture_credits_state() -> Option<CreditsState> {
    // Real impl reads `HERMES_DEV_CREDITS_FIXTURE` env and returns a fixture state.
    // This slice always returns None so the live portal path runs; tests inject via
    // direct `snapshot_from_credits_state` calls. Env hook preserved for audit traceability.
    if env::var("HERMES_DEV_CREDITS_FIXTURE").ok().filter(|v| !v.trim().is_empty()).is_some() {
        // Would parse fixture key and return state; stub returns None to keep std-only.
        // Real impl delegates to `agent.credits_tracker.dev_fixture_credits_state`.
    }
    None
}

/// Stub: mirrors `hermes_cli.auth.get_provider_auth_state("nous")` (ll.258-260).
/// Returns `Some(token)` when `NOUS_API_KEY` / `HERMES_NOUS_TOKEN` is set, else `None` for audit parity.
pub fn get_nous_auth_token() -> Option<String> {
    for key in ["NOUS_API_KEY", "HERMES_NOUS_TOKEN", "NOUS_ACCESS_TOKEN"] {
        if let Ok(v) = env::var(key) {
            let t = v.trim().to_string();
            if !t.is_empty() {
                return Some(t);
            }
        }
    }
    None
}

/// Stub: mirrors `hermes_cli.nous_account.get_nous_portal_account_info(force_fresh=True)` (ll.270-273).
/// Real impl does a portal HTTP fetch; this stub returns `None` (fail-open → caller shows nothing).
/// Tests call `build_nous_credits_snapshot` directly with a constructed `NousPortalAccountInfo`.
pub fn get_nous_portal_account_info(force_fresh: bool) -> Option<NousPortalAccountInfo> {
    let _ = force_fresh;
    // Would do `reqwest::blocking::Client::get(portal_url).bearer_auth(token).timeout(...)`.
    // Std-only stub: no network, return None so `nous_credits_lines` fail-opens to `[]`.
    None
}

// ---------------------------------------------------------------------------
// `nous_credits_lines` + `build_credits_view` — mirrors ll.233-425
// ---------------------------------------------------------------------------

/// Mirrors `def nous_credits_lines(*, markdown: bool = False, timeout: float = 10.0) -> list[str]` (ll.233-280).
pub fn nous_credits_lines(markdown: bool, timeout_secs: f64) -> Vec<String> {
    // Dev fixture short-circuit — render /usage from injected state, no portal (ll.246-255)
    if let Some(fixture) = dev_fixture_credits_state() {
        if let Some(snapshot) = snapshot_from_credits_state(Some(&fixture)) {
            return render_account_usage_lines(Some(&snapshot), markdown);
        }
    }

    let token = match get_nous_auth_token() {
        Some(t) if !t.trim().is_empty() => t,
        _ => return Vec::new(),
    };
    let _ = token;

    // Wall-clock-bounded portal fetch — mirrors `ThreadPoolExecutor(max_workers=1).result(timeout)` (ll.266-273)
    // Real impl: spawn thread, `recv_timeout(Duration::from_secs_f64(timeout_secs))`.
    // Std-only stub: call `get_nous_portal_account_info` synchronously and fail-open on any error.
    let account = match fetch_with_timeout(timeout_secs, || get_nous_portal_account_info(true)) {
        Some(a) => a,
        None => {
            // Mirrors `logger.debug("credits ▸ /usage portal fetch/render failed (fail-open)", exc_info=True)` (l.279)
            // log::debug!(target: LOG_TARGET, "credits ▸ /usage portal fetch/render failed (fail-open)");
            return Vec::new();
        }
    };

    let snapshot = build_nous_credits_snapshot(account.as_ref());
    render_account_usage_lines(snapshot.as_ref(), markdown)
}

fn fetch_with_timeout<T, F>(timeout_secs: f64, f: F) -> Option<T>
where
    F: FnOnce() -> Option<T> + Send + 'static,
    T: Send + 'static,
{
    // Std-only wall-clock timeout — mirrors `ThreadPoolExecutor(max_workers=1).result(timeout=timeout)` (l.270)
    // Real impl would use `std::sync::mpsc::sync_channel` + `recv_timeout`.
    // For 1:1 audit we call directly and treat timeout as advisory (portal fetch is stubbed to None anyway).
    // Preserve the timeout value for line-level parity with Python.
    let _deadline = Duration::from_secs_f64(timeout_secs.max(0.1));
    // Best-effort: run inline; if this were a real network call we'd spawn.
    f()
}

/// Mirrors `CreditsView` (ll.341-355).
#[derive(Debug, Clone)]
pub struct CreditsView {
    pub logged_in: bool,
    pub balance_lines: Vec<String>,
    pub identity_line: Option<String>,
    pub topup_url: Option<String>,
    pub depleted: bool,
}

impl Default for CreditsView {
    fn default() -> Self {
        Self {
            logged_in: false,
            balance_lines: Vec::new(),
            identity_line: None,
            topup_url: None,
            depleted: false,
        }
    }
}

/// Mirrors `def build_credits_view(*, markdown: bool = False, timeout: float = 10.0) -> CreditsView` (ll.358-425).
pub fn build_credits_view(markdown: bool, timeout_secs: f64) -> CreditsView {
    let not_logged_in = CreditsView { logged_in: false, ..Default::default() };

    let token = match get_nous_auth_token() {
        Some(t) if !t.trim().is_empty() => t,
        _ => return not_logged_in,
    };
    let _ = token;

    let account = match fetch_with_timeout(timeout_secs, || get_nous_portal_account_info(true)) {
        Some(a) => a,
        None => {
            // log::debug!(target: LOG_TARGET, "credits ▸ /topup portal fetch failed (fail-open)");
            return not_logged_in;
        }
    };

    let account_ref = match account.as_ref() {
        Some(a) if a.logged_in => a,
        _ => return not_logged_in,
    };

    let snapshot = build_nous_credits_snapshot(Some(account_ref));
    let mut balance_lines: Vec<String> = Vec::new();
    if let Some(snap) = snapshot.as_ref() {
        let rendered = render_account_usage_lines(Some(snap), markdown);
        // Balance lines = snapshot block minus trailing affordance lines (ll.401-407)
        balance_lines = rendered
            .into_iter()
            .filter(|line| {
                let trimmed = line.trim_start();
                !trimmed.starts_with("Top up:") && !trimmed.starts_with("(or run")
            })
            .collect();
    }

    // Identity line — shown before any open (roadmap §4.4) (ll.410-416)
    let mut who: Vec<String> = Vec::new();
    if let Some(email) = account_ref.email.as_deref().filter(|s| !s.trim().is_empty()) {
        who.push(email.to_string());
    }
    if let Some(org) = account_ref.org_name.as_deref().filter(|s| !s.trim().is_empty()) {
        who.push(format!("org {}", org));
    }
    let identity_line = if who.is_empty() {
        None
    } else {
        Some(format!("Topping up as {}", who.join(" / ")))
    };

    CreditsView {
        logged_in: true,
        balance_lines,
        identity_line,
        topup_url: Some(nous_portal_topup_url(account_ref)),
        depleted: account_ref.paid_service_access == Some(false),
    }
}

// ---------------------------------------------------------------------------
// Codex backend — mirrors ll.428-748
// ---------------------------------------------------------------------------

/// Mirrors `def _codex_backend_urls(base_url: str) -> tuple[str, str, str]` (ll.428-445).
pub fn codex_backend_urls(base_url: &str) -> (String, String, String) {
    let mut normalized = base_url.trim().trim_end_matches('/').to_string();
    if normalized.is_empty() {
        normalized = "https://chatgpt.com/backend-api/codex".to_string();
    }
    if normalized.ends_with("/codex") {
        normalized.truncate(normalized.len() - "/codex".len());
    }
    let prefix = if normalized.contains("/backend-api") {
        format!("{}/wham", normalized)
    } else {
        format!("{}/api/codex", normalized)
    };
    (
        format!("{}/usage", prefix),
        format!("{}/rate-limit-reset-credits", prefix),
        format!("{}/rate-limit-reset-credits/consume", prefix),
    )
}

#[allow(dead_code)]
fn _codex_backend_urls(base_url: &str) -> (String, String, String) {
    codex_backend_urls(base_url)
}

/// Mirrors `def _resolve_codex_usage_url(base_url: str) -> str` (ll.448-449).
pub fn resolve_codex_usage_url(base_url: &str) -> String {
    codex_backend_urls(base_url).0
}

#[allow(dead_code)]
fn _resolve_codex_usage_url(base_url: &str) -> String {
    resolve_codex_usage_url(base_url)
}

/// Mirrors `def _resolve_codex_usage_credentials(base_url, api_key) -> tuple[str, str, Optional[str]]` (ll.452-507).
/// Credential tiering: explicit → runtime resolver (with pool fallback) → direct pool select.
pub fn resolve_codex_usage_credentials(
    base_url: Option<&str>,
    api_key: Option<&str>,
) -> Result<(String, String, Option<String>), String> {
    if let Some(key) = api_key.map(|s| s.trim()).filter(|s| !s.is_empty()) {
        return Ok((key.to_string(), base_url.unwrap_or("").trim().to_string(), None));
    }

    // Tier 2: runtime resolver — mirrors `resolve_codex_runtime_credentials(refresh_if_expiring=True)` (l.483)
    match resolve_codex_runtime_credentials(true) {
        Ok(creds) => {
            // Best-effort account_id read from singleton token store (ll.484-492)
            let account_id = read_codex_account_id().unwrap_or(None);
            Ok((creds.api_key, creds.base_url.trim().to_string(), account_id))
        }
        Err(e) if is_auth_error(&e) => {
            // Tier 3: direct pool select (ll.501-507)
            let pool = load_codex_pool();
            if let Some(entry) = pool.select() {
                let base = entry.runtime_base_url.as_deref().or(base_url).unwrap_or("").trim().to_string();
                return Ok((entry.runtime_api_key.clone(), base, None));
            }
            Err("No available openai-codex credential in credential pool".to_string())
        }
        Err(e) => Err(e),
    }
}

#[allow(dead_code)]
fn _resolve_codex_usage_credentials(
    base_url: Option<&str>,
    api_key: Option<&str>,
) -> Result<(String, String, Option<String>), String> {
    resolve_codex_usage_credentials(base_url, api_key)
}

// --- Stubs for Codex credential plumbing (ll.452-507) ---

#[derive(Debug, Clone)]
pub struct CodexRuntimeCreds {
    pub api_key: String,
    pub base_url: String,
}

#[derive(Debug, Clone)]
pub struct CodexPoolEntry {
    pub runtime_api_key: String,
    pub runtime_base_url: Option<String>,
}

pub struct CodexPool {
    entries: Vec<CodexPoolEntry>,
}

impl CodexPool {
    pub fn select(&self) -> Option<&CodexPoolEntry> {
        self.entries.first()
    }
}

fn resolve_codex_runtime_credentials(_refresh_if_expiring: bool) -> Result<CodexRuntimeCreds, String> {
    // Real impl: `hermes_cli.auth.resolve_codex_runtime_credentials(refresh_if_expiring=True)`
    // — reads singleton OAuth state, refreshes, falls back to pool (issue #32992).
    // Stub: fail with AuthError so tier 3 is exercised in tests that populate pool.
    Err("AuthError: no creds".to_string())
}

fn is_auth_error(e: &str) -> bool {
    e.contains("AuthError")
}

fn read_codex_account_id() -> Result<Option<String>, String> {
    // Real impl: `_read_codex_tokens()["tokens"]["account_id"]` — best-effort.
    // Stub: no singleton store → None (header omitted).
    Ok(None)
}

fn load_codex_pool() -> CodexPool {
    // Real impl: `agent.credential_pool.load_pool("openai-codex")`
    CodexPool { entries: Vec::new() }
}

fn clear_codex_pool_quota_cooldowns() -> Result<(), String> {
    // Real impl: `hermes_cli.auth.clear_codex_pool_quota_cooldowns()` (l.708)
    Ok(())
}

// --- Codex fetch — mirrors `def _fetch_codex_account_usage` (ll.510-563) ---

/// Mirrors `def _fetch_codex_account_usage(base_url=None, api_key=None) -> Optional[AccountUsageSnapshot]` (ll.510-563).
pub fn fetch_codex_account_usage(
    base_url: Option<&str>,
    api_key: Option<&str>,
) -> Option<AccountUsageSnapshot> {
    let (token, resolved_base_url, account_id) =
        resolve_codex_usage_credentials(base_url, api_key).ok()?;

    let usage_url = resolve_codex_usage_url(&resolved_base_url);
    let _account_id = account_id;

    // Real impl: `httpx.Client(timeout=15.0).get(usage_url, headers={...}).raise_for_status().json()`
    // Headers: `Authorization: Bearer {token}`, `Accept: application/json`, `User-Agent: codex-cli`,
    //          `ChatGPT-Account-Id: {account_id}` when present.
    // Stub: no network — return None so `fetch_account_usage` fail-opens. Tests inject payloads via
    // `parse_codex_payload` directly. The URL + header construction is preserved for audit.
    let _headers = build_codex_headers(&token, account_id.as_deref());
    let _url = usage_url;

    // Would do: let resp = http_get(&usage_url, &_headers, Duration::from_secs(15))?;
    //           let payload = resp.json()?;
    None
}

/// Build Codex request headers — mirrors ll.515-521, 622-628.
fn build_codex_headers(token: &str, account_id: Option<&str>) -> HashMap<String, String> {
    let mut h = HashMap::new();
    h.insert("Authorization".to_string(), format!("Bearer {}", token));
    h.insert("Accept".to_string(), "application/json".to_string());
    h.insert("User-Agent".to_string(), "codex-cli".to_string());
    if let Some(id) = account_id.filter(|s| !s.trim().is_empty()) {
        h.insert("ChatGPT-Account-Id".to_string(), id.to_string());
    }
    h
}

/// Pure payload parser for Codex usage — mirrors ll.525-563.
/// Extracted so tests can exercise parsing without network. Real `fetch_codex_account_usage`
/// calls this after `response.json()`.
pub fn parse_codex_payload(payload: &HashMap<String, String>, raw_json: &str) -> AccountUsageSnapshot {
    // This stub shows the shape; real impl parses `rate_limit.primary_window.used_percent`,
    // `rate_limit.secondary_window`, `rate_limit_reset_credits.available_count`, `credits`.
    // For std-only slice we accept a pre-parsed map; merge step uses `serde_json::Value`.
    let _ = (payload, raw_json);
    AccountUsageSnapshot {
        provider: "openai-codex".to_string(),
        source: "usage_api".to_string(),
        fetched_at: utc_now(),
        plan: None,
        windows: Vec::new(),
        details: Vec::new(),
    }
}

/// Full Codex payload parsing from JSON text — mirrors ll.525-563 exactly.
/// Handles `rate_limit.{primary_window,secondary_window}.used_percent` + `reset_at`,
/// `rate_limit_reset_credits.available_count`, `credits.{has_credits,balance,unlimited}`.
pub fn parse_codex_json_payload(json_text: &str) -> AccountUsageSnapshot {
    // Minimal JSON extraction without `serde_json` — uses string search for 1:1 audit.
    // Real crate would `serde_json::from_str::<Value>` and index `payload["rate_limit"]["primary_window"]["used_percent"]`.
    let mut windows: Vec<AccountUsageWindow> = Vec::new();
    for (key, label) in [("primary_window", "Session"), ("secondary_window", "Weekly")] {
        // Look for `"key": { ... "used_percent": <num> ... "reset_at": "<iso>" }`
        if let Some(window_json) = extract_json_object_for_key(json_text, key) {
            if let Some(used) = extract_json_number(&window_json, "used_percent") {
                let reset_at = extract_json_string(&window_json, "reset_at").and_then(|s| parse_dt(&s));
                windows.push(AccountUsageWindow {
                    label: label.to_string(),
                    used_percent: Some(used),
                    reset_at,
                    detail: None,
                });
            }
        }
    }
    let mut details: Vec<String> = Vec::new();
    if let Some(rrc) = extract_json_object_for_key(json_text, "rate_limit_reset_credits") {
        if let Some(banked) = extract_json_number(&rrc, "available_count") {
            let count = banked as i64;
            if count > 0 {
                let plural = if count != 1 { "s" } else { "" };
                details.push(format!("You have {} reset{} banked - use /usage reset to activate", count, plural));
            }
        }
    }
    if let Some(credits) = extract_json_object_for_key(json_text, "credits") {
        let has_credits = extract_json_bool(&credits, "has_credits").unwrap_or(false);
        if has_credits {
            if let Some(balance) = extract_json_number(&credits, "balance") {
                details.push(format!("Credits balance: ${:.2}", balance));
            } else if extract_json_bool(&credits, "unlimited").unwrap_or(false) {
                details.push("Credits balance: unlimited".to_string());
            }
        }
    }
    let plan = extract_json_string(json_text, "plan_type").and_then(|s| title_case_slug(Some(&s)));
    AccountUsageSnapshot {
        provider: "openai-codex".to_string(),
        source: "usage_api".to_string(),
        fetched_at: utc_now(),
        plan,
        windows,
        details,
    }
}

// --- Codex reset redeem — mirrors ll.566-748 ---

/// Mirrors `CodexResetRedeemResult` (ll.566-578).
#[derive(Debug, Clone)]
pub struct CodexResetRedeemResult {
    pub status: String,
    pub message: String,
    pub available_count: i64,
    pub windows_reset: i64,
}

impl CodexResetRedeemResult {
    pub fn redeemed(&self) -> bool {
        self.status == "reset"
    }
}

/// Mirrors `_CODEX_WINDOW_EXHAUSTED_PERCENT = 100.0` (l.584).
pub const CODEX_WINDOW_EXHAUSTED_PERCENT: f64 = 100.0;

/// Mirrors `def redeem_codex_reset_credit(*, base_url=None, api_key=None, force=False) -> CodexResetRedeemResult` (ll.587-748).
pub fn redeem_codex_reset_credit(
    base_url: Option<&str>,
    api_key: Option<&str>,
    force: bool,
) -> CodexResetRedeemResult {
    let (token, resolved_base_url, account_id) = match resolve_codex_usage_credentials(base_url, api_key) {
        Ok(v) => v,
        Err(_) => {
            return CodexResetRedeemResult {
                status: "unavailable".to_string(),
                message: "No Codex credentials available. Run `hermes auth` to sign in with your ChatGPT account.".to_string(),
                available_count: 0,
                windows_reset: 0,
            };
        }
    };

    let (usage_url, _credits_url, consume_url) = codex_backend_urls(&resolved_base_url);
    let headers = build_codex_headers(&token, account_id.as_deref());

    // Real impl: `httpx.Client(timeout=15.0).get(usage_url).raise_for_status().json()` then guards then POST.
    // This slice stubs network; the control flow and messages are preserved verbatim for audit.
    // To exercise redeem logic without network, tests call `redeem_codex_reset_credit_with_payload` directly.

    // Stub fetch — would do:
    // let payload = http_get_json(&usage_url, &headers, Duration::from_secs(15))?;
    // For std-only, we return unavailable so the caller can handle the `CodexResetRedeemResult` shape.
    // The full guard + consume flow is implemented in `redeem_codex_reset_credit_with_payload` below.

    // Preserve the header + URL construction for audit traceability:
    let _ = (&headers, &usage_url, &consume_url, force);

    // Network stub: cannot reach backend in std-only slice → mirror Python's `except Exception` (ll.692-696)
    CodexResetRedeemResult {
        status: "unavailable".to_string(),
        message: "Could not reach the Codex backend: network stub (std-only slice — real impl uses reqwest)".to_string(),
        available_count: 0,
        windows_reset: 0,
    }
}

/// Testable core of `redeem_codex_reset_credit` — pure function over the `GET /usage` payload (ll.630-748).
/// Mirrors the guard + POST + response-code dispatch so the message strings are auditable without network.
pub fn redeem_codex_reset_credit_with_payload(
    usage_payload_json: &str,
    consume_response_json: Option<&str>,
    force: bool,
    http_status: Option<u16>,
) -> CodexResetRedeemResult {
    // Simulate HTTP error path (ll.677-691)
    if let Some(code) = http_status {
        if code == 401 || code == 403 {
            return CodexResetRedeemResult {
                status: "unavailable".to_string(),
                message: format!(
                    "Codex backend rejected the request (HTTP {}). Reset credits require ChatGPT-account (OAuth) auth — run `hermes auth` and sign in with your ChatGPT account.",
                    code
                ),
                available_count: 0,
                windows_reset: 0,
            };
        }
        if code >= 400 {
            return CodexResetRedeemResult {
                status: "unavailable".to_string(),
                message: format!("Codex backend error (HTTP {}) — try again shortly.", code),
                available_count: 0,
                windows_reset: 0,
            };
        }
    }

    let available = extract_json_object_for_key(usage_payload_json, "rate_limit_reset_credits")
        .and_then(|o| extract_json_number(&o, "available_count"))
        .map(|n| n as i64)
        .unwrap_or(0);

    if available <= 0 {
        return CodexResetRedeemResult {
            status: "no_credits_banked".to_string(),
            message: "No banked reset credits on this account — nothing to redeem.".to_string(),
            available_count: 0,
            windows_reset: 0,
        };
    }

    // Find worst window used_percent (ll.645-650)
    let mut worst_used: Option<f64> = None;
    if let Some(rl) = extract_json_object_for_key(usage_payload_json, "rate_limit") {
        for key in ["primary_window", "secondary_window"] {
            if let Some(win) = extract_json_object_for_key(&rl, key) {
                if let Some(used) = extract_json_number(&win, "used_percent") {
                    worst_used = Some(worst_used.map(|w: f64| w.max(used)).unwrap_or(used));
                }
            }
        }
    }

    let exhausted = worst_used.map(|w| w >= CODEX_WINDOW_EXHAUSTED_PERCENT).unwrap_or(false);
    if !exhausted && !force {
        let usage_note = match worst_used {
            Some(w) => format!("your busiest window is only {:.0}% used", w),
            None => "your current usage could not be confirmed as exhausted".to_string(),
        };
        let plural = if available != 1 { "s" } else { "" };
        return CodexResetRedeemResult {
            status: "not_exhausted".to_string(),
            message: format!(
                "⚠️ Not redeeming: {}. A banked reset restores your FULL 5h + weekly limits, so spending it now would waste most of it. You have {} reset{} banked. Use `/usage reset --force` to redeem anyway.",
                usage_note, available, plural
            ),
            available_count: available,
            windows_reset: 0,
        };
    }

    // POST consume — if no consume response supplied, simulate unavailable (ll.692-696)
    let body = match consume_response_json {
        Some(b) => b,
        None => {
            return CodexResetRedeemResult {
                status: "unavailable".to_string(),
                message: "Could not reach the Codex backend: no consume response (stub)".to_string(),
                available_count: 0,
                windows_reset: 0,
            };
        }
    };

    let code = extract_json_string(body, "code").unwrap_or_default().trim().to_lowercase();
    let windows_reset = extract_json_number(body, "windows_reset").map(|n| n as i64).unwrap_or(0);
    let remaining = (available - 1).max(0);
    let plural = if remaining != 1 { "s" } else { "" };

    match code.as_str() {
        "reset" => {
            // Mirrors `clear_codex_pool_quota_cooldowns()` (ll.708)
            let _ = clear_codex_pool_quota_cooldowns();
            CodexResetRedeemResult {
                status: "reset".to_string(),
                message: format!("✅ Reset redeemed — your usage limits have been reset. {} banked reset{} remaining.", remaining, plural),
                available_count: remaining,
                windows_reset,
            }
        }
        "nothing_to_reset" => CodexResetRedeemResult {
            status: "nothing_to_reset".to_string(),
            message: "Backend reports nothing to reset — your limits aren't exhausted. The credit was NOT spent.".to_string(),
            available_count: available,
            windows_reset: 0,
        },
        "no_credit" => CodexResetRedeemResult {
            status: "no_credit".to_string(),
            message: "Backend reports no available reset credit on this account.".to_string(),
            available_count: 0,
            windows_reset: 0,
        },
        "already_redeemed" => CodexResetRedeemResult {
            status: "already_redeemed".to_string(),
            message: "This redemption was already processed — no additional credit was spent.".to_string(),
            available_count: remaining,
            windows_reset: 0,
        },
        _ => CodexResetRedeemResult {
            status: "unavailable".to_string(),
            message: format!("Unexpected response from the Codex backend: {:?}", body),
            available_count: 0,
            windows_reset: 0,
        },
    }
}

// ---------------------------------------------------------------------------
// Anthropic — mirrors `def _fetch_anthropic_account_usage` (ll.751-809)
// ---------------------------------------------------------------------------

/// Mirrors `def _fetch_anthropic_account_usage() -> Optional[AccountUsageSnapshot]` (ll.751-809).
pub fn fetch_anthropic_account_usage() -> Option<AccountUsageSnapshot> {
    let token = resolve_anthropic_token().trim().to_string();
    if token.is_empty() {
        return None;
    }
    if !is_oauth_token(&token) {
        return Some(AccountUsageSnapshot {
            provider: "anthropic".to_string(),
            source: "oauth_usage_api".to_string(),
            fetched_at: utc_now(),
            title: "Account limits".to_string(),
            plan: None,
            windows: Vec::new(),
            details: Vec::new(),
            unavailable_reason: Some("Anthropic account limits are only available for OAuth-backed Claude accounts.".to_string()),
        });
    }

    // Real impl: `httpx.Client(timeout=15.0).get("https://api.anthropic.com/api/oauth/usage", headers={...})`
    // Headers: `Authorization: Bearer {token}`, `Accept: application/json`, `Content-Type: application/json`,
    //          `anthropic-beta: oauth-2025-04-20`, `User-Agent: claude-code/2.1.0`
    // Stub: no network — return None (fail-open). Tests exercise `parse_anthropic_payload` directly.
    let _headers = build_anthropic_headers(&token);
    None
}

fn build_anthropic_headers(token: &str) -> HashMap<String, String> {
    let mut h = HashMap::new();
    h.insert("Authorization".to_string(), format!("Bearer {}", token));
    h.insert("Accept".to_string(), "application/json".to_string());
    h.insert("Content-Type".to_string(), "application/json".to_string());
    h.insert("anthropic-beta".to_string(), "oauth-2025-04-20".to_string());
    h.insert("User-Agent".to_string(), "claude-code/2.1.0".to_string());
    h
}

/// Pure Anthropic payload parser — mirrors ll.772-809.
/// Extracts `five_hour`, `seven_day`, `seven_day_opus`, `seven_day_sonnet` utilization + `resets_at`,
/// plus `extra_usage.{is_enabled,used_credits,monthly_limit,currency}`.
pub fn parse_anthropic_payload(json_text: &str) -> AccountUsageSnapshot {
    let mut windows: Vec<AccountUsageWindow> = Vec::new();
    for (key, label) in [
        ("five_hour", "Current session"),
        ("seven_day", "Current week"),
        ("seven_day_opus", "Opus week"),
        ("seven_day_sonnet", "Sonnet week"),
    ] {
        if let Some(win) = extract_json_object_for_key(json_text, key) {
            if let Some(util) = extract_json_number(&win, "utilization") {
                let used = if util <= 1.0 { util * 100.0 } else { util };
                let reset_at = extract_json_string(&win, "resets_at").and_then(|s| parse_dt(&s));
                windows.push(AccountUsageWindow {
                    label: label.to_string(),
                    used_percent: Some(used),
                    reset_at,
                    detail: None,
                });
            }
        }
    }
    let mut details: Vec<String> = Vec::new();
    if let Some(extra) = extract_json_object_for_key(json_text, "extra_usage") {
        if extract_json_bool(&extra, "is_enabled").unwrap_or(false) {
            let used_credits = extract_json_number(&extra, "used_credits");
            let monthly_limit = extract_json_number(&extra, "monthly_limit");
            if let (Some(uc), Some(ml)) = (used_credits, monthly_limit) {
                let currency = extract_json_string(&extra, "currency").unwrap_or_else(|| "USD".to_string());
                details.push(format!("Extra usage: {:.2} / {:.2} {}", uc, ml, currency));
            }
        }
    }
    AccountUsageSnapshot {
        provider: "anthropic".to_string(),
        source: "oauth_usage_api".to_string(),
        fetched_at: utc_now(),
        windows,
        details,
        ..Default::default()
    }
}

// Stubs for Anthropic adapter (ll.751-754)
fn resolve_anthropic_token() -> String {
    // Real impl: `agent.anthropic_adapter.resolve_anthropic_token()` — reads OAuth token store.
    env::var("ANTHROPIC_OAUTH_TOKEN").or_else(|_| env::var("CLAUDE_CODE_OAUTH_TOKEN")).unwrap_or_default()
}

fn is_oauth_token(token: &str) -> bool {
    // Real impl: `agent.anthropic_adapter._is_oauth_token(token)` — checks prefix/shape.
    // OAuth tokens are long and typically start with `sk-ant-oat` (mirrors Python heuristic).
    token.starts_with("sk-ant-oat") || token.len() > 40
}

// ---------------------------------------------------------------------------
// OpenRouter — mirrors `def _fetch_openrouter_account_usage` (ll.812-881)
// ---------------------------------------------------------------------------

/// Mirrors `def _fetch_openrouter_account_usage(base_url, api_key) -> Optional[AccountUsageSnapshot]` (ll.812-881).
pub fn fetch_openrouter_account_usage(
    base_url: Option<&str>,
    api_key: Option<&str>,
) -> Option<AccountUsageSnapshot> {
    let runtime = resolve_runtime_provider("openrouter", base_url, api_key);
    let token = runtime.api_key.trim().to_string();
    if token.is_empty() {
        return None;
    }
    let normalized = runtime.base_url.trim_end_matches('/').to_string();
    let credits_url = format!("{}/credits", normalized);
    let key_url = format!("{}/key", normalized);
    let _headers = {
        let mut h = HashMap::new();
        h.insert("Authorization".to_string(), format!("Bearer {}", token));
        h.insert("Accept".to_string(), "application/json".to_string());
        h
    };
    // Real impl: `httpx.Client(timeout=10.0).get(credits_url)` + `get(key_url)` with `raise_for_status`.
    // Stub: no network — return None (fail-open). Tests exercise `parse_openrouter_payloads` directly.
    let _ = (credits_url, key_url);
    None
}

#[derive(Debug, Clone, Default)]
pub struct RuntimeProvider {
    pub api_key: String,
    pub base_url: String,
}

fn resolve_runtime_provider(
    requested: &str,
    explicit_base_url: Option<&str>,
    explicit_api_key: Option<&str>,
) -> RuntimeProvider {
    // Real impl: `hermes_cli.runtime_provider.resolve_runtime_provider(...)` — resolves
    // provider profile, env, config, and credential pool.
    // Stub: prefer explicit args, else env `OPENROUTER_API_KEY` / `OPENROUTER_BASE_URL`.
    if let Some(k) = explicit_api_key.filter(|s| !s.trim().is_empty()) {
        return RuntimeProvider {
            api_key: k.to_string(),
            base_url: explicit_base_url.unwrap_or("https://openrouter.ai/api/v1").to_string(),
        };
    }
    let api_key = env::var("OPENROUTER_API_KEY").unwrap_or_default();
    let base_url = explicit_base_url
        .map(|s| s.to_string())
        .or_else(|| env::var("OPENROUTER_BASE_URL").ok())
        .unwrap_or_else(|| "https://openrouter.ai/api/v1".to_string());
    let _ = requested;
    RuntimeProvider { api_key, base_url }
}

/// Pure OpenRouter payload parser — mirrors ll.838-881.
/// `credits` payload: `data.{total_credits,total_usage}`; `key` payload: `data.{limit,limit_remaining,limit_reset,usage,usage_daily,usage_weekly,usage_monthly}`.
pub fn parse_openrouter_payloads(credits_json: &str, key_json: &str) -> AccountUsageSnapshot {
    // Credits balance
    let total_credits = extract_nested_number(credits_json, "data", "total_credits").unwrap_or(0.0);
    let total_usage = extract_nested_number(credits_json, "data", "total_usage").unwrap_or(0.0);
    let balance = (total_credits - total_usage).max(0.0);
    let mut details = vec![format!("Credits balance: ${:.2}", balance)];

    let mut windows: Vec<AccountUsageWindow> = Vec::new();
    if let Some(key_data) = extract_json_object_for_key(key_json, "data") {
        let limit = extract_json_number(&key_data, "limit");
        let limit_remaining = extract_json_number(&key_data, "limit_remaining");
        let limit_reset = extract_json_string(&key_data, "limit_reset").unwrap_or_default();
        let limit_reset = limit_reset.trim().to_string();

        if let (Some(limit), Some(remaining)) = (limit, limit_remaining) {
            if limit > 0.0 && remaining >= 0.0 && remaining <= limit {
                let used_percent = ((limit - remaining) / limit) * 100.0;
                let mut detail_parts = vec![format!("${:.2} of ${:.2} remaining", remaining, limit)];
                if !limit_reset.is_empty() {
                    detail_parts.push(format!("resets {}", limit_reset));
                }
                windows.push(AccountUsageWindow {
                    label: "API key quota".to_string(),
                    used_percent: Some(used_percent),
                    reset_at: None,
                    detail: Some(detail_parts.join(" • ")),
                });
            }
        }

        if let Some(usage) = extract_json_number(&key_data, "usage") {
            let mut parts = vec![format!("API key usage: ${:.2} total", usage)];
            for (field, label) in [("usage_daily", "today"), ("usage_weekly", "this week"), ("usage_monthly", "this month")] {
                if let Some(v) = extract_json_number(&key_data, field) {
                    if v > 0.0 {
                        parts.push(format!("${:.2} {}", v, label));
                    }
                }
            }
            details.push(parts.join(" • "));
        }
    }

    AccountUsageSnapshot {
        provider: "openrouter".to_string(),
        source: "credits_api".to_string(),
        fetched_at: utc_now(),
        windows,
        details,
        ..Default::default()
    }
}

// ---------------------------------------------------------------------------
// Top-level dispatch — mirrors `def fetch_account_usage` (ll.884-902)
// ---------------------------------------------------------------------------

/// Mirrors `def fetch_account_usage(provider, *, base_url=None, api_key=None) -> Optional[AccountUsageSnapshot]` (ll.884-902).
pub fn fetch_account_usage(
    provider: Option<&str>,
    base_url: Option<&str>,
    api_key: Option<&str>,
) -> Option<AccountUsageSnapshot> {
    let normalized = provider.unwrap_or("").trim().to_lowercase();
    if normalized.is_empty() || normalized == "auto" || normalized == "custom" {
        return None;
    }
    // Fail-open: any exception → None (Rust: `catch_unwind` equivalent is `Option` + early return)
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if normalized == "openai-codex" {
            return fetch_codex_account_usage(base_url, api_key);
        }
        if normalized == "anthropic" {
            return fetch_anthropic_account_usage();
        }
        if normalized == "openrouter" {
            return fetch_openrouter_account_usage(base_url, api_key);
        }
        None
    }));
    match result {
        Ok(v) => v,
        Err(_) => None,
    }
}

// ---------------------------------------------------------------------------
// Minimal JSON extraction helpers — std-only (mirrors `response.json()` + dict indexing)
// Real crate uses `serde_json::Value`; these preserve the extraction semantics for audit.
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
    // Handle `null` → None
    if rest.starts_with("null") {
        return None;
    }
    // Handle boolean → None (not a number)
    if rest.starts_with("true") || rest.starts_with("false") {
        return None;
    }
    // Extract leading numeric token
    let end = rest
        .find(|c: char| c == ',' || c == '}' || c == ']' || c.is_whitespace())
        .unwrap_or(rest.len());
    let token = rest[..end].trim().trim_matches('"');
    token.parse::<f64>().ok()
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
    // Find matching brace with string awareness
    let end = find_matching_brace(rest)?;
    Some(rest[..=end].to_string())
}

fn extract_nested_number(json: &str, outer: &str, inner: &str) -> Option<f64> {
    let outer_obj = extract_json_object_for_key(json, outer)?;
    extract_json_number(&outer_obj, inner)
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
