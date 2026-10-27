use super::validate_direct_model_id;

fn models() -> Vec<(String, String)> {
    vec![
        ("zai/glm-5.2".to_string(), "GLM 5.2".to_string()),
        ("openai/gpt-5".to_string(), "GPT 5".to_string()),
    ]
}

#[test]
fn direct_model_id_normalizes_and_resolves() {
    assert_eq!(
        validate_direct_model_id("zai/glm-5.2", &models()).unwrap(),
        "zai/glm-5.2"
    );
    assert_eq!(
        validate_direct_model_id("  zai/glm-5.2  ", &models()).unwrap(),
        "zai/glm-5.2",
        "input is trimmed"
    );
    assert_eq!(
        validate_direct_model_id("ZAI/GLM-5.2", &models()).unwrap(),
        "zai/glm-5.2",
        "case-insensitive ids canonicalize"
    );
    assert_eq!(
        validate_direct_model_id("glm-5.2", &models()).unwrap(),
        "zai/glm-5.2",
        "unique provider tail resolves"
    );
    assert_eq!(
        validate_direct_model_id("my-local-model", &[]).unwrap(),
        "my-local-model",
        "empty known list fails open for custom endpoints"
    );
}

#[test]
fn direct_model_id_rejects_with_a_browse_hint() {
    let err = validate_direct_model_id("bogus-model-xyz-123", &models()).unwrap_err();
    assert!(err.contains("unknown model"), "{err}");
    assert!(err.contains("bogus-model-xyz-123"), "{err}");
    assert!(err.contains("/model"), "{err}");

    let dup = vec![
        ("a/same".to_string(), "A".to_string()),
        ("b/same".to_string(), "B".to_string()),
    ];
    let err = validate_direct_model_id("same", &dup).unwrap_err();
    assert!(err.contains("ambiguous"), "{err}");
    assert!(err.contains("/model"), "{err}");

    let err = validate_direct_model_id("   ", &models()).unwrap_err();
    assert!(
        err.contains("/model"),
        "empty input is usage, not a silent default: {err}"
    );
}

fn mock_models_server(body: &'static str) -> u16 {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming().take(4) {
            let Ok(mut s) = stream else { continue };
            let mut buf = [0u8; 2048];
            let _ = s.read(&mut buf);
            let _ = s.write_all(
                format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                )
                .as_bytes(),
            );
        }
    });
    port
}

#[test]
fn the_live_fetch_works_from_a_thread_with_no_runtime() {
    let body = r#"{"data":[{"id":"m-one","name":"Model One"},{"id":"m-two","name":"Model Two"}]}"#;
    let port = mock_models_server(body);
    let base = format!("http://127.0.0.1:{port}/v1");
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let got = super::super::context::fetch_live_provider_models(&base, None);
        let _ = tx.send(got);
    });
    let got = rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("background fetch answered");
    let ids: Vec<&str> = got.iter().map(|(id, _)| id.as_str()).collect();
    assert!(ids.contains(&"m-one"), "{got:?}");
    assert!(ids.contains(&"m-two"), "{got:?}");
}

#[test]
fn saved_models_need_no_network() {
    let _ = super::saved_models_for("http://127.0.0.1:1/v1", "http://127.0.0.1:1/v1");
}

#[test]
fn a_live_list_merges_into_the_saved_ordering() {
    let live = vec![
        ("zeta/1".to_string(), "Zeta".to_string()),
        ("alpha/1".to_string(), "Alpha".to_string()),
    ];
    let merged = super::merge_models("http://127.0.0.1:1/v1", live);
    assert_eq!(merged.len(), 2, "every live model is offered");
    let ids: Vec<&str> = merged.iter().map(|(id, _)| id.as_str()).collect();
    assert!(ids.contains(&"zeta/1") && ids.contains(&"alpha/1"));
}

fn sorted_models() -> Vec<(String, String)> {
    vec![
        ("cur/m".to_string(), "Current".to_string()),
        ("a/1".to_string(), "A".to_string()),
        ("b/2".to_string(), "B".to_string()),
        ("c/3".to_string(), "C".to_string()),
    ]
}

#[test]
fn recent_prefix_counts_the_sorted_head() {
    let models = sorted_models();
    let n = super::recent_prefix_len(
        Some("cur/m"),
        &["a/1".to_string(), "b/2".to_string()],
        models.iter().map(|(id, _)| id.as_str()),
    );
    assert_eq!(n, 3, "current + two recents park first");
}

#[test]
fn recent_prefix_stops_at_the_first_unknown() {
    let shuffled = [
        ("a/1".to_string(), "A".to_string()),
        ("c/3".to_string(), "C".to_string()),
        ("b/2".to_string(), "B".to_string()),
    ];
    let n = super::recent_prefix_len(
        None,
        &["a/1".to_string(), "b/2".to_string()],
        shuffled.iter().map(|(id, _)| id.as_str()),
    );
    assert_eq!(n, 1, "an unknown id ends the recent section");
}

