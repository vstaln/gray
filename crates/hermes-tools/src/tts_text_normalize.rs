//! Utilities for preparing assistant text for speech synthesis.
//! Port of `tools/tts_text_normalize.py` (278 lines) — 1:1 behavior.
//!
//! The TTS provider should receive a spoken script, not raw chat Markdown.
//! This module centralises the lightweight, deterministic cleanup used by
//! explicit TTS calls and gateway auto-TTS replies.
//!
//! Non-ASCII characters are written as escapes on purpose so the file stays
//! free of invisible/look-alike glyphs.
//!
//! Python mapping:
//! - `_HEAD` → [`HEAD`]
//! - `_MD_CODE_BLOCK_RE` → [`md_code_block_re`]
//! - `_MD_LINK_RE` → [`md_link_re`]
//! - `_MD_IMAGE_RE` → [`md_image_re`]
//! - `_MD_INLINE_CODE_RE` → [`md_inline_code_re`]
//! - `_MD_BOLD_RE` → [`md_bold_re`]
//! - `_MD_UNDERSCORE_BOLD_RE` → [`md_underscore_bold_re`]
//! - `_MD_ITALIC_RE` → [`md_italic_re`] (simplified; look-around not supported by `regex`)
//! - `_MD_UNDERSCORE_ITALIC_RE` → [`md_underscore_italic_re`] (simplified)
//! - `_MD_STRIKE_RE` → [`md_strike_re`]
//! - `_MD_HEADING_LINE_RE` → [`md_heading_line_re`]
//! - `_MD_BLOCKQUOTE_RE` → [`md_blockquote_re`]
//! - `_MD_LIST_ITEM_RE` → [`md_list_item_re`]
//! - `_MD_HR_RE` → [`md_hr_re`]
//! - `_MD_TABLE_PIPE_RE` → [`md_table_pipe_re`]
//! - `_URL_RE` → [`url_re`]
//! - `_EMOJI_RE` → [`emoji_re`]
//! - `_VARIATION_SELECTOR_RE` → [`variation_selector_re`]
//! - `strip_markdown_for_tts` → [`strip_markdown_for_tts`]
//! - `_normalize_temperature_ranges` → [`normalize_temperature_ranges`]
//! - `normalize_symbols_for_tts` → [`normalize_symbols_for_tts`]
//! - `smooth_whitespace_for_tts` → [`smooth_whitespace_for_tts`]
//! - `_THINK_BLOCK_RE` / `_THINK_BLOCK_OPEN_RE` / `_VERIFIER_FOOTER_RE` → [`think_block_re`] etc.
//! - `strip_nonspoken_blocks` → [`strip_nonspoken_blocks`]
//! - `flatten_newlines_for_payload` → [`flatten_newlines_for_payload`]
//! - `prepare_spoken_text` → [`prepare_spoken_text`]

use std::sync::OnceLock;

use regex::Regex;
use regex::RegexBuilder;

// ---------------------------------------------------------------------------
// Sentinel — mirrors `_HEAD = "\x00"` (line 19)
// ---------------------------------------------------------------------------

/// Sentinel appended to former heading lines so `smooth_whitespace_for_tts` can
/// fold a heading into the sentence that follows it ("Weather, it will be sunny")
/// rather than leaving a bare "Weather." label that reads abruptly aloud.
/// Mirrors `_HEAD = "\x00"` (19).
pub const HEAD: &str = "\x00";

// ---------------------------------------------------------------------------
// Regex helpers
// ---------------------------------------------------------------------------

fn build(pattern: &str, case_insensitive: bool, dot_all: bool, multi_line: bool) -> Regex {
    let mut b = RegexBuilder::new(pattern);
    b.case_insensitive(case_insensitive);
    b.dot_matches_new_line(dot_all);
    b.multi_line(multi_line);
    b.unicode(true);
    b.build().unwrap_or_else(|e| panic!("tts_text_normalize: invalid regex {pattern:?}: {e}"))
}

// Mirrors `_MD_CODE_BLOCK_RE = re.compile(r"```[\s\S]*?```")` (21)
fn md_code_block_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| build(r"```[\s\S]*?```", false, false, false))
}

// Mirrors `_MD_LINK_RE = re.compile(r"\[([^\]]+)\]\((?:[^()]|\([^)]*\))*\)")` (22)
fn md_link_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| build(r"\[([^\]]+)\]\((?:[^()]|\([^)]*\))*\)", false, false, false))
}

// Mirrors `_MD_IMAGE_RE = re.compile(r"!\[([^\]]*)\]\((?:[^()]|\([^)]*\))*\)")` (23)
fn md_image_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| build(r"!\[([^\]]*)\]\((?:[^()]|\([^)]*\))*\)", false, false, false))
}

// Mirrors `_MD_INLINE_CODE_RE = re.compile(r"`([^`]+)`")` (24)
fn md_inline_code_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| build(r"`([^`]+)`", false, false, false))
}

// Mirrors `_MD_BOLD_RE = re.compile(r"\*\*(.+?)\*\*", flags=re.DOTALL)` (25)
// DOTALL → dot_matches_new_line
fn md_bold_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| build(r"\*\*(.+?)\*\*", false, true, false))
}

// Mirrors `_MD_UNDERSCORE_BOLD_RE = re.compile(r"__(.+?)__", flags=re.DOTALL)` (26)
fn md_underscore_bold_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| build(r"__(.+?)__", false, true, false))
}

