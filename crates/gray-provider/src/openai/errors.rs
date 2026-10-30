use super::*;

pub(super) fn error_body_message(body: &str) -> Option<String> {
    let v: Value = serde_json::from_str(body.trim()).ok()?;
    let msg = v
        .pointer("/error/message")
        .or_else(|| v.get("message"))
        .and_then(Value::as_str)?
        .trim();
    (!msg.is_empty()).then(|| msg.to_string())
}

pub(super) const QUOTA_EXHAUSTED_HINTS: [&str; 14] = [
    "insufficient_quota",
    "subscription_sharing_usage_limit_exceeded",
    "insufficient quota",
    "insufficient credits",
    "insufficient balance",
    "exceeded your current quota",
    "exceeds your allocated quota",
    "quota exceeded",
    "quota_exceeded",
    "gousagelimiterror",
    "available balance",
    "usage limit reached",
    "session limit",
    "resets in ~",
];

pub const QUOTA_VENDOR_CODE: &str = "quota_exhausted";

pub(super) fn is_quota_code(code: &str) -> bool {
    code.split(';').next() == Some(QUOTA_VENDOR_CODE)
}

pub(super) fn quota_hinted(text: &str) -> bool {
    let lower = text.to_lowercase();
    QUOTA_EXHAUSTED_HINTS.iter().any(|h| lower.contains(h))
}

pub(super) fn quota_vendor_code(orig: Option<String>, resets_in: Option<Duration>) -> String {
    let mut code = String::from(QUOTA_VENDOR_CODE);
    if let Some(orig) = orig.filter(|s| !s.is_empty()) {
        code.push_str(&format!(";code={}", orig.replace([';', '='], "_")));
    }
    if let Some(d) = resets_in {
        code.push_str(&format!(";reset_in={}", d.as_secs()));
    }
    code
}

pub fn is_quota_exhausted(err: &ProviderError) -> bool {
    if let Some(code) = err.detail().and_then(|d| d.vendor_code.as_deref())
        && is_quota_code(code)
    {
        return true;
    }
    matches!(err.class(), ProviderError::Auth(msg) | ProviderError::BadRequest(msg) | ProviderError::RateLimited(msg) if quota_hinted(msg))
}

pub fn quota_reset_in(err: &ProviderError) -> Option<Duration> {
    if let Some(code) = err.detail().and_then(|d| d.vendor_code.as_deref())
        && is_quota_code(code)
    {
        return code
            .split(';')
            .skip(1)
            .find_map(|p| p.strip_prefix("reset_in="))
            .and_then(|v| v.parse::<u64>().ok())
            .map(Duration::from_secs);
    }
    if is_quota_exhausted(err) {
        return parse_quota_reset_delay(&err.to_string());
    }
    None
}

pub(super) fn error_body_code(body: &str) -> Option<String> {
    let v: Value = serde_json::from_str(body.trim()).ok()?;
    v.pointer("/error/code")
        .or_else(|| v.pointer("/error/type"))
        .and_then(|v| {
            v.as_str()
                .map(str::to_string)
                .or_else(|| v.as_u64().map(|n| n.to_string()))
        })
        .filter(|s| !s.is_empty())
}

pub(crate) fn classify_http_error(
    status: reqwest::StatusCode,
    snippet: &str,
    cf_ray: Option<&str>,
    req_id: Option<&str>,
) -> ProviderError {
    let class = classify_http_error_inner(status, snippet, cf_ray, req_id);
    let vendor_message = error_body_message(snippet);
    let mut vendor_code = error_body_code(snippet);
    let mut resets_in = None;
    if matches!(
        class.class(),
        ProviderError::Auth(_) | ProviderError::BadRequest(_)
    ) && quota_hinted(snippet)
    {
        resets_in = vendor_message
            .as_deref()
            .and_then(parse_quota_reset_delay)
            .or_else(|| parse_quota_reset_delay(snippet));
        vendor_code = Some(quota_vendor_code(vendor_code, resets_in));
    }
    ProviderError::Detailed {
        class: Box::new(class),
        detail: Box::new(gray_core::error::ProviderDetail {
            status: Some(status.as_u16()),
            vendor_code,
            vendor_message,
            cf_ray: cf_ray.map(str::to_string),
            request_id: req_id.map(str::to_string),
            retry_after_ms: resets_in.map(|d| d.as_millis().min(u128::from(u64::MAX)) as u64),
        }),
    }
}

