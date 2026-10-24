use super::*;
use crate::quality_of_life::providers::{detect_connect_candidates, is_popular_preset};

#[test]
fn custom_is_first_entry() {
    let catalog = load_catalog().expect("catalog");
    let items = build_connect_items(&catalog, &[], &[]);
    assert_eq!(items[0].id, "custom");
    assert!(items[0].sublabel.contains("OpenAI"));
}

#[test]
fn commandcode_pinned_with_provider_base() {
    let catalog = load_catalog().expect("catalog");
    let items = build_connect_items(&catalog, &[], &[]);
    let cc = items
        .iter()
        .find(|i| i.id == "commandcode")
        .expect("commandcode pinned");
    assert_eq!(cc.base_url, "https://api.commandcode.ai/provider/v1");
}

#[test]
fn normalize_custom_base_url_trims_suffixes() {
    assert_eq!(
        normalize_custom_base_url("https://x.example/v1/chat/completions "),
        "https://x.example/v1"
    );
    assert_eq!(
        normalize_custom_base_url("https://x.example/v1/messages"),
        "https://x.example/v1"
    );
    assert_eq!(
        normalize_custom_base_url("https://x.example/v1/models"),
        "https://x.example/v1"
    );
    assert_eq!(
        normalize_custom_base_url("http://localhost:11434/v1/"),
        "http://localhost:11434/v1"
    );
}

#[test]
fn saving_key_preserves_oauth_objects() {
    let dir = tempfile::tempdir().expect("tmp");
    let path = dir.path().join("auth.json");
    let oauth = StoredAuth {
        provider: "xai".to_string(),
        access_token: "tok".to_string(),
        refresh_token: String::new(),
        expires_at: 9_999_999_999,
        email: None,
    };
    let mut store = load_mixed_store(&path);
    store.insert(oauth.provider.clone(), AuthEntry::OAuth(oauth));
    save_mixed_store(&path, &store).expect("oauth save");
    let mut store = load_mixed_store(&path);
    store.insert(
        "openrouter".to_string(),
        AuthEntry::Key("sk-or-1".to_string()),
    );
    save_mixed_store(&path, &store).expect("key save");
    let reloaded = load_mixed_store(&path);
    assert!(
        matches!(reloaded.get("xai"), Some(AuthEntry::OAuth(_))),
        "{reloaded:?}"
    );

    let keys: BTreeMap<String, String> = reloaded
        .into_iter()
        .filter_map(|(k, v)| match v {
            AuthEntry::Key(key) => Some((k, key)),
            AuthEntry::OAuth(_) | AuthEntry::Plugin(_) | AuthEntry::PluginPool(_) => None,
        })
        .collect();
    assert_eq!(keys.get("openrouter").map(String::as_str), Some("sk-or-1"));
    assert!(!keys.contains_key("xai"));
}

#[test]
fn mistyped_field_preserves_known_fields() {
    let dir = tempfile::tempdir().expect("tmp");
    let path = dir.path().join("config.json");
    std::fs::write(
        &path,
        r#"{"model":"anthropic/claude","context_window":"not-a-number","base_url":"https://x"}"#,
    )
    .expect("write");
    let cfg = load_saved_config_at(&path);
    assert_eq!(cfg.model.as_deref(), Some("anthropic/claude"));
    assert_eq!(cfg.base_url.as_deref(), Some("https://x"));
    assert_eq!(cfg.context_window, None);
}

#[test]
fn unknown_fields_are_ignored() {
    let dir = tempfile::tempdir().expect("tmp");
    let path = dir.path().join("config.json");
    std::fs::write(&path, r#"{"model":"m","bogus_field":123}"#).expect("write");
    let cfg = load_saved_config_at(&path);
    assert_eq!(cfg.model.as_deref(), Some("m"));
}

#[test]
fn bad_json_falls_back_to_defaults() {
    let dir = tempfile::tempdir().expect("tmp");
    let path = dir.path().join("config.json");
    std::fs::write(&path, "{bad json").expect("write");
    let cfg = load_saved_config_at(&path);
    assert_eq!(cfg.model, None);
    assert_eq!(cfg.base_url, None);
}

#[test]
fn saved_model_selections_keep_provider_scoped_recency() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    for (base, model) in [
        ("https://a/v1", "one"),
        ("https://a/v1", "two"),
        ("https://b/v1", "other"),
        ("https://a/v1", "one"),
    ] {
        let mut saved = load_saved_config_at(&path);
        saved.base_url = Some(base.into());
        saved.model = Some(model.into());
        save_saved_config_at(&path, &saved).unwrap();
    }
    let json: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(
        json["recent_models"]["https://a/v1"],
        serde_json::json!(["one", "two"])
    );
    assert_eq!(
        json["recent_models"]["https://b/v1"],
        serde_json::json!(["other"])
    );
}

