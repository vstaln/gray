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

// ── the modal paints before the network answers ──

/// A `/models` endpoint that answers with two models, served on a real
/// port so the fetch path is exercised end to end.
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
    // The refresher thread has no ambient runtime: before this split the
    // call returned an empty list there and the modal never filled in.
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
    // Whatever the saved config holds, this must not fail or block: it is
    // what the first frame is drawn from.
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

// ── recent/all divider ──

fn sorted_models() -> Vec<(String, String)> {
    // What `sort_models` yields: current first, then recents, then the rest.
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
    // The picker collapses swe-2-{high,medium,max} into one `swe-2` row;
    // a stale or hand-typed variant id resolves to the row + its tier.
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
    // `qwen3-max` listed as its own model is not a tier of `qwen3`.
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
    // The devin-sub catalog serves speed baked into the id:
    // claude rows use `-fast`, gpt rows `-priority`.
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
    // No effort stem: fast flag alone.
    assert_eq!(
        decompose_model_variant("swe-1-6-fast", &rows),
        Some(("swe-1-6".to_string(), None, true))
    );
    // Effort stem alone (the stale/hand-typed path).
    assert_eq!(
        decompose_model_variant("swe-2-max", &rows),
        Some(("swe-2".to_string(), Some("max".to_string()), false))
    );
    // Plain rows and unknown ids don't decompose.
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
    // `-fast` in the product name with no resolvable base is not a variant.
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
    // Effort "off" maps to the catalog's `-none-` product spelling.
    assert_eq!(
        compose_fast_model("gpt-6-sol", Some("off"), &rows).as_deref(),
        Some("gpt-6-sol-none-priority")
    );
    assert_eq!(
        compose_fast_model("gpt-6-sol", Some("high"), &rows).as_deref(),
        Some("gpt-6-sol-high-priority")
    );
    // Effort-free fast row.
    assert_eq!(
        compose_fast_model("swe-1-6", Some("high"), &rows).as_deref(),
        Some("swe-1-6-fast")
    );
    // No fast sibling → None (caller sends the base id).
    assert_eq!(compose_fast_model("swe-2", Some("high"), &rows), None);
    assert_eq!(compose_fast_model("swe-2", Some("max"), &rows), None);
    // An id already carrying the suffix never re-composes.
    assert_eq!(
        compose_fast_model("swe-1-6-fast", Some("high"), &rows),
        None
    );
}

// ── per-row effort (←/→ in the picker) ──

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
        provider_id: String::new(),
        credential_source: String::new(),
        auth_ref: String::new(),
    }
}

/// The step family always reasons at low/medium/high/max — a fixed,
/// cache-independent level list to step through.
const STEP_MODEL: &str = "step-3";

#[test]
fn arrows_step_within_supported_levels_and_clamp_at_the_ends() {
    let config = effort_config("other/live-model", Some("low"));
    let saved = SavedConfig::default();
    let mut efforts = RowEfforts::default();
    // Not the live model, nothing remembered: the `high` default.
    assert_eq!(efforts.shown(&config, &saved, STEP_MODEL), Some("high"));
    assert!(!efforts.was_stepped(STEP_MODEL));
    efforts.step(&config, &saved, STEP_MODEL, true);
    assert_eq!(efforts.shown(&config, &saved, STEP_MODEL), Some("max"));
    efforts.step(&config, &saved, STEP_MODEL, true);
    assert_eq!(
        efforts.shown(&config, &saved, STEP_MODEL),
        Some("max"),
        "→ clamps at the top, no wrap"
    );
    for _ in 0..5 {
        efforts.step(&config, &saved, STEP_MODEL, false);
    }
    assert_eq!(
        efforts.shown(&config, &saved, STEP_MODEL),
        Some("low"),
        "← clamps at the bottom; `off` is not a step-family level"
    );
    assert!(efforts.was_stepped(STEP_MODEL));
    // Stepping one row never touches another.
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
    // Another model is live: the row shows this model's own memory.
    let config = effort_config("other/live-model", Some("max"));
    assert_eq!(
        initial_row_effort(&config, &saved, STEP_MODEL),
        Some("medium")
    );
    // The live model's row shows what it actually runs at.
    let config = effort_config(STEP_MODEL, Some("low"));
    assert_eq!(initial_row_effort(&config, &saved, STEP_MODEL), Some("low"));
    // An unsupported remembered level clamps to what the model accepts.
    let mut saved = SavedConfig::default();
    saved.remember_effort(&effort_memory_key("", PICKER_BASE, STEP_MODEL), "xhigh");
    let config = effort_config("other/live-model", None);
    assert_eq!(initial_row_effort(&config, &saved, STEP_MODEL), Some("max"));
}

