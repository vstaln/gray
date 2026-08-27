//! The `read` tool: reads a UTF-8 text file with optional line windowing.

use async_trait::async_trait;
use gray_core::agent::{ToolContext, ToolOutput};
use gray_core::message::ToolDef;
use serde_json::json;
use serde_json::Value;

use crate::truncate::{format_size, truncate_head, DEFAULT_MAX_BYTES};
use crate::{fail, get_opt_u64, get_str, resolve_path, Tool};

pub const READ_SNIPPET: &str = "Read file contents";
pub const READ_GUIDELINES: &[&str] = &["Use read to examine files instead of cat or sed."];

/// Reads a text file (`path`, optional 1-based `offset`, optional `limit`).
pub struct ReadTool;

#[async_trait]
impl Tool for ReadTool {
    fn def(&self) -> ToolDef {
        ToolDef::new(
            "read",
            "Read a UTF-8 text file. Returns file contents, capped at \
             2000 lines / 50 KiB.",
            json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "File path (absolute or relative to cwd)"
                    },
                    "offset": {
                        "type": "integer",
                        "description": "1-based starting line number"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of lines to return (default 2000)"
                    }
                },
                "required": ["path"]
            }),
        )
    }

    fn prompt_snippet(&self) -> Option<&'static str> {
        Some(READ_SNIPPET)
    }

    fn prompt_guidelines(&self) -> Option<&'static [&'static str]> {
        Some(READ_GUIDELINES)
    }

    // Pure read: safe to run alongside other tools.
    async fn execute(&self, ctx: &ToolContext, args: Value) -> ToolOutput {
        let path = match get_str(&args, "path") {
            Ok(p) => p,
            Err(e) => return e,
        };
        let offset = match get_opt_u64(&args, "offset") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let limit = match get_opt_u64(&args, "limit") {
            Ok(v) => v,
            Err(e) => return e,
        };

        let full = resolve_path(&ctx.cwd, &path);
        let data = match tokio::fs::read(&full).await {
            Ok(d) => d,
            Err(e) => return fail(format!("read failed for {}: {e}", full.display())),
        };
        let text = match String::from_utf8(data) {
            Ok(t) => t,
            Err(_) => return fail(format!("{}: not valid UTF-8 (binary file?)", full.display())),
        };

        let total_lines = text.lines().count();
        let start = offset.unwrap_or(1).max(1).saturating_sub(1) as usize;
        if start > 0 && start >= total_lines {
            return fail(format!(
                "offset {start} is past the end of {} ({} lines)",
                full.display(),
                total_lines
            ));
        }

        // Apply offset/limit windowing first (pi: user limit honored before truncation).
        let limit_opt = limit.map(|v| v as usize);
        let selected: Vec<&str> = match limit_opt {
            Some(lim) => text.lines().skip(start).take(lim).collect(),
            None => text.lines().skip(start).collect(),
        };
        let selected_content = selected.join("\n");
        let truncation = truncate_head(&selected_content);
        let start_display = start + 1; // 1-indexed for messages

        let output = if truncation.first_line_exceeds_limit {
            let first_line = selected.first().copied().unwrap_or("");
            let first_size = format_size(first_line.as_bytes().len());
            format!(
                "[Line {} is {}, exceeds {} limit. Use bash: sed -n '{}p' {} | head -c {}]",
                start_display,
                first_size,
                format_size(DEFAULT_MAX_BYTES),
                start_display,
                path,
                DEFAULT_MAX_BYTES
            )
        } else if truncation.truncated {
            let end_display = start_display + truncation.output_lines.saturating_sub(1);
            let next_offset = end_display + 1;
            let hint = if truncation.truncated_by == Some(crate::truncate::TruncatedBy::Lines) {
                format!(
                    "[Showing lines {}-{} of {}. Use offset={} to continue.]",
                    start_display, end_display, total_lines, next_offset
                )
            } else {
                format!(
                    "[Showing lines {}-{} of {} ({} limit). Use offset={} to continue.]",
                    start_display,
                    end_display,
                    total_lines,
                    format_size(DEFAULT_MAX_BYTES),
                    next_offset
                )
            };
            if truncation.content.is_empty() {
                hint
            } else {
                format!("{}\n\n{}", truncation.content, hint)
            }
        } else if limit_opt.is_some() {
            let lim = limit_opt.unwrap();
            if start + lim < total_lines {
                let remaining = total_lines - (start + lim);
                let next_offset = start + lim + 1;
                if truncation.content.is_empty() {
                    format!("[{} more lines in file. Use offset={} to continue.]", remaining, next_offset)
                } else {
                    format!(
                        "{}\n\n[{} more lines in file. Use offset={} to continue.]",
                        truncation.content, remaining, next_offset
                    )
                }
            } else {
                truncation.content
            }
        } else {
            truncation.content
        };

        // Already truncated via truncate_head with actionable hint; bypass the
        // generic head+tail truncation that would hide it.
        ToolOutput::ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(dir: &std::path::Path) -> ToolContext {
        ToolContext { cwd: dir.to_path_buf(), cancel: Default::default() }
    }

    #[tokio::test]
    async fn round_trips_file_contents() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "line1\nline2\nline3\n").unwrap();
        let out = ReadTool.execute(&ctx(dir.path()), json!({"path": "a.txt"})).await;
        assert!(!out.is_error, "{}", out.content);
        // normalize: pi-style read no longer guarantees trailing newline
        assert!(out.content.starts_with("line1\nline2\nline3"));
    }

    #[tokio::test]
    async fn absolute_paths_work() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("abs.txt");
        std::fs::write(&file, "hi").unwrap();
        let out =
            ReadTool.execute(&ctx(dir.path()), json!({"path": file.to_str().unwrap()})).await;
        assert!(!out.is_error, "{}", out.content);
        assert_eq!(out.content, "hi");
    }

    #[tokio::test]
    async fn honors_offset_and_limit() {
        let dir = tempfile::tempdir().unwrap();
        let body: String = (1..=10).map(|i| format!("line{i}\n")).collect();
        std::fs::write(dir.path().join("a.txt"), body).unwrap();
        let out = ReadTool
            .execute(&ctx(dir.path()), json!({"path": "a.txt", "offset": 3, "limit": 2}))
            .await;
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.starts_with("line3\nline4"));
    }

    #[tokio::test]
    async fn missing_file_is_error_not_panic() {
        let dir = tempfile::tempdir().unwrap();
        let out = ReadTool.execute(&ctx(dir.path()), json!({"path": "nope.txt"})).await;
        assert!(out.is_error);
        assert!(out.content.contains("read failed"));
    }

    #[tokio::test]
    async fn binary_content_is_rejected_cleanly() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("bin"), [0xFF, 0xFE, 0x00, 0x01]).unwrap();
        let out = ReadTool.execute(&ctx(dir.path()), json!({"path": "bin"})).await;
        assert!(out.is_error);
        assert!(out.content.contains("UTF-8"));
    }

    #[tokio::test]
    async fn missing_path_argument_is_error() {
        let dir = tempfile::tempdir().unwrap();
        let out = ReadTool.execute(&ctx(dir.path()), json!({})).await;
        assert!(out.is_error);
        assert!(out.content.contains("missing required argument 'path'"));
    }
}
