//! Helpers for rendering gateway message timestamps exactly once.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/gateway/message_timestamps.py` (166 LOC).
//! Gateway messages need timestamps in the LLM context for temporal awareness, but
//! persisted message content should stay clean so replay does not accumulate
//! `[timestamp] [timestamp] ...` prefixes across turns.
//!
//! Python source docstring (preserved):
//! ```text
//! Helpers for rendering gateway message timestamps exactly once.
//!
//! Gateway messages need timestamps in the LLM context for temporal awareness, but
//! persisted message content should stay clean so replay does not accumulate
//! ``[timestamp] [timestamp] ...`` prefixes across turns.
//! ```

use chrono::{DateTime, FixedOffset, Local, NaiveDateTime, TimeZone, Utc};

// ---------------------------------------------------------------------------
// TimestampValue — Rust analogue for Python's `Any` ts_value
// ---------------------------------------------------------------------------

/// Generic timestamp input mirroring Python's `Any` for `ts_value`.
///
/// Covers the cases handled by `coerce_message_timestamp`:
/// - `None` → `None` (no timestamp)
/// - `int`/`float` → `Number(f64)`
/// - `datetime` with `.timestamp()` → `DateTime`
/// - `str` (bracketed, ISO, float text) → `Str` / `OwnedString`
#[derive(Debug, Clone)]
pub enum TimestampValue<'a> {
    Number(f64),
    Str(&'a str),
    OwnedString(String),
    DateTime(DateTime<FixedOffset>),
    DateTimeUtc(DateTime<Utc>),
}

impl<'a> From<f64> for TimestampValue<'a> {
    fn from(v: f64) -> Self {
        Self::Number(v)
    }
}
impl<'a> From<&'a str> for TimestampValue<'a> {
    fn from(s: &'a str) -> Self {
        Self::Str(s)
    }
}
impl From<String> for TimestampValue<'_> {
    fn from(s: String) -> Self {
        Self::OwnedString(s)
    }
}
impl<'a> From<DateTime<FixedOffset>> for TimestampValue<'a> {
    fn from(dt: DateTime<FixedOffset>) -> Self {
        Self::DateTime(dt)
    }
}
impl<'a> From<DateTime<Utc>> for TimestampValue<'a> {
    fn from(dt: DateTime<Utc>) -> Self {
        Self::DateTimeUtc(dt)
    }
}

// ---------------------------------------------------------------------------
// Public API — mirrors Python top-level functions
// ---------------------------------------------------------------------------

/// Coerce a timestamp-like value to Unix epoch seconds.
///
/// Accepts Unix epoch numbers, datetime objects, ISO strings, and the gateway's
/// bracketed human-readable timestamp format. Returns `None` when the value
/// cannot be interpreted.
///
/// Mirrors `coerce_message_timestamp(ts_value: Any, tz=None) -> Optional[float]`.
pub fn coerce_message_timestamp(
    ts_value: Option<&TimestampValue<'_>>,
    tz: Option<FixedOffset>,
) -> Option<f64> {
    let v = ts_value?;
    match v {
        TimestampValue::Number(n) => {
            if n.is_finite() {
                Some(*n)
            } else {
                None
            }
        }
        TimestampValue::DateTime(dt) => Some(dt.timestamp() as f64 + dt.timestamp_subsec_nanos() as f64 / 1e9),
        TimestampValue::DateTimeUtc(dt) => Some(dt.timestamp() as f64 + dt.timestamp_subsec_nanos() as f64 / 1e9),
        TimestampValue::Str(s) => coerce_message_timestamp_str(s, tz),
        TimestampValue::OwnedString(s) => coerce_message_timestamp_str(s, tz),
    }
}

