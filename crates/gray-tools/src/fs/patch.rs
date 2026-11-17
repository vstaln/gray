use std::collections::HashMap;
use std::path::{Path, PathBuf};

use gray_core::agent::ToolOutput;
use gray_core::tool_out::{get_str, resolve_path};
use serde_json::Value;

use super::edit::fold;
use super::{R, err, write_atomic};

pub(super) const DESCRIPTION: &str = "Edit files with a patch. `input` is the whole patch:
*** Begin Patch
*** Add File: <path>
+<every line of the new file, prefixed with +>
*** Delete File: <path>
*** Update File: <path>
*** Move to: <new path>   (optional, renames the file)
@@ <optional line that sits just above the change, e.g. a function signature>
 <context line, prefixed with one space>
-<removed line>
+<added line>
*** End of File   (optional, anchors the last chunk to the end of the file)
*** End Patch
Give about 3 lines of context around each change; add another @@ chunk for each separate change in a file. Paths are relative to the working directory. The patch applies whole or not at all.";

enum Hunk {
    Add(String, Vec<String>),
    Delete(String),
    Update(String, Option<String>, Vec<Chunk>),
}

#[derive(Default)]
struct Chunk {
    ctx: Option<String>,
    old: Vec<String>,
    new: Vec<String>,
    eof: bool,
}

impl Chunk {
    fn is_empty(&self) -> bool {
        self.old.is_empty() && self.new.is_empty()
    }
}

fn parse(input: &str) -> Result<Vec<Hunk>, String> {
    let all: Vec<&str> = input
        .trim()
        .lines()
        .map(|l| l.trim_end_matches('\r'))
        .collect();
    let mut lines = &all[..];
    if lines.len() >= 2 && lines[0].trim_start().starts_with("<<") {
        lines = &lines[1..lines.len() - 1];
    }
    if lines.first().map(|l| l.trim()) != Some("*** Begin Patch") {
        return Err("The first line of the patch must be '*** Begin Patch'.".into());
    }
    if lines.len() < 2 || lines.last().map(|l| l.trim()) != Some("*** End Patch") {
        return Err("The last line of the patch must be '*** End Patch'.".into());
    }
    let body = &lines[1..lines.len() - 1];
    let mut hunks = Vec::new();
    let mut i = 0;
    while i < body.len() {
        let head = body[i].trim();
        i += 1;
        if head.is_empty() {
            continue;
        }
        if let Some(p) = head.strip_prefix("*** Add File: ") {
            let mut out = Vec::new();
            while let Some(l) = body.get(i).and_then(|l| l.strip_prefix('+')) {
                out.push(l.to_string());
                i += 1;
            }
            hunks.push(Hunk::Add(p.trim().to_string(), out));
        } else if let Some(p) = head.strip_prefix("*** Delete File: ") {
            hunks.push(Hunk::Delete(p.trim().to_string()));
        } else if let Some(p) = head.strip_prefix("*** Update File: ") {
            let path = p.trim().to_string();
            let to = body
                .get(i)
                .and_then(|l| l.trim().strip_prefix("*** Move to: "))
                .map(|t| t.trim().to_string());
            if to.is_some() {
                i += 1;
            }
            let mut chunks = Vec::new();
            let mut cur = Chunk::default();
            while let Some(&l) = body.get(i) {
                if l.trim() == "*** End of File" {
                    cur.eof = true;
                    chunks.push(std::mem::take(&mut cur));
                    i += 1;
                    continue;
                }
                if l.starts_with("*** ") {
                    break;
                }
                i += 1;
                if let Some(rest) = l.strip_prefix("@@") {
                    if !cur.is_empty() {
                        chunks.push(std::mem::take(&mut cur));
                    }
                    let rest = rest.trim();
                    cur.ctx = (!rest.is_empty()).then(|| rest.to_string());
                } else if l.is_empty() {
                    cur.old.push(String::new());
                    cur.new.push(String::new());
                } else if let Some(t) = l.strip_prefix(' ') {
                    cur.old.push(t.to_string());
                    cur.new.push(t.to_string());
                } else if let Some(t) = l.strip_prefix('-') {
                    cur.old.push(t.to_string());
                } else if let Some(t) = l.strip_prefix('+') {
                    cur.new.push(t.to_string());
                } else {
                    return Err(format!(
                        "Unexpected line in the update of {path}: '{l}'. Every line must start with ' ', '-', '+' or '@@'."
                    ));
                }
            }
            if !cur.is_empty() || cur.ctx.is_some() {
                chunks.push(cur);
            }
            if chunks.iter().all(Chunk::is_empty) {
                return Err(format!("The update of {path} has no changes."));
            }
            hunks.push(Hunk::Update(path, to, chunks));
        } else {
            return Err(format!(
                "'{head}' is not a valid hunk header. Use '*** Add File: <path>', '*** Delete File: <path>' or '*** Update File: <path>'."
            ));
        }
    }
    if hunks.is_empty() {
        return Err("The patch changes no files.".into());
    }
    Ok(hunks)
}

