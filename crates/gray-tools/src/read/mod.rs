//! The `read` tool: reads a UTF-8 text file with optional line windowing.

pub mod args;
mod bulk;
mod dedup;
mod guard;
pub mod hygiene;
pub mod image;
pub mod notebook;
pub mod notices;
mod resolve;
pub mod stream;
mod tail;
#[cfg(test)]
pub(crate) mod testkit;
pub mod window;

use std::sync::Arc;
use std::time::{Instant, SystemTime};

use async_trait::async_trait;
use gray_core::agent::{ToolContext, ToolOutput};
use gray_core::message::ToolDef;
use serde_json::Value;
use serde_json::json;

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

        // T4.1 unicode retry: invisible respellings before failing (same
        // parent only — a repair never changes directories). Guard + I/O run
        // on `full`; `repaired` is prepended to ok outputs below.
        let given = resolve_path(&ctx.cwd, &path);
        let full = resolve::resolve_existing(&given).unwrap_or_else(|| given.clone());
        let repaired = (full != given).then(|| {
            resolve::repaired_note(&full.display().to_string(), &given.display().to_string())
        });
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
                    return ToolOutput::ok(with_repaired(&repaired, notices::directory(&display)));
                }
                Ok(guard::MetadataDecision::RegularFile) => {}
            }
        }
        // T3.3 ledger: consume-on-hit dedup (same window, unchanged file).
        // Runs after the device/dir gates above, before any content I/O.
        // Absent offset normalizes to 1 — the same key the record path uses.
        let win_off = offset.unwrap_or(1);
        if let Some(stub) = dedup::check(
            &self.ledger,
            &full,
            &display,
            win_off,
            limit,
            dedup::enabled(),
        ) {
            return ToolOutput::ok(with_repaired(&repaired, stub.content));
        }
        // T2.1/T2.2 streaming driver: a bounded-memory `LineStream` replaces
        // `tokio::fs::read` + `prepare` + `text.lines()`. Hygiene runs on the
        // stream (BOM on the first line, one trailing `\r` per line); the
        // T1.4 sniff already ran at open. No line past the window is ever
        // claimed before it is observed (T2.2 deferred cut).
        self.execute_streamed(ctx, &full, &display, &path, offset, limit, &repaired)
            .await
    }
}

