//! Truncation utilities — limits tool output to 2000 lines / 50 KiB.
//! Keeps the *first* N lines/bytes (for `read`). Never splits a line.
//! If first line alone exceeds limit, returns empty with `first_line_exceeds_limit=true`.
//! This is a pure-logic file — no async, no I/O — good for learning Rust basics.

// `pub const` = public constant, known at compile time.
// `usize` = pointer-sized unsigned int (size of indexing), `2000` and `50*1024` are values.
pub const DEFAULT_MAX_LINES: usize = 2000;
pub const DEFAULT_MAX_BYTES: usize = 50 * 1024;

/// Which limit caused truncation.
// `#[derive(...)]` auto-generates trait impls:
// `Debug` = `{:?}` printing, `PartialEq/Eq` = `==`, `Clone/Copy` = cheap duplication.
// `Copy` only works when all fields are `Copy` (no `String`/`Vec` inside).
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum TruncatedBy {
    Lines, // hit line limit first
    Bytes, // hit byte limit first
}

// `struct` = product type (has all fields at once).
// `pub` on each field = visible outside. `String` = owned heap string (vs `&str` borrow).
// `bool`/`usize`/`Option<TruncatedBy>` show common field types.
// `Option<T>` = `Some(T)` or `None` — Rust's null-safe alternative.
#[derive(Debug, Clone)]
pub struct TruncationResult {
    pub content: String,
    pub truncated: bool,
    pub truncated_by: Option<TruncatedBy>,
    pub total_lines: usize,
    pub total_bytes: usize,
    pub output_lines: usize,
    pub output_bytes: usize,
    pub first_line_exceeds_limit: bool,
    pub max_lines: usize,
    pub max_bytes: usize,
}

/// Human-readable byte size (mirrors `formatSize` in pi).
// `pub fn` = public function. `bytes: usize` = input arg, `-> String` = return type.
// `String` is owned; caller gets ownership of the returned string.
pub fn format_size(bytes: usize) -> String {
    // `if` is an expression in Rust — each branch returns a `String`.
    // `format!("{bytes}B")` = `format!` macro does `Display` interpolation.
    if bytes < 1024 {
        format!("{bytes}B")
    } else if bytes < 1024 * 1024 {
        // `bytes as f64` = numeric cast. `/ 1024.0` = float division.
        format!("{:.1}KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1}MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

// Private helper: no `pub`, visible only in this file.
// `content: &str` = borrowed string slice — function borrows, doesn't take ownership.
// `-> Vec<&str>` = returns a vector of borrowed slices pointing into `content`.
fn split_lines_for_counting(content: &str) -> Vec<&str> {
    if content.is_empty() {
        return Vec::new(); // `return` early; `Vec::new()` = empty vector
    }
    // `content.split('\n')` = iterator over `&str` pieces split by newline.
    // `.collect()` = gather iterator into `Vec`. Type inferred from return type.
    let mut lines: Vec<&str> = content.split('\n').collect();
    if content.ends_with('\n') {
        lines.pop(); // trailing newline creates extra empty piece — remove it, like pi does
    }
    lines
}

/// Keep the first `max_lines` / `max_bytes` of `content`, never splitting a line.
/// Byte counting is UTF-8 length + 1 per newline (matching `Buffer.byteLength` in TS).
pub fn truncate_head(content: &str) -> TruncationResult {
    // Delegates to the general function with default limits — shows function composition.
    truncate_head_with_limits(content, DEFAULT_MAX_LINES, DEFAULT_MAX_BYTES)
}

// Core logic: `&str` borrow + `usize` limits -> owned `TruncationResult`.
pub fn truncate_head_with_limits(
    content: &str,
    max_lines: usize,
    max_bytes: usize,
) -> TruncationResult {
    // `as_bytes().len()` = byte length (not char count). `"café".len()=5 bytes, 4 chars.
    let total_bytes = content.as_bytes().len();
    let lines = split_lines_for_counting(content);
    let total_lines = lines.len();

    // Fast path: nothing to truncate — return early with `truncated:false`.
    if total_lines <= max_lines && total_bytes <= max_bytes {
        return TruncationResult {
            content: content.to_string(), // `to_string()` clones borrowed `&str` into owned `String`
            truncated: false,
            truncated_by: None, // `None` = no truncation, `Some(...)` would say which limit
            total_lines,
            total_bytes,
            output_lines: total_lines,
            output_bytes: total_bytes,
            first_line_exceeds_limit: false,
            max_lines,
            max_bytes,
        };
    }

    // Edge case: first line alone too big — return empty, flag it.
    if !lines.is_empty() {
        let first_line_bytes = lines[0].as_bytes().len();
        if first_line_bytes > max_bytes {
            return TruncationResult {
                content: String::new(), // empty owned string
                truncated: true,
                truncated_by: Some(TruncatedBy::Bytes),
                total_lines,
                total_bytes,
                output_lines: 0,
                output_bytes: 0,
                first_line_exceeds_limit: true,
                max_lines,
                max_bytes,
            };
        }
    }

    // Accumulate lines until a limit would be hit.
    let mut output: Vec<&str> = Vec::new(); // `mut` = mutable binding
    let mut bytes_used: usize = 0;
    let mut truncated_by = TruncatedBy::Lines; // default, overwritten if bytes limit hits

    // `for line in lines.iter()` = borrow each `&str` in `lines` (no move).
    // `lines.iter()` yields `&&str`, pattern `line` is `& &str` auto-deref to `&str`.
    for line in lines.iter() {
        if output.len() >= max_lines {
            truncated_by = TruncatedBy::Lines;
            break; // `break` exits loop
        }
        // `+ 1` for newline separator, except for first line (no leading `\n`).
        let line_bytes = line.as_bytes().len() + if output.is_empty() { 0 } else { 1 };
        if bytes_used + line_bytes > max_bytes {
            truncated_by = TruncatedBy::Bytes;
            break;
        }
        output.push(line); // store borrowed `&str` — still pointing into original `content`
        bytes_used += line_bytes;

        if output.len() >= max_lines {
            truncated_by = TruncatedBy::Lines;
        }
    }

    // `join("\n")` concatenates borrowed slices into a new owned `String` with `\n` between.
    let out_content = output.join("\n");
    let out_bytes = out_content.as_bytes().len();
    // Decide which limit actually caused truncation (if any).
    let truncated_by = if output.len() < total_lines || out_bytes < total_bytes {
        Some(truncated_by)
    } else {
        None
    };
    // `.or(Some(...))` = if `None`, replace with `Some(Lines)`. Here ensures `Some` when truncated.
    let truncated_by = truncated_by.or(Some(TruncatedBy::Lines));

    TruncationResult {
        content: out_content, // move owned String into struct (ownership transfer)
        truncated: true,
        truncated_by,
        total_lines,
        total_bytes,
        output_lines: output.len(),
        output_bytes: out_bytes,
        first_line_exceeds_limit: false,
        max_lines,
        max_bytes,
    }
}
