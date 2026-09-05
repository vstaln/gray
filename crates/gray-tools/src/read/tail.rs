//! T1.5 tail unit for the `read` tool: negative `offset` reads the tail.
//!
//! Wired in `read/mod.rs` (`mod tail;`): `offset` is parsed with
//! [`get_offset`] instead of `get_opt_u64` (which rejects negatives), the
//! selection rings over the stream with [`drain_tail`], and the notes below
//! are appended to the output.
//!
//! Spec: plan.ts T1.5 ("Negative offset reads the tail"). `limit` is ignored
//! when a tail is requested ([`limit_ignored_note`]).
//!
//! Note wording lives in `notices.rs` (moved verbatim at the wave gate);
//! [`tail_note`]/[`limit_ignored_note`] below are thin delegates so existing
//! callers and unit tests keep working with one owner per string.

use std::collections::VecDeque;

use serde_json::Value;

use super::args;
use super::stream::{LineStream, RawLine};

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

/// Drain the stream keeping only the last `n` lines (T2.2: rings over the
/// stream instead of `tail::last_n` over whole-file text, so a tail never
/// materializes the file).
///
/// Never holds more than `min(n, total)` lines; deliberately no
/// `with_capacity(n)` — `n` is untrusted (up to `i64::MAX`) and must not
/// drive allocation. Cancel surfaces as `Ok` with the stream's cancelled
/// flag set (the caller renders the cancelled note).
pub async fn drain_tail(s: &mut LineStream, n: u64) -> std::io::Result<VecDeque<RawLine>> {
    let mut buf = VecDeque::new();
    loop {
        match s.next_line().await? {
            None => break,
            Some(line) => {
                buf.push_back(line);
                while buf.len() as u64 > n {
                    buf.pop_front();
                }
            }
        }
    }
    Ok(buf)
}

/// `[read: last <shown> lines of <T> (lines <a>-<T>)]`.
///
/// `<shown>` is the lines actually shown (`min(|offset|, T)`), so the note
/// never claims more lines than the output holds (e.g. `offset=-10` on a
/// 4-line file says "last 4 lines of 4"). `total` must be > 0; callers skip
/// the note for empty files (T1.3 owns that note).
pub fn tail_note(shown: u64, total: usize) -> String {
    super::notices::tail_note(shown, total)
}

/// One-line note when `limit` accompanies a negative offset.
pub fn limit_ignored_note(limit: u64) -> String {
    super::notices::limit_ignored_note(limit)
}

#[cfg(test)]
mod tests {
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
