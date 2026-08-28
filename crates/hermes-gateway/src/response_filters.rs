//! Gateway response filtering helpers.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/gateway/response_filters.py` (147 LOC).
//! These helpers operate at the gateway boundary: they decide whether a completed
//! agent turn should be delivered to the chat, not what should be persisted in the
//! conversation history.
//!
//! Python source docstring (preserved):
//! ```text
//! Gateway response filtering helpers.
//!
//! These helpers operate at the gateway boundary: they decide whether a completed
//! agent turn should be delivered to the chat, not what should be persisted in the
//! conversation history.
//! ```

// ---------------------------------------------------------------------------
// Constants — mirrors Python module-level constants
// ---------------------------------------------------------------------------

/// Canonical model-emitted control token for intentional silence.
pub const SILENT_REPLY_TOKEN: &str = "NO_REPLY";

/// Exact whole-response markers that mean "the agent intentionally chose not to reply".
/// Keep this list small and explicit; arbitrary empty output remains an error/empty-response
/// path, not silence. Mirrors `LIVE_GATEWAY_SILENT_MARKERS` (frozenset).
pub const LIVE_GATEWAY_SILENT_MARKERS: &[&str] = &["[SILENT]", "SILENT", "NO_REPLY", "NO REPLY"];

// ---------------------------------------------------------------------------
// Internal helpers — mirrors Python underscore-prefixed helpers
// ---------------------------------------------------------------------------

