use super::*;

#[test]
fn parse_context_window_accepts_separators() {
    assert_eq!(parse_context_window("1000000"), Some(1_000_000));
    assert_eq!(parse_context_window("1,000,000"), Some(1_000_000));
    assert_eq!(parse_context_window("1.000.000"), Some(1_000_000));
    assert_eq!(parse_context_window("1_000_000"), Some(1_000_000));
    assert_eq!(parse_context_window("1000k"), Some(1_000_000));
    assert_eq!(parse_context_window("128k"), Some(128_000));
    assert_eq!(parse_context_window("1.5m"), Some(1_500_000));
    assert_eq!(parse_context_window("1.5"), None); // bare decimals rejected
    assert_eq!(parse_context_window(""), None);
    assert_eq!(parse_context_window("abc"), None);
}

#[test]
fn model_max_ignores_user_override() {
    set_user_context_window(Some(8_000));
    // gpt-4o fallback is 128k; max must ignore the 8k override
    assert_eq!(model_max_context("gpt-4o"), 128_000);
    set_user_context_window(None);
}

#[test]
fn compaction_defaults_match_legacy() {
    assert_eq!(user_keep_recent_tokens(), 20_000);
}

#[test]
fn reasoning_capability_from_models_dev_and_live() {
    // Unknown until a provider speaks.
    assert_eq!(
        model_supports_reasoning("test-reason-capability-never-seen"),
        None
    );
    // models.dev `reasoning` flag.
    let v: serde_json::Value = serde_json::json!({
        "prov": {"models": {
            "test-reason-thinker": {"reasoning": true, "limit": {"context": 200000}},
            "test-reason-plain": {"reasoning": false, "limit": {"context": 32000}},
        }},
    });
    parse_models_dev_json(&v);
    assert_eq!(model_supports_reasoning("test-reason-thinker"), Some(true));
    assert_eq!(
        model_supports_reasoning("prov/test-reason-plain"),
        Some(false)
    );
    // Non-reasoning models only get `off`.
    assert_eq!(
        supported_thinking_levels("prov/test-reason-plain"),
        vec![("off", "No reasoning")]
    );
    // Family efforts are provider-driven, not the hardcoded catalog.
    let grok: Vec<&str> = supported_thinking_levels("xai/grok-4")
        .iter()
        .map(|(l, _)| *l)
        .collect();
    assert_eq!(grok, vec!["off", "low", "medium", "high"]);
    let claude: Vec<&str> = supported_thinking_levels("anthropic/claude-opus-4-6")
        .iter()
        .map(|(l, _)| *l)
        .collect();
    assert_eq!(claude, vec!["off", "low", "medium", "high", "max"]);
    // Modern Claude takes xhigh; GPT-5.6 additionally takes max.
    let opus47: Vec<&str> = supported_thinking_levels("anthropic/claude-opus-4-7")
        .iter()
        .map(|(l, _)| *l)
        .collect();
    assert_eq!(opus47, vec!["off", "low", "medium", "high", "xhigh", "max"]);
    let gpt56: Vec<&str> = supported_thinking_levels("openai/gpt-5.6-sol")
        .iter()
        .map(|(l, _)| *l)
        .collect();
    assert_eq!(gpt56, vec!["off", "low", "medium", "high", "xhigh", "max"]);
    let gemini: Vec<&str> = supported_thinking_levels("google/gemini-3.6-flash")
        .iter()
        .map(|(l, _)| *l)
        .collect();
    assert_eq!(gemini, vec!["off", "minimal", "low", "medium", "high"]);
    // Muse Spark contributor has no `max` (models.dev + family table agree).
    let spark: Vec<&str> = supported_thinking_levels("testprov/muse-spark-9.9-contributor")
        .iter()
        .map(|(l, _)| *l)
        .collect();
    assert_eq!(
        spark,
        vec!["off", "minimal", "low", "medium", "high", "xhigh"]
    );
    // Qualified provider entries must not leak efforts into another
    // provider's bare id: kilo/openrouter list their qualified
    // `meta/muse-spark-1.3-contributor` WITH `max` while bare-id
    // providers list the contributor id WITHOUT it — the live
    // `/thinking` picker showed `max` once the background models.dev
    // fetch landed. Bare-first order (the observed poisoning):
    let v3: serde_json::Value = serde_json::json!({
        "testpoisa": {"models": {
            "spark-poison-7-contributor": {
                "reasoning": true,
                "limit": {"context": 200000},
                "reasoning_options": [{"type": "effort", "values": ["minimal", "low", "medium", "high", "xhigh"]}],
            },
        }},
    });
    parse_models_dev_json(&v3);
    let v4: serde_json::Value = serde_json::json!({
        "testpoisq": {"models": {
            "testpoisq/spark-poison-7-contributor": {
                "reasoning": true,
                "limit": {"context": 200000},
                "reasoning_options": [{"type": "effort", "values": ["minimal", "low", "medium", "high", "xhigh", "max"]}],
            },
        }},
    });
    parse_models_dev_json(&v4);
    let bare7: Vec<&str> = supported_thinking_levels("spark-poison-7-contributor")
        .iter()
        .map(|(l, _)| *l)
        .collect();
    assert_eq!(
        bare7,
        vec!["off", "minimal", "low", "medium", "high", "xhigh"]
    );
    // The qualified id keeps its own provider-specific values (with max).
    let qual7: Vec<&str> = supported_thinking_levels("testpoisq/spark-poison-7-contributor")
        .iter()
        .map(|(l, _)| *l)
        .collect();
    assert_eq!(
        qual7,
        vec!["off", "minimal", "low", "medium", "high", "xhigh", "max"]
    );
    // Reverse order (qualified first): the later exact bare entry still wins.
    let v5: serde_json::Value = serde_json::json!({
        "testpoisq": {"models": {
            "testpoisq/spark-poison-8-contributor": {
                "reasoning": true,
                "limit": {"context": 200000},
                "reasoning_options": [{"type": "effort", "values": ["minimal", "low", "medium", "high", "xhigh", "max"]}],
            },
        }},
    });
    parse_models_dev_json(&v5);
    let v6: serde_json::Value = serde_json::json!({
        "testpoisa": {"models": {
            "spark-poison-8-contributor": {
                "reasoning": true,
                "limit": {"context": 200000},
                "reasoning_options": [{"type": "effort", "values": ["minimal", "low", "medium", "high", "xhigh"]}],
            },
        }},
    });
    parse_models_dev_json(&v6);
    let bare8: Vec<&str> = supported_thinking_levels("spark-poison-8-contributor")
        .iter()
        .map(|(l, _)| *l)
        .collect();
    assert_eq!(
        bare8,
        vec!["off", "minimal", "low", "medium", "high", "xhigh"]
    );
    // Future GPT family (e.g. gpt-6-astra) gets the generous modern set.
    let gpt6: Vec<&str> = supported_thinking_levels("openai/gpt-6-astra")
        .iter()
        .map(|(l, _)| *l)
        .collect();
    assert_eq!(gpt6, vec!["off", "low", "medium", "high", "xhigh"]);
    // models.dev `reasoning_options` drive levels automatically — no
    // family table entry needed for unknown families ...
    let v2: serde_json::Value = serde_json::json!({
        "testprov": {"models": {
            "test-effort-auto": {
                "reasoning": true,
                "limit": {"context": 200000},
                "reasoning_options": [{"type": "effort", "values": [null, "low", "medium", "high", "xhigh", "max"]}],
            },
            "test-grok-future": {
                "reasoning": true,
                "limit": {"context": 128000},
                "reasoning_options": [{"type": "effort", "values": ["low", "medium", "high", "max"]}],
            },
        }},
    });
    parse_models_dev_json(&v2);
    let auto: Vec<&str> = supported_thinking_levels("testprov/test-effort-auto")
        .iter()
        .map(|(l, _)| *l)
        .collect();
    assert_eq!(auto, vec!["off", "low", "medium", "high", "xhigh", "max"]);
    // ... and beat the family table where it disagrees (grok has no max).
    let grok_future: Vec<&str> = supported_thinking_levels("xai/test-grok-future")
        .iter()
        .map(|(l, _)| *l)
        .collect();
    assert_eq!(grok_future, vec!["off", "low", "medium", "high", "max"]);
}

