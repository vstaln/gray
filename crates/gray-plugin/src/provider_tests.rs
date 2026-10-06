use serde_json::{Value, json};

use super::provider::ProviderDecl;
use crate::Manifest;

fn valid_provider_value() -> Value {
    json!({
        "id": "good",
        "name": "Good provider",
        "transport": {
            "kind": "openai-responses",
            "base_url": "https://example.test/v1",
            "authorization": {
                "kind": "bearer",
                "secret_name": "access_token"
            },
            "request": {
                "prompt_cache_key": false,
                "store": false,
                "include_reasoning_encrypted": true,
                "previous_response_id": false,
                "tool_choice": "auto",
                "parallel_tool_calls": true,
                "text_verbosity": "low"
            },
            "headers": [
                {"name": "X-Static", "value": "ok", "required": false},
                {
                    "name": "X-Account",
                    "source": {"kind": "metadata", "name": "account_id"},
                    "required": true
                },
                {
                    "name": "X-Session",
                    "source": {"kind": "session_id"},
                    "required": false
                }
            ]
        },
        "auth_methods": [{
            "id": "oauth",
            "name": "OAuth",
            "kind": "oauth",
            "operations": ["login", "refresh", "revoke", "models"]
        }]
    })
}

fn valid_provider_decl() -> ProviderDecl {
    ProviderDecl::from_value(&valid_provider_value()).unwrap()
}

#[test]
fn protocol_1_2_manifest_exposes_valid_provider() {
    let manifest = Manifest::from_result(&json!({
        "name": "provider-fixture",
        "version": "0.1.0",
        "protocol": "1.2",
        "capabilities": ["provider.credentials"],
        "providers": [valid_provider_value()]
    }));
    assert_eq!(manifest.providers.len(), 1);
    assert_eq!(manifest.providers[0].id, "good");
    assert!(manifest.provider_errors.is_empty());
}

#[test]
fn invalid_provider_does_not_hide_a_valid_peer() {
    let mut invalid = valid_provider_value();
    invalid["transport"]["headers"][0]["value"] = json!("bad\r\nInjected");
    let manifest = Manifest::from_result(&json!({
        "name": "provider-fixture",
        "version": "0.1.0",
        "protocol": "1.2",
        "providers": [invalid, valid_provider_value()]
    }));
    assert_eq!(manifest.providers.len(), 1);
    assert_eq!(manifest.providers[0].id, "good");
    assert_eq!(manifest.provider_errors.len(), 1);
}

#[test]
fn protocol_1_1_manifest_has_no_provider_surface() {
    let manifest = Manifest::from_result(&json!({
        "name": "legacy-fixture",
        "version": "0.1.0",
        "protocol": "1.1",
        "providers": [valid_provider_value()]
    }));
    assert!(manifest.providers.is_empty());
    assert_eq!(manifest.provider_errors.len(), 1);
}

#[test]
fn profile_binding_changes_when_request_policy_changes() {
    let a = valid_provider_decl();
    let mut b = a.clone();
    b.transport.request.parallel_tool_calls = Some(false);
    assert_ne!(
        a.profile_binding("oauth").unwrap(),
        b.profile_binding("oauth").unwrap()
    );
}

#[test]
fn duplicate_provider_ids_keep_one_provider_and_report_one_error() {
    let manifest = Manifest::from_result(&json!({
        "name": "provider-fixture",
        "version": "0.1.0",
        "protocol": "1.2",
        "providers": [valid_provider_value(), valid_provider_value()]
    }));
    assert_eq!(manifest.providers.len(), 1);
    assert_eq!(manifest.provider_errors.len(), 1);
}

#[test]
fn malformed_provider_list_is_reported_without_breaking_legacy_fields() {
    let manifest = Manifest::from_result(&json!({
        "name": "provider-fixture",
        "version": "0.1.0",
        "protocol": "1.2",
        "commands": ["/still-live"],
        "providers": {"not": "an array"}
    }));
    assert!(manifest.providers.is_empty());
    assert_eq!(manifest.provider_errors.len(), 1);
    assert_eq!(manifest.commands, vec!["/still-live"]);
}

#[test]
fn profile_binding_normalizes_default_ports_and_trailing_slashes() {
    let mut with_port = valid_provider_decl();
    with_port.transport.base_url = "https://EXAMPLE.test:443/v1/".parse().unwrap();
    let mut without_port = valid_provider_decl();
    without_port.transport.base_url = "https://example.test/v1".parse().unwrap();
    assert_eq!(
        with_port.profile_binding("oauth").unwrap(),
        without_port.profile_binding("oauth").unwrap()
    );
}

#[test]
fn provider_declaration_limit_is_checked_before_acceptance() {
    let mut value = valid_provider_value();
    value["name"] = json!("x".repeat(70 * 1024));
    assert!(ProviderDecl::from_value(&value).is_err());
}

