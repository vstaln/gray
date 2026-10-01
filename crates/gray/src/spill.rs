//! `gray spill …` — read back a tool result that did not fit in context.
//!
//! A `[spilled …]` footer names a handle; this is what a handle is for. It is
//! a subcommand rather than a tool on purpose: gray is bash-only, the model
//! already has a shell, and a new tool schema entry is schema the provider
//! re-reads and prompt caches miss on every single turn.
//!
//! Every read is bounded. A recovery path that can dump 8 MiB back into the
//! context is a second truncation problem, not a fix for the first.

use anyhow::Result;
use gray_core::spill;
use regex::Regex;

use crate::SpillCmd;

/// Lines one read may print before it says so.
const MAX_OUT_LINES: usize = 200;

/// Bytes one read may print before it says so.
const MAX_OUT_BYTES: usize = 12 * 1024;

/// Runs before `Config::resolve`, like `view`/`find`/`grep`: reading a local
/// file is nobody's provider concern.
pub fn run_cli(cmd: &SpillCmd) -> Result<()> {
    let out = match cmd {
        SpillCmd::Head { handle, lines } => {
            let text = spill::read(handle)?;
            slice(handle, &text, 0, *lines)
        }
        SpillCmd::Tail { handle, lines } => {
            let text = spill::read(handle)?;
            let start = text.lines().count().saturating_sub(*lines);
            slice(handle, &text, start, *lines)
        }
        SpillCmd::Grep {
            handle,
            pattern,
            context,
            ignore_case,
            literal,
            limit,
        } => {
            let text = spill::read(handle)?;
            let re = build(pattern, *ignore_case, *literal)?;
            grep_lines(handle, &text, &re, *context, *limit)
        }
        SpillCmd::Stats => stats(),
    };
    println!("{}", out.trim_end());
    Ok(())
}

fn build(pattern: &str, ignore_case: bool, literal: bool) -> Result<Regex> {
    regex::RegexBuilder::new(&if literal {
        regex::escape(pattern)
    } else {
        pattern.to_string()
    })
    .case_insensitive(ignore_case)
    .build()
    .map_err(|e| anyhow::anyhow!("bad pattern: {e}"))
}

/// Lines `start..start+count` (0-based `start`), numbered 1-based, with the
/// ceiling stated rather than silently applied.
fn slice(handle: &str, text: &str, start: usize, count: usize) -> String {
    let total = text.lines().count();
    if total == 0 {
        return "(that result is empty)".to_string();
    }
    if start >= total {
        return format!(
            "(that result has {total} line(s); nothing at line {})",
            start + 1
        );
    }
    let stop = (start + count).min(total);
    let mut out = String::new();
    let mut bytes = 0usize;
    let mut last = start;
    for (i, line) in text.lines().enumerate().skip(start).take(stop - start) {
        last = i;
        if i - start >= MAX_OUT_LINES || bytes >= MAX_OUT_BYTES {
            out.push_str(&format!(
                "\n[… showing lines {}-{i} of {total}; `gray spill grep {handle} <pattern>` \
                 or `gray spill tail {handle}` finds the rest]",
                start + 1
            ));
            return out;
        }
        out.push_str(&format!("{:>6}\t{}\n", i + 1, line));
        bytes += line.len() + 1;
    }
    if start > 0 {
        out.push_str(&format!("(lines {}-{} of {total})", start + 1, last + 1));
    }
    out
}

/// Matching lines with context, numbered against the original so the model can
/// ask for the exact window next. The match count is a full scan of the stored
/// original — cheap, and it means "212 match(es), showing 200" is a true
/// statement rather than a count of what fit.
fn grep_lines(
    handle: &str,
    text: &str,
    re: &Regex,
    context: usize,
    limit: Option<usize>,
) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let hits: Vec<usize> = (0..lines.len())
        .filter(|i| re.is_match(lines[*i]))
        .collect();
    if hits.is_empty() {
        return format!(
            "no match in the {} lines stored\ntry a shorter pattern, or `gray spill head {handle}` / `gray spill tail {handle}` for the ends",
            lines.len()
        );
    }
    // One pass over the hits: a line is shown when it matches, or when it is
    // within `context` lines of a match — leading context included, which is
    // what makes the line above an error readable.
    let mut shown = vec![false; lines.len()];
    for hit in &hits {
        let from = hit.saturating_sub(context);
        let to = (hit + context).min(lines.len() - 1);
        for mark in &mut shown[from..=to] {
            *mark = true;
        }
    }
    let cap = limit.unwrap_or(MAX_OUT_LINES).min(MAX_OUT_LINES);
    let mut out = String::new();
    let mut printed = 0usize;
    let mut bytes = 0usize;
    for (i, line) in lines.iter().enumerate() {
        if !shown[i] {
            continue;
        }
        if printed >= cap || bytes >= MAX_OUT_BYTES {
            break;
        }
        if re.is_match(line) {
            out.push_str(&format!("{:>6}\t{}\n", i + 1, line));
        } else {
            out.push_str(&format!("{:>6}\t- {}\n", i + 1, line));
        }
        printed += 1;
        bytes += line.len() + 1;
    }
    let mut note = format!("{} match(es)", hits.len());
    if printed < shown.iter().filter(|s| **s).count() {
        note.push_str(&format!(", showing {printed}"));
    }
    out.push_str(&format!(
        "\n[{note} in {} lines · `gray spill grep {handle} <pat> -n <N>` for context, \
         `gray spill head/tail {handle}` for the ends]",
        lines.len()
    ));
    out
}

/// What compression has saved so far, by rule — the counterfactual `/usage`
/// has never had.
fn stats() -> String {
    let totals = spill::totals();
    if totals.events == 0 {
        return "nothing squeezed or spilled yet on this machine".to_string();
    }
    let mut out = format!(
        "{} result(s) metered · {} produced → {} reached the model · {} saved ({:.0}%)",
        totals.events,
        spill::fmt_bytes(totals.raw_bytes as usize),
        spill::fmt_bytes(totals.sent_bytes as usize),
        spill::fmt_bytes(totals.saved() as usize),
        totals.saved_pct()
    );
    // The same bytes-per-token estimate the context gauge uses.
    out.push_str(&format!(
        "\n≈ {} tokens of tool output never entered a context window\n",
        totals.saved() / 4
    ));
    out.push_str("\nby rule:\n");
    let mut rows: Vec<_> = totals.by_rule.iter().collect();
    // Biggest saving first; the empty rule is a plain spill, no compressor.
    rows.sort_by_key(|(_, (_, raw, sent))| std::cmp::Reverse(raw.saturating_sub(*sent)));
    for (rule, (events, raw, sent)) in rows {
        let name = if rule.is_empty() {
            "spill (no rule)"
        } else {
            rule
        };
        out.push_str(&format!(
            "  {name:<18} {events:>4} result(s)  {} → {}\n",
            spill::fmt_bytes(*raw as usize),
            spill::fmt_bytes(*sent as usize)
        ));
    }
    out
}

#[path = "spill_tests.rs"]
#[cfg(test)]
mod tests;
