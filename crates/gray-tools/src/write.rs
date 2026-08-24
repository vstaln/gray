//! The `write` tool: creates or overwrites a text file.

use async_trait::async_trait;
use gray_core::agent::{ToolContext, ToolOutput};
use gray_core::message::ToolDef;
use serde_json::json;
use serde_json::Value;

use crate::{fail, finish, get_str, resolve_path, Tool};

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
                    "content": {"type": "string", "description": "Full file contents"}
                },
                "required": ["path", "content"]
            }),
        )
    }

    // Mutates the filesystem: never run in parallel.
    fn is_concurrency_safe(&self, _args: &Value) -> bool {
        false
    }

    async fn execute(&self, ctx: &ToolContext, args: Value) -> ToolOutput {
        let path = match get_str(&args, "path") {
            Ok(p) => p,
            Err(e) => return e,
        };
        let content = match get_str(&args, "content") {
            Ok(c) => c,
            Err(e) => return e,
        };

        let full = resolve_path(&ctx.cwd, &path);
        if let Some(parent) = full.parent()
            && let Err(e) = tokio::fs::create_dir_all(parent).await
        {
            return fail(format!("write failed for {}: {e}", full.display()));
        }
        match tokio::fs::write(&full, content.as_bytes()).await {
            Ok(()) => finish(format!("wrote {} bytes to {}", content.len(), full.display())),
            Err(e) => fail(format!("write failed for {}: {e}", full.display())),
        }
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
