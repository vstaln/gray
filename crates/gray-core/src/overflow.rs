use std::sync::OnceLock;

use regex::RegexSet;

use crate::event::{StopReason, Usage};

const OVERFLOW_PATTERNS: &[&str] = &[
    r"prompt is too long",
    r"request_too_large",
    r"input is too long for requested model",
    r"exceeds the context window",
    r"exceeds (?:the )?(?:model'?s )?maximum context length(?: of [\d,]+ tokens?|\s*\([\d,]+\))",
    r"input token count.*exceeds the maximum",
    r"maximum prompt length is \d+",
    r"reduce the length of the messages",
    r"maximum context length is \d+ tokens",
    r"exceeds (?:the )?maximum allowed input length of [\d,]+ tokens?",
    r"input \(\d+ tokens\) is longer than the model'?s context length \(\d+ tokens\)",
    r"exceeds the limit of \d+",
    r"exceeds the available context size",
    r"greater than the context length",
    r"context window exceeds limit",
    r"exceeded model token limit",
    r"too large for model with \d+ maximum context length",
    r"prompt has [\d,]+ tokens?, but the configured context size is [\d,]+ tokens?",
    r"model_context_window_exceeded",
    r"prompt too long; exceeded (?:max )?context length",
    r"range of input length should be",
    r"context[_ ]length[_ ]exceeded",
    r"too many tokens",
    r"token limit exceeded",
    r"^4(?:00|13)\s*(?:status code)?\s*\(no body\)",
];

const NON_OVERFLOW_PATTERNS: &[&str] = &[
    r"^(?:Throttling error|Service unavailable):",
    r"rate limit",
    r"too many requests",
];

fn extra_patterns(name: &str) -> Vec<String> {
    let Some(path) = crate::paths::gray_home().map(|h| h.join(name)) else {
        return Vec::new();
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter(|l| regex::Regex::new(l).is_ok())
        .map(String::from)
        .collect()
}

fn set(patterns: &[&str], extra_file: &str) -> RegexSet {
    let mut owned: Vec<String> = patterns.iter().map(|s| (*s).to_string()).collect();
    owned.extend(extra_patterns(extra_file));
    regex::RegexSetBuilder::new(owned.iter().map(String::as_str))
        .case_insensitive(true)
        .build()
        .expect("overflow patterns compile")
}

fn overflow_set() -> &'static RegexSet {
    static S: OnceLock<RegexSet> = OnceLock::new();
    S.get_or_init(|| set(OVERFLOW_PATTERNS, "overflow-patterns.txt"))
}

fn non_overflow_set() -> &'static RegexSet {
    static S: OnceLock<RegexSet> = OnceLock::new();
    S.get_or_init(|| set(NON_OVERFLOW_PATTERNS, "non-overflow-patterns.txt"))
}

pub fn is_non_overflow_message(msg: &str) -> bool {
    non_overflow_set().is_match(msg)
}

pub fn is_context_overflow_message(msg: &str) -> bool {
    !is_non_overflow_message(msg) && overflow_set().is_match(msg)
}

pub fn is_silent_overflow(usage: &Usage, stop: StopReason, window: usize) -> bool {
    if window == 0 {
        return false;
    }
    // `input_tokens` already counts cached reads and writes on every provider
    // (see `Usage::normalize`); adding `cache_read` again fired at half the window.
    let input = usage.input_tokens.max(
        usage.non_cached_input_tokens
            + usage.cache_read_input_tokens
            + usage.cache_write_input_tokens,
    );
    match stop {
        StopReason::EndTurn => input > window,
        StopReason::MaxTokens => usage.output_tokens == 0 && input >= window * 99 / 100,
        _ => false,
    }
}
