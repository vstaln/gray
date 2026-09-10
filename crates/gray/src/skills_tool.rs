//! The `skill` tool: loads a skill's SKILL.md body into context.
//!
//! Reads the file, strips YAML frontmatter, substitutes `$ARGUMENTS` / `${SKILL_DIR}`,
//! and returns the body wrapped in a `<skill>` envelope so the model treats it as
//! instructions to follow rather than a program to run.
//!
//! (Moved from `gray-tools` so `gray-tools` depends on `gray-core` only;
//! the tool lives next to skill discovery in [`crate::skills`].)

use async_trait::async_trait;
use gray_core::agent::{Tool, ToolContext, ToolOutput};
use gray_core::message::ToolDef;
use gray_core::tool_out::{fail, finish, resolve_path};
use serde_json::Value;
use serde_json::json;
use std::path::{Path, PathBuf};

pub const SKILL_SNIPPET: &str = "Load a skill's instructions into context";

/// Loads a skill file (`path`, or `name` resolved against skill directories).
pub struct SkillTool;

/// Resolve a skill name to its SKILL.md path via [`crate::skills::discover_skills`]
/// (global + project roots, first name match wins).
pub fn resolve_skill_name(cwd: &Path, name: &str) -> Option<PathBuf> {
    crate::skills::discover_skills(cwd)
        .skills
        .into_iter()
        .find(|s| s.name == name)
        .map(|s| s.file_path)
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

    fn prompt_snippet(&self) -> Option<&str> {
        Some(SKILL_SNIPPET)
    }

    fn prompt_guidelines(&self) -> Option<&'static [&'static str]> {
        Some(&[
            "When a task matches a skill in <available_skills>, load it with the skill tool and follow its instructions.",
        ])
    }

    // Pure read: safe to run alongside other tools.
    async fn execute(&self, ctx: &ToolContext, args: Value) -> ToolOutput {
        let path = match args.get("path") {
            Some(Value::String(s)) if !s.is_empty() => resolve_path(&ctx.cwd, s),
            _ => match args.get("name").and_then(|v| v.as_str()) {
                Some(name) if !name.is_empty() => match resolve_skill_name(&ctx.cwd, name) {
                    Some(p) => p,
                    None => return fail(format!("no skill named '{name}' found")),
                },
                _ => return fail("missing required argument: 'path' or 'name'".to_string()),
            },
        };
        let args_str = args
            .get("args")
            .and_then(|v| v.as_str())
            .map(str::to_string);

        let content = match tokio::fs::read_to_string(&path).await {
            Ok(c) => c,
            Err(e) => return fail(format!("read failed for {}: {e}", path.display())),
        };
        // Bug1: unknown invocation args must fail locally (naming valid args)
        // instead of silently substituting/ignoring. Skills with no declared
        // `args:` take none — any passed arg is an error.
        if let Some(a) = args_str.as_deref()
            && !a.trim().is_empty()
        {
            let (maybe_skill, _) = crate::skills::load_skill_from_file(&path, "path");
            if let Some(skill) = maybe_skill
                && let Err(msg) = crate::skills::validate_skill_args(&skill, Some(a))
            {
                return fail(msg);
            }
        }
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
        std::fs::write(
            dir.join("SKILL.md"),
            "---\nname: commit\ndescription: commit changes\n---\nBody",
        )
        .unwrap();
        let resolved = resolve_skill_name(tmp.path(), "commit").unwrap();
        assert_eq!(resolved, dir.join("SKILL.md"));
        assert!(resolve_skill_name(tmp.path(), "missing").is_none());
    }

    fn tool_ctx(cwd: &std::path::Path) -> gray_core::agent::ToolContext {
        gray_core::agent::ToolContext {
            cwd: cwd.to_path_buf(),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn skill_tool_rejects_unknown_args_naming_valid() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(".gray/skills/deploy");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            "---\nname: deploy\ndescription: test\nargs: env, force\n---\nDeploy $ARGUMENTS.",
        )
        .unwrap();
        let tool = SkillTool;
        let ctx = tool_ctx(tmp.path());
        let skill_path = dir.join("SKILL.md");
        // known arg passes (no error)
        let ok = tool
            .execute(
                &ctx,
                serde_json::json!({"path": skill_path.to_string_lossy(), "args": "env"}),
            )
            .await;
        assert!(!ok.is_error, "known arg must pass: {}", ok.content);
        // unknown arg fails locally naming valid args, no model needed
        let err = tool
            .execute(
                &ctx,
                serde_json::json!({"path": skill_path.to_string_lossy(), "args": "bogus-args"}),
            )
            .await;
        assert!(err.is_error, "unknown arg must fail");
        assert!(
            err.content.contains("bogus-args"),
            "names unknown: {}",
            err.content
        );
        assert!(err.content.contains("env"), "names valid: {}", err.content);
    }

    #[tokio::test]
    async fn skill_tool_rejects_any_arg_when_no_args_declared() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(".gray/skills/plain");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            "---\nname: plain\ndescription: test\n---\nPlain body.",
        )
        .unwrap();
        let tool = SkillTool;
        let ctx = tool_ctx(tmp.path());
        let skill_path = dir.join("SKILL.md");
        let err = tool
            .execute(
                &ctx,
                serde_json::json!({"path": skill_path.to_string_lossy(), "args": "bogus-args"}),
            )
            .await;
        assert!(err.is_error, "arg on no-args skill must fail");
        assert!(
            err.content.contains("(none)"),
            "names valid: {}",
            err.content
        );
    }
}