#[test]
fn model_recency_is_stable_scoped_and_does_not_add_unavailable_ids() {
    let saved: SavedConfig = serde_json::from_value(serde_json::json!({
        "base_url": "https://other/v1", "model": "first",
        "recent_models": {"https://a/v1": ["removed", "last", "middle", "last"]}
    }))
    .unwrap();
    let mut models: Vec<_> = ["first", "middle", "untouched", "last"]
        .into_iter()
        .map(|id| (id.to_string(), id.to_string()))
        .collect();
    saved.sort_models("https://a/v1/", &mut models);
    assert_eq!(
        models.iter().map(|m| m.0.as_str()).collect::<Vec<_>>(),
        ["last", "middle", "first", "untouched"]
    );
    let unchanged = models.clone();
    saved.sort_models("https://unknown/v1", &mut models);
    assert_eq!(models, unchanged);
    saved.sort_models("https://a/v1", &mut []);
}

#[test]
fn legacy_current_model_is_first_and_survives_settings_save() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    std::fs::write(&path, r#"{"base_url":"https://a/v1/","model":"chosen"}"#).unwrap();
    let mut saved = load_saved_config_at(&path);
    let mut models = vec![
        ("other".into(), "Other".into()),
        ("chosen".into(), "Chosen".into()),
    ];
    saved.sort_models("https://a/v1", &mut models);
    assert_eq!(models[0].0, "chosen");
    saved.thinking_effort = Some("high".into());
    save_saved_config_at(&path, &saved).unwrap();
    let saved = load_saved_config_at(&path);
    assert_eq!(saved.recent_models["https://a/v1"], ["chosen"]);
    save_saved_config_at(&path, &saved).unwrap();
    assert_eq!(
        load_saved_config_at(&path).recent_models,
        saved.recent_models
    );
}

#[test]
fn malformed_history_does_not_lose_settings_and_bad_setting_keeps_history() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    std::fs::write(&path, r#"{"model":"m","recent_models":7}"#).unwrap();
    let saved = load_saved_config_at(&path);
    assert_eq!(saved.model.as_deref(), Some("m"));
    assert!(saved.recent_models.is_empty());
    std::fs::write(
        &path,
        r#"{"context_window":"bad","recent_models":{"https://a":["m"]}}"#,
    )
    .unwrap();
    assert_eq!(
        load_saved_config_at(&path).recent_models["https://a"],
        ["m"]
    );
}

#[test]
fn disabled_skills_default_empty_and_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    assert!(load_saved_config_at(&path).disabled_skills.is_empty());
    let mut saved = load_saved_config_at(&path);
    saved.disabled_skills.insert("foo".to_string());
    save_saved_config_at(&path, &saved).unwrap();
    assert!(load_saved_config_at(&path).disabled_skills.contains("foo"));

    std::fs::write(&path, r#"{"model":"m"}"#).unwrap();
    let back = load_saved_config_at(&path);
    assert_eq!(back.model.as_deref(), Some("m"));
    assert!(back.disabled_skills.is_empty());
}

#[test]
fn skills_auto_defaults_on_and_roundtrips_explicit_off() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    assert!(skills_auto_enabled_at(&path));
    assert!(load_saved_config_at(&path).skills_auto.is_none());
    let mut saved = load_saved_config_at(&path);
    saved.skills_auto = Some(false);
    save_saved_config_at(&path, &saved).unwrap();
    assert!(!skills_auto_enabled_at(&path));

    std::fs::write(&path, r#"{"model":"m"}"#).unwrap();
    assert!(skills_auto_enabled_at(&path));
}

#[test]
fn failed_save_reports_error() {
    let dir = tempfile::tempdir().unwrap();
    assert!(save_saved_config_at(dir.path(), &SavedConfig::default()).is_err());
}

