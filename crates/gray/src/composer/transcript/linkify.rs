use std::ops::Range;
use std::sync::{Mutex, RwLock};

use gray_markdown::HyperlinkTarget;
use ratatui::text::Line;
use unicode_width::UnicodeWidthStr;

type Fence = (u8, usize);

#[derive(Default)]
struct Scan {
    fence: Option<Fence>,
    partial: String,
    resume: usize,
}

static SCAN: Mutex<Scan> = Mutex::new(Scan {
    fence: None,
    partial: String::new(),
    resume: 0,
});

struct Rule {
    owner: String,
    pattern: regex::Regex,
    url: String,
}

static RULES: RwLock<Vec<Rule>> = RwLock::new(Vec::new());

pub(crate) fn begin_turn() {
    *SCAN.lock().unwrap_or_else(|e| e.into_inner()) = Scan::default();
}

pub(crate) fn set_rules(owner: &str, rules: &[(String, String)]) -> Result<(), String> {
    let mut compiled = Vec::new();
    for (pattern, url) in rules {
        let pattern = regex::Regex::new(pattern).map_err(|e| format!("bad pattern: {e}"))?;
        compiled.push(Rule {
            owner: owner.to_string(),
            pattern,
            url: url.clone(),
        });
    }
    let mut all = RULES.write().unwrap_or_else(|e| e.into_inner());
    all.retain(|r| r.owner != owner);
    all.extend(compiled);
    Ok(())
}

fn has_rules() -> bool {
    !RULES.read().unwrap_or_else(|e| e.into_inner()).is_empty()
}

struct Hit {
    range: Range<usize>,
    url: String,
}

fn hits(line: &str, code: &[Range<usize>]) -> Vec<Hit> {
    let rules = RULES.read().unwrap_or_else(|e| e.into_inner());
    let mut out: Vec<Hit> = Vec::new();
    for rule in rules.iter() {
        for caps in rule.pattern.captures_iter(line) {
            let Some(m) = caps.get(0) else { continue };
            if m.is_empty() {
                continue;
            }
            let r = m.range();
            if code.iter().any(|c| c.start < r.end && r.start < c.end)
                || after_url(line, r.start)
                || in_link_syntax(line, r.start)
                || out
                    .iter()
                    .any(|h| h.range.start < r.end && r.start < h.range.end)
            {
                continue;
            }
            let mut url = String::new();
            caps.expand(&rule.url, &mut url);
            if url.trim().is_empty() {
                continue;
            }
            out.push(Hit { range: r, url });
        }
    }
    out.sort_by_key(|h| h.range.start);
    out
}

fn fence_open(line: &str) -> Option<Fence> {
    let t = line.trim_start();
    let b = t.as_bytes();
    let marker = *b.first()?;
    if marker != b'`' && marker != b'~' {
        return None;
    }
    let n = b.iter().take_while(|c| **c == marker).count();
    if n < 3 {
        return None;
    }
    if marker == b'`' && b[n..].contains(&b'`') {
        return None;
    }
    Some((marker, n))
}

fn fence_close(line: &str, (marker, len): Fence) -> bool {
    let t = line.trim();
    let b = t.as_bytes();
    let n = b.iter().take_while(|c| **c == marker).count();
    n >= len && n == b.len()
}

fn code_ranges(line: &str) -> Vec<Range<usize>> {
    let b = line.as_bytes();
    let mut out = Vec::new();
    let mut open: Option<(usize, usize)> = None;
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'`' {
            i += 1;
            continue;
        }
        let start = i;
        let n = b[i..].iter().take_while(|c| **c == b'`').count();
        match open {
            None => open = Some((start, n)),
            Some((s, opened)) if opened == n => {
                out.push(s..start + n);
                open = None;
            }
            Some(_) => {}
        }
        i = start + n;
    }
    if let Some((s, _)) = open {
        out.push(s..b.len());
    }
    out
}

fn after_url(line: &str, i: usize) -> bool {
    let before = &line[..i];
    let start = before
        .rfind(|c: char| c.is_whitespace() || c == '(')
        .map(|p| p + 1)
        .unwrap_or(0);
    let tok = &before[start..];
    tok.contains("://") || tok.starts_with("www.")
}

fn in_link_syntax(line: &str, i: usize) -> bool {
    let before = &line[..i];
    if before.ends_with("](") {
        return true;
    }
    match (before.rfind('['), before.rfind(']')) {
        (Some(open), Some(close)) => open > close,
        (Some(_), None) => true,
        _ => false,
    }
}

