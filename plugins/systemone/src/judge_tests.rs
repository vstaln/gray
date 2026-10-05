use serde_json::json;

use super::*;

#[test]
fn rejects_missing_state() {
    let e = validate(&json!({"questions": {"q": {"type": "noul", "instructions": "ok?"}}}))
        .unwrap_err();
    assert!(e.contains("state"), "{e}");
}

#[test]
fn rejects_non_object_questions() {
    let e = validate(&json!({"state": "s", "questions": []})).unwrap_err();
    assert!(e.contains("questions"), "{e}");
}

#[test]
fn rejects_too_many_questions() {
    let questions: serde_json::Map<String, Value> = (0..65)
        .map(|i| {
            (
                format!("q{i}"),
                json!({"type": "noul", "instructions": "x?"}),
            )
        })
        .collect();
    let e = validate(&json!({"state": "s", "questions": questions})).unwrap_err();
    assert!(e.contains("64"), "{e}");
}

#[test]
fn accepts_all_three_types() {
    let args = json!({
        "state": {"cpu": 0.9},
        "questions": {
            "c": {"type": "choice", "instructions": "pick", "criteria": {"a": "A", "b": "B"}},
            "s": {"type": "score", "instructions": "rate", "criteria": ["bad", "ok", "good"]},
            "n": {"type": "noul", "instructions": "yes?"},
        }
    });
    validate(&args).unwrap();
}

#[test]
fn rejects_unknown_type() {
    let e = validate(&json!({
        "state": "s",
        "questions": {"q": {"type": "essay", "instructions": "write"}}
    }))
    .unwrap_err();
    assert!(e.contains("essay"), "{e}");
}

#[test]
fn choice_requires_nonempty_object_criteria() {
    let e = validate(&json!({
        "state": "s",
        "questions": {"q": {"type": "choice", "instructions": "i", "criteria": {}}}
    }))
    .unwrap_err();
    assert!(e.contains("criteria"), "{e}");
}

#[test]
fn score_requires_two_to_ten_levels() {
    for levels in [1usize, 11] {
        let criteria: Vec<Value> = (0..levels).map(|i| json!(format!("l{i}"))).collect();
        let e = validate(&json!({
            "state": "s",
            "questions": {"q": {"type": "score", "instructions": "i", "criteria": criteria}}
        }))
        .unwrap_err();
        assert!(e.contains("criteria"), "{e}");
    }
}

#[test]
fn noul_criteria_allows_only_true_false_keys() {
    validate(&json!({
        "state": "s",
        "questions": {"q": {"type": "noul", "instructions": "i",
            "criteria": {"true": "yes means", "false": "no means"}}}
    }))
    .unwrap();
    let e = validate(&json!({
        "state": "s",
        "questions": {"q": {"type": "noul", "instructions": "i", "criteria": {"maybe": "?"}}}
    }))
    .unwrap_err();
    assert!(e.contains("maybe"), "{e}");
}

#[test]
fn answer_probability_reads_typed_noul_and_bare_numbers() {
    assert_eq!(
        answer_probability(&json!({"type": "noul", "noul": 0.95})),
        Some(0.95)
    );
    assert_eq!(answer_probability(&json!(0.9)), Some(0.9));
    assert_eq!(answer_probability(&json!({"p": 0.7})), None);
    // A score answer's `score` field is a level, not a probability.
    assert_eq!(
        answer_probability(&json!({"type": "score", "score": 1.4})),
        None
    );
    assert_eq!(answer_probability(&json!({"label": "yes"})), None);
}