pub(super) fn classify_http_error_inner(
    status: reqwest::StatusCode,
    snippet: &str,
    cf_ray: Option<&str>,
    req_id: Option<&str>,
) -> ProviderError {
    let shown = error_body_message(snippet).unwrap_or_else(|| snippet.to_string());
    let mut msg = if shown.is_empty() {
        format!("status {status}")
    } else {
        format!("status {status}: {shown}")
    };
    if let Some(ray) = cf_ray {
        msg.push_str(&format!(", cf-ray: {ray}"));
    }
    if let Some(rid) = req_id {
        msg.push_str(&format!(", request-id: {rid}"));
    }
    let lower = snippet.to_lowercase();

    if status == reqwest::StatusCode::FORBIDDEN
        && cf_ray.is_some()
        && lower.trim_start().starts_with("error code: 1")
    {
        return ProviderError::ServerError(format!("provider edge blocked the request ({msg})"));
    }

    const CONTEXT_OVERFLOW_HINTS: [&str; 4] = [
        "context length",
        "context window",
        "too many tokens",
        "prompt is too long",
    ];
    let max_tokens_qualified = lower.contains("max_tokens")
        && (lower.contains("context") || lower.contains("too long") || lower.contains("exceeded"));
    if !gray_core::overflow::is_non_overflow_message(snippet)
        && (CONTEXT_OVERFLOW_HINTS.iter().any(|h| lower.contains(h))
            || max_tokens_qualified
            || gray_core::overflow::is_context_overflow_message(snippet))
    {
        return ProviderError::ContextOverflow(format!(
            "context exhausted — start /new or compact ({msg})"
        ));
    }
    const CONTENT_FILTER_HINTS: [&str; 3] = ["content_filter", "content-filter", "usage policies"];
    if CONTENT_FILTER_HINTS.iter().any(|h| lower.contains(h)) {
        return ProviderError::BadRequest(msg);
    }
    let is_unsupported = lower.contains("not supported")
        || lower.contains("unsupported")
        || lower.contains("model not found")
        || lower.contains("unknown model");

    if is_unsupported {
        return ProviderError::BadRequest(msg);
    }
    let quota_exhausted = QUOTA_EXHAUSTED_HINTS.iter().any(|h| lower.contains(h));
    match status.as_u16() {
        401 | 403 => ProviderError::Auth(msg),

        402 => ProviderError::Auth(msg),
        429 if quota_exhausted => ProviderError::Auth(msg),
        429 => ProviderError::RateLimited(msg),

        413 => ProviderError::ContextOverflow(format!(
            "context exhausted — start /new or compact ({msg})"
        )),
        400 | 404 => ProviderError::BadRequest(msg),

        500..=599 => ProviderError::ServerError(format!("provider-side {msg} (request was valid)")),
        _ => ProviderError::Stream(msg),
    }
}

pub(crate) fn is_retryable_error(err: &ProviderError) -> bool {
    err.retryable()
}

pub(super) fn should_retry_request(err: &ProviderError, http_status: u16, attempt: usize) -> bool {
    if is_quota_exhausted(err) {
        return false;
    }
    if is_retryable_error(err) {
        return true;
    }
    http_status == 403 && attempt == 1 && matches!(err.class(), ProviderError::Auth(_))
}

pub(super) fn is_midstream_transport(err: &SseReadError) -> bool {
    matches!(
        err,
        SseReadError::Upstream(eventsource_stream::EventStreamError::Transport(_))
    )
}

