//! Typed failure-reason codes for bot turns and relay replies (#93091).
//! Port of `tools/bot_failure_reasons.py` (172 lines) — 1:1 behavior.
//!
//! A closed vocabulary of machine-readable reason codes carried ALONGSIDE the
//! existing free-text `error` fields (additive schema — old consumers keep
//! working). Platform-side codes are assigned by the transport/relay layer;
//! agent-side codes are derived from raw agent/provider error text via
//! [`classify_agent_error`].
//!
//! Classifier precedence (deterministic, documented, tested):
//! 1. auth — an explicit `authentication_error` type, a 401/403 status, or
//!    "invalid api key" wins over everything else. Rationale: real provider
//!    401 bodies (e.g. Anthropic) say "invalid, blocked or out of funds" —
//!    quota words inside an auth error must not misclassify it.
//! 2. quota — 402 / out of funds / quota / balance.
//! 3. rate — 429 / rate limit.
//! 4. server — 5xx / server error / overloaded.
//! 5. context — context length / context_overflow / maximum context.
//! 6. config — No LLM provider configured / missing config / No access token.
//! 7. model — model not found / does not exist.
//! 8. unknown — anything else (including empty text).
//!
//! Retry session policy (#93091 item 5):
//! Maintainer ruling (2026-08-23, #93091): a retried bot turn NEVER mints a
//! fresh session. Transient classes resume the session as-is. context_overflow
//! runs context compression — the one sanctioned context mutation, already in
//! the agent core — on the same session and retries against the compacted
//! context. Everything else (auth/quota/config/model/unknown) is not
//! auto-retried at all: surface the typed reason and stop.
//!
//! Python mapping:
//! - `RUNTIME_OFFLINE = "runtime_offline"` (28) → [`RUNTIME_OFFLINE`]
//! - `QUEUED_EXPIRED = "queued_expired"` (29) → [`QUEUED_EXPIRED`]
//! - `DELIVERY_TIMEOUT = "delivery_timeout"` (30) → [`DELIVERY_TIMEOUT`]
//! - `AGENT_BLOCKED = "agent_blocked"` (31) → [`AGENT_BLOCKED`]
//! - `CANCELLED = "cancelled"` (32) → [`CANCELLED`]
//! - `PROVIDER_AUTH_OR_ACCESS = "provider_auth_or_access"` (35) → [`PROVIDER_AUTH_OR_ACCESS`]
//! - `PROVIDER_QUOTA_LIMIT = "provider_quota_limit"` (36) → [`PROVIDER_QUOTA_LIMIT`]
//! - `PROVIDER_RATE_LIMIT = "provider_rate_limit"` (37) → [`PROVIDER_RATE_LIMIT`]
//! - `PROVIDER_SERVER_ERROR = "provider_server_error"` (38) → [`PROVIDER_SERVER_ERROR`]
//! - `CONTEXT_OVERFLOW = "context_overflow"` (39) → [`CONTEXT_OVERFLOW`]
//! - `MISSING_CONFIG = "missing_config"` (40) → [`MISSING_CONFIG`]
//! - `MODEL_UNAVAILABLE = "model_unavailable"` (41) → [`MODEL_UNAVAILABLE`]
//! - `UNKNOWN = "unknown"` (42) → [`UNKNOWN`]
//! - `ALL_REASONS = frozenset({...})` (44-60) → [`ALL_REASONS`]
//! - `AUTO_RETRYABLE = frozenset({...})` (63-65) → [`AUTO_RETRYABLE`]
//! - `def is_auto_retryable(reason)` (68-70) → [`is_auto_retryable`]
//! - `RETRY_RESUME = "resume"` (83) → [`RETRY_RESUME`]
//! - `RETRY_COMPRESS_THEN_RESUME = "compress_then_resume"` (84) → [`RETRY_COMPRESS_THEN_RESUME`]
//! - `RETRY_NONE = "none"` (85) → [`RETRY_NONE`]
//! - `def retry_action(reason)` (88-104) → [`retry_action`]
//! - `_RULES: tuple[Pattern, str]` (109-156) → `auth_re`..`model_re` + [`classify_agent_error`]
//! - `def classify_agent_error(text)` (159-172) → [`classify_agent_error`]

