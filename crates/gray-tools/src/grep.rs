//! The `grep` tool: content search via ripgrep (plain `--vimgrep` fast
//! path for match-only searches, `rg --json` when context lines are needed).

use std::path::Path;
use std::process::Stdio;

use async_trait::async_trait;
use gray_core::agent::{ToolContext, ToolOutput};
use gray_core::message::ToolDef;
use serde_json::{Value, json};
use tokio::io::AsyncBufReadExt;
use tokio::process::Command;

use crate::{MAX_BYTES, Tool, fail, finish, get_opt_bool, get_opt_u64, get_str, resolve_path};

const DEFAULT_LIMIT: usize = 100;
const GREP_MAX_LINE_LENGTH: usize = 500;

pub const GREP_SNIPPET: &str = "Search file contents for patterns (respects .gitignore)";
pub const GREP_GUIDELINES: &[&str] = &[];

/// Search file contents with ripgrep. Respects .gitignore.
pub struct GrepTool;

impl GrepTool {
    /// Match-only fast path over `rg --vimgrep` (`path:line:col:text`).
    ///
    /// Returns `None` when the fast path declines (rg missing → caller falls
    /// through to `--json`, which owns the "not installed" error; any other
    /// spawn failure fails loudly here). Output contract is identical to the
    /// `--json` path for match-only searches: same `rel:line: text` lines,
    /// same limit notice, same truncation behavior.
    #[allow(clippy::too_many_arguments)]
    async fn execute_vimgrep(
        &self,
        ctx: &ToolContext,
        pattern: &str,
        search_path: &Path,
        is_dir: bool,
        glob: &Option<String>,
        ignore_case: bool,
        literal: bool,
        effective_limit: usize,
    ) -> Option<ToolOutput> {
        let mut cmd = Command::new("rg");
        cmd.arg("--vimgrep").arg("--color=never").arg("--hidden");
        if ignore_case {
            cmd.arg("--ignore-case");
        }
        if literal {
            cmd.arg("--fixed-strings");
        }
        if let Some(g) = glob {
            cmd.arg("--glob").arg(g);
        }
        cmd.arg("--").arg(pattern).arg(search_path);
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                if e.kind() == std::io::ErrorKind::NotFound {
                    return None; // `--json` path reports "not installed".
                }
                return Some(fail(format!("Failed to run ripgrep: {e}")));
            }
        };
        let stdout = child.stdout.take().expect("stdout piped");
        let stderr = child.stderr.take().expect("stderr piped");
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

        let mut matches: Vec<(String, usize, String)> = Vec::new();
        let mut match_count = 0usize;
        let mut match_limit_reached = false;
        let mut cancelled = false;
        let mut reader = tokio::io::BufReader::new(stdout).lines();
        loop {
            tokio::select! {
                line_res = reader.next_line() => {
                    let Some(line) = line_res.unwrap_or(None) else { break };
                    if line.trim().is_empty() {
                        continue;
                    }
                    if match_count >= effective_limit {
                        break;
                    }
                    // `path:line:col:text` — split off the first three fields;
                    // the text keeps any further colons. Mirrors the `--json`
                    // path: only lines with a real path + line number survive.
                    let mut parts = line.splitn(4, ':');
                    let (Some(fp), Some(no), Some(_col), Some(text)) =
                        (parts.next(), parts.next(), parts.next(), parts.next())
                    else {
                        continue;
                    };
                    let line_number: usize = match no.parse() {
                        Ok(n) if n > 0 => n,
                        _ => continue,
                    };
                    if fp.is_empty() {
                        continue;
                    }
                    match_count += 1;
                    matches.push((fp.to_string(), line_number, text.to_string()));
                    if match_count >= effective_limit {
                        match_limit_reached = true;
                        let _ = child.kill().await;
                        break;
                    }
                }
                _ = ctx.cancel.cancelled() => {
                    cancelled = true;
                    let _ = child.kill().await;
                    break;
                }
            }
        }

        let status = match child.wait().await {
            Ok(s) => s,
            Err(e) => return Some(fail(format!("ripgrep wait failed: {e}"))),
        };
        let stderr_str = if cancelled {
            stderr_handle.abort();
            String::new()
        } else {
            stderr_handle.await.unwrap_or_default()
        };
        if cancelled {
            return Some(finish("cancelled by user".to_string()));
        }
        // rg exit codes: 0 = matches, 1 = no matches, 2+ = error. A limit
        // kill reads as terminated: fine, same as the `--json` path.
        if !match_limit_reached
            && let Some(code) = status.code()
            && code != 0
            && code != 1
        {
            let msg = stderr_str.trim();
            if msg.is_empty() {
                return Some(fail(format!("ripgrep exited with code {code}")));
            }
            return Some(fail(msg.to_string()));
        }

        if matches.is_empty() {
            return Some(finish("No matches found".to_string()));
        }

        // Same assembly as the `--json` path: `rel:line: text` + notices.
        let mut output_lines: Vec<String> = Vec::new();
        let mut lines_truncated = false;
        for (file_path, line_number, raw) in &matches {
            let rel = relativize(search_path, file_path, is_dir);
            let sanitized = raw
                .replace("\r\n", "\n")
                .replace('\r', "")
                .trim_end_matches('\n')
                .to_string();
            let (text, was_truncated) = truncate_line(&sanitized);
            if was_truncated {
                lines_truncated = true;
            }
            output_lines.push(format!("{rel}:{line_number}: {text}"));
        }
        let raw_output = output_lines.join("\n");
        let trunc = truncate_head(&raw_output);
        let mut output = trunc.content;
        let mut notices: Vec<String> = Vec::new();
        if match_limit_reached {
            notices.push(format!(
                "{effective_limit} matches limit reached. Use limit={} for more, or refine pattern",
                effective_limit * 2
            ));
        }
        if trunc.truncated {
            notices.push(format!(
                "{} limit reached",
                crate::truncate::format_size(MAX_BYTES)
            ));
        }
        if lines_truncated {
            notices.push(format!(
                "Some lines truncated to {GREP_MAX_LINE_LENGTH} chars. Use read tool to see full lines"
            ));
        }
        append_notices(&mut output, &notices);
        Some(finish(output))
    }
}

