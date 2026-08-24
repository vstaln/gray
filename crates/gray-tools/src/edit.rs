//! The `edit` tool: exact-string replacement inside an existing file.

use async_trait::async_trait;
use gray_core::agent::{ToolContext, ToolOutput};
use gray_core::message::ToolDef;
use serde_json::json;
use serde_json::Value;

use crate::{fail, finish, get_opt_bool, get_str, resolve_path, Tool};

/// Replaces `old_text` with `new_text` in `path`.
pub struct EditTool;

#[async_trait]
impl Tool for EditTool {
    fn def(&self) -> ToolDef {
        ToolDef::new(
            "edit",
            "Replace an exact string in a file. Fails if old_text is not \
             found or matches more than once without replace_all.",
            json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "File path (absolute or relative to cwd)"
                    },
                    "old_text": {
                        "type": "string",
                        "description": "Exact text to replace (must match the file byte-for-byte)"
                    },
                    "new_text": {
                        "type": "string",
                        "description": "Replacement text"
                    },
                    "replace_all": {
                        "type": "boolean",
                        "description": "Replace every occurrence instead of just the first (default false)"
                    }
                },
                "required": ["path", "old_text", "new_text"]
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
        let old_text = match get_str(&args, "old_text") {
            Ok(t) => t,
            Err(e) => return e,
        };
        let new_text = match get_str(&args, "new_text") {
            Ok(t) => t,
            Err(e) => return e,
        };
        let replace_all = match get_opt_bool(&args, "replace_all") {
            Ok(v) => v.unwrap_or(false),
            Err(e) => return e,
        };

        let full = resolve_path(&ctx.cwd, &path);
        let content = match tokio::fs::read_to_string(&full).await {
            Ok(c) => c,
            Err(e) => return fail(format!("edit failed for {}: {e}", full.display())),
        };

        let matches = content.matches(old_text.as_str()).count();
        if matches == 0 {
            return fail(format!(
                "edit failed for {}: old_text not found in file",
                full.display()
            ));
        }
        if matches > 1 && !replace_all {
            return fail(format!(
                "edit failed for {}: old_text matches {} times; pass \
                 replace_all=true or include more surrounding context",
                full.display(),
                matches
            ));
        }

        let updated =
            if replace_all { content.replace(old_text.as_str(), &new_text) } else { content.replacen(old_text.as_str(), &new_text, 1) };

        if let Err(e) = tokio::fs::write(&full, updated.as_bytes()).await {
            return fail(format!("edit failed for {}: {e}", full.display()));
        }
        finish(format!(
            "edited {}: {} occurrence(s) replaced",
            full.display(),
            if replace_all { matches } else { 1 }
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(dir: &std::path::Path) -> ToolContext {
        ToolContext { cwd: dir.to_path_buf(), cancel: Default::default() }
    }

    fn edit_args(path: &str, old: &str, new: &str) -> Value {
        json!({"path": path, "old_text": old, "new_text": new})
    }

    #[tokio::test]
    async fn replaces_first_occurrence_by_default() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "foo bar baz").unwrap();
        let out = EditTool
            .execute(&ctx(dir.path()), edit_args("a.txt", "foo", "qux"))
            .await;
        assert!(!out.is_error, "{}", out.content);
        assert_eq!(std::fs::read_to_string(dir.path().join("a.txt")).unwrap(), "qux bar baz");
    }

    #[tokio::test]
    async fn zero_matches_is_a_clear_error() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "hello world").unwrap();
        let out = EditTool
            .execute(&ctx(dir.path()), edit_args("a.txt", "absent", "x"))
            .await;
        assert!(out.is_error);
        assert!(out.content.contains("not found"), "{}", out.content);
        // File untouched.
        assert_eq!(std::fs::read_to_string(dir.path().join("a.txt")).unwrap(), "hello world");
    }

    #[tokio::test]
    async fn ambiguous_match_without_flag_is_a_clear_error() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "dup dup dup").unwrap();
        let out = EditTool
            .execute(&ctx(dir.path()), edit_args("a.txt", "dup", "x"))
            .await;
        assert!(out.is_error);
        assert!(out.content.contains("matches 3 times"), "{}", out.content);
        assert!(out.content.contains("replace_all"), "{}", out.content);
        assert_eq!(std::fs::read_to_string(dir.path().join("a.txt")).unwrap(), "dup dup dup");
    }

    #[tokio::test]
    async fn replace_all_replaces_every_occurrence() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "dup dup dup").unwrap();
        let out = EditTool
            .execute(
                &ctx(dir.path()),
                json!({"path": "a.txt", "old_text": "dup", "new_text": "x", "replace_all": true}),
            )
            .await;
        assert!(!out.is_error, "{}", out.content);
        assert_eq!(std::fs::read_to_string(dir.path().join("a.txt")).unwrap(), "x x x");
    }

    #[tokio::test]
    async fn missing_file_is_error() {
        let dir = tempfile::tempdir().unwrap();
        let out = EditTool
            .execute(&ctx(dir.path()), edit_args("nope.txt", "a", "b"))
            .await;
        assert!(out.is_error);
    }

    #[tokio::test]
    async fn empty_new_text_deletes_the_match() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "keep drop keep").unwrap();
        let out = EditTool
            .execute(&ctx(dir.path()), edit_args("a.txt", "drop ", ""))
            .await;
        assert!(!out.is_error, "{}", out.content);
        assert_eq!(std::fs::read_to_string(dir.path().join("a.txt")).unwrap(), "keep keep");
    }

    #[test]
    fn edit_is_never_concurrency_safe() {
        assert!(!EditTool.is_concurrency_safe(&json!({})));
    }
}