use std::sync::OnceLock;

use regex::Regex;
use regex::RegexBuilder;

// ---------------------------------------------------------------------------
// Platform-side reason codes — mirrors lines 28-32
// ---------------------------------------------------------------------------

/// Mirrors `RUNTIME_OFFLINE = "runtime_offline"` (28).
pub const RUNTIME_OFFLINE: &str = "runtime_offline";
/// Mirrors `QUEUED_EXPIRED = "queued_expired"` (29).
pub const QUEUED_EXPIRED: &str = "queued_expired";
/// Mirrors `DELIVERY_TIMEOUT = "delivery_timeout"` (30).
pub const DELIVERY_TIMEOUT: &str = "delivery_timeout";
/// Mirrors `AGENT_BLOCKED = "agent_blocked"` (31).
pub const AGENT_BLOCKED: &str = "agent_blocked";
/// Mirrors `CANCELLED = "cancelled"` (32).
pub const CANCELLED: &str = "cancelled";

// ---------------------------------------------------------------------------
// Agent-side reason codes — mirrors lines 35-42
// ---------------------------------------------------------------------------

/// Mirrors `PROVIDER_AUTH_OR_ACCESS = "provider_auth_or_access"` (35).
pub const PROVIDER_AUTH_OR_ACCESS: &str = "provider_auth_or_access";
/// Mirrors `PROVIDER_QUOTA_LIMIT = "provider_quota_limit"` (36).
pub const PROVIDER_QUOTA_LIMIT: &str = "provider_quota_limit";
/// Mirrors `PROVIDER_RATE_LIMIT = "provider_rate_limit"` (37).
pub const PROVIDER_RATE_LIMIT: &str = "provider_rate_limit";
/// Mirrors `PROVIDER_SERVER_ERROR = "provider_server_error"` (38).
pub const PROVIDER_SERVER_ERROR: &str = "provider_server_error";
/// Mirrors `CONTEXT_OVERFLOW = "context_overflow"` (39).
pub const CONTEXT_OVERFLOW: &str = "context_overflow";
/// Mirrors `MISSING_CONFIG = "missing_config"` (40).
pub const MISSING_CONFIG: &str = "missing_config";
/// Mirrors `MODEL_UNAVAILABLE = "model_unavailable"` (41).
pub const MODEL_UNAVAILABLE: &str = "model_unavailable";
/// Mirrors `UNKNOWN = "unknown"` (42).
pub const UNKNOWN: &str = "unknown";

// ---------------------------------------------------------------------------
// Sets — mirrors lines 44-65
// ---------------------------------------------------------------------------

/// Mirrors `ALL_REASONS = frozenset({...})` (44-60) — all 13 reason codes.
pub const ALL_REASONS: &[&str] = &[
    RUNTIME_OFFLINE,
    QUEUED_EXPIRED,
    DELIVERY_TIMEOUT,
    AGENT_BLOCKED,
    CANCELLED,
    PROVIDER_AUTH_OR_ACCESS,
    PROVIDER_QUOTA_LIMIT,
    PROVIDER_RATE_LIMIT,
    PROVIDER_SERVER_ERROR,
    CONTEXT_OVERFLOW,
    MISSING_CONFIG,
    MODEL_UNAVAILABLE,
    UNKNOWN,
];

/// Mirrors `AUTO_RETRYABLE = frozenset({RUNTIME_OFFLINE, DELIVERY_TIMEOUT, PROVIDER_RATE_LIMIT, PROVIDER_SERVER_ERROR})` (63-65).
///
/// Reasons a supervisor may retry automatically without human intervention.
pub const AUTO_RETRYABLE: &[&str] = &[
    RUNTIME_OFFLINE,
    DELIVERY_TIMEOUT,
    PROVIDER_RATE_LIMIT,
    PROVIDER_SERVER_ERROR,
];

// ---------------------------------------------------------------------------
// Retry actions — mirrors lines 83-85
// ---------------------------------------------------------------------------

