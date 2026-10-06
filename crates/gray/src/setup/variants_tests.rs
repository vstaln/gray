use super::*;

fn v(id: &str, effort: Option<&str>, fast: bool) -> ModelVariant {
    ModelVariant {
        id: id.to_string(),
        effort: effort.map(str::to_string),
        fast,
        parts: Parts::new(),
    }
}

fn parts(lead: &str, sidekick: &str) -> Parts {
    Parts::from([
        ("lead".to_string(), lead.to_string()),
        ("sidekick".to_string(), sidekick.to_string()),
    ])
}

fn vp(id: &str, effort: Option<&str>, fast: bool, p: Parts) -> ModelVariant {
    ModelVariant {
        parts: p,
        ..v(id, effort, fast)
    }
}

/// The GPT-6 Sol shape: standard ids for every tier, priority only at
/// some.
fn sol() -> Vec<ModelVariant> {
    vec![
        v("gpt-6-sol-none", Some("off"), false),
        v("gpt-6-sol-low", Some("low"), false),
        v("gpt-6-sol-medium", Some("medium"), false),
        v("gpt-6-sol-high", Some("high"), false),
        v("gpt-6-sol-max", Some("max"), false),
        v("gpt-6-sol-high-priority", Some("high"), true),
        v("gpt-6-sol-none-priority", Some("off"), true),
    ]
}

fn id_of(v: Option<&ModelVariant>) -> Option<&str> {
    v.map(|v| v.id.as_str())
}

#[test]
fn an_exact_effort_match_wins() {
    let vs = sol();
    let none = Parts::new();
    assert_eq!(
        id_of(resolve_variant(&vs, Some("high"), false, &none)),
        Some("gpt-6-sol-high")
    );
    assert_eq!(
        id_of(resolve_variant(&vs, Some("high"), true, &none)),
        Some("gpt-6-sol-high-priority")
    );
    // `none` and `off` are one level.
    assert_eq!(
        id_of(resolve_variant(&vs, Some("none"), false, &none)),
        Some("gpt-6-sol-none")
    );
}

#[test]
fn a_variant_without_effort_is_the_fallback_before_nearest() {
    let vs = vec![
        v("swe-1-6", None, false),
        v("swe-1-6-fast", None, true),
        v("swe-1-6-low", Some("low"), false),
    ];
    let none = Parts::new();
    assert_eq!(
        id_of(resolve_variant(&vs, Some("max"), false, &none)),
        Some("swe-1-6"),
        "no exact match: the provider-default id, not the nearest"
    );
    assert_eq!(
        id_of(resolve_variant(&vs, Some("high"), true, &none)),
        Some("swe-1-6-fast")
    );
    assert_eq!(
        id_of(resolve_variant(&vs, None, false, &none)),
        Some("swe-1-6")
    );
}

#[test]
fn otherwise_the_nearest_effort_wins_with_ties_going_up() {
    let vs = sol();
    let none = Parts::new();
    // xhigh sits between high and max: the tie goes up.
    assert_eq!(
        id_of(resolve_variant(&vs, Some("xhigh"), false, &none)),
        Some("gpt-6-sol-max")
    );
    // minimal: off and low are equidistant — up again.
    assert_eq!(
        id_of(resolve_variant(&vs, Some("minimal"), false, &none)),
        Some("gpt-6-sol-low")
    );
    // Only high/off are fast: max lands on high-priority.
    assert_eq!(
        id_of(resolve_variant(&vs, Some("max"), true, &none)),
        Some("gpt-6-sol-high-priority")
    );
}

#[test]
fn fast_falls_back_to_standard_serving_when_no_fast_variant_matches() {
    let vs = vec![
        v("plain-low", Some("low"), false),
        v("plain-high", Some("high"), false),
    ];
    let none = Parts::new();
    assert_eq!(
        id_of(resolve_variant(&vs, Some("high"), true, &none)),
        Some("plain-high")
    );
    assert!(!effective_fast(&vs, true, &none));
    assert!(effective_fast(&sol(), true, &none));
}

