use super::*;

#[test]
fn unseeded_models_get_full_ladder_and_keep_requested_levels() {
    for id in [
        "muse-spark-1.3-contributor",
        "deepseek-v4",
        "some-unknown-model-xyz",
    ] {
        let levels: Vec<&str> = supported_thinking_levels(id)
            .iter()
            .map(|(l, _)| *l)
            .collect();
        assert_eq!(
            levels,
            vec!["off", "minimal", "low", "medium", "high", "xhigh", "max"],
            "{id} has no declared efforts — the full menu is offered"
        );
    }

    assert_eq!(
        clamp_thinking_level("provider/muse-spark-1.3-contributor", "max"),
        "max"
    );
    for model in [
        "muse-spark-1.3-contributor",
        "deepseek-v4-flash",
        "deepseek/deepseek-v4.1-flash",
    ] {
        assert_eq!(clamp_thinking_level(model, "xhigh"), "xhigh", "{model}");
    }
    assert_eq!(
        clamp_thinking_level("muse-spark-1.3-contributor", ""),
        "off"
    );
}

#[test]
fn merged_cache_skips_write_when_unchanged() {
    let disk: std::collections::HashMap<String, usize> =
        [("a".to_string(), 1)].into_iter().collect();
    let mem = vec![("a".to_string(), 1)];
    assert!(merged_models_cache(disk, mem).is_none());
}

#[test]
fn merged_cache_returns_map_on_new_or_changed() {
    let disk: std::collections::HashMap<String, usize> =
        [("a".to_string(), 1)].into_iter().collect();
    let out = merged_models_cache(disk, vec![("b".to_string(), 2)]).expect("new key must dirty");
    assert_eq!(out.get("b"), Some(&2));
    let disk: std::collections::HashMap<String, usize> =
        [("a".to_string(), 1)].into_iter().collect();
    let out =
        merged_models_cache(disk, vec![("a".to_string(), 9)]).expect("changed value must dirty");
    assert_eq!(out.get("a"), Some(&9));
}

#[test]
fn unseeded_models_keep_requested_level_through_clamp() {
    for model in [
        "deepseek-v4-flash",
        "deepseek-v4.1-flash",
        "deepseek/deepseek-v4.1-flash",
    ] {
        assert_eq!(clamp_thinking_level(model, "xhigh"), "xhigh", "{model}");
    }
}

fn cached_list() -> Vec<(String, String)> {
    vec![
        ("zai/glm-5.2".to_string(), "GLM 5.2".to_string()),
        ("openai/gpt-5".to_string(), "GPT 5".to_string()),
    ]
}

#[test]
fn loopback_bases_get_no_disk_entry() {
    let dir = tempfile::TempDir::new().expect("temp home");
    save_provider_model_list_at(dir.path(), "http://127.0.0.1:35757/v1", &cached_list());
    save_provider_model_list_at(dir.path(), "http://[::1]:1234/v1", &cached_list());
    save_provider_model_list_at(dir.path(), "http://localhost:8080/v1", &cached_list());
    assert!(
        !dir.path().join("provider_models.json").exists(),
        "no cache file at all for a loopback-only session"
    );

    save_provider_model_list_at(dir.path(), "https://api.x.ai/v1", &cached_list());
    assert_eq!(
        load_provider_model_list_at(dir.path(), "https://api.x.ai/v1").len(),
        2
    );
}

#[test]
fn model_list_round_trips_through_disk() {
    let dir = tempfile::TempDir::new().expect("temp home");
    save_provider_model_list_at(dir.path(), "https://api.x.ai/v1/", &cached_list());
    assert_eq!(
        load_provider_model_list_at(dir.path(), "https://api.x.ai/v1"),
        cached_list()
    );
}

#[test]
fn unknown_base_and_corrupt_file_read_as_empty() {
    let dir = tempfile::TempDir::new().expect("temp home");
    assert!(load_provider_model_list_at(dir.path(), "https://nope/v1").is_empty());
    std::fs::write(dir.path().join("provider_models.json"), b"{oops").unwrap();
    assert!(load_provider_model_list_at(dir.path(), "https://nope/v1").is_empty());
}

#[test]
fn empty_save_never_clobbers_a_cache() {
    let dir = tempfile::TempDir::new().expect("temp home");
    save_provider_model_list_at(dir.path(), "https://api.x.ai/v1", &cached_list());
    save_provider_model_list_at(dir.path(), "https://api.x.ai/v1", &[]);
    assert_eq!(
        load_provider_model_list_at(dir.path(), "https://api.x.ai/v1").len(),
        2,
        "a failed fetch must not wipe the last good list"
    );
}