fn canonical_silence_candidate(text: &str) -> String {
    // Mirrors: " ".join(text.strip().upper().split())
    text.trim()
        .to_uppercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Check if `c` is Unicode punctuation (General Category P) excluding structural `[]`.
///
/// Python uses `unicodedata.category(c).startswith("P")`.
/// Rust stdlib has no unicode category table, so we approximate:
/// - ASCII: `is_ascii_punctuation()` (covers `!\"#$%&'()*+,-./:;<=>?@[\\]^_`{|}~`)
/// - Non-ASCII: common punctuation blocks (Po/Ps/Pe/Pi/Pf/Pd/Pc). This covers
///   `“”‘’—–…¡¿。，` etc which the gateway may see in non-English locales.
/// Conservative: unknown non-ASCII non-punctuation (e.g. emoji, symbols) is NOT stripped,
/// matching Python's `category != P` behaviour.
// ponytail: ascii + block approximation, add unicode-general-category crate if non-ascii punct fidelity matters
fn is_punctuation(c: char) -> bool {
    if c == '[' || c == ']' {
        return false;
    }
    if c.is_ascii() {
        return c.is_ascii_punctuation();
    }
    matches!(c,
        '\u{00A1}' | '\u{00A7}' | '\u{00AB}' | '\u{00B6}' | '\u{00B7}' | '\u{00BB}' | '\u{00BF}' |
        '\u{2010}'..='\u{2027}' |
        '\u{2030}'..='\u{203E}' |
        '\u{2041}'..='\u{205E}' |
        '\u{3001}'..='\u{303F}' |
        '\u{FE30}'..='\u{FE4F}' |
        '\u{FE50}'..='\u{FE6F}' |
        '\u{FF01}'..='\u{FF0F}' |
        '\u{FF1A}'..='\u{FF20}' |
        '\u{FF3B}'..='\u{FF40}' |
        '\u{FF5B}'..='\u{FF65}'
    )
}

fn strip_edge_silence_punctuation(text: &str) -> String {
    // Mirrors _strip_edge_silence_punctuation:
    //   while start < end and text[start] not in "[]" and category.startswith("P"): start+=1
    //   while end > start and text[end-1] not in "[]" and category.startswith("P"): end-=1
    //   return text[start:end].strip()
    let chars: Vec<char> = text.chars().collect();
    let mut start = 0usize;
    let mut end = chars.len();
    while start < end && chars[start] != '[' && chars[start] != ']' && is_punctuation(chars[start]) {
        start += 1;
    }
    while end > start && chars[end - 1] != '[' && chars[end - 1] != ']' && is_punctuation(chars[end - 1]) {
        end -= 1;
    }
    chars[start..end].iter().collect::<String>().trim().to_string()
}

fn canonical_silence_candidates(text: &str) -> Vec<String> {
    let exact = canonical_silence_candidate(text);
    let trimmed = text.trim();
    let stripped = strip_edge_silence_punctuation(trimmed);
    if stripped == trimmed {
        vec![exact]
    } else {
        let fallback = canonical_silence_candidate(&stripped);
        vec![exact, fallback]
    }
}

// ---------------------------------------------------------------------------
// Public API — mirrors Python top-level functions
// ---------------------------------------------------------------------------

/// Return true only when `response` is exactly a silence marker.
///
/// Substantive prose that merely mentions `NO_REPLY` or `[SILENT]` must be
/// delivered normally. A blank response is also not silence; blank output is
/// handled by the empty-response failure path.
///
/// Mirrors `is_intentional_silence_response(response: Any) -> bool`.
pub fn is_intentional_silence_response(response: &str) -> bool {
    let stripped = response.trim();
    if stripped.is_empty() {
        return false;
    }
    if stripped.chars().count() > 64 {
        return false;
    }
    canonical_silence_candidates(stripped)
        .iter()
        .any(|c| LIVE_GATEWAY_SILENT_MARKERS.contains(&c.as_str()))
}

/// `serde_json::Value` variant — returns false for non-string values, mirroring
/// Python's `isinstance(response, str)` guard.
pub fn is_intentional_silence_response_value(response: &serde_json::Value) -> bool {
    match response {
        serde_json::Value::String(s) => is_intentional_silence_response(s),
        _ => false,
    }
}

/// Loose silence matcher for autonomous lanes (cron, webhook).
///
/// Autonomous lanes instruct the agent to emit `[SILENT]` when a tick
/// produced nothing worth a human's attention, and models reliably bracket
/// the marker with a short note explaining why they stayed quiet. Unlike
/// `is_intentional_silence_response` (the interactive-chat rule, which
/// demands the response be EXACTLY a marker), this suppresses when a marker
/// is the whole response, sits on its own first or last line, or the
/// bracketed sentinel opens the response (the documented
/// `[SILENT] No changes detected` pattern). A token buried mid-sentence
/// in a genuine report is still delivered.
///
/// Shares `LIVE_GATEWAY_SILENT_MARKERS` so the interactive and autonomous
/// marker sets can never drift apart.
///
/// Mirrors `is_autonomous_silence_response(response: Any) -> bool`.
pub fn is_autonomous_silence_response(response: &str) -> bool {
    let stripped = response.trim();
    if stripped.is_empty() {
        return false;
    }

    let is_token = |line: &str| -> bool {
        LIVE_GATEWAY_SILENT_MARKERS.contains(&canonical_silence_candidate(line).as_str())
    };

    // Whole response is exactly a token.
    if is_token(stripped) {
        return true;
    }
    // Marker on its own first or last line (leading/trailing note on a
    // separate line — e.g. "2 deals filtered\n\n[SILENT]").
    let lines: Vec<&str> = stripped
        .splitlines()
        .map(|ln| ln.trim())
        .filter(|ln| !ln.is_empty())
        .collect();
    if !lines.is_empty() && (is_token(lines[0]) || is_token(lines[lines.len() - 1])) {
        return true;
    }
    // Bracketed sentinel used as a same-line prefix — the documented pattern
    // "[SILENT] No changes detected". Restricted to the bracketed form so a
    // bare word like "Silent retry succeeded" is NOT swallowed.
    if stripped.to_uppercase().starts_with("[SILENT]") {
        return true;
    }
    false
}

/// `serde_json::Value` variant for autonomous check.
pub fn is_autonomous_silence_response_value(response: &serde_json::Value) -> bool {
    match response {
        serde_json::Value::String(s) => is_autonomous_silence_response(s),
        _ => false,
    }
}

/// Python truthiness for `serde_json::Value`, mirroring `if agent_result.get("failed"):`.
fn is_json_truthy(v: &serde_json::Value) -> bool {
    match v {
        serde_json::Value::Null => false,
        serde_json::Value::Bool(b) => *b,
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                i != 0
            } else if let Some(f) = n.as_f64() {
                f != 0.0 && !f.is_nan()
            } else {
                true
            }
        }
        serde_json::Value::String(s) => !s.is_empty(),
        serde_json::Value::Array(a) => !a.is_empty(),
        serde_json::Value::Object(o) => !o.is_empty(),
    }
}