/// String-specialised coerce — handles the `str` branch of `coerce_message_timestamp`.
///
/// Mirrors the `isinstance(ts_value, str)` block in Python.
pub fn coerce_message_timestamp_str(text: &str, tz: Option<FixedOffset>) -> Option<f64> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    // 1. bracketed prefix
    if let Some(v) = parse_timestamp_prefix(trimmed, tz) {
        return Some(v);
    }
    // 2. float text
    if let Ok(f) = trimmed.parse::<f64>() {
        if f.is_finite() {
            return Some(f);
        }
    }
    // 3. fromisoformat / strptime fallbacks for bare ISO strings
    if let Some(v) = parse_iso_like(trimmed, tz) {
        return Some(v);
    }
    None
}

/// Convenience for `serde_json::Value` callers (gateway stores timestamps as JSON numbers/strings).
///
/// Mirrors `coerce_message_timestamp` when `ts_value` comes from `msg.get("timestamp")`.
pub fn coerce_message_timestamp_value(
    value: &serde_json::Value,
    tz: Option<FixedOffset>,
) -> Option<f64> {
    match value {
        serde_json::Value::Null => None,
        serde_json::Value::Number(n) => n.as_f64().filter(|v| v.is_finite()),
        serde_json::Value::String(s) => coerce_message_timestamp_str(s, tz),
        _ => None,
    }
}

/// Format a timestamp value as `[Tue 2026-04-28 13:40:53 CEST]`.
///
/// Mirrors `format_message_timestamp(ts_value: Any, tz=None) -> str`.
pub fn format_message_timestamp(
    ts_value: Option<&TimestampValue<'_>>,
    tz: Option<FixedOffset>,
) -> String {
    let epoch = coerce_message_timestamp(ts_value, tz);
    format_message_timestamp_epoch(epoch, tz)
}

/// `serde_json::Value` variant of `format_message_timestamp`.
pub fn format_message_timestamp_value(
    value: &serde_json::Value,
    tz: Option<FixedOffset>,
) -> String {
    let epoch = coerce_message_timestamp_value(value, tz);
    format_message_timestamp_epoch(epoch, tz)
}

/// Core formatter that renders an epoch (or `None`) as a bracketed timestamp.
///
/// Returns `""` when `epoch` is `None` — mirrors Python's early return.
pub fn format_message_timestamp_epoch(epoch: Option<f64>, tz: Option<FixedOffset>) -> String {
    let epoch = match epoch {
        Some(v) if v.is_finite() => v,
        _ => return String::new(),
    };
    // floor-based split to handle negative epochs correctly (Python handles negatives via datetime)
    let secs = epoch.floor() as i64;
    let nsecs = ((epoch - secs as f64) * 1e9).round() as u32;
    // clamp nsecs to valid range after rounding
    let (secs, nsecs) = if nsecs >= 1_000_000_000 {
        (secs + 1, nsecs - 1_000_000_000)
    } else {
        (secs, nsecs)
    };
    let utc = match Utc.timestamp_opt(secs, nsecs) {
        chrono::LocalResult::Single(dt) => dt,
        _ => return String::new(),
    };
    let rendered = if let Some(offset) = tz {
        let dt = utc.with_timezone(&offset);
        dt.format("%a %Y-%m-%d %H:%M:%S %Z").to_string()
    } else {
        let dt = utc.with_timezone(&Local);
        dt.format("%a %Y-%m-%d %H:%M:%S %Z").to_string()
    };
    format!("[{}]", rendered)
}