#[test]
fn head_first_lifts_the_head_and_keeps_the_tail_order() {
    let models = sorted_models();
    let refs: Vec<&(String, String)> = models.iter().collect();
    let head = vec!["c/3".to_string(), "a/1".to_string()];
    let out = super::head_first(&refs, &head);
    let ids: Vec<&str> = out.iter().map(|(id, _)| id.as_str()).collect();
    assert_eq!(ids, ["c/3", "a/1", "cur/m", "b/2"]);
}

#[test]
fn head_first_skips_ids_that_are_not_listed() {
    let models = sorted_models();
    let refs: Vec<&(String, String)> = models.iter().collect();
    let head = vec!["ghost/x".to_string(), "b/2".to_string()];
    let out = super::head_first(&refs, &head);
    let ids: Vec<&str> = out.iter().map(|(id, _)| id.as_str()).collect();
    assert_eq!(ids, ["b/2", "cur/m", "a/1", "c/3"]);
}

#[test]
fn selection_never_rest_on_the_divider() {
    use super::Row;
    let rows = vec![Row::Model(0), Row::Divider, Row::Model(1)];
    assert_eq!(super::skip_divider(&rows, 1, true), 2);
    assert_eq!(super::skip_divider(&rows, 1, false), 0);
    assert_eq!(
        super::skip_divider(&rows, 0, true),
        0,
        "models pass through"
    );
    assert_eq!(
        super::skip_divider(&rows, 2, false),
        2,
        "models pass through"
    );
}

use super::split_effort_variant;

#[test]
fn collapsed_variant_splits_to_base_and_tier() {
    let rows = vec![
        ("swe-2".to_string(), "SWE-2".to_string()),
        ("swe-1-7".to_string(), "SWE-1.7".to_string()),
    ];
    assert_eq!(
        split_effort_variant("swe-2-max", &rows),
        Some(("swe-2".to_string(), "max".to_string()))
    );
    assert_eq!(
        split_effort_variant("swe-2-medium", &rows),
        Some(("swe-2".to_string(), "medium".to_string()))
    );
}

#[test]
fn declared_row_never_splits() {
    let rows = vec![
        ("qwen3".to_string(), "Qwen 3".to_string()),
        ("qwen3-max".to_string(), "Qwen 3 Max".to_string()),
    ];
    assert_eq!(split_effort_variant("qwen3-max", &rows), None);
}

#[test]
fn unknown_bases_and_non_tier_suffixes_stay_put() {
    let rows = vec![("swe-2".to_string(), "SWE-2".to_string())];
    assert_eq!(split_effort_variant("swe-9-max", &rows), None);
    assert_eq!(split_effort_variant("swe-2-fast", &rows), None);
    assert_eq!(split_effort_variant("swe-2", &rows), None);
    assert_eq!(split_effort_variant("swe-2-max", &[]), None);
}

use super::{compose_fast_model, decompose_model_variant};

#[test]
fn fast_variant_decomposes_to_base_tier_fast() {
    let rows = vec![
        ("claude-opus-5-5".to_string(), "Opus 5.5".to_string()),
        (
            "claude-opus-5-5-high-fast".to_string(),
            "Opus 5.5 High Fast".to_string(),
        ),
        ("gpt-6-sol".to_string(), "GPT-6 Sol".to_string()),
        (
            "gpt-6-sol-high-priority".to_string(),
            "GPT-6 Sol Thinking Fast".to_string(),
        ),
    ];
    assert_eq!(
        decompose_model_variant("claude-opus-5-5-high-fast", &rows),
        Some((
            "claude-opus-5-5".to_string(),
            Some("high".to_string()),
            true
        ))
    );
    assert_eq!(
        decompose_model_variant("gpt-6-sol-high-priority", &rows),
        Some(("gpt-6-sol".to_string(), Some("high".to_string()), true))
    );
}

#[test]
fn fast_variant_without_tier_and_tier_without_fast() {
    let rows = vec![
        ("swe-1-6".to_string(), "SWE-1.6".to_string()),
        ("swe-1-6-fast".to_string(), "SWE-1.6 Fast".to_string()),
        ("swe-2".to_string(), "SWE-2".to_string()),
    ];

    assert_eq!(
        decompose_model_variant("swe-1-6-fast", &rows),
        Some(("swe-1-6".to_string(), None, true))
    );

    assert_eq!(
        decompose_model_variant("swe-2-max", &rows),
        Some(("swe-2".to_string(), Some("max".to_string()), false))
    );

    assert_eq!(decompose_model_variant("swe-2", &rows), None);
    assert_eq!(decompose_model_variant("swe-9-max-fast", &rows), None);
}

