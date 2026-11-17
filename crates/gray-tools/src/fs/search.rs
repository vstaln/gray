use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use globset::{Glob, GlobBuilder};
use gray_core::agent::{ToolContext, ToolOutput};
use gray_core::tool_out::{get_opt_bool, get_str, resolve_path};
use grep_regex::RegexMatcherBuilder;
use grep_searcher::sinks::Lossy;
use grep_searcher::{BinaryDetection, SearcherBuilder};
use ignore::{Walk, WalkBuilder};
use serde_json::Value;

use super::{R, cut, err};

const MAX_MATCHES: usize = 100;
const SCAN_CAP: usize = 1000;
const LINE_CHARS: usize = 300;
const MAX_PATHS: usize = 100;

fn root(ctx: &ToolContext, args: &Value) -> Result<PathBuf, ToolOutput> {
    match args
        .get("path")
        .and_then(Value::as_str)
        .filter(|p| !p.is_empty() && *p != ".")
    {
        None => Ok(ctx.cwd.clone()),
        Some(p) => {
            let root = resolve_path(&ctx.cwd, p);
            if root.exists() {
                Ok(root)
            } else {
                err(format!("Path not found: {p}"))
            }
        }
    }
}

fn walk(root: &Path) -> Walk {
    WalkBuilder::new(root)
        .require_git(false)
        .sort_by_file_path(|a, b| a.cmp(b))
        .build()
}

fn show(cwd: &Path, path: &Path) -> String {
    path.strip_prefix(cwd).unwrap_or(path).display().to_string()
}

pub(super) fn grep(ctx: &ToolContext, args: &Value) -> R {
    let pattern = get_str(args, "pattern")?;
    let mode = args
        .get("output_mode")
        .and_then(Value::as_str)
        .unwrap_or("content");
    if !matches!(mode, "content" | "files_with_matches" | "count") {
        return err(format!(
            "Invalid output_mode {mode:?}: use content, files_with_matches or count"
        ));
    }
    let content = mode == "content";
    let root = root(ctx, args)?;
    let matcher = RegexMatcherBuilder::new()
        .case_insensitive(get_opt_bool(args, "ignore_case")?.unwrap_or(false))
        .build(&pattern)
        .map_err(|e| ToolOutput::error(format!("Invalid regex: {e}")))?;
    let filter = match args.get("glob").and_then(Value::as_str) {
        Some(g) => Some(
            Glob::new(g)
                .map_err(|e| ToolOutput::error(format!("Invalid glob: {e}")))?
                .compile_matcher(),
        ),
        None => None,
    };
    let mut searcher = SearcherBuilder::new()
        .binary_detection(BinaryDetection::quit(0))
        .line_number(true)
        .build();
    let (mut hits, mut count, mut files) = (Vec::new(), 0, 0);
    for ent in walk(&root).flatten() {
        if ctx.cancel.is_cancelled() || count >= SCAN_CAP {
            break;
        }
        let path = ent.path();
        if !ent.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        if let Some(g) = &filter
            && !g.is_match(ent.file_name())
            && !g.is_match(path.strip_prefix(&root).unwrap_or(path))
        {
            continue;
        }
        let shown = show(&ctx.cwd, path);
        let mut in_file = 0;
        let _ = searcher.search_path(
            &matcher,
            path,
            Lossy(|n, line| {
                in_file += 1;
                if content {
                    count += 1;
                    if hits.len() < MAX_MATCHES {
                        hits.push(format!("{shown}:{n}: {}", cut(line.trim_end(), LINE_CHARS)));
                    }
                }
                Ok(count < SCAN_CAP && mode != "files_with_matches")
            }),
        );
        if !content && in_file > 0 {
            files += 1;
            count = files;
            if hits.len() < MAX_MATCHES {
                hits.push(if mode == "count" {
                    format!("{shown}:{in_file}")
                } else {
                    shown
                });
            }
        }
    }
    if hits.is_empty() {
        return Ok("No matches found.".into());
    }
    let mut out = hits.join("\n");
    if count > MAX_MATCHES {
        let plus = if count >= SCAN_CAP { "+" } else { "" };
        let unit = if content { "matches" } else { "files" };
        out.push_str(&format!(
            "\n[{MAX_MATCHES} of {count}{plus} {unit}; narrow the pattern or path]"
        ));
    }
    Ok(out)
}

pub(super) fn glob(ctx: &ToolContext, args: &Value) -> R {
    let pattern = get_str(args, "pattern")?;
    let pattern = pattern.trim_start_matches("./");
    let root = root(ctx, args)?;
    let matcher = GlobBuilder::new(pattern)
        .literal_separator(true)
        .build()
        .map_err(|e| ToolOutput::error(format!("Invalid glob: {e}")))?
        .compile_matcher();
    let by_name = !pattern.contains('/');
    let mut found: Vec<(SystemTime, PathBuf)> = Vec::new();
    for ent in walk(&root).flatten() {
        if ctx.cancel.is_cancelled() {
            break;
        }
        if !ent.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let path = ent.path();
        let hit = if by_name {
            matcher.is_match(ent.file_name())
        } else {
            matcher.is_match(path.strip_prefix(&root).unwrap_or(path))
        };
        if hit {
            let mtime = ent.metadata().ok().and_then(|m| m.modified().ok());
            found.push((mtime.unwrap_or(UNIX_EPOCH), path.to_path_buf()));
        }
    }
    if found.is_empty() {
        return Ok("No files found.".into());
    }
    found.sort_by(|a, b| b.0.cmp(&a.0));
    let total = found.len();
    let mut out: Vec<String> = found
        .iter()
        .take(MAX_PATHS)
        .map(|(_, p)| show(&ctx.cwd, p))
        .collect();
    if total > MAX_PATHS {
        out.push(format!(
            "[{MAX_PATHS} of {total} files; narrow the pattern or path]"
        ));
    }
    Ok(out.join("\n"))
}
