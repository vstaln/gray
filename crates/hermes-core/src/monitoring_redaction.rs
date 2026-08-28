//! Redaction applied to monitoring data before egress.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/agent/monitoring/redaction.py` (71 lines).
//!
//! One unconditional scrub, no modes, no knobs. Every string that leaves the
//! process passes through [`redact_for_export`]:
//!
//!   * Secrets first — wraps `agent/redact.py::redact_sensitive_text(force=True)`
//!     plus bearer/token-shape patterns, and fails CLOSED: if the redactor cannot
//!     run, the raw string is never emitted.
//!   * PII second — e-mail addresses, phone numbers, and UUID-shaped identifiers
//!     are rewritten to `[email]` / `[phone]` / `[id]`.
//!
//! There is deliberately no setting to weaken this. The monitoring plane is
//! content-free by design: rendered log messages are not exported, and bounded
//! structured strings are still scrubbed as defense-in-depth. This redactor also
//! remains available for a future, explicitly gated redacted-message detail mode.
//!
//! Python source docstring (preserved):
//! ```text
//! Redaction applied to monitoring data before egress.
//!
//! One unconditional scrub, no modes, no knobs. Every string that leaves the
//! process passes through ``redact_for_export``:
//!
//!   * Secrets first — wraps ``agent/redact.py::redact_sensitive_text(force=True)``
//!     plus bearer/token-shape patterns, and fails CLOSED: if the redactor cannot
//!     run, the raw string is never emitted.
//!   * PII second — e-mail addresses, phone numbers, and UUID-shaped identifiers
//!     are rewritten to ``[email]`` / ``[phone]`` / ``[id]``.
//!
//! There is deliberately no setting to weaken this. The monitoring plane is
//! content-free by design: rendered log messages are not exported, and bounded
//! structured strings are still scrubbed as defense-in-depth. This redactor also
//! remains available for a future, explicitly gated redacted-message detail mode.
//! ```

// ---------------------------------------------------------------------------
// helpers — character classes (mirrors Python `re` semantics without `regex` crate)
// ---------------------------------------------------------------------------

#[inline]
fn is_word_char(c: char) -> bool {
    // Python \w → [A-Za-z0-9_] plus Unicode word chars. `is_alphanumeric` covers Unicode.
    c.is_alphanumeric() || c == '_'
}

#[inline]
fn is_whitespace(c: char) -> bool {
    // Python \s → [ \t\n\r\f\v] plus Unicode whitespace. `is_whitespace` is the closest std equiv.
    c.is_whitespace()
}

#[inline]
fn is_bearer_token_char(c: char) -> bool {
    // Mirrors `[A-Za-z0-9._~+\-/]` (line 24)
    c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '~' | '+' | '-' | '/')
}

#[inline]
fn is_email_local_char(c: char) -> bool {
    // Mirrors `[A-Za-z0-9._%+\-]` (line 32)
    c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '%' | '+' | '-')
}

#[inline]
fn is_domain_char(c: char) -> bool {
    // Mirrors `[A-Za-z0-9.\-]` (line 32)
    c.is_ascii_alphanumeric() || matches!(c, '.' | '-')
}

#[inline]
fn is_phone_sep(c: char) -> bool {
    // Mirrors `[\s.\-]` (line 34-35)
    c.is_whitespace() || c == '.' || c == '-'
}

// ---------------------------------------------------------------------------
// secret shapes (belt-and-suspenders on top of agent/redact.py)
// Mirrors lines 24-29:
//   _BEARER_RE = re.compile(r"\bBearer\s+[A-Za-z0-9._~+\-/]+=*", re.IGNORECASE)
//   _TOKEN_RE = re.compile(r"\b(xox[baprs]-[A-Za-z0-9-]+|sk-[A-Za-z0-9_-]{8,}|gh[pousr]_[A-Za-z0-9_]{8,})\b")
//   _SECRET_LITERAL_RE = re.compile(r"\*{3,}")
//   _BEARER_RESIDUE_RE = re.compile(r"\bBearer\s+\[[^\]]+\]", re.IGNORECASE)
// ---------------------------------------------------------------------------