#[test]
fn fast_suffix_case_insensitive_and_unresolvable_stem() {
    let rows = vec![("glm-5-2".to_string(), "GLM 5.2".to_string())];
    assert_eq!(
        decompose_model_variant("GLM-5-2-Fast", &rows),
        Some(("glm-5-2".to_string(), None, true))
    );

    let rows = vec![("some-fast-model".to_string(), "X".to_string())];
    assert_eq!(decompose_model_variant("some-fast-model", &rows), None);
}

#[test]
fn compose_fast_model_picks_catalog_spelling() {
    let rows = vec![
        ("claude-opus-5-5".to_string(), "Opus 5.5".to_string()),
        (
            "claude-opus-5-5-high-fast".to_string(),
            "Opus 5.5 High Fast".to_string(),
        ),
        (
            "claude-opus-5-5-medium-fast".to_string(),
            "Opus 5.5 Medium Fast".to_string(),
        ),
        ("gpt-6-sol".to_string(), "GPT-6 Sol".to_string()),
        (
            "gpt-6-sol-none-priority".to_string(),
            "GPT-6 Sol Instant Fast".to_string(),
        ),
        (
            "gpt-6-sol-high-priority".to_string(),
            "GPT-6 Sol Thinking Fast".to_string(),
        ),
        ("swe-1-6".to_string(), "SWE-1.6".to_string()),
        ("swe-1-6-fast".to_string(), "SWE-1.6 Fast".to_string()),
        ("swe-2".to_string(), "SWE-2".to_string()),
    ];
    assert_eq!(
        compose_fast_model("claude-opus-5-5", Some("high"), &rows).as_deref(),
        Some("claude-opus-5-5-high-fast")
    );
    assert_eq!(
        compose_fast_model("claude-opus-5-5", Some("medium"), &rows).as_deref(),
        Some("claude-opus-5-5-medium-fast")
    );

    assert_eq!(
        compose_fast_model("gpt-6-sol", Some("off"), &rows).as_deref(),
        Some("gpt-6-sol-none-priority")
    );
    assert_eq!(
        compose_fast_model("gpt-6-sol", Some("high"), &rows).as_deref(),
        Some("gpt-6-sol-high-priority")
    );

    assert_eq!(
        compose_fast_model("swe-1-6", Some("high"), &rows).as_deref(),
        Some("swe-1-6-fast")
    );

    assert_eq!(compose_fast_model("swe-2", Some("high"), &rows), None);
    assert_eq!(compose_fast_model("swe-2", Some("max"), &rows), None);

    assert_eq!(
        compose_fast_model("swe-1-6-fast", Some("high"), &rows),
        None
    );
}

use super::{RowEfforts, commit_row_effort, initial_row_effort};
use crate::config::Config;
use crate::setup::{SavedConfig, effort_memory_key};

const PICKER_BASE: &str = "http://127.0.0.1:1/v1";

fn effort_config(model: &str, effort: Option<&str>) -> Config {
    Config {
        fast_mode: None,
        model_parts: Default::default(),
        model: Some(model.into()),
        base_url: PICKER_BASE.into(),
        api_key: None,
        thinking_effort: effort.map(str::to_string),
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
        lean: false,
        compact_at: None,
        provider_id: String::new(),
        credential_source: String::new(),
        auth_ref: String::new(),
    }
}

const STEP_MODEL: &str = "step-3";

#[test]
fn arrows_step_within_supported_levels_and_clamp_at_the_ends() {
    let config = effort_config("other/live-model", Some("low"));
    let saved = SavedConfig::default();
    let mut efforts = RowEfforts::default();

    assert_eq!(efforts.shown(&config, &saved, STEP_MODEL), Some("high"));
    assert!(!efforts.was_stepped(STEP_MODEL));
    efforts.step(&config, &saved, STEP_MODEL, true);
    assert_eq!(efforts.shown(&config, &saved, STEP_MODEL), Some("xhigh"));
    efforts.step(&config, &saved, STEP_MODEL, true);
    assert_eq!(efforts.shown(&config, &saved, STEP_MODEL), Some("max"));
    efforts.step(&config, &saved, STEP_MODEL, true);
    assert_eq!(
        efforts.shown(&config, &saved, STEP_MODEL),
        Some("max"),
        "→ clamps at the top, no wrap"
    );
    for _ in 0..8 {
        efforts.step(&config, &saved, STEP_MODEL, false);
    }
    assert_eq!(
        efforts.shown(&config, &saved, STEP_MODEL),
        Some("off"),
        "← clamps at the bottom"
    );
    assert!(efforts.was_stepped(STEP_MODEL));

    assert!(!efforts.was_stepped("other/live-model"));
}

