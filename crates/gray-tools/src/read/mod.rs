//! The `read` tool: reads a UTF-8 text file with optional line windowing.

pub mod args;
pub mod hygiene;
pub mod image;
pub mod notices;
pub mod stream;
pub mod window;
mod bulk;
mod dedup;
mod guard;
mod tail;
#[cfg(test)]
pub(crate) mod testkit;

use std::sync::Arc;
use std::time::{Instant, SystemTime};

use async_trait::async_trait;
use gray_core::agent::{ToolContext, ToolOutput};
use gray_core::message::ToolDef;
use serde_json::Value;
use serde_json::json;

use crate::truncate::truncate_head;
use crate::{FileLedger, LedgerEntry, Tool, fail, get_opt_u64, get_str, resolve_path};

pub const READ_SNIPPET: &str = "Read file contents";
pub const READ_GUIDELINES: &[&str] = &["Use read to examine files instead of cat or sed."];

/// Reads a text file (`path`, optional 1-based `offset`, optional `limit`).
///
/// Shares a [`FileLedger`] with the write/edit tools so repeat reads can be
/// stubbed (T3.3) and writes can verify the file was seen (T3.2).
pub struct ReadTool {
    ledger: Arc<FileLedger>,
}

impl ReadTool {
    /// Share `ledger` (the registry/plugin wiring); [`Default`] keeps a
    /// private ledger so existing tests compile.
    pub fn new(ledger: Arc<FileLedger>) -> Self {
        Self { ledger }
    }
}

impl Default for ReadTool {
    fn default() -> Self {
        Self {
            ledger: Arc::new(FileLedger::new()),
        }
    }
}