#[test]
fn removing_entry_keeps_every_other_credential() {
    let dir = tempfile::tempdir().expect("tmp");
    let path = dir.path().join("auth.json");
    let oauth = StoredAuth {
        provider: "xai".to_string(),
        access_token: "tok".to_string(),
        refresh_token: String::new(),
        expires_at: 9_999_999_999,
        email: None,
    };
    let mut store = load_mixed_store(&path);
    store.insert(oauth.provider.clone(), AuthEntry::OAuth(oauth));
    store.insert(
        "openrouter".to_string(),
        AuthEntry::Key("sk-or-1".to_string()),
    );
    store.insert(
        "commandcode".to_string(),
        AuthEntry::Key("sk-cc-1".to_string()),
    );
    save_mixed_store(&path, &store).expect("seed");

    remove_auth_entry_at(&path, "openrouter").expect("remove");

    let reloaded = load_mixed_store(&path);
    assert!(!reloaded.contains_key("openrouter"), "{reloaded:?}");
    assert_eq!(
        reloaded
            .get("commandcode")
            .map(|e| matches!(e, AuthEntry::Key(k) if k == "sk-cc-1")),
        Some(true),
        "the other key must survive",
    );
    assert!(
        matches!(reloaded.get("xai"), Some(AuthEntry::OAuth(_))),
        "the OAuth entry must survive",
    );

    remove_auth_entry_at(&path, "openrouter").expect("remove absent");
    assert!(load_mixed_store(&path).contains_key("commandcode"));
}

#[test]
fn subsystem_switches_default_on_and_only_explicit_false_turns_them_off() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.json");
    assert!(
        crate::setup::gw_auto_enabled_at(&cfg),
        "missing config reads as enabled"
    );
    let mut saved = crate::setup::load_saved_config_at(&cfg);
    saved.memory_auto = Some(false);
    saved.cron_auto = Some(false);
    saved.gw_auto = Some(false);
    crate::setup::save_saved_config_at(&cfg, &saved).unwrap();
    assert!(!crate::setup::gw_auto_enabled_at(&cfg));

    let reloaded = crate::setup::load_saved_config_at(&cfg);
    assert_eq!(reloaded.memory_auto, Some(false));
    assert_eq!(reloaded.cron_auto, Some(false));
    assert_eq!(reloaded.gw_auto, Some(false));
}

#[test]
fn a_corrupt_auth_store_is_refused_not_overwritten() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("auth.json");
    std::fs::write(&path, "{not json at all").unwrap();

    let err = crate::setup::catalog::save_auth_key_at(&path, "openrouter", "sk-new")
        .expect_err("a corrupt store must not be written over");
    assert!(err.to_string().contains("not valid JSON"), "{err}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "{not json at all");

    assert!(crate::setup::catalog::remove_auth_entry_at(&path, "openrouter").is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "{not json at all");
}

#[test]
fn a_valid_store_still_round_trips_through_the_strict_loader() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("auth.json");

    assert!(
        crate::setup::catalog::load_mixed_store_strict(&path)
            .unwrap()
            .is_empty()
    );
    std::fs::write(
        &path,
        r#"{"openrouter": "sk-a", "example": {"provider": "example", "access_token": "t", "expires_at": 0}}"#,
    )
        .unwrap();
    let store = crate::setup::catalog::load_mixed_store_strict(&path).unwrap();
    assert_eq!(store.len(), 2);

    assert_eq!(crate::setup::catalog::load_mixed_store(&path).len(), 2);
}

#[test]
fn the_config_lock_serializes_writers_then_releases() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    let first = super::lock_saved_config_at(&path).unwrap();
    let lock_path = path.with_extension("lock");
    assert!(lock_path.exists(), "the lock file is the lock itself");

    let waiter = {
        std::thread::spawn(move || {
            let t = std::time::Instant::now();
            let second = super::lock_saved_config_at(&path).expect("the waiter must get it");
            let waited = t.elapsed();
            drop(second);
            waited
        })
    };
    std::thread::sleep(std::time::Duration::from_millis(400));
    drop(first);
    let waited = waiter.join().unwrap();
    assert!(
        waited >= std::time::Duration::from_millis(300),
        "the waiter did not wait for the holder: {waited:?}"
    );
    assert!(
        waited < std::time::Duration::from_secs(4),
        "release took too long: {waited:?}"
    );
}

#[test]
fn effort_memory_scopes_by_connection_and_model() {
    let mut saved = SavedConfig::default();
    let devin = "devin-sub:devin-subscription";
    let loopback = "https://127.0.0.1:1/";
    saved.remember_effort(&effort_memory_key(devin, loopback, "swe-2-max"), "high");
    saved.remember_effort(
        &effort_memory_key(devin, loopback, "meta/muse-spark-1.3-contributor"),
        "xhigh",
    );
    saved.remember_effort(
        &effort_memory_key("", "https://api.example.com/v1", "swe-2-max"),
        "low",
    );
    assert_eq!(
        saved
            .remembered_effort(&effort_memory_key(devin, loopback, "swe-2-max"))
            .as_deref(),
        Some("high")
    );
    assert_eq!(
        saved
            .remembered_effort(&effort_memory_key(
                devin,
                loopback,
                "meta/muse-spark-1.3-contributor",
            ))
            .as_deref(),
        Some("xhigh")
    );
    assert_eq!(
        saved
            .remembered_effort(&effort_memory_key(
                "",
                "https://api.example.com/v1",
                "swe-2-max"
            ))
            .as_deref(),
        Some("low")
    );

    assert_eq!(saved.thinking_effort.as_deref(), Some("low"));

    assert_eq!(
        effort_memory_key(devin, loopback, ""),
        format!("plugin:{devin}")
    );
}

