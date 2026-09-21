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
    AUTH_MODE_API_KEY, AUTH_MODE_NONE, Catalog, CatalogProvider, ConnectItem, PROVIDERS_JSON,
    SavedConfig, build_connect_items, cron_auto_enabled, cron_auto_enabled_at,
    disabled_skill_names, gray_home, gw_auto_enabled, gw_auto_enabled_at, load_auth_keys,
    load_catalog, load_saved_config_at, mask_key_pretty, memory_auto_enabled,
    memory_auto_enabled_at, normalize_custom_base_url, save_saved_config_at, saved_config_path,
    skills_auto_enabled, skills_auto_enabled_at,
};

pub mod context;
pub use context::{
    ContextParts, DEFAULT_KEEP_RECENT_TOKENS, ModelRate, cache_model_context,
    cache_model_context_if_absent, cache_model_reasoning, cache_models_dev_if_absent,
    cached_model_ids, clamp_thinking_level, context_source, default_keep_for_window,
    default_reserve_for_window, estimate_str_tokens, extract_context_length_from_json,
    fetch_litellm_context_windows, fetch_live_provider_models, fetch_models_dev_context,
    fetch_openrouter_rates, format_context_length, format_cost, friendly_model_name,
    get_cached_model_context, get_model_rate, get_user_context_window, load_models_cache_to_memory,
    model_context_info, model_max_context, model_supports_reasoning, parse_context_window,
    parse_litellm_context_json, parse_models_dev_json, parse_openrouter_models_json,
    resolve_model_context_length, save_models_cache_to_disk, set_active_model_provider,
    set_user_context_window, set_user_keep_recent_tokens, set_user_reserve_tokens,
    supported_efforts, supported_thinking_levels, turn_cost, user_keep_for,
    user_keep_recent_tokens, user_reserve_tokens_for,
};

pub mod ui;
pub use ui::{BackgroundSnapshot, dim_color, render_dimmed_background};
pub mod icons;
pub use icons::icon;
pub mod tabs;

mod context_modal;
mod effort;
mod install_manager;
mod model_modal;

pub use context_modal::run_context_modal;
mod connect;
mod connect_draw;
mod connect_models;

pub use connect::{ConnectOutcome, run_connect_modal};
pub use effort::run_effort_modal;
pub(crate) use install_manager::{
    ManagerItem, ManagerSpec, format_plugin_row_parts, run_install_manager,
};
pub use install_manager::{run_plugins_modal, run_skills_modal};
pub(crate) use model_modal::{provider_models_for, run_model_modal, validate_direct_model_id};

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
    // Honest by default: an account exists (`/login`), but nothing in gray
    // is gated on it, so say that instead of implying one is required.
    print_wrapped(
        "\x1b[2mNo account needed. `/login` connects this machine to gray.alignment.id if you want one — optional, and it unlocks nothing yet.\x1b[0m",
        2,
    );
    print!("\r\n");
    // Returning users with a saved model + key skip the picker entirely:
    // the modal only earns its interruption on first run.
    let model_ok = config
        .model
        .as_deref()
        .is_some_and(|m| !m.trim().is_empty());
    let key_ok = config
        .api_key
        .as_deref()
        .is_some_and(|k| !k.trim().is_empty());
    if model_ok && key_ok {
        let model = config.model.as_deref().unwrap_or("default");
        print_wrapped(&format!("using {model} — /connect to change"), 2);
        print!("\r\n");
        return Ok(true);
    }
    // Onboarding only cares "is a provider ready": a removal leaves the
    // user exactly as unconfigured as a dismissal.
    match run_connect_modal(config, None)? {
        ConnectOutcome::Connected => Ok(true),
        ConnectOutcome::Removed(_) | ConnectOutcome::Dismissed => Ok(false),
    }
}
