use super::*;

static AUTO_COMPACT_ENABLED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(true);

fn set_auto_compact_enabled(on: bool) {
    AUTO_COMPACT_ENABLED.store(on, std::sync::atomic::Ordering::Relaxed);
}

pub fn is_auto_compact_enabled() -> bool {
    AUTO_COMPACT_ENABLED.load(std::sync::atomic::Ordering::Relaxed)
}

pub fn init_auto_compact_from_env() {
    let disabled = std::env::var("GRAY_NO_AUTO_COMPACT")
        .map(|s| {
            !matches!(
                s.trim().to_ascii_lowercase().as_str(),
                "" | "0" | "false" | "no" | "off"
            )
        })
        .unwrap_or(false);
    set_auto_compact_enabled(!disabled);
}

pub fn compact_reason_note(reason: CompactReason) -> String {
    match reason {
        CompactReason::Window => String::new(),
        CompactReason::Overflow => " · overflow".to_string(),
        CompactReason::UserCeiling => " · compact-at".to_string(),
        CompactReason::PriceCliff { cliff } => {
            format!(
                " · price cliff {}",
                crate::setup::format_context_length(cliff)
            )
        }
    }
}

pub fn ceiling_for(
    compact_at: Option<crate::setup::CompactAt>,
    window: usize,
    reserve: usize,
    price_cliff: Option<usize>,
) -> (Option<usize>, Option<usize>) {
    match compact_at {
        Some(crate::setup::CompactAt::Tokens(n)) => (
            Some(n.min(window.saturating_sub(reserve))).filter(|&c| c > 0),
            None,
        ),
        _ => {
            let cliff = price_cliff.and_then(|c| gray_core::price_cliff_ceiling(c, reserve));
            let cap =
                (window.saturating_sub(reserve) > DEFAULT_COMPACT_AT).then_some(DEFAULT_COMPACT_AT);
            (cliff.into_iter().chain(cap).min(), price_cliff)
        }
    }
}

/// Where compaction starts on a large-window model when nothing else is
/// set. Past this, a 1M window costs more per turn and recalls worse, so
/// waiting for the window to fill helps nobody. `compact_at_tokens` or
/// `--compact-at` overrides it.
pub const DEFAULT_COMPACT_AT: usize = 200_000;

pub fn effective_compact_ceiling(config: &crate::Config) -> usize {
    let model = config.model.as_deref().unwrap_or("");
    let window = crate::setup::resolve_model_context_length(model);
    let reserve = crate::setup::user_reserve_tokens_for(window);
    ceiling_for(
        config.compact_at,
        window,
        reserve,
        crate::setup::model_price_cliff(model).map(|c| c as usize),
    )
    .0
    .unwrap_or_else(|| window.saturating_sub(reserve))
}

pub fn estimate_tokens(msg: &Message) -> usize {
    gray_core::agent::estimate_message_tokens(msg)
}

pub fn estimate_context_tokens(messages: &[Message], last: Option<Usage>) -> usize {
    if let Some(u) = last
        && u.total() > 0
    {
        return u.total();
    }
    messages.iter().map(estimate_tokens).sum()
}

pub fn is_context_overflow_error(err: &CoreError) -> bool {
    match err.class() {
        CoreError::ContextOverflow(_) => true,
        CoreError::Provider(msg) => {
            let lower = msg.to_lowercase();
            lower.contains("context_length")
                || lower.contains("context window")
                || lower.contains("context length")
                || gray_core::overflow::is_context_overflow_message(msg)
        }
        _ => false,
    }
}

#[cfg(test)]
mod ceiling_tests {
    use super::*;
    use crate::setup::CompactAt;

    #[test]
    fn large_windows_default_to_200k() {
        assert_eq!(ceiling_for(None, 1_000_000, 61_000, None).0, Some(200_000));
        assert_eq!(ceiling_for(None, 200_000, 20_000, None).0, None);
        assert_eq!(
            ceiling_for(None, 1_000_000, 61_000, Some(128_000))
                .0
                .map(|c| c < 200_000),
            Some(true)
        );
        let full = Some(CompactAt::Tokens(1_000_000));
        assert_eq!(ceiling_for(full, 1_000_000, 61_000, None).0, Some(939_000));
    }
}
