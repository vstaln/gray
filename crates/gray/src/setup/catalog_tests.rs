use super::*;

#[test]
fn custom_is_first_entry() {
    let catalog = load_catalog().expect("catalog");
    let items = build_connect_items(&catalog);
    assert_eq!(items[0].id, "custom");
    assert!(items[0].sublabel.contains("OpenAI"));
}

#[test]
fn commandcode_pinned_with_provider_base() {
    let catalog = load_catalog().expect("catalog");
    let items = build_connect_items(&catalog);
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
    // Mirror of the oauth-side clobber regression: key saves must not
    // wipe OAuth entries sharing auth.json.
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
    // And the key-only view exposes just the key.
    let keys: BTreeMap<String, String> = reloaded
        .into_iter()
        .filter_map(|(k, v)| match v {
            AuthEntry::Key(key) => Some((k, key)),
            AuthEntry::OAuth(_) => None,
        })
        .collect();
    assert_eq!(keys.get("openrouter").map(String::as_str), Some("sk-or-1"));
    assert!(!keys.contains_key("xai"));
}

// UNRUN (cargo test banned under X — verified via check + clippy only).
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

// UNRUN (cargo test banned under X — verified via check + clippy only).
#[test]
fn unknown_fields_are_ignored() {
    let dir = tempfile::tempdir().expect("tmp");
    let path = dir.path().join("config.json");
    std::fs::write(&path, r#"{"model":"m","bogus_field":123}"#).expect("write");
    let cfg = load_saved_config_at(&path);
    assert_eq!(cfg.model.as_deref(), Some("m"));
}

// UNRUN (cargo test banned under X — verified via check + clippy only).
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
    // Old files without the field load as enabled-everything.
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
    // Old files without the key load as enabled.
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
    // A removed provider must not take its neighbours down with it: keys and
    // OAuth objects share auth.json, so the delete is one map entry.
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
    // Removing what is not there is a successful no-op, not an error.
    remove_auth_entry_at(&path, "openrouter").expect("remove absent");
    assert!(load_mixed_store(&path).contains_key("commandcode"));
}

#[test]
fn subsystem_switches_default_on_and_only_explicit_false_turns_them_off() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.json");
    for at in [
        crate::setup::memory_auto_enabled_at as fn(&std::path::Path) -> bool,
        crate::setup::cron_auto_enabled_at,
        crate::setup::gw_auto_enabled_at,
    ] {
        assert!(at(&cfg), "missing config reads as enabled");
    }
    let mut saved = crate::setup::load_saved_config_at(&cfg);
    saved.memory_auto = Some(false);
    saved.cron_auto = Some(false);
    saved.gw_auto = Some(false);
    crate::setup::save_saved_config_at(&cfg, &saved).unwrap();
    assert!(!crate::setup::memory_auto_enabled_at(&cfg));
    assert!(!crate::setup::cron_auto_enabled_at(&cfg));
    assert!(!crate::setup::gw_auto_enabled_at(&cfg));
    // Round-trips through the tolerant parser (not just the struct).
    let reloaded = crate::setup::load_saved_config_at(&cfg);
    assert_eq!(reloaded.memory_auto, Some(false));
    assert_eq!(reloaded.cron_auto, Some(false));
    assert_eq!(reloaded.gw_auto, Some(false));
    // A mistyped value degrades to None (default on) instead of nuking the file.
    std::fs::write(&cfg, r#"{"memory_auto":"yes"}"#).unwrap();
    assert!(crate::setup::memory_auto_enabled_at(&cfg));
}
