use std::collections::HashMap;
use std::ops::Range;
use std::path::Path;
use std::sync::Mutex;

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

static REMOTES: Mutex<Option<HashMap<String, Option<String>>>> = Mutex::new(None);

pub(crate) fn begin_turn() {
    *SCAN.lock().unwrap_or_else(|e| e.into_inner()) = Scan::default();
}

fn remote_base(cwd: &Path) -> Option<String> {
    let key = cwd.to_string_lossy().to_string();
    {
        let cache = REMOTES.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(hit) = cache.as_ref().and_then(|m| m.get(&key)) {
            return hit.clone();
        }
    }
    let base = crate::system_prompt::git_field(cwd, &["remote", "get-url", "origin"])
        .as_deref()
        .and_then(normalize_remote);
    let mut cache = REMOTES.lock().unwrap_or_else(|e| e.into_inner());
    cache
        .get_or_insert_with(HashMap::new)
        .insert(key, base.clone());
    base
}

fn normalize_remote(remote: &str) -> Option<String> {
    let r = remote.trim().trim_end_matches('/');
    let r = r.strip_suffix(".git").unwrap_or(r);
    let (host, path) = if let Some(rest) = r.strip_prefix("git@") {
        rest.split_once(':')?
    } else {
        let rest = r.split_once("://").map(|(_, rest)| rest).unwrap_or(r);
        if rest == r && !rest.contains('/') {
            return None;
        }
        let rest = rest.split_once('@').map(|(_, rest)| rest).unwrap_or(rest);
        rest.split_once('/')?
    };
    if host.is_empty() || path.is_empty() || !path.contains('/') {
        return None;
    }
    Some(format!("https://{host}/{path}"))
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

fn issue_ref(line: &str, i: usize, bytes_end: usize, code: &[Range<usize>]) -> bool {
    if code.iter().any(|r| r.start <= i && i < r.end) {
        return false;
    }
    let prev_ok = i == 0 || matches!(line.as_bytes()[i - 1], b' ' | b'\t' | b'(' | b'\r' | b'\n');
    if !prev_ok || after_url(line, i) || in_link_syntax(line, i) {
        return false;
    }
    match line.as_bytes().get(bytes_end) {
        Some(c) => !(c.is_ascii_alphanumeric() || *c == b'_'),
        None => true,
    }
}

fn rewrite_line(
    line: &str,
    emit_from: usize,
    code: &[Range<usize>],
    base: &str,
    hold_tail: bool,
) -> (String, Option<usize>) {
    if emit_from >= line.len() {
        return (String::new(), None);
    }
    let b = line.as_bytes();
    let mut out = String::with_capacity(line.len() - emit_from + 16);
    let mut i = emit_from;
    while i < b.len() {
        if b[i] == b'#' {
            let mut e = i + 1;
            while e < b.len() && b[e].is_ascii_digit() {
                e += 1;
            }
            if hold_tail && e == b.len() {
                return (out, Some(i));
            }
            if e > i + 1 && issue_ref(line, i, e, code) {
                out.push('[');
                out.push_str(&line[i..e]);
                out.push_str("](");
                out.push_str(base);
                out.push_str("/issues/");
                out.push_str(&line[i + 1..e]);
                out.push(')');
                i = e;
                continue;
            }
        }
        let ch = line[i..].chars().next().unwrap_or(' ');
        out.push(ch);
        i += ch.len_utf8();
    }
    (out, None)
}

pub(crate) fn rewrite(cwd: &Path, chunk: &str) -> String {
    if !crate::settings::issue_links() {
        begin_turn();
        return chunk.to_string();
    }
    let Some(base) = remote_base(cwd) else {
        begin_turn();
        return chunk.to_string();
    };
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
                    let (text, held) = rewrite_line(&line, prefix, &code, &base, !complete);
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

pub(crate) fn flush(cwd: &Path) -> String {
    let mut st = SCAN.lock().unwrap_or_else(|e| e.into_inner());
    let partial = std::mem::take(&mut st.partial);
    let resume = std::mem::take(&mut st.resume);
    let fenced = st.fence.take().is_some();
    if partial.is_empty() {
        return String::new();
    }
    let start = resume.min(partial.len());
    if fenced || !crate::settings::issue_links() {
        return partial[start..].to_string();
    }
    let Some(base) = remote_base(cwd) else {
        return partial[start..].to_string();
    };
    let code = code_ranges(&partial);
    rewrite_line(&partial, start, &code, &base, false).0
}
