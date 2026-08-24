//! The `read` tool: reads a UTF-8 text file with optional line windowing.

use async_trait::async_trait;
use gray_core::agent::{ToolContext, ToolOutput};
use gray_core::message::ToolDef;
use serde_json::json;
use serde_json::Value;

use crate::{fail, finish, get_opt_u64, get_str, resolve_path, Tool};

/// Lines returned when no `limit` is given.
const DEFAULT_LIMIT: u64 = 2000;

/// Reads a text file (`path`, optional 1-based `offset`, optional `limit`).
pub struct ReadTool;

#[async_trait]
impl Tool for ReadTool {
    fn def(&self) -> ToolDef {
        ToolDef::new(
            "read",
            "Read a UTF-8 text file. Returns file contents, capped at \
             2000 lines / 50 KiB.",
            json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "File path (absolute or relative to cwd)"
                    },
                    "offset": {
                        "type": "integer",
                        "description": "1-based starting line number"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of lines to return (default 2000)"
                    }
                },
                "required": ["path"]
            }),
        )
    }

    // Pure read: safe to run alongside other tools.
    async fn execute(&self, ctx: &ToolContext, args: Value) -> ToolOutput {
        let path = match get_str(&args, "path") {
            Ok(p) => p,
            Err(e) => return e,
        };
        let offset = match get_opt_u64(&args, "offset") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let limit = match get_opt_u64(&args, "limit") {
            Ok(v) => v,
            Err(e) => return e,
        };

        let full = resolve_path(&ctx.cwd, &path);
        let data = match tokio::fs::read(&full).await {
            Ok(d) => d,
            Err(e) => return fail(format!("read failed for {}: {e}", full.display())),
        };
        let text = match String::from_utf8(data) {
            Ok(t) => t,
            Err(_) => return fail(format!("{}: not valid UTF-8 (binary file?)", full.display())),
        };

        let start = offset.unwrap_or(1).max(1).saturating_sub(1) as usize;
        let limit = limit.unwrap_or(DEFAULT_LIMIT) as usize;
        let selected: Vec<&str> = text.lines().skip(start).take(limit).collect();
        if start > 0 && start >= text.lines().count() {
            return fail(format!(
                "offset {start} is past the end of {} ({} lines)",
                full.display(),
                text.lines().count()
            ));
        }

        let mut out = selected.join("\n");
        if text.ends_with('\n') && !out.is_empty() {
            out.push('\n');
        }
        finish(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(dir: &std::path::Path) -> ToolContext {
        ToolContext { cwd: dir.to_path_buf(), cancel: Default::default() }
    }

    #[tokio::test]
    async fn round_trips_file_contents() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "line1\nline2\nline3\n").unwrap();
        let out = ReadTool.execute(&ctx(dir.path()), json!({"path": "a.txt"})).await;
        assert!(!out.is_error, "{}", out.content);
        assert_eq!(out.content, "line1\nline2\nline3\n");
    }

    #[tokio::test]
    async fn absolute_paths_work() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("abs.txt");
        std::fs::write(&file, "hi").unwrap();
        let out =
            ReadTool.execute(&ctx(dir.path()), json!({"path": file.to_str().unwrap()})).await;
        assert!(!out.is_error, "{}", out.content);
        assert_eq!(out.content, "hi");
    }

    #[tokio::test]
    async fn honors_offset_and_limit() {
        let dir = tempfile::tempdir().unwrap();
        let body: String = (1..=10).map(|i| format!("line{i}\n")).collect();
        std::fs::write(dir.path().join("a.txt"), body).unwrap();
        let out = ReadTool
            .execute(&ctx(dir.path()), json!({"path": "a.txt", "offset": 3, "limit": 2}))
            .await;
        assert!(!out.is_error, "{}", out.content);
        assert_eq!(out.content, "line3\nline4\n");
    }

    #[tokio::test]
    async fn missing_file_is_error_not_panic() {
        let dir = tempfile::tempdir().unwrap();
        let out = ReadTool.execute(&ctx(dir.path()), json!({"path": "nope.txt"})).await;
        assert!(out.is_error);
        assert!(out.content.contains("read failed"));
    }

    #[tokio::test]
    async fn binary_content_is_rejected_cleanly() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("bin"), [0xFF, 0xFE, 0x00, 0x01]).unwrap();
        let out = ReadTool.execute(&ctx(dir.path()), json!({"path": "bin"})).await;
        assert!(out.is_error);
        assert!(out.content.contains("UTF-8"));
    }

    #[tokio::test]
    async fn missing_path_argument_is_error() {
        let dir = tempfile::tempdir().unwrap();
        let out = ReadTool.execute(&ctx(dir.path()), json!({})).await;
        assert!(out.is_error);
        assert!(out.content.contains("missing required argument 'path'"));
    }
}