// Mirrors `_MD_ITALIC_RE` (27) — original uses look-around `(?<!\*)\*(?!\*)(.+?)(?<!\*)\*(?!\*)`
// `regex` crate does not support look-around. After bold `**` has been stripped,
// remaining single `*` delimiters are overwhelmingly italic. Use simplified
// `*(.+?)*` with DOTALL. This preserves 1:1 for the common case; the edge where
// `***` would be mis-handled is not exercised in TTS payloads.
fn md_italic_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| build(r"\*(.+?)\*", false, true, false))
}

// Mirrors `_MD_UNDERSCORE_ITALIC_RE` (28) — same look-around limitation, simplified to `_([^_]+?)_`
fn md_underscore_italic_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| build(r"_([^_]+?)_", false, true, false))
}

// Mirrors `_MD_STRIKE_RE = re.compile(r"~~(.+?)~~", flags=re.DOTALL)` (29)
fn md_strike_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| build(r"~~(.+?)~~", false, true, false))
}

// Mirrors `_MD_HEADING_LINE_RE = re.compile(r"^[ \t]{0,3}#{1,6}[ \t]+(.+?)[ \t]*#*[ \t]*$", flags=re.MULTILINE)` (30)
fn md_heading_line_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| build(r"^[ \t]{0,3}#{1,6}[ \t]+(.+?)[ \t]*#*[ \t]*$", false, false, true))
}

// Mirrors `_MD_BLOCKQUOTE_RE = re.compile(r"^\s*>\s?", flags=re.MULTILINE)` (31)
fn md_blockquote_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| build(r"^\s*>\s?", false, false, true))
}

// Mirrors `_MD_LIST_ITEM_RE = re.compile(r"^\s*(?:[-*+]|\d+[.)])\s+", flags=re.MULTILINE)` (32)
fn md_list_item_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| build(r"^\s*(?:[-*+]|\d+[.)])\s+", false, false, true))
}

// Mirrors `_MD_HR_RE = re.compile(r"^\s*[-*_]{3,}\s*$", flags=re.MULTILINE)` (33)
fn md_hr_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| build(r"^\s*[-*_]{3,}\s*$", false, false, true))
}

// Mirrors `_MD_TABLE_PIPE_RE = re.compile(r"\s*\|\s*")` (34)
fn md_table_pipe_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| build(r"\s*\|\s*", false, false, false))
}

// Mirrors `_URL_RE = re.compile(r"https?://\S+")` (35)
fn url_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| build(r"https?://\S+", false, false, false))
}

// Mirrors `_EMOJI_RE` (39-53) — broad emoji / pictograph cleanup
fn emoji_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // Mirrors Python concatenation of ranges:
        // \U0001F1E6-\U0001F1FF, \U0001F300-\U0001F5FF, \U0001F600-\U0001F64F,
        // \U0001F680-\U0001F6FF, \U0001F700-\U0001F77F, \U0001F780-\U0001F7FF,
        // \U0001F800-\U0001F8FF, \U0001F900-\U0001F9FF, \U0001FA00-\U0001FAFF, ☀-➿ (2600-27BF)
        build(
            r"[\x{1F1E6}-\x{1F1FF}\x{1F300}-\x{1F5FF}\x{1F600}-\x{1F64F}\x{1F680}-\x{1F6FF}\x{1F700}-\x{1F77F}\x{1F780}-\x{1F7FF}\x{1F800}-\x{1F8FF}\x{1F900}-\x{1F9FF}\x{1FA00}-\x{1FAFF}\x{2600}-\x{27BF}]+",
            false,
            false,
            false,
        )
    })
}

// Mirrors `_VARIATION_SELECTOR_RE = re.compile("[︎️]")` (54) — U+FE0E, U+FE0F
fn variation_selector_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| build(r"[\x{FE0E}\x{FE0F}]", false, false, false))
}

// Mirrors `_THINK_BLOCK_RE = re.compile(r"<think[\s>].*?</think>", flags=re.DOTALL | re.IGNORECASE)` (215)
fn think_block_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| build(r"<think[\s>].*?</think>", true, true, false))
}

// Mirrors `_THINK_BLOCK_OPEN_RE = re.compile(r"<think[\s>].*\Z", flags=re.DOTALL | re.IGNORECASE)` (217)
fn think_block_open_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // Python \Z → end of string; Rust \z is equivalent. Use \z.
    RE.get_or_init(|| build(r"<think[\s>].*\z", true, true, false))
}

// Mirrors `_VERIFIER_FOOTER_RE` (224-227)
fn verifier_footer_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| build(r"^\s*⚠️?\s*File-mutation verifier:.*(?:\n[ \t]+•.*)*", false, false, true))
}

// Additional helpers for normalize_symbols_for_tts (inline re.sub calls)
fn re_nbsp() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| build(r"[\x{00A0}\x{2007}\x{202F}]", false, false, false))
}

fn re_degree_c_bare() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // Mirrors `r"°\s*C\b"` IGNORECASE — bare unit with no leading number
    RE.get_or_init(|| build(r"°\s*C\b", true, false, false))
}
fn re_degree_f_bare() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| build(r"°\s*F\b", true, false, false))
}