#[async_trait]
impl Tool for ReadTool {
    fn def(&self) -> ToolDef {
        ToolDef::new(
            "read",
            "Read a UTF-8 text file. Returns file contents, capped at \
             2000 lines / 50 KiB. Lines are prefixed with `<n>\\t` like \
             cat -n; do not include the prefix when quoting text for edit. \
             Pass `paths` (files/globs) to read several files at once \
             (limit applies per file).",
            json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "File path (absolute or relative to cwd)"
                    },
                    "paths": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Files and/or globs to read in one call (sorted, max 200)"
                    },
                    "exclude": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Glob patterns to exclude from paths expansion"
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
                "required": []
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
        // T6.1 bulk: `paths[]`/`exclude[]` short-circuit here. Single-`path`
        // behavior below is untouched (sibling regions own it).
        if let Some(out) = self.execute_bulk(ctx, &args).await {
            return out;
        }
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
        // T3.3 ledger: consume-on-hit dedup (same window, unchanged file).
        // Runs after the device/dir gates above, before any content I/O.
        // Absent offset normalizes to 1 — the same key the record path uses.
        let win_off = offset.unwrap_or(1);
        if let Some(stub) =
            dedup::check(&self.ledger, &full, &display, win_off, limit, dedup::enabled())
        {
            return stub;
        }
        let data = match tokio::fs::read(&full).await {
            Ok(d) => d,
            Err(e) => return fail(format!("read failed for {}: {e}", full.display())),
        };
        // T1.3: empty files name the fact + recovery (is_error=false);
        // never return ok("").
        if data.is_empty() {
            // T3.3 ledger: an empty file is a full view (nothing unseen), so
            // a later write is allowed without a re-read.
            if let Ok(meta) = std::fs::metadata(&full) {
                self.ledger.record_read(
                    &full,
                    LedgerEntry {
                        mtime: meta.modified().unwrap_or_else(|_| SystemTime::now()),
                        size: 0,
                        content_hash: FileLedger::hash_bytes(&data),
                        full_view: true,
                        window: (win_off, limit),
                        first_line: 1,
                        last_line: 0,
                        dedup_armed: true,
                        read_at: Instant::now(),
                    },
                );
            }
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
        // T1.1 clamp (T5.1): per-line ceiling in chars, before the byte cap
        // so a minified line can't eat the budget. Char-boundary safe.
        let max_chars = window::max_line_chars();
        let (clamped_lines, clamped_count) = window::clamp_lines(&selected, max_chars);
        let clamped_refs: Vec<&str> =
            clamped_lines.iter().map(|s| s.as_str()).collect();
        // T1.2: cat -n prefixes with absolute numbers, before the caps so
        // truncation math (output_lines, next_offset) is unchanged.
        let selected_content = window::prefix_lines(first_shown, &clamped_refs).join("\n");
        let truncation = truncate_head(&selected_content);
        let start_display = first_shown; // 1-indexed for messages

        // T1.4 deferred this deletion until the clamp landed: clamped lines
        // can no longer exceed the byte cap, so the sed-hint branch is dead.
        let output = if truncation.truncated {
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

        // T1.1 clamp note (T5.1): a fact (is_error=false), blank-line
        // separated via the shared join (note alone when no content).
        let output = if clamped_count > 0 {
            notices::join(&output, &notices::clamped(clamped_count))
        } else {
            output
        };

        // T3.3 ledger: record what was shown (a miss re-arms dedup).
        // `full_view` = the window covered lines 1..=T with no line/byte cut
        // (a clamped-but-complete read still counts as full — the T3.2
        // relational fix; clamp never cuts the window, only shortens lines).
        if let Ok(meta) = std::fs::metadata(&full) {
            let covers_all = match (tail_n, limit_opt) {
                (Some(_), _) => selected.len() == total_lines,
                (None, Some(lim)) => start.saturating_add(lim) >= total_lines,
                (None, None) => start == 0,
            };
            self.ledger.record_read(
                &full,
                LedgerEntry {
                    mtime: meta.modified().unwrap_or_else(|_| SystemTime::now()),
                    size: data.len() as u64,
                    content_hash: FileLedger::hash_bytes(&data),
                    full_view: covers_all
                        && !truncation.truncated
                        && !truncation.first_line_exceeds_limit,
                    window: (win_off, limit),
                    first_line: first_shown,
                    last_line: first_shown
                        .saturating_add(selected.len())
                        .saturating_sub(1),
                    dedup_armed: true,
                    read_at: Instant::now(),
                },
            );
        }
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

// ── T6.1 bulk region (owner: this task; sibling regions above untouched) ──

/// Lenient string-list read for `paths`/`exclude`: absent/null → empty;
/// array → its string items; a bare string → one item (coerce_args already
/// wraps scalars for array-typed props — this is just belt-and-braces).
fn get_str_list(args: &Value, key: &str) -> Vec<String> {
    match args.get(key) {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
        Some(Value::String(s)) => vec![s.clone()],
        Some(_) => Vec::new(),
    }
}

impl ReadTool {
    /// Bulk `paths[]` mode. `None` = not a bulk call (no `paths`), so
    /// `execute` falls through to the single-`path` code untouched. Neither
    /// `path` nor `paths` → the spec-fixed missing-input message.
    async fn execute_bulk(&self, ctx: &ToolContext, args: &Value) -> Option<ToolOutput> {
        let paths = get_str_list(args, "paths");
        if paths.is_empty() {
            if args.get("path").is_none() || args.get("path").is_some_and(Value::is_null) {
                return Some(fail(bulk::MISSING_INPUT_MESSAGE.to_string()));
            }
            return None;
        }
        let excludes = get_str_list(args, "exclude");
        let rels = bulk::expand(&ctx.cwd, &paths, &excludes);
        if rels.is_empty() {
            // ponytail: wording staged here; notices.rs owner may adopt it.
            let mut shown = paths.iter().take(3).cloned().collect::<Vec<_>>().join(", ");
            if paths.len() > 3 {
                shown.push_str(", …");
            }
            return Some(fail(format!(
                "read failed: no files matched {} pattern(s): {shown}. \
                 Check the globs and exclude[].",
                paths.len()
            )));
        }
        // Per-file render reuses the single-path path above (one recursive
        // call per file, offset/limit forwarded raw): guards, hygiene,
        // windowing, and per-file ledger/dedup (T3.2/T3.3) apply unchanged.
        let mut rendered: Vec<(String, String)> = Vec::with_capacity(rels.len());
        for (i, rel) in rels.iter().enumerate() {
            let mut obj = serde_json::Map::with_capacity(3);
            obj.insert("path".to_string(), Value::String(rel.clone()));
            if let Some(o) = args.get("offset") {
                obj.insert("offset".to_string(), o.clone());
            }
            if let Some(l) = args.get("limit") {
                obj.insert("limit".to_string(), l.clone());
            }
            let single = Value::Object(obj);
            let mut out = self.execute(ctx, single.clone()).await;
            // Spec: bulk never stubs the first file. A stub consumes the T3.3
            // arm, so one retry returns it in full (no dedup code touched).
            if i == 0
                && !out.is_error
                && out.content.contains("unchanged since your previous read")
            {
                out = self.execute(ctx, single).await;
            }
            rendered.push((rel.clone(), out.content));
        }
        let sizes: Vec<(String, u64)> = rendered
            .iter()
            .map(|(n, c)| (n.clone(), c.len() as u64))
            .collect();
        let (shown, skipped) = bulk::fit_within_cap(&sizes);
        let mut blocks = Vec::with_capacity(shown.len());
        for name in &shown {
            let body = rendered
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, c)| c.as_str())
                .unwrap_or("");
            blocks.push(format!("{}\n{body}", bulk::header(name)));
        }
        let mut out = blocks.join("\n\n");
        if !skipped.is_empty() {
            let note = bulk::aggregate_note(shown.len(), rendered.len(), &skipped);
            out = if out.is_empty() {
                note
            } else {
                format!("{out}\n\n{note}")
            };
        }
        Some(ToolOutput::ok(out))
    }
}

#[cfg(test)]
mod bulk_wiring_tests {
    use super::*;

    #[test]
    fn schema_has_single_scalar_types_paths_exclude_and_empty_required() {
        let def = ReadTool::default().def();
        let props = def
            .parameters
            .get("properties")
            .and_then(|p| p.as_object())
            .expect("properties");
        for key in ["path", "paths", "exclude", "offset", "limit"] {
            assert!(props.contains_key(key), "schema missing {key}");
        }
        for (name, schema) in props {
            let t = schema
                .get("type")
                .unwrap_or_else(|| panic!("property {name} missing scalar type"));
            assert!(
                t.is_string(),
                "property {name} must have exactly one scalar type (no unions), got {t}"
            );
        }
        let req = def
            .parameters
            .get("required")
            .and_then(|r| r.as_array())
            .expect("required");
        assert!(
            req.is_empty(),
            "required must be [] (path-or-paths enforced at runtime)"
        );
    }
}
