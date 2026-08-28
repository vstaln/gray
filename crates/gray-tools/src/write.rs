//! The `write` tool: creates or overwrites a text file.

use async_trait::async_trait;
use gray_core::agent::{ToolContext, ToolOutput};
use gray_core::message::ToolDef;
use serde_json::json;
use serde_json::Value;

use crate::file_mutation_queue::with_file_mutation_queue;
use crate::{fail, finish, get_str, resolve_path, Tool};

pub const WRITE_SNIPPET: &str = "Create or overwrite files";
pub const WRITE_GUIDELINES: &[&str] = &["Use write only for new files or complete rewrites."];

/// Writes `content` to `path`, creating parent directories as needed.
pub struct WriteTool;

#[async_trait]
impl Tool for WriteTool {
    fn def(&self) -> ToolDef {
        ToolDef::new(
            "write",
            "Create or overwrite a file with the given content. Parent \
             directories are created automatically.",
            json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "File path (absolute or relative to cwd)"
                    },
                    "file_path": {
                        "type": "string",
                        "description": "Alias for path"
                    },
                    "content": {
                        "type": "string",
                        "description": "Full file contents (default: empty string)"
                    },
                    "contents": {
                        "type": "string",
                        "description": "Alias for content"
                    },
                    "text": {
                        "type": "string",
                        "description": "Alias for content"
                    }
                },
                "required": ["path"]
            }),
        )
    }

    fn prompt_snippet(&self) -> Option<&'static str> {
        Some(WRITE_SNIPPET)
    }

    fn prompt_guidelines(&self) -> Option<&'static [&'static str]> {
        Some(WRITE_GUIDELINES)
    }

    // Mutates the filesystem: never run in parallel.
    fn is_concurrency_safe(&self, _args: &Value) -> bool {
        false
    }

    async fn execute(&self, ctx: &ToolContext, args: Value) -> ToolOutput {
        let path = args.get("path")
            .or_else(|| args.get("file_path"))
            .or_else(|| args.get("filePath"))
            .or_else(|| args.get("file"))
            .or_else(|| args.get("filename"))
            .or_else(|| args.get("target"))
            .or_else(|| args.get("destination"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let path = match path {
            Some(p) => p,
            None => return fail("missing required argument 'path'".to_string()),
        };

        let content = args.get("content")
            .or_else(|| args.get("contents"))
            .or_else(|| args.get("text"))
            .or_else(|| args.get("code"))
            .or_else(|| args.get("body"))
            .or_else(|| args.get("data"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let full = resolve_path(&ctx.cwd, &path);
        with_file_mutation_queue(full.clone(), || {
            let full = full.clone();
            let content = content.clone();
            async move {
                if let Some(parent) = full.parent()
                    && let Err(e) = tokio::fs::create_dir_all(parent).await
                {
                    return fail(format!("write failed for {}: {e}", full.display()));
                }
                let existed = tokio::fs::metadata(&full).await.is_ok();
                let old = if existed { tokio::fs::read_to_string(&full).await.unwrap_or_default() } else { String::new() };
                match tokio::fs::write(&full, content.as_bytes()).await {
                    Ok(()) => {
                        if existed && !old.is_empty() {
                            let patch = crate::edit_diff::generate_unified_patch(&path, &old, &content, 3);
                            if patch.is_empty() { finish(format!("wrote {} bytes to {} (no change)", content.len(), full.display())) }
                            else { finish(patch) }
                        } else {
                            finish(format!("wrote {} bytes to {}", content.len(), full.display()))
                        }
                    }
                    Err(e) => fail(format!("write failed for {}: {e}", full.display())),
                }
            }
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(dir: &std::path::Path) -> ToolContext {
        ToolContext { cwd: dir.to_path_buf(), cancel: Default::default() }
    }

    #[tokio::test]
    async fn writes_and_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let out = WriteTool
            .execute(&ctx(dir.path()), json!({"path": "a.txt", "content": "hello\n"}))
            .await;
        assert!(!out.is_error, "{}", out.content);
        assert_eq!(std::fs::read_to_string(dir.path().join("a.txt")).unwrap(), "hello\n");
    }

    #[tokio::test]
    async fn creates_missing_parent_directories() {
        let dir = tempfile::tempdir().unwrap();
        let out = WriteTool
            .execute(&ctx(dir.path()), json!({"path": "deep/nested/dir/a.txt", "content": "x"}))
            .await;
        assert!(!out.is_error, "{}", out.content);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("deep/nested/dir/a.txt")).unwrap(),
            "x"
        );
    }

    #[tokio::test]
    async fn overwrite_replaces_previous_content() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "old").unwrap();
        let out = WriteTool
            .execute(&ctx(dir.path()), json!({"path": "a.txt", "content": "new"}))
            .await;
        assert!(!out.is_error, "{}", out.content);
        assert_eq!(std::fs::read_to_string(dir.path().join("a.txt")).unwrap(), "new");
    }

    #[tokio::test]
    async fn missing_arguments_are_errors() {
        let dir = tempfile::tempdir().unwrap();
        let c = ctx(dir.path());
        let out = WriteTool.execute(&c, json!({"path": "a.txt"})).await;
        assert!(out.is_error);
        let out = WriteTool.execute(&c, json!({"content": "x"})).await;
        assert!(out.is_error);
    }

    #[test]
    fn write_is_never_concurrency_safe() {
        assert!(!WriteTool.is_concurrency_safe(&json!({})));
    }
}
