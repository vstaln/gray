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
        _ => format!("agent error: {e}"),
    }
}

pub(crate) fn base64_encode(input: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(input)
}

/// Builds the user message with MIME-driven attachments (opencode parity):
/// images normalized (downscaled, capped), PDFs as extracted text, videos as
/// a native part on a model that takes one and a contact sheet otherwise.
/// Audio/anything else is reported loudly, never silently dropped.
pub(crate) fn build_user_message_with_attachments(
    text: &str,
    paths: &[std::path::PathBuf],
    model: &str,
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
            // Native video only where the wire actually has a video part and
            // the clip fits the cap; every other model gets the same contact
            // sheet `gray view` would produce, so one pasted file works
            // everywhere.
            AttachmentKind::Video => {
                let raw = match std::fs::read(path) {
                    Ok(bytes) => bytes,
                    Err(e) => {
                        blocks.push(gray_core::message::ContentBlock::text(format!(
                            "(attached video {name} unreadable: {e})"
                        )));
                        continue;
                    }
                };
                if gray_provider::openai::model_accepts_video(model)
                    && raw.len() <= gray_provider::openai::MAX_NATIVE_VIDEO_BYTES
                {
                    blocks.push(gray_core::message::ContentBlock::video(
                        super::attachments::video_media_type(path),
                        base64_encode(&raw),
                    ));
                } else {
                    let reason = if gray_provider::openai::model_accepts_video(model) {
                        "over the native size cap"
                    } else {
                        "model has no native video input"
                    };
                    match gray_tools::video_sheet::video_sheet(
                        path,
                        gray_tools::video_sheet::DEFAULT_FRAMES,
                    ) {
                        Ok(sheet) => match super::attachments::normalize_image_bytes(&sheet) {
                            Ok((mime, out)) => {
                                blocks.push(gray_core::message::ContentBlock::text(format!(
                                    "({name}: contact sheet, {reason})"
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
                    }
                }
            }
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
