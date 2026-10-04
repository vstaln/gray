use gray_core::error::CoreError;
use gray_core::message::Message;

/// Truncates a string slice to at most `max_chars` unicode scalar values / chars.
pub(crate) fn truncate_chars(s: &str, max_chars: usize) -> &str {
    match s.char_indices().nth(max_chars) {
        Some((idx, _)) => &s[..idx],
        None => s,
    }
}

/// Formats a token count with comma separators (e.g., 1000 -> 1,000).
pub fn fmt_usage(total: usize) -> String {
    let s = total.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (count, ch) in s.chars().rev().enumerate() {
        if count != 0 && count % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out.chars().rev().collect()
}

/// Codex steal (agent-loop.ts): pull Status/Code/Type/Message out of a
/// `status 503: {"error":{"message":..,"type":..,"code":..}}` blob so the UI
/// never dumps raw `{"model":..,"param":null}` JSON. Returns a short human
/// line; falls back to the raw detail when no JSON is found.
pub fn clean_provider_detail(detail: &str) -> String {
    let start = match detail.find('{') {
        Some(i) => i,
        None => return detail.to_string(),
    };
    let end = match detail.rfind('}') {
        Some(i) if i > start => i,
        _ => return detail.to_string(),
    };
    let parsed: serde_json::Value = match serde_json::from_str(&detail[start..=end]) {
        Ok(v) => v,
        Err(_) => return detail.to_string(),
    };
    // Shape is usually {"model":..,"error":{"message","type","code"}} or just {"error":..}.
    let err_obj = parsed.get("error").unwrap_or(&parsed);
    let message = err_obj
        .get("message")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    if message.is_empty() {
        return detail.to_string();
    }
    let typ = err_obj.get("type").and_then(|v| v.as_str()).unwrap_or("");
    let code = err_obj
        .get("code")
        .and_then(|v| {
            v.as_str()
                .map(|s| s.to_string())
                .or_else(|| v.as_u64().map(|n| n.to_string()))
        })
        .unwrap_or_default();
    let prefix = detail[..start]
        .trim()
        .trim_end_matches(':')
        .trim_end()
        .to_string();
    // Preserve trailing diagnostics Codex keeps (cf-ray / request-id).
    let mut suffix = String::new();
    for key in ["cf-ray: ", "request-id: ", "request_id: "] {
        if let Some(pos) = detail[end..].find(key) {
            let tail = detail[end + pos + key.len()..].trim();
            let val: String = tail
                .chars()
                .take_while(|c| !c.is_whitespace() && *c != ',')
                .collect();
            if !val.is_empty() {
                let label = if key.starts_with("cf") {
                    "cf-ray"
                } else {
                    "request-id"
                };
                if !suffix.is_empty() {
                    suffix.push_str(", ");
                }
                suffix.push_str(&format!("{label}: {val}"));
            }
        }
    }
    let mut out = if prefix.is_empty() {
        message.to_string()
    } else {
        format!("{prefix}: {message}")
    };
    if !typ.is_empty() || !code.is_empty() {
        let mut meta = vec![];
        if !typ.is_empty() {
            meta.push(format!("type: {typ}"));
        }
        if !code.is_empty() {
            meta.push(format!("code: {code}"));
        }
        out.push_str(&format!(" ({})", meta.join(", ")));
    }
    if !suffix.is_empty() {
        out.push_str(&format!(", {suffix}"));
    }
    out
}

/// Formats a [`CoreError`] for REPL display.
/// Connection/timeout failures get a friendly, actionable message with the
/// provider's `base_url`; all other errors fall back to the generic prefix.
pub fn format_core_error(e: &CoreError, base_url: &str) -> String {
    match e {
        CoreError::Connection(detail) => format!(
            "✗ Connection failed: Unable to reach {base_url} ({detail})\n  Please check your internet connection or run /connect to configure provider settings."
        ),
        CoreError::Timeout(detail) => format!(
            "✗ Connection failed: Unable to reach {base_url} (request timed out: {detail})\n  Please check your internet connection or run /connect to configure provider settings."
        ),
        CoreError::Provider(detail) => {
            // Bounded detail (never raw multi-KB dumps) + explicit
            // retryability on the first line of each classified arm.
            // Codex steal: extract Status/Code/Type/Message from JSON blobs
            // instead of dumping {"model":..,"error":{...}} raw.
            let cleaned = clean_provider_detail(detail);
            let short = truncate_chars(&cleaned, 600);
            let lower = short.to_lowercase();
            if lower.contains("not supported")
                || lower.contains("unsupported")
                || lower.contains("model not found")
                || lower.contains("unknown model")
                || short.contains(" 404")
                || short.contains("status 404")
            {
                format!(
                    "✗ Bad request (not retryable): {short}\n  Model may not be supported on {base_url}. Run /model to pick a valid model or /connect to change provider."
                )
            } else if lower.contains("auth")
                || short.contains(" 401")
                || short.contains(" 403")
                || lower.contains("unauthorized")
            {
                format!(
                    "✗ Auth failed (not retryable): {short}\n  Check API key or run /connect to reconfigure provider."
                )
            } else if lower.contains("rate") || short.contains(" 429") {
                format!(
                    "✗ Rate limited (retryable): {short}\n  Try again later or switch model via /model."
                )
            } else if lower.contains("bad request") || short.contains(" 400") {
                format!(
                    "✗ Bad request (not retryable): {short}\n  Check model/provider settings via /model or /connect."
                )
            } else if lower.contains("server error")
                || lower.contains("status 5")
                || lower.contains("500 internal server error")
                || lower.contains("502")
                || lower.contains("503")
                || lower.contains("504")
            {
                format!(
                    "✗ Provider server error (retryable): {short}\n  Upstream model or provider ({base_url}) encountered a server error. Run /model to switch to another model or try again later."
                )
            } else {
                // Steal codex's UnexpectedResponseError display: keep status+body but add provider hint
                format!(
                    "✗ Provider error: {short}\n  Provider: {base_url} — try /model or /connect if this persists."
                )
            }
        }
        CoreError::Auth(detail) => {
            // Subscription relays surface here with no HTTP status (the 403
            // came from native inside the sidecar, already classified). Name
            // the login that failed so the next command is obvious.
            let cleaned = clean_provider_detail(detail);
            let short = truncate_chars(&cleaned, 600);
            format!(
                "✗ Auth failed (not retryable): {short}\n  Run /connect to re-login the subscription provider."
            )
        }
        CoreError::BadRequest(detail) => {
            let cleaned = clean_provider_detail(detail);
            let short = truncate_chars(&cleaned, 600);
            format!(
                "✗ Bad request (not retryable): {short}\n  Check model/provider settings via /model or /connect."
            )
        }
        CoreError::RateLimited(detail) => {
            let cleaned = clean_provider_detail(detail);
            let short = truncate_chars(&cleaned, 600);
            format!(
                "✗ Rate limited (retryable): {short}\n  Try again later or switch model via /model."
            )
        }
        CoreError::ContextOverflow(detail) => {
            let cleaned = clean_provider_detail(detail);
            let short = truncate_chars(&cleaned, 600);
            format!("✗ Context exhausted (not retryable): {short}\n  Start /new or run /compact.")
        }
        CoreError::ServerError(detail) => {
            let cleaned = clean_provider_detail(detail);
            let short = truncate_chars(&cleaned, 600);
            format!(
                "✗ Provider server error (retryable): {short}\n  Upstream model or provider ({base_url}) encountered a server error. Run /model to switch to another model or try again later."
            )
        }
        CoreError::Stream(detail) => {
            let cleaned = clean_provider_detail(detail);
            let short = truncate_chars(&cleaned, 600);
            format!("✗ Stream broken (retryable): {short}\n  Retrying the turn usually succeeds.")
        }
        CoreError::LoopDetected(detail) => {
            let short = truncate_chars(detail, 600);
            format!("✗ Tool loop detected (not retryable): {short}")
        }
        CoreError::Cancelled => "■ Cancelled.".to_string(),
        _ => format!("agent error: {e}"),
    }
}

/// Builds the user message with MIME-driven attachments (opencode parity):
/// images normalized (downscaled, capped); video, PDF and audio as native
/// media carrying a fallback (contact sheet, PDF text, a note) that the
/// provider sends instead on a model without that input — the same parts a
/// `cat` of the file produces. Anything else is reported loudly.
pub(crate) fn build_user_message_with_attachments(
    text: &str,
    paths: &[std::path::PathBuf],
) -> Message {
    use gray_core::message::ContentBlock;
    use gray_tools::view::Attached;
    if paths.is_empty() {
        return Message::user(text);
    }
    let mut blocks = Vec::new();
    if !text.is_empty() {
        blocks.push(ContentBlock::text(text.to_string()));
    }
    for path in paths {
        if !gray_tools::images::is_viewable_extension(path) {
            blocks.push(ContentBlock::text(format!(
                "(attached file {} skipped: unsupported type)",
                path.display()
            )));
            continue;
        }
        blocks.push(match gray_tools::view::attach(path) {
            Ok(Attached::Image(img, _)) => ContentBlock::image(img.media_type, img.data),
            Ok(Attached::Media(m, _)) => ContentBlock::media(m.media_type, m.data, m.fallback),
            Ok(Attached::Text(t, _)) => ContentBlock::text(t),
            Err(e) => ContentBlock::text(format!("(attachment skipped: {e})")),
        });
    }
    Message::new(gray_core::message::Role::User, blocks)
}

/// ANSI dim + italic — pi's styling for rendered thinking blocks
/// (italic muted color; dim stands in for pi's `thinkingText` theme color).
pub const THINKING_STYLE: &str = "\x1b[2m\x1b[3m";

/// Formats a turn duration like the TUI `Worked for` line: `850ms`, `6s`, `6.5s`, `2m 5s`.
pub fn fmt_duration_ms(ms: u64) -> String {
    let secs = ms as f64 / 1000.0;
    if secs < 1.0 {
        format!("{ms}ms")
    } else if secs < 60.0 {
        let s = format!("{secs:.1}s");
        if s.ends_with(".0s") {
            s.replacen(".0s", "s", 1)
        } else {
            s
        }
    } else {
        let total_s = ms / 1000;
        let m = total_s / 60;
        let s = total_s % 60;
        if s == 0 {
            format!("{m}m")
        } else {
            format!("{m}m {s}s")
        }
    }
}

#[path = "format_tests.rs"]
#[cfg(test)]
mod tests;
