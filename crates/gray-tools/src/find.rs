//! The `find` tool: glob filename search. Respects .gitignore.

use std::path::Path;

use async_trait::async_trait;
use gray_core::agent::{ToolContext, ToolOutput};
use gray_core::message::ToolDef;
use serde_json::{Value, json};
use tokio::io::AsyncBufReadExt;
use tokio::process::Command;

use crate::{MAX_BYTES, Tool, fail, finish, get_opt_u64, get_str, resolve_path};

use crate::truncate::truncate_head;

const DEFAULT_LIMIT: usize = 1000;

fn relativize(result_path: &str, search_path: &Path) -> String {
    let rp = Path::new(result_path);
    let had_trailing_sep =
        result_path.ends_with('/') || result_path.ends_with(std::path::MAIN_SEPARATOR);
    let relative = if rp.is_absolute() {
        rp.strip_prefix(search_path).unwrap_or(rp).to_path_buf()
    } else {
        rp.to_path_buf()
    };
    let mut posix = relative.to_string_lossy().replace('\\', "/");
    if had_trailing_sep && !posix.ends_with('/') && !posix.is_empty() {
        posix.push('/');
    }
    if posix.is_empty() {
        result_path.to_string()
    } else {
        posix
    }
}

pub const FIND_SNIPPET: &str = "Find files by glob pattern (respects .gitignore)";
pub const FIND_GUIDELINES: &[&str] = &[];

/// Filename glob search. Respects .gitignore via `fd` when available,
/// otherwise falls back to a manual walk.
pub struct FindTool;

#[async_trait]
impl Tool for FindTool {
    fn def(&self) -> ToolDef {
        ToolDef::new(
            "find",
            format!(
                "Search for files by glob pattern. Returns matching file paths relative to the search directory. Respects .gitignore. Output is truncated to {DEFAULT_LIMIT} results or {}KB (whichever is hit first).",
                MAX_BYTES / 1024
            ),
            json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "Glob pattern to match files, e.g. '*.ts', '**/*.json', or 'src/**/*.spec.ts'" },
                    "path": { "type": "string", "description": "Directory to search in (default: current directory)" },
                    "limit": { "type": "integer", "description": "Maximum number of results (default: 1000)" }
                },
                "required": ["pattern"]
            }),
        )
    }

    fn prompt_snippet(&self) -> Option<&'static str> {
        Some(FIND_SNIPPET)
    }

    fn prompt_guidelines(&self) -> Option<&'static [&'static str]> {
        Some(FIND_GUIDELINES)
    }

    async fn execute(&self, ctx: &ToolContext, args: Value) -> ToolOutput {
        let pattern = match get_str(&args, "pattern") {
            Ok(p) => p,
            Err(e) => return e,
        };
        let search_dir = match args.get("path") {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) => Some(s.clone()),
            Some(_) => return fail("invalid argument 'path': expected string".to_string()),
        };
        let effective_limit = match get_opt_u64(&args, "limit") {
            Ok(v) => v.map(|n| n as usize).unwrap_or(DEFAULT_LIMIT).max(1),
            Err(e) => return e,
        };

        let search_path = resolve_path(&ctx.cwd, search_dir.as_deref().unwrap_or("."));

        match tokio::fs::metadata(&search_path).await {
            Ok(m) if m.is_dir() => {}
            Ok(_) => return fail(format!("Not a directory: {}", search_path.display())),
            Err(e) => return fail(format!("Path not found: {}: {e}", search_path.display())),
        }

        // Try fd first (preferred, respects .gitignore correctly).
        if let Some(output) = try_fd(&pattern, &search_path, effective_limit).await {
            return output;
        }

        // Fallback: manual recursive walk with simple glob matching.
        fallback_walk(&pattern, &search_path, effective_limit).await
    }
}