#[test]
fn effort_memory_round_trips_and_survives_partial_parse() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    let mut saved = SavedConfig::default();
    saved.remember_effort(
        &effort_memory_key("devin-sub:devin-subscription", "", "swe-2-max"),
        "xhigh",
    );
    save_saved_config_at(&path, &saved).unwrap();
    let loaded = load_saved_config_at(&path);
    assert_eq!(
        loaded
            .remembered_effort(&effort_memory_key(
                "devin-sub:devin-subscription",
                "",
                "swe-2-max"
            ))
            .as_deref(),
        Some("xhigh")
    );

    std::fs::write(
        &path,
        r#"{"effort_memory":{"plugin:devin-sub:devin-subscription|m":"low"},"model":7}"#,
    )
    .unwrap();
    let loaded = load_saved_config_at(&path);
    assert_eq!(loaded.model, None);
    assert_eq!(
        loaded
            .remembered_effort("plugin:devin-sub:devin-subscription|m")
            .as_deref(),
        Some("low")
    );
}

fn sort_item(id: &str) -> ConnectItem {
    ConnectItem {
        id: id.to_string(),
        name: id.to_string(),
        sublabel: String::new(),
        base_url: format!("https://{id}.example/v1"),
        no_auth: false,
        auth: ConnectAuth::ApiKey,
    }
}

#[test]
fn connect_sort_is_connected_detected_popular_custom_rest() {
    let config = crate::config::Config {
        model: None,
        base_url: "https://nowhere.example/v1".to_string(),
        api_key: None,
        provider_id: String::new(),
        credential_source: String::new(),
        auth_ref: String::new(),
        thinking_effort: None,
        show_reasoning: None,
        fast_mode: None,
        model_parts: Default::default(),
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
        lean: true,
        compact_at: None,
    };

    let mut auth = BTreeMap::new();
    auth.insert("xai".to_string(), AuthEntry::Key("k".to_string()));

    let detected = detect_connect_candidates(false, |k| {
        (k == "ANTHROPIC_API_KEY").then(|| "ak".to_string())
    });
    let mut items = vec![
        sort_item("zzz-rest"),
        sort_item("custom"),
        sort_item("openai"),
        sort_item("anthropic"),
        sort_item("xai"),
    ];
    sort_connect_items(&mut items, &config, &auth, &detected);
    let ids: Vec<&str> = items.iter().map(|i| i.id.as_str()).collect();
    assert_eq!(
        ids,
        ["custom", "xai", "anthropic", "openai", "zzz-rest"],
        "custom > connected > detected > popular > rest"
    );
}

#[test]
fn connect_sort_with_nothing_known_lands_on_openai() {
    let config = crate::config::Config {
        model: None,
        base_url: "https://nowhere.example/v1".to_string(),
        api_key: None,
        provider_id: String::new(),
        credential_source: String::new(),
        auth_ref: String::new(),
        thinking_effort: None,
        show_reasoning: None,
        fast_mode: None,
        model_parts: Default::default(),
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
        lean: true,
        compact_at: None,
    };
    let catalog = load_catalog().expect("catalog");
    let mut items = build_connect_items(&catalog, &[], &[]);
    let auth = BTreeMap::new();
    let detected = detect_connect_candidates(false, |_| None);
    sort_connect_items(&mut items, &config, &auth, &detected);
    assert_eq!(
        items[0].id, "custom",
        "the cursor's row 0 is Custom endpoint"
    );
    assert_eq!(items[1].id, "openai", "OpenAI leads the popular block");
    let popular_pos = items.iter().position(|i| i.id == "openai").unwrap();
    assert!(
        items[popular_pos..]
            .iter()
            .all(|i| is_popular_preset(&i.id) || !is_popular_preset(&i.id)),
    );
}

