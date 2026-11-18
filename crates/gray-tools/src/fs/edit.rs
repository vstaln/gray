use std::path::Path;

use gray_core::agent::ToolOutput;
use gray_core::tool_out::{get_str, resolve_path};
use serde_json::Value;
use unicode_normalization::{UnicodeNormalization, char::canonical_combining_class};

use super::{R, err, not_found, write_atomic};

type Span = (usize, usize);

pub(super) fn run(cwd: &Path, args: &Value) -> R {
    let p = get_str(args, "path")?;
    let edits = args
        .get("edits")
        .and_then(Value::as_array)
        .filter(|a| !a.is_empty())
        .ok_or_else(|| {
            ToolOutput::error("missing required argument 'edits': expected a non-empty array")
        })?;
    let path = resolve_path(cwd, &p);
    let raw = std::fs::read(&path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => not_found(&p, &path),
        _ => ToolOutput::error(format!("{p}: {e}")),
    })?;
    let text = String::from_utf8(raw)
        .map_err(|_| ToolOutput::error(format!("Not a UTF-8 text file: {p}")))?;
    let (bom, content) = match text.strip_prefix('\u{feff}') {
        Some(rest) => ("\u{feff}", rest),
        None => ("", text.as_str()),
    };
    let crlf = content.contains("\r\n")
        && content.matches('\n').count() == content.matches("\r\n").count();
    let conv = |s: String| {
        if crlf {
            s.replace("\r\n", "\n").replace('\n', "\r\n")
        } else {
            s
        }
    };

    let mut folded: Option<(String, Vec<Span>)> = None;
    let mut found: Vec<(Span, usize, String)> = Vec::new();
    for (i, edit) in edits.iter().enumerate() {
        let n = i + 1;
        let old = conv(get_str(edit, "oldText")?);
        let new = conv(get_str(edit, "newText")?);
        if old.is_empty() {
            return err(format!("oldText for edit #{n} is empty in {p}."));
        }
        let mut hits = exact(content, &old);
        if hits.is_empty() {
            let (text, map) = folded.get_or_insert_with(|| fold_with_map(content));
            hits = exact(text, &fold_with_map(&old).0)
                .into_iter()
                .map(|(a, b)| (map[a].0, map[b - 1].1))
                .collect();
        }
        if hits.is_empty() {
            hits = by_lines(content, &old);
        }
        match hits.len() {
            0 => {
                let mut msg = format!("oldText for edit #{n} not found in {p}.");
                if !new.is_empty() && content.contains(&new) {
                    msg.push_str(
                        " newText is already in the file, so this edit may already be applied.",
                    );
                }
                return err(msg);
            }
            1 => found.push((hits[0], n, new)),
            k => {
                return err(format!(
                    "oldText for edit #{n} matches {k} places in {p}; add surrounding lines to make it unique."
                ));
            }
        }
    }

    found.sort_by_key(|f| f.0.0);
    for w in found.windows(2) {
        if w[1].0.0 < w[0].0.1 {
            let (a, b) = (w[0].1.min(w[1].1), w[0].1.max(w[1].1));
            return err(format!(
                "edits #{a} and #{b} overlap in {p}; merge them into one edit."
            ));
        }
    }
    let mut out = String::with_capacity(content.len());
    let mut at = 0;
    for ((start, end), _, new) in &found {
        out.push_str(&content[at..*start]);
        out.push_str(new);
        at = *end;
    }
    out.push_str(&content[at..]);
    if out == content {
        return err(format!(
            "Edits made no changes to {p}: oldText and newText are identical."
        ));
    }
    write_atomic(&path, format!("{bom}{out}").as_bytes())
        .map_err(|e| ToolOutput::error(format!("{p}: {e}")))?;

    let n = found.len();
    Ok(format!(
        "Edited {p}: {n} edit{}.",
        if n == 1 { "" } else { "s" }
    ))
}