fn rewrite_line(
    line: &str,
    emit_from: usize,
    code: &[Range<usize>],
    hold_tail: bool,
) -> (String, Option<usize>) {
    if emit_from >= line.len() {
        return (String::new(), None);
    }
    let mut cut = line.len();
    if hold_tail {
        cut = line[emit_from..]
            .rfind(char::is_whitespace)
            .map(|p| {
                let at = emit_from + p;
                at + line[at..].chars().next().map_or(1, char::len_utf8)
            })
            .unwrap_or(emit_from);
    }
    let found = hits(line, code);
    if let Some(h) = found
        .iter()
        .find(|h| h.range.start < cut && h.range.end > cut && h.range.start >= emit_from)
    {
        cut = h.range.start;
    }
    let mut out = String::with_capacity(cut.saturating_sub(emit_from) + 16);
    let mut i = emit_from;
    for h in found
        .iter()
        .filter(|h| h.range.start >= emit_from && h.range.end <= cut)
    {
        out.push_str(&line[i..h.range.start]);
        out.push('[');
        out.push_str(&line[h.range.clone()]);
        out.push_str("](");
        out.push_str(&h.url);
        out.push(')');
        i = h.range.end;
    }
    if i < cut {
        out.push_str(&line[i..cut]);
    }
    (out, hold_tail.then_some(cut))
}

pub(crate) fn rewrite(chunk: &str) -> String {
    if !has_rules() {
        let st = SCAN.lock().unwrap_or_else(|e| e.into_inner());
        if st.partial.is_empty() && st.fence.is_none() {
            return chunk.to_string();
        }
    }
    if chunk.is_empty() {
        return String::new();
    }
    let mut st = SCAN.lock().unwrap_or_else(|e| e.into_inner());
    let carried = std::mem::take(&mut st.partial);
    let resume = std::mem::take(&mut st.resume);
    let mut out = String::with_capacity(chunk.len() + 32);
    for (i, piece) in chunk.split_inclusive('\n').enumerate() {
        let complete = piece.ends_with('\n');
        let body = piece.strip_suffix('\n').unwrap_or(piece);
        let prefix = if i == 0 && !carried.is_empty() {
            resume
        } else {
            0
        };
        let line = if i == 0 && !carried.is_empty() {
            format!("{carried}{body}")
        } else {
            body.to_string()
        };
        match st.fence {
            Some(f) => {
                if complete && fence_close(&line, f) {
                    st.fence = None;
                }
                out.push_str(piece);
            }
            None => {
                if complete && let Some(f) = fence_open(&line) {
                    st.fence = Some(f);
                    out.push_str(piece);
                } else {
                    let code = code_ranges(&line);
                    let (text, held) = rewrite_line(&line, prefix, &code, !complete);
                    out.push_str(&text);
                    if complete {
                        out.push('\n');
                    }
                    if !complete {
                        let next = held.unwrap_or(line.len());
                        st.partial = line;
                        st.resume = next;
                    }
                }
            }
        }
    }
    out
}

pub(crate) fn flush() -> String {
    let mut st = SCAN.lock().unwrap_or_else(|e| e.into_inner());
    let partial = std::mem::take(&mut st.partial);
    let resume = std::mem::take(&mut st.resume);
    let fenced = st.fence.take().is_some();
    if partial.is_empty() {
        return String::new();
    }
    let start = resume.min(partial.len());
    if fenced || !has_rules() {
        return partial[start..].to_string();
    }
    drop(st);
    let code = code_ranges(&partial);
    rewrite_line(&partial, start, &code, false).0
}

pub(crate) fn rewrite_final(chunk: &str) -> String {
    let mut out = rewrite(chunk);
    out.push_str(&flush());
    out
}

pub(crate) fn plain_links(lines: &[Line<'static>], first_id: u32) -> Vec<HyperlinkTarget> {
    if !has_rules() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut id = first_id;
    for (line_index, line) in lines.iter().enumerate() {
        let content: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        for h in hits(&content, &[]) {
            let start = content[..h.range.start].width();
            let end = start + content[h.range.clone()].width();
            out.push(HyperlinkTarget {
                line_index,
                column_range: start..end,
                url: h.url,
                id,
            });
            id += 1;
        }
    }
    out
}