fn seek(lines: &[String], pat: &[String], start: usize, eof: bool) -> Option<usize> {
    if pat.is_empty() {
        return Some(start.min(lines.len()));
    }
    if pat.len() > lines.len() {
        return None;
    }
    let last = lines.len() - pat.len();
    let norms: [fn(&str) -> String; 4] = [
        |s| s.to_string(),
        |s| s.trim_end().to_string(),
        |s| s.trim().to_string(),
        |s| s.trim().chars().map(fold).collect(),
    ];
    let mut starts: Vec<usize> = Vec::new();
    if eof {
        starts.push(last);
    }
    starts.extend(start.min(last + 1)..=last);
    norms.iter().find_map(|f| {
        starts
            .iter()
            .copied()
            .find(|&i| pat.iter().zip(&lines[i..]).all(|(p, l)| f(p) == f(l)))
    })
}

fn update(text: &str, chunks: &[Chunk], shown: &str) -> Result<String, String> {
    let crlf = text.contains("\r\n");
    let norm = text.replace("\r\n", "\n");
    let mut lines: Vec<String> = norm.split('\n').map(String::from).collect();
    if lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    let mut repl: Vec<(usize, usize, Vec<String>)> = Vec::new();
    let mut at = 0;
    for c in chunks {
        if let Some(ctx) = &c.ctx {
            match seek(&lines, std::slice::from_ref(ctx), at, false) {
                Some(i) => at = i + 1,
                None => return Err(format!("Failed to find context '{ctx}' in {shown}.")),
            }
        }
        if c.old.is_empty() {
            let i = if c.ctx.is_some() { at } else { lines.len() };
            repl.push((i, 0, c.new.clone()));
            continue;
        }
        let (mut old, mut new) = (&c.old[..], &c.new[..]);
        let mut found = seek(&lines, old, at, c.eof);
        if found.is_none() && old.last().is_some_and(String::is_empty) {
            old = &old[..old.len() - 1];
            if new.last().is_some_and(String::is_empty) {
                new = &new[..new.len() - 1];
            }
            found = seek(&lines, old, at, c.eof);
        }
        let Some(i) = found else {
            return Err(format!(
                "Failed to find these lines in {shown}:\n{}",
                c.old.join("\n")
            ));
        };
        repl.push((i, old.len(), new.to_vec()));
        at = i + old.len();
    }
    repl.sort_by_key(|r| r.0);
    for (i, n, new) in repl.into_iter().rev() {
        lines.splice(i..i + n, new);
    }
    let mut out = lines.join("\n");
    out.push('\n');
    Ok(if crlf { out.replace('\n', "\r\n") } else { out })
}

pub(super) fn run(cwd: &Path, args: &Value) -> R {
    let input = get_str(args, "input").or_else(|_| get_str(args, "patch"))?;
    let hunks = parse(&input).map_err(ToolOutput::error)?;
    let mut staged: HashMap<PathBuf, Option<String>> = HashMap::new();
    let mut order: Vec<PathBuf> = Vec::new();
    let mut summary = Vec::new();
    let current = |staged: &HashMap<PathBuf, Option<String>>, path: &Path, shown: &str| match staged
        .get(path)
    {
        Some(Some(t)) => Ok(t.clone()),
        Some(None) => Err(format!("{shown} was deleted earlier in this patch.")),
        None => std::fs::read_to_string(path).map_err(|e| format!("{shown}: {e}")),
    };
    let mut stage = |staged: &mut HashMap<PathBuf, Option<String>>, path: PathBuf, v| {
        if !order.contains(&path) {
            order.push(path.clone());
        }
        staged.insert(path, v);
    };
    for h in &hunks {
        match h {
            Hunk::Add(p, lines) => {
                let mut text = lines.join("\n");
                text.push('\n');
                stage(&mut staged, resolve_path(cwd, p), Some(text));
                summary.push(format!("A {p}"));
            }
            Hunk::Delete(p) => {
                let path = resolve_path(cwd, p);
                current(&staged, &path, p).map_err(ToolOutput::error)?;
                stage(&mut staged, path, None);
                summary.push(format!("D {p}"));
            }
            Hunk::Update(p, to, chunks) => {
                let path = resolve_path(cwd, p);
                let text = current(&staged, &path, p).map_err(ToolOutput::error)?;
                let out = update(&text, chunks, p).map_err(ToolOutput::error)?;
                match to {
                    Some(t) => {
                        stage(&mut staged, path, None);
                        stage(&mut staged, resolve_path(cwd, t), Some(out));
                        summary.push(format!("M {t}"));
                    }
                    None => {
                        stage(&mut staged, path, Some(out));
                        summary.push(format!("M {p}"));
                    }
                }
            }
        }
    }
    for path in &order {
        let shown = path.display();
        match &staged[path] {
            Some(text) => {
                if let Some(dir) = path.parent() {
                    std::fs::create_dir_all(dir)
                        .map_err(|e| ToolOutput::error(format!("{shown}: {e}")))?;
                }
                write_atomic(path, text.as_bytes())
                    .map_err(|e| ToolOutput::error(format!("{shown}: {e}")))?;
            }
            None => match std::fs::remove_file(path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                    return err(format!("{shown}: {e}"));
                }
                _ => {}
            },
        }
    }
    Ok(format!(
        "Success. Updated the following files:\n{}",
        summary.join("\n")
    ))
}