/// Mirrors `RETRY_RESUME = "resume"` (83).
pub const RETRY_RESUME: &str = "resume";
/// Mirrors `RETRY_COMPRESS_THEN_RESUME = "compress_then_resume"` (84).
pub const RETRY_COMPRESS_THEN_RESUME: &str = "compress_then_resume";
/// Mirrors `RETRY_NONE = "none"` (85).
pub const RETRY_NONE: &str = "none";

// ---------------------------------------------------------------------------
// is_auto_retryable — mirrors lines 68-70
// ---------------------------------------------------------------------------

/// Mirrors `def is_auto_retryable(reason: str) -> bool:` (68-70).
///
/// True when `reason` is safe to retry automatically.
pub fn is_auto_retryable(reason: &str) -> bool {
    AUTO_RETRYABLE.contains(&reason)
}

// ---------------------------------------------------------------------------
// retry_action — mirrors lines 88-104
// ---------------------------------------------------------------------------

/// Mirrors `def retry_action(reason: str) -> str:` (88-104).
///
/// Map a failure reason to the bot-turn retry action.
/// - transient (`AUTO_RETRYABLE`) → `"resume"`: retry the same session unchanged.
/// - `CONTEXT_OVERFLOW` → `"compress_then_resume"`: run context compression then retry.
/// - anything else → `"none"`.
pub fn retry_action(reason: &str) -> &'static str {
    if is_auto_retryable(reason) {
        return RETRY_RESUME;
    }
    if reason == CONTEXT_OVERFLOW {
        return RETRY_COMPRESS_THEN_RESUME;
    }
    RETRY_NONE
}

// ---------------------------------------------------------------------------
// Classifier rules — mirrors lines 109-156
// ---------------------------------------------------------------------------

fn build(pattern: &str) -> Regex {
    RegexBuilder::new(pattern)
        .case_insensitive(true)
        .build()
        .unwrap_or_else(|e| panic!("bot_failure_reasons: invalid regex {pattern:?}: {e}"))
}

// Mirrors first tuple in `_RULES` (110-117):
// `r"authentication_error|invalid api key|(?:error code:?\s*|status(?:\s*code)?:?\s*|http\s*)(?:401|403)\b"` IGNORECASE
fn auth_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        build(r"authentication_error|invalid api key|(?:error code:?\s*|status(?:\s*code)?:?\s*|http\s*)(?:401|403)\b")
    })
}

// Mirrors second tuple (118-125):
// `r"(?:error code:?\s*|status(?:\s*code)?:?\s*|http\s*)402\b|out of funds|quota|balance"` IGNORECASE
fn quota_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        build(r"(?:error code:?\s*|status(?:\s*code)?:?\s*|http\s*)402\b|out of funds|quota|balance")
    })
}

// Mirrors third tuple (126-132):
// `r"(?:error code:?\s*|status(?:\s*code)?:?\s*|http\s*)429\b|rate.?limit"` IGNORECASE
fn rate_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| build(r"(?:error code:?\s*|status(?:\s*code)?:?\s*|http\s*)429\b|rate.?limit"))
}

// Mirrors fourth tuple (133-140):
// `r"(?:error code:?\s*|status(?:\s*code)?:?\s*|http\s*)5\d{2}\b|server error|overloaded"` IGNORECASE
fn server_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        build(r"(?:error code:?\s*|status(?:\s*code)?:?\s*|http\s*)5\d{2}\b|server error|overloaded")
    })
}

// Mirrors fifth tuple (141-144):
// `r"context length|context_overflow|maximum context"` IGNORECASE
fn context_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| build(r"context length|context_overflow|maximum context"))
}

// Mirrors sixth tuple (145-151):
// `r"no llm provider configured|missing config|no access token"` IGNORECASE
fn config_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| build(r"no llm provider configured|missing config|no access token"))
}

// Mirrors seventh tuple (152-155):
// `r"model .*(not found|does not exist)|model_not_found"` IGNORECASE
fn model_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| build(r"model .*(not found|does not exist)|model_not_found"))
}

// ---------------------------------------------------------------------------
// classify_agent_error — mirrors lines 159-172
// ---------------------------------------------------------------------------

