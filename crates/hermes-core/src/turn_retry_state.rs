//! Per-attempt recovery bookkeeping for the conversation turn loop.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/agent/turn_retry_state.py` (93 lines).
//!
//! The inner retry loop in `run_conversation` (`while retry_count <
//! max_retries`) makes several distinct recovery attempts on a single model API
//! call: a credential-pool 429 retry, a per-provider OAuth refresh (codex,
//! anthropic, nous, copilot), a long-context compression restart, a length-
//! continuation restart, and a handful of format-recovery branches (thinking-
//! signature stripping, multimodal-tool-content stripping, llama.cpp grammar
//! fallback, image shrink, invalid-encrypted-content, 1M-beta header).
//!
//! Each of those branches is guarded by a one-shot boolean so it fires at most
//! once per attempt. They used to be ~16 bare `*_attempted` / `has_retried_*`
//! / `restart_with_*` locals declared inline before the loop and threaded
//! through its 2,400-line body. `TurnRetryState` collapses them into one object
//! the loop mutates in place (`state.codex_auth_retry_attempted = True`), giving
//! the recovery bookkeeping a single named, testable home.
//!
//! Loop-control variables (`retry_count`, `max_retries`,
//! `max_compression_attempts`) intentionally stay as plain locals — they are the
//! `while` mechanics, not recovery bookkeeping, and putting them on the object
//! would add indirection without clarifying anything.
//!
//! This module is dependency-free so it can be unit-tested in isolation and
//! imported by the turn loop without an import cycle.

// ---------------------------------------------------------------------------
// TurnRetryState — mirrors `@dataclass class TurnRetryState` (lines 32-93)
// ---------------------------------------------------------------------------

/// One-shot recovery guards + restart signals for a single API-call attempt.
///
/// A fresh instance is created for each iteration of the outer turn loop
/// (once per `api_call_count`). Each guard fires its recovery branch at most
/// once; the `restart_with_*` signals are read by the loop after the attempt
/// to decide whether to rebuild the request and retry.
///
/// Mirrors `TurnRetryState` dataclass (lines 32-93).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TurnRetryState {
    // ── Per-provider OAuth / credential refresh guards ───────────────────
    // Mirrors lines 42-56
    pub codex_auth_retry_attempted: bool,
    pub anthropic_auth_retry_attempted: bool,
    pub nous_auth_retry_attempted: bool,
    pub nous_paid_entitlement_refresh_attempted: bool,
    pub copilot_auth_retry_attempted: bool,
    /// Copilot surfaces a stale/degraded credential as a 400
    /// `model_not_available_for_integrator` / `model_not_supported` instead
    /// of a clean 401 (e.g. a raw OAuth token seeded when the token exchange
    /// degraded at startup, routing the request to the restricted
    /// `copilot-language-server` integrator). Guard a single-shot forced
    /// re-exchange + client rebuild for that case, separate from the 401 guard
    /// so both can fire within one attempt if needed.
    /// Mirrors `copilot_stale_cred_retry_attempted` (lines 48-55).
    pub copilot_stale_cred_retry_attempted: bool,
    pub vertex_auth_retry_attempted: bool,

    // ── Format / payload recovery guards ─────────────────────────────────
    // Mirrors lines 58-65
    pub thinking_sig_retry_attempted: bool,
    pub invalid_encrypted_content_retry_attempted: bool,
    pub native_compaction_reject_retry_attempted: bool,
    pub image_shrink_retry_attempted: bool,
    pub multimodal_tool_content_retry_attempted: bool,
    pub oauth_1m_beta_retry_attempted: bool,
    pub llama_cpp_grammar_retry_attempted: bool,

    // ── Transport / rate-limit recovery ──────────────────────────────────
    // Mirrors lines 67-69
    pub primary_recovery_attempted: bool,
    pub has_retried_429: bool,

    // ── Auth-failure provider failover ───────────────────────────────────
    // Set once we've escalated a persistent 401/403 (after the per-provider
    // credential-refresh attempt above failed) to the fallback chain, so we
    // don't loop on the same auth failover within one attempt.
    // Mirrors `auth_failover_attempted` (lines 71-75).
    pub auth_failover_attempted: bool,

    // ── Restart signals (read by the outer loop after the attempt) ───────
    // Mirrors lines 77-88
    pub restart_with_compressed_messages: bool,
    pub restart_with_length_continuation: bool,
    /// Set when a content-filter stream stall (e.g. MiniMax "new_sensitive")
    /// has been escalated to the fallback chain: the partial-stream content
    /// was rolled back off `messages` and the loop should re-issue the API
    /// call against the newly-activated provider (#32421).
    /// Mirrors `restart_with_rebuilt_messages` (lines 80-84).
    pub restart_with_rebuilt_messages: bool,
    /// A user correction cancelled the in-flight provider request. The outer
    /// loop must append a role-safe checkpoint + user message, rebuild the API
    /// payload, and retry the same logical iteration.
    /// Mirrors `restart_with_redirected_messages` (lines 85-88).
    pub restart_with_redirected_messages: bool,
}