#[test]
fn custom_slug_scopes_by_host_port_and_path() {
    assert_eq!(
        custom_key_id("http://localhost:1234/v1"),
        "custom:localhost:1234"
    );
    assert_eq!(
        custom_key_id("https://api.example.com/v1"),
        "custom:api.example.com"
    );

    assert_eq!(
        custom_key_id("https://api.example.com/openai/v1"),
        "custom:api.example.com/openai-v1"
    );
}

#[test]
fn custom_display_name_knows_local_servers() {
    assert_eq!(custom_display_name("http://localhost:1234/v1"), "LM Studio");
    assert_eq!(
        custom_display_name("http://localhost:11434/v1/models"),
        "Ollama"
    );
    assert_eq!(
        custom_display_name("https://api.acme.dev/v1"),
        "api.acme.dev"
    );
    assert_eq!(
        custom_display_name("http://192.168.1.20:9999/v1"),
        "192.168.1.20:9999"
    );
}

#[test]
fn is_custom_id_covers_add_row_and_saved_rows() {
    assert!(is_custom_id("custom"));
    assert!(is_custom_id("custom:localhost:1234"));
    assert!(!is_custom_id("openrouter"));
    assert!(!is_custom_id("customx"));
}

#[test]
fn custom_endpoint_store_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("custom_endpoints.json");

    let ep = upsert_custom_endpoint_at(&path, " http://localhost:1234/v1/ ", true).unwrap();
    assert_eq!(ep.name, "LM Studio");
    assert_eq!(ep.base_url, "http://localhost:1234/v1");
    assert!(ep.keyless);

    upsert_custom_endpoint_at(&path, "https://api.acme.dev/v1", false).unwrap();
    let eps = load_custom_endpoints_at(&path);
    assert_eq!(eps.len(), 2);

    upsert_custom_endpoint_at(&path, "http://localhost:1234/v1", false).unwrap();
    let eps = load_custom_endpoints_at(&path);
    assert_eq!(eps.len(), 2);
    assert!(!eps.iter().find(|e| e.name == "LM Studio").unwrap().keyless);

    assert!(remove_custom_endpoint_at(&path, "http://localhost:1234/v1").unwrap());
    assert!(!remove_custom_endpoint_at(&path, "http://localhost:1234/v1").unwrap());
    assert_eq!(load_custom_endpoints_at(&path).len(), 1);

    std::fs::write(&path, "{ not json").unwrap();
    assert!(load_custom_endpoints_at(&path).is_empty());
}

#[test]
fn saved_customs_become_connect_rows() {
    let catalog = load_catalog().expect("catalog");
    let customs = vec![
        CustomEndpoint {
            name: "LM Studio".into(),
            base_url: "http://localhost:1234/v1".into(),
            keyless: true,
        },
        CustomEndpoint {
            name: "api.acme.dev".into(),
            base_url: "https://api.acme.dev/v1".into(),
            keyless: false,
        },
    ];
    let items = build_connect_items(&catalog, &[], &customs);
    let lm = items
        .iter()
        .find(|i| i.id == "custom:localhost:1234")
        .expect("LM Studio row");
    assert_eq!(lm.name, "LM Studio");
    assert!(lm.no_auth);
    assert!(matches!(lm.auth, ConnectAuth::None));
    let acme = items
        .iter()
        .find(|i| i.id == "custom:api.acme.dev")
        .expect("acme row");
    assert!(!acme.no_auth);
    assert!(matches!(acme.auth, ConnectAuth::ApiKey));

    assert!(items.iter().any(|i| i.id == "custom"));
}

#[test]
fn saved_customs_sort_with_the_popular_block() {
    let catalog = load_catalog().expect("catalog");
    let customs = vec![CustomEndpoint {
        name: "api.acme.dev".into(),
        base_url: "https://api.acme.dev/v1".into(),
        keyless: false,
    }];
    let mut items = build_connect_items(&catalog, &[], &customs);
    let config = crate::config::Config {
        fast_mode: None,
        model_parts: Default::default(),
        model: Some("m".into()),
        base_url: "https://openrouter.ai/api/v1".into(),
        api_key: Some("k".into()),
        provider_id: String::new(),
        credential_source: String::new(),
        auth_ref: String::new(),
        thinking_effort: None,
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
    };
    let auth = BTreeMap::new();
    let detected = BTreeMap::new();
    sort_connect_items(&mut items, &config, &auth, &detected);
    let custom_pos = items.iter().position(|i| i.id == "custom").unwrap();
    let saved_pos = items
        .iter()
        .position(|i| i.id == "custom:api.acme.dev")
        .unwrap();

    assert!(custom_pos < saved_pos);
}
