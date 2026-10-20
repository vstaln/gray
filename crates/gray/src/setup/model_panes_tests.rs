use super::{
    GroupId, GroupStates, PaneEfforts, PaneProvider, RailRow, active_rail_index, enclosing_group,
    list_height, match_range, provider_is_active, rail_rows, scoped_config,
};
use crate::config::Config;
use crate::setup::SavedConfig;
use crate::setup::catalog::{ConnectAuth, ConnectItem, plugin_models_key};
use std::collections::BTreeMap;

const BASE_A: &str = "http://127.0.0.1:1/v1";
const BASE_B: &str = "http://127.0.0.1:2/v1";
const STEP_MODEL: &str = "step-3";

fn config(base_url: &str, model: &str, effort: Option<&str>) -> Config {
    Config {
        fast_mode: None,
        model_parts: Default::default(),
        model: Some(model.into()),
        base_url: base_url.into(),
        api_key: Some("live-key".into()),
        thinking_effort: effort.map(str::to_string),
        show_reasoning: None,
        temperature: None,
        top_p: None,
        context_window: None,
        context_reserve: None,
        context_keep: None,
        exec_prefix: None,
        max_turns: None,
        max_cost_micros: None,
        max_wall_secs: None,
        bare: false,
        lean: false,
        compact_at: None,
        provider_id: String::new(),
        credential_source: String::new(),
        auth_ref: String::new(),
    }
}

fn item(id: &str, base_url: &str, auth: ConnectAuth) -> ConnectItem {
    ConnectItem {
        id: id.into(),
        name: id.into(),
        sublabel: String::new(),
        base_url: base_url.into(),
        no_auth: false,
        auth,
    }
}

fn row(item: ConnectItem) -> PaneProvider {
    PaneProvider {
        list_key: PaneProvider::list_key(&item),
        connected: false,
        removable: false,
        installed: None,
        detected: false,
        item,
    }
}

fn plugin_auth() -> ConnectAuth {
    ConnectAuth::Plugin {
        auth_ref: "ref".into(),
        profile_binding: String::new(),
    }
}

#[test]
fn list_key_uses_plugin_pseudo_key_or_base_url() {
    let plugin = item("acme", BASE_A, plugin_auth());
    assert_eq!(PaneProvider::list_key(&plugin), plugin_models_key("acme"));
    let keyed = item("openai", BASE_B, ConnectAuth::ApiKey);
    assert_eq!(PaneProvider::list_key(&keyed), BASE_B);
    let keyless = item("ollama", BASE_A, ConnectAuth::None);
    assert_eq!(PaneProvider::list_key(&keyless), BASE_A);
}

#[test]
fn active_provider_matches_plugin_id_or_base_url() {
    let rail = vec![
        row(item("a", BASE_A, ConnectAuth::ApiKey)),
        row(item("acme", "", plugin_auth())),
        row(item("b", BASE_B, ConnectAuth::ApiKey)),
    ];
    let cfg = config(BASE_B, "m", None);
    assert_eq!(active_rail_index(&rail, &cfg), 2);
    assert!(provider_is_active(&rail[2], &cfg));
    assert!(!provider_is_active(&rail[0], &cfg));
    assert!(
        !provider_is_active(&rail[1], &cfg),
        "an empty base never matches"
    );

    let mut plugin = config("http://relay/v1", "m", None);
    plugin.credential_source = "plugin".into();
    plugin.provider_id = "acme".into();
    assert_eq!(active_rail_index(&rail, &plugin), 1);
    assert!(provider_is_active(&rail[1], &plugin));

    plugin.base_url = BASE_A.into();
    assert_eq!(
        active_rail_index(&rail, &plugin),
        1,
        "a plugin config never matches by base"
    );
    assert!(!provider_is_active(&rail[0], &plugin));

    let relay = "https://127.0.0.1:1/";
    let shared = vec![
        row(item("first", relay, plugin_auth())),
        row(item("acme", relay, plugin_auth())),
    ];
    let mut on_relay = config(relay, "m", None);
    on_relay.credential_source = "plugin".into();
    on_relay.provider_id = "acme".into();
    assert_eq!(
        active_rail_index(&shared, &on_relay),
        1,
        "placeholder bases are shared"
    );
    assert!(!provider_is_active(&shared[0], &on_relay));
    let keyed_on_relay = config(relay, "m", None);
    assert!(!provider_is_active(&shared[0], &keyed_on_relay));

    let elsewhere = config("http://127.0.0.1:9/v1", "m", None);
    assert_eq!(active_rail_index(&rail, &elsewhere), 0);
}

