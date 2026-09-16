use super::validate_direct_model_id;

fn models() -> Vec<(String, String)> {
    vec![
        ("zai/glm-5.2".to_string(), "GLM 5.2".to_string()),
        ("openai/gpt-5".to_string(), "GPT 5".to_string()),
    ]
}

#[test]
fn bogus_model_id_is_rejected_with_browse_hint() {
    let err = validate_direct_model_id("bogus-model-xyz-123", &models()).unwrap_err();
    assert!(err.contains("unknown model"), "{err}");
    assert!(err.contains("bogus-model-xyz-123"), "{err}");
    assert!(err.contains("/model"), "{err}");
}

#[test]
fn exact_id_is_accepted() {
    assert_eq!(
        validate_direct_model_id("zai/glm-5.2", &models()).unwrap(),
        "zai/glm-5.2"
    );
}

#[test]
fn input_is_trimmed() {
    assert_eq!(
        validate_direct_model_id("  zai/glm-5.2  ", &models()).unwrap(),
        "zai/glm-5.2"
    );
}

#[test]
fn case_insensitive_id_canonicalizes() {
    assert_eq!(
        validate_direct_model_id("ZAI/GLM-5.2", &models()).unwrap(),
        "zai/glm-5.2"
    );
}

#[test]
fn unique_provider_tail_resolves() {
    assert_eq!(
        validate_direct_model_id("glm-5.2", &models()).unwrap(),
        "zai/glm-5.2"
    );
}

#[test]
fn ambiguous_tail_is_rejected() {
    let dup = vec![
        ("a/same".to_string(), "A".to_string()),
        ("b/same".to_string(), "B".to_string()),
    ];
    let err = validate_direct_model_id("same", &dup).unwrap_err();
    assert!(err.contains("ambiguous"), "{err}");
    assert!(err.contains("/model"), "{err}");
}

#[test]
fn empty_known_list_fails_open_for_custom_endpoints() {
    assert_eq!(
        validate_direct_model_id("my-local-model", &[]).unwrap(),
        "my-local-model"
    );
}

#[test]
fn empty_input_is_usage_not_silent_default() {
    let err = validate_direct_model_id("   ", &models()).unwrap_err();
    assert!(err.contains("/model"), "{err}");
}
