use gray_core::error::CoreError;
use gray_core::event::AgentEvent;
use gray_core::message::Message;

/// Maximum characters of error output rendered in tool results.
pub(crate) const MAX_ERROR_DISPLAY_CHARS: usize = 200;

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
        _ => format!("agent error: {e}"),
    }
}

pub(crate) fn base64_encode(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((n >> 18) & 63) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            TABLE[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

/// Builds the user message with MIME-driven attachments (opencode parity):
/// images normalized (downscaled, capped), PDFs as extracted text, videos
/// as first-frame stills. Audio/anything else is reported loudly, never
/// silently dropped.
pub(crate) fn build_user_message_with_attachments(
    text: &str,
    paths: &[std::path::PathBuf],
) -> Message {
    use super::attachments::{AttachmentKind, attachment_kind};
    if paths.is_empty() {
        return Message::user(text);
    }
    let mut blocks = Vec::new();
    if !text.is_empty() {
        blocks.push(gray_core::message::ContentBlock::text(text.to_string()));
    }
    for path in paths {
        let name = path.display().to_string();
        match attachment_kind(path) {
            AttachmentKind::Image => match std::fs::read(path) {
                Ok(bytes) => match super::attachments::normalize_image_bytes(&bytes) {
                    Ok((mime, out)) => blocks.push(gray_core::message::ContentBlock::image(
                        mime,
                        base64_encode(&out),
                    )),
                    Err(e) => blocks.push(gray_core::message::ContentBlock::text(format!(
                        "(attached image {name} skipped: {e})"
                    ))),
                },
                Err(e) => blocks.push(gray_core::message::ContentBlock::text(format!(
                    "(attached image {name} unreadable: {e})"
                ))),
            },
            AttachmentKind::Pdf => match super::attachments::pdf_text(path) {
                Ok(t) => blocks.push(gray_core::message::ContentBlock::text(format!(
                    "--- {name} (PDF text) ---\n{t}"
                ))),
                Err(e) => blocks.push(gray_core::message::ContentBlock::text(format!(
                    "(attached PDF {name} skipped: {e})"
                ))),
            },
            AttachmentKind::Video => match super::attachments::video_frame(path) {
                Ok(frame) => match super::attachments::normalize_image_bytes(&frame) {
                    Ok((mime, out)) => {
                        blocks.push(gray_core::message::ContentBlock::text(format!(
                            "(first frame of {name})"
                        )));
                        blocks.push(gray_core::message::ContentBlock::image(
                            mime,
                            base64_encode(&out),
                        ));
                    }
                    Err(e) => blocks.push(gray_core::message::ContentBlock::text(format!(
                        "(attached video {name} skipped: {e})"
                    ))),
                },
                Err(e) => blocks.push(gray_core::message::ContentBlock::text(format!(
                    "(attached video {name} skipped: {e})"
                ))),
            },
            // No model-agnostic wire path for audio on our providers — loud skip.
            AttachmentKind::Audio | AttachmentKind::Unsupported => {
                blocks.push(gray_core::message::ContentBlock::text(format!(
                    "(attached file {name} skipped: audio/unsupported type, no model wire path yet)"
                )))
            }
        }
    }
    if blocks.is_empty() {
        return Message::user(text);
    }
    Message::new(gray_core::message::Role::User, blocks)
}

/// ANSI dim + italic — pi's styling for rendered thinking blocks
/// (italic muted color; dim stands in for pi's `thinkingText` theme color).
pub const THINKING_STYLE: &str = "\x1b[2m\x1b[3m";

/// Formats an [`AgentEvent`] for display in the interactive REPL.
pub fn fmt_event(event: &AgentEvent) -> String {
    match event {
        AgentEvent::Start | AgentEvent::ToolCallEnd { .. } | AgentEvent::StepUsage { .. } => {
            String::new()
        }
        AgentEvent::TextDelta { delta } => delta.clone(),
        AgentEvent::ThinkingDelta { delta } => {
            // Streamed live, dim+italic like pi's rendered thinking blocks.
            format!("{THINKING_STYLE}{}\x1b[0m", delta.clone())
        }
        AgentEvent::ToolCallStart { name, .. } => {
            format!("\n\x1b[2m· {name}\x1b[0m\n")
        }
        AgentEvent::ToolResult {
            output, is_error, ..
        } => {
            if *is_error {
                let truncated = truncate_chars(output, MAX_ERROR_DISPLAY_CHARS);
                format!("\x1b[31m✗ {truncated}\x1b[0m\n")
            } else {
                String::new()
            }
        }
        AgentEvent::TurnEnd { usage, .. } => {
            if usage.total() > 0 {
                let cached = if usage.cached_tokens > 0 {
                    format!(
                        " · {} cached ({:.0}%)",
                        fmt_usage(usage.cached_tokens),
                        usage.cache_hit_rate() * 100.0
                    )
                } else {
                    String::new()
                };
                format!(
                    "\n\x1b[2m\u{b7} {} tok{cached}\x1b[0m\n",
                    fmt_usage(usage.total())
                )
            } else {
                "\n".to_string()
            }
        }
        // Codex steal (`new_stream_error_event`): dim `⚠ msg` + `└ details`.
        AgentEvent::StreamError { message, details } => {
            let trunc = truncate_chars(details, MAX_ERROR_DISPLAY_CHARS);
            if trunc.is_empty() {
                format!("\n\x1b[2m⚠ {message}\x1b[0m\n")
            } else {
                format!("\n\x1b[2m⚠ {message}\n└ {trunc}\x1b[0m\n")
            }
        }
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_style_server_error_extracts_message_not_raw_json() {
        // Screenshot case: opencode/zen 503 with {"model":..,"error":{...}} blob.
        // Codex-style: show Status/Code/Type/Message, never the raw JSON dump.
        let detail = r#"server error: status 503 Service Unavailable: {"model":"muse-spark-1.3-contributor","error":{"param":null,"type":"server_error","message":"Error from provider (Console Go): Upstream request failed: [service_overloaded] The backend is temporarily overloaded. Please retry."}}, cf-ray: a35cf09b5c1b7537-SEA"#;
        let out = format_core_error(
            &CoreError::Provider(detail.to_string()),
            "https://opencode.ai/zen/go/v1",
        );
        assert!(out.contains("(retryable)"), "must stay retryable: {out}");
        assert!(
            out.contains("The backend is temporarily overloaded"),
            "must surface extracted message: {out}"
        );
        assert!(!out.contains("\"model\":"), "must not dump raw JSON: {out}");
        assert!(!out.contains("\"param\":"), "must not dump raw JSON: {out}");
        assert!(out.contains("503"), "must keep status: {out}");
        assert!(out.contains("cf-ray"), "must keep cf-ray: {out}");
    }

    #[test]
    fn codex_style_stream_error_renders_reconnecting_with_details() {
        // Codex steal: StreamError cell is `⚠ Reconnecting... n/m` + `└ details`.
        let ev = AgentEvent::StreamError {
            message: "Reconnecting... 1/3".to_string(),
            details: "status 503: backend overloaded".to_string(),
        };
        let out = fmt_event(&ev);
        assert!(
            out.contains("Reconnecting... 1/3"),
            "must show attempt: {out}"
        );
        assert!(
            out.contains("backend overloaded"),
            "must show details: {out}"
        );
    }

    #[test]
    fn formats_subsecond_as_ms() {
        assert_eq!(fmt_duration_ms(850), "850ms");
    }

    #[test]
    fn formats_seconds_trimming_point_zero() {
        assert_eq!(fmt_duration_ms(6000), "6s");
        assert_eq!(fmt_duration_ms(6500), "6.5s");
    }

    #[test]
    fn formats_minutes() {
        assert_eq!(fmt_duration_ms(125_000), "2m 5s");
        assert_eq!(fmt_duration_ms(120_000), "2m");
    }
}