#[test]
fn a_model_without_reasoning_shows_no_effort_and_ignores_arrows() {
    let plain = "picker-test/plain-no-reasoning";
    crate::setup::cache_model_reasoning(plain, false);
    let config = effort_config(plain, Some("high"));
    let saved = SavedConfig::default();
    let mut efforts = RowEfforts::default();
    assert_eq!(efforts.shown(&config, &saved, plain), None);
    efforts.step(&config, &saved, plain, true);
    efforts.step(&config, &saved, plain, false);
    assert_eq!(efforts.shown(&config, &saved, plain), None);
    assert!(!efforts.was_stepped(plain));
}

#[test]
fn rows_open_at_the_remembered_effort_and_the_live_level() {
    let mut saved = SavedConfig::default();
    saved.remember_effort(&effort_memory_key("", PICKER_BASE, STEP_MODEL), "medium");

    let config = effort_config("other/live-model", Some("max"));
    assert_eq!(
        initial_row_effort(&config, &saved, STEP_MODEL),
        Some("medium")
    );

    let config = effort_config(STEP_MODEL, Some("low"));
    assert_eq!(initial_row_effort(&config, &saved, STEP_MODEL), Some("low"));

    let mut saved = SavedConfig::default();
    saved.remember_effort(&effort_memory_key("", PICKER_BASE, STEP_MODEL), "xhigh");
    let config = effort_config("other/live-model", None);
    assert_eq!(
        initial_row_effort(&config, &saved, STEP_MODEL),
        Some("xhigh")
    );
}

#[test]
fn enter_commits_model_and_row_effort_together() {
    let mut config = effort_config(STEP_MODEL, Some("high"));
    let mut saved = SavedConfig::default();
    commit_row_effort(&mut config, &mut saved, Some("low"), true);
    assert_eq!(config.thinking_effort.as_deref(), Some("low"));
    assert_eq!(
        saved
            .remembered_effort(&effort_memory_key("", PICKER_BASE, STEP_MODEL))
            .as_deref(),
        Some("low"),
        "the effort persists per model"
    );
    assert_eq!(
        saved.thinking_effort.as_deref(),
        Some("low"),
        "the flat saved level syncs with it"
    );
}

#[test]
fn enter_on_a_row_without_effort_restores_the_remembered_level() {
    let plain = "picker-test/plain-commit";
    crate::setup::cache_model_reasoning(plain, false);
    let mut config = effort_config(plain, Some("max"));
    let mut saved = SavedConfig::default();
    saved.remember_effort(&effort_memory_key("", PICKER_BASE, plain), "off");
    saved.thinking_effort = Some("max".into());
    commit_row_effort(&mut config, &mut saved, None, false);
    assert_eq!(config.thinking_effort.as_deref(), Some("off"));
    assert_eq!(saved.thinking_effort.as_deref(), Some("off"));
}

#[test]
fn fit_chars_marks_a_cut() {
    assert_eq!(super::fit_chars("Claude Opus", 20), "Claude Opus");
    assert_eq!(super::fit_chars("Claude Opus", 7), "Claude…");
    assert_eq!(super::fit_chars("abc", 0), "");
}

use super::{
    METER_W, PickerKey, PickerToggles, effort_label, list_window, meter_fill, per_million,
    picker_key, price_level, rate_text,
};

#[test]
fn the_meter_fills_by_position_among_the_models_own_levels() {
    let step = ["low", "medium", "high", "max"];
    let fills: Vec<usize> = step.iter().map(|l| meter_fill(&step, l, METER_W)).collect();
    assert_eq!(
        fills,
        vec![2, 3, 5, 6],
        "every step moves the bar; top is full"
    );
    let full = ["off", "minimal", "low", "medium", "high", "xhigh", "max"];
    let fills: Vec<usize> = full.iter().map(|l| meter_fill(&full, l, METER_W)).collect();
    assert_eq!(fills, vec![0, 1, 2, 3, 4, 5, 6], "`off` draws empty");
    assert_eq!(
        meter_fill(&step, "xhigh", METER_W),
        0,
        "a level the model lacks draws empty"
    );
    assert_eq!(meter_fill(&["off", "high"], "high", METER_W), METER_W);
}

#[test]
fn effort_labels_read_as_words() {
    assert_eq!(effort_label("high"), "High");
    assert_eq!(effort_label("xhigh"), "XHigh");
    assert_eq!(effort_label("minimal"), "Minimal");
    assert_eq!(effort_label("off"), "Off");
    assert!(
        ["off", "minimal", "low", "medium", "high", "xhigh", "max"]
            .iter()
            .all(|l| effort_label(l).chars().count() <= 7),
        "the label column is 7 cells"
    );
}

