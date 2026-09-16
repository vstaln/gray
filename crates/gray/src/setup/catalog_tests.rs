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