async fn try_fd(pattern: &str, search_path: &Path, effective_limit: usize) -> Option<ToolOutput> {
    // Probe fd availability quickly.
    let mut args: Vec<String> = vec![
        "--glob".to_string(),
        "--color=never".to_string(),
        "--hidden".to_string(),
    ];

    // Detect git repo to decide --no-require-git
    let mut inside_git = false;
    let mut cur = search_path.to_path_buf();
    loop {
        if tokio::fs::metadata(cur.join(".git")).await.is_ok() {
            inside_git = true;
            break;
        }
        match cur.parent() {
            Some(p) if p != cur => cur = p.to_path_buf(),
            _ => break,
        }
    }
    if !inside_git {
        args.push("--no-require-git".to_string());
    }
    args.push("--max-results".to_string());
    args.push(effective_limit.to_string());

    let mut effective_pattern = pattern.to_string();
    let needs_full_path = pattern.contains('/');
    if needs_full_path {
        args.push("--full-path".to_string());
        if !pattern.starts_with('/') && !pattern.starts_with("**/") && pattern != "**" {
            effective_pattern = format!("**/{pattern}");
        }
        #[cfg(windows)]
        {
            effective_pattern = effective_pattern.replace('/', "[/\\\\]");
        }
    }
    args.push("--".to_string());
    args.push(effective_pattern);
    args.push(search_path.to_string_lossy().to_string());

    let mut child = match Command::new("fd")
        .args(&args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(_) => return None,
    };

    let stdout = child.stdout.take()?;
    let stderr = child.stderr.take()?;
    let stderr_handle = tokio::spawn(async move {
        use tokio::io::AsyncReadExt;
        let mut buf = Vec::new();
        let mut r = stderr;
        let mut tmp = [0u8; 1024];
        loop {
            match r.read(&mut tmp).await {
                Ok(0) | Err(_) => break,
                Ok(n) => buf.extend_from_slice(&tmp[..n]),
            }
        }
        String::from_utf8_lossy(&buf).to_string()
    });

    let mut reader = tokio::io::BufReader::new(stdout).lines();
    let mut lines: Vec<String> = Vec::new();
    while let Ok(Some(line)) = reader.next_line().await {
        lines.push(line);
    }
    let status = child.wait().await.ok()?;
    let stderr_str = stderr_handle.await.unwrap_or_default();

    // If fd exited with error and produced no output, treat as failure and fall back.
    if let Some(code) = status.code()
        && code != 0
        && lines.is_empty()
    {
        // Check if fd is actually usable; if error is about missing fd, fall back.
        // Otherwise surface the error.
        let msg = stderr_str.trim();
        if !msg.is_empty() && lines.is_empty() {
            // If no output, let fallback handle it or return no matches.
            // Only return error if we clearly have no results and an error.
            // For now, fall through to fallback if we got nothing.
            if code != 0 && code != 1 {
                // Return error only if we have stderr and no fallback would help.
                // But still try fallback first by returning None? Let's surface error.
                return Some(fail(msg.to_string()));
            }
        }
    }

    if lines.is_empty() {
        return Some(finish("No files found matching pattern".to_string()));
    }

    let relativized: Vec<String> = lines
        .iter()
        .map(|l| {
            let trimmed = l.trim().trim_end_matches('\r').to_string();
            if trimmed.is_empty() {
                return String::new();
            }
            relativize(&trimmed, search_path)
        })
        .filter(|s| !s.is_empty())
        .collect();

    if relativized.is_empty() {
        return Some(finish("No files found matching pattern".to_string()));
    }

    let result_limit_reached = relativized.len() >= effective_limit;
    let raw_output = relativized.join("\n");
    let trunc = truncate_head(&raw_output);
    let mut output = trunc.content;

    let mut notices: Vec<String> = Vec::new();
    if result_limit_reached {
        notices.push(format!(
            "{effective_limit} results limit reached. Use limit={} for more, or refine pattern",
            effective_limit * 2
        ));
    }
    if trunc.truncated {
        notices.push(format!(
            "{} limit reached",
            crate::truncate::format_size(MAX_BYTES)
        ));
    }
    if !notices.is_empty() {
        output.push_str("\n\n[");
        output.push_str(&notices.join(". "));
        output.push(']');
    }

    Some(finish(output))
}


/// Fallback when `fd` is missing: recursive walk via the `ignore` crate
/// (real .gitignore handling) + `globset` matching with fd `--glob`
/// semantics (no `/` in pattern = basename match, else rel-path match).
async fn fallback_walk(pattern: &str, search_path: &Path, effective_limit: usize) -> ToolOutput {
    // Mirror the `--full-path` anchoring `try_fd` applies.
    let full_pattern = if pattern.contains('/')
        && !pattern.starts_with('/')
        && !pattern.starts_with("**/")
        && pattern != "**"
    {
        format!("**/{pattern}")
    } else {
        pattern.to_string()
    };
    let matcher = match globset::GlobBuilder::new(&full_pattern)
        .literal_separator(true)
        .build()
    {
        Ok(g) => g.compile_matcher(),
        Err(_) => return fail(format!("invalid glob pattern: {pattern}")),
    };
    let match_basename = !pattern.contains('/');

    let mut results: Vec<String> = Vec::new();
    let walker = ignore::WalkBuilder::new(search_path)
        .hidden(false) // fd --hidden parity: include dotfiles
        .require_git(false) // honor .gitignore outside repos, like before
        .filter_entry(|e| {
            // Always prune these, even when not ignored.
            !(e.file_type().is_some_and(|t| t.is_dir())
                && (e.file_name() == ".git" || e.file_name() == "node_modules"))
        })
        .build();
    for entry in walker {
        if results.len() >= effective_limit {
            break;
        }
        let Ok(entry) = entry else { continue };
        let Ok(rel) = entry.path().strip_prefix(search_path) else {
            continue;
        };
        if rel.as_os_str().is_empty() {
            continue; // the root itself
        }
        let is_dir = entry.file_type().is_some_and(|t| t.is_dir());
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        let hit = if match_basename {
            matcher.is_match_candidate(&globset::Candidate::new(entry.file_name()))
        } else {
            matcher.is_match(&rel_str)
        };
        if hit {
            // Directories keep a trailing slash, as before.
            if is_dir && !rel_str.ends_with('/') {
                results.push(format!("{rel_str}/"));
            } else {
                results.push(rel_str);
            }
        }
    }

    if results.is_empty() {
        return finish("No files found matching pattern".to_string());
    }

    results.sort();
    let result_limit_reached = results.len() >= effective_limit;
    let raw_output = results.join("\n");
    let trunc = truncate_head(&raw_output);
    let mut output = trunc.content;

    let mut notices: Vec<String> = Vec::new();
    if result_limit_reached {
        notices.push(format!(
            "{effective_limit} results limit reached. Use limit={} for more, or refine pattern",
            effective_limit * 2
        ));
    }
    if trunc.truncated {
        notices.push(format!(
            "{} limit reached",
            crate::truncate::format_size(MAX_BYTES)
        ));
    }
    if !notices.is_empty() {
        output.push_str("\n\n[");
        output.push_str(&notices.join(". "));
        output.push(']');
    }

    finish(output)
}