/// Strip one or more leading gateway timestamp prefixes from `content`.
///
/// Returns `(clean_content, embedded_epoch)`. If multiple timestamp prefixes
/// are present, the timestamp closest to the actual message text wins. That
/// preserves the original platform-send time for legacy contaminated rows like
/// `[processing time] [platform time] [sender] message`.
///
/// Mirrors `strip_leading_message_timestamps(content: str, tz=None) -> Tuple[str, Optional[float]]`.
pub fn strip_leading_message_timestamps(
    content: &str,
    tz: Option<FixedOffset>,
) -> (String, Option<f64>) {
    if content.is_empty() {
        return (content.to_string(), None);
    }
    let mut text = content;
    let mut embedded_epoch: Option<f64> = None;

    loop {
        match match_human_or_iso_prefix(text) {
            None => break,
            Some((consumed_len, date_opt, time_opt, iso_opt)) => {
                // Try to parse the matched prefix to epoch
                let parsed = if let Some(iso) = iso_opt {
                    parse_iso_text(&iso, tz)
                } else if let (Some(d), Some(t)) = (date_opt, time_opt) {
                    human_to_epoch(&d, &t, tz)
                } else {
                    None
                };
                if let Some(v) = parsed {
                    embedded_epoch = Some(v);
                }
                // Always consume the syntactic match, even if parse failed (mirrors Python)
                if consumed_len >= text.len() {
                    text = "";
                    break;
                } else {
                    text = &text[consumed_len..];
                }
            }
        }
    }

    (text.to_string(), embedded_epoch)
}

/// Render a user message for LLM context with exactly one timestamp prefix.
///
/// Existing leading timestamp prefixes are removed first. If such a prefix was
/// present, its parsed time wins over `ts_value`; otherwise `ts_value` is
/// formatted and prepended. If no timestamp is available, the cleaned content is
/// returned unchanged.
///
/// Mirrors `render_user_content_with_timestamp(content: str, ts_value: Any = None, tz=None) -> str`.
pub fn render_user_content_with_timestamp(
    content: &str,
    ts_value: Option<&TimestampValue<'_>>,
    tz: Option<FixedOffset>,
) -> String {
    let (clean_content, embedded_epoch) = strip_leading_message_timestamps(content, tz);
    let effective_epoch = if let Some(e) = embedded_epoch {
        Some(e)
    } else {
        coerce_message_timestamp(ts_value, tz)
    };
    let prefix = format_message_timestamp_epoch(effective_epoch, tz);
    if prefix.is_empty() {
        return clean_content;
    }
    if clean_content.is_empty() {
        return prefix;
    }
    format!("{} {}", prefix, clean_content)
}

/// `serde_json::Value` variant of `render_user_content_with_timestamp`.
pub fn render_user_content_with_timestamp_value(
    content: &str,
    ts_value: Option<&serde_json::Value>,
    tz: Option<FixedOffset>,
) -> String {
    let (clean_content, embedded_epoch) = strip_leading_message_timestamps(content, tz);
    let effective_epoch = if let Some(e) = embedded_epoch {
        Some(e)
    } else if let Some(v) = ts_value {
        coerce_message_timestamp_value(v, tz)
    } else {
        None
    };
    let prefix = format_message_timestamp_epoch(effective_epoch, tz);
    if prefix.is_empty() {
        return clean_content;
    }
    if clean_content.is_empty() {
        return prefix;
    }
    format!("{} {}", prefix, clean_content)
}

/// Convenience epoch variant for callers that already have `Option<f64>`.
pub fn render_user_content_with_timestamp_epoch(
    content: &str,
    epoch: Option<f64>,
    tz: Option<FixedOffset>,
) -> String {
    let (clean_content, embedded_epoch) = strip_leading_message_timestamps(content, tz);
    let effective_epoch = embedded_epoch.or(epoch);
    let prefix = format_message_timestamp_epoch(effective_epoch, tz);
    if prefix.is_empty() {
        return clean_content;
    }
    if clean_content.is_empty() {
        return prefix;
    }
    format!("{} {}", prefix, clean_content)
}

// ---------------------------------------------------------------------------
// Internal helpers — mirrors `_parse_timestamp_prefix` / `_parse_timestamp_match`
// ---------------------------------------------------------------------------