pub(crate) fn retry_notice_event(attempt: usize, max: usize, err: &ProviderError) -> StreamEvent {
    let details: String = err.to_string().chars().take(200).collect();
    StreamEvent::stream_error(format!("Reconnecting... {attempt}/{max}"), details)
}

pub(super) const MAX_RETRY_AFTER: Duration = Duration::from_secs(60);

pub(crate) fn parse_retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    if let Some(ms) = headers
        .get("retry-after-ms")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<u64>().ok())
    {
        return Some(MAX_RETRY_AFTER.min(Duration::from_millis(ms)));
    }
    let raw = headers
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim();
    if let Ok(secs) = raw.parse::<u64>() {
        return Some(MAX_RETRY_AFTER.min(Duration::from_secs(secs)));
    }
    parse_http_date_delay(raw).map(|d| d.min(MAX_RETRY_AFTER))
}

pub(super) fn parse_http_date_delay(raw: &str) -> Option<Duration> {
    let raw = match raw.strip_suffix(" UTC") {
        Some(rest) => format!("{rest} GMT"),
        None => raw.to_string(),
    };
    let target = chrono::DateTime::parse_from_rfc2822(&raw).ok()?;
    Some(delay_until(target.timestamp()))
}

pub(super) fn delay_until(epoch: i64) -> Duration {
    Duration::from_secs((epoch - chrono::Utc::now().timestamp()).max(0) as u64)
}

pub(super) fn parse_quota_reset_delay(text: &str) -> Option<Duration> {
    let lower = text.to_lowercase();
    let mut offset = 0;
    while let Some(at) = lower[offset..].find("reset") {
        let tail = lower[offset + at + 5..].trim_start();
        offset += at + 5;
        let tail = tail.strip_prefix('s').map(str::trim_start).unwrap_or(tail);
        let tail = tail
            .strip_prefix("back")
            .map(str::trim_start)
            .unwrap_or(tail);
        for prep in ["in", "at", "on"] {
            let Some(rest) = tail.strip_prefix(prep).map(str::trim_start) else {
                continue;
            };
            let rest = rest.trim_start_matches(['~', '\u{2248}', ' ']);
            let rest = rest
                .strip_prefix("about")
                .map(str::trim_start)
                .unwrap_or(rest);
            if let Some(d) = parse_reset_duration(rest)
                .or_else(|| parse_reset_timestamp(rest))
                .or_else(|| parse_http_date_delay(rest))
            {
                return Some(d);
            }
        }
    }
    None
}

pub(super) fn parse_reset_duration(s: &str) -> Option<Duration> {
    let nlen = s
        .bytes()
        .take_while(|b| b.is_ascii_digit() || *b == b'.')
        .count();
    if nlen == 0 {
        return None;
    }
    let num: f64 = s[..nlen].parse().ok()?;
    let unit = s[nlen..].trim_start();
    let ulen = unit.bytes().take_while(|b| b.is_ascii_alphabetic()).count();
    let secs: u64 = match &unit[..ulen] {
        "s" | "sec" | "secs" | "second" | "seconds" => 1,
        "m" | "min" | "mins" | "minute" | "minutes" => 60,
        "h" | "hr" | "hrs" | "hour" | "hours" => 3600,
        "d" | "day" | "days" => 86400,
        "w" | "week" | "weeks" => 604800,
        _ => return None,
    };
    Some(Duration::from_secs_f64(num * secs as f64))
}

pub(super) fn parse_reset_timestamp(s: &str) -> Option<Duration> {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs() as i64;
    let digits = s.bytes().take_while(|b| b.is_ascii_digit()).count();
    if digits >= 10 {
        let raw: i64 = s[..digits].parse().ok()?;
        let epoch = if digits >= 13 { raw / 1000 } else { raw };
        return Some(Duration::from_secs(epoch.saturating_sub(now).max(0) as u64));
    }
    if let Some(epoch) = parse_iso_reset_epoch(s) {
        return Some(Duration::from_secs(epoch.saturating_sub(now).max(0) as u64));
    }
    let (clock, _) = parse_reset_clock(s)?;
    let today = now - now.rem_euclid(86400);
    let mut target = today + clock;
    if target <= now {
        target += 86400;
    }
    Some(Duration::from_secs((target - now) as u64))
}