/// Mirrors `def classify_agent_error(text: str) -> str:` (159-172).
///
/// Map raw agent/provider error text to a closed reason code.
/// First matching rule in `_RULES` wins; anything unmatched (or empty) is `unknown`.
/// Auth intentionally outranks quota: a 401 body that also mentions "out of funds" is still auth.
pub fn classify_agent_error(text: &str) -> &'static str {
    // Mirrors `raw = str(text or "")` + `if not raw.strip(): return UNKNOWN` (166-168)
    if text.trim().is_empty() {
        return UNKNOWN;
    }
    if auth_re().is_match(text) {
        return PROVIDER_AUTH_OR_ACCESS;
    }
    if quota_re().is_match(text) {
        return PROVIDER_QUOTA_LIMIT;
    }
    if rate_re().is_match(text) {
        return PROVIDER_RATE_LIMIT;
    }
    if server_re().is_match(text) {
        return PROVIDER_SERVER_ERROR;
    }
    if context_re().is_match(text) {
        return CONTEXT_OVERFLOW;
    }
    if config_re().is_match(text) {
        return MISSING_CONFIG;
    }
    if model_re().is_match(text) {
        return MODEL_UNAVAILABLE;
    }
    UNKNOWN
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constants_match_python() {
        assert_eq!(RUNTIME_OFFLINE, "runtime_offline");
        assert_eq!(QUEUED_EXPIRED, "queued_expired");
        assert_eq!(DELIVERY_TIMEOUT, "delivery_timeout");
        assert_eq!(AGENT_BLOCKED, "agent_blocked");
        assert_eq!(CANCELLED, "cancelled");
        assert_eq!(PROVIDER_AUTH_OR_ACCESS, "provider_auth_or_access");
        assert_eq!(PROVIDER_QUOTA_LIMIT, "provider_quota_limit");
        assert_eq!(PROVIDER_RATE_LIMIT, "provider_rate_limit");
        assert_eq!(PROVIDER_SERVER_ERROR, "provider_server_error");
        assert_eq!(CONTEXT_OVERFLOW, "context_overflow");
        assert_eq!(MISSING_CONFIG, "missing_config");
        assert_eq!(MODEL_UNAVAILABLE, "model_unavailable");
        assert_eq!(UNKNOWN, "unknown");
    }

    #[test]
    fn all_reasons_contains_13() {
        assert_eq!(ALL_REASONS.len(), 13);
        for c in [
            RUNTIME_OFFLINE,
            QUEUED_EXPIRED,
            DELIVERY_TIMEOUT,
            AGENT_BLOCKED,
            CANCELLED,
            PROVIDER_AUTH_OR_ACCESS,
            PROVIDER_QUOTA_LIMIT,
            PROVIDER_RATE_LIMIT,
            PROVIDER_SERVER_ERROR,
            CONTEXT_OVERFLOW,
            MISSING_CONFIG,
            MODEL_UNAVAILABLE,
            UNKNOWN,
        ] {
            assert!(ALL_REASONS.contains(&c), "missing {c}");
        }
    }

    #[test]
    fn auto_retryable_contains_4() {
        assert_eq!(AUTO_RETRYABLE.len(), 4);
        assert!(AUTO_RETRYABLE.contains(&RUNTIME_OFFLINE));
        assert!(AUTO_RETRYABLE.contains(&DELIVERY_TIMEOUT));
        assert!(AUTO_RETRYABLE.contains(&PROVIDER_RATE_LIMIT));
        assert!(AUTO_RETRYABLE.contains(&PROVIDER_SERVER_ERROR));
        // negative
        assert!(!AUTO_RETRYABLE.contains(&QUEUED_EXPIRED));
        assert!(!AUTO_RETRYABLE.contains(&AGENT_BLOCKED));
        assert!(!AUTO_RETRYABLE.contains(&PROVIDER_AUTH_OR_ACCESS));
    }

    #[test]
    fn is_auto_retryable_mirrors_python() {
        assert!(is_auto_retryable(RUNTIME_OFFLINE));
        assert!(is_auto_retryable(DELIVERY_TIMEOUT));
        assert!(is_auto_retryable(PROVIDER_RATE_LIMIT));
        assert!(is_auto_retryable(PROVIDER_SERVER_ERROR));
        assert!(!is_auto_retryable(CANCELLED));
        assert!(!is_auto_retryable(CONTEXT_OVERFLOW));
        assert!(!is_auto_retryable(UNKNOWN));
        assert!(!is_auto_retryable("bogus"));
    }

    #[test]
    fn retry_action_mirrors_python() {
        assert_eq!(retry_action(RUNTIME_OFFLINE), RETRY_RESUME);
        assert_eq!(retry_action(DELIVERY_TIMEOUT), RETRY_RESUME);
        assert_eq!(retry_action(PROVIDER_RATE_LIMIT), RETRY_RESUME);
        assert_eq!(retry_action(PROVIDER_SERVER_ERROR), RETRY_RESUME);
        assert_eq!(retry_action(CONTEXT_OVERFLOW), RETRY_COMPRESS_THEN_RESUME);
        assert_eq!(retry_action(PROVIDER_AUTH_OR_ACCESS), RETRY_NONE);
        assert_eq!(retry_action(PROVIDER_QUOTA_LIMIT), RETRY_NONE);
        assert_eq!(retry_action(MISSING_CONFIG), RETRY_NONE);
        assert_eq!(retry_action(MODEL_UNAVAILABLE), RETRY_NONE);
        assert_eq!(retry_action(UNKNOWN), RETRY_NONE);
        assert_eq!(retry_action("not_a_reason"), RETRY_NONE);
        assert_eq!(RETRY_RESUME, "resume");
        assert_eq!(RETRY_COMPRESS_THEN_RESUME, "compress_then_resume");
        assert_eq!(RETRY_NONE, "none");
    }

    #[test]
    fn classify_empty_is_unknown() {
        assert_eq!(classify_agent_error(""), UNKNOWN);
        assert_eq!(classify_agent_error("   "), UNKNOWN);
        assert_eq!(classify_agent_error("\t\n"), UNKNOWN);
    }

    #[test]
    fn classify_auth_precedence_over_quota() {
        // Auth beats quota by design — 401 body that also mentions quota words is auth
        assert_eq!(
            classify_agent_error("authentication_error: invalid, blocked or out of funds"),
            PROVIDER_AUTH_OR_ACCESS
        );
        assert_eq!(
            classify_agent_error("Error code: 401 out of funds quota exceeded"),
            PROVIDER_AUTH_OR_ACCESS
        );
        // Plain auth variants
        assert_eq!(classify_agent_error("authentication_error"), PROVIDER_AUTH_OR_ACCESS);
        assert_eq!(classify_agent_error("Invalid API Key provided"), PROVIDER_AUTH_OR_ACCESS);
        assert_eq!(classify_agent_error("error code: 401 Unauthorized"), PROVIDER_AUTH_OR_ACCESS);
        assert_eq!(classify_agent_error("status code: 403 forbidden"), PROVIDER_AUTH_OR_ACCESS);
        assert_eq!(classify_agent_error("http 401"), PROVIDER_AUTH_OR_ACCESS);
        assert_eq!(classify_agent_error("HTTP 403"), PROVIDER_AUTH_OR_ACCESS);
        // case insensitive
        assert_eq!(classify_agent_error("AUTHENTICATION_ERROR"), PROVIDER_AUTH_OR_ACCESS);
        assert_eq!(classify_agent_error("INVALID API KEY"), PROVIDER_AUTH_OR_ACCESS);
    }

    #[test]
    fn classify_quota() {
        assert_eq!(classify_agent_error("Error code: 402 Payment Required"), PROVIDER_QUOTA_LIMIT);
        assert_eq!(classify_agent_error("status: 402"), PROVIDER_QUOTA_LIMIT);
        assert_eq!(classify_agent_error("http 402"), PROVIDER_QUOTA_LIMIT);
        assert_eq!(classify_agent_error("out of funds"), PROVIDER_QUOTA_LIMIT);
        assert_eq!(classify_agent_error("quota exceeded"), PROVIDER_QUOTA_LIMIT);
        assert_eq!(classify_agent_error("balance insufficient"), PROVIDER_QUOTA_LIMIT);
        // case insensitive
        assert_eq!(classify_agent_error("QUOTA"), PROVIDER_QUOTA_LIMIT);
        assert_eq!(classify_agent_error("OUT OF FUNDS"), PROVIDER_QUOTA_LIMIT);
    }

    #[test]
    fn classify_rate() {
        assert_eq!(classify_agent_error("error code: 429 Too Many Requests"), PROVIDER_RATE_LIMIT);
        assert_eq!(classify_agent_error("http 429"), PROVIDER_RATE_LIMIT);
        assert_eq!(classify_agent_error("rate limit exceeded"), PROVIDER_RATE_LIMIT);
        assert_eq!(classify_agent_error("rate_limit hit"), PROVIDER_RATE_LIMIT);
        assert_eq!(classify_agent_error("Rate Limit"), PROVIDER_RATE_LIMIT);
    }

    #[test]
    fn classify_server() {
        assert_eq!(classify_agent_error("error code: 500 Internal Server Error"), PROVIDER_SERVER_ERROR);
        assert_eq!(classify_agent_error("status code: 503"), PROVIDER_SERVER_ERROR);
        assert_eq!(classify_agent_error("http 502 bad gateway"), PROVIDER_SERVER_ERROR);
        assert_eq!(classify_agent_error("server error occurred"), PROVIDER_SERVER_ERROR);
        assert_eq!(classify_agent_error("model overloaded"), PROVIDER_SERVER_ERROR);
        // 5xx range via 5\d{2}
        assert_eq!(classify_agent_error("http 599"), PROVIDER_SERVER_ERROR);
        assert_eq!(classify_agent_error("OVERLOADED"), PROVIDER_SERVER_ERROR);
    }

    #[test]
    fn classify_context() {
        assert_eq!(classify_agent_error("context length exceeded"), CONTEXT_OVERFLOW);
        assert_eq!(classify_agent_error("context_overflow"), CONTEXT_OVERFLOW);
        assert_eq!(classify_agent_error("maximum context length 128k"), CONTEXT_OVERFLOW);
        assert_eq!(classify_agent_error("CONTEXT LENGTH"), CONTEXT_OVERFLOW);
        assert_eq!(classify_agent_error("Maximum Context"), CONTEXT_OVERFLOW);
    }

    #[test]
    fn classify_config() {
        assert_eq!(
            classify_agent_error("No LLM provider configured"),
            MISSING_CONFIG
        );
        assert_eq!(classify_agent_error("missing config file"), MISSING_CONFIG);
        assert_eq!(classify_agent_error("No access token provided"), MISSING_CONFIG);
        assert_eq!(classify_agent_error("NO ACCESS TOKEN"), MISSING_CONFIG);
    }

    #[test]
    fn classify_model() {
        assert_eq!(classify_agent_error("model foo not found"), MODEL_UNAVAILABLE);
        assert_eq!(classify_agent_error("model bar does not exist"), MODEL_UNAVAILABLE);
        assert_eq!(classify_agent_error("model_not_found"), MODEL_UNAVAILABLE);
        assert_eq!(classify_agent_error("Model X Does Not Exist"), MODEL_UNAVAILABLE);
        // must have "model" prefix
        assert_eq!(classify_agent_error("not found"), UNKNOWN);
        assert_eq!(classify_agent_error("does not exist"), UNKNOWN);
    }

    #[test]
    fn classify_unknown_fallback() {
        assert_eq!(classify_agent_error("some random error"), UNKNOWN);
        assert_eq!(classify_agent_error("timeout connecting to provider"), UNKNOWN);
        assert_eq!(classify_agent_error("unexpected failure"), UNKNOWN);
    }

    #[test]
    fn classify_order_matters_auth_wins() {
        // quota words inside auth error must not misclassify
        let text = "authentication_error: out of funds due to 401 and quota balance rate limit";
        assert_eq!(classify_agent_error(text), PROVIDER_AUTH_OR_ACCESS);
        // rate words inside quota error → quota wins because quota is before rate
        assert_eq!(classify_agent_error("402 quota rate limit"), PROVIDER_QUOTA_LIMIT);
        // server words inside rate → rate wins
        assert_eq!(classify_agent_error("429 rate limit server error"), PROVIDER_RATE_LIMIT);
    }
}
