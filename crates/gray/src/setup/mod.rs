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

pub(crate) fn open_modal() -> std::io::Result<(
    TuiSession,
    ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
)> {
    let session = TuiSession::acquire()?;
    let mut stdout = std::io::stdout();
    crossterm::execute!(
        stdout,
        crossterm::terminal::EnterAlternateScreen,
        crossterm::terminal::Clear(crossterm::terminal::ClearType::All),
        crossterm::cursor::Hide
    )?;
    let terminal = ratatui::Terminal::new(ratatui::backend::CrosstermBackend::new(stdout))?;
    drain_queued_events();
    Ok((session, terminal))
}

fn drain_queued_events() {
    use crossterm::event::{poll, read};
    use std::time::Duration;

    for _ in 0..256 {
        match poll(Duration::ZERO) {
            Ok(true) => {
                if read().is_err() {
                    break;
                }
            }
            _ => break,
        }
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
pub(crate) use catalog::effort_memory_key;
pub(crate) use catalog::save_auth_key;
pub use catalog::{
    AUTH_MODE_API_KEY, AUTH_MODE_NONE, Catalog, CatalogProvider, ConnectAuth, ConnectItem,
    PROVIDERS_JSON, SavedConfig, auto_update_enabled, auto_update_enabled_at, build_connect_items,
    disabled_skill_names, gray_home, gw_auto_enabled, gw_auto_enabled_at, intro_enabled,
    load_auth_keys, load_catalog, load_saved_config_at, lock_saved_config_at, mask_key_pretty,
    normalize_custom_base_url, quality_of_life_enabled, quality_of_life_enabled_at,
    save_saved_config_at, saved_config_path, skills_auto_enabled, skills_auto_enabled_at,
};

pub fn user_home() -> anyhow::Result<std::path::PathBuf> {
    gray_core::paths::user_home()
        .ok_or_else(|| anyhow::anyhow!("cannot resolve the user home; set HOME"))
}

pub mod context;
pub use context::{
    CompactAt, ContextParts, DEFAULT_KEEP_RECENT_TOKENS, ModelRate, cache_model_context,
    cache_model_context_if_absent, cache_model_reasoning, cache_models_dev_if_absent,
    cached_model_ids, clamp_thinking_level, context_source, default_keep_for_window,
    default_reserve_for_window, estimate_str_tokens, extract_context_length_from_json,
    fetch_litellm_context_windows, fetch_live_provider_models, fetch_models_dev_context,
    fetch_openrouter_rates, fetch_recommended_models, format_context_length, format_cost,
    friendly_model_name, get_cached_model_context, get_model_rate, get_user_context_window,
    load_models_cache_to_memory, model_context_info, model_max_context, model_price_cliff,
    model_supports_reasoning, parse_compact_at, parse_context_window, parse_litellm_context_json,
    parse_models_dev_json, parse_openrouter_models_json, resolve_model_context_length,
    save_models_cache_to_disk, set_active_model_provider, set_user_context_window,
    set_user_keep_recent_tokens, set_user_reserve_tokens, supported_efforts,
    supported_thinking_levels, turn_cost, uncached_cost, user_keep_for, user_keep_recent_tokens,
    user_reserve_tokens_for,
};

pub mod ui;
pub use ui::{BackgroundSnapshot, dim_color, render_dimmed_background};
pub mod icons;
pub use icons::icon;
pub mod tabs;

pub(crate) mod composite_rows;
mod context_modal;
mod install_manager;
pub(crate) mod marketplace;
mod model_modal;
mod model_panes;
mod plugins_modal;
mod theme_modal;
pub(crate) mod variants;

pub use context_modal::run_context_modal;
mod connect;
mod connect_draw;
mod provider_auth;
mod recommend;

pub use connect::{ConnectOutcome, active_connect_id, run_connect_modal, run_connect_modal_for};
pub(crate) use install_manager::{
    ManagerItem, ManagerSpec, format_plugin_row_parts, run_install_manager,
};
pub use install_manager::{
    ManagerOutcome, PluginModalOutcome, run_plugins_modal, run_skills_modal,
};
pub(crate) use model_modal::{
    canonicalize_adopted_model, composite_label_for, composite_label_parts_for,
    decompose_model_variant, effort_chip, picker_scope, provider_models_for_config,
    run_model_modal, saved_models_for, selected_model_label, selection_parts,
    validate_direct_model_id, wire_model_for,
};
pub(crate) use model_panes::{PanesOutcome, run_model_panes};
pub(crate) use plugins_modal::run_plugins_manager;
pub use provider_auth::{
    PluginLoginProgress, activate_plugin_connection, adopt_saved_key, connect_saved_key,
    forget_plugin_connection, run_plugin_login, select_api_key_connection,
};
pub(crate) use provider_auth::{adopt_connection_effort, apply_connection_effort};
pub(crate) use theme_modal::run_theme_picker;

use crate::{config::Config, tui::print_wrapped};

pub const THINKING_LEVELS: &[(&str, &str)] = &[
    ("off", "No reasoning"),
    ("minimal", "Very brief reasoning"),
    ("low", "Light reasoning"),
    ("medium", "Moderate reasoning"),
    ("high", "Deep reasoning"),
    ("xhigh", "Very deep reasoning"),
    ("max", "Maximum reasoning"),
];

pub async fn run_model_menu(
    config: &mut Config,
    bg: Option<&BackgroundSnapshot>,
    focus: Option<&str>,
) -> anyhow::Result<bool> {
    run_model_modal(config, bg, focus)
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
    let detected = crate::quality_of_life::detect::first_run_detected();
    if !detected.is_empty() {
        let names = detected
            .iter()
            .map(|d| d.name)
            .collect::<Vec<_>>()
            .join(", ");
        print_wrapped(&format!("\x1b[2m{names} detected\x1b[0m"), 2);
        print_wrapped(
            "\x1b[2m[i] import setup via /setup · any other key skips\x1b[0m",
            2,
        );
        use crossterm::event::{Event, KeyCode, KeyEventKind, read};
        let _ = crossterm::terminal::enable_raw_mode();
        drain_queued_events();
        let key = loop {
            match read() {
                Ok(Event::Key(k)) if k.kind == KeyEventKind::Press => break Some(k),
                Ok(_) => continue,
                Err(_) => break None,
            }
        };
        let _ = crossterm::terminal::disable_raw_mode();
        print!("\r\n");
        if matches!(key, Some(k) if matches!(k.code, KeyCode::Char('i') | KeyCode::Char('I'))) {
            crate::quality_of_life::detect::set_pending_setup(true);
        }
    }

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
        print_wrapped(&format!("using {model} — /models to change"), 2);
        print!("\r\n");
        return Ok(true);
    }

    match run_connect_modal(config, None)? {
        ConnectOutcome::Connected => Ok(true),
        ConnectOutcome::Removed(_) | ConnectOutcome::Dismissed => Ok(false),
    }
}