/// Mirrors `_parse_timestamp_prefix(text: str, tz=None) -> Optional[float]`.
fn parse_timestamp_prefix(text: &str, tz: Option<FixedOffset>) -> Option<f64> {
    let (consumed, date_opt, time_opt, iso_opt) = match_human_or_iso_prefix(text)?;
    // Ensure the match is at the start (match_human_or_iso_prefix already does)
    let _ = consumed;
    if let Some(iso) = iso_opt {
        parse_iso_text(&iso, tz)
    } else if let (Some(d), Some(t)) = (date_opt, time_opt) {
        human_to_epoch(&d, &t, tz)
    } else {
        None
    }
}

/// Result of syntactic prefix match: (consumed_bytes, date, time, iso).
/// `iso` is Some when ISO pattern matched; otherwise date/time are Some for human.
fn match_human_or_iso_prefix(text: &str) -> Option<(usize, Option<String>, Option<String>, Option<String>)> {
    if let Some(m) = try_match_human(text) {
        return Some(m);
    }
    if let Some(m) = try_match_iso(text) {
        return Some(m);
    }
    None
}

/// Try to match human timestamp prefix `^[Tue 2026-04-28 13:40:53 ...]`.
/// Returns (consumed_len, Some(date), Some(time), None) on syntactic match.
fn try_match_human(text: &str) -> Option<(usize, Option<String>, Option<String>, Option<String>)> {
    // Minimal length: "[Tue 2026-04-28 13:40:53]" = 25
    if !text.starts_with('[') {
        return None;
    }
    // Find closing bracket
    let closing = text.find(']')?;
    // inner between brackets
    let inner = &text[1..closing];
    // Need at least "Dow YYYY-MM-DD HH:MM:SS"
    // Strict positional check to mirror regex `^\[(?P<dow>[A-Z][a-z]{2}) (?P<date>\d{4}-\d{2}-\d{2}) (?P<time>\d{2}:\d{2}:\d{2})(?: (?P<tz>[A-Za-z0-9_+\-/:]+))?\]`
    let inner_bytes = inner.as_bytes();
    if inner_bytes.len() < 3 + 1 + 10 + 1 + 8 {
        return None;
    }
    // dow check
    if !(inner_bytes[0].is_ascii_uppercase()
        && inner_bytes[1].is_ascii_lowercase()
        && inner_bytes[2].is_ascii_lowercase())
    {
        return None;
    }
    if inner_bytes[3] != b' ' {
        return None;
    }
    // date check
    let date_part = &inner[4..14];
    if !is_date_str(date_part) {
        return None;
    }
    if inner_bytes[14] != b' ' {
        return None;
    }
    // time check
    if inner_bytes.len() < 23 {
        return None;
    }
    let time_part = &inner[15..23];
    if !is_time_str(time_part) {
        return None;
    }
    // After time, either end or space + tz
    let rest = &inner[23..];
    if rest.is_empty() {
        // no tz
        let trailing = count_leading_ws(&text[closing + 1..]);
        let consumed = closing + 1 + trailing;
        return Some((consumed, Some(date_part.to_string()), Some(time_part.to_string()), None));
    } else if rest.starts_with(' ') {
        let tz_candidate = &rest[1..];
        if tz_candidate.is_empty() {
            return None;
        }
        // tz must match [A-Za-z0-9_+\-/:]+ and contain no spaces
        if !tz_candidate
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '+' | '-' | '/' | ':'))
        {
            return None;
        }
        // tz token must not contain spaces — we already ensured rest has no extra spaces
        // because rest is the remainder after time; if it contains space, it would be second space
        // But original regex allows only one tz token, so any space inside tz fails
        if tz_candidate.contains(' ') {
            return None;
        }
        let trailing = count_leading_ws(&text[closing + 1..]);
        let consumed = closing + 1 + trailing;
        return Some((consumed, Some(date_part.to_string()), Some(time_part.to_string()), None));
        // Note: we intentionally ignore the tz string value — Python's _parse_timestamp_match
        // ignores the captured tz and uses the passed-in `tz` param instead.
    } else {
        return None;
    }
}

