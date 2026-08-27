//! Truncation utilities ported from pi (`truncate.ts`).
//!
//! Limits: 2000 lines / 50 KiB — whichever is hit first wins. `truncate_head`
//! keeps the *first* N lines/bytes (suitable for `read`). Never returns a
//! partial line; if the first line alone exceeds the byte limit it returns
//! an empty result with `first_line_exceeds_limit = true`.

pub const DEFAULT_MAX_LINES: usize = 2000;
pub const DEFAULT_MAX_BYTES: usize = 50 * 1024;

/// Which limit caused truncation.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum TruncatedBy {
    Lines,
    Bytes,
}

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
pub fn format_size(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{bytes}B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1}KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1}MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

fn split_lines_for_counting(content: &str) -> Vec<&str> {
    if content.is_empty() {
        return Vec::new();
    }
    let mut lines: Vec<&str> = content.split('\n').collect();
    if content.ends_with('\n') {
        lines.pop();
    }
    lines
}

/// Keep the first `max_lines` / `max_bytes` of `content`, never splitting a line.
///
/// Byte counting is UTF-8 length + 1 per newline (matching `Buffer.byteLength`
/// in the TypeScript implementation). The first line exceeding `max_bytes`
/// alone produces an empty result with `first_line_exceeds_limit = true`.
pub fn truncate_head(content: &str) -> TruncationResult {
    truncate_head_with_limits(content, DEFAULT_MAX_LINES, DEFAULT_MAX_BYTES)
}

pub fn truncate_head_with_limits(
    content: &str,
    max_lines: usize,
    max_bytes: usize,
) -> TruncationResult {
    let total_bytes = content.as_bytes().len();
    let lines = split_lines_for_counting(content);
    let total_lines = lines.len();

    if total_lines <= max_lines && total_bytes <= max_bytes {
        return TruncationResult {
            content: content.to_string(),
            truncated: false,
            truncated_by: None,
            total_lines,
            total_bytes,
            output_lines: total_lines,
            output_bytes: total_bytes,
            first_line_exceeds_limit: false,
            max_lines,
            max_bytes,
        };
    }

    // First line alone exceeds the byte limit.
    if !lines.is_empty() {
        let first_line_bytes = lines[0].as_bytes().len();
        if first_line_bytes > max_bytes {
            return TruncationResult {
                content: String::new(),
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

    let mut output: Vec<&str> = Vec::new();
    let mut bytes_used: usize = 0;
    let mut truncated_by = TruncatedBy::Lines;

    for line in lines.iter() {
        if output.len() >= max_lines {
            truncated_by = TruncatedBy::Lines;
            break;
        }
        let line_bytes = line.as_bytes().len() + if output.is_empty() { 0 } else { 1 };
        if bytes_used + line_bytes > max_bytes {
            truncated_by = TruncatedBy::Bytes;
            break;
        }
        output.push(line);
        bytes_used += line_bytes;

        // If next iteration would exceed line limit, mark it.
        if output.len() >= max_lines {
            // only treat as line-limit if bytes still fit; byte limit already handled above
            truncated_by = TruncatedBy::Lines;
        }
    }

    let out_content = output.join("\n");
    let out_bytes = out_content.as_bytes().len();
    // Determine truncated_by correctly when we stopped exactly at the limit
    // but bytes still fit: lines wins. The loop above already sets it.
    let truncated_by = if output.len() < total_lines || out_bytes < total_bytes {
        Some(truncated_by)
    } else {
        None
    };
    // If truncated_by is still None but we know we truncated (guard above), default to Lines.
    let truncated_by = truncated_by.or(Some(TruncatedBy::Lines));

    TruncationResult {
        content: out_content,
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
