//! Rate limit tracking for inference API responses.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/agent/rate_limit_tracker.py` (246 lines).
//!
//! Captures x-ratelimit-* headers from provider responses and provides
//! formatted display for the /usage slash command. Currently supports
//! the Nous Portal header format (also used by OpenRouter and OpenAI-compatible
//! APIs that follow the same convention).
//!
//! Header schema (12 headers total):
//!     x-ratelimit-limit-requests          RPM cap
//!     x-ratelimit-limit-requests-1h       RPH cap
//!     x-ratelimit-limit-tokens            TPM cap
//!     x-ratelimit-limit-tokens-1h         TPH cap
//!     x-ratelimit-remaining-requests      requests left in minute window
//!     x-ratelimit-remaining-requests-1h   requests left in hour window
//!     x-ratelimit-remaining-tokens        tokens left in minute window
//!     x-ratelimit-remaining-tokens-1h     tokens left in hour window
//!     x-ratelimit-reset-requests          seconds until minute request window resets
//!     x-ratelimit-reset-requests-1h       seconds until hour request window resets
//!     x-ratelimit-reset-tokens            seconds until minute token window resets
//!     x-ratelimit-reset-tokens-1h         seconds until hour token window resets

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// helpers — time
// ---------------------------------------------------------------------------

fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

// ---------------------------------------------------------------------------
// RateLimitBucket — mirrors `class RateLimitBucket` (lines 30-53)
// ---------------------------------------------------------------------------

/// One rate-limit window (e.g. requests per minute).
/// Mirrors `RateLimitBucket` dataclass (lines 30-53).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RateLimitBucket {
    pub limit: i64,
    pub remaining: i64,
    pub reset_seconds: f64,
    pub captured_at: f64,
}

impl RateLimitBucket {
    pub fn new(limit: i64, remaining: i64, reset_seconds: f64, captured_at: f64) -> Self {
        Self {
            limit,
            remaining,
            reset_seconds,
            captured_at,
        }
    }

    /// Mirrors `used` property (line 40-41): max(0, limit - remaining).
    pub fn used(&self) -> i64 {
        std::cmp::max(0, self.limit - self.remaining)
    }

    /// Mirrors `usage_pct` property (lines 43-47).
    pub fn usage_pct(&self) -> f64 {
        if self.limit <= 0 {
            return 0.0;
        }
        (self.used() as f64 / self.limit as f64) * 100.0
    }

    /// Mirrors `remaining_seconds_now` property (lines 49-53).
    /// Estimated seconds remaining until reset, adjusted for elapsed time.
    pub fn remaining_seconds_now(&self) -> f64 {
        self.remaining_seconds_at(now_secs())
    }

    /// Testable variant that accepts an explicit `now` timestamp.
    pub fn remaining_seconds_at(&self, now: f64) -> f64 {
        let elapsed = now - self.captured_at;
        (self.reset_seconds - elapsed).max(0.0)
    }
}

// ---------------------------------------------------------------------------
// RateLimitState — mirrors `class RateLimitState` (lines 56-76)
// ---------------------------------------------------------------------------

/// Full rate-limit state parsed from response headers.
/// Mirrors `RateLimitState` dataclass (lines 56-76).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RateLimitState {
    pub requests_min: RateLimitBucket,
    pub requests_hour: RateLimitBucket,
    pub tokens_min: RateLimitBucket,
    pub tokens_hour: RateLimitBucket,
    pub captured_at: f64,
    pub provider: String,
}

impl RateLimitState {
    /// Mirrors `has_data` property (lines 67-69).
    pub fn has_data(&self) -> bool {
        self.captured_at > 0.0
    }

    /// Mirrors `age_seconds` property (lines 71-76).
    pub fn age_seconds(&self) -> f64 {
        self.age_seconds_at(now_secs())
    }

    /// Testable variant.
    pub fn age_seconds_at(&self, now: f64) -> f64 {
        if !self.has_data() {
            return f64::INFINITY;
        }
        now - self.captured_at
    }
}

