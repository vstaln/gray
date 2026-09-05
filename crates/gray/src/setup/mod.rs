pub mod catalog;
pub(crate) use catalog::save_auth_key;
pub use catalog::{
    AUTH_MODE_API_KEY, AUTH_MODE_NONE, AUTH_MODE_OAUTH, Catalog, CatalogModel, CatalogProvider,
    ConnectItem, OAUTH_CAPABLE, PROVIDERS_JSON, SavedConfig, build_connect_items, gray_home,
    load_auth_keys, load_catalog, load_saved_config_at, mask_key_pretty, normalize_auth_mode,
    save_saved_config_at, saved_config_path,
};

pub mod context;
pub use context::{
    ContextParts, DEFAULT_KEEP_RECENT_TOKENS, DEFAULT_RESERVE_TOKENS, ModelRate,
    cache_model_context, cache_model_context_if_absent, cache_models_dev_if_absent, context_source,
    default_keep_for_window, default_reserve_for_window, estimate_str_tokens,
    extract_context_length_from_json, fetch_litellm_context_windows, fetch_live_provider_models,
    fetch_models_dev_context, fetch_openrouter_rates, format_context_length, format_cost,
    friendly_model_name, get_cached_model_context, get_model_rate, get_provider_models,
    get_provider_models_with_live, get_user_context_window, load_models_cache_to_memory,
    model_context_info, model_max_context, parse_context_window, parse_litellm_context_json,
    parse_models_dev_json, parse_openrouter_models_json, resolve_model_context_length,
    save_models_cache_to_disk, set_user_context_window, set_user_keep_recent_tokens,
    set_user_reserve_tokens, turn_cost, user_keep_for, user_keep_recent_tokens,
    user_reserve_tokens, user_reserve_tokens_for,
};

pub mod ui;
pub use ui::{BackgroundSnapshot, dim_color, dim_line, dim_style, render_dimmed_background};
pub mod icons;
pub use icons::{has_nerd_font, icon, init_nerd_font, set_nerd_font};

mod context_modal;
mod effort;
mod gateway_modal;
mod model_modal;
mod skills_modal;

pub use context_modal::run_context_modal;
mod connect;
mod connect_draw;
mod connect_models;

pub use connect::run_connect_modal;
pub use effort::run_effort_modal;
pub use gateway_modal::run_gateway_modal;
pub use model_modal::run_model_modal;
pub use skills_modal::run_skills_modal;

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

/// Rows for the `/gateway` picker: (command, label, status). Platform rows
/// carry an empty command — their action (connect vs disconnect) is decided at
/// Enter time from the enabled state.
pub fn gateway_modal_rows(
    cfg: &gray_gateway::config::GatewayConfig,
    running: bool,
) -> Vec<(String, String, String)> {
    use gray_gateway::config::Platform;
    let mut rows = Vec::new();
    for plat in [Platform::Telegram, Platform::Discord, Platform::Slack] {
        let status = match cfg.platforms.get(&plat) {
            Some(p) if p.enabled => "enabled".to_string(),
            Some(p) if p.token.as_ref().is_some_and(|t| !t.is_empty()) => {
                "disabled — token saved".to_string()
            }
            _ => "disabled — enter token".to_string(),
        };
        rows.push((String::new(), plat.label().to_string(), status));
    }
    rows.push(("__sep".to_string(), String::new(), String::new()));
    rows.push((
        format!("/gateway {}", if running { "stop" } else { "run" }),
        if running {
            "Stop gateway"
        } else {
            "Start gateway"
        }
        .to_string(),
        String::new(),
    ));
    rows.push((
        "/gateway install".to_string(),
        "Install systemd service".to_string(),
        String::new(),
    ));
    rows.push((
        "/gateway uninstall".to_string(),
        "Remove systemd service".to_string(),
        String::new(),
    ));
    rows.push((
        format!(
            "/gateway autostart {}",
            if cfg.autostart { "off" } else { "on" }
        ),
        format!(
            "Autostart on launch: {}",
            if cfg.autostart { "on" } else { "off" }
        ),
        String::new(),
    ));
    rows
}

/// Move a modal selection, skipping `__sep` spacer rows so arrow keys never
/// land on the invisible gap (e.g. between platforms and actions).
fn move_sel(rows: &[(String, String, String)], sel: usize, delta: i32) -> usize {
    if rows.is_empty() {
        return 0;
    }
    let max = rows.len() - 1;
    let mut next = (sel as i32 + delta).clamp(0, max as i32) as usize;
    while rows[next].0 == "__sep" {
        let stepped = (next as i32 + delta.signum()).clamp(0, max as i32) as usize;
        if stepped == next {
            break;
        }
        next = stepped;
    }
    next
}

pub async fn run_skills_picker(
    cwd: &std::path::Path,
    bg: Option<&BackgroundSnapshot>,
) -> anyhow::Result<Option<(crate::skills::Skill, String)>> {
    run_skills_modal(cwd, bg)
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