#[test]
fn friendly_model_name_restores_version_dots() {
    assert_eq!(friendly_model_name("claude-opus-5-5"), "Claude Opus 5.5");
    assert_eq!(
        friendly_model_name("claude-opus-5-5-low-fast"),
        "Claude Opus 5.5 Low Fast"
    );
    assert_eq!(friendly_model_name("deepseek-v3-1"), "Deepseek v3.1");
    assert_eq!(friendly_model_name("gpt-5-2"), "GPT 5.2");
    assert_eq!(friendly_model_name("swe-2"), "SWE 2");
    assert_eq!(friendly_model_name("swe-1-6"), "SWE 1.6");

    assert_eq!(friendly_model_name("gpt-5-2025-08-07"), "GPT 5 2025 08 07");
    assert_eq!(friendly_model_name("deepseek-v3-0324"), "Deepseek v3 0324");
    assert_eq!(friendly_model_name("qwen3-30b-a3b"), "Qwen3 30b A3b");
    assert_eq!(friendly_model_name(""), "");
}

#[test]
fn loopback_hosts_are_recognized_with_and_without_brackets() {
    assert!(is_loopback_host("localhost"));
    assert!(is_loopback_host("127.0.0.1"));
    assert!(is_loopback_host("127.0.0.2"));
    assert!(is_loopback_host("[::1]"));
    assert!(is_loopback_host("::1"));
    assert!(!is_loopback_host("example.com"));
    assert!(!is_loopback_host("0.0.0.0"));
    assert!(!is_loopback_host(""));
}

#[test]
fn a_legacy_pair_cache_still_loads_and_objects_keep_their_metadata() {
    use crate::setup::variants::{CachedModel, CachedRow};
    let dir = tempfile::TempDir::new().expect("temp home");

    std::fs::write(
        dir.path().join("provider_models.json"),
        br#"{"plugin:devin-subscription":[["swe-2","SWE-2"],["gpt-6-sol","GPT-6 Sol"]]}"#,
    )
    .unwrap();
    assert_eq!(
        load_provider_model_list_at(dir.path(), "plugin:devin-subscription"),
        vec![
            ("swe-2".to_string(), "SWE-2".to_string()),
            ("gpt-6-sol".to_string(), "GPT-6 Sol".to_string()),
        ]
    );

    let rich = CachedRow::Full(CachedModel {
        id: "gpt-6-sol".into(),
        name: "GPT-6 Sol".into(),
        reasoning_efforts: vec!["off".into(), "high".into()],
        variants: vec![gray_plugin::ModelVariant {
            id: "gpt-6-sol-high-priority".into(),
            effort: Some("high".into()),
            fast: true,
            parts: Default::default(),
        }],
        slots: vec![],
        declared: true,
        price_cliff_tokens: None,
    });
    save_provider_model_rows_at(dir.path(), "plugin:devin-subscription", vec![rich]);
    save_provider_model_list_at(
        dir.path(),
        "plugin:devin-subscription",
        &[("gpt-6-sol".to_string(), "GPT 6 Sol".to_string())],
    );
    let rows = load_provider_model_rows_at(dir.path(), "plugin:devin-subscription");
    let [CachedRow::Full(m)] = rows.as_slice() else {
        panic!("one object row: {rows:?}");
    };
    assert_eq!(m.name, "GPT 6 Sol");
    assert_eq!(m.variants.len(), 1);
}

#[test]
fn key_check_request_picks_each_providers_probe() {
    let (url, style, r400) = key_check_request("https://openrouter.ai/api/v1/");
    assert_eq!(url, "https://openrouter.ai/api/v1/key");
    assert_eq!(style, AuthStyle::Bearer);
    assert!(!r400);

    let (url, style, r400) = key_check_request("https://api.anthropic.com");
    assert_eq!(url, "https://api.anthropic.com/models");
    assert_eq!(style, AuthStyle::Anthropic);
    assert!(!r400);

    let (url, style, r400) = key_check_request("https://api.anthropic.com.evil.example/v1//");
    assert_eq!(url, "https://api.anthropic.com.evil.example/v1/models");
    assert_eq!(style, AuthStyle::Bearer);
    assert!(!r400);
    let (url, style, r400) = key_check_request("http://127.0.0.1:9999/v1");
    assert_eq!(url, "http://127.0.0.1:9999/v1/models");
    assert_eq!(style, AuthStyle::Bearer);
    assert!(!r400);
}