#[test]
fn the_list_window_keeps_the_selection_in_view_and_hints_hidden_rows() {
    assert_eq!(list_window(5, 4, 0, 8), (0, 5));

    assert_eq!(list_window(20, 0, 0, 8), (0, 7));

    let (top, shown) = list_window(20, 10, 0, 8);
    assert!(top > 0 && top <= 10 && 10 < top + shown, "{top} {shown}");
    assert_eq!(shown, 6, "a hint above and below");

    assert_eq!(list_window(20, 19, 0, 8), (13, 7));

    assert_eq!(list_window(20, 19, 18, 8), (13, 7));

    assert_eq!(list_window(20, 19, 0, 2), (18, 2));
}

#[test]
fn prices_read_per_million_tokens() {
    assert_eq!(per_million(3e-6), "$3");
    assert_eq!(per_million(3e-7), "$0.3");
    assert_eq!(per_million(1.25e-6), "$1.25");
    assert_eq!(per_million(7.5e-8), "$0.075");
    assert_eq!(per_million(0.0), "$0");
}

#[test]
fn the_price_level_is_absolute_bands_on_blended_per_million() {
    assert_eq!(price_level(0.0), 1, "free");
    assert_eq!(price_level(0.20), 1, "sub-$0.50");
    assert_eq!(price_level(0.60), 2);
    assert_eq!(price_level(5.80), 3);
    assert_eq!(price_level(18.0), 4, "Kimi K3 territory");
    assert_eq!(price_level(30.0), 5, "frontier");
    assert_eq!(price_level(60.0), 5, "priciest known");

    assert_eq!(price_level(0.5), 1);
    assert_eq!(price_level(20.0), 4);
}

#[test]
fn the_rate_line_folds_to_the_room_it_gets() {
    use super::super::context::ModelRate;
    let r = ModelRate {
        input: 2e-6,
        output: 1e-5,
        cache_read: 2e-7,
        has_cache_prices: true,
        ..ModelRate::default()
    };
    assert_eq!(rate_text(&r, 80), "in $2 · cached $0.2 · out $10 / 1M");
    assert_eq!(rate_text(&r, 24), "in $2 · out $10 / 1M", "cached drops");
    assert_eq!(rate_text(&r, 17), "in $2 · out $10", "then the suffix");
    assert_eq!(rate_text(&r, 8), "in $2 ·…", "then it truncates");
    let plain = ModelRate {
        input: 2e-6,
        output: 1e-5,
        ..ModelRate::default()
    };
    assert_eq!(
        rate_text(&plain, 80),
        "in $2 · out $10 / 1M",
        "no cache price"
    );
}

#[test]
fn picker_keys_map_tab_to_fast_and_ctrl_r_to_reasoning() {
    use crossterm::event::{KeyCode, KeyModifiers};
    let none = KeyModifiers::NONE;
    let ctrl = KeyModifiers::CONTROL;
    assert_eq!(picker_key(KeyCode::Tab, none), Some(PickerKey::ToggleFast));
    assert_eq!(
        picker_key(KeyCode::Char('r'), ctrl),
        Some(PickerKey::ToggleReasoning)
    );
    assert_eq!(
        picker_key(KeyCode::Char('r'), none),
        Some(PickerKey::Type('r'))
    );
    assert_eq!(picker_key(KeyCode::Left, none), Some(PickerKey::EffortDown));
    assert_eq!(picker_key(KeyCode::Right, none), Some(PickerKey::EffortUp));
    assert_eq!(
        picker_key(KeyCode::Char('c'), ctrl),
        Some(PickerKey::Cancel)
    );
    assert_eq!(picker_key(KeyCode::Esc, none), Some(PickerKey::Cancel));
    assert_eq!(picker_key(KeyCode::Char('x'), ctrl), None);
}

#[test]
fn toggles_stay_pending_and_commit_only_what_changed() {
    let mut config = effort_config(STEP_MODEL, Some("high"));
    config.fast_mode = Some(false);
    let mut toggles = PickerToggles::from_config(&config);
    toggles.toggle_fast(false);
    assert!(!toggles.fast, "no fast variant: tab does nothing");
    toggles.toggle_fast(true);
    toggles.toggle_reasoning();
    assert_eq!(config.fast_mode, Some(false), "nothing lands before Enter");
    let mut saved = SavedConfig::default();
    toggles.commit(&mut config, &mut saved);
    assert_eq!(config.fast_mode, Some(true));
    assert_eq!(saved.fast_mode, Some(true));
    assert_eq!(config.show_reasoning, Some(false));
    assert_eq!(saved.show_reasoning, Some(false));

    let mut toggles = PickerToggles::from_config(&config);
    toggles.toggle_reasoning();
    toggles.toggle_reasoning();
    let mut saved = SavedConfig::default();
    toggles.commit(&mut config, &mut saved);
    assert_eq!(saved.show_reasoning, None);
    assert_eq!(saved.fast_mode, None);
}

