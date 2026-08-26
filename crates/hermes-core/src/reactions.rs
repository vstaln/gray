//! Token-free detection of user *reactions* to the agent.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/agent/reactions.py` (56 lines).
//!
//! Currently the only reaction is [`VIBE`] — an expression of affection or
//! gratitude toward the agent (`ily`, `<3`, `love you`, `good bot`, a heart
//! emoji, …). Detection is a curated lexicon: **no model call, no tokens**.
//!
//! This is the single source of truth shared by every surface — the CLI pet, the
//! TUI heart, and the desktop floating hearts all react off the same signal,
//! delivered via `AIAgent.reaction_callback` (wired per interactive host).
//!
//! Generalized on purpose: [`detect_reaction`] returns a reaction *kind*
//! string, so new kinds (other emoji reactions, etc.) can be added here without
//! touching any caller. We match affection specifically — not general positive
//! sentiment — so "this is great" does NOT fire, but "good bot" / "❤️" do.
//!
//! Python source docstring (preserved):
//! ```text
//! Token-free detection of user *reactions* to the agent.
//!
//! Currently the only reaction is ``vibe`` — an expression of affection or
//! gratitude toward the agent (``ily``, ``<3``, ``love you``, ``good bot``, a heart
//! emoji, …). Detection is a curated regex/lexicon: **no model call, no tokens**.
//!
//! This is the single source of truth shared by every surface — the CLI pet, the
//! TUI heart, and the desktop floating hearts all react off the same signal,
//! delivered via ``AIAgent.reaction_callback`` (wired per interactive host).
//!
//! Generalized on purpose: :func:`detect_reaction` returns a reaction *kind*
//! string, so new kinds (other emoji reactions, etc.) can be added here without
//! touching any caller. We match affection specifically — not general positive
//! sentiment — so "this is great" does NOT fire, but "good bot" / "❤️" do.
//! ```

// ---------------------------------------------------------------------------
// Constants — mirrors lines 21-45
// ---------------------------------------------------------------------------

/// The affection/gratitude reaction — the only kind today.
/// Mirrors `VIBE = "vibe"` (line 22).
pub const VIBE: &str = "vibe";

// Keep underscore-prefixed alias for 1:1 traceability with Python name.
#[allow(dead_code)]
const _VIBE: &str = VIBE;

// Original regex patterns — preserved for traceability (lines 26-45).
// Implemented without the `regex` crate (NEVER cargo) via manual scanning.
#[allow(dead_code)]
const _VIBE_RE_GOOD_BOT: &str = r"\bgood\s*bot\b";
#[allow(dead_code)]
const _VIBE_RE_I_LOVE_YOU: &str = r"\bi\s*(?:love|luv)\s*(?:you|u|ya)\b";
#[allow(dead_code)]
const _VIBE_RE_LOVE_YOU: &str = r"\b(?:love|luv)\s*(?:you|u|ya)\b";
#[allow(dead_code)]
const _VIBE_RE_ILY: &str = r"\bily(?:sm)?\b";
#[allow(dead_code)]
const _VIBE_RE_THANK_YOU: &str = r"\bthank\s*(?:you|u)\b";
#[allow(dead_code)]
const _VIBE_RE_THANKS: &str = r"\b(?:thanks|thx|tysm|ty)\b";
#[allow(dead_code)]
const _VIBE_RE_LT3: &str = r"<3+";
#[allow(dead_code)]
const _VIBE_RE_HEARTS: &str =
    r"[\u2764\u2665\U0001F970\U0001F60D\U0001F618\U0001F495\U0001F496\U0001F497\U0001F49E\U0001F49B\U0001F49C\U0001F49A\U0001F499\U0001F493\U0001F498\U0001F49D\U0001FA77]";

