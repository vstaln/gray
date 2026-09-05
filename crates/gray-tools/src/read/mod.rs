//! The `read` tool: reads a UTF-8 text file with optional line windowing.

pub mod args;
pub mod hygiene;
pub mod notices;
pub mod window;
mod guard;
mod tail;
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
             2000 lines / 50 KiB. Lines are prefixed with `<n>\\t` like \
             cat -n; do not include the prefix when quoting text for edit.",
            json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "File path (absolute or relative to cwd)"
                    },
                    "offset": {
                        "type": "integer",
                        "description": "1-based starting line number (negative -N reads the last N lines)"
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
        // T1.5: signed offset so offset<0 reads the tail (get_opt_u64
        // rejects negatives). Non-tail behavior is unchanged.
        let offset = match tail::get_offset(&args) {
            Ok(v) => v,
            Err(e) => return fail(e),
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
            match guard::check_metadata(&meta, &display) {
                Err(msg) => return fail(msg),
                // T1.3 directory note: a fact, not an error (is_error=false).
                Ok(guard::MetadataDecision::Directory) => {
                    return ToolOutput::ok(notices::directory(&display));
                }
                Ok(guard::MetadataDecision::RegularFile) => {}
            }
        }
        let data = match tokio::fs::read(&full).await {
            Ok(d) => d,
            Err(e) => return fail(format!("read failed for {}: {e}", full.display())),
        };
        // T1.3: empty files name the fact + recovery (is_error=false);
        // never return ok("").
        if data.is_empty() {
            return ToolOutput::ok(notices::empty(&display));
        }
        // T1.4 hygiene: BOM strip → magic-byte/NUL sniff → lossy decode →
        // CRLF normalize, before any line counting. Binary notes are facts
        // (is_error=false), not failures.
        let text = match hygiene::prepare(&data, &display) {
            Ok(t) => t,
            Err(note) => return ToolOutput::ok(note),
        };

        let total_lines = text.lines().count();
        // T1.5: negative offset requests the tail of |offset| lines.
        let tail_n = offset.filter(|o| *o < 0).map(|o| o.unsigned_abs());
        let start = offset.unwrap_or(1).max(1).saturating_sub(1) as usize;
        if tail_n.is_none() && start > 0 && start >= total_lines {
            // T1.3: past-EOF is a fact (is_error=false) with a tail retry;
            // `start + 1` recovers the requested 1-indexed offset.
            return ToolOutput::ok(notices::offset_past_eof(
                &display,
                start as u64 + 1,
                total_lines,
            ));
        }

        // Apply offset/limit windowing first (pi: user limit honored before truncation).
        let limit_opt = limit.map(|v| v as usize);
        let selected: Vec<&str> = match tail_n {
            // T1.5: bounded ring buffer — never holds more than |offset| lines.
            Some(n) => tail::last_n(text.lines(), n).into_iter().collect(),
            None => match limit_opt {
                Some(lim) => text.lines().skip(start).take(lim).collect(),
                None => text.lines().skip(start).collect(),
            },
        };
        // T1.5: tail counts back from the total so prefixes stay absolute.
        let first_shown = match tail_n {
            Some(_) => total_lines.saturating_sub(selected.len()) + 1,
            None => start + 1,
        };
        // T1.2: cat -n prefixes with absolute numbers, before the caps so
        // truncation math (output_lines, next_offset) is unchanged.
        let selected_content = window::prefix_lines(first_shown, &selected).join("\n");
        let truncation = truncate_head(&selected_content);
        let start_display = first_shown; // 1-indexed for messages

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
                // T1.3: contract wording, identical numbers (next = last + 1).
                notices::line_cap(start_display, end_display, total_lines)
            } else {
                // T1.3: contract wording. `next_offset` is the first unshown
                // line (truncate_head never splits a line), matching byte_cap's
                // "that line was not shown" contract.
                notices::byte_cap(start_display, end_display, next_offset)
            };
            if truncation.content.is_empty() {
                hint
            } else {
                format!("{}\n\n{}", truncation.content, hint)
            }
        // T1.5: in tail mode `limit` is ignored (noted below), so the
        // head-window "more lines" hint must not fire here.
        } else if tail_n.is_none() && let Some(lim) = limit_opt {
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
        // T1.5 tail notes (skipped for empty files — T1.3 owns that note).
        let output = if tail_n.is_some() && total_lines > 0 {
            let mut notes = vec![tail::tail_note(selected.len() as u64, total_lines)];
            if let Some(lim) = limit {
                notes.push(tail::limit_ignored_note(lim));
            }
            let notes = notes.join("\n");
            if output.is_empty() {
                notes
            } else {
                format!("{output}\n\n{notes}")
            }
        } else {
            output
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
