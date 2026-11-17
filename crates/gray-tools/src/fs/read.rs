use std::io::{BufRead, BufReader, ErrorKind, Read, Seek};
use std::path::Path;

use gray_core::agent::ToolOutput;
use gray_core::tool_out::{get_opt_u64, get_str, resolve_path};
use serde_json::Value;

use super::{R, cut, err, not_found};

const MAX_LINES: usize = 2000;
const MAX_BYTES: usize = 50 * 1024;
const MAX_CHARS: usize = 2000;
const MAX_ENTRIES: usize = 500;

pub(super) fn run(cwd: &Path, args: &Value, tab: bool) -> R {
    let p = get_str(args, "path")?;
    let offset = get_opt_u64(args, "offset")?.unwrap_or(1).max(1) as usize;
    let limit = get_opt_u64(args, "limit")?.map_or(MAX_LINES, |l| (l as usize).clamp(1, MAX_LINES));
    let path = resolve_path(cwd, &p);
    let io = |e: std::io::Error| match e.kind() {
        ErrorKind::NotFound => not_found(&p, &path),
        _ => ToolOutput::error(format!("{p}: {e}")),
    };
    if std::fs::metadata(&path).map_err(io)?.is_dir() {
        return list(&path).map_err(io);
    }
    let mut file = std::fs::File::open(&path).map_err(io)?;
    let mut head = [0u8; 8192];
    let n = file.read(&mut head).map_err(io)?;
    if head[..n].contains(&0) {
        return err(format!("Binary file: {p}"));
    }
    file.rewind().map_err(io)?;
    let mut reader = BufReader::new(file);
    let (mut buf, mut out) = (Vec::new(), String::new());
    let (mut total, mut last, mut full) = (0, 0, false);
    loop {
        buf.clear();
        if reader.read_until(b'\n', &mut buf).map_err(io)? == 0 {
            break;
        }
        total += 1;
        if full || total < offset {
            continue;
        }
        let text = String::from_utf8_lossy(&buf);
        let body = cut(text.trim_end_matches(['\r', '\n']), MAX_CHARS);
        let line = if tab {
            format!("{total:>6}\t{body}\n")
        } else {
            format!("{total}: {body}\n")
        };
        if total - offset >= limit || out.len() + line.len() > MAX_BYTES {
            full = true;
            continue;
        }
        out.push_str(&line);
        last = total;
    }
    if total == 0 {
        return Ok("(empty file)".into());
    }
    if offset > total {
        return err(format!(
            "offset {offset} is beyond the end of {p} ({total} lines)"
        ));
    }
    out.pop();
    if last < total {
        out.push_str(&format!(
            "\n[showing lines {offset}-{last} of {total}; continue with offset={}]",
            last + 1
        ));
    }
    Ok(out)
}

fn list(path: &Path) -> std::io::Result<String> {
    let mut names: Vec<String> = std::fs::read_dir(path)?
        .flatten()
        .map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if e.path().is_dir() { name + "/" } else { name }
        })
        .collect();
    names.sort();
    let total = names.len();
    names.truncate(MAX_ENTRIES);
    let mut out = names.join("\n");
    if total == 0 {
        out.push_str("(empty directory)");
    } else if total > MAX_ENTRIES {
        out.push_str(&format!("\n[showing {MAX_ENTRIES} of {total} entries]"));
    }
    Ok(out)
}
