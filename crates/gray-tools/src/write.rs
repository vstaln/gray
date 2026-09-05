//! The `write` tool: creates or overwrites a text file.

use std::sync::Arc;

use async_trait::async_trait;
use gray_core::agent::{ToolContext, ToolOutput};
use gray_core::message::ToolDef;
use serde_json::Value;
use serde_json::json;

use crate::ledger::{FileLedger, LedgerEntry};
use crate::read::notices;
use crate::{Tool, fail, finish, get_opt_bool, resolve_path};

pub const WRITE_SNIPPET: &str = "Create or overwrite files";
pub const WRITE_GUIDELINES: &[&str] = &["Use write only for new files or complete rewrites."];

/// Writes `content` to `path`, creating parent directories as needed.
///
/// Shares a [`FileLedger`] with the read/edit tools for the read-before-write
/// guard (T3.2).
pub struct WriteTool {
    ledger: Arc<FileLedger>,
}

impl WriteTool {
    /// Share `ledger` (the registry/plugin wiring); [`Default`] keeps a
    /// private ledger so existing tests compile.
    pub fn new(ledger: Arc<FileLedger>) -> Self {
        Self { ledger }
    }

    /// T3.2 decision table (pure; `None` = allow), in order: (1) new file →
    /// allow; (2) unread → refuse unless `force`; (3) changed on disk →
    /// refuse; (4) full view → allow; (5) same bytes (clamped-but-complete) →
    /// allow; (6) partial → refuse with the resume offset. `force` bypasses
    /// (2) and (6) only.
    pub(crate) fn decide(
        entry: Option<&LedgerEntry>,
        meta: Option<&std::fs::Metadata>,
        old: &str,
        force: bool,
        display: &str,
    ) -> Option<String> {
        let meta = meta?;
        let Some(entry) = entry else {
            if force {
                return None;
            }
            return Some(notices::write_unread(display));
        };
        if meta.modified().is_ok_and(|t| t != entry.mtime) || meta.len() != entry.size {
            return Some(notices::write_changed(display));
        }
        // `force` bypasses (2) and (6) — but never (3) above.
        if force {
            return None;
        }
        if entry.full_view {
            return None;
        }
        if entry
            .content_hash
            .is_some_and(|h| Some(h) == FileLedger::hash_bytes(old.as_bytes()))
        {
            return None;
        }
        Some(notices::write_partial(
            display,
            entry.first_line,
            entry.last_line,
            old.lines().count(),
        ))
    }
}

