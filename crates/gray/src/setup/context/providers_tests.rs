use super::*;

#[test]
fn spark_levels_exclude_max_deepseek_v4_keeps_it() {
    let spark: Vec<&str> = supported_thinking_levels("muse-spark-1.3-contributor")
        .iter()
        .map(|(l, _)| *l)
        .collect();
    assert!(
        !spark.contains(&"max"),
        "spark must not offer max: {spark:?}"
    );
    assert!(spark.contains(&"xhigh"), "spark keeps xhigh: {spark:?}");
    let v4: Vec<&str> = supported_thinking_levels("deepseek-v4")
        .iter()
        .map(|(l, _)| *l)
        .collect();
    assert!(v4.contains(&"max"), "deepseek-v4 keeps max: {v4:?}");
}

#[test]
fn clamp_max_to_xhigh_on_spark_keeps_supported_levels() {
    // The footer bug: deepseek-v4(max) -> spark must show `xhigh`.
    assert_eq!(
        clamp_thinking_level("muse-spark-1.3-contributor", "max"),
        "xhigh"
    );
    assert_eq!(
        clamp_thinking_level("provider/muse-spark-1.3-contributor", "max"),
        "xhigh"
    );
    assert_eq!(
        clamp_thinking_level("muse-spark-1.3-contributor", "high"),
        "high"
    );
    assert_eq!(
        clamp_thinking_level("muse-spark-1.3-contributor", "off"),
        "off"
    );
    assert_eq!(clamp_thinking_level("deepseek-v4", "max"), "max");
    // Off is always valid; unknown family keeps the level.
    assert_eq!(clamp_thinking_level("some-unknown-model-xyz", "max"), "max");
    assert_eq!(
        clamp_thinking_level("muse-spark-1.3-contributor", ""),
        "off"
    );
}

// UNRUN (cargo test banned under X — verified via check + clippy only).
#[test]
fn merged_cache_skips_write_when_unchanged() {
    let disk: std::collections::HashMap<String, usize> =
        [("a".to_string(), 1)].into_iter().collect();
    let mem = vec![("a".to_string(), 1)];
    assert!(merged_models_cache(disk, mem).is_none());
}

// UNRUN (cargo test banned under X — verified via check + clippy only).
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
fn deepseek_flash_clamps_carried_xhigh_to_max() {
    for model in [
        "deepseek-v4-flash",
        "deepseek-v4.1-flash",
        "deepseek/deepseek-v4.1-flash",
    ] {
        assert_eq!(clamp_thinking_level(model, "xhigh"), "max", "{model}");
    }
}

// ── provider model-list disk cache ──

fn cached_list() -> Vec<(String, String)> {
    vec![
        ("zai/glm-5.2".to_string(), "GLM 5.2".to_string()),
        ("openai/gpt-5".to_string(), "GPT 5".to_string()),
    ]
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
fn model_list_base_url_normalizes() {
    let dir = tempfile::TempDir::new().expect("temp home");
    save_provider_model_list_at(dir.path(), "https://api.x.ai/v1/", &cached_list());
    assert_eq!(
        load_provider_model_list_at(dir.path(), "https://api.x.ai/v1").len(),
        2,
        "trailing slash must not split the cache"
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