// ---------------------------------------------------------------------------
// _safe_int / _safe_float — mirrors lines 78-89
// ---------------------------------------------------------------------------

fn safe_int(value: Option<&str>, default: i64) -> i64 {
    match value {
        None => default,
        Some(v) => {
            let trimmed = v.trim();
            if trimmed.is_empty() {
                return default;
            }
            // Mirrors int(float(value)) — handles "3.7" -> 3, "1e3" -> 1000
            match trimmed.parse::<f64>() {
                Ok(f) => {
                    if !f.is_finite() {
                        return default;
                    }
                    // Truncate toward zero like Python int()
                    f.trunc() as i64
                }
                Err(_) => default,
            }
        }
    }
}

fn safe_float(value: Option<&str>, default: f64) -> f64 {
    match value {
        None => default,
        Some(v) => {
            let trimmed = v.trim();
            if trimmed.is_empty() {
                return default;
            }
            match trimmed.parse::<f64>() {
                Ok(f) => f,
                Err(_) => default,
            }
        }
    }
}

// ---------------------------------------------------------------------------
// parse_rate_limit_headers — mirrors lines 92-129
// ---------------------------------------------------------------------------

/// Parse x-ratelimit-* headers into a RateLimitState.
/// Returns None if no rate limit headers are present.
/// Mirrors `parse_rate_limit_headers` (lines 92-129).
pub fn parse_rate_limit_headers(
    headers: &HashMap<String, String>,
    provider: &str,
) -> Option<RateLimitState> {
    parse_rate_limit_headers_at(headers, provider, now_secs())
}

/// Testable variant with explicit `now` timestamp.
pub fn parse_rate_limit_headers_at(
    headers: &HashMap<String, String>,
    provider: &str,
    now: f64,
) -> Option<RateLimitState> {
    // Normalize to lowercase so lookups work regardless of how the server
    // capitalises headers (HTTP header names are case-insensitive per RFC 7230).
    let lowered: HashMap<String, String> = headers
        .iter()
        .map(|(k, v)| (k.to_lowercase(), v.clone()))
        .collect();

    // Quick check: at least one rate limit header must exist
    let has_any = lowered.keys().any(|k| k.starts_with("x-ratelimit-"));
    if !has_any {
        return None;
    }

    let bucket = |resource: &str, suffix: &str| -> RateLimitBucket {
        let tag = format!("{resource}{suffix}");
        RateLimitBucket {
            limit: safe_int(
                lowered.get(&format!("x-ratelimit-limit-{tag}")).map(|s| s.as_str()),
                0,
            ),
            remaining: safe_int(
                lowered
                    .get(&format!("x-ratelimit-remaining-{tag}"))
                    .map(|s| s.as_str()),
                0,
            ),
            reset_seconds: safe_float(
                lowered
                    .get(&format!("x-ratelimit-reset-{tag}"))
                    .map(|s| s.as_str()),
                0.0,
            ),
            captured_at: now,
        }
    };

    Some(RateLimitState {
        requests_min: bucket("requests", ""),
        requests_hour: bucket("requests", "-1h"),
        tokens_min: bucket("tokens", ""),
        tokens_hour: bucket("tokens", "-1h"),
        captured_at: now,
        provider: provider.to_string(),
    })
}

// ---------------------------------------------------------------------------
// Formatting — mirrors lines 134-246
// ---------------------------------------------------------------------------

/// Mirrors `_fmt_count` (lines 135-143):
/// Human-friendly number: 7999856 -> '8.0M', 33599 -> '33.6K', 799 -> '799'.
pub fn fmt_count(n: i64) -> String {
    // Python uses float division; preserve 1 decimal even for exact thousands
    if n >= 1_000_000 {
        return format!("{:.1}M", n as f64 / 1_000_000.0);
    }
    if n >= 10_000 {
        return format!("{:.1}K", n as f64 / 1_000.0);
    }
    if n >= 1_000 {
        return format!("{:.1}K", n as f64 / 1_000.0);
    }
    n.to_string()
}