impl TurnRetryState {
    /// Create a new instance with all guards clear.
    /// Mirrors `TurnRetryState()` default construction (all `False`).
    pub fn new() -> Self {
        Self::default()
    }

    /// Iterate over `(field_name, value)` pairs in dataclass declaration order.
    ///
    /// Mirrors `__iter__` (lines 90-93):
    /// ```python
    /// def __iter__(self):
    ///     for f in fields(self):
    ///         yield f.name, getattr(self, f.name)
    /// ```
    /// Convenience for debugging / tests.
    pub fn iter(&self) -> std::array::IntoIter<(&'static str, bool), 21> {
        [
            (
                "codex_auth_retry_attempted",
                self.codex_auth_retry_attempted,
            ),
            (
                "anthropic_auth_retry_attempted",
                self.anthropic_auth_retry_attempted,
            ),
            ("nous_auth_retry_attempted", self.nous_auth_retry_attempted),
            (
                "nous_paid_entitlement_refresh_attempted",
                self.nous_paid_entitlement_refresh_attempted,
            ),
            (
                "copilot_auth_retry_attempted",
                self.copilot_auth_retry_attempted,
            ),
            (
                "copilot_stale_cred_retry_attempted",
                self.copilot_stale_cred_retry_attempted,
            ),
            (
                "vertex_auth_retry_attempted",
                self.vertex_auth_retry_attempted,
            ),
            (
                "thinking_sig_retry_attempted",
                self.thinking_sig_retry_attempted,
            ),
            (
                "invalid_encrypted_content_retry_attempted",
                self.invalid_encrypted_content_retry_attempted,
            ),
            (
                "native_compaction_reject_retry_attempted",
                self.native_compaction_reject_retry_attempted,
            ),
            (
                "image_shrink_retry_attempted",
                self.image_shrink_retry_attempted,
            ),
            (
                "multimodal_tool_content_retry_attempted",
                self.multimodal_tool_content_retry_attempted,
            ),
            (
                "oauth_1m_beta_retry_attempted",
                self.oauth_1m_beta_retry_attempted,
            ),
            (
                "llama_cpp_grammar_retry_attempted",
                self.llama_cpp_grammar_retry_attempted,
            ),
            (
                "primary_recovery_attempted",
                self.primary_recovery_attempted,
            ),
            ("has_retried_429", self.has_retried_429),
            ("auth_failover_attempted", self.auth_failover_attempted),
            (
                "restart_with_compressed_messages",
                self.restart_with_compressed_messages,
            ),
            (
                "restart_with_length_continuation",
                self.restart_with_length_continuation,
            ),
            (
                "restart_with_rebuilt_messages",
                self.restart_with_rebuilt_messages,
            ),
            (
                "restart_with_redirected_messages",
                self.restart_with_redirected_messages,
            ),
        ]
        .into_iter()
    }
}

impl<'a> IntoIterator for &'a TurnRetryState {
    type Item = (&'static str, bool);
    type IntoIter = std::array::IntoIter<(&'static str, bool), 21>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}
