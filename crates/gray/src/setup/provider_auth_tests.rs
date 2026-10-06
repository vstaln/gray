use gray_core::credential::CredentialMaterial;
use gray_plugin::{
    AuthMethodDecl, ProviderAuthorizationDecl, ProviderDecl, ProviderHeaderDecl,
    ProviderTransportDecl,
};

use super::*;
use crate::setup::{ConnectAuth, build_connect_items};

fn installed_provider() -> InstalledProvider {
    let provider = ProviderDecl {
        id: "example".into(),
        name: "Example".into(),
        transport: ProviderTransportDecl {
            kind: "openai-responses".into(),
            base_url: "https://example.test/v1".parse().unwrap(),
            authorization: ProviderAuthorizationDecl {
                kind: "bearer".into(),
                secret_name: "access_token".into(),
            },
            request: gray_plugin::ProviderRequestPolicyDecl {
                prompt_cache_key: false,
                store: false,
                include_reasoning_encrypted: true,
                previous_response_id: false,
                tool_choice: Some("auto".into()),
                parallel_tool_calls: Some(true),
                text_verbosity: Some("low".into()),
            },
            headers: vec![ProviderHeaderDecl {
                name: "originator".into(),
                value: Some("gray".into()),
                source: None,
                required: false,
            }],
        },
        auth_methods: vec![AuthMethodDecl {
            id: "example-login".into(),
            name: "Example login".into(),
            kind: "oauth".into(),
            operations: vec!["refresh".into()],
        }],
    };
    InstalledProvider {
        plugin: "example-sub".into(),
        auth_method: provider.auth_methods[0].clone(),
        provider,
        profile_binding: "sha256:test".into(),
        argv: Vec::new(),
    }
}

#[test]
fn connect_rows_include_each_plugin_auth_method() {
    let catalog = crate::setup::catalog::Catalog::default();
    let providers = vec![installed_provider()];
    let rows = build_connect_items(&catalog, &providers);
    assert!(
        rows.iter().any(|row| row.id == "example-sub:example"
            && matches!(&row.auth, ConnectAuth::Plugin { .. }))
    );
    assert!(
        rows.iter()
            .any(|row| row.id == "openai" && row.auth == ConnectAuth::ApiKey)
    );
}

#[test]
fn selecting_plugin_clears_api_key_and_writes_only_references() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    let installed = installed_provider();
    let mut config = Config {
        fast_mode: None,
        model_parts: Default::default(),
        model: None,
        base_url: "https://api.openai.com/v1".into(),
        api_key: Some("test-openai-key".into()),
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
        provider_id: String::new(),
        credential_source: String::new(),
        auth_ref: String::new(),
    };
    activate_for_test(&mut config, &installed, "gpt-test", &path).unwrap();
    let body = std::fs::read_to_string(&path).unwrap();
    assert_eq!(config.api_key, None);
    assert_eq!(config.credential_source, "plugin");
    assert_eq!(config.provider_id, "example-sub:example");
    assert_eq!(config.auth_ref, "plugin:example-sub:example:example-login");
    assert!(!body.contains("test-openai-key"));
    assert!(body.contains("\"credential_source\": \"plugin\""));
}

#[test]
fn plugin_models_request_uses_host_identity() {
    let installed = installed_provider();
    let request = ProviderModelsRequest {
        provider: installed.provider.id.clone(),
        auth_method: installed.auth_method.id.clone(),
        profile_binding: installed.profile_binding.clone(),
        credential: CredentialEnvelope::new(
            "example-sub",
            "example",
            "example-login",
            installed.profile_binding.clone(),
            CredentialMaterial::empty(),
        )
        .unwrap(),
    };
    assert_eq!(request.profile_binding, installed.profile_binding.clone());
}

fn key_config(base_url: &str, key: Option<&str>) -> Config {
    Config {
        fast_mode: None,
        model_parts: Default::default(),
        model: Some("meta/muse-spark-1.3-contributor".into()),
        base_url: base_url.into(),
        api_key: key.map(str::to_string),
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
        provider_id: String::new(),
        credential_source: String::new(),
        auth_ref: String::new(),
    }
}

#[test]
fn a_dismissed_connect_leaves_the_session_as_it_was() {
    let before = key_config("https://api.commandcode.ai/provider/v1", Some("sk-live"));
    for outcome in [
        Ok(crate::setup::ConnectOutcome::Dismissed),
        Err(anyhow!("modal failed")),
    ] {
        // Picking a row wrote its endpoint and key before the model step.
        let mut config = key_config("https://openrouter.ai/api/v1", Some("sk-picked"));
        settle_connect_config(&mut config, before.clone(), &outcome);
        assert!(config == before);
    }
    let mut config = key_config("https://openrouter.ai/api/v1", Some("sk-picked"));
    settle_connect_config(
        &mut config,
        before,
        &Ok(crate::setup::ConnectOutcome::Connected),
    );
    assert_eq!(config.api_key.as_deref(), Some("sk-picked"));
}

#[test]
fn only_a_changed_key_for_the_same_endpoint_is_adopted() {
    let url = "https://api.commandcode.ai/provider/v1";
    let config = key_config(url, Some("sk-old"));
    let saved = |base: &str, key: &str| SavedConfig {
        base_url: Some(base.into()),
        api_key: Some(key.into()),
        ..SavedConfig::default()
    };
    assert_eq!(
        saved_key_update(&config, &saved(url, "sk-new")).as_deref(),
        Some("sk-new")
    );
    assert_eq!(
        saved_key_update(&config, &saved(&format!("{url}/"), "sk-new")).as_deref(),
        Some("sk-new"),
        "a trailing slash is the same endpoint"
    );
    assert_eq!(saved_key_update(&config, &saved(url, "sk-old")), None);
    assert_eq!(saved_key_update(&config, &saved(url, "  ")), None);
    // Another window switched provider: that is not this session's key.
    assert_eq!(
        saved_key_update(&config, &saved("https://openrouter.ai/api/v1", "sk-or")),
        None
    );
    let plugin = SavedConfig {
        credential_source: "plugin".into(),
        ..saved(url, "sk-new")
    };
    assert_eq!(saved_key_update(&config, &plugin), None);
}
