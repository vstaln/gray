//! The `grep` tool: content search via ripgrep (`rg --json`).

use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;

use async_trait::async_trait;
use gray_core::agent::{ToolContext, ToolOutput};
use gray_core::message::ToolDef;
use serde_json::{json, Value};
use tokio::io::AsyncBufReadExt;
use tokio::process::Command;

use crate::{fail, finish, get_opt_bool, get_opt_u64, get_str, resolve_path, Tool, MAX_BYTES};

const DEFAULT_LIMIT: usize = 100;
const GREP_MAX_LINE_LENGTH: usize = 500;

pub const GREP_SNIPPET: &str = "Search file contents for patterns (respects .gitignore)";
pub const GREP_GUIDELINES: &[&str] = &[];

/// Search file contents with ripgrep. Respects .gitignore.
pub struct GrepTool;

fn truncate_line(line: &str) -> (String, bool) {
    if line.chars().count() <= GREP_MAX_LINE_LENGTH {
        return (line.to_string(), false);
    }
    let truncated: String = line.chars().take(GREP_MAX_LINE_LENGTH).collect();
    (format!("{truncated}... [truncated]"), true)
}

use crate::truncate::truncate_head;

fn relativize(search_path: &Path, file_path: &str, is_dir: bool) -> String {
    let fp = Path::new(file_path);
    if is_dir {
        if let Ok(rel) = fp.strip_prefix(search_path) {
            let s = rel.to_string_lossy().replace('\\', "/");
            if s.is_empty() {
                return fp.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| file_path.to_string());
            }
            return s;
        }
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

    fn prompt_snippet(&self) -> Option<&'static str> {
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

        let mut cmd = Command::new("rg");
        cmd.arg("--json")
            .arg("--line-number")
            .arg("--color=never")
            .arg("--hidden");
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
            .stderr(Stdio::piped());

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                if e.kind() == std::io::ErrorKind::NotFound {
                    return fail("ripgrep (rg) is not available and could not be found on PATH".to_string());
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
        let mut matches: Vec<(String, usize, Option<String>)> = Vec::new();
        let mut match_count: usize = 0;
        let mut match_limit_reached = false;

        while let Ok(Some(line)) = reader.next_line().await {
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
            if event.get("type").and_then(|t| t.as_str()) != Some("match") {
                continue;
            }
            match_count += 1;
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
                matches.push((file_path, line_number, line_text));
            }
            if match_count >= effective_limit {
                match_limit_reached = true;
                let _ = child.kill().await;
                break;
            }
        }

        let status = match child.wait().await {
            Ok(s) => s,
            Err(e) => return fail(format!("ripgrep wait failed: {e}")),
        };
        let stderr_str = stderr_handle.await.unwrap_or_default();

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

        // Format matches
        let mut output_lines: Vec<String> = Vec::new();
        let mut lines_truncated = false;
        let context_val = context;

        if context_val == 0 {
            for (file_path, line_number, line_text) in &matches {
                let rel = relativize(&search_path, file_path, is_dir);
                if let Some(raw) = line_text {
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
                } else {
                    // Fallback: read file to get line (should not happen)
                    let rel2 = rel.clone();
                    output_lines.push(format!("{rel2}:{line_number}: (unable to read line)"));
                }
            }
        } else {
            // Context mode: read files and show surrounding lines
            let mut file_cache: HashMap<String, Vec<String>> = HashMap::new();
            for (file_path, line_number, _line_text) in &matches {
                let rel = relativize(&search_path, file_path, is_dir);
                let lines: Vec<String> = if let Some(cached) = file_cache.get(file_path) {
                    cached.clone()
                } else {
                    match tokio::fs::read_to_string(file_path).await {
                        Ok(content) => {
                            let v: Vec<String> = content
                                .replace("\r\n", "\n")
                                .replace('\r', "\n")
                                .split('\n')
                                .map(|s| s.to_string())
                                .collect();
                            file_cache.insert(file_path.clone(), v.clone());
                            v
                        }
                        Err(_) => {
                            file_cache.insert(file_path.clone(), Vec::new());
                            Vec::new()
                        }
                    }
                };
                if lines.is_empty() {
                    output_lines.push(format!("{rel}:{line_number}: (unable to read file)"));
                    continue;
                }
                let start = if *line_number > context_val {
                    line_number - context_val
                } else {
                    1
                };
                let end = (*line_number + context_val).min(lines.len());
                for current in start..=end {
                    let raw = lines.get(current - 1).map(|s| s.as_str()).unwrap_or("");
                    let sanitized = raw.replace('\r', "");
                    let (text, was_truncated) = truncate_line(&sanitized);
                    if was_truncated {
                        lines_truncated = true;
                    }
                    if current == *line_number {
                        output_lines.push(format!("{rel}:{current}: {text}"));
                    } else {
                        output_lines.push(format!("{rel}-{current}- {text}"));
                    }
                }
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
            notices.push(format!("{} limit reached", crate::truncate::format_size(MAX_BYTES)));
        }
        if lines_truncated {
            notices.push(format!(
                "Some lines truncated to {GREP_MAX_LINE_LENGTH} chars. Use read tool to see full lines"
            ));
        }
        if !notices.is_empty() {
            output.push_str("\n\n[");
            output.push_str(&notices.join(". "));
            output.push(']');
        }

        finish(output)
    }
}
