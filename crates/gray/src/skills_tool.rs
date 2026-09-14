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

/// Project context for the bash-only surface (`ProjectContextPlugin`).
///
/// Gray never injects project files into the stored system prompt (that
/// prefix must stay byte-stable for provider prefix caching). Instead,
/// project `AGENTS.md` / `CLAUDE.md` files attach as ephemeral per-turn
/// context through the `prompt/context` hook — the same seam as
/// [`SkillsPlugin`]. The model no longer needs to `cat` them manually;
/// `/context` bills the same block it actually receives, so the
/// "Project context" row stays honest.
///
/// Discovery mirrors skill project roots: `cwd` up to (and including) the
/// git root, nearest directory last so the most specific file reads last.
/// The stored system prompt itself (`~/.gray/AGENTS.md`) is excluded so it
/// is never billed twice. Each file is capped at
/// [`PROJECT_CONTEXT_MAX_CHARS`] with a truncation note — an unbounded
/// read here would blow the context window it reports to.
pub(crate) const PROJECT_CONTEXT_FILES: &[&str] = &["AGENTS.md", "CLAUDE.md"];
pub(crate) const PROJECT_CONTEXT_MAX_CHARS: usize = 16_000;

/// Project context files for `cwd`, git-root-first so the nearest reads last.
pub fn discover_project_context_files(cwd: &Path) -> Vec<PathBuf> {
    let resolved = std::fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());
    let self_prompt = crate::sys_prompt_path()
        .ok()
        .and_then(|p| std::fs::canonicalize(&p).ok());
    let git_root = crate::skills::find_git_root(&resolved);
    // Same ancestor shape as skill project roots: cwd up, git root inclusive.
    let mut dirs: Vec<PathBuf> = Vec::new();
    let mut cur = Some(resolved);
    while let Some(dir) = cur {
        dirs.push(dir.clone());
        if let Some(root) = &git_root
            && &dir == root
        {
            break;
        }
        cur = dir.parent().map(|p| p.to_path_buf());
    }
    let mut out = Vec::new();
    for dir in dirs.iter().rev() {
        for name in PROJECT_CONTEXT_FILES {
            let f = dir.join(name);
            if !f.is_file() {
                continue;
            }
            if let Ok(canon) = std::fs::canonicalize(&f)
                && Some(&canon) == self_prompt.as_ref()
            {
                continue;
            }
            out.push(f);
        }
    }
    out
}

/// The `<project_context>` hook block for `cwd`, or `None` when no project
/// files exist. Single definition: [`ProjectContextPlugin`] serves it and
/// `/context` estimates this exact block.
pub fn project_context_block(cwd: &Path) -> Option<String> {
    let mut out = String::from("<project_context>\n");
    let mut any = false;
    for f in discover_project_context_files(cwd) {
        let Ok(body) = std::fs::read_to_string(&f) else {
            continue;
        };
        let body = body.trim();
        if body.is_empty() {
            continue;
        }
        let (body, truncated) = if body.len() > PROJECT_CONTEXT_MAX_CHARS {
            (body[..PROJECT_CONTEXT_MAX_CHARS].to_string(), true)
        } else {
            (body.to_string(), false)
        };
        out.push_str(&format!(
            "<file path=\"{}\">{}{}</file>\n",
            f.display(),
            body,
            if truncated {
                "\n<!-- truncated: file exceeds per-file cap -->"
            } else {
                ""
            }
        ));
        any = true;
    }
    out.push_str("</project_context>");
    any.then_some(out)
}

/// Context-only builtin plugin carrying the `<project_context>` block, so
/// project `AGENTS.md` / `CLAUDE.md` files reach the model without bash
/// round-trips and without touching the cached system prefix.
///
/// - `tools()` is empty: the tool surface stays bash-only.
/// - `prompt_context()` returns [`project_context_block`] for the turn cwd,
///   `None` when nothing is discovered (hook stays silent, prefix stable).
pub struct ProjectContextPlugin;

#[async_trait::async_trait]
impl gray_plugin::Plugin for ProjectContextPlugin {
    fn manifest(&self) -> gray_plugin::Manifest {
        gray_plugin::Manifest {
            name: "project-context".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            tools: vec![],
            ..Default::default()
        }
    }

    fn tools(&self) -> Vec<std::sync::Arc<dyn gray_core::agent::Tool>> {
        vec![]
    }

    async fn prompt_context(&self, cwd: &str) -> Option<String> {
        project_context_block(Path::new(cwd))
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
    fn project_context_block_attaches_cwd_agents_md() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("AGENTS.md"), "# rules\nBe nice.").unwrap();
        let block = project_context_block(tmp.path()).expect("block");
        assert!(block.contains("<project_context>"), "{block}");
        assert!(block.contains("Be nice."), "{block}");
        assert!(block.contains("AGENTS.md"), "{block}");
    }

    #[test]
    fn project_context_block_empty_without_files() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(project_context_block(tmp.path()), None);
    }

    #[test]
    fn project_context_skips_stored_system_prompt() {
        // `~/.gray/AGENTS.md` is the system prompt — billing it again as
        // project context would double-count the prefix.
        let home = tempfile::tempdir().unwrap();
        let gray_home = home.path().join(".gray");
        std::fs::create_dir_all(&gray_home).unwrap();
        std::fs::write(gray_home.join("AGENTS.md"), "system prompt").unwrap();
        unsafe { std::env::set_var("GRAY_HOME", &gray_home) };
        let block = project_context_block(&gray_home);
        unsafe { std::env::remove_var("GRAY_HOME") };
        assert_eq!(block, None, "system prompt must not self-attach");
    }

    #[test]
    fn project_context_caps_huge_files() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("AGENTS.md"), "x".repeat(20_000)).unwrap();
        let block = project_context_block(tmp.path()).expect("block");
        assert!(block.contains("truncated"), "{block}");
        assert!(
            block.len() < 20_000 + 512,
            "cap must bound the block: {}",
            block.len()
        );
    }

    #[tokio::test]
    async fn project_context_plugin_is_context_only() {
        use gray_plugin::Plugin;
        let plugin = ProjectContextPlugin;
        assert!(plugin.tools().is_empty(), "must carry no tools");
        assert!(plugin.manifest().tools.is_empty(), "manifest: no tools");
        let tmp = tempfile::tempdir().unwrap();
        let none = plugin.prompt_context(tmp.path().to_str().unwrap()).await;
        assert_eq!(none, None);
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