/// Try to match ISO timestamp prefix `^[2026-04-13T...]`.
fn try_match_iso(text: &str) -> Option<(usize, Option<String>, Option<String>, Option<String>)> {
    if !text.starts_with('[') {
        return None;
    }
    let closing = text.find(']')?;
    let inner = &text[1..closing];
    // iso pattern: ^\d{4}-\d{2}-\d{2}T[^\]]+
    if inner.len() < 11 {
        return None;
    }
    let bytes = inner.as_bytes();
    if !(bytes[0].is_ascii_digit()
        && bytes[1].is_ascii_digit()
        && bytes[2].is_ascii_digit()
        && bytes[3].is_ascii_digit()
        && bytes[4] == b'-'
        && bytes[5].is_ascii_digit()
        && bytes[6].is_ascii_digit()
        && bytes[7] == b'-'
        && bytes[8].is_ascii_digit()
        && bytes[9].is_ascii_digit()
        && bytes[10] == b'T')
    {
        return None;
    }
    // Ensure at least one char after T
    if inner.len() <= 11 {
        return None;
    }
    // inner[11..] must contain at least one non-']' char — by definition it does until closing
    // And regex is ^\[(?P<iso>\d{4}-\d{2}-\d{2}T[^\]]+)\]\s*
    // So if we reached here, it's a syntactic iso match
    let iso_part = inner.to_string();
    let trailing = count_leading_ws(&text[closing + 1..]);
    let consumed = closing + 1 + trailing;
    Some((consumed, None, None, Some(iso_part)))
}

fn is_date_str(s: &str) -> bool {
    if s.len() != 10 {
        return false;
    }
    let b = s.as_bytes();
    b[4] == b'-'
        && b[7] == b'-'
        && b[0].is_ascii_digit()
        && b[1].is_ascii_digit()
        && b[2].is_ascii_digit()
        && b[3].is_ascii_digit()
        && b[5].is_ascii_digit()
        && b[6].is_ascii_digit()
        && b[8].is_ascii_digit()
        && b[9].is_ascii_digit()
}

fn is_time_str(s: &str) -> bool {
    if s.len() != 8 {
        return false;
    }
    let b = s.as_bytes();
    b[2] == b':' && b[5] == b':' && b[0].is_ascii_digit() && b[1].is_ascii_digit() && b[3].is_ascii_digit() && b[4].is_ascii_digit() && b[6].is_ascii_digit() && b[7].is_ascii_digit()
}

fn count_leading_ws(s: &str) -> usize {
    let mut count = 0;
    for c in s.chars() {
        if c.is_whitespace() {
            count += c.len_utf8();
        } else {
            break;
        }
    }
    count
}

// ---------------------------------------------------------------------------
// Epoch conversion helpers
// ---------------------------------------------------------------------------

fn human_to_epoch(date_part: &str, time_part: &str, tz: Option<FixedOffset>) -> Option<f64> {
    let naive_str = format!("{} {}", date_part, time_part);
    let naive = NaiveDateTime::parse_from_str(&naive_str, "%Y-%m-%d %H:%M:%S").ok()?;
    naive_to_epoch(naive, tz)
}

fn naive_to_epoch(naive: NaiveDateTime, tz: Option<FixedOffset>) -> Option<f64> {
    if let Some(offset) = tz {
        let dt = offset.from_local_datetime(&naive).single()?;
        Some(dt.timestamp() as f64 + dt.timestamp_subsec_nanos() as f64 / 1e9)
    } else {
        let dt = Local.from_local_datetime(&naive).single()?;
        Some(dt.timestamp() as f64 + dt.timestamp_subsec_nanos() as f64 / 1e9)
    }
}