impl ReadTool {
    /// Single-`path` read over a `LineStream`. Gates and dedup ran above;
    /// bulk callers re-enter through `execute` so they share this path too.
    #[allow(clippy::too_many_arguments)]
    async fn execute_streamed(
        &self,
        ctx: &ToolContext,
        full: &std::path::Path,
        display: &str,
        path: &str,
        offset: Option<i64>,
        limit: Option<u64>,
        repaired: &Option<String>,
    ) -> ToolOutput {
        use stream::LineStream;
        let read_failed =
            |e: std::io::Error| fail(format!("read failed for {}: {e}", full.display()));
        let mut s = match LineStream::open(full, display, ctx.cancel.clone()).await {
            Ok(s) => s,
            Err(e) => return read_failed(e),
        };
        // Binary notes are facts (is_error=false), not failures. No ledger
        // entry: nothing was shown to authorize a later write.
        if let Some(note) = s.binary_note() {
            return ToolOutput::ok(with_repaired(repaired, note.to_string()));
        }
        let file_size = s.file_size();
        let max_lines = window::max_lines();
        let max_bytes = window::max_bytes();
        let max_chars = window::max_line_chars();
        let win_off = offset.unwrap_or(1);
        // T1.3: empty files name the fact + recovery (is_error=false);
        // never return ok("").
        if file_size == 0 {
            // T3.3 ledger: an empty file is a full view (nothing unseen), so
            // a later write is allowed without a re-read.
            if let Ok(meta) = std::fs::metadata(full) {
                self.ledger.record_read(
                    full,
                    LedgerEntry {
                        mtime: meta.modified().unwrap_or_else(|_| SystemTime::now()),
                        size: 0,
                        content_hash: s.content_hash(),
                        full_view: true,
                        window: (win_off, limit),
                        first_line: 1,
                        last_line: 0,
                        dedup_armed: true,
                        read_at: Instant::now(),
                    },
                );
            }
            return ToolOutput::ok(with_repaired(repaired, notices::empty(display)));
        }
        // T1.5: negative offset requests the tail of |offset| lines.
        let tail_n = offset.filter(|o| *o < 0).map(|o| o.unsigned_abs());
        // Phase 1 — collect decoded lines: (raws, first_n, total, has_more,
        // tail_mode). `total` is None past the exact-count limit (T2.1);
        // `has_more` is the T2.2 deferred peek (a line past the window was
        // actually observed).
        let (raws, first_n, total, has_more, peeked_no, tail_mode);
        if let Some(n) = tail_n {
            // T1.5: bounded ring over the stream — never holds more than
            // min(|offset|, total) lines; numbering stays absolute below.
            let ring = match tail::drain_tail(&mut s, n).await {
                Ok(r) => r,
                Err(e) => return read_failed(e),
            };
            if s.cancelled() {
                return ToolOutput::ok(with_repaired(
                    repaired,
                    notices::cancelled_note(s.line_no()),
                ));
            }
            let total_lines = s.line_no();
            first_n = total_lines.saturating_sub(ring.len()) + 1;
            raws = ring
                .into_iter()
                .map(|rl| window::WindowLine {
                    text: rl.text().into_owned(),
                    overflow_chars: rl.overflow_chars,
                })
                .collect();
            total = Some(total_lines);
            has_more = false;
            peeked_no = None;
            tail_mode = true;
        } else {
            let start = offset.unwrap_or(1).max(1).saturating_sub(1) as usize;
            // Lines before `offset` are counted, never decoded or stored.
            let mut skipped = 0usize;
            let mut eof = false;
            while skipped < start {
                match s.next_line().await {
                    Err(e) => return read_failed(e),
                    Ok(None) => {
                        eof = true;
                        break;
                    }
                    Ok(Some(_)) => skipped += 1,
                }
            }
            if s.cancelled() {
                return ToolOutput::ok(with_repaired(
                    repaired,
                    notices::cancelled_note(s.line_no()),
                ));
            }
            // Deferred skip peek (T2.2): `start` lines were counted without
            // observing EOF, so one more line is read before deciding. Empty
            // → the skip ended exactly at EOF (offset is past it); non-empty
            // → it becomes the first window line (nothing is lost).
            let mut pending: Option<stream::RawLine> = None;
            if !eof {
                match s.next_line().await {
                    Err(e) => return read_failed(e),
                    Ok(None) => eof = true,
                    Ok(Some(rl)) => pending = Some(rl),
                }
            }
            if s.cancelled() {
                return ToolOutput::ok(with_repaired(
                    repaired,
                    notices::cancelled_note(s.line_no()),
                ));
            }
            if eof {
                // The whole file was consumed, so the total is exact.
                let total_lines = s.line_no();
                if start > 0 && start >= total_lines {
                    // T1.3: past-EOF is a fact (is_error=false) with a tail
                    // retry; `start + 1` recovers the requested 1-indexed
                    // offset.
                    return ToolOutput::ok(with_repaired(
                        repaired,
                        notices::offset_past_eof(display, start as u64 + 1, total_lines),
                    ));
                }
            }
            // User limit honored before the system ceilings (pi), and the
            // system line ceiling bounds the collection either way.
            let cap = match limit {
                Some(lim) => (lim.min(max_lines as u64)) as usize,
                None => max_lines,
            };
            let mut collected: Vec<window::WindowLine> = Vec::new();
            if let Some(rl) = pending {
                collected.push(window::WindowLine {
                    text: rl.text().into_owned(),
                    overflow_chars: rl.overflow_chars,
                });
            }
            while !eof && collected.len() < cap {
                match s.next_line().await {
                    Err(e) => return read_failed(e),
                    Ok(None) => {
                        eof = true;
                        break;
                    }
                    Ok(Some(rl)) => collected.push(window::WindowLine {
                        text: rl.text().into_owned(),
                        overflow_chars: rl.overflow_chars,
                    }),
                }
            }
            if s.cancelled() {
                return ToolOutput::ok(with_repaired(
                    repaired,
                    notices::cancelled_note(s.line_no()),
                ));
            }
            // T2.2 deferred peek: the window filled without observing EOF,
            // so read one more line before claiming more remains. Empty →
            // complete, no note; non-empty → the cut names this line.
            let mut more = false;
            let mut peeked = None;
            if !eof && collected.len() == cap {
                match s.next_line().await {
                    Err(e) => return read_failed(e),
                    Ok(None) => eof = true,
                    Ok(Some(rl)) => {
                        more = true;
                        peeked = Some(rl.line_no);
                    }
                }
            }
            if s.cancelled() {
                return ToolOutput::ok(with_repaired(
                    repaired,
                    notices::cancelled_note(s.line_no()),
                ));
            }
            first_n = start + 1;
            if eof {
                total = Some(s.line_no());
            } else if stream::should_count_exact(file_size) {
                match s.count_rest_lines().await {
                    Err(e) => return read_failed(e),
                    Ok(rest) => total = Some(s.line_no() + rest as usize),
                }
                if s.cancelled() {
                    return ToolOutput::ok(with_repaired(
                        repaired,
                        notices::cancelled_note(s.line_no()),
                    ));
                }
            } else {
                // Huge file: skip the count, keep the observed resume line.
                total = None;
            }
            raws = collected;
            has_more = more;
            peeked_no = peeked;
            tail_mode = false;
        }
        // Phase 2 — window (clamp → prefix → caps) with absolute numbering.
        let w = window::window(first_n, &raws, max_lines, max_bytes, max_chars, has_more);
        let last = first_n.saturating_add(w.shown.len()).saturating_sub(1);
        let mut output = w.shown.join("\n");
        if let Some(cut) = w.cut {
            // The cut always names an observed line: after the window (line
            // cut) or the unshown line itself (byte cut).
            let next = w.next_offset.unwrap_or(first_n);
            let hint = match cut {
                window::Cut::Lines => match total {
                    // T1.3 contract wording (next = last + 1).
                    Some(t) => notices::line_cap(first_n, last, t),
                    None => notices::line_cap_count_skipped(first_n, last, next, file_size, next),
                },
                // T1.3 contract wording ("that line was not shown").
                window::Cut::Bytes => notices::byte_cap(first_n, last, next),
            };
            output = notices::join(&output, &hint);
        } else if !tail_mode && limit.is_some_and(|lim| raws.len() as u64 == lim) && has_more {
            // T1.5: in tail mode `limit` is ignored (noted below), so the
            // head-window "more lines" hint must not fire there.
            let lim = limit
                .map(|v| v.min(usize::MAX as u64) as usize)
                .unwrap_or(0);
            let next_offset = first_n + lim;
            match total {
                Some(t) => {
                    let remaining = t.saturating_sub(first_n - 1 + lim);
                    output = notices::join(
                        &output,
                        &format!(
                            "[{remaining} more lines in file. Use offset={next_offset} to continue.]"
                        ),
                    );
                }
                None => {
                    let rest_min =
                        peeked_no.map_or(1, |p| p.saturating_sub(first_n - 1 + lim).max(1));
                    output = notices::join(
                        &output,
                        &format!(
                            "[≥{rest_min} more lines in file (count skipped). \
                             Use offset={next_offset} to continue.]"
                        ),
                    );
                }
            }
        }
        // T1.5 tail notes (empty files return via the T1.3 note above).
        if tail_mode && let Some(t) = total {
            let mut notes = vec![tail::tail_note(raws.len() as u64, t)];
            if let Some(lim) = limit {
                notes.push(tail::limit_ignored_note(lim));
            }
            output = notices::join(&output, &notes.join("\n"));
        }
        // T1.1 clamp note: a fact (is_error=false), blank-line separated
        // (note alone when no content).
        if w.clamped > 0 {
            output = notices::join(&output, &notices::clamped(w.clamped));
        }

        // T3.3 ledger: record what was shown (a miss re-arms dedup).
        // `full_view` = the window covered lines 1..=T with no line/byte cut
        // (a clamped-but-complete read still counts as full — the T3.2
        // relational fix; clamp never cuts the window, only shortens lines).
        let covers_all = if tail_mode {
            total.is_some_and(|t| raws.len() == t)
        } else {
            match limit {
                Some(lim) => {
                    total.is_some_and(|t| (first_n as u64 - 1).saturating_add(lim) >= t as u64)
                }
                None => first_n == 1,
            }
        };
        if let Ok(meta) = std::fs::metadata(full) {
            self.ledger.record_read(
                full,
                LedgerEntry {
                    mtime: meta.modified().unwrap_or_else(|_| SystemTime::now()),
                    size: file_size,
                    content_hash: s.content_hash(),
                    full_view: covers_all && w.cut.is_none(),
                    window: (win_off, limit),
                    first_line: first_n,
                    last_line: last,
                    dedup_armed: true,
                    read_at: Instant::now(),
                },
            );
        }
        // T0.2 meter: no-op unless GRAY_TOOL_STATS=1, so zero behavior change.
        crate::stats::ToolStats {
            tool: "read",
            path,
            bytes: output.len() as u64,
            lines: output.lines().count() as u64,
            truncated_by: match w.cut {
                Some(window::Cut::Lines) => crate::stats::CUT_LINES,
                Some(window::Cut::Bytes) => crate::stats::CUT_BYTES,
                None => crate::stats::CUT_NONE,
            },
            notice: "none",
        }
        .report();
        // Windowed output already carries its actionable hint; bypass the
        // generic head+tail truncation that would hide it.
        ToolOutput::ok(with_repaired(repaired, output))
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

/// Prepend the T4.1 repair note (when the path was respelled) above content.
fn with_repaired(repaired: &Option<String>, content: String) -> String {
    match repaired {
        Some(note) => format!("{note}\n\n{content}"),
        None => content,
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
            return Some(fail(notices::no_files_matched(&paths)));
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
            if i == 0 && !out.is_error && out.content.contains("unchanged since your previous read")
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
