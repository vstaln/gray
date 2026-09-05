//! The `read` tool: reads a UTF-8 text file with optional line windowing.

pub mod args;
mod guard;
#[cfg(test)]
pub(crate) mod testkit;

use async_trait::async_trait;
use gray_core::agent::{ToolContext, ToolOutput};
use gray_core::message::ToolDef;
use serde_json::Value;
use serde_json::json;

use crate::truncate::{DEFAULT_MAX_BYTES, format_size, truncate_head};
use crate::{Tool, fail, get_opt_u64, get_str, resolve_path};

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
        // Legacy names (file_path/filePath/…) are renamed by the ALIASES table in
        // `crate` (coerce_args) before lookup — no per-tool chain here (T4.3).
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
        // T2.3 device/FIFO/socket refusal before any content I/O. Missing paths
        // skip both gates (canonicalize/metadata fail) and keep today's error.
        // Directories fall through: today's I/O-error path handles them (T1.3 owns the note).
        let display = full.display().to_string();
        if let Err(msg) = guard::check_name(&full, None, &display) {
            return fail(msg);
        }
        if let Err(msg) = guard::check_name(
            &full,
            std::fs::canonicalize(&full).ok().as_deref(),
            &display,
        ) {
            return fail(msg);
        }
        if let Ok(meta) = std::fs::metadata(&full) {
            if let Err(msg) = guard::check_metadata(&meta, &display) {
                return fail(msg);
            }
        }
        let data = match tokio::fs::read(&full).await {
            Ok(d) => d,
            Err(e) => return fail(format!("read failed for {}: {e}", full.display())),
        };
        let text = match String::from_utf8(data) {
            Ok(t) => t,
            Err(_) => {
                return fail(format!(
                    "{}: not valid UTF-8 (binary file?)",
                    full.display()
                ));
            }
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
            let first_size = format_size(first_line.len());
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
        } else if let Some(lim) = limit_opt {
            if start + lim < total_lines {
                let remaining = total_lines - (start + lim);
                let next_offset = start + lim + 1;
                if truncation.content.is_empty() {
                    format!(
                        "[{} more lines in file. Use offset={} to continue.]",
                        remaining, next_offset
                    )
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

        // T0.2 meter: no-op unless GRAY_TOOL_STATS=1, so zero behavior change.
        crate::stats::ToolStats {
            tool: "read",
            path: &path,
            bytes: output.len() as u64,
            lines: output.lines().count() as u64,
            truncated_by: match truncation.truncated_by {
                Some(crate::truncate::TruncatedBy::Lines) => crate::stats::CUT_LINES,
                Some(crate::truncate::TruncatedBy::Bytes) => crate::stats::CUT_BYTES,
                None => crate::stats::CUT_NONE,
            },
            notice: "none",
        }
        .report();
        // Already truncated via truncate_head with actionable hint; bypass the
        // generic head+tail truncation that would hide it.
        ToolOutput::ok(output)
    }
}
