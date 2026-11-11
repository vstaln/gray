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