#[test]
fn row_key_prefers_store_then_live_session_key() {
    let a = row(item("a", BASE_A, ConnectAuth::ApiKey));
    let b = row(item("b", BASE_B, ConnectAuth::ApiKey));
    let cfg = config(BASE_A, "m", None);
    let mut keys = BTreeMap::new();
    assert_eq!(a.key(&keys, &cfg).as_deref(), Some("live-key"));
    assert_eq!(b.key(&keys, &cfg), None);
    keys.insert("a".to_string(), "stored".to_string());
    keys.insert("b".to_string(), String::new());
    assert_eq!(a.key(&keys, &cfg).as_deref(), Some("stored"));
    assert_eq!(b.key(&keys, &cfg), None, "an empty stored key is no key");
}

#[test]
fn stepped_effort_stays_on_its_provider() {
    let a = row(item("a", BASE_A, ConnectAuth::ApiKey));
    let b = row(item("b", BASE_B, ConnectAuth::ApiKey));
    let cfg = config(BASE_A, "other/live", None);
    let keys = BTreeMap::new();
    let saved = SavedConfig::default();
    let (sa, sb) = (
        scoped_config(&cfg, &a, &keys),
        scoped_config(&cfg, &b, &keys),
    );
    let mut efforts = PaneEfforts::default();
    efforts.step(&sa, &saved, &a, STEP_MODEL, false);
    assert_eq!(efforts.shown(&sa, &saved, &a, STEP_MODEL), Some("medium"));
    assert!(efforts.was_stepped(&a, STEP_MODEL));
    assert_eq!(efforts.shown(&sb, &saved, &b, STEP_MODEL), Some("high"));
    assert!(!efforts.was_stepped(&b, STEP_MODEL));
}

#[test]
fn live_effort_only_shows_on_the_active_provider() {
    let a = row(item("a", BASE_A, ConnectAuth::ApiKey));
    let b = row(item("b", BASE_B, ConnectAuth::ApiKey));
    let cfg = config(BASE_A, STEP_MODEL, Some("low"));
    let keys = BTreeMap::new();
    let saved = SavedConfig::default();
    let efforts = PaneEfforts::default();
    let sa = scoped_config(&cfg, &a, &keys);
    let sb = scoped_config(&cfg, &b, &keys);
    assert_eq!(efforts.shown(&sa, &saved, &a, STEP_MODEL), Some("low"));
    assert_eq!(efforts.shown(&sb, &saved, &b, STEP_MODEL), Some("high"));
    assert_eq!(sb.model, None);
    assert_eq!(sb.api_key, None);
}

#[test]
fn list_height_matches_draw_geometry() {
    assert_eq!(list_height(5), 0);
    assert_eq!(list_height(10), 1);
    assert_eq!(list_height(14), 5);
    assert_eq!(list_height(20), 10);
    assert_eq!(list_height(24), 14);
    assert_eq!(list_height(60), 16);
    for term_h in 10..=60u16 {
        let inner = term_h.clamp(10, 24).min(term_h - 2) - 2;
        let chrome = if inner >= 12 { 6 } else { 5 };
        assert_eq!(list_height(term_h), (inner - chrome) as usize);
    }
}

