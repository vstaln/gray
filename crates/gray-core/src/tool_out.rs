use serde_json::Value;

use crate::agent::ToolOutput;
use crate::spill;

pub const MAX_LINES: usize = 2000;

pub const MAX_BYTES: usize = 12 * 1024;

pub const MAX_ERROR_BYTES: usize = 2048;

fn strip_ns(n: &str) -> &str {
    n.rsplit("__").next().unwrap_or(n)
}

pub fn unknown_tool(name: &str, available: &[String]) -> String {
    let bare = strip_ns(name);
    let mut scored: Vec<(&str, f64)> = available
        .iter()
        .map(|n| {
            let s = strsim::jaro_winkler(name, n)
                .max(strsim::jaro_winkler(bare, strip_ns(n)))
                .max(strsim::jaro_winkler(name, strip_ns(n)));
            (n.as_str(), s)
        })
        .filter(|(_, score)| *score >= 0.87)
        .collect();
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let mut msg = format!("Tool '{name}' does not exist.");
    let mut names = scored.iter().take(3).map(|(n, _)| format!("'{n}'"));
    if let Some(first) = names.next() {
        let rest: Vec<String> = names.collect();
        msg.push_str(&format!(
            " Did you mean {first}{}?",
            if rest.is_empty() {
                String::new()
            } else {
                format!(", {}", rest.join(", "))
            }
        ));
    }
    msg
}

pub fn truncate_output(text: &str) -> String {
    let mut notes: Vec<String> = Vec::new();

    let total_lines = text.lines().count();
    let body = if total_lines > MAX_LINES {
        let dropped = total_lines - MAX_LINES;
        notes.push(format!("{dropped} lines"));
        let keep = MAX_LINES / 2;
        let nth_nl = |n: usize| {
            text.match_indices('\n')
                .nth(n)
                .map(|(i, _)| i + 1)
                .unwrap_or(text.len())
        };

        let head_end = nth_nl(keep - 1);
        let tail_start = nth_nl(total_lines - keep - 1);
        let mut b = String::with_capacity(head_end + text.len() - tail_start);
        b.push_str(&text[..head_end]);
        b.push_str(&text[tail_start..]);
        if b.ends_with('\n') {
            b.pop();
        }
        b
    } else {
        text.to_string()
    };

    if body.len() > MAX_BYTES {
        let dropped_bytes = body.len() - MAX_BYTES;
        notes.push(format!("{dropped_bytes} bytes"));
        let half = MAX_BYTES / 2;
        let head_end = body.floor_char_boundary(half);
        let tail_start = body.ceil_char_boundary(body.len() - half);
        format!(
            "{}\n{}\n{}",
            &body[..head_end],
            annotation(&notes),
            &body[tail_start..]
        )
    } else if notes.is_empty() {
        body
    } else {
        let mid = body.lines().count() / 2;
        let split = body
            .match_indices('\n')
            .nth(mid - 1)
            .map(|(i, _)| i + 1)
            .unwrap_or(body.len());
        let mut out = String::with_capacity(body.len() + 40);
        out.push_str(&body[..split]);
        out.push_str(&annotation(&notes));
        out.push('\n');
        out.push_str(&body[split..]);
        out
    }
}

pub fn truncate_error(text: &str) -> String {
    let truncated = truncate_output(text);
    if truncated.len() > MAX_ERROR_BYTES {
        let cut = truncated.floor_char_boundary(MAX_ERROR_BYTES);
        format!("{}\n[error truncated to 2KiB]", &truncated[..cut])
    } else {
        truncated
    }
}

pub fn finish(raw: String) -> ToolOutput {
    ToolOutput::ok(recoverable(&raw, truncate_output(&raw)))
}

pub fn fail(raw: String) -> ToolOutput {
    ToolOutput::error(recoverable(&raw, truncate_error(&raw)))
}

fn recoverable(original: &str, shown: String) -> String {
    if shown == original {
        return shown;
    }
    let Some(handle) = spill::store(original) else {
        return shown;
    };
    let lines = original.lines().count();
    format!("{shown}{}", spill::footer(&handle, original.len(), lines))
}

fn annotation(notes: &[String]) -> String {
    format!("[truncated {}]", notes.join(" / "))
}

pub fn get_str(args: &Value, key: &str) -> Result<String, ToolOutput> {
    match args.get(key) {
        Some(Value::String(s)) => Ok(s.clone()),
        Some(_) => Err(fail(format!("invalid argument '{key}': expected string"))),
        None => Err(fail(format!("missing required argument '{key}'"))),
    }
}

pub fn get_opt_u64(args: &Value, key: &str) -> Result<Option<u64>, ToolOutput> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => n.as_u64().map(Some).ok_or_else(|| {
            fail(format!(
                "invalid argument '{key}': expected non-negative integer"
            ))
        }),
        Some(_) => Err(fail(format!("invalid argument '{key}': expected integer"))),
    }
}

pub fn get_opt_bool(args: &Value, key: &str) -> Result<Option<bool>, ToolOutput> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        Some(_) => Err(fail(format!("invalid argument '{key}': expected boolean"))),
    }
}

pub fn resolve_path(cwd: &std::path::Path, p: &str) -> std::path::PathBuf {
    let path = std::path::Path::new(p);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    }
}