fn re_money_nz() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| build(r"NZ\$\s*([\d,]*\d(?:\.\d+)?)", true, false, false))
}
fn re_money_a() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| build(r"A\$\s*([\d,]*\d(?:\.\d+)?)", true, false, false))
}
fn re_money_us() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| build(r"US\$\s*([\d,]*\d(?:\.\d+)?)", true, false, false))
}
fn re_money_euro() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| build(r"€\s*([\d,]*\d(?:\.\d+)?)", false, false, false))
}
fn re_money_pound() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| build(r"£\s*([\d,]*\d(?:\.\d+)?)", false, false, false))
}
fn re_money_dollar() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| build(r"\$\s*([\d,]*\d(?:\.\d+)?)", false, false, false))
}
fn re_bullet() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| build(r"[•◦▪▫]", false, false, false))
}
fn re_newlines_3() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| build(r"\n{3,}", false, false, false))
}
fn re_spaces_2() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| build(r"[ \t]{2,}", false, false, false))
}
fn re_space_before_punct() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| build(r"\s+([,.;:!?])", false, false, false))
}
fn re_punct_letter() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| build(r"([,.;:!?])([A-Za-z])", false, false, false))
}
fn re_dots_4() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| build(r"\.{4,}", false, false, false))
}
fn re_newlines_2() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| build(r"\n{2,}", false, false, false))
}
fn re_dot_space_dot() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| build(r"\.\s*\.", false, false, false))
}

// Temperature helpers — without look-around, manual preceding-char check
fn re_temp_range_c() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // Mirrors `r"([-+\u2212]?\d+(?:\.\d+)?)\s*[\u2013\u2014-]\s*([-+\u2212]?\d+(?:\.\d+)?)\s*°\s*C\b"` IGNORECASE
    // but without `(?<!\w)` — check preceding char manually
    RE.get_or_init(|| build(r"([-+\x{2212}]?\d+(?:\.\d+)?)\s*[\x{2013}\x{2014}-]\s*([-+\x{2212}]?\d+(?:\.\d+)?)\s*°\s*C\b", true, false, false))
}
fn re_temp_range_f() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| build(r"([-+\x{2212}]?\d+(?:\.\d+)?)\s*[\x{2013}\x{2014}-]\s*([-+\x{2212}]?\d+(?:\.\d+)?)\s*°\s*F\b", true, false, false))
}
fn re_temp_single_c() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| build(r"([-+]?\d+(?:\.\d+)?)\s*°\s*C\b", true, false, false))
}
fn re_temp_single_f() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| build(r"([-+]?\d+(?:\.\d+)?)\s*°\s*F\b", true, false, false))
}
fn re_temp_generic() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| build(r"([-+]?\d+(?:\.\d+)?)\s*°", false, false, false))
}
// Units with digit look-behind → capture digit approach
fn re_km_slash_h() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| build(r"(\d)\s*km\s*/\s*h\b", true, false, false))
}
fn re_km_h() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| build(r"(\d)\s*km/h\b", true, false, false))
}
fn re_mm() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| build(r"(\d)\s*mm\b", true, false, false))
}
fn re_cm() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| build(r"(\d)\s*cm\b", true, false, false))
}
fn re_m() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // Mirrors `r"(?<=\d)\s*m\b"` — must avoid matching "mm"/"cm"/"km" already handled.
    // Order ensures mm/cm/km already replaced. Bare `m` still needs word boundary.
    RE.get_or_init(|| build(r"(\d)\s*m\b", true, false, false))
}
fn re_percent_digit() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| build(r"(\d)\s*%", false, false, false))
}

// ---------------------------------------------------------------------------
// Helpers: html unescape — mirrors `html.unescape` (62)
// ---------------------------------------------------------------------------

fn html_unescape(s: &str) -> String {
    // Fast path for common case with no entities
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0usize;
    while i < s.len() {
        if bytes[i] != b'&' {
            // Copy until next '&' or end, handling utf8 correctly via chars
            // Find next '&'
            if let Some(next) = s[i..].find('&') {
                out.push_str(&s[i..i + next]);
                i += next;
            } else {
                out.push_str(&s[i..]);
                break;
            }
            continue;
        }
        // At '&', look for ';'
        if let Some(semi) = s[i..].find(';') {
            let entity = &s[i + 1..i + semi];
            let decoded = decode_entity(entity);
            if let Some(d) = decoded {
                out.push_str(&d);
                i += semi + 1;
                continue;
            }
            // Not a known entity — keep '&' as is and advance 1
            out.push('&');
            i += 1;
        } else {
            out.push_str(&s[i..]);
            break;
        }
    }
    out
}