/// Heart emoji codepoints that trigger `VIBE` — mirrors the character class
/// on lines 37-41 (case: 17 codepoints).
const HEARTS: &[char] = &[
    '\u{2764}', // ❤  HEAVY BLACK HEART
    '\u{2665}', // ♥  BLACK HEART SUIT
    '\u{1F970}', // 🥰  SMILING FACE WITH HEARTS
    '\u{1F60D}', // 😍  SMILING FACE WITH HEART-EYES
    '\u{1F618}', // 😘  FACE BLOWING A KISS
    '\u{1F495}', // 💕  TWO HEARTS
    '\u{1F496}', // 💖  SPARKLING HEART
    '\u{1F497}', // 💗  GROWING HEART
    '\u{1F49E}', // 💞  REVOLVING HEARTS
    '\u{1F49B}', // 💛  YELLOW HEART
    '\u{1F49C}', // 💜  PURPLE HEART
    '\u{1F49A}', // 💚  GREEN HEART
    '\u{1F499}', // 💙  BLUE HEART
    '\u{1F493}', // 💓  BEATING HEART
    '\u{1F498}', // 💘  HEART WITH ARROW
    '\u{1F49D}', // 💝  HEART WITH RIBBON
    '\u{1FA77}', // 🩷  PINK HEART
];

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

// ---------------------------------------------------------------------------
// Pattern checkers — mirrors _VIBE_RE alternatives (lines 28-41)
// All operate on the lowercased char slice for IGNORECASE semantics.
// ---------------------------------------------------------------------------

/// Mirrors `r"\bgood\s*bot\b"` (line 29).
fn contains_good_bot(chars: &[char]) -> bool {
    let n = chars.len();
    for i in 0..n {
        // \b before "good"
        if i > 0 && is_word_char(chars[i - 1]) {
            continue;
        }
        if i + 4 > n {
            continue;
        }
        if chars[i] != 'g' || chars[i + 1] != 'o' || chars[i + 2] != 'o' || chars[i + 3] != 'd' {
            continue;
        }
        // \s* (zero or more whitespace)
        let mut j = i + 4;
        while j < n && is_whitespace(chars[j]) {
            j += 1;
        }
        if j + 3 > n {
            continue;
        }
        if chars[j] != 'b' || chars[j + 1] != 'o' || chars[j + 2] != 't' {
            continue;
        }
        // \b after "bot"
        let after = j + 3;
        if after < n && is_word_char(chars[after]) {
            continue;
        }
        return true;
    }
    false
}

/// Mirrors `r"\bi\s*(?:love|luv)\s*(?:you|u|ya)\b"` (line 30).
fn contains_i_love_you(chars: &[char]) -> bool {
    let n = chars.len();
    for i in 0..n {
        if chars[i] != 'i' {
            continue;
        }
        if i > 0 && is_word_char(chars[i - 1]) {
            continue;
        }
        // \s* after "i"
        let mut j = i + 1;
        while j < n && is_whitespace(chars[j]) {
            j += 1;
        }
        if j >= n {
            continue;
        }
        // Try "love" then "luv" at j
        for (word, word_len) in [("love", 4), ("luv", 3)] {
            if j + word_len > n {
                continue;
            }
            let slice: String = chars[j..j + word_len].iter().collect();
            if slice != word {
                continue;
            }
            // \s* after love/luv
            let mut k = j + word_len;
            while k < n && is_whitespace(chars[k]) {
                k += 1;
            }
            if k >= n {
                continue;
            }
            // Try "you" (3), "ya" (2), "u" (1) at k
            // "you"
            if k + 3 <= n && chars[k] == 'y' && chars[k + 1] == 'o' && chars[k + 2] == 'u' {
                let after = k + 3;
                if after == n || !is_word_char(chars[after]) {
                    return true;
                }
            }
            // "ya"
            if k + 2 <= n && chars[k] == 'y' && chars[k + 1] == 'a' {
                let after = k + 2;
                if after == n || !is_word_char(chars[after]) {
                    return true;
                }
            }
            // "u"
            if chars[k] == 'u' {
                let after = k + 1;
                if after == n || !is_word_char(chars[after]) {
                    return true;
                }
            }
        }
    }
    false
}