#[test]
fn enter_commits_model_and_row_effort_together() {
    // Enter already set `config.model` to the picked row.
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

// ── picker rendering helpers ──

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
    // Everything fits: no hints, no scroll.
    assert_eq!(list_window(5, 4, 0, 8), (0, 5));
    // At the top: rows below hide behind one hint slot.
    assert_eq!(list_window(20, 0, 0, 8), (0, 7));
    // Moving past the window scrolls; both hints take a slot.
    let (top, shown) = list_window(20, 10, 0, 8);
    assert!(top > 0 && top <= 10 && 10 < top + shown, "{top} {shown}");
    assert_eq!(shown, 6, "a hint above and below");
    // At the bottom: the tail fills, only the above hint remains.
    assert_eq!(list_window(20, 19, 0, 8), (13, 7));
    // A stale scroll past the end snaps back so the slots stay full.
    assert_eq!(list_window(20, 19, 18, 8), (13, 7));
    // Too short for hints: plain scrolling.
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
    // Free/flashy models sit at the bottom whatever else is listed.
    assert_eq!(price_level(0.0), 1, "free");
    assert_eq!(price_level(0.20), 1, "sub-$0.50");
    assert_eq!(price_level(0.60), 2);
    assert_eq!(price_level(5.80), 3);
    assert_eq!(price_level(18.0), 4, "Kimi K3 territory");
    assert_eq!(price_level(30.0), 5, "frontier");
    assert_eq!(price_level(60.0), 5, "priciest known");
    // Band edges belong to the lower level.
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
    // Toggled twice is untouched: nothing is written.
    let mut toggles = PickerToggles::from_config(&config);
    toggles.toggle_reasoning();
    toggles.toggle_reasoning();
    let mut saved = SavedConfig::default();
    toggles.commit(&mut config, &mut saved);
    assert_eq!(saved.show_reasoning, None);
    assert_eq!(saved.fast_mode, None);
}

// ── structured variants + composite dropdowns ──

use super::{DropState, PickLevel, enter_level, esc_level, open_option, option_parts, step_option};
use crate::setup::variants::{Parts, RowShape, cache_row_shape, row_shape};

#[test]
fn enter_walks_a_composites_slots_and_esc_backs_out() {
    // Plain row: Enter applies from the list; Esc there cancels.
    assert_eq!(enter_level(PickLevel::List, 0), (PickLevel::List, true));
    assert_eq!(esc_level(PickLevel::List), None);
    // Fusion: list → lead → sidekick → apply.
    assert_eq!(enter_level(PickLevel::List, 2), (PickLevel::Slot(0), false));
    assert_eq!(
        enter_level(PickLevel::Slot(0), 2),
        (PickLevel::Slot(1), false)
    );
    assert_eq!(enter_level(PickLevel::Slot(1), 2), (PickLevel::List, true));
    // Esc: sidekick → lead → list.
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
            // `c` only pairs with sonnet.
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
    // ↓ from `b` cannot land on `c`: stays put.
    assert_eq!(step_option(&shape, &cur, 1, 1, true), 1);
    // Switching the lead keeps a servable sidekick, else takes the first
    // one that pairs with the new lead.
    assert_eq!(
        option_parts(&shape, &cur, 0, "sonnet"),
        Some(lead_sidekick("sonnet", "c"))
    );
    // The dropdown opens on the current pick.
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
    // Standard serving: opus+a offers high and max.
    assert_eq!(efforts.shown(&config, &saved, id), Some("high"));
    efforts.step(&config, &saved, id, true);
    assert_eq!(efforts.shown(&config, &saved, id), Some("max"));
    // Fast: only high exists, so the shown level clamps into it.
    efforts.fast = true;
    assert_eq!(efforts.shown(&config, &saved, id), Some("high"));
    // opus+b has a single level: no effort knob at all.
    efforts.fast = false;
    efforts.parts.insert(id.into(), lead_sidekick("opus", "b"));
    assert_eq!(efforts.shown(&config, &saved, id), None);
    // Each lead keeps its own stepped level.
    let drop = DropState {
        row: id.into(),
        slot: 0,
        sel: 0,
    };
    assert_eq!(drop.slot, 0);
    // The wire id resolves with the composite's parts, fast falling back.
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