fn exact(hay: &str, needle: &str) -> Vec<Span> {
    let mut hits = Vec::new();
    if needle.is_empty() {
        return hits;
    }
    let mut from = 0;
    while let Some(i) = hay[from..].find(needle) {
        let start = from + i;
        hits.push((start, start + needle.len()));
        from = start + hay[start..].chars().next().map_or(1, char::len_utf8);
    }
    hits
}

pub(super) fn fold(c: char) -> char {
    match c {
        '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}' => '\'',
        '\u{201C}' | '\u{201D}' | '\u{201E}' | '\u{201F}' => '"',
        '\u{2010}'..='\u{2015}' | '\u{2212}' => '-',
        '\u{00A0}' => ' ',
        c => c,
    }
}

fn fold_with_map(s: &str) -> (String, Vec<Span>) {
    let mut out = String::with_capacity(s.len());
    let mut map = Vec::with_capacity(s.len());
    let mut chars = s.char_indices().peekable();
    while let Some((start, c)) = chars.next() {
        let mut cluster = String::from(c);
        while let Some(&(_, next)) = chars.peek() {
            if canonical_combining_class(next) == 0 {
                break;
            }
            cluster.push(next);
            chars.next();
        }
        let end = chars.peek().map_or(s.len(), |&(i, _)| i);
        for c in cluster.nfkc().map(fold) {
            out.push(c);
            map.extend(std::iter::repeat_n((start, end), c.len_utf8()));
        }
    }
    (out, map)
}

fn by_lines(content: &str, old: &str) -> Vec<Span> {
    let lines: Vec<(usize, &str)> = content
        .split_inclusive('\n')
        .scan(0, |at, l| {
            let start = *at;
            *at += l.len();
            Some((start, l))
        })
        .collect();
    let want: Vec<&str> = old.split_inclusive('\n').map(str::trim_end).collect();
    if want.is_empty() || lines.len() < want.len() {
        return Vec::new();
    }
    (0..=lines.len() - want.len())
        .filter(|&i| {
            want.iter()
                .zip(&lines[i..])
                .all(|(w, (_, l))| l.trim_end() == *w)
        })
        .map(|i| {
            let (last_at, last) = lines[i + want.len() - 1];
            let tail = if old.ends_with('\n') {
                last
            } else {
                last.trim_end_matches(['\r', '\n'])
            };
            (lines[i].0, last_at + tail.len())
        })
        .collect()
}

pub(super) fn replace_all(cwd: &Path, args: &Value) -> R {
    let p = get_str(args, "path")?;
    let old = get_str(args, "oldText")?;
    let new = get_str(args, "newText")?;
    if old.is_empty() {
        return err(format!("old_string is empty in {p}."));
    }
    let path = resolve_path(cwd, &p);
    let raw = std::fs::read(&path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => not_found(&p, &path),
        _ => ToolOutput::error(format!("{p}: {e}")),
    })?;
    let text = String::from_utf8(raw)
        .map_err(|_| ToolOutput::error(format!("Not a UTF-8 text file: {p}")))?;
    let crlf = text.contains("\r\n") && text.matches('\n').count() == text.matches("\r\n").count();
    let (old, new) = if crlf {
        let c = |s: String| s.replace("\r\n", "\n").replace('\n', "\r\n");
        (c(old), c(new))
    } else {
        (old, new)
    };
    let n = text.matches(&old).count();
    if n == 0 {
        return err(format!("old_string not found in {p}."));
    }
    if old == new {
        return err(format!(
            "No changes to {p}: old_string and new_string are identical."
        ));
    }
    write_atomic(&path, text.replace(&old, &new).as_bytes())
        .map_err(|e| ToolOutput::error(format!("{p}: {e}")))?;
    Ok(format!(
        "Edited {p}: replaced {n} occurrence{}.",
        if n == 1 { "" } else { "s" }
    ))
}
