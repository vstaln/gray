use gray_core::credential::CredentialMaterial;
use gray_plugin::{
    AuthMethodDecl, ProviderAuthorizationDecl, ProviderDecl, ProviderHeaderDecl,
    ProviderTransportDecl,
};

use super::*;
use crate::setup::{ConnectAuth, build_connect_items};

fn installed_provider() -> InstalledProvider {
    let provider = ProviderDecl {
        id: "codex".into(),
        name: "Codex".into(),
        transport: ProviderTransportDecl {
            kind: "openai-responses".into(),
            base_url: "https://chatgpt.com/backend-api/codex".parse().unwrap(),
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
            id: "chatgpt-subscription".into(),
            name: "ChatGPT".into(),
            kind: "oauth".into(),
            operations: vec!["refresh".into()],
        }],
    };
    InstalledProvider {
        plugin: "codex-auth".into(),
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
        rows.iter()
            .any(|row| row.id == "codex-auth:codex"
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
        provider_id: String::new(),
        credential_source: String::new(),
        auth_ref: String::new(),
    };
    activate_for_test(&mut config, &installed, "gpt-test", &path).unwrap();
    let body = std::fs::read_to_string(&path).unwrap();
    assert_eq!(config.api_key, None);
    assert_eq!(config.credential_source, "plugin");
    assert_eq!(config.provider_id, "codex-auth:codex");
    assert_eq!(
        config.auth_ref,
        "plugin:codex-auth:codex:chatgpt-subscription"
    );
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
            "codex-auth",
            "codex",
            "chatgpt-subscription",
            installed.profile_binding.clone(),
            CredentialMaterial::empty(),
        )
        .unwrap(),
    };
    assert_eq!(request.profile_binding, installed.profile_binding.clone());
}
