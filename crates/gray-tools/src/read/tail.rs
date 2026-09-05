//! T1.5 tail unit for the `read` tool: negative `offset` reads the tail.
//!
//! Pure functions only (std + `serde_json`, both already gray-tools deps).
//! Wired in `read/mod.rs` (`mod tail;`): `offset` is parsed with
//! [`get_offset`] instead of `get_opt_u64` (which rejects negatives), the
//! selection uses [`last_n`], and the notes below are appended to the output.
//!
//! Spec: plan.ts T1.5 ("Negative offset reads the tail"). `limit` is ignored
//! when a tail is requested ([`limit_ignored_note`]).
//!
//! Note wording lives HERE until T1.3's `notices.rs` lands — T1.3 owns that
//! file concurrently, so moving these strings there now would collide. T1.3:
//! please move [`tail_note`]/[`limit_ignored_note`] into `notices.rs`
//! verbatim (contract strings, covered by the tests below).

use std::collections::VecDeque;

use serde_json::Value;

use super::args;

/// Parses the `offset` arg as a signed integer.
///
/// * absent / `null` → `Ok(None)` (same leniency as `get_opt_u64`).
/// * otherwise routed through [`args::coerce_integer`]: `"2000"` → `2000`,
///   `"-3"` / `-3` → `-3`; `"2abc"` → expected-integer error, `"1.5"` /
///   JSON `1.5` → whole-number error (never floored).
/// * Range is the spec's `[-i64::MAX, i64::MAX]`; `i64::MIN` (un-abs-able)
///   and integers beyond `i64::MAX` are rejected with the whole-number
///   message. Callers wrap the `Err(String)` with `fail()`.
pub fn get_offset(args: &Value) -> Result<Option<i64>, String> {
    let Some(v) = args.get("offset") else {
        return Ok(None);
    };
    if v.is_null() {
        return Ok(None);
    }
    let o = args::coerce_integer("offset", v)?;
    if o == i64::MIN {
        return Err(format!(
            "invalid argument 'offset': expected a whole number, got {o}"
        ));
    }
    Ok(Some(o))
}

/// Last `n` items of the line iterator via a ring buffer.
///
/// Never holds more than `min(n, total)` lines; deliberately no
/// `with_capacity(n)` — `n` is untrusted (up to `i64::MAX`) and must not
/// drive allocation. (The file text itself is already in RAM today; bounding
/// the read stream is T2.1's job — this bounds only the tail window.)
pub fn last_n<'a>(lines: impl Iterator<Item = &'a str>, n: u64) -> VecDeque<&'a str> {
    let mut buf = VecDeque::new();
    for line in lines {
        buf.push_back(line);
        while buf.len() as u64 > n {
            buf.pop_front();
        }
    }
    buf
}

/// `[read: last <shown> lines of <T> (lines <a>-<T>)]`.
///
/// `<shown>` is the lines actually shown (`min(|offset|, T)`), so the note
/// never claims more lines than the output holds (e.g. `offset=-10` on a
/// 4-line file says "last 4 lines of 4"). `total` must be > 0; callers skip
/// the note for empty files (T1.3 owns that note).
pub fn tail_note(shown: u64, total: usize) -> String {
    let first = total as u64 - shown + 1;
    format!("[read: last {shown} lines of {total} (lines {first}-{total})]")
}

/// One-line note when `limit` accompanies a negative offset.
pub fn limit_ignored_note(limit: u64) -> String {
    format!(
        "[read: limit={limit} ignored with negative offset; showing the tail instead. \
         Omit limit when offset is negative.]"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn obj(offset: Value) -> Value {
        json!({ "path": "f", "offset": offset })
    }

    #[test]
    fn ring_keeps_only_the_last_n() {
        let lines = ["a", "b", "c", "d", "e"];
        let got: Vec<_> = last_n(lines.into_iter(), 3).into_iter().collect();
        assert_eq!(got, ["c", "d", "e"]);
    }

    #[test]
    fn ring_larger_than_input_keeps_everything_in_order() {
        let lines = ["a", "b", "c", "d"];
        let got: Vec<_> = last_n(lines.into_iter(), 10).into_iter().collect();
        assert_eq!(got, ["a", "b", "c", "d"]);
    }

    #[test]
    fn absent_or_null_offset_is_none() {
        assert_eq!(get_offset(&json!({ "path": "f" })), Ok(None));
        assert_eq!(get_offset(&json!({ "path": "f", "offset": null })), Ok(None));
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
            tail_note(3, 3000),
            "[read: last 3 lines of 3000 (lines 2998-3000)]"
        );
        assert_eq!(tail_note(4, 4), "[read: last 4 lines of 4 (lines 1-4)]");
    }

    #[test]
    fn limit_ignored_note_names_value_and_recovery() {
        let note = limit_ignored_note(2);
        assert!(note.contains("limit=2"), "{note}");
        assert!(note.contains("Omit limit"), "{note}");
        assert_eq!(note.lines().count(), 1);
    }
}
