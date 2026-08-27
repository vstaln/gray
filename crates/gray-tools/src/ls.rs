//! The `ls` tool: directory listing.

use async_trait::async_trait;
use gray_core::agent::{ToolContext, ToolOutput};
use gray_core::message::ToolDef;
use serde_json::{json, Value};

use crate::{fail, finish, get_opt_u64, resolve_path, Tool, MAX_BYTES};

const DEFAULT_LIMIT: usize = 500;

fn format_size(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{bytes}B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1}KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1}MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

fn truncate_head(content: &str, max_bytes: usize) -> (String, bool) {
    if content.len() <= max_bytes {
        return (content.to_string(), false);
    }
    let mut out_lines: Vec<&str> = Vec::new();
    let mut bytes = 0usize;
    for (i, line) in content.split('\n').enumerate() {
        let line_bytes = line.len() + if i > 0 { 1 } else { 0 };
        if bytes + line_bytes > max_bytes {
            break;
        }
        out_lines.push(line);
        bytes += line_bytes;
    }
    if out_lines.is_empty() {
        return (String::new(), true);
    }
    (out_lines.join("\n"), true)
}

pub const LS_SNIPPET: &str = "List directory contents";
pub const LS_GUIDELINES: &[&str] = &[];

/// List directory contents, sorted alphabetically with `/` suffix for directories.
pub struct LsTool;

#[async_trait]
impl Tool for LsTool {
    fn def(&self) -> ToolDef {
        ToolDef::new(
            "ls",
            format!(
                "List directory contents. Returns entries sorted alphabetically, with '/' suffix for directories. Includes dotfiles. Output is truncated to {DEFAULT_LIMIT} entries or {}KB (whichever is hit first).",
                MAX_BYTES / 1024
            ),
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Directory to list (default: current directory)" },
                    "limit": { "type": "integer", "description": "Maximum number of entries to return (default: 500)" }
                },
                "required": []
            }),
        )
    }

    fn prompt_snippet(&self) -> Option<&'static str> {
        Some(LS_SNIPPET)
    }

    fn prompt_guidelines(&self) -> Option<&'static [&'static str]> {
        Some(LS_GUIDELINES)
    }

    fn is_concurrency_safe(&self, _args: &Value) -> bool {
        true
    }

    async fn execute(&self, ctx: &ToolContext, args: Value) -> ToolOutput {
        let path_arg = match args.get("path") {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) => Some(s.clone()),
            Some(_) => return fail("invalid argument 'path': expected string".to_string()),
        };
        let effective_limit = match get_opt_u64(&args, "limit") {
            Ok(v) => v.map(|n| n as usize).unwrap_or(DEFAULT_LIMIT).max(1),
            Err(e) => return e,
        };

        let dir_path = resolve_path(&ctx.cwd, path_arg.as_deref().unwrap_or("."));

        // Existence check
        let meta = match tokio::fs::metadata(&dir_path).await {
            Ok(m) => m,
            Err(e) => return fail(format!("Path not found: {}: {e}", dir_path.display())),
        };
        if !meta.is_dir() {
            return fail(format!("Not a directory: {}", dir_path.display()));
        }

        let mut entries: Vec<String> = Vec::new();
        let mut rd = match tokio::fs::read_dir(&dir_path).await {
            Ok(rd) => rd,
            Err(e) => return fail(format!("Cannot read directory: {e}")),
        };

        while let Ok(Some(entry)) = rd.next_entry().await {
            let name = entry.file_name().to_string_lossy().to_string();
            entries.push(name);
        }

        // Sort case-insensitive
        entries.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()));

        // Build results with directory suffix, respecting limit
        let mut results: Vec<String> = Vec::new();
        let mut entry_limit_reached = false;

        for name in &entries {
            if results.len() >= effective_limit {
                entry_limit_reached = true;
                break;
            }
            let full = dir_path.join(name);
            let suffix = match tokio::fs::metadata(&full).await {
                Ok(m) if m.is_dir() => "/",
                Ok(_) => "",
                Err(_) => continue, // skip entries we cannot stat
            };
            results.push(format!("{name}{suffix}"));
        }
        // If we broke due to limit but there were more entries, mark reached
        if !entry_limit_reached && results.len() >= effective_limit && entries.len() > results.len() {
            entry_limit_reached = true;
        }
        // More precise: if total entries exceeds limit, mark reached
        if entries.len() > effective_limit && !entry_limit_reached {
            entry_limit_reached = true;
        }

        if results.is_empty() {
            return finish("(empty directory)".to_string());
        }

        let raw_output = results.join("\n");
        let (mut output, byte_truncated) = truncate_head(&raw_output, MAX_BYTES);

        let mut notices: Vec<String> = Vec::new();
        if entry_limit_reached {
            notices.push(format!(
                "{effective_limit} entries limit reached. Use limit={} for more",
                effective_limit * 2
            ));
        }
        if byte_truncated {
            notices.push(format!("{} limit reached", format_size(MAX_BYTES)));
        }
        if !notices.is_empty() {
            output.push_str("\n\n[");
            output.push_str(&notices.join(". "));
            output.push(']');
        }

        finish(output)
    }
}
