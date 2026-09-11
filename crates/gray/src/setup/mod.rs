/// Owns the terminal modes an alternate-screen modal acquires. Create it
/// after raw mode is enabled and before the first fallible terminal write:
/// a `?` during setup then can never strand the user's terminal. Restoration
/// is best-effort by design — `Drop` cannot report errors, and the callers'
/// primary result must win (audit 24.03). `LeaveAlternateScreen` alone
/// restores the main buffer: never emit `ClearType::All`/blank-line floods
/// on the way out, they race the compositor's synchronized flush (ghost text).
pub(crate) struct TuiSession {
    was_raw: bool,
}

impl TuiSession {
    pub(crate) fn acquire() -> std::io::Result<Self> {
        let was_raw = crossterm::terminal::is_raw_mode_enabled().unwrap_or(false);
        if !was_raw {
            crossterm::terminal::enable_raw_mode()?;
        }
        Ok(Self { was_raw })
    }
}

impl Drop for TuiSession {
    fn drop(&mut self) {
        use std::io::Write as _;
        let _ = crossterm::execute!(
            std::io::stdout(),
            crossterm::terminal::LeaveAlternateScreen,
            crossterm::cursor::Show,
        );
        if self.was_raw {
            let _ = crossterm::terminal::enable_raw_mode();
        } else {
            let _ = crossterm::terminal::disable_raw_mode();
        }
        let _ = std::io::stdout().flush();
    }
}

pub mod catalog;
pub(crate) use catalog::save_auth_key;
pub use catalog::{
    AUTH_MODE_API_KEY, AUTH_MODE_NONE, ConnectItem, SavedConfig, build_connect_items, gray_home,
    load_auth_keys, load_saved_config_at, mask_key_pretty, popular_provider_name,
    save_saved_config_at, saved_config_path,
};

pub mod context;
pub use context::{
    ContextParts, DEFAULT_KEEP_RECENT_TOKENS, ModelRate, cache_model_context,
    cache_model_context_if_absent, cache_model_reasoning, cache_models_dev_if_absent,
    cached_model_ids, context_source, default_keep_for_window, default_reserve_for_window,
    estimate_str_tokens, extract_context_length_from_json, fetch_litellm_context_windows,
    fetch_live_provider_models, fetch_models_dev_context, fetch_openrouter_rates,
    format_context_length, format_cost, friendly_model_name, get_cached_model_context,
    get_model_rate, get_user_context_window, load_models_cache_to_memory, model_context_info,
    model_max_context, model_supports_reasoning, parse_context_window, parse_litellm_context_json,
    parse_models_dev_json, parse_openrouter_models_json, resolve_model_context_length,
    save_models_cache_to_disk, set_user_context_window, set_user_keep_recent_tokens,
    set_user_reserve_tokens, supported_efforts, supported_thinking_levels, turn_cost,
    user_keep_for, user_keep_recent_tokens, user_reserve_tokens_for,
};

pub mod ui;
pub use ui::{BackgroundSnapshot, render_dimmed_background};
pub mod icons;
pub use icons::icon;
pub mod tabs;

#[cfg(feature = "acp")]
mod acp_modal;
mod effort;
mod install_manager;
mod model_modal;
mod permissions_modal;

mod connect;
mod connect_draw;
mod connect_models;

#[cfg(feature = "acp")]
pub use acp_modal::run_acp_modal;
pub use connect::run_connect_modal;
pub use effort::run_effort_modal;
pub use install_manager::{run_plugins_modal, run_skills_modal};
pub(crate) use model_modal::{provider_models_for, run_model_modal, validate_direct_model_id};
pub use permissions_modal::run_permissions_modal;

use crate::{config::Config, tui::print_wrapped};

/// Thinking levels and descriptions matching Pi / Prime-Agent.
pub const THINKING_LEVELS: &[(&str, &str)] = &[
    ("off", "No reasoning"),
    ("minimal", "Very brief reasoning"),
    ("low", "Light reasoning"),
    ("medium", "Moderate reasoning"),
    ("high", "Deep reasoning"),
    ("xhigh", "Very deep reasoning"),
    ("max", "Maximum reasoning"),
];

pub async fn run_effort_menu(
    config: &mut Config,
    bg: Option<&BackgroundSnapshot>,
) -> anyhow::Result<bool> {
    run_effort_modal(config, bg)
}

pub async fn run_model_menu(
    config: &mut Config,
    bg: Option<&BackgroundSnapshot>,
) -> anyhow::Result<bool> {
    run_model_modal(config, bg)
}

pub async fn run_provider_menu(
    config: &mut Config,
    bg: Option<&BackgroundSnapshot>,
) -> anyhow::Result<bool> {
    run_connect_modal(config, bg)
}

pub async fn run_onboarding(config: &mut Config) -> anyhow::Result<bool> {
    let _ = crossterm::terminal::disable_raw_mode();
    crate::tui::clear_screen();
    print!("\r\n");
    crate::tui::print_logo();
    print!("\r\n");
    print_wrapped("\x1b[2mWelcome to gray by alignment\x1b[0m", 2);
    print_wrapped(
        "\x1b[2mgray is a minimal agent that runs tools, edits code, and works with any model provider.\x1b[0m",
        2,
    );
    print!("\r\n");
    run_provider_menu(config, None).await
}