/// Mirrors `r"\b(?:love|luv)\s*(?:you|u|ya)\b"` (line 31).
fn contains_love_you(chars: &[char]) -> bool {
    let n = chars.len();
    for i in 0..n {
        if i > 0 && is_word_char(chars[i - 1]) {
            continue;
        }
        // Try "love" / "luv" at i
        for (word, word_len) in [("love", 4), ("luv", 3)] {
            if i + word_len > n {
                continue;
            }
            let slice: String = chars[i..i + word_len].iter().collect();
            if slice != word {
                continue;
            }
            let mut k = i + word_len;
            while k < n && is_whitespace(chars[k]) {
                k += 1;
            }
            if k >= n {
                continue;
            }
            // "you"
            if k + 3 <= n && chars[k] == 'y' && chars[k + 1] == 'o' && chars[k + 2] == 'u' {
                let after = k + 3;
                if after == n || !is_word_char(chars[after]) {
                    return true;
                }
            }
            // "ya"
            if k + 2 <= n && chars[k] == 'y' && chars[k + 1] == 'a' {
                let after = k + 2;
                if after == n || !is_word_char(chars[after]) {
                    return true;
                }
            }
            // "u"
            if chars[k] == 'u' {
                let after = k + 1;
                if after == n || !is_word_char(chars[after]) {
                    return true;
                }
            }
        }
    }
    false
}

/// Mirrors `r"\bily(?:sm)?\b"` (line 32).
fn contains_ily(chars: &[char]) -> bool {
    let n = chars.len();
    for i in 0..n {
        if i > 0 && is_word_char(chars[i - 1]) {
            continue;
        }
        if i + 3 > n {
            continue;
        }
        if chars[i] != 'i' || chars[i + 1] != 'l' || chars[i + 2] != 'y' {
            continue;
        }
        // Try "ilysm" (longest) first
        if i + 5 <= n && chars[i + 3] == 's' && chars[i + 4] == 'm' {
            let after = i + 5;
            if after == n || !is_word_char(chars[after]) {
                return true;
            }
            // "ilysm" matched but boundary failed → do not fall through to "ily"
            // because that would incorrectly match "ilysm..." as "ily".
            // However Python's regex would still try "ily" alternative: "ilysm" text
            // contains "ily" at same start, but \b after ily would see 's' (word char)
            // so it would fail as well. So continue scanning next i.
            continue;
        }
        // Try "ily" alone
        let after = i + 3;
        if after == n || !is_word_char(chars[after]) {
            return true;
        }
    }
    false
}

/// Mirrors `r"\bthank\s*(?:you|u)\b"` (line 33).
fn contains_thank_you(chars: &[char]) -> bool {
    let n = chars.len();
    for i in 0..n {
        if i > 0 && is_word_char(chars[i - 1]) {
            continue;
        }
        if i + 5 > n {
            continue;
        }
        if chars[i] != 't'
            || chars[i + 1] != 'h'
            || chars[i + 2] != 'a'
            || chars[i + 3] != 'n'
            || chars[i + 4] != 'k'
        {
            continue;
        }
        let mut k = i + 5;
        while k < n && is_whitespace(chars[k]) {
            k += 1;
        }
        if k >= n {
            continue;
        }
        // "you"
        if k + 3 <= n && chars[k] == 'y' && chars[k + 1] == 'o' && chars[k + 2] == 'u' {
            let after = k + 3;
            if after == n || !is_word_char(chars[after]) {
                return true;
            }
        }
        // "u"
        if chars[k] == 'u' {
            let after = k + 1;
            if after == n || !is_word_char(chars[after]) {
                return true;
            }
        }
    }
    false
}

/// Mirrors `r"\b(?:thanks|thx|tysm|ty)\b"` (line 34).
fn contains_thanks_variants(chars: &[char]) -> bool {
    let n = chars.len();
    for i in 0..n {
        if i > 0 && is_word_char(chars[i - 1]) {
            continue;
        }
        // "thanks" (6)
        if i + 6 <= n
            && chars[i] == 't'
            && chars[i + 1] == 'h'
            && chars[i + 2] == 'a'
            && chars[i + 3] == 'n'
            && chars[i + 4] == 'k'
            && chars[i + 5] == 's'
        {
            let after = i + 6;
            if after == n || !is_word_char(chars[after]) {
                return true;
            }
        }
        // "tysm" (4)
        if i + 4 <= n
            && chars[i] == 't'
            && chars[i + 1] == 'y'
            && chars[i + 2] == 's'
            && chars[i + 3] == 'm'
        {
            let after = i + 4;
            if after == n || !is_word_char(chars[after]) {
                return true;
            }
        }
        // "thx" (3)
        if i + 3 <= n && chars[i] == 't' && chars[i + 1] == 'h' && chars[i + 2] == 'x' {
            let after = i + 3;
            if after == n || !is_word_char(chars[after]) {
                return true;
            }
        }
        // "ty" (2) — shortest, check last so longer wins on boundary ambiguity
        if i + 2 <= n && chars[i] == 't' && chars[i + 1] == 'y' {
            let after = i + 2;
            if after == n || !is_word_char(chars[after]) {
                return true;
            }
        }
    }
    false
}

