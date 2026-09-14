//! Skill context for the bash-only surface + paste helpers for `/skills <name>`.
//!
//! No `skill` tool — the tool surface stays blocking-`bash`-only.
//! Skills work through context + bash:
//! [`SkillsPlugin`] serves the per-turn `<available_skills>` list (names,
//! descriptions, exact `<location>` paths) via the `prompt/context` hook,
//! and the model reads one with bash (`cat <location>`).
//! This module also keeps the pure helpers the REPL slash command needs:
//! frontmatter stripping, `$ARGUMENTS` / `${SKILL_DIR}` substitution, and
//! name→path resolution via [`crate::skills::discover_skills`].

use std::path::{Path, PathBuf};

/// Context-only builtin plugin carrying the per-turn `<available_skills>`
/// list, so the model finds skills without bash-hunting for `SKILL.md`.
///
/// - `tools()` is empty: the tool surface stays bash-only.
/// - `prompt_context()` returns [`crate::skills::format_skills_for_prompt`]
///   for the turn cwd, `None` when nothing is discovered (system prefix stays
///   byte-stable for prefix caching).
///
/// The stored system prompt (`~/.gray/AGENTS.md`) stays verbatim — this list
/// is ephemeral per-turn context, never written anywhere.
pub struct SkillsPlugin;

#[async_trait::async_trait]
impl gray_plugin::Plugin for SkillsPlugin {
    fn manifest(&self) -> gray_plugin::Manifest {
        gray_plugin::Manifest {
            name: "skills".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            tools: vec![],
            ..Default::default()
        }
    }

    fn tools(&self) -> Vec<std::sync::Arc<dyn gray_core::agent::Tool>> {
        vec![]
    }

    async fn prompt_context(&self, cwd: &str) -> Option<String> {
        let found = crate::skills::discover_skills(Path::new(cwd));
        let block = crate::skills::format_skills_for_prompt(&found.skills);
        if block.trim().is_empty() {
            None
        } else {
            Some(block)
        }
    }
}

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

    #[tokio::test]
    async fn skills_plugin_is_context_only_and_serves_block() {
        use gray_plugin::Plugin;
        let plugin = SkillsPlugin;
        // Bash-only: no tools ride this plugin.
        assert!(
            plugin.tools().is_empty(),
            "skills plugin must carry no tools"
        );
        assert!(
            plugin.manifest().tools.is_empty(),
            "manifest must advertise no tools"
        );
        // Isolate from global skills for the empty case.
        let prev_home = std::env::var("HOME").ok();
        let prev_gray = std::env::var("GRAY_HOME").ok();
        let prev_xdg = std::env::var("XDG_CONFIG_HOME").ok();
        let iso = tempfile::tempdir().unwrap();
        unsafe {
            std::env::set_var("HOME", iso.path());
            std::env::set_var("GRAY_HOME", iso.path().join(".gray"));
            std::env::set_var("XDG_CONFIG_HOME", iso.path().join(".config"));
        }
        let empty = tempfile::tempdir().unwrap();
        let none = plugin.prompt_context(empty.path().to_str().unwrap()).await;
        assert_eq!(none, None);
        // Restore before the project-skill case (discovery walks cwd only,
        // globals stay isolated only for the empty check above).
        unsafe {
            match &prev_home {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
            match &prev_gray {
                Some(v) => std::env::set_var("GRAY_HOME", v),
                None => std::env::remove_var("GRAY_HOME"),
            }
            match &prev_xdg {
                Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
                None => std::env::remove_var("XDG_CONFIG_HOME"),
            }
        }
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(".gray/skills/paste-demo");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            "---\ndescription: demo skill\n---\nDemo body.",
        )
        .unwrap();
        let ctx = plugin
            .prompt_context(tmp.path().to_str().unwrap())
            .await
            .expect("discovered skills must produce hook context");
        assert!(ctx.contains("<available_skills>"), "missing block: {ctx}");
        assert!(ctx.contains("paste-demo"), "missing skill name: {ctx}");
        assert!(ctx.contains("SKILL.md"), "missing exact location: {ctx}");
        assert!(
            ctx.contains("cat <location>") || ctx.contains("cat "),
            "block must tell the model to read via bash: {ctx}"
        );
    }
}