#[test]
fn litellm_parse_fills_gaps_only() {
    let v: serde_json::Value = serde_json::json!({
        "sample_spec": {"max_input_tokens": 1},
        "test-litellm-alpha": {"max_input_tokens": 123000, "max_tokens": 16000},
        "test-litellm-beta": {"max_tokens": 64000},
        "test-litellm-tiny": {"max_tokens": 16},
        "prov/test-litellm-gamma": {"max_input_tokens": 50000},
    });
    assert_eq!(parse_litellm_context_json(&v), 3);
    // max_input_tokens wins over output-sized max_tokens
    assert_eq!(model_max_context("test-litellm-alpha"), 123_000);
    assert_eq!(model_max_context("test-litellm-beta"), 64_000);
    // suffix fallback: `some-provider/model` hits the bare key
    assert_eq!(model_max_context("someprov/test-litellm-alpha"), 123_000);
    assert_eq!(model_max_context("prov/test-litellm-gamma"), 50_000);
    // provider values win over litellm gaps regardless of order
    cache_model_context("test-litellm-alpha", 999_000);
    assert_eq!(parse_litellm_context_json(&v), 3);
    assert_eq!(model_max_context("test-litellm-alpha"), 999_000);
}

#[test]
fn litellm_rates_and_turn_cost() {
    let v: serde_json::Value = serde_json::json!({
        "test-rate-full": {
            "max_input_tokens": 200000,
            "input_cost_per_token": 0.000003,
            "output_cost_per_token": 0.000015,
            "cache_read_input_token_cost": 0.0000003,
            "cache_creation_input_token_cost": 0.00000375,
        },
        "test-rate-nocache": {
            "max_input_tokens": 128000,
            "input_cost_per_token": 0.000002,
            "output_cost_per_token": 0.000008,
        },
        "test-rate-half": {
            "max_input_tokens": 64000,
            "input_cost_per_token": 0.000001,
        },
    });
    parse_litellm_context_json(&v);
    // full entry: cache-aware
    let r = get_model_rate("someprov/test-rate-full").expect("rate with suffix fallback");
    assert!(r.has_cache_prices);
    let u = gray_core::event::Usage {
        input_tokens: 10_000,
        output_tokens: 2_000,
        non_cached_input_tokens: 6_000,
        cache_read_input_tokens: 3_000,
        cache_write_input_tokens: 1_000,
        ..Default::default()
    };
    let cost = turn_cost(&u, "test-rate-full").expect("priced");
    let want = 6_000.0 * 0.000003 + 3_000.0 * 0.0000003 + 1_000.0 * 0.00000375 + 2_000.0 * 0.000015;
    assert!((cost - want).abs() < 1e-9, "got {cost}, want {want}");
    // no cache prices: everything at input rate
    let u2 = gray_core::event::Usage::new(10_000, 2_000);
    let cost2 = turn_cost(&u2, "test-rate-nocache").expect("priced");
    assert!((cost2 - (10_000.0 * 0.000002 + 2_000.0 * 0.000008)).abs() < 1e-9);
    // inclusive-only providers: all input priced fresh
    let u3 = gray_core::event::Usage {
        input_tokens: 5_000,
        output_tokens: 0,
        ..Default::default()
    };
    assert!(turn_cost(&u3, "test-rate-full").expect("priced") > 0.0);
    // half entry dropped, unknown model unpriced
    assert!(get_model_rate("test-rate-half").is_none());
    assert!(turn_cost(&u, "no-such-model").is_none());
    // formatting
    assert_eq!(format_cost(0.004), "$0.004");
    assert_eq!(format_cost(0.41), "$0.41");
    assert_eq!(format_cost(1.5), "$1.50");
    assert_eq!(format_cost(0.0), "$0");
}

