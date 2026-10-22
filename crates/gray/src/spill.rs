use anyhow::Result;
use gray_core::spill;
use regex::Regex;

use crate::SpillCmd;

const MAX_OUT_LINES: usize = 200;

const MAX_OUT_BYTES: usize = 12 * 1024;

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

#[path = "spill_tests.rs"]
#[cfg(test)]
mod tests;
