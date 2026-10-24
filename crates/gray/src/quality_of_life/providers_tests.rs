use super::*;

#[test]
fn env_key_detection_picks_the_first_non_empty_candidate() {
    let env = |k: &str| match k {
        "GOOGLE_API_KEY" => Some("".to_string()),
        "GOOGLE_GENERATIVE_AI_API_KEY" => Some("gk".to_string()),
        _ => None,
    };
    assert_eq!(
        detected_env_key("google", env),
        Some(("GOOGLE_GENERATIVE_AI_API_KEY".to_string(), "gk".to_string()))
    );
    assert_eq!(detected_env_key("openai", env), None);
    assert_eq!(detected_env_key("not-a-provider", env), None);
}

#[test]
fn detect_candidates_reports_notes_and_ollama() {
    let env = |k: &str| (k == "ANTHROPIC_API_KEY").then(|| "ak".to_string());
    let found = detect_connect_candidates(true, env);
    assert_eq!(found["anthropic"].note, "(found ANTHROPIC_API_KEY)");
    assert_eq!(found["ollama"].note, "(running locally)");
    assert!(found["ollama"].env.is_none());
    assert!(!found.contains_key("openai"));
}

#[test]
fn preset_catalog_exposes_pinned_openrouter_entry() {
    let catalog = preset_catalog().expect("preset catalog");
    assert_eq!(
        catalog["openrouter"].base_url,
        "https://openrouter.ai/api/v1"
    );
    assert_eq!(catalog["kenari"].base_url, "https://kenari.id/v1");
}

#[test]
fn popular_preset_ids_match_the_pinned_list() {
    assert!(is_popular_preset("openai"));
    assert!(is_popular_preset("ollama"));
    assert!(!is_popular_preset("zzz-rest"));
}