/// Silence markers suppress delivery only for successful agent turns.
///
/// Mirrors `is_intentional_silence_agent_result(agent_result: dict | None, response: Any) -> bool`.
pub fn is_intentional_silence_agent_result(
    agent_result: Option<&serde_json::Value>,
    response: &str,
) -> bool {
    let Some(v) = agent_result else {
        return false;
    };
    let Some(obj) = v.as_object() else {
        return false;
    };
    if let Some(failed) = obj.get("failed") {
        if is_json_truthy(failed) {
            return false;
        }
    }
    is_intentional_silence_response(response)
}

/// `serde_json::Value` variant for the response argument.
pub fn is_intentional_silence_agent_result_value(
    agent_result: Option<&serde_json::Value>,
    response: &serde_json::Value,
) -> bool {
    let s = match response {
        serde_json::Value::String(s) => s.as_str(),
        _ => return false,
    };
    is_intentional_silence_agent_result(agent_result, s)
}

/// Convenience that accepts a `serde_json::Map` directly (common when callers already have an object).
pub fn is_intentional_silence_agent_result_map(
    agent_result: Option<&serde_json::Map<String, serde_json::Value>>,
    response: &str,
) -> bool {
    let Some(map) = agent_result else {
        return false;
    };
    if let Some(failed) = map.get("failed") {
        if is_json_truthy(failed) {
            return false;
        }
    }
    is_intentional_silence_response(response)
}

/// Return true while `text` could still resolve to a silence marker.
///
/// The streaming path accumulates the reply delta-by-delta and must decide,
/// before the whole response is known, whether to show what it has so far.
/// A buffer whose canonical form is a non-empty *prefix* of a silence marker
/// (e.g. `"NO"` on the way to `"NO_REPLY"`, or an exact marker that has
/// not yet been terminated by stream-end) is held back so a raw marker is
/// never edited onto the screen and then belatedly retracted.
///
/// Anything that has already diverged from every marker (ordinary prose) —
/// and anything longer than the marker cap — returns false so normal
/// streaming resumes immediately. This is the streaming counterpart to
/// `is_intentional_silence_response`, sharing the same marker set and
/// canonicalization so the two never drift.
///
/// Mirrors `is_partial_silence_marker(text: Any) -> bool`.
pub fn is_partial_silence_marker(text: &str) -> bool {
    let stripped = text.trim();
    if stripped.is_empty() || stripped.chars().count() > 64 {
        return false;
    }
    for candidate in canonical_silence_candidates(stripped) {
        if candidate.is_empty() {
            continue;
        }
        if LIVE_GATEWAY_SILENT_MARKERS
            .iter()
            .any(|m| m.starts_with(candidate.as_str()))
        {
            return true;
        }
    }
    false
}

/// `serde_json::Value` variant — false for non-strings.
pub fn is_partial_silence_marker_value(text: &serde_json::Value) -> bool {
    match text {
        serde_json::Value::String(s) => is_partial_silence_marker(s),
        _ => false,
    }
}

// Provide private aliases mirroring Python's underscore-prefixed helpers for traceability
#[allow(dead_code)]
fn _canonical_silence_candidate(text: &str) -> String {
    canonical_silence_candidate(text)
}

#[allow(dead_code)]
fn _strip_edge_silence_punctuation(text: &str) -> String {
    strip_edge_silence_punctuation(text)
}

#[allow(dead_code)]
fn _canonical_silence_candidates(text: &str) -> Vec<String> {
    canonical_silence_candidates(text)
}
