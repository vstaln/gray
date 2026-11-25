use serde_json::{Value, json};

pub const RELAY_SECRET: &str = "relay_token";
pub const PLACEHOLDER_BASE: &str = "https://127.0.0.1:1/";

pub fn provider_decl(
    provider_id: &str,
    display_name: &str,
    auth_method_id: &str,
    auth_method_name: &str,
    auth_kind: &str,
    operations: &[&str],
    cache_ttl_secs: u64,
) -> Value {
    json!({
        "id": provider_id,
        "name": display_name,
        "transport": {
            "kind": "openai-responses",
            "base_url": PLACEHOLDER_BASE,
            "authorization": {"kind": "bearer", "secret_name": RELAY_SECRET},
            "request": {
                "prompt_cache_key": false,
                "store": false,
                "include_reasoning_encrypted": true,
                "previous_response_id": false,
                "tool_choice": "auto",
                "parallel_tool_calls": true,
                "text_verbosity": "low",
                "cache_ttl_secs": cache_ttl_secs,
            },
            "headers": [
                {"name": "session-id", "required": true,
                 "source": {"kind": "session_id"}},
            ],
        },
        "auth_methods": [{
            "id": auth_method_id,
            "name": auth_method_name,
            "kind": auth_kind,
            "operations": operations,
        }],
    })
}

pub fn provider_decl_http(
    provider_id: &str,
    display_name: &str,
    auth_method_id: &str,
    auth_method_name: &str,
    auth_kind: &str,
    operations: &[&str],
    cache_ttl_secs: u64,
) -> Value {
    let mut decl = provider_decl(
        provider_id,
        display_name,
        auth_method_id,
        auth_method_name,
        auth_kind,
        operations,
        cache_ttl_secs,
    );
    decl["transport"]["request"]["warm_replay"] = Value::Bool(true);
    decl
}
