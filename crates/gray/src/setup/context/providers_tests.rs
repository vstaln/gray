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