impl Default for WriteTool {
    fn default() -> Self {
        Self {
            ledger: Arc::new(FileLedger::new()),
        }
    }
}

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
                    "force": {
                        "type": "boolean",
                        "description": "Overwrite without having read the file (bypasses the read-before-write guard)"
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

    async fn execute(&self, ctx: &ToolContext, args: Value) -> ToolOutput {
        let path = args
            .get("path")
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

        let content = args
            .get("content")
            .or_else(|| args.get("contents"))
            .or_else(|| args.get("text"))
            .or_else(|| args.get("code"))
            .or_else(|| args.get("body"))
            .or_else(|| args.get("data"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let full = resolve_path(&ctx.cwd, &path);
        let display = full.display().to_string();
        let force = match get_opt_bool(&args, "force") {
            Ok(v) => v.unwrap_or(false),
            Err(e) => return e,
        };
        let disk_meta = tokio::fs::metadata(&full).await.ok();
        let old = if disk_meta.is_some() {
            tokio::fs::read_to_string(&full).await.unwrap_or_default()
        } else {
            String::new()
        };
        // T3.2 ledger guard: refuse blind/partial/stale overwrites (pure table
        // above). Runs before any directory creation or write.
        if let Some(msg) = Self::decide(
            self.ledger.get(&full).as_ref(),
            disk_meta.as_ref(),
            &old,
            force,
            &display,
        ) {
            return fail(msg);
        }
        if let Some(parent) = full.parent()
            && let Err(e) = tokio::fs::create_dir_all(parent).await
        {
            return fail(format!("write failed for {}: {e}", full.display()));
        }
        let existed = disk_meta.is_some();
        match tokio::fs::write(&full, content.as_bytes()).await {
            Ok(()) => {
                // T3.2 ledger: the whole new content is known — the next write
                // is allowed without a re-read, and the next read is full.
                self.ledger.mark_written(&full, content.as_bytes());
                if existed && !old.is_empty() {
                    let patch = crate::edit_diff::generate_unified_patch(&path, &old, &content, 3);
                    if patch.is_empty() {
                        finish(format!(
                            "wrote {} bytes to {} (no change)",
                            content.len(),
                            full.display()
                        ))
                    } else {
                        finish(patch)
                    }
                } else {
                    finish(format!(
                        "wrote {} bytes to {}",
                        content.len(),
                        full.display()
                    ))
                }
            }
            Err(e) => fail(format!("write failed for {}: {e}", full.display())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry_for(
        path: &std::path::Path,
        full_view: bool,
        content_hash: Option<u64>,
    ) -> LedgerEntry {
        let meta = std::fs::metadata(path).unwrap();
        LedgerEntry {
            mtime: meta.modified().unwrap(),
            size: meta.len(),
            content_hash,
            full_view,
            window: (1, None),
            first_line: 1,
            last_line: 2,
            dedup_armed: true,
            read_at: std::time::Instant::now(),
        }
    }

    fn fixture(content: &[u8]) -> (tempfile::TempDir, std::path::PathBuf, String) {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f.txt");
        std::fs::write(&p, content).unwrap();
        let old = String::from_utf8(content.to_vec()).unwrap();
        (dir, p, old)
    }

    #[test]
    fn new_file_is_always_allowed() {
        assert!(WriteTool::decide(None, None, "", false, "new.txt").is_none());
    }

    #[test]
    fn unread_existing_file_is_refused_naming_read_and_force() {
        let (_dir, p, old) = fixture(b"one\ntwo\n");
        let meta = std::fs::metadata(&p).unwrap();
        let msg = WriteTool::decide(None, Some(&meta), &old, false, "f.txt")
            .expect("unread write must be refused");
        assert!(msg.contains("has not been read"), "{msg}");
        assert!(msg.contains("read f.txt"), "{msg}");
        assert!(msg.contains("force=true"), "{msg}");
    }

    #[test]
    fn force_bypasses_unread_and_partial_but_not_stale() {
        let (_dir, p, old) = fixture(b"one\ntwo\n");
        let meta = std::fs::metadata(&p).unwrap();
        assert!(WriteTool::decide(None, Some(&meta), &old, true, "f.txt").is_none());
        let partial = entry_for(&p, false, None);
        assert!(WriteTool::decide(Some(&partial), Some(&meta), &old, true, "f.txt").is_none());
        // Staleness still refuses even with force.
        std::fs::write(&p, b"one\ntwo\nthree\n").unwrap();
        let grown = std::fs::metadata(&p).unwrap();
        assert!(WriteTool::decide(Some(&partial), Some(&grown), &old, true, "f.txt").is_some());
    }

    #[test]
    fn full_view_and_clamped_complete_writes_are_allowed() {
        let (_dir, p, old) = fixture(b"one\ntwo\n");
        let meta = std::fs::metadata(&p).unwrap();
        let full = entry_for(&p, true, None);
        assert!(WriteTool::decide(Some(&full), Some(&meta), &old, false, "f.txt").is_none());
        // Rule 5: partial window, but every byte was seen (hash matches disk).
        let clamped = entry_for(&p, false, FileLedger::hash_bytes(old.as_bytes()));
        assert!(WriteTool::decide(Some(&clamped), Some(&meta), &old, false, "f.txt").is_none());
    }

    #[test]
    fn changed_on_disk_is_refused() {
        let (_dir, p, old) = fixture(b"one\ntwo\n");
        let stale = entry_for(&p, true, None);
        std::fs::write(&p, b"one\ntwo\nthree\n").unwrap();
        let meta = std::fs::metadata(&p).unwrap();
        let msg = WriteTool::decide(Some(&stale), Some(&meta), &old, false, "f.txt")
            .expect("stale write must be refused");
        assert!(msg.contains("changed on disk"), "{msg}");
    }

    #[test]
    fn partial_view_is_refused_with_resume_offset_not_unread_wording() {
        let (_dir, p, old) = fixture(b"l1\nl2\nl3\nl4\nl5\n");
        let meta = std::fs::metadata(&p).unwrap();
        let mut partial = entry_for(&p, false, None);
        partial.last_line = 2;
        let msg = WriteTool::decide(Some(&partial), Some(&meta), &old, false, "f.txt")
            .expect("partial write must be refused");
        assert!(msg.contains("only part of f.txt"), "{msg}");
        assert!(msg.contains("lines 1-2 of 5"), "{msg}");
        assert!(msg.contains("offset=3"), "{msg}");
        assert!(!msg.contains("has not been read"), "{msg}");
    }
}