fn fmt_count_internal(n: i64) -> String {
    fmt_count(n)
}

/// Mirrors `_fmt_seconds` (lines 146-156):
/// Seconds -> human-friendly duration: '58s', '2m 14s', '58m 57s', '1h 2m'.
pub fn fmt_seconds(seconds: f64) -> String {
    let s = std::cmp::max(0, seconds as i64);
    if s < 60 {
        return format!("{s}s");
    }
    if s < 3600 {
        let m = s / 60;
        let sec = s % 60;
        if sec != 0 {
            return format!("{m}m {sec}s");
        } else {
            return format!("{m}m");
        }
    }
    let h = s / 3600;
    let remainder = s % 3600;
    let m = remainder / 60;
    if m != 0 {
        format!("{h}h {m}m")
    } else {
        format!("{h}h")
    }
}

fn fmt_seconds_internal(seconds: f64) -> String {
    fmt_seconds(seconds)
}

/// Mirrors `_bar` (lines 159-164): ASCII progress bar: [████████░░░░░░░░░░░░] 40%.
pub fn bar(pct: f64) -> String {
    bar_with_width(pct, 20)
}

pub fn bar_with_width(pct: f64, width: usize) -> String {
    let mut filled = (pct / 100.0 * width as f64) as usize;
    // int() truncates toward zero for positive; negative clamps to 0
    // Clamp to [0, width]
    if pct < 0.0 {
        filled = 0;
    }
    if filled > width {
        filled = width;
    }
    let empty = width - filled;
    format!("[{}{}]", "█".repeat(filled), "░".repeat(empty))
}

/// Mirrors `_bucket_line` (lines 167-179): Format one bucket as a single line.
pub fn bucket_line(label: &str, bucket: &RateLimitBucket) -> String {
    bucket_line_with_width(label, bucket, 14)
}

pub fn bucket_line_with_width(label: &str, bucket: &RateLimitBucket, label_width: usize) -> String {
    if bucket.limit <= 0 {
        return format!("  {label:<label_width$}  (no data)", label_width = label_width);
    }
    let pct = bucket.usage_pct();
    let used = fmt_count_internal(bucket.used());
    let limit = fmt_count_internal(bucket.limit);
    let remaining = fmt_count_internal(bucket.remaining);
    let reset = fmt_seconds_internal(bucket.remaining_seconds_now());
    let b = bar(pct);
    format!(
        "  {label:<label_width$} {b} {pct:5.1}%  {used}/{limit} used  ({remaining} left, resets in {reset})",
        label_width = label_width
    )
}

/// Testable variant accepting explicit `now` for remaining calculation.
pub fn bucket_line_at(label: &str, bucket: &RateLimitBucket, label_width: usize, now: f64) -> String {
    if bucket.limit <= 0 {
        return format!("  {label:<label_width$}  (no data)", label_width = label_width);
    }
    let pct = bucket.usage_pct();
    let used = fmt_count_internal(bucket.used());
    let limit = fmt_count_internal(bucket.limit);
    let remaining = fmt_count_internal(bucket.remaining);
    let reset = fmt_seconds_internal(bucket.remaining_seconds_at(now));
    let b = bar(pct);
    format!(
        "  {label:<label_width$} {b} {pct:5.1}%  {used}/{limit} used  ({remaining} left, resets in {reset})",
        label_width = label_width
    )
}