pub(super) fn parse_iso_reset_epoch(s: &str) -> Option<i64> {
    let date = chrono::NaiveDate::parse_from_str(s.get(..10)?, "%Y-%m-%d").ok()?;
    let rest = s[10..].trim_start_matches(['T', ' ']);
    let (clock, used) = if rest.as_bytes().first().is_some_and(u8::is_ascii_digit) {
        parse_reset_clock(rest)?
    } else {
        (0, 0)
    };
    let offset = parse_reset_tz_offset(rest[used..].trim_start());
    Some(date.and_hms_opt(0, 0, 0)?.and_utc().timestamp() + clock - offset)
}

pub(super) fn parse_reset_clock(s: &str) -> Option<(i64, usize)> {
    let b = s.as_bytes();
    if b.len() < 5 || b[2] != b':' {
        return None;
    }
    let hour: i64 = s[0..2].parse().ok()?;
    let min: i64 = s[3..5].parse().ok()?;
    if hour > 23 || min > 59 {
        return None;
    }
    let mut sec = 0i64;
    let mut used = 5;
    if b.get(5) == Some(&b':') {
        sec = s.get(6..8)?.parse().ok()?;
        if sec > 60 {
            return None;
        }
        used = 8;
        if b.get(8) == Some(&b'.') {
            used += 1 + s[9..].bytes().take_while(|b| b.is_ascii_digit()).count();
        }
    }
    Some((hour * 3600 + min * 60 + sec, used))
}

pub(super) fn parse_reset_tz_offset(s: &str) -> i64 {
    let (sign, rest) = match s.as_bytes().first() {
        Some(b'+') => (1i64, &s[1..]),
        Some(b'-') => (-1i64, &s[1..]),
        _ => return 0,
    };
    let raw_len = rest
        .bytes()
        .take_while(|b| b.is_ascii_digit() || *b == b':')
        .count();
    let raw = &rest[..raw_len];
    let (hour, min) = match raw.split_once(':') {
        Some((h, m)) => (h.parse().unwrap_or(0), m.parse().unwrap_or(0)),
        None if raw.len() > 2 => (
            raw[..raw.len() - 2].parse().unwrap_or(0),
            raw[raw.len() - 2..].parse().unwrap_or(0),
        ),
        None => (raw.parse().unwrap_or(0), 0),
    };
    sign * (hour * 3600 + min * 60)
}

pub(crate) fn backoff_delay(
    initial: Duration,
    attempt: usize,
    retry_after: Option<Duration>,
) -> Duration {
    let exp_factor = 1u64 << (attempt.saturating_sub(1).min(10));
    let backoff_ms = (initial.as_millis() as u64).saturating_mul(exp_factor);
    let max_jitter = backoff_ms / 2;
    let jitter_ms = if max_jitter > 0 {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.subsec_nanos() as u64)
            .unwrap_or(0);
        nanos % (max_jitter + 1)
    } else {
        0
    };
    let base = Duration::from_millis(backoff_ms + jitter_ms).min(MAX_BACKOFF);
    retry_after.map(|floor| base.max(floor)).unwrap_or(base)
}

pub(super) fn rate_limit_floor(failed_attempt: usize) -> Duration {
    Duration::from_secs(5 * (1u64 << failed_attempt.saturating_sub(1).min(6)))
}

pub(super) fn retry_floor(
    err: &ProviderError,
    floor: Option<Duration>,
    failed_attempt: usize,
) -> Option<Duration> {
    if floor.is_none() && matches!(err.class(), ProviderError::RateLimited(_)) {
        return Some(rate_limit_floor(failed_attempt));
    }
    floor
}