use super::{DropState, PickLevel, enter_level, esc_level, open_option, option_parts, step_option};
use crate::setup::variants::{Parts, RowShape, cache_row_shape, row_shape};

#[test]
fn enter_walks_a_composites_slots_and_esc_backs_out() {
    assert_eq!(enter_level(PickLevel::List, 0), (PickLevel::List, true));
    assert_eq!(esc_level(PickLevel::List), None);

    assert_eq!(enter_level(PickLevel::List, 2), (PickLevel::Slot(0), false));
    assert_eq!(
        enter_level(PickLevel::Slot(0), 2),
        (PickLevel::Slot(1), false)
    );
    assert_eq!(enter_level(PickLevel::Slot(1), 2), (PickLevel::List, true));

    assert_eq!(esc_level(PickLevel::Slot(1)), Some(PickLevel::Slot(0)));
    assert_eq!(esc_level(PickLevel::Slot(0)), Some(PickLevel::List));
}

fn lead_sidekick(lead: &str, sidekick: &str) -> Parts {
    Parts::from([
        ("lead".to_string(), lead.to_string()),
        ("sidekick".to_string(), sidekick.to_string()),
    ])
}

fn fusion_shape() -> RowShape {
    let opt = |id: &str| gray_plugin::SlotOption {
        id: id.into(),
        name: id.to_uppercase(),
    };
    let var = |id: &str, effort: &str, fast: bool, p: Parts| gray_plugin::ModelVariant {
        id: id.into(),
        effort: Some(effort.into()),
        fast,
        parts: p,
    };
    RowShape {
        variants: vec![
            var("f-opus-a-high", "high", false, lead_sidekick("opus", "a")),
            var("f-opus-a-max", "max", false, lead_sidekick("opus", "a")),
            var(
                "f-opus-a-high-fast",
                "high",
                true,
                lead_sidekick("opus", "a"),
            ),
            var("f-opus-b-high", "high", false, lead_sidekick("opus", "b")),
            var("f-sonnet-c-low", "low", false, lead_sidekick("sonnet", "c")),
        ],
        slots: vec![
            gray_plugin::ModelSlot {
                key: "lead".into(),
                label: "Lead".into(),
                options: vec![opt("opus"), opt("sonnet")],
            },
            gray_plugin::ModelSlot {
                key: "sidekick".into(),
                label: "Sidekick".into(),
                options: vec![opt("a"), opt("b"), opt("c")],
            },
        ],
    }
}

#[test]
fn sidekicks_without_a_variant_for_the_lead_are_unservable_and_skipped() {
    let shape = fusion_shape();
    let cur = lead_sidekick("opus", "a");
    assert_eq!(
        option_parts(&shape, &cur, 1, "b"),
        Some(lead_sidekick("opus", "b"))
    );
    assert_eq!(
        option_parts(&shape, &cur, 1, "c"),
        None,
        "c never pairs with opus"
    );

    assert_eq!(step_option(&shape, &cur, 1, 1, true), 1);

    assert_eq!(
        option_parts(&shape, &cur, 0, "sonnet"),
        Some(lead_sidekick("sonnet", "c"))
    );

    assert_eq!(open_option(&shape, &cur, 0), 0);
    assert_eq!(open_option(&shape, &lead_sidekick("sonnet", "c"), 1), 2);
}

#[test]
fn variant_rows_take_their_knobs_from_the_catalog() {
    let id = "picker-test/fusion";
    let shape = fusion_shape();
    cache_row_shape(id, shape.variants.clone(), shape.slots.clone());
    assert!(row_shape(id).is_some_and(|s| s.is_composite()));
    let mut config = effort_config(id, Some("high"));
    config.model_parts = lead_sidekick("opus", "a");
    let saved = SavedConfig::default();
    let mut efforts = RowEfforts::default();

    assert_eq!(efforts.shown(&config, &saved, id), Some("high"));
    efforts.step(&config, &saved, id, true);
    assert_eq!(efforts.shown(&config, &saved, id), Some("max"));

    efforts.fast = true;
    assert_eq!(efforts.shown(&config, &saved, id), Some("high"));

    efforts.fast = false;
    efforts.parts.insert(id.into(), lead_sidekick("opus", "b"));
    assert_eq!(efforts.shown(&config, &saved, id), None);

    let drop = DropState {
        row: id.into(),
        slot: 0,
        sel: 0,
    };
    assert_eq!(drop.slot, 0);

    let resolved = crate::setup::variants::resolve_variant(
        &shape.variants,
        Some("max"),
        true,
        &lead_sidekick("opus", "a"),
    );
    assert_eq!(resolved.map(|v| v.id.as_str()), Some("f-opus-a-high-fast"));
    cache_row_shape(id, Vec::new(), Vec::new());
    assert!(row_shape(id).is_none());
}