#[test]
fn match_range_finds_case_insensitive_char_span() {
    assert_eq!(match_range("Claude Opus 5.5", "opu"), Some((7, 10)));
    assert_eq!(match_range("Claude Opus 5.5", "CLAUDE"), Some((0, 6)));
    assert_eq!(match_range("Kimi K3 · fast…", "fast"), Some((10, 14)));
    assert_eq!(match_range("SWE-2", ""), None);
    assert_eq!(match_range("SWE-2", "swe-2x"), None);
    assert_eq!(match_range("GPT-6 Sol", "luna"), None);
}

#[test]
fn step_reports_whether_the_level_moved() {
    let a = row(item("a", BASE_A, ConnectAuth::ApiKey));
    let cfg = config(BASE_A, "other/live", None);
    let saved = SavedConfig::default();
    let sa = scoped_config(&cfg, &a, &BTreeMap::new());
    let mut efforts = PaneEfforts::default();
    let mut downs = 0;
    while efforts.step(&sa, &saved, &a, STEP_MODEL, false) {
        downs += 1;
        assert!(downs < 10);
    }
    assert!(downs > 0);
    assert!(!efforts.step(&sa, &saved, &a, STEP_MODEL, false));
    assert!(efforts.step(&sa, &saved, &a, STEP_MODEL, true));
}

#[test]
fn rail_rows_flat_then_custom_then_groups() {
    let mut rail = vec![
        row(item(
            "openai",
            "https://api.openai.com/v1",
            ConnectAuth::ApiKey,
        )),
        row(item("custom", "", ConnectAuth::ApiKey)),
        row(item("devin", "http://x", plugin_auth())),
        row(item(
            "mistral",
            "https://api.mistral.ai/v1",
            ConnectAuth::ApiKey,
        )),
    ];
    rail[0].connected = true; // connected API key stays flat
    let rows = rail_rows(&rail, &GroupStates::default());
    assert_eq!(
        rows,
        vec![
            RailRow::Provider(0),
            RailRow::Provider(1), // + Custom endpoint after the flat section
            RailRow::Group(GroupId::Logins),
            RailRow::Provider(2), // devin (login, open by default)
            RailRow::Group(GroupId::ApiKey),
        ]
    );
    // API-key group starts closed: mistral hidden behind the header.
    assert!(!rows.contains(&RailRow::Provider(3)));
}

#[test]
fn rail_rows_open_api_key_group_reveals_members() {
    let rail = vec![
        row(item(
            "openai",
            "https://api.openai.com/v1",
            ConnectAuth::ApiKey,
        )),
        row(item(
            "mistral",
            "https://api.mistral.ai/v1",
            ConnectAuth::ApiKey,
        )),
    ];
    let mut groups = GroupStates::default();
    groups.set(GroupId::ApiKey, true);
    let rows = rail_rows(&rail, &groups);
    assert_eq!(
        rows,
        vec![
            RailRow::Group(GroupId::ApiKey),
            RailRow::Provider(0),
            RailRow::Provider(1),
        ]
    );
    assert_eq!(enclosing_group(&rows, 2), Some(0));
}

#[test]
fn rail_rows_logins_holds_connected_and_new_logins() {
    let mut rail = vec![
        row(item("devin", "http://x", plugin_auth())),
        row(item("claude", "http://y", plugin_auth())),
    ];
    rail[0].connected = true; // connected logins stay inside the group
    let rows = rail_rows(&rail, &GroupStates::default());
    assert_eq!(
        rows,
        vec![
            RailRow::Group(GroupId::Logins),
            RailRow::Provider(0),
            RailRow::Provider(1),
        ]
    );
}

#[test]
fn rail_rows_detected_and_keyless_stay_flat() {
    let mut rail = vec![
        row(item(
            "openai",
            "https://api.openai.com/v1",
            ConnectAuth::ApiKey,
        )),
        row(item(
            "ollama",
            "http://localhost:11434/v1",
            ConnectAuth::None,
        )),
    ];
    rail[0].detected = true;
    let rows = rail_rows(&rail, &GroupStates::default());
    assert_eq!(rows, vec![RailRow::Provider(0), RailRow::Provider(1)]);
}