fn parse_iso_text(iso_text: &str, tz: Option<FixedOffset>) -> Option<f64> {
    // Try RFC3339 (handles +02:00, Z, fractional)
    if let Ok(dt) = DateTime::parse_from_rfc3339(iso_text) {
        return Some(dt.timestamp() as f64 + dt.timestamp_subsec_nanos() as f64 / 1e9);
    }
    // Try Z suffix manually (chrono's RFC3339 already handles Z, but try explicit)
    if iso_text.ends_with('Z') {
        let with_offset = format!("{}+00:00", &iso_text[..iso_text.len() - 1]);
        if let Ok(dt) = DateTime::parse_from_rfc3339(&with_offset) {
            return Some(dt.timestamp() as f64 + dt.timestamp_subsec_nanos() as f64 / 1e9);
        }
    }
    // Try %Y-%m-%dT%H:%M:%S%z  (handles +0200)
    if let Ok(dt) = DateTime::parse_from_str(iso_text, "%Y-%m-%dT%H:%M:%S%z") {
        return Some(dt.timestamp() as f64 + dt.timestamp_subsec_nanos() as f64 / 1e9);
    }
    if let Ok(dt) = DateTime::parse_from_str(iso_text, "%Y-%m-%dT%H:%M:%S%.f%z") {
        return Some(dt.timestamp() as f64 + dt.timestamp_subsec_nanos() as f64 / 1e9);
    }
    // Try %:z variants (colon)
    if let Ok(dt) = DateTime::parse_from_str(iso_text, "%Y-%m-%dT%H:%M:%S%:z") {
        return Some(dt.timestamp() as f64 + dt.timestamp_subsec_nanos() as f64 / 1e9);
    }
    if let Ok(dt) = DateTime::parse_from_str(iso_text, "%Y-%m-%dT%H:%M:%S%.f%:z") {
        return Some(dt.timestamp() as f64 + dt.timestamp_subsec_nanos() as f64 / 1e9);
    }
    // Try without colon with fractional already handled, try naive + tz
    if let Ok(naive) = NaiveDateTime::parse_from_str(iso_text, "%Y-%m-%dT%H:%M:%S%.f") {
        return naive_to_epoch(naive, tz);
    }
    if let Ok(naive) = NaiveDateTime::parse_from_str(iso_text, "%Y-%m-%dT%H:%M:%S") {
        return naive_to_epoch(naive, tz);
    }
    None
}

/// Handle bare ISO-like strings outside brackets (Python's `fromisoformat` / `strptime` fallback).
fn parse_iso_like(text: &str, tz: Option<FixedOffset>) -> Option<f64> {
    // Try full RFC3339 first (covers fromisoformat with offset)
    if let Some(v) = parse_iso_text(text, tz) {
        return Some(v);
    }
    // Python also tries `datetime.fromisoformat(text)` which handles `YYYY-MM-DDTHH:MM:SS[.ffffff][+HH:MM[:SS[.ffffff]]]`
    // Our parse_iso_text already covers the common cases.
    // Try `YYYY-MM-DD HH:MM:SS` naive (though Python's fromisoformat for gateway timestamps is T-focused)
    if let Ok(naive) = NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S") {
        return naive_to_epoch(naive, tz);
    }
    if let Ok(naive) = NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S%.f") {
        return naive_to_epoch(naive, tz);
    }
    // Try date-only? Python's fromisoformat can handle date, but gateway timestamps are datetime — skip
    None
}

// Provide private aliases mirroring Python's underscore-prefixed helpers for traceability
#[allow(dead_code)]
fn _parse_timestamp_prefix(text: &str, tz: Option<FixedOffset>) -> Option<f64> {
    parse_timestamp_prefix(text, tz)
}

#[allow(dead_code)]
fn _parse_timestamp_match_iso(iso_text: &str, tz: Option<FixedOffset>) -> Option<f64> {
    parse_iso_text(iso_text, tz)
}

#[allow(dead_code)]
fn _parse_timestamp_match_human(date: &str, time: &str, tz: Option<FixedOffset>) -> Option<f64> {
    human_to_epoch(date, time, tz)
}
