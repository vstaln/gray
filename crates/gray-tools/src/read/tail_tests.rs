use super::*;
use serde_json::json;

fn obj(offset: Value) -> Value {
    json!({ "path": "f", "offset": offset })
}

#[tokio::test]
async fn ring_keeps_only_the_last_n() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("t.txt");
    std::fs::write(&p, b"a\nb\nc\nd\ne\n").unwrap();
    let cancel = tokio_util::sync::CancellationToken::new();
    let mut s = LineStream::open(&p, "t.txt", cancel).await.unwrap();
    let got = drain_tail(&mut s, 3).await.unwrap();
    let texts: Vec<String> = got.iter().map(|l| l.text().into_owned()).collect();
    assert_eq!(texts, ["c", "d", "e"]);
    assert_eq!(got[0].line_no, 3);
    assert_eq!(s.line_no(), 5);
}

#[tokio::test]
async fn ring_larger_than_input_keeps_everything_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("t.txt");
    std::fs::write(&p, b"a\nb\nc\nd\n").unwrap();
    let cancel = tokio_util::sync::CancellationToken::new();
    let mut s = LineStream::open(&p, "t.txt", cancel).await.unwrap();
    let got = drain_tail(&mut s, 10).await.unwrap();
    let texts: Vec<String> = got.iter().map(|l| l.text().into_owned()).collect();
    assert_eq!(texts, ["a", "b", "c", "d"]);
}

#[test]
fn absent_or_null_offset_is_none() {
    assert_eq!(get_offset(&json!({ "path": "f" })), Ok(None));
    assert_eq!(
        get_offset(&json!({ "path": "f", "offset": null })),
        Ok(None)
    );
}

#[test]
fn negative_and_positive_shapes_parse() {
    assert_eq!(get_offset(&obj(json!(-3))), Ok(Some(-3)));
    assert_eq!(get_offset(&obj(json!("-3"))), Ok(Some(-3)));
    assert_eq!(get_offset(&obj(json!(7))), Ok(Some(7)));
    assert_eq!(get_offset(&obj(json!(" 42 "))), Ok(Some(42)));
}

#[test]
fn fractional_offsets_are_rejected_never_floored() {
    assert_eq!(
        get_offset(&obj(json!(1.5))),
        Err("invalid argument 'offset': expected a whole number, got 1.5".to_string())
    );
    assert_eq!(
        get_offset(&obj(json!("1.5"))),
        Err("invalid argument 'offset': expected a whole number, got 1.5".to_string())
    );
}

#[test]
fn non_numeric_offsets_keep_expected_integer_message() {
    assert_eq!(
        get_offset(&obj(json!("2abc"))),
        Err("invalid argument 'offset': expected integer, got \"2abc\"".to_string())
    );
    assert!(get_offset(&obj(json!(true))).is_err());
}

#[test]
fn unrepresentable_magnitudes_are_rejected() {
    assert!(get_offset(&obj(json!(i64::MIN))).is_err());
    assert!(get_offset(&obj(json!(u64::MAX))).is_err());
}

#[test]
fn tail_note_strings_are_contract_exact() {
    assert_eq!(
        crate::read::notices::tail_note(3, 3000),
        "[read: last 3 lines of 3000 (lines 2998-3000)]"
    );
    assert_eq!(
        crate::read::notices::tail_note(4, 4),
        "[read: last 4 lines of 4 (lines 1-4)]"
    );
}

#[test]
fn limit_ignored_note_names_value_and_recovery() {
    let note = crate::read::notices::limit_ignored_note(2);
    assert!(note.contains("limit=2"), "{note}");
    assert!(note.contains("Omit limit"), "{note}");
    assert_eq!(note.lines().count(), 1);
}