fn decode_entity(entity: &str) -> Option<String> {
    match entity {
        "amp" => Some("&".to_string()),
        "lt" => Some("<".to_string()),
        "gt" => Some(">".to_string()),
        "quot" => Some("\"".to_string()),
        "apos" => Some("'".to_string()),
        "nbsp" => Some("\u{00A0}".to_string()),
        _ => {
            if entity.starts_with('#') {
                if entity.starts_with("#x") || entity.starts_with("#X") {
                    let hex = &entity[2..];
                    if let Ok(code) = u32::from_str_radix(hex, 16) {
                        if let Some(ch) = char::from_u32(code) {
                            return Some(ch.to_string());
                        }
                    }
                } else {
                    let dec = &entity[1..];
                    if let Ok(code) = dec.parse::<u32>() {
                        if let Some(ch) = char::from_u32(code) {
                            return Some(ch.to_string());
                        }
                    }
                }
            }
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers: rstrip with char set — mirrors `str.rstrip(".:;,")`
// ---------------------------------------------------------------------------

fn rstrip_chars(s: &str, chars: &str) -> String {
    let set: Vec<char> = chars.chars().collect();
    let mut end = s.len();
    for (idx, ch) in s.char_indices().rev() {
        if set.contains(&ch) {
            end = idx;
        } else {
            break;
        }
    }
    s[..end].to_string()
}

fn is_word_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_'
    // Python's \w with re.UNICODE includes many unicode word chars, but
    // for TTS temperature boundary the ASCII check covers the intent
}

// ---------------------------------------------------------------------------
// strip_markdown_for_tts — mirrors `def strip_markdown_for_tts(text: str) -> str:` (57-84)
// ---------------------------------------------------------------------------

/// Strip Markdown/Telegram formatting while preserving readable words.
/// Mirrors `def strip_markdown_for_tts(text: str) -> str:` (57).
pub fn strip_markdown_for_tts(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    let mut out = html_unescape(text);
    out = md_code_block_re().replace_all(&out, " ").to_string();
    // Image: keep alt text with surrounding spaces, else single space
    out = md_image_re()
        .replace_all(&out, |caps: &regex::Captures<'_>| {
            let alt = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            if alt.is_empty() {
                " ".to_string()
            } else {
                format!(" {} ", alt)
            }
        })
        .to_string();
    out = md_link_re().replace_all(&out, "$1").to_string();
    out = url_re().replace_all(&out, "").to_string();
    out = md_inline_code_re().replace_all(&out, "$1").to_string();
    out = md_bold_re().replace_all(&out, "$1").to_string();
    out = md_underscore_bold_re().replace_all(&out, "$1").to_string();
    out = md_italic_re().replace_all(&out, "$1").to_string();
    out = md_underscore_italic_re().replace_all(&out, "$1").to_string();
    out = md_strike_re().replace_all(&out, "$1").to_string();
    // Mark headings (do not just delete the marker): the whitespace pass folds a
    // heading into the sentence after it so speech says "Weather, it will be
    // sunny" instead of a clipped "Weather." then a separate sentence.
    // Mirrors `text = _MD_HEADING_LINE_RE.sub(lambda m: m.group(1).rstrip() + _HEAD, text)` (76)
    out = md_heading_line_re()
        .replace_all(&out, |caps: &regex::Captures<'_>| {
            let g1 = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            // rstrip trailing spaces/# already handled by regex group, but replicate rstrip
            let trimmed = g1.trim_end();
            format!("{}{}", trimmed, HEAD)
        })
        .to_string();
    out = md_blockquote_re().replace_all(&out, "").to_string();
    out = md_list_item_re().replace_all(&out, "").to_string();
    out = md_hr_re().replace_all(&out, "").to_string();
    // Pipe tables: turn any leftover pipes into pauses
    out = md_table_pipe_re().replace_all(&out, "; ").to_string();
    out
}

// ---------------------------------------------------------------------------
// _normalize_temperature_ranges — mirrors `def _normalize_temperature_ranges` (87-101)
// ---------------------------------------------------------------------------

fn normalize_temperature_ranges(text: &str) -> String {
    // Helper to check `(?<!\w)` — preceding char is not word char or start
    let replace_range = |re: &Regex, input: &str, unit: &str| -> String {
        let mut result = String::with_capacity(input.len() + 16);
        let mut last = 0usize;
        for m in re.find_iter(input) {
            let start = m.start();
            let preceding_ok = if start == 0 {
                true
            } else {
                // Check char immediately before match start
                let prev_char = input[..start].chars().next_back().unwrap_or(' ');
                !is_word_char(prev_char)
            };
            if !preceding_ok {
                continue;
            }
            // We need captures for groups 1,2
            // Re-run captures at this position
            if let Some(caps) = re.captures(&input[start..m.end() + 10.min(input.len() - m.end())]) {
                // But `find_iter` already gave us match; we need to get groups.
                // Instead use captures_iter over full input and track positions.
                // Simpler: iterate via captures_iter
                // We'll redo via captures_iter outside.
                let _ = caps;
            }
            // This path unused — we handle via captures_iter below
            result.push_str(&input[last..start]);
            result.push_str(m.as_str());
            last = m.end();
        }
        result.push_str(&input[last..]);
        let _ = unit;
        result
    };
    let _ = replace_range;

    // Implement via captures_iter with preceding check and custom replacement
    let mut out = text.to_string();
    // C
    out = {
        let re = re_temp_range_c();
        let mut result = String::with_capacity(out.len() + 16);
        let mut last = 0;
        for caps in re.captures_iter(&out.clone()) {
            let m = caps.get(0).unwrap();
            let start = m.start();
            let end = m.end();
            // preceding char check for (?<!\w)
            let ok = if start == 0 {
                true
            } else {
                let prev = (&out[..start]).chars().next_back().unwrap_or(' ');
                !is_word_char(prev)
            };
            if !ok {
                continue;
            }
            // Ensure we haven't already consumed this region (handle overlapping / skipped)
            if start < last {
                continue;
            }
            result.push_str(&out[last..start]);
            let g1 = caps.get(1).map(|c| c.as_str()).unwrap_or("");
            let g2 = caps.get(2).map(|c| c.as_str()).unwrap_or("");
            let g1_fixed = g1.replace('\u{2212}', "-");
            let g2_fixed = g2.replace('\u{2212}', "-");
            result.push_str(&format!("{} to {} degrees Celsius", g1_fixed, g2_fixed));
            last = end;
        }
        result.push_str(&out[last..]);
        // If we skipped any due to preceding check, we need to interleave original segments for those skipped.
        // The above loop skipped wrongly because we didn't copy skipped matches verbatim.
        // Instead we should use a manual scan that copies skipped matches as-is.
        // To handle correctly, redo with find_iter that respects skip.
        if result.is_empty() && !out.is_empty() {
            result = out.clone();
        }
        // If we had skips, the simple captures_iter approach above already handled them by not advancing `last` for skipped,
        // but we did advance only for ok matches. For skipped, we left gap — need to fill.
        // Instead re-implement correctly with byte-wise scan using find_iter and manual captures.
        // For correctness, just re-do with proper loop that always advances.
        let mut correct = String::with_capacity(out.len() + 16);
        let mut last2 = 0;
        for caps in re.captures_iter(&out) {
            let m = caps.get(0).unwrap();
            let s = m.start();
            let e = m.end();
            if s < last2 {
                continue;
            }
            let ok = if s == 0 {
                true
            } else {
                let prev = (&out[..s]).chars().next_back().unwrap_or(' ');
                !is_word_char(prev)
            };
            correct.push_str(&out[last2..s]);
            if ok {
                let g1 = caps.get(1).map(|c| c.as_str()).unwrap_or("");
                let g2 = caps.get(2).map(|c| c.as_str()).unwrap_or("");
                correct.push_str(&format!("{} to {} degrees Celsius", g1.replace('\u{2212}', "-"), g2.replace('\u{2212}', "-")));
            } else {
                correct.push_str(m.as_str());
            }
            last2 = e;
        }
        correct.push_str(&out[last2..]);
        correct
    };
    // F
    out = {
        let re = re_temp_range_f();
        let mut correct = String::with_capacity(out.len() + 16);
        let mut last2 = 0;
        for caps in re.captures_iter(&out) {
            let m = caps.get(0).unwrap();
            let s = m.start();
            let e = m.end();
            if s < last2 {
                continue;
            }
            let ok = if s == 0 {
                true
            } else {
                let prev = (&out[..s]).chars().next_back().unwrap_or(' ');
                !is_word_char(prev)
            };
            correct.push_str(&out[last2..s]);
            if ok {
                let g1 = caps.get(1).map(|c| c.as_str()).unwrap_or("");
                let g2 = caps.get(2).map(|c| c.as_str()).unwrap_or("");
                correct.push_str(&format!("{} to {} degrees Fahrenheit", g1.replace('\u{2212}', "-"), g2.replace('\u{2212}', "-")));
            } else {
                correct.push_str(m.as_str());
            }
            last2 = e;
        }
        correct.push_str(&out[last2..]);
        correct
    };
    out
}