#[test]
fn composite_rows_lead_and_the_plain_rows_keep_their_order() {
    let (lead, other) = ("picker-test/lead-fusion", "picker-test/other-fusion");
    cache_row_shape(lead, Vec::new(), fusion_shape().slots);
    cache_row_shape(other, Vec::new(), fusion_shape().slots);
    let list = [
        ("a/1".to_string(), "A".to_string()),
        (lead.to_string(), "Fusion".to_string()),
        ("b/2".to_string(), "B".to_string()),
        (other.to_string(), "Fusion 2".to_string()),
        ("c/3".to_string(), "C".to_string()),
    ];
    let refs: Vec<&(String, String)> = list.iter().collect();
    let ids: Vec<&str> = super::composites_first(&refs)
        .iter()
        .map(|(id, _)| id.as_str())
        .collect();
    assert_eq!(
        ids,
        [lead, other, "a/1", "b/2", "c/3"],
        "composites first, each group in order"
    );

    let plain = sorted_models();
    let refs: Vec<&(String, String)> = plain.iter().collect();
    let ids: Vec<&str> = super::composites_first(&refs)
        .iter()
        .map(|(id, _)| id.as_str())
        .collect();
    assert_eq!(ids, ["cur/m", "a/1", "b/2", "c/3"]);
    cache_row_shape(lead, Vec::new(), Vec::new());
    cache_row_shape(other, Vec::new(), Vec::new());
}

#[test]
fn a_composite_leads_the_sorted_list_above_the_active_model() {
    let fusion = "picker-test/fusion-lead";
    let shape = fusion_shape();
    cache_row_shape(fusion, shape.variants, shape.slots);
    let list = || {
        vec![
            ("a/sonnet".to_string(), "Sonnet".to_string()),
            (fusion.to_string(), "Fusion".to_string()),
            ("a/opus".to_string(), "Opus".to_string()),
        ]
    };
    let recent: SavedConfig = serde_json::from_value(serde_json::json!({
        "base_url": "https://a/v1", "model": "a/opus",
        "recent_models": {"https://a/v1": ["a/opus", fusion]}
    }))
    .unwrap();
    let bare: SavedConfig = serde_json::from_value(serde_json::json!({
        "base_url": "https://a/v1", "model": "a/opus"
    }))
    .unwrap();
    for saved in [&recent, &bare] {
        let mut models = list();
        saved.sort_models("https://a/v1", &mut models);
        let ids: Vec<&str> = models.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(ids, [fusion, "a/opus", "a/sonnet"], "{ids:?}");
    }
    cache_row_shape(fusion, Vec::new(), Vec::new());
}

#[test]
fn the_blank_gap_takes_a_slot_and_never_a_row() {
    for gap in [None, Some(1), Some(4)] {
        for row in 0..5 {
            let slot = super::slot_of(row, gap);
            assert_eq!(super::row_at(slot, gap), Some(row));
            assert_eq!(super::rows_before(slot, gap), row);
        }
        if let Some(g) = gap {
            assert_eq!(super::row_at(g, gap), None, "the gap's slot holds no row");
        }
    }

    assert_eq!(super::slot_of(0, Some(1)), 0);
    assert_eq!(super::slot_of(1, Some(1)), 2);
    assert_eq!(super::rows_before(2, Some(1)), 1);
}

#[test]
fn the_window_keeps_the_selection_in_view_with_the_gap_counted() {
    let gap = Some(1);
    let slots = 5 + 1;
    for top in 0..slots {
        for sel in 0..5 {
            let at = super::slot_of(sel, gap);
            let (shown_top, shown) = super::list_window(slots, at, top, 3);
            assert!(
                shown_top <= at && at < shown_top + shown,
                "row {sel} from top {top} lands in {shown_top}+{shown}"
            );
        }
    }
}