fn title_case(s: &str) -> String {
    // Mirrors str.title() — word-wise first-letter uppercase, rest lowercase
    s.split_whitespace()
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                None => String::new(),
                Some(first) => {
                    let mut out = first.to_uppercase().to_string();
                    out.push_str(&chars.as_str().to_lowercase());
                    out
                }
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Mirrors `format_rate_limit_display` (lines 182-223).
pub fn format_rate_limit_display(state: &RateLimitState) -> String {
    format_rate_limit_display_at(state, now_secs())
}

/// Testable variant with explicit `now`.
pub fn format_rate_limit_display_at(state: &RateLimitState, now: f64) -> String {
    if !state.has_data() {
        return "No rate limit data yet — make an API request first.".to_string();
    }

    let age = state.age_seconds_at(now);
    let freshness = if age < 5.0 {
        "just now".to_string()
    } else if age < 60.0 {
        format!("{}s ago", age as i64)
    } else {
        format!("{} ago", fmt_seconds_internal(age))
    };

    let provider_label = if state.provider.is_empty() {
        "Provider".to_string()
    } else {
        title_case(&state.provider)
    };

    let mut lines = vec![
        format!("{provider_label} Rate Limits (captured {freshness}):"),
        String::new(),
        bucket_line_at("Requests/min", &state.requests_min, 14, now),
        bucket_line_at("Requests/hr", &state.requests_hour, 14, now),
        String::new(),
        bucket_line_at("Tokens/min", &state.tokens_min, 14, now),
        bucket_line_at("Tokens/hr", &state.tokens_hour, 14, now),
    ];

    // Add warnings if any bucket is getting hot
    let mut warnings: Vec<String> = Vec::new();
    let buckets = [
        ("requests/min", &state.requests_min),
        ("requests/hr", &state.requests_hour),
        ("tokens/min", &state.tokens_min),
        ("tokens/hr", &state.tokens_hour),
    ];
    for (label, bucket) in buckets {
        if bucket.limit > 0 && bucket.usage_pct() >= 80.0 {
            let reset = fmt_seconds_internal(bucket.remaining_seconds_at(now));
            warnings.push(format!("  ⚠ {label} at {:.0}% — resets in {reset}", bucket.usage_pct()));
        }
    }

    if !warnings.is_empty() {
        lines.push(String::new());
        lines.extend(warnings);
    }

    lines.join("\n")
}

/// Mirrors `format_rate_limit_compact` (lines 226-246).
pub fn format_rate_limit_compact(state: &RateLimitState) -> String {
    format_rate_limit_compact_at(state, now_secs())
}

/// Testable variant with explicit `now`.
pub fn format_rate_limit_compact_at(state: &RateLimitState, now: f64) -> String {
    if !state.has_data() {
        return "No rate limit data.".to_string();
    }

    let rm = &state.requests_min;
    let tm = &state.tokens_min;
    let rh = &state.requests_hour;
    let th = &state.tokens_hour;

    let mut parts: Vec<String> = Vec::new();
    if rm.limit > 0 {
        parts.push(format!("RPM: {}/{}", rm.remaining, rm.limit));
    }
    if rh.limit > 0 {
        parts.push(format!(
            "RPH: {}/{} (resets {})",
            fmt_count_internal(rh.remaining),
            fmt_count_internal(rh.limit),
            fmt_seconds_internal(rh.remaining_seconds_at(now))
        ));
    }
    if tm.limit > 0 {
        parts.push(format!(
            "TPM: {}/{}",
            fmt_count_internal(tm.remaining),
            fmt_count_internal(tm.limit)
        ));
    }
    if th.limit > 0 {
        parts.push(format!(
            "TPH: {}/{} (resets {})",
            fmt_count_internal(th.remaining),
            fmt_count_internal(th.limit),
            fmt_seconds_internal(th.remaining_seconds_at(now))
        ));
    }

    parts.join(" | ")
}

// Keep underscore-prefixed aliases for 1:1 traceability with Python private names
#[allow(dead_code)]
fn _fmt_count(n: i64) -> String {
    fmt_count(n)
}
#[allow(dead_code)]
fn _fmt_seconds(s: f64) -> String {
    fmt_seconds(s)
}
#[allow(dead_code)]
fn _bar(pct: f64, width: usize) -> String {
    bar_with_width(pct, width)
}
#[allow(dead_code)]
fn _bucket_line(label: &str, bucket: &RateLimitBucket, label_width: usize) -> String {
    bucket_line_with_width(label, bucket, label_width)
}
