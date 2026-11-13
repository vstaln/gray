use gray_core::agent::Agent;
use gray_core::error::CoreError;
use gray_core::event::{CompactReason, Usage};
use gray_core::message::Message;

pub mod policy;

pub use policy::{
    ceiling_for, compact_reason_note, effective_compact_ceiling, estimate_context_tokens,
    estimate_tokens, init_auto_compact_from_env, is_auto_compact_enabled,
    is_context_overflow_error,
};

pub async fn auto_compact_if_needed(agent: &mut Agent) -> Result<bool, CoreError> {
    if !is_auto_compact_enabled() {
        return Ok(false);
    }
    let keep = crate::setup::user_keep_recent_tokens();
    Ok(compact_with_keep(agent, None, keep).await?.is_some())
}

/// Below this many message tokens, a cold resend is cheap enough that
/// summarizing before cache expiry isn't worth losing the detail.
const EXPIRY_MIN_TOKENS: usize = 40_000;

/// The verbatim tail to keep when compacting just before the prompt cache
/// expires: at most a quarter of the context, so the first message after
/// the idle stretch re-sends a small prompt even when `context_keep` is
/// large. `None` when the context is too small to bother.
fn expiry_keep(estimate: usize, user_keep: usize) -> Option<usize> {
    (estimate >= EXPIRY_MIN_TOKENS).then(|| user_keep.min(estimate / 4))
}

pub fn expiry_keep_for(agent: &Agent) -> Option<usize> {
    if !is_auto_compact_enabled() {
        return None;
    }
    let estimate: usize = agent.messages().iter().map(estimate_tokens).sum();
    let keep = expiry_keep(estimate, crate::setup::user_keep_recent_tokens());
    match keep {
        Some(k) => log::info!(target: "gray_agent", "expiry compaction: est={estimate} keep={k}"),
        None => log::info!(target: "gray_agent", "expiry compaction: skipped est={estimate}"),
    }
    keep
}

pub async fn compact_before_expiry(agent: &mut Agent) -> Result<bool, CoreError> {
    let Some(keep) = expiry_keep_for(agent) else {
        return Ok(false);
    };
    Ok(compact_with_keep(agent, None, keep).await?.is_some())
}

pub async fn compact_with_keep(
    agent: &mut Agent,
    custom_instructions: Option<&str>,
    keep_tokens: usize,
) -> Result<Option<String>, CoreError> {
    let replaced = agent.messages().len();
    if replaced == 0 {
        return Ok(None);
    }
    let Some(summary) = agent
        .compact_v2(custom_instructions, Some(keep_tokens))
        .await?
    else {
        return Ok(None);
    };

    Ok(Some(summary))
}

#[cfg(test)]
mod expiry_tests {
    use super::expiry_keep;

    #[test]
    fn expiry_keep_caps_tail_at_a_quarter() {
        assert_eq!(expiry_keep(100_000, 100_000), Some(25_000));
        assert_eq!(expiry_keep(100_000, 10_000), Some(10_000));
        assert_eq!(expiry_keep(30_000, 100_000), None);
    }
}