fn truncate_line(line: &str) -> (String, bool) {
    if line.chars().count() <= GREP_MAX_LINE_LENGTH {
        return (line.to_string(), false);
    }
    let truncated: String = line.chars().take(GREP_MAX_LINE_LENGTH).collect();
    (format!("{truncated}... [truncated]"), true)
}

use crate::truncate::{append_notices, truncate_head};

fn relativize(search_path: &Path, file_path: &str, is_dir: bool) -> String {
    let fp = Path::new(file_path);
    if is_dir && let Ok(rel) = fp.strip_prefix(search_path) {
        let s = rel.to_string_lossy().replace('\\', "/");
        if s.is_empty() {
            return fp
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| file_path.to_string());
        }
        return s;
    }
    fp.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| file_path.to_string())
}

#[async_trait]
impl Tool for GrepTool {
    fn def(&self) -> ToolDef {
        ToolDef::new(
            "grep",
            format!(
                "Search file contents for a pattern. Returns matching lines with file paths and line numbers. Respects .gitignore. Output is truncated to {DEFAULT_LIMIT} matches or {}KB (whichever is hit first). Long lines are truncated to {GREP_MAX_LINE_LENGTH} chars.",
                MAX_BYTES / 1024
            ),
            json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "Search pattern (regex or literal string)" },
                    "path": { "type": "string", "description": "Directory or file to search (default: current directory)" },
                    "glob": { "type": "string", "description": "Filter files by glob pattern, e.g. '*.ts' or '**/*.spec.ts'" },
                    "ignoreCase": { "type": "boolean", "description": "Case-insensitive search (default: false)" },
                    "literal": { "type": "boolean", "description": "Treat pattern as literal string instead of regex (default: false)" },
                    "context": { "type": "integer", "description": "Number of lines to show before and after each match (default: 0)" },
                    "limit": { "type": "integer", "description": "Maximum number of matches to return (default: 100)" }
                },
                "required": ["pattern"]
            }),
        )
    }

    fn prompt_snippet(&self) -> Option<&str> {
        Some(GREP_SNIPPET)
    }

    fn prompt_guidelines(&self) -> Option<&'static [&'static str]> {
        Some(GREP_GUIDELINES)
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
        let glob = match args.get("glob") {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) => Some(s.clone()),
            Some(_) => return fail("invalid argument 'glob': expected string".to_string()),
        };
        let ignore_case = match get_opt_bool(&args, "ignoreCase") {
            Ok(v) => v.unwrap_or(false),
            Err(e) => return e,
        };
        let literal = match get_opt_bool(&args, "literal") {
            Ok(v) => v.unwrap_or(false),
            Err(e) => return e,
        };
        let context = match get_opt_u64(&args, "context") {
            Ok(v) => v.unwrap_or(0) as usize,
            Err(e) => return e,
        };
        let effective_limit = match get_opt_u64(&args, "limit") {
            Ok(v) => v.map(|n| n as usize).unwrap_or(DEFAULT_LIMIT).max(1),
            Err(e) => return e,
        };

        let search_path = resolve_path(&ctx.cwd, search_dir.as_deref().unwrap_or("."));

        let is_dir = match tokio::fs::metadata(&search_path).await {
            Ok(m) => m.is_dir(),
            Err(e) => return fail(format!("Path not found: {}: {e}", search_path.display())),
        };

        // Fast path (match-only searches): plain `--vimgrep` output parses
        // with one split per line instead of one JSON document per match.
        // Byte-identical output or this lane doesn't ship (see
        // `fast_path_parity` test). Context searches keep `--json` below.
        if context == 0
            && let Some(output) = self
                .execute_vimgrep(
                    ctx,
                    &pattern,
                    &search_path,
                    is_dir,
                    &glob,
                    ignore_case,
                    literal,
                    effective_limit,
                )
                .await
        {
            return output;
        }

        let mut cmd = Command::new("rg");
        cmd.arg("--json")
            .arg("--line-number")
            .arg("--color=never")
            .arg("--hidden");
        if context > 0 {
            cmd.arg("--context").arg(context.to_string());
        }
        if ignore_case {
            cmd.arg("--ignore-case");
        }
        if literal {
            cmd.arg("--fixed-strings");
        }
        if let Some(g) = &glob {
            cmd.arg("--glob").arg(g);
        }
        cmd.arg("--").arg(&pattern).arg(&search_path);
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                if e.kind() == std::io::ErrorKind::NotFound {
                    return fail(
                        "ripgrep (rg) is not available and could not be found on PATH".to_string(),
                    );
                }
                return fail(format!("Failed to run ripgrep: {e}"));
            }
        };

        let stdout = child.stdout.take().expect("stdout piped");
        let stderr = child.stderr.take().expect("stderr piped");

        // Collect stderr concurrently
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
        // (path, line_number, text, is_match)
        let mut matches: Vec<(String, usize, Option<String>, bool)> = Vec::new();
        let mut match_count: usize = 0;
        let mut match_limit_reached = false;
        let mut cancelled = false;

        loop {
            tokio::select! {
                line_res = reader.next_line() => {
                    let Some(line) = line_res.unwrap_or(None) else { break };
                    if line.trim().is_empty() {
                        continue;
                    }
                    if match_count >= effective_limit {
                        break;
                    }
                    let event: Value = match serde_json::from_str(&line) {
                        Ok(v) => v,
                        Err(_) => continue,
                    };
                    let kind = event.get("type").and_then(|t| t.as_str());
                    let is_match = kind == Some("match");
                    // Context events only exist when `--context` was passed.
                    if !is_match && !(context > 0 && kind == Some("context")) {
                        continue;
                    }
                    if is_match {
                        match_count += 1;
                    }
                    let data = &event["data"];
                    let file_path = data
                        .get("path")
                        .and_then(|p| p.get("text"))
                        .and_then(|t| t.as_str())
                        .unwrap_or("")
                        .to_string();
                    let line_number = data
                        .get("line_number")
                        .and_then(|n| n.as_u64())
                        .unwrap_or(0) as usize;
                    let line_text = data
                        .get("lines")
                        .and_then(|l| l.get("text"))
                        .and_then(|t| t.as_str())
                        .map(|s| s.to_string());
                    if !file_path.is_empty() && line_number > 0 {
                        matches.push((file_path, line_number, line_text, is_match));
                    }
                    if is_match && match_count >= effective_limit {
                        match_limit_reached = true;
                        let _ = child.kill().await;
                        break;
                    }
                }
                _ = ctx.cancel.cancelled() => {
                    cancelled = true;
                    let _ = child.kill().await;
                    break;
                }
            }
        }

        // Always reap: normal EOF, limit kill, and cancel kill all land here.
        let status = match child.wait().await {
            Ok(s) => s,
            Err(e) => return fail(format!("ripgrep wait failed: {e}")),
        };
        // Cancel must not hang on a grandchild-held stderr pipe.
        let stderr_str = if cancelled {
            stderr_handle.abort();
            String::new()
        } else {
            stderr_handle.await.unwrap_or_default()
        };
        if cancelled {
            return finish("cancelled by user".to_string());
        }

        // rg exit codes: 0 = matches found, 1 = no matches, 2+ = error
        if !match_limit_reached {
            if let Some(code) = status.code() {
                if code != 0 && code != 1 {
                    let msg = stderr_str.trim();
                    if msg.is_empty() {
                        return fail(format!("ripgrep exited with code {code}"));
                    } else {
                        return fail(msg.to_string());
                    }
                }
            } else if !status.success() {
                // killed due to limit is ok; otherwise treat as abort
                if !match_limit_reached {
                    return fail("ripgrep was terminated".to_string());
                }
            }
        }

        if matches.is_empty() {
            return finish("No matches found".to_string());
        }

        // Format matches: `file:line: text`, context lines `file-line- text`.
        let mut output_lines: Vec<String> = Vec::new();
        let mut lines_truncated = false;

        for (file_path, line_number, line_text, is_match) in &matches {
            let rel = relativize(&search_path, file_path, is_dir);
            let Some(raw) = line_text else {
                if *is_match {
                    output_lines.push(format!("{rel}:{line_number}: (unable to read line)"));
                }
                continue;
            };
            let sanitized = raw
                .replace("\r\n", "\n")
                .replace('\r', "")
                .trim_end_matches('\n')
                .to_string();
            let (text, was_truncated) = truncate_line(&sanitized);
            if was_truncated {
                lines_truncated = true;
            }
            if *is_match {
                output_lines.push(format!("{rel}:{line_number}: {text}"));
            } else {
                output_lines.push(format!("{rel}-{line_number}- {text}"));
            }
        }

        let raw_output = output_lines.join("\n");
        let trunc = truncate_head(&raw_output);
        let mut output = trunc.content;

        let mut notices: Vec<String> = Vec::new();
        if match_limit_reached {
            notices.push(format!(
                "{effective_limit} matches limit reached. Use limit={} for more, or refine pattern",
                effective_limit * 2
            ));
        }
        if trunc.truncated {
            notices.push(format!(
                "{} limit reached",
                crate::truncate::format_size(MAX_BYTES)
            ));
        }
        if lines_truncated {
            notices.push(format!(
                "Some lines truncated to {GREP_MAX_LINE_LENGTH} chars. Use read tool to see full lines"
            ));
        }
        append_notices(&mut output, &notices);

        finish(output)
    }
}
