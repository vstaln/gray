//! T1.5 tail unit for the `read` tool: negative `offset` reads the tail.
//!
//! Wired in `read/mod.rs` (`mod tail;`): `offset` is parsed with
//! [`get_offset`] instead of `get_opt_u64` (which rejects negatives), the
//! selection rings over the stream with [`drain_tail`], and the notes below
//! are appended to the output.
//!
//! Spec: plan.ts T1.5 ("Negative offset reads the tail"). `limit` is ignored
//! when a tail is requested (see `notices::limit_ignored_note`).

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

#[path = "tail_tests.rs"]
#[cfg(test)]
mod tests;
