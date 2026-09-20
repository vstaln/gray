// UNRUN (cargo test banned under X): run in TTY/CI.
// Shift+Enter removal policy + the confirmation render.

use super::connect_draw::{render_confirm_remove, render_selecting};
use super::*;
use catalog::{load_mixed_store, save_mixed_store};
use ratatui::style::Color;
use std::collections::BTreeMap;

fn item(id: &str, name: &str, base_url: &str) -> ConnectItem {
    ConnectItem {
        id: id.to_string(),
        name: name.to_string(),
        sublabel: "(API key)".to_string(),
        base_url: base_url.to_string(),
        no_auth: false,
    }
}

fn colors() -> ConnectColors {
    ConnectColors {
        box_bg: Color::Black,
        input_bg: Color::Black,
        accent_peach: Color::Yellow,
        text_dim: Color::DarkGray,
    }
}

fn auth_map(entries: &[(&str, &str)]) -> BTreeMap<String, catalog::AuthEntry> {
    entries
        .iter()
        .map(|(pid, key)| {
            (
                (*pid).to_string(),
                catalog::AuthEntry::Key((*key).to_string()),
            )
        })
        .collect()
}

#[test]
fn only_rows_holding_a_credential_are_removable() {
    let mut config = crate::config::Config {
        model: Some("openrouter/auto".into()),
        base_url: "https://openrouter.ai/api/v1".into(),
        api_key: None,
        thinking_effort: None,
        show_reasoning: None,
        temperature: None,
        top_p: None,
        context_window: None,
        context_reserve: None,
        context_keep: None,
        max_turns: None,
        max_cost_micros: None,
        max_wall_secs: None,
    };
    let auth = auth_map(&[("openrouter", "sk-or"), ("custom", "sk-custom")]);

    let openrouter = item("openrouter", "OpenRouter", "https://openrouter.ai/api/v1");
    let custom = item("custom", "Custom", "https://example.test/v1");
    let fresh = item("anthropic", "Anthropic", "https://api.anthropic.com");

    // Connected through auth.json.
    assert!(is_removable(&openrouter, &config, &auth));
    // Custom never shows a check, but its stored key is still removable.
    assert!(is_removable(&custom, &config, &auth));
    // A provider with nothing stored is not.
    assert!(!is_removable(&fresh, &config, &auth));

    // The active provider is removable through config alone (no auth entry).
    config.api_key = Some("sk-live".into());
    let empty = BTreeMap::new();
    assert!(is_removable(&openrouter, &config, &empty));
    assert!(!is_removable(&fresh, &config, &empty));
}

#[test]
fn confirm_dialog_names_provider_and_both_keys() {
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
    let target = item(
        "commandcode",
        "CommandCode",
        "https://api.commandcode.dev/v1",
    );
    terminal
        .draw(|frame| {
            render_confirm_remove(frame, frame.area(), &target, &colors());
        })
        .unwrap();
    let text: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(text.contains("Remove provider"), "{text}");
    assert!(text.contains("Remove CommandCode?"), "{text}");
    assert!(text.contains("api.commandcode.dev"), "{text}");
    assert!(text.contains("API key will be deleted"), "{text}");
    assert!(text.contains("confirm"), "{text}");
    assert!(text.contains("cancel"), "{text}");
}

#[test]
fn list_footer_advertises_removal_only_for_a_stored_provider() {
    let config = crate::config::Config {
        model: Some("openrouter/auto".into()),
        base_url: "https://openrouter.ai/api/v1".into(),
        api_key: Some("sk-live".into()),
        thinking_effort: None,
        show_reasoning: None,
        temperature: None,
        top_p: None,
        context_window: None,
        context_reserve: None,
        context_keep: None,
        max_turns: None,
        max_cost_micros: None,
        max_wall_secs: None,
    };
    let auth = auth_map(&[("openrouter", "sk-or")]);
    let mut items = build_connect_items(&load_catalog().unwrap());
    catalog::sort_connect_items(&mut items, &config, &auth);
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
    let mut scroll = 0usize;
    // Highlight the first connected provider: the binding is advertised.
    terminal
        .draw(|frame| {
            render_selecting(
                frame,
                frame.area(),
                &items,
                "",
                0,
                &mut scroll,
                &config,
                &auth,
                &colors(),
            )
        })
        .unwrap();
    let text: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(text.contains("shift+enter"), "{text}");
    assert!(text.contains("remove"), "{text}");
}

