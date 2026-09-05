//! T4.3 input-repair unit for the `read` tool: arg aliases + strict integers.
//!
//! Pure functions only (`serde_json` is already a gray-tools dep — no new deps).
//! Intended home is `read::args` once T1.1 splits `read.rs` into `read/`; until
//! then this file is unwired on purpose (see FOLLOW-UPS below).
//!
//! Spec: plan.ts T4.3 ("Input repair: 10 path aliases, offset/limit aliases,
//! never floor"). Exact contract strings live here so the reviewer diffs one file.
//!
//! FOLLOW-UPS (not done here — files outside T4.3 ownership):
//! 1. `gray-tools/src/lib.rs`: append [`READ_ARG_ALIASES`] rows to `ALIASES`.
//! 2. `gray-tools/src/lib.rs` `coerce_args`: delete the `f64 -> i64` fallback;
//!    route `("integer", String|Number)` through [`coerce_integer`].
//! 3. `read.rs` (T1.1 owns the split): add `pub mod args;`, delete the inline
//!    `.or_else(get_str("file_path"))` chain (the table runs first), surface
//!    [`LIMIT_ZERO_NOTE`] when `limit == 0`, and surface [`coerce_integer`]
//!    errors (message upgrade may touch gray-core's `get_opt_u64` — separate
//!    crate, needs its own owner).

use serde_json::Value;

/// New alias -> canonical rows T4.3 adds to the single `ALIASES` table in
/// `gray-tools/src/lib.rs`. (Existing rows stay untouched.)
pub const READ_ARG_ALIASES: &[(&str, &str)] = &[
    // path aliases
    ("absolutePath", "path"),
    ("absolute_path", "path"),
    ("filepath", "path"),
    ("fileName", "path"),
    ("file_name", "path"),
    ("relative_path", "path"),
    ("relativePath", "path"),
    // offset aliases
    ("start_line", "offset"),
    ("startLine", "offset"),
    ("line", "offset"),
    ("from", "offset"),
    // limit aliases
    ("max_lines", "limit"),
    ("maxLines", "limit"),
    ("num_lines", "limit"),
    ("count", "limit"),
    ("lines", "limit"),
];

/// Canonical field for one of the T4.3 aliases, or `None`.
pub fn canonical_name(alias: &str) -> Option<&'static str> {
    READ_ARG_ALIASES
        .iter()
        .find(|(a, _)| *a == alias)
        .map(|(_, c)| *c)
}

/// Note returned (is_error=false) when `limit` is explicitly 0.
pub const LIMIT_ZERO_NOTE: &str =
    "[read: limit=0 shows nothing; omit limit or use limit>=1]";

/// True when the (already coerced) arg value is numeric zero.
pub fn is_limit_zero(value: &Value) -> bool {
    value.as_u64() == Some(0) || value.as_i64() == Some(0)
}

/// Renders the received value for rejection messages: strings JSON-quoted
/// (`"2abc"`), numbers plain (`1.5`).
pub fn render_got(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "<unprintable>".to_string())
}

/// Strict integer coercion for `offset`/`limit` — the "never floor" rule.
///
/// * `"2000"` (trimmed) -> `Ok(2000)`; JSON integers -> `Ok` as-is.
/// * `"2abc"`, bools, arrays, … -> `Err("invalid argument '{field}': expected
///   integer, got …")`; the caller leaves the value as-is and the tool rejects.
/// * `"1.5"` and JSON `1.5` (any fractional spelling, incl. `"2.0"`/exponents)
///   -> `Err("invalid argument '{field}': expected a whole number, got …")`.
///   There is deliberately no `f64 -> i64` fallback (that cast floored 1.5 to 1).
///   Fractional strings render trimmed and unquoted (`got 1.5`) per the contract.
///
/// Note: a JSON integer beyond `i64::MAX` falls into the whole-number branch;
/// unreachable for line windows, and the message wording stays spec-fixed.
pub fn coerce_integer(field: &str, value: &Value) -> Result<i64, String> {
    match value {
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(i)
            } else {
                Err(format!(
                    "invalid argument '{field}': expected a whole number, got {}",
                    render_got(value)
                ))
            }
        }
        Value::String(s) => {
            let t = s.trim();
            if let Ok(i) = t.parse::<i64>() {
                Ok(i)
            } else if t.parse::<f64>().is_ok() {
                Err(format!(
                    "invalid argument '{field}': expected a whole number, got {t}"
                ))
            } else {
                Err(format!(
                    "invalid argument '{field}': expected integer, got {}",
                    render_got(value)
                ))
            }
        }
        _ => Err(format!(
            "invalid argument '{field}': expected integer, got {}",
            render_got(value)
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn each_new_alias_maps_to_its_canonical_field() {
        assert_eq!(READ_ARG_ALIASES.len(), 16);
        for (alias, canon) in READ_ARG_ALIASES {
            assert_eq!(canonical_name(alias), Some(*canon), "alias {alias}");
        }
        assert_eq!(canonical_name("path"), None);
        assert_eq!(canonical_name("filepath"), Some("path"));
        assert_eq!(canonical_name("from"), Some("offset"));
        assert_eq!(canonical_name("count"), Some("limit"));
    }

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
    fn limit_zero_note_is_exact_and_detected() {
        assert_eq!(
            LIMIT_ZERO_NOTE,
            "[read: limit=0 shows nothing; omit limit or use limit>=1]"
        );
        assert!(is_limit_zero(&json!(0)));
        assert!(!is_limit_zero(&json!(3)));
        assert!(!is_limit_zero(&json!("0")));
    }
}