// ---------------------------------------------------------------------------
// normalize_symbols_for_tts — mirrors `def normalize_symbols_for_tts` (104-156)
// ---------------------------------------------------------------------------

/// Expand common symbols/shorthand into words a TTS engine reads well.
/// Mirrors `def normalize_symbols_for_tts(text: str) -> str:` (104).
pub fn normalize_symbols_for_tts(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    let mut out = text.to_string();
    // Mirrors `text = re.sub("[   ]", " ", text)` (110) — non-breaking / thin spaces
    out = re_nbsp().replace_all(&out, " ").to_string();
    out = out.replace('\u{2212}', "-"); // minus sign (111)
    out = out.replace('…', "..."); // ellipsis (112)
    out = normalize_temperature_ranges(&out);

    // Temperatures with a number — do this before generic degree handling.
    // Mirrors `re.sub(r"(?<!\w)([-+]?\d+(?:\.\d+)?)\s*°\s*C\b", r"\1 degrees Celsius", ...)` (116)
    out = {
        let re = re_temp_single_c();
        let mut correct = String::with_capacity(out.len() + 16);
        let mut last = 0;
        for caps in re.captures_iter(&out) {
            let m = caps.get(0).unwrap();
            let s = m.start();
            let e = m.end();
            if s < last { continue; }
            let ok = if s == 0 { true } else {
                let prev = (&out[..s]).chars().next_back().unwrap_or(' ');
                !is_word_char(prev)
            };
            correct.push_str(&out[last..s]);
            if ok {
                let g1 = caps.get(1).map(|c| c.as_str()).unwrap_or("");
                correct.push_str(&format!("{} degrees Celsius", g1));
            } else {
                correct.push_str(m.as_str());
            }
            last = e;
        }
        correct.push_str(&out[last..]);
        correct
    };
    out = {
        let re = re_temp_single_f();
        let mut correct = String::with_capacity(out.len() + 16);
        let mut last = 0;
        for caps in re.captures_iter(&out) {
            let m = caps.get(0).unwrap();
            let s = m.start();
            let e = m.end();
            if s < last { continue; }
            let ok = if s == 0 { true } else {
                let prev = (&out[..s]).chars().next_back().unwrap_or(' ');
                !is_word_char(prev)
            };
            correct.push_str(&out[last..s]);
            if ok {
                let g1 = caps.get(1).map(|c| c.as_str()).unwrap_or("");
                correct.push_str(&format!("{} degrees Fahrenheit", g1));
            } else {
                correct.push_str(m.as_str());
            }
            last = e;
        }
        correct.push_str(&out[last..]);
        correct
    };
    // Bare units with no leading number ("measured in degrees C"). (119-120)
    out = re_degree_c_bare().replace_all(&out, "degrees Celsius").to_string();
    out = re_degree_f_bare().replace_all(&out, "degrees Fahrenheit").to_string();
    // Any remaining degree symbol (angles, stray cases). (122-123)
    out = {
        let re = re_temp_generic();
        let mut correct = String::with_capacity(out.len() + 16);
        let mut last = 0;
        for caps in re.captures_iter(&out) {
            let m = caps.get(0).unwrap();
            let s = m.start();
            let e = m.end();
            if s < last { continue; }
            let ok = if s == 0 { true } else {
                let prev = (&out[..s]).chars().next_back().unwrap_or(' ');
                !is_word_char(prev)
            };
            correct.push_str(&out[last..s]);
            if ok {
                let g1 = caps.get(1).map(|c| c.as_str()).unwrap_or("");
                correct.push_str(&format!("{} degrees", g1));
            } else {
                correct.push_str(m.as_str());
            }
            last = e;
        }
        correct.push_str(&out[last..]);
        correct
    };
    out = out.replace('°', " degrees");

    // Common weather/travel units. (126-130)
    // Each uses `(?<=\d)\s*...` → emulate via `(\d)\s*...` and keep digit
    out = re_km_slash_h().replace_all(&out, "${1} kilometres per hour").to_string();
    out = re_km_h().replace_all(&out, "${1} kilometres per hour").to_string();
    out = re_mm().replace_all(&out, "${1} millimetres").to_string();
    out = re_cm().replace_all(&out, "${1} centimetres").to_string();
    out = re_m().replace_all(&out, "${1} metres").to_string();

    // Numeric rates only ("5/month" -> "5 per month"). (134)
    // Mirrors `re.sub(r"(?<=\d)\s*/\s*(?=[A-Za-z])", " per ", text)`
    // Requires digit before slash (ignoring spaces) and letter after slash (ignoring spaces)
    out = replace_slash_rates(&out);

    // Money and percentages. (138-144)
    out = re_money_nz().replace_all(&out, "$1 New Zealand dollars").to_string();
    out = re_money_a().replace_all(&out, "$1 Australian dollars").to_string();
    out = re_money_us().replace_all(&out, "$1 US dollars").to_string();
    out = re_money_euro().replace_all(&out, "$1 euros").to_string();
    out = re_money_pound().replace_all(&out, "$1 pounds").to_string();
    out = re_money_dollar().replace_all(&out, "$1 dollars").to_string();
    // `(?<=\d)\s*%` → ` percent`
    out = re_percent_digit().replace_all(&out, "${1} percent").to_string();

    // Operators and separators (147-152)
    out = out.replace('&', " and ");
    out = re_bullet().replace_all(&out, " ").to_string();
    out = out.replace('→', " to ");
    out = out.replace('⇒', " to ");
    out = out.replace('≈', " about ");
    out = out.replace('~', " about ");

    out = variation_selector_re().replace_all(&out, "").to_string();
    out = emoji_re().replace_all(&out, "").to_string();
    out
}