/// Mirrors `r"<3+"` (line 35) — not `</3`. Simple substring search.
#[inline]
fn contains_lt3(text: &str) -> bool {
    // The regex "<3+" matches "<" followed by one or more "3". Any occurrence
    // of "<3" satisfies it; "</3" is "<" + "/" + "3" and does NOT contain "<3".
    text.contains("<3")
}

/// Mirrors the hearts character class (lines 37-41).
#[inline]
fn contains_heart(text: &str) -> bool {
    text.chars().any(|c| HEARTS.contains(&c))
}

// ---------------------------------------------------------------------------
// Public API — mirrors `detect_reaction` (lines 48-56)
// ---------------------------------------------------------------------------

/// Return the reaction kind for `text` (currently [`VIBE`]), or `None`.
///
/// Pure, token-free, and safe to call on every user turn.
/// Mirrors `detect_reaction` (lines 48-56):
/// ```python
/// def detect_reaction(text: str | None) -> str | None:
///     if not text:
///         return None
///     return VIBE if _VIBE_RE.search(text) else None
/// ```
pub fn detect_reaction(text: Option<&str>) -> Option<&'static str> {
    let s = text?;
    if s.is_empty() {
        return None;
    }
    if is_vibe(s) {
        Some(VIBE)
    } else {
        None
    }
}

/// Convenience overload for `&str` — mirrors `detect_reaction("...")` when
/// the caller knows the value is present.
pub fn detect_reaction_str(text: &str) -> Option<&'static str> {
    detect_reaction(Some(text))
}

/// Internal: true iff any vibe pattern matches.
///
/// Mirrors `_VIBE_RE.search(text)` (line 56) — the union of all eight
/// alternatives, case-insensitive (`re.IGNORECASE`) and dependency-free.
fn is_vibe(text: &str) -> bool {
    // Fast paths that don't need lowercasing: hearts (unicode) and "<3"
    if contains_heart(text) {
        return true;
    }
    if contains_lt3(text) {
        return true;
    }
    // Lowercase once for all IGNORECASE word patterns.
    let lower = text.to_lowercase();
    let chars: Vec<char> = lower.chars().collect();
    if contains_good_bot(&chars) {
        return true;
    }
    if contains_i_love_you(&chars) {
        return true;
    }
    if contains_love_you(&chars) {
        return true;
    }
    if contains_ily(&chars) {
        return true;
    }
    if contains_thank_you(&chars) {
        return true;
    }
    if contains_thanks_variants(&chars) {
        return true;
    }
    false
}

// Keep underscore-prefixed alias for 1:1 traceability with Python private name.
#[allow(dead_code)]
fn _detect_reaction(text: Option<&str>) -> Option<&'static str> {
    detect_reaction(text)
}

#[allow(dead_code)]
const _VIBE_RE: &str = r"\bgood\s*bot\b|\bi\s*(?:love|luv)\s*(?:you|u|ya)\b|\b(?:love|luv)\s*(?:you|u|ya)\b|\bily(?:sm)?\b|\bthank\s*(?:you|u)\b|\b(?:thanks|thx|tysm|ty)\b|<3+|[\u2764\u2665\U0001F970\U0001F60D\U0001F618\U0001F495\U0001F496\U0001F497\U0001F49E\U0001F49B\U0001F49C\U0001F49A\U0001F499\U0001F493\U0001F498\U0001F49D\U0001FA77]";
