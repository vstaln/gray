//! Compaction policy: settings, token estimates, overflow detection (split from `compact`).

use super::*;

pub struct CompactionSettings {
    pub enabled: bool,
    pub reserve_tokens: usize,
    pub keep_recent_tokens: usize,
}

/// Master switch for *automatic* compaction only (`should_compact` callers and
/// `auto_compact_if_needed`). Manual entry points (`compact_with_keep`,
/// `compact_with_instructions`) always run, so an `enabled=false` session keeps
/// its manual escape hatch.
static AUTO_COMPACT_ENABLED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(true);

/// Disables/enables automatic compaction for this session. Manual `/compact` ignores this.
pub fn set_auto_compact_enabled(on: bool) {
    AUTO_COMPACT_ENABLED.store(on, std::sync::atomic::Ordering::Relaxed);
}

pub fn is_auto_compact_enabled() -> bool {
    AUTO_COMPACT_ENABLED.load(std::sync::atomic::Ordering::Relaxed)
}

/// Env kill-switch for automatic compaction, following the `GRAY_*` pattern in
/// `config.rs`: `GRAY_NO_AUTO_COMPACT=1` (also `true`/`yes`/`on`) disables it.
/// `0`/`false`/`no`/`off`/unset leave it enabled. Manual `/compact` still runs.
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

pub const DEFAULT_COMPACTION_SETTINGS: CompactionSettings = CompactionSettings {
    enabled: true,
    reserve_tokens: 16384,
    keep_recent_tokens: 20000,
};

pub fn should_compact(tokens: usize, window: usize, s: &CompactionSettings) -> bool {
    s.enabled && tokens > window.saturating_sub(s.reserve_tokens)
}

/// Effective compaction settings from user overrides (or proportional defaults).
/// Window-aware: reserve ≈ window/16, keep ≈ window/13 when no override.
pub fn compaction_settings() -> CompactionSettings {
    CompactionSettings {
        enabled: true,
        reserve_tokens: crate::setup::user_reserve_tokens(),
        keep_recent_tokens: crate::setup::user_keep_recent_tokens(),
    }
}

/// Window-aware variant — prefer this where the model window is known.
pub fn compaction_settings_for(window: usize) -> CompactionSettings {
    CompactionSettings {
        enabled: true,
        reserve_tokens: crate::setup::user_reserve_tokens_for(window),
        keep_recent_tokens: crate::setup::user_keep_for(window),
    }
}

/// Recent tail of `messages` whose estimated tokens fit in `keep_tokens`.
/// Walks from the newest message backwards; `0` keeps nothing.
pub fn tail_messages(messages: &[Message], keep_tokens: usize) -> Vec<Message> {
    if keep_tokens == 0 {
        return Vec::new();
    }
    let mut kept = Vec::new();
    let mut acc = 0usize;
    for msg in messages.iter().rev() {
        let t = estimate_tokens(msg);
        if kept.is_empty() && acc == 0 {
            // Always keep at least the newest message when budget > 0,
            // even if that single message exceeds the budget.
            kept.push(msg.clone());
            acc += t;
            continue;
        }
        if acc + t > keep_tokens {
            break;
        }
        kept.push(msg.clone());
        acc += t;
    }
    kept.reverse();
    kept
}

pub fn calculate_context_tokens(u: &Usage) -> usize {
    if u.total() > 0 {
        u.total()
    } else {
        u.input_tokens + u.output_tokens
    }
}

pub fn estimate_tokens(msg: &Message) -> usize {
    // Must measure billable context, not displayable prose: a message whose
    // only block is a 50 KiB tool result is ~12.8k tokens, not 0. See
    // `Message::context_text`.
    (msg.context_text().len() as f64 / 4.0).ceil() as usize
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
    if let CoreError::Provider(msg) = err {
        let lower = msg.to_lowercase();
        lower.contains("context_length")
            || lower.contains("context window")
            || lower.contains("context length")
            || lower.contains("max_tokens")
    } else {
        false
    }
}
