//! The `skill` tool: loads a skill's SKILL.md body into context.
//!
//! Pattern ported from Grok (`xai-grok-tools/src/implementations/skills/skill.rs`):
//! read the file, strip YAML frontmatter, substitute `$ARGUMENTS` / `${SKILL_DIR}`,
//! and return the body wrapped in a `<skill>` envelope so the model treats it as
//! instructions to follow rather than a program to run.

use async_trait::async_trait;
use gray_core::agent::{ToolContext, ToolOutput};
use gray_core::message::ToolDef;
use serde_json::json;
use serde_json::Value;
use std::path::{Path, PathBuf};

use crate::{fail, finish, Tool};

pub const SKILL_SNIPPET: &str = "Load a skill's instructions into context";

/// Loads a skill file (`path`, or `name` resolved against gray's skill dirs).
pub struct SkillTool;

/// Resolve a skill name to a SKILL.md path by scanning gray's skill dirs
/// (mirrors `crates/gray/src/skills.rs` discovery roots): agent dir (`$GRAY_HOME`
/// or `~/.gray`, plus `~/.pi/agent` compat) and project `.gray`/`.pi` dirs up to
/// the git root. First match wins.
pub fn resolve_skill_name(cwd: &Path, name: &str) -> Option<PathBuf> {
    let mut dirs = Vec::new();
    let home = std::env::var("GRAY_HOME")
        .or_else(|_| std::env::var("HOME").map(|h| format!("{h}/.gray")))
        .ok();
    if let Some(base) = &home {
        dirs.push(PathBuf::from(base).join("skills"));
    }
    if let Ok(h) = std::env::var("HOME") {
        dirs.push(PathBuf::from(h).join(".pi/agent/skills"));
    }
    // project dirs up to git root
    let mut cur = Some(cwd.to_path_buf());
    while let Some(dir) = cur {
        dirs.push(dir.join(".gray/skills"));
        dirs.push(dir.join(".pi/skills"));
        if dir.join(".git").exists() {
            break;
        }
        cur = dir.parent().map(Path::to_path_buf);
    }
    dirs.into_iter()
        .map(|d| d.join(name).join("SKILL.md"))
        .find(|p| p.is_file())
}

/// Strip YAML frontmatter (`---`-delimited block at the top), returning the body.
pub fn strip_frontmatter(content: &str) -> &str {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return content;
    }
    let after = &trimmed[3..];
    for (idx, line) in after.lines().enumerate() {
        if line.trim() == "---" {
            let mut off = 0usize;
            for (i, l) in after.lines().enumerate() {
                if i == idx {
                    let body = &after[off + l.len()..];
                    return body.trim_start_matches('\n');
                }
                off += l.len() + 1;
            }
        }
    }
    content
}

/// Substitute `$ARGUMENTS` and `${SKILL_DIR}` in a skill body (Grok-style).
pub fn apply_substitutions(body: &str, args: Option<&str>, skill_dir: &str) -> String {
    body.replace("$ARGUMENTS", args.unwrap_or(""))
        .replace("${SKILL_DIR}", skill_dir)
}

#[async_trait]
impl Tool for SkillTool {
    fn def(&self) -> ToolDef {
        ToolDef::new(
            "skill",
            "Load a skill's instructions (SKILL.md body) into context. Use when the \
             task matches a skill listed in <available_skills>. Returns the skill \
             content wrapped in a <skill> envelope; follow it as instructions.",
            json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to the skill file (from <available_skills> <location>)"
                    },
                    "name": {
                        "type": "string",
                        "description": "Skill name to resolve against known skill dirs (used when no path is known)"
                    },
                    "args": {
                        "type": "string",
                        "description": "Optional arguments, substituted for $ARGUMENTS in the skill body"
                    }
                }
            }),
        )
    }

    fn prompt_snippet(&self) -> Option<&'static str> {
        Some(SKILL_SNIPPET)
    }

    fn prompt_guidelines(&self) -> Option<&'static [&'static str]> {
        Some(&["When a task matches a skill in <available_skills>, load it with the skill tool and follow its instructions."])
    }

    // Pure read: safe to run alongside other tools.
    async fn execute(&self, ctx: &ToolContext, args: Value) -> ToolOutput {
        let path = match args.get("path") {
            Some(Value::String(s)) if !s.is_empty() => {
                crate::resolve_path(&ctx.cwd, s)
            }
            _ => match args.get("name").and_then(|v| v.as_str()) {
                Some(name) if !name.is_empty() => match resolve_skill_name(&ctx.cwd, name) {
                    Some(p) => p,
                    None => return fail(format!("no skill named '{name}' found")),
                },
                _ => return fail("missing required argument: 'path' or 'name'".to_string()),
            },
        };
        let args_str = args.get("args").and_then(|v| v.as_str()).map(str::to_string);

        let content = match tokio::fs::read_to_string(&path).await {
            Ok(c) => c,
            Err(e) => return fail(format!("read failed for {}: {e}", path.display())),
        };
        let skill_dir = path.parent().unwrap_or(Path::new(".")).to_string_lossy();
        let body = strip_frontmatter(&content);
        let body = apply_substitutions(body, args_str.as_deref(), &skill_dir);
        let envelope = format!(
            "<skill name=\"{}\" path=\"{}\">\n{}\n</skill>",
            path.file_stem().and_then(|s| s.to_str()).unwrap_or("skill"),
            path.display(),
            body
        );
        finish(envelope)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_frontmatter_removes_yaml_header() {
        let content = "---\nname: test\ndescription: A test skill\n---\n\nBody here.\n";
        assert_eq!(strip_frontmatter(content), "Body here.\n");
    }

    #[test]
    fn strip_frontmatter_keeps_body_without_frontmatter() {
        assert_eq!(strip_frontmatter("Just content."), "Just content.");
    }

    #[test]
    fn substitutions_expand_arguments_and_skill_dir() {
        let body = "Deploy $ARGUMENTS from ${SKILL_DIR}/bin.";
        assert_eq!(
            apply_substitutions(body, Some("staging"), "/skills/deploy"),
            "Deploy staging from /skills/deploy/bin."
        );
        assert_eq!(
            apply_substitutions("No args here.", None, "/d"),
            "No args here."
        );
    }

    #[test]
    fn resolve_skill_name_finds_project_skill() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(".gray/skills/commit");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("SKILL.md"), "---\nname: commit\n---\nBody").unwrap();
        let resolved = resolve_skill_name(tmp.path(), "commit").unwrap();
        assert_eq!(resolved, dir.join("SKILL.md"));
        assert!(resolve_skill_name(tmp.path(), "missing").is_none());
    }
}