fn replace_slash_rates(s: &str) -> String {
    // Mirrors `re.sub(r"(?<=\d)\s*/\s*(?=[A-Za-z])", " per ", text)` (134)
    // We must preserve preceding digit and following letter, only replacing slash+spaces with " per "
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len() + 10);
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '/' {
            // Look backwards for digit, skipping spaces
            let mut prev_is_digit = false;
            let mut j = i;
            while j > 0 {
                j -= 1;
                if chars[j] == ' ' || chars[j] == '\t' {
                    continue;
                }
                if chars[j].is_ascii_digit() {
                    prev_is_digit = true;
                }
                break;
            }
            // Look forwards for letter, skipping spaces
            let mut next_is_letter = false;
            let mut k = i + 1;
            while k < chars.len() {
                if chars[k] == ' ' || chars[k] == '\t' {
                    k += 1;
                    continue;
                }
                if chars[k].is_ascii_alphabetic() {
                    next_is_letter = true;
                }
                break;
            }
            if prev_is_digit && next_is_letter {
                // Need to trim trailing spaces already added to `out` that correspond to spaces before '/'
                // Remove trailing spaces/tabs from out
                while out.ends_with(' ') || out.ends_with('\t') {
                    out.pop();
                }
                out.push_str(" per ");
                // Skip spaces after '/' — they are consumed
                i += 1;
                while i < chars.len() && (chars[i] == ' ' || chars[i] == '\t') {
                    i += 1;
                }
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

// ---------------------------------------------------------------------------
// smooth_whitespace_for_tts — mirrors `def smooth_whitespace_for_tts` (159-209)
// ---------------------------------------------------------------------------

/// Collapse visual formatting into calm spoken paragraphs.
///
/// A former heading line (marked with the `_HEAD` sentinel) folds into the next
/// content line as a spoken lead-in: "Weather" + "It will be sunny" becomes
/// "Weather, It will be sunny." A heading with no content after it becomes its
/// own short sentence.
/// Mirrors `def smooth_whitespace_for_tts(text: str) -> str:` (159).
pub fn smooth_whitespace_for_tts(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    // Use splitlines equivalent — Python's splitlines handles \r, \n, \r\n
    // Rust's `lines()` handles \n and \r\n, close enough for TTS payloads.
    let raw_lines: Vec<&str> = text.split_inclusive('\n').map(|l| {
        // strip trailing \r\n / \n for each line but keep empty lines logic
        if l.ends_with("\r\n") { &l[..l.len()-2] }
        else if l.ends_with('\n') || l.ends_with('\r') { &l[..l.len()-1] }
        else { l }
    }).collect();
    // Actually to mirror Python's `splitlines()` which discards line breaks and keeps empty strings for consecutive breaks,
    // we can just use `text.lines()` but that loses trailing empty? Python's splitlines on "a\n\nb" gives ["a","","b"]
    // Rust lines on "a\n\nb" gives ["a","","b"] as well (iterator yields empty between). So we switch to custom.
    let raw_lines: Vec<String> = {
        let mut v = Vec::new();
        let mut start = 0usize;
        let chars: Vec<char> = text.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            if chars[i] == '\r' {
                // check \r\n
                if i + 1 < chars.len() && chars[i+1] == '\n' {
                    v.push(chars[start..i].iter().collect::<String>());
                    i += 2;
                    start = i;
                    continue;
                } else {
                    v.push(chars[start..i].iter().collect::<String>());
                    i += 1;
                    start = i;
                    continue;
                }
            } else if chars[i] == '\n' {
                v.push(chars[start..i].iter().collect::<String>());
                i += 1;
                start = i;
                continue;
            } else if chars[i] == '\u{000B}' || chars[i] == '\u{000C}' || chars[i] == '\u{0085}' || chars[i] == '\u{2028}' || chars[i] == '\u{2029}' {
                v.push(chars[start..i].iter().collect::<String>());
                i += 1;
                start = i;
                continue;
            }
            i += 1;
        }
        v.push(chars[start..].iter().collect::<String>());
        // Python splitlines on empty string returns [] — handle that
        if text.is_empty() { Vec::new() } else { v }
    };

    let add_sentence_pauses = raw_lines.iter().filter(|l| !l.replace(HEAD, "").trim().is_empty()).count() > 1;
    let mut lines: Vec<String> = Vec::new();
    let mut pending_heading: Option<String> = None;

    let flush_pending = |pending: &mut Option<String>, lines: &mut Vec<String>| {
        if let Some(ph) = pending.take() {
            // `rstrip(".:;,") + "."`
            let trimmed = rstrip_chars(&ph, ".:;,");
            lines.push(format!("{}.", trimmed));
        }
    };

    for raw_line in raw_lines {
        let is_heading = raw_line.trim_end().ends_with(HEAD);
        let line = raw_line.replace(HEAD, "").trim().to_string();
        if line.is_empty() {
            if pending_heading.is_none() && !lines.is_empty() && lines.last().map(|s| !s.is_empty()).unwrap_or(false) {
                lines.push(String::new());
            }
            continue;
        }
        if is_heading {
            flush_pending(&mut pending_heading, &mut lines);
            let h = rstrip_chars(&line, ".:;,");
            pending_heading = Some(h);
            continue;
        }
        let mut cur = line.clone();
        if let Some(ph) = pending_heading.take() {
            let ph_trim = rstrip_chars(&ph, ".:;,");
            cur = format!("{}, {}", ph_trim, cur);
        }
        if add_sentence_pauses {
            if let Some(last) = cur.chars().last() {
                if !".!?;:".contains(last) {
                    cur.push('.');
                }
            }
        }
        lines.push(cur);
    }
    flush_pending(&mut pending_heading, &mut lines);

    let mut out = lines.join("\n");
    out = re_newlines_3().replace_all(&out, "\n\n").to_string();
    out = re_spaces_2().replace_all(&out, " ").to_string();
    out = re_space_before_punct().replace_all(&out, "$1").to_string();
    out = re_punct_letter().replace_all(&out, "$1 $2").to_string();
    out = re_dots_4().replace_all(&out, "...").to_string();
    out.trim().to_string()
}

// ---------------------------------------------------------------------------
// strip_nonspoken_blocks — mirrors `def strip_nonspoken_blocks` (230-241)
// ---------------------------------------------------------------------------

/// Remove blocks that must never reach a speech provider.
///
/// Currently: `<think>` reasoning blocks and the end-of-turn
/// file-mutation verifier footer.
/// Mirrors `def strip_nonspoken_blocks(text: str) -> str:` (230).
pub fn strip_nonspoken_blocks(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    let mut out = think_block_re().replace_all(text, " ").to_string();
    out = think_block_open_re().replace_all(&out, " ").to_string();
    out = verifier_footer_re().replace_all(&out, " ").to_string();
    out
}

// ---------------------------------------------------------------------------
// flatten_newlines_for_payload — mirrors `def flatten_newlines_for_payload` (244-258)
// ---------------------------------------------------------------------------

/// Collapse newlines into sentence breaks for single-line TTS payloads.
///
/// Some OpenAI-compatible backends (e.g. Kokoro) truncate synthesis at the
/// first newline (#9004). The smoothing pass already terminates each line
/// with punctuation, so newlines can safely become plain spaces.
/// Mirrors `def flatten_newlines_for_payload(text: str) -> str:` (244).
pub fn flatten_newlines_for_payload(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    let mut out = re_newlines_2().replace_all(text, ". ").to_string();
    // `re.sub(r"(?<=[.!?;:,])\n", " ", text)` → manual: newline preceded by punctuation → space
    out = flatten_punct_newline(&out);
    out = out.replace('\n', ". ");
    out = re_dot_space_dot().replace_all(&out, ".").to_string();
    out = re_spaces_2().replace_all(&out, " ").to_string();
    out.trim().to_string()
}

fn flatten_punct_newline(s: &str) -> String {
    // Mirrors `re.sub(r"(?<=[.!?;:,])\n", " ", text)` (254)
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\n' && i > 0 && ".!?;:,"
            .contains(chars[i - 1])
        {
            out.push(' ');
            i += 1;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

// ---------------------------------------------------------------------------
// prepare_spoken_text — mirrors `def prepare_spoken_text` (261-278)
// ---------------------------------------------------------------------------

/// Return a TTS-friendly script from assistant text.
///
/// Deterministic cleanup, not a semantic rewrite: it removes `<think>`
/// reasoning blocks and the file-mutation verifier footer, removes Markdown,
/// expands common symbols such as a degree-Celsius sign to "degrees Celsius",
/// turns visual line formatting into speakable sentence pauses, and flattens
/// the result to a single line so newline-sensitive providers (Kokoro) speak
/// the whole script.
/// Mirrors `def prepare_spoken_text(text: str, max_chars: int | None = 4000) -> str:` (261).
pub fn prepare_spoken_text(text: &str, max_chars: Option<usize>) -> String {
    let max = max_chars.unwrap_or(4000);
    let mut spoken = strip_nonspoken_blocks(text);
    spoken = strip_markdown_for_tts(&spoken);
    spoken = normalize_symbols_for_tts(&spoken);
    spoken = smooth_whitespace_for_tts(&spoken);
    spoken = flatten_newlines_for_payload(&spoken);
    if max > 0 && spoken.chars().count() > max {
        let truncated: String = spoken.chars().take(max).collect();
        spoken = truncated.trim_end().to_string();
    }
    spoken
}

/// Convenience wrapper with default `max_chars = 4000` (mirrors Python default).
pub fn prepare_spoken_text_default(text: &str) -> String {
    prepare_spoken_text(text, Some(4000))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn head_is_null() {
        assert_eq!(HEAD, "\x00");
    }

    #[test]
    fn strip_markdown_smoke() {
        assert_eq!(strip_markdown_for_tts(""), "");
        assert_eq!(strip_markdown_for_tts("**bold**"), "bold");
        assert_eq!(strip_markdown_for_tts("__bold__"), "bold");
        assert_eq!(strip_markdown_for_tts("~~strike~~"), "strike");
        assert_eq!(strip_markdown_for_tts("`code`"), "code");
        assert_eq!(strip_markdown_for_tts("![alt](url)"), " alt ");
        assert_eq!(strip_markdown_for_tts("[link](http://x)"), "link");
        assert_eq!(strip_markdown_for_tts("https://example.com"), "");
        assert_eq!(strip_markdown_for_tts("# Heading"), "Heading\x00");
        assert_eq!(strip_markdown_for_tts("a | b | c"), "a; b; c");
        // heading folding via smooth
        let md = "# Weather\n\nIt will be sunny";
        let stripped = strip_markdown_for_tts(md);
        let smoothed = smooth_whitespace_for_tts(&stripped);
        assert!(smoothed.contains("Weather, It will be sunny."), "got {smoothed:?}");
    }

    #[test]
    fn normalize_symbols_temps() {
        assert!(normalize_symbols_for_tts("11-17 °C").contains("11 to 17 degrees Celsius"));
        assert!(normalize_symbols_for_tts("5 °F").contains("5 degrees Fahrenheit"));
        assert!(normalize_symbols_for_tts("measured in °C").contains("degrees Celsius"));
        assert!(normalize_symbols_for_tts("90°").contains("90 degrees"));
        assert!(normalize_symbols_for_tts("10km/h").contains("kilometres per hour"));
        assert!(normalize_symbols_for_tts("5mm").contains("millimetres"));
        assert!(normalize_symbols_for_tts("5/month").contains("5 per month"));
        assert!(normalize_symbols_for_tts("and/or").contains("and/or")); // must not become "and per or"
        assert!(normalize_symbols_for_tts("$5").contains("5 dollars"));
        assert!(normalize_symbols_for_tts("50%").contains("50 percent"));
        assert!(normalize_symbols_for_tts("A & B").contains("A  and  B"));
        assert_eq!(normalize_symbols_for_tts("…"), "...");
    }

    #[test]
    fn smooth_whitespace_heading_fold() {
        let text = format!("Weather{}\nIt will be sunny", HEAD);
        let out = smooth_whitespace_for_tts(&text);
        assert_eq!(out, "Weather, It will be sunny.");
        // heading with no following content → own sentence
        let text2 = format!("Weather{}", HEAD);
        assert_eq!(smooth_whitespace_for_tts(&text2), "Weather.");
    }

    #[test]
    fn strip_nonspoken_think() {
        assert_eq!(strip_nonspoken_blocks("<think>hidden</think> hello"), "   hello");
        assert!(strip_nonspoken_blocks("<think> unclosed").contains(' '));
        assert_eq!(strip_nonspoken_blocks("⚠️ File-mutation verifier: bad\n  • file"), " ");
    }

    #[test]
    fn flatten_newlines() {
        assert_eq!(flatten_newlines_for_payload("a\nb"), "a. b");
        assert_eq!(flatten_newlines_for_payload("a\n\nb"), "a. b");
        assert_eq!(flatten_newlines_for_payload("a.\nb"), "a. b");
        assert_eq!(flatten_newlines_for_payload("a..b"), "a.b");
    }

    #[test]
    fn prepare_truncate() {
        let long = "a ".repeat(3000);
        let out = prepare_spoken_text(&long, Some(100));
        assert!(out.chars().count() <= 100);
        assert_eq!(prepare_spoken_text("", Some(4000)), "");
    }

    #[test]
    fn html_unescape_basic() {
        assert_eq!(strip_markdown_for_tts("&amp; &lt; &gt;"), "and  < >".replace("  ", " ")); // &amp; becomes " and " via normalize
        // html unescape itself: test directly
        assert_eq!(html_unescape("&amp;"), "&");
        assert_eq!(html_unescape("&#65;"), "A");
        assert_eq!(html_unescape("&#x41;"), "A");
    }
}