#[test]
fn openrouter_rates_gapfill() {
    let v: serde_json::Value = serde_json::json!({
        "data": [
            {"id": "test-or-new-model", "pricing": {"prompt": "0.000003", "completion": "0.000015"}},
            {"id": "test-or-claimed", "pricing": {"prompt": "0.99", "completion": "0.99"}},
            {"id": "test-or-free", "pricing": {"prompt": "0", "completion": "0"}},
        ]
    });
    assert_eq!(parse_openrouter_models_json(&v), 2);
    let r = get_model_rate("test-or-new-model").expect("openrouter rate cached");
    assert!(!r.has_cache_prices);
    // gap-fill only: second parse can't overwrite an existing rate
    let v2: serde_json::Value = serde_json::json!({
        "data": [{"id": "test-or-new-model", "pricing": {"prompt": "0.99", "completion": "0.99"}}]
    });
    parse_openrouter_models_json(&v2);
    let kept = get_model_rate("test-or-new-model").expect("rate kept");
    assert!((kept.input - 0.000003).abs() < 1e-12);
}

#[test]
fn breakdown_free_and_grid_sum_to_window() {
    let p = ContextParts {
        system_prompt: 2_300,
        project_context: 1_600,
        tools: 16_700,
        skills: 279,
        messages: 42_200,
    };
    let window = 200_000;
    let reserve = 45_000;
    assert_eq!(p.used(), 2_300 + 1_600 + 16_700 + 279 + 42_200);
    assert_eq!(p.free(window, reserve), window - p.used() - reserve);
    assert_eq!(p.grid_cells(window, reserve).iter().sum::<usize>(), 100);
    // saturates instead of underflowing when over budget
    assert_eq!(p.free(10_000, reserve), 0);
}
