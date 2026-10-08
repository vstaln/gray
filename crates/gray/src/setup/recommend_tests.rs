use super::*;

// UNRUN (cargo test banned under X — verified via check + clippy only).

fn models(ids: &[&str]) -> Vec<(String, String)> {
    ids.iter()
        .map(|id| (id.to_string(), id.to_string()))
        .collect()
}

fn meta(
    tool_call: bool,
    text_only: bool,
    release_date: Option<&str>,
    family: Option<&str>,
) -> ModelMeta {
    ModelMeta {
        tool_call: Some(tool_call),
        text_only: Some(text_only),
        release_date: release_date.map(str::to_string),
        family: family.map(str::to_string),
    }
}

/// Meta lookup over `(id, meta)` pairs, mirroring the real cache's signature.
fn lookup<'a>(table: &'a [(&'a str, ModelMeta)]) -> impl Fn(&str) -> Option<ModelMeta> + 'a {
    move |id| table.iter().find(|(k, _)| *k == id).map(|(_, m)| m.clone())
}

#[test]
fn curated_wins_and_skips_missing_ids() {
    let m = models(&["a/one", "a/two", "a/three"]);
    let curated = vec![
        "a/ghost".to_string(),
        "a/three".to_string(),
        "a/one".to_string(),
    ];
    let got = recommended_ids(&m, Some(&curated), |_| None, 5);
    assert_eq!(got, ["a/three", "a/one"]);
}

#[test]
fn curated_with_zero_present_falls_back_to_automatic() {
    let m = models(&["a/one"]);
    let table = [("a/one", meta(true, true, Some("2025-01-01"), Some("fam")))];
    let curated = vec!["a/ghost".to_string()];
    let got = recommended_ids(&m, Some(&curated), lookup(&table), 5);
    assert_eq!(got, ["a/one"]);
}

#[test]
fn image_output_models_are_not_recommended() {
    let m = models(&["img/banana", "txt/one"]);
    let table = [
        (
            "img/banana",
            meta(true, false, Some("2025-01-01"), Some("img")),
        ),
        ("txt/one", meta(true, true, Some("2024-01-01"), Some("txt"))),
    ];
    let got = recommended_ids(&m, None, lookup(&table), 5);
    assert_eq!(got, ["txt/one"]);
}

#[test]
fn non_tool_or_unknown_meta_models_are_not_recommended() {
    let m = models(&["a/no-tools", "a/unknown", "a/good"]);
    let table = [
        (
            "a/no-tools",
            meta(false, true, Some("2026-01-01"), Some("a")),
        ),
        ("a/good", meta(true, true, Some("2025-01-01"), Some("a"))),
    ];
    // `a/unknown` has no meta entry at all.
    let got = recommended_ids(&m, None, lookup(&table), 5);
    assert_eq!(got, ["a/good"]);
}

#[test]
fn one_per_family_and_it_is_the_newest() {
    let m = models(&["a/old", "a/new", "b/only"]);
    let table = [
        ("a/old", meta(true, true, Some("2024-01-01"), Some("a"))),
        ("a/new", meta(true, true, Some("2025-06-01"), Some("a"))),
        ("b/only", meta(true, true, Some("2024-06-01"), Some("b"))),
    ];
    let got = recommended_ids(&m, None, lookup(&table), 5);
    assert_eq!(got, ["a/new", "b/only"]);
}

#[test]
fn preview_beta_batch_and_experimental_ids_are_not_recommended() {
    let m = models(&[
        "a/beta-model",
        "a/preview-x",
        "a/model-exp",
        "a/batch-api",
        "a/stable",
    ]);
    let mut table: Vec<(String, ModelMeta)> = Vec::new();
    for id in [
        "a/beta-model",
        "a/preview-x",
        "a/model-exp",
        "a/batch-api",
        "a/stable",
    ] {
        table.push((
            id.to_string(),
            meta(true, true, Some("2025-01-01"), Some("a")),
        ));
    }
    let got = recommended_ids(
        &m,
        None,
        move |id| table.iter().find(|(k, _)| k == id).map(|(_, m)| m.clone()),
        5,
    );
    assert_eq!(got, ["a/stable"]);
}

#[test]
fn missing_release_date_sorts_last_and_limit_is_respected() {
    let m = models(&["a/undated", "b/dated", "c/dated"]);
    let table = [
        ("a/undated", meta(true, true, None, Some("a"))),
        ("b/dated", meta(true, true, Some("2024-01-01"), Some("b"))),
        ("c/dated", meta(true, true, Some("2025-01-01"), Some("c"))),
    ];
    let got = recommended_ids(&m, None, lookup(&table), 2);
    assert_eq!(got, ["c/dated", "b/dated"]);
}