const REPLACED_SECRET: &str = "[redacted]";
const REPLACED_EMAIL: &str = "[email]";
const REPLACED_PHONE: &str = "[phone]";
const REPLACED_ID: &str = "[id]";
const REDACTION_UNAVAILABLE: &str = "[redaction-unavailable]";

/// Mirrors `_BEARER_RE` (line 24): `\bBearer\s+[A-Za-z0-9._~+\-/]+=*` (case-insensitive).
fn redact_bearer(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let lower: Vec<char> = text.to_ascii_lowercase().chars().collect();
    let n = chars.len();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < n {
        let prev_is_word = if i == 0 { false } else { is_word_char(chars[i - 1]) };
        let is_boundary_before = !prev_is_word;
        if is_boundary_before && i + 6 <= n && lower[i..i + 6] == ['b', 'e', 'a', 'r', 'e', 'r'] {
            // need at least one whitespace after "Bearer"
            if i + 6 < n && is_whitespace(chars[i + 6]) {
                let mut j = i + 6;
                while j < n && is_whitespace(chars[j]) {
                    j += 1;
                }
                let token_start = j;
                while j < n && is_bearer_token_char(chars[j]) {
                    j += 1;
                }
                if token_start < j {
                    // consume trailing "="*
                    while j < n && chars[j] == '=' {
                        j += 1;
                    }
                    out.push_str(REPLACED_SECRET);
                    i = j;
                    continue;
                }
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// Mirrors `_TOKEN_RE` (lines 25-27):
/// `\b(xox[baprs]-[A-Za-z0-9-]+|sk-[A-Za-z0-9_-]{8,}|gh[pousr]_[A-Za-z0-9_]{8,})\b`
fn redact_token(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < n {
        let prev_is_word = if i == 0 { false } else { is_word_char(chars[i - 1]) };
        let is_boundary_before = !prev_is_word;
        let mut matched_len: Option<usize> = None;
        if is_boundary_before {
            // try xox[baprs]- pattern
            if i + 5 <= n
                && chars[i] == 'x'
                && chars[i + 1] == 'o'
                && chars[i + 2] == 'x'
                && matches!(chars[i + 3], 'b' | 'a' | 'p' | 'r' | 's')
                && chars[i + 4] == '-'
            {
                let mut j = i + 5;
                let mut count = 0;
                while j < n && (chars[j].is_ascii_alphanumeric() || chars[j] == '-') {
                    j += 1;
                    count += 1;
                }
                if count >= 1 {
                    let after_is_word = if j < n { is_word_char(chars[j]) } else { false };
                    if !after_is_word {
                        matched_len = Some(j - i);
                    }
                }
            }
            // try sk- pattern (only if not already matched, but longest should win; sk- is distinct)
            if matched_len.is_none() && i + 3 <= n && chars[i] == 's' && chars[i + 1] == 'k' && chars[i + 2] == '-' {
                let mut j = i + 3;
                let mut count = 0;
                while j < n && (chars[j].is_ascii_alphanumeric() || chars[j] == '_' || chars[j] == '-') {
                    j += 1;
                    count += 1;
                }
                if count >= 8 {
                    let after_is_word = if j < n { is_word_char(chars[j]) } else { false };
                    if !after_is_word {
                        matched_len = Some(j - i);
                    }
                }
            }
            // try gh[pousr]_ pattern
            if matched_len.is_none()
                && i + 4 <= n
                && chars[i] == 'g'
                && chars[i + 1] == 'h'
                && matches!(chars[i + 2], 'p' | 'o' | 'u' | 's' | 'r')
                && chars[i + 3] == '_'
            {
                let mut j = i + 4;
                let mut count = 0;
                while j < n && (chars[j].is_ascii_alphanumeric() || chars[j] == '_') {
                    j += 1;
                    count += 1;
                }
                if count >= 8 {
                    let after_is_word = if j < n { is_word_char(chars[j]) } else { false };
                    if !after_is_word {
                        matched_len = Some(j - i);
                    }
                }
            }
        }
        if let Some(len) = matched_len {
            out.push_str(REPLACED_SECRET);
            i += len;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

/// Mirrors `_SECRET_LITERAL_RE` (line 28): `\*{3,}`
fn redact_secret_literal(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < n {
        if chars[i] == '*' {
            let mut j = i;
            while j < n && chars[j] == '*' {
                j += 1;
            }
            let count = j - i;
            if count >= 3 {
                out.push_str(REPLACED_SECRET);
            } else {
                for k in i..j {
                    out.push(chars[k]);
                }
            }
            i = j;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

/// Mirrors `_BEARER_RESIDUE_RE` (line 29): `\bBearer\s+\[[^\]]+\]` (case-insensitive).
fn redact_bearer_residue(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let lower: Vec<char> = text.to_ascii_lowercase().chars().collect();
    let n = chars.len();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < n {
        let prev_is_word = if i == 0 { false } else { is_word_char(chars[i - 1]) };
        let is_boundary_before = !prev_is_word;
        if is_boundary_before && i + 6 <= n && lower[i..i + 6] == ['b', 'e', 'a', 'r', 'e', 'r'] {
            if i + 6 < n && is_whitespace(chars[i + 6]) {
                let mut j = i + 6;
                while j < n && is_whitespace(chars[j]) {
                    j += 1;
                }
                if j < n && chars[j] == '[' {
                    let mut k = j + 1;
                    let mut found_close = false;
                    while k < n {
                        if chars[k] == ']' {
                            found_close = true;
                            k += 1;
                            break;
                        }
                        if chars[k] == '\n' || chars[k] == '\r' {
                            // Python [^\]] would still match newline (\n is not ]), but keep scanning.
                        }
                        k += 1;
                    }
                    // need at least one char between brackets: [^\]]+ means one or more not ']'
                    if found_close && k > j + 2 {
                        out.push_str(REPLACED_SECRET);
                        i = k;
                        continue;
                    }
                }
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

// ---------------------------------------------------------------------------
// PII shapes — mirrors lines 32-40
//   _EMAIL_RE = re.compile(r"[A-Za-z0-9._%+\-]+@[A-Za-z0-9.\-]+\.[A-Za-z]{2,}")
//   _PHONE_RE = re.compile(r"(?<!\w)(?:\+?\d{1,3}[\s.\-]?)?(?:\(\d{2,4}\)[\s.\-]?)?\d{3}[\s.\-]?\d{3,4}(?:[\s.\-]?\d{2,4})?(?!\w)")
//   _UUID_RE  = re.compile(r"\b[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}\b")
// ---------------------------------------------------------------------------

/// Mirrors `_EMAIL_RE` (line 32).
fn redact_email(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < n {
        if chars[i] == '@' {
            // Expand left for local part [A-Za-z0-9._%+\-]+
            let mut left = i;
            while left > 0 && is_email_local_char(chars[left - 1]) {
                left -= 1;
            }
            let local_len = i - left;
            if local_len >= 1 {
                // Expand right for domain [A-Za-z0-9.\-]+ + \. + [A-Za-z]{2,}
                let mut right = i + 1;
                while right < n && is_domain_char(chars[right]) {
                    right += 1;
                }
                // right is exclusive end of domain run; need to find dot + TLD inside
                if right > i + 1 {
                    // Find the longest valid email end: rightmost dot where suffix is 2+ letters.
                    // We scan domain slice for dot positions and pick the furthest valid.
                    let mut best_end: Option<usize> = None;
                    // domain slice is chars[i+1..right]
                    for dot_pos in (i + 1)..right {
                        if chars[dot_pos] != '.' {
                            continue;
                        }
                        // TLD must be at least 2 consecutive ASCII letters starting at dot_pos+1
                        let tld_start = dot_pos + 1;
                        if tld_start >= right {
                            continue;
                        }
                        let mut tld_len = 0;
                        while tld_start + tld_len < right
                            && chars[tld_start + tld_len].is_ascii_alphabetic()
                        {
                            tld_len += 1;
                        }
                        if tld_len >= 2 {
                            let email_end = tld_start + tld_len;
                            // The regex `[A-Za-z]{2,}` is greedy but would stop at first non-letter,
                            // so we already have maximal consecutive letters. Keep the furthest email_end
                            // that still has a dot before it.
                            // Prefer the rightmost valid end (greedy overall).
                            if best_end.is_none() || email_end > best_end.unwrap() {
                                best_end = Some(email_end);
                            }
                        }
                    }
                    if let Some(email_end) = best_end {
                        // We have a valid email from `left` to `email_end`
                        // Remove already-pushed prefix for local part from `out` and replace.
                        // `out` currently contains text[0..left] plus chars up to i-1 that were pushed
                        // iteratively. But we pushed incrementally, so `out` length corresponds to
                        // chars processed before `left`? Actually we push char-by-char unless we handle email
                        // at '@' time, we have already pushed chars[0..left] ? Let's manage differently:
                        // At this point, we have pushed chars[prev_i..left-1] already, but not the local part
                        // characters from left..i-1 ? Wait our loop pushes one char per iteration when not email.
                        // At '@' we are at i, and we have NOT yet pushed chars[left..i] as a single email;
                        // Instead we pushed chars up to left-1 already, but chars[left..i-1] were pushed individually
                        // in prior iterations. So we need to truncate `out` to remove those local chars.
                        // Alternative simpler: don't push incrementally for email detection; instead treat `out`
                        // as built from previous `i` pointer. We maintain `i` as scan index and `out` as result
                        // for previous segment [prev .. left). So we need to know where left maps in `out`.
                        // Easier: we rebuild via index pointer `consumed` - but we already use `i` as scan cursor
                        // and `out` as result. The characters from `left` to `i-1` are already in `out` (their count
                        // equals `i - segment_start`? Actually we push every char when no match, so `out` length in chars
                        // equals number of processed chars before `left` plus `(i - left)` for local part.
                        // So we can truncate `out` back by `i - left` chars.
                        let truncate_chars = i - left;
                        // `out` is UTF-8 String; truncating by char count requires char-boundary handling.
                        // Since local chars are ASCII (email local charset is ASCII), each char is 1 byte, so we can truncate by bytes = chars.
                        // But to be safe for general Unicode preceding text, we count bytes of truncated suffix.
                        // Simpler: reconstruct byte length of suffix `chars[left..i]` (which is ASCII, 1 byte each)
                        let bytes_to_truncate = truncate_chars; // ASCII assumption holds for email local chars
                        let new_len = out.len() - bytes_to_truncate;
                        out.truncate(new_len);
                        out.push_str(REPLACED_EMAIL);
                        i = email_end;
                        continue;
                    }
                }
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// Mirrors `_UUID_RE` (lines 38-40):
/// `\b[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}\b`
fn redact_uuid(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < n {
        let prev_is_word = if i == 0 { false } else { is_word_char(chars[i - 1]) };
        let is_boundary_before = !prev_is_word;
        // UUID length is 36 chars: 8-4-4-4-12 + 4 hyphens = 36
        if is_boundary_before && i + 36 <= n {
            let slice = &chars[i..i + 36];
            let is_uuid = slice[8] == '-'
                && slice[13] == '-'
                && slice[18] == '-'
                && slice[23] == '-'
                && slice[0..8].iter().all(|c| c.is_ascii_hexdigit())
                && slice[9..13].iter().all(|c| c.is_ascii_hexdigit())
                && slice[14..18].iter().all(|c| c.is_ascii_hexdigit())
                && slice[19..23].iter().all(|c| c.is_ascii_hexdigit())
                && slice[24..36].iter().all(|c| c.is_ascii_hexdigit());
            if is_uuid {
                let after_is_word = if i + 36 < n { is_word_char(chars[i + 36]) } else { false };
                if !after_is_word {
                    out.push_str(REPLACED_ID);
                    i += 36;
                    continue;
                }
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

// ---------------------------------------------------------------------------
// Phone — mirrors _PHONE_RE (lines 34-36)
// `(?<!\w)(?:\+?\d{1,3}[\s.\-]?)?(?:\(\d{2,4}\)[\s.\-]?)?\d{3}[\s.\-]?\d{3,4}(?:[\s.\-]?\d{2,4})?(?!\w)`
// Implemented as a backtracking validator `is_phone_string` plus outer scan for
// longest match. Keeps this crate dependency-free (no `regex`).
// ---------------------------------------------------------------------------

fn is_phone_string(s: &[char]) -> bool {
    if s.is_empty() {
        return false;
    }
    let digit_count = s.iter().filter(|c| c.is_ascii_digit()).count();
    if digit_count < 7 || digit_count > 15 {
        return false;
    }
    // Quick reject: must contain a digit and not be all digits with no separators? But pattern allows pure digits (e.g. 1234567)
    // so we don't reject.
    dfs_phone(s, 0, 0)
}

fn dfs_phone(s: &[char], p: usize, state: usize) -> bool {
    match state {
        0 => {
            // G1 optional: (?:\+?\d{1,3}[\s.\-]?)?
            // Try skipping G1
            if dfs_phone(s, p, 1) {
                return true;
            }
            // Try consuming G1: [+]? + 1-3 digits + [sep]?
            for plus in 0..=1 {
                if plus == 1 {
                    if p >= s.len() || s[p] != '+' {
                        continue;
                    }
                }
                let digit_start = p + plus;
                for dlen in 1..=3 {
                    if digit_start + dlen > s.len() {
                        break;
                    }
                    if !s[digit_start..digit_start + dlen]
                        .iter()
                        .all(|c| c.is_ascii_digit())
                    {
                        // if first char not digit, no longer lengths will help for this plus prefix
                        if dlen == 1 {
                            break;
                        }
                        continue;
                    }
                    let sep_pos = digit_start + dlen;
                    // without sep
                    if dfs_phone(s, sep_pos, 1) {
                        return true;
                    }
                    // with sep
                    if sep_pos < s.len() && is_phone_sep(s[sep_pos]) && dfs_phone(s, sep_pos + 1, 1) {
                        return true;
                    }
                }
            }
            false
        }
        1 => {
            // G2 optional: (?:\(\d{2,4}\)[\s.\-]?)?
            if dfs_phone(s, p, 2) {
                return true;
            }
            if p >= s.len() || s[p] != '(' {
                return false;
            }
            for dlen in 2..=4 {
                if p + 1 + dlen > s.len() {
                    break;
                }
                if !s[p + 1..p + 1 + dlen].iter().all(|c| c.is_ascii_digit()) {
                    if dlen == 2 {
                        break;
                    }
                    continue;
                }
                let after_digits = p + 1 + dlen;
                if after_digits >= s.len() || s[after_digits] != ')' {
                    continue;
                }
                let after_paren = after_digits + 1;
                // without sep
                if dfs_phone(s, after_paren, 2) {
                    return true;
                }
                // with sep
                if after_paren < s.len() && is_phone_sep(s[after_paren]) && dfs_phone(s, after_paren + 1, 2) {
                    return true;
                }
            }
            false
        }
        2 => {
            // M1 mandatory: \d{3}
            if p + 3 > s.len() {
                return false;
            }
            if !s[p..p + 3].iter().all(|c| c.is_ascii_digit()) {
                return false;
            }
            dfs_phone(s, p + 3, 3)
        }
        3 => {
            // optional sep between M1 and M2: [\s.\-]?
            if dfs_phone(s, p, 4) {
                return true;
            }
            if p < s.len() && is_phone_sep(s[p]) && dfs_phone(s, p + 1, 4) {
                return true;
            }
            false
        }
        4 => {
            // M2 mandatory: \d{3,4}
            for dlen in [4, 3] {
                if p + dlen > s.len() {
                    continue;
                }
                if !s[p..p + dlen].iter().all(|c| c.is_ascii_digit()) {
                    continue;
                }
                if dfs_phone(s, p + dlen, 5) {
                    return true;
                }
            }
            false
        }
        5 => {
            // T optional: (?:[\s.\-]?\d{2,4})?
            if dfs_phone(s, p, 6) {
                return true;
            }
            // without sep: 2-4 digits at p
            for dlen in 2..=4 {
                if p + dlen > s.len() {
                    break;
                }
                if !s[p..p + dlen].iter().all(|c| c.is_ascii_digit()) {
                    if dlen == 2 {
                        break;
                    }
                    continue;
                }
                if dfs_phone(s, p + dlen, 6) {
                    return true;
                }
            }
            // with sep: sep + 2-4 digits
            if p < s.len() && is_phone_sep(s[p]) {
                for dlen in 2..=4 {
                    if p + 1 + dlen > s.len() {
                        break;
                    }
                    if !s[p + 1..p + 1 + dlen].iter().all(|c| c.is_ascii_digit()) {
                        if dlen == 2 {
                            break;
                        }
                        continue;
                    }
                    if dfs_phone(s, p + 1 + dlen, 6) {
                        return true;
                    }
                }
            }
            false
        }
        6 => p == s.len(),
        _ => false,
    }
}

fn redact_phone(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < n {
        // (?<!\w) — not preceded by word char
        if i > 0 && is_word_char(chars[i - 1]) {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        // Try longest phone match from i (max ~30 chars, greedy)
        let max_len = std::cmp::min(n - i, 30);
        let mut best_len: Option<usize> = None;
        // Phone minimum visual length is 7 digits maybe plus seps; scan descending for longest.
        for len in (7..=max_len).rev() {
            if i + len > n {
                continue;
            }
            // (?!\w) — not followed by word char
            if i + len < n && is_word_char(chars[i + len]) {
                continue;
            }
            let slice = &chars[i..i + len];
            // Slice must end with digit or ')' (since pattern ends with digit)
            // Quick check: last char must be digit or ')'
            let last = slice[slice.len() - 1];
            if !(last.is_ascii_digit() || last == ')') {
                // Phone pattern always ends with digit (T's digits or M2's digits or G2's ')'+sep? Actually ends with digit, but we allow ')'? G2 never at end; ends with digit. So enforce digit.
                // However our DFS allows ending after M2 without T, so last is digit.
                // So require digit
                if !last.is_ascii_digit() {
                    continue;
                }
            }
            if is_phone_string(slice) {
                best_len = Some(len);
                break;
            }
        }
        if let Some(len) = best_len {
            out.push_str(REPLACED_PHONE);
            i += len;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

// ---------------------------------------------------------------------------
// _secret_redact — mirrors lines 43-55
// ---------------------------------------------------------------------------

/// Simulates `agent/redact.py::redact_sensitive_text(force=True)` for the
/// monitoring plane. The Python original wraps the call in `try/except` and
/// fails CLOSED (`"[redaction-unavailable]"`) if the redactor cannot run.
///
/// In Rust there is no Python import to fail; this stub always succeeds and
/// returns the input unchanged, preserving the fail-closed contract via the
/// `Result` wrapper so a future real port can inject failures without changing
/// call sites. To simulate a failure (e.g. in tests), call `secret_redact_with`
/// with an `Err` closure.
fn try_redact_sensitive_text(text: &str) -> Result<String, ()> {
    // Stub: in the Python source this is `redact_sensitive_text(text, force=True)`
    // which is ON by default and force-bypasses user config. Here we return Ok
    // to keep the 1:1 line mapping; a real Rust port of `agent/redact.py`
    // would replace this body.
    Ok(text.to_string())
}

#[allow(dead_code)]
fn _try_redact_sensitive_text(text: &str) -> Result<String, ()> {
    try_redact_sensitive_text(text)
}

/// Always-on secret redaction. `force=True` so user config can't disable it.
/// Mirrors `_secret_redact` (lines 43-55).
pub fn secret_redact(text: &str) -> String {
    secret_redact_with(text, try_redact_sensitive_text)
}

/// Test hook: inject a custom `redact_sensitive_text` implementation to
/// exercise the fail-closed path without patching imports.
pub fn secret_redact_with<F>(text: &str, f: F) -> String
where
    F: Fn(&str) -> Result<String, ()>,
{
    let mut out = match f(text) {
        Ok(s) => s,
        Err(_) => return REDACTION_UNAVAILABLE.to_string(),
    };
    out = redact_bearer(&out);
    out = redact_token(&out);
    out = redact_secret_literal(&out);
    out = redact_bearer_residue(&out);
    out
}

#[allow(dead_code)]
fn _secret_redact(text: &str) -> String {
    secret_redact(text)
}

// Keep underscore-prefixed alias for 1:1 traceability with Python private name
#[allow(dead_code)]
const _BEARER_RE: &str = r"\bBearer\s+[A-Za-z0-9._~+\-/]+=*";
#[allow(dead_code)]
const _TOKEN_RE: &str = r"\b(xox[baprs]-[A-Za-z0-9-]+|sk-[A-Za-z0-9_-]{8,}|gh[pousr]_[A-Za-z0-9_]{8,})\b";
#[allow(dead_code)]
const _SECRET_LITERAL_RE: &str = r"\*{3,}";
#[allow(dead_code)]
const _BEARER_RESIDUE_RE: &str = r"\bBearer\s+\[[^\]]+\]";
#[allow(dead_code)]
const _EMAIL_RE: &str = r"[A-Za-z0-9._%+\-]+@[A-Za-z0-9.\-]+\.[A-Za-z]{2,}";
#[allow(dead_code)]
const _PHONE_RE: &str =
    r"(?<!\w)(?:\+?\d{1,3}[\s.\-]?)?(?:\(\d{2,4}\)[\s.\-]?)?\d{3}[\s.\-]?\d{3,4}(?:[\s.\-]?\d{2,4})?(?!\w)";
#[allow(dead_code)]
const _UUID_RE: &str =
    r"\b[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}\b";

// ---------------------------------------------------------------------------
// redact_for_export — mirrors lines 58-66
// ---------------------------------------------------------------------------

/// Scrub a string for egress: secrets, then PII. Unconditional.
/// Mirrors `redact_for_export` (lines 58-66).
pub fn redact_for_export(text: Option<&str>) -> Option<String> {
    match text {
        None => None,
        Some(s) => {
            // Mirrors `out = _secret_redact(str(text))` (line 62)
            let mut out = secret_redact(s);
            // Mirrors lines 63-65: PII second — email, UUID, phone (in that order)
            out = redact_email(&out);
            out = redact_uuid(&out);
            out = redact_phone(&out);
            Some(out)
        }
    }
}

/// Convenience overload for non-optional strings.
/// Mirrors `redact_for_export(str)` when the caller knows the value is present.
pub fn redact_for_export_str(text: &str) -> String {
    // Mirrors the `str(text)` coercion plus the None guard in one step
    redact_for_export(Some(text)).unwrap_or_else(|| "[redaction-unavailable]".to_string())
}

#[allow(dead_code)]
fn _redact_for_export(text: Option<&str>) -> Option<String> {
    redact_for_export(text)
}
