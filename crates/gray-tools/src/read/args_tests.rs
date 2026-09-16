use super::*;
use serde_json::json;

#[test]
fn plain_integer_strings_and_numbers_coerce() {
    assert_eq!(coerce_integer("limit", &json!("2000")), Ok(2000));
    assert_eq!(coerce_integer("offset", &json!(" 42 ")), Ok(42));
    assert_eq!(coerce_integer("limit", &json!(7)), Ok(7));
    assert_eq!(coerce_integer("offset", &json!(-3)), Ok(-3));
}

#[test]
fn fractional_string_is_not_floored_to_1_regression() {
    // The bug: coerce_args parsed "1.5" as f64 and cast to i64 -> 1.
    assert_eq!(
        coerce_integer("offset", &json!("1.5")),
        Err("invalid argument 'offset': expected a whole number, got 1.5".to_string())
    );
    assert_eq!(
        coerce_integer("offset", &json!(1.5)),
        Err("invalid argument 'offset': expected a whole number, got 1.5".to_string())
    );
}

#[test]
fn whole_valued_floats_are_still_rejected_never_floor() {
    assert!(coerce_integer("limit", &json!("2.0")).is_err());
    assert!(coerce_integer("limit", &json!(2.0)).is_err());
}

#[test]
fn non_numeric_strings_keep_expected_integer_message() {
    assert_eq!(
        coerce_integer("limit", &json!("2abc")),
        Err("invalid argument 'limit': expected integer, got \"2abc\"".to_string())
    );
    assert!(coerce_integer("limit", &json!(true)).is_err());
}

#[test]
fn limit_zero_note_is_exact() {
    assert_eq!(
        LIMIT_ZERO_NOTE,
        "[read: limit=0 shows nothing; omit limit or use limit>=1]"
    );
}