fn painted_picker(
    list: &[(String, String)],
    drop: Option<&super::DropState>,
    sel: usize,
    top: usize,
) -> (Vec<String>, usize) {
    use ratatui::backend::TestBackend;
    let refs: Vec<&(String, String)> = list.iter().collect();
    let filtered = super::composites_first(&refs);
    let lead = filtered
        .iter()
        .filter(|(id, _)| super::is_composite_id(id))
        .count();
    let mut rows: Vec<super::Row> = (0..filtered.len()).map(super::Row::Model).collect();
    if let Some(d) = drop {
        let at = filtered
            .iter()
            .position(|(id, _)| *id == d.row)
            .expect("the open row is listed");
        let options = row_shape(&d.row)
            .and_then(|s| s.slots.get(d.slot).map(|slot| slot.options.len()))
            .unwrap_or(0);
        for i in (0..options).rev() {
            rows.insert(at + 1, super::Row::Opt(i));
        }
    }
    let gap = super::composite_gap(&rows, lead);
    let config = effort_config("other/live-model", None);
    let saved = SavedConfig::default();
    let efforts = RowEfforts::default();
    let view = super::PickerView {
        provider: "test provider",
        refreshing: false,
        filter: "",
        rows: &rows,
        filtered: &filtered,
        models: list,
        config: &config,
        saved: &saved,
        efforts: &efforts,
        toggles: super::PickerToggles::from_config(&config),
        drop,
        all_head: false,
        gap,
    };
    let mut terminal = ratatui::Terminal::new(TestBackend::new(80, 30)).expect("test terminal");
    let bg = crate::setup::BackgroundSnapshot::default();
    let mut scroll = top;
    terminal
        .draw(|frame| super::draw_picker(frame, &bg, &view, sel, &mut scroll))
        .expect("draw");
    let buf = terminal.backend().buffer();
    let screen: Vec<String> = (0..30u16)
        .map(|y| {
            (0..80u16)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect()
        })
        .collect();
    (screen, scroll)
}

#[test]
fn a_blank_row_separates_the_composites_from_the_plain_models() {
    let fusion = "picker-test/gap-render";
    let shape = fusion_shape();
    cache_row_shape(fusion, shape.variants, shape.slots);
    let list = vec![
        ("a/1".to_string(), "Alpha".to_string()),
        (fusion.to_string(), "Fusion".to_string()),
        ("b/2".to_string(), "Beta".to_string()),
    ];
    let (screen, _) = painted_picker(&list, None, 0, 0);
    let fusion_at = screen
        .iter()
        .position(|r| r.contains("Fusion"))
        .expect("the composite row is painted");
    assert!(
        screen[fusion_at + 1].trim().is_empty(),
        "exactly one blank row under the composite: {screen:?}"
    );
    assert!(screen[fusion_at + 2].contains("Alpha"), "{screen:?}");
    assert!(screen[fusion_at + 3].contains("Beta"), "{screen:?}");
    cache_row_shape(fusion, Vec::new(), Vec::new());
}

#[test]
fn an_open_dropdown_sits_under_its_composite_with_the_gap_below_it() {
    let fusion = "picker-test/gap-dropdown";
    let shape = fusion_shape();
    cache_row_shape(fusion, shape.variants, shape.slots);
    let list = vec![
        ("a/1".to_string(), "Alpha".to_string()),
        (fusion.to_string(), "Fusion".to_string()),
        ("b/2".to_string(), "Beta".to_string()),
    ];
    let drop = super::DropState {
        row: fusion.to_string(),
        slot: 0,
        sel: 0,
    };

    let (screen, _) = painted_picker(&list, Some(&drop), 1, 0);
    let fusion_at = screen
        .iter()
        .position(|r| r.contains("Fusion"))
        .expect("the composite row is painted");
    assert!(screen[fusion_at + 1].contains("OPUS"), "{screen:?}");
    assert!(screen[fusion_at + 2].contains("SONNET"), "{screen:?}");
    assert!(
        screen[fusion_at + 3].trim().is_empty(),
        "the gap sits below the options, not under the composite: {screen:?}"
    );

    assert!(
        screen.iter().any(|r| r.contains("2 more below")),
        "{screen:?}"
    );
    cache_row_shape(fusion, Vec::new(), Vec::new());
}

#[test]
fn a_list_without_composites_paints_no_blank_row() {
    let list = vec![
        ("a/1".to_string(), "Alpha".to_string()),
        ("b/2".to_string(), "Beta".to_string()),
    ];
    let (screen, _) = painted_picker(&list, None, 0, 0);
    let alpha = screen
        .iter()
        .position(|r| r.contains("Alpha"))
        .expect("alpha is painted");
    assert!(screen[alpha + 1].contains("Beta"), "{screen:?}");
}

#[test]
fn the_gap_counts_toward_the_scroll_window() {
    let fusion = "picker-test/gap-scroll";
    let shape = fusion_shape();
    cache_row_shape(fusion, shape.variants, shape.slots);
    let mut list = vec![(fusion.to_string(), "Fusion".to_string())];
    list.extend((0..30).map(|i| (format!("m/{i:02}"), format!("Model {i:02}"))));

    let (screen, scrolled) = painted_picker(&list, None, 30, 0);
    assert!(scrolled > 0, "the list scrolled to reach the last row");
    assert!(screen.iter().any(|r| r.contains("Model 29")), "{screen:?}");

    let (screen, _) = painted_picker(&list, None, 0, scrolled);
    assert!(screen.iter().any(|r| r.contains("Fusion")), "{screen:?}");
    cache_row_shape(fusion, Vec::new(), Vec::new());
}