#[test]
fn key_check_marks_400_as_rejection_only_on_measured_hosts() {
    for base in [
        "https://generativelanguage.googleapis.com/v1beta/openai",
        "https://api.x.ai/v1",
    ] {
        let (_, _, r400) = key_check_request(base);
        assert!(r400, "{base}");
        assert_eq!(classify_key_status(400, r400), KeyCheck::Rejected(400));
    }

    let (_, _, r400) = key_check_request("https://api.openai.com/v1");
    assert!(!r400);
    assert_eq!(classify_key_status(400, r400), KeyCheck::Unknown);
}

#[test]
fn key_check_status_only_an_auth_refusal_rejects() {
    assert_eq!(classify_key_status(200, false), KeyCheck::Valid);
    assert_eq!(classify_key_status(204, false), KeyCheck::Valid);
    assert_eq!(classify_key_status(401, false), KeyCheck::Rejected(401));
    assert_eq!(classify_key_status(403, false), KeyCheck::Rejected(403));

    for s in [301, 404, 418, 500, 503] {
        assert_eq!(
            classify_key_status(s, false),
            KeyCheck::Unknown,
            "status {s}"
        );
        assert_eq!(
            classify_key_status(s, true),
            KeyCheck::Unknown,
            "status {s}"
        );
    }
}

#[test]
fn model_list_candidates_cover_v1_and_bare_bases() {
    let v1 = model_list_candidates("http://localhost:1234/v1");
    assert_eq!(v1[0], "http://localhost:1234/v1/models");
    assert!(v1.iter().any(|u| u.ends_with("/tags")));
    let bare = model_list_candidates("http://localhost:8080");
    assert!(bare.iter().any(|u| u == "http://localhost:8080/v1/models"));

    assert_eq!(
        model_list_candidates("https://openrouter.ai/api/v1"),
        vec!["https://openrouter.ai/api/v1/models".to_string()]
    );
}

#[test]
fn count_models_json_reads_every_list_shape() {
    assert_eq!(count_models_json(&serde_json::json!([{"id": "a"}])), 1);
    assert_eq!(
        count_models_json(&serde_json::json!({"data": [{"id": "a"}, {"id": "b"}]})),
        2
    );
    assert_eq!(
        count_models_json(&serde_json::json!({"models": [{"name": "m"}]})),
        1
    );
    assert_eq!(count_models_json(&serde_json::json!({"nope": 1})), 0);
}

#[test]
fn provider_meta_snapshot_parses_with_all_sections() {
    let meta = embedded_provider_meta();
    assert!(
        meta.catalog.len() > 100,
        "catalog snapshot: {}",
        meta.catalog.len()
    );
    assert_eq!(
        meta.catalog.get("openrouter").map(|p| p.base_url.as_str()),
        Some("https://openrouter.ai/api/v1")
    );
    assert!(meta.popular.iter().any(|p| p.id == "openai"));
    assert!(
        meta.popular
            .iter()
            .find(|p| p.id == "ollama")
            .is_some_and(|p| p.no_auth)
    );
    assert!(
        meta.env_keys["openai"]
            .vars
            .iter()
            .any(|v| v == "OPENAI_API_KEY")
    );
    assert!(
        meta.local_endpoints
            .iter()
            .any(|e| e.url == "http://localhost:11434/v1")
    );
    assert!(
        meta.key_check
            .anthropic_hosts
            .iter()
            .any(|h| h == "api.anthropic.com")
    );
}

#[test]
fn provider_meta_disk_roundtrip_and_fallback() {
    let dir = tempfile::TempDir::new().expect("temp home");
    let path = dir.path().join("providers.json");
    assert!(
        load_provider_meta_at(&path).is_none(),
        "missing cache file reads as absent"
    );
    std::fs::write(&path, b"{oops").unwrap();
    assert!(
        load_provider_meta_at(&path).is_none(),
        "corrupt cache file reads as absent"
    );
    std::fs::write(
        &path,
        br#"{"catalog":{"edge":{"name":"Edge","base_url":"https://edge.example/v1"}},"popular":[{"id":"edge","name":"Edge","base_url":"https://edge.example/v1"}]}"#,
    )
    .unwrap();
    let meta = load_provider_meta_at(&path).expect("valid cache doc parses");
    assert_eq!(meta.catalog.len(), 1);
    assert_eq!(meta.popular[0].id, "edge");
}