#[test]
fn composite_parts_must_match_exactly() {
    let a = parts("opus", "swe-2-high");
    let b = parts("sonnet", "swe-2-high");
    let vs = vec![
        vp("fusion-opus-high", Some("high"), false, a.clone()),
        vp("fusion-opus-max", Some("max"), false, a.clone()),
        vp("fusion-opus-high-fast", Some("high"), true, a.clone()),
        vp("fusion-sonnet-low", Some("low"), false, b.clone()),
    ];
    assert_eq!(
        id_of(resolve_variant(&vs, Some("max"), false, &a)),
        Some("fusion-opus-max")
    );
    assert_eq!(
        id_of(resolve_variant(&vs, Some("max"), false, &b)),
        Some("fusion-sonnet-low"),
        "nearest within the matching parts only"
    );
    assert_eq!(
        id_of(resolve_variant(&vs, Some("high"), true, &b)),
        Some("fusion-sonnet-low"),
        "no fast variant for these parts: standard serving"
    );
    let unknown = parts("haiku", "swe-2-high");
    assert_eq!(resolve_variant(&vs, Some("high"), false, &unknown), None);
}

#[test]
fn knobs_come_from_the_variants_of_the_current_fast_set() {
    let vs = sol();
    let none = Parts::new();
    assert_eq!(
        variant_levels(&vs, false, &none),
        vec!["off", "low", "medium", "high", "max"]
    );
    assert_eq!(
        variant_levels(&vs, true, &none),
        vec!["off", "high"],
        "fast offers only the efforts its variants pin"
    );
    let plain = vec![v("swe-1-6", None, false), v("swe-1-6-fast", None, true)];
    assert!(
        variant_levels(&plain, false, &none).is_empty(),
        "no effort knob"
    );
    assert!(has_variant(&plain, true, &none), "but Tab is live");
    assert!(!has_variant(&[v("x", None, false)], true, &none));
    // Clamping into a fast set's levels.
    assert_eq!(clamp_to_levels(&["off", "high"], "max"), Some("high"));
    assert_eq!(clamp_to_levels(&["off", "high"], "minimal"), Some("off"));
    assert_eq!(
        clamp_to_levels(&["off", "high"], "low"),
        Some("high"),
        "ties go up"
    );
    assert_eq!(clamp_to_levels(&[], "low"), None);
}

#[test]
fn the_composite_label_names_each_pick_and_the_lead_effort() {
    let shape = RowShape {
        variants: vec![vp("f", Some("high"), false, parts("opus", "swe"))],
        slots: vec![
            ModelSlot {
                key: "lead".into(),
                label: "Lead".into(),
                options: vec![gray_plugin::SlotOption {
                    id: "opus".into(),
                    name: "Claude Opus 5.5".into(),
                }],
            },
            ModelSlot {
                key: "sidekick".into(),
                label: "Sidekick".into(),
                options: vec![gray_plugin::SlotOption {
                    id: "swe".into(),
                    name: "SWE-2 High".into(),
                }],
            },
        ],
    };
    assert_eq!(
        composite_label(&shape, "Fusion", &parts("opus", "swe"), Some("high")).as_deref(),
        Some("Fusion · Opus 5.5 High + SWE-2 High")
    );
    assert_eq!(
        shape.settle_parts(Some(&parts("nope", "swe"))),
        parts("opus", "swe"),
        "an unserved selection settles on the catalog default"
    );
    let plain = RowShape {
        variants: sol(),
        slots: vec![],
    };
    assert_eq!(
        composite_label(&plain, "GPT-6 Sol", &Parts::new(), None),
        None
    );
}

#[test]
fn cached_rows_load_both_the_legacy_pair_and_the_object_form() {
    let json = r#"[["old-model","Old Model"],{"id":"gpt-6-sol","name":"GPT-6 Sol","reasoning_efforts":["off","high"],"variants":[{"id":"gpt-6-sol-high-priority","effort":"high","fast":true}],"declared":true}]"#;
    let rows: Vec<CachedRow> = serde_json::from_str(json).expect("both forms parse");
    assert_eq!(
        rows[0],
        CachedRow::Pair("old-model".into(), "Old Model".into())
    );
    assert_eq!(rows[1].pair(), ("gpt-6-sol".into(), "GPT-6 Sol".into()));
    let CachedRow::Full(m) = &rows[1] else {
        panic!("object row");
    };
    assert_eq!(m.variants[0].id, "gpt-6-sol-high-priority");
    assert!(m.variants[0].fast);
    // Pairs stay pairs on the way back out (old readers keep working on
    // lists without metadata).
    let back = serde_json::to_string(&rows[0]).unwrap();
    assert_eq!(back, r#"["old-model","Old Model"]"#);
}