fn config_for(base_url: &str) -> crate::config::Config {
    crate::config::Config {
        model: Some("openrouter/auto".into()),
        base_url: base_url.into(),
        api_key: Some("sk-live".into()),
        thinking_effort: None,
        show_reasoning: None,
        temperature: None,
        top_p: None,
        context_window: None,
        context_reserve: None,
        context_keep: None,
        max_turns: None,
        max_cost_micros: None,
        max_wall_secs: None,
    }
}

#[test]
fn removing_the_active_provider_clears_both_credential_copies() {
    let dir = tempfile::tempdir().unwrap();
    let auth_path = dir.path().join("auth.json");
    let saved_path = dir.path().join("config.json");
    // The active provider is the one case where the key exists twice: once in
    // auth.json, once in the saved config that survives a restart.
    let mut store = load_mixed_store(&auth_path);
    store.insert("openrouter".into(), catalog::AuthEntry::Key("sk-or".into()));
    save_mixed_store(&auth_path, &store).unwrap();
    let mut saved = load_saved_config_at(&saved_path);
    saved.base_url = Some("https://openrouter.ai/api/v1".into());
    saved.api_key = Some("sk-live".into());
    saved.model = Some("openrouter/auto".into());
    saved.auth_mode = Some(AUTH_MODE_API_KEY.into());
    save_saved_config_at(&saved_path, &saved).unwrap();

    let mut config = config_for("https://openrouter.ai/api/v1");
    let target = item("openrouter", "OpenRouter", "https://openrouter.ai/api/v1");
    let name = forget_provider_at(&target, &mut config, &auth_path, &saved_path).unwrap();

    assert_eq!(name, "OpenRouter");
    assert!(config.api_key.is_none(), "in-memory key must go");
    assert!(
        !load_mixed_store(&auth_path).contains_key("openrouter"),
        "stored key must go"
    );
    let saved = load_saved_config_at(&saved_path);
    assert!(saved.api_key.is_none(), "saved key must not resurrect it");
    assert!(saved.auth_mode.is_none());
    assert_eq!(
        saved.model.as_deref(),
        Some("openrouter/auto"),
        "the model choice survives a credential removal"
    );
}

#[test]
fn removing_another_provider_leaves_the_active_one_alone() {
    let dir = tempfile::tempdir().unwrap();
    let auth_path = dir.path().join("auth.json");
    let saved_path = dir.path().join("config.json");
    let mut store = load_mixed_store(&auth_path);
    store.insert("openrouter".into(), catalog::AuthEntry::Key("sk-or".into()));
    store.insert(
        "commandcode".into(),
        catalog::AuthEntry::Key("sk-cc".into()),
    );
    save_mixed_store(&auth_path, &store).unwrap();
    let mut saved = load_saved_config_at(&saved_path);
    saved.base_url = Some("https://openrouter.ai/api/v1".into());
    saved.api_key = Some("sk-live".into());
    save_saved_config_at(&saved_path, &saved).unwrap();

    let mut config = config_for("https://openrouter.ai/api/v1");
    let other = item(
        "commandcode",
        "CommandCode",
        "https://api.commandcode.dev/v1",
    );
    forget_provider_at(&other, &mut config, &auth_path, &saved_path).unwrap();

    assert!(
        config.api_key.is_some(),
        "the active provider keeps its key"
    );
    let saved = load_saved_config_at(&saved_path);
    assert_eq!(saved.api_key.as_deref(), Some("sk-live"));
    let left = load_mixed_store(&auth_path);
    assert!(!left.contains_key("commandcode"));
    assert!(left.contains_key("openrouter"));
}

#[test]
fn list_footer_hides_removal_for_a_provider_with_nothing_stored() {
    let config = config_for("https://openrouter.ai/api/v1");
    let auth = BTreeMap::new();
    let mut items = build_connect_items(&load_catalog().unwrap());
    catalog::sort_connect_items(&mut items, &config, &auth);
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
    let mut scroll = 0usize;
    terminal
        .draw(|frame| {
            render_selecting(
                frame,
                frame.area(),
                &items,
                "anthropic",
                0,
                &mut scroll,
                &config,
                &auth,
                &colors(),
            )
        })
        .unwrap();
    let text: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(text.contains("Anthropic"), "{text}");
    assert!(
        !text.contains("shift+enter"),
        "a provider with no stored key offers no removal: {text}"
    );
}