#[test]
fn invalid_url_and_request_policy_are_rejected() {
    let mut bad_url = valid_provider_value();
    bad_url["transport"]["base_url"] = json!("http://example.test/v1");
    assert!(ProviderDecl::from_value(&bad_url).is_err());

    let mut bad_policy = valid_provider_value();
    bad_policy["transport"]["request"]["tool_choice"] = json!("bogus");
    assert!(ProviderDecl::from_value(&bad_policy).is_err());
}

#[test]
fn chat_operation_is_accepted_and_bound_to_profile() {
    let mut value = valid_provider_value();
    value["auth_methods"][0]["operations"] =
        serde_json::json!(["login", "refresh", "revoke", "models", "chat"]);
    let decl = ProviderDecl::from_value(&value).expect("chat is a valid operation");
    // The profile binding covers the ops list: adding chat changes it.
    assert_ne!(
        decl.profile_binding("oauth").unwrap(),
        valid_provider_decl().profile_binding("oauth").unwrap()
    );
}

#[test]
fn chat_request_result_round_trip() {
    use super::provider::{ProviderChatRequest, ProviderChatResult};
    let req = ProviderChatRequest {
        provider: "antigravity-subscription".into(),
        auth_method: "antigravity-login".into(),
        model: "flash".into(),
    };
    let v = serde_json::to_value(&req).unwrap();
    assert_eq!(v["model"], serde_json::json!("flash"));
    // `model` defaults empty so older sidecars stay parseable.
    let back: ProviderChatRequest =
        serde_json::from_value(serde_json::json!({"provider": "p", "auth_method": "a"})).unwrap();
    assert!(back.model.is_empty());
    let res = ProviderChatResult {
        relay_url: "http://127.0.0.1:9/relay/tok/responses".into(),
        relay_token: "tok".into(),
    };
    let v = serde_json::to_value(&res).unwrap();
    assert_eq!(
        v["relay_url"],
        serde_json::json!("http://127.0.0.1:9/relay/tok/responses")
    );
    // Debug never leaks the per-turn bearer.
    let dbg = format!("{res:?}");
    assert!(!dbg.contains("tok"), "{dbg}");
}

#[test]
fn provider_model_legacy_json_parses_without_variants_or_slots() {
    use super::provider::ProviderModel;
    let m: ProviderModel = serde_json::from_value(json!({
        "id": "swe-2",
        "name": "SWE-2",
        "reasoning_efforts": ["low", "high"]
    }))
    .unwrap();
    assert_eq!(m.id, "swe-2");
    assert!(m.context_window.is_none());
    assert!(m.variants.is_empty());
    assert!(m.slots.is_empty());
    // Empty composite fields stay off the wire so older hosts see the old shape.
    let v = serde_json::to_value(&m).unwrap();
    assert!(v.get("variants").is_none(), "{v}");
    assert!(v.get("slots").is_none(), "{v}");
}

#[test]
fn provider_model_fusion_row_round_trips() {
    use super::provider::{ModelSlot, ModelVariant, ProviderModel, SlotOption};
    use std::collections::BTreeMap;
    let parts = BTreeMap::from([
        ("lead".to_string(), "claude-opus-5-5".to_string()),
        ("sidekick".to_string(), "swe-2-high".to_string()),
    ]);
    let model = ProviderModel {
        id: "fusion".into(),
        name: "Fusion".into(),
        context_window: None,
        reasoning_efforts: vec!["high".into()],
        variants: vec![
            ModelVariant {
                id: "fusion-claude-opus-5-5-high-sidekick-swe-2-high".into(),
                effort: Some("high".into()),
                fast: false,
                parts: parts.clone(),
            },
            ModelVariant {
                id: "fusion-claude-opus-5-5-high-fast-sidekick-swe-2-high-priority".into(),
                effort: Some("high".into()),
                fast: true,
                parts,
            },
        ],
        slots: vec![
            ModelSlot {
                key: "lead".into(),
                label: "Lead".into(),
                options: vec![SlotOption {
                    id: "claude-opus-5-5".into(),
                    name: "Claude Opus 5.5".into(),
                }],
            },
            ModelSlot {
                key: "sidekick".into(),
                label: "Sidekick".into(),
                options: vec![SlotOption {
                    id: "swe-2-high".into(),
                    name: "SWE-2 High".into(),
                }],
            },
        ],
    };
    let v = serde_json::to_value(&model).unwrap();
    // `fast: false` is omitted; `fast: true` is emitted.
    assert!(v["variants"][0].get("fast").is_none(), "{v}");
    assert_eq!(v["variants"][1]["fast"], json!(true));
    assert_eq!(v["variants"][0]["parts"]["sidekick"], json!("swe-2-high"));
    let back: ProviderModel = serde_json::from_value(v).unwrap();
    assert_eq!(back.id, "fusion");
    assert_eq!(back.variants, model.variants);
    assert_eq!(back.slots, model.slots);
    assert_eq!(back.reasoning_efforts, model.reasoning_efforts);
}
