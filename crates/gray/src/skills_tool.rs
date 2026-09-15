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
/// Discovery walks ~300 files, so the served block is cached per cwd behind
/// a [`crate::skills::discovery_fingerprint`] (stat-only, no reads): any
/// add/edit/remove flips the fingerprint and rescans, so a cache hit can
/// never serve bytes a fresh discovery wouldn't — behavior is identical to
/// per-turn rediscovery, minus the IO.
///
/// The stored system prompt (`~/.gray/AGENTS.md`) stays verbatim — this list
/// is ephemeral per-turn context, never written anywhere.
#[derive(Default)]
pub struct SkillsPlugin {
    cache: std::sync::Mutex<Option<(String, u64, Option<String>)>>,
}

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
        let fingerprint = crate::skills::discovery_fingerprint(Path::new(cwd));
        if let Ok(guard) = self.cache.lock()
            && let Some((cached_cwd, cached_fp, cached_block)) = guard.as_ref()
            && *cached_cwd == cwd
            && *cached_fp == fingerprint
        {
            return cached_block.clone();
        }
        let found = crate::skills::discover_skills(Path::new(cwd));
        let block = crate::skills::format_skills_for_prompt(&found.skills);
        let out = if block.trim().is_empty() {
            None
        } else {
            Some(block)
        };
        if let Ok(mut guard) = self.cache.lock() {
            *guard = Some((cwd.to_string(), fingerprint, out.clone()));
        }
        out
    }
}
/// Read the exact `<project_context>` block the prompt hook serves for `cwd`:
/// the project AGENTS.md / CLAUDE.md the model sees each turn. `None` when
/// the hook serves nothing (keeps `/context` honest without duplicating
/// discovery logic).
pub fn project_context_block(cwd: &std::path::Path) -> Option<String> {
    // No hook serves a `<project_context>` block: the system prompt tells
    // the model to read AGENTS.md / CLAUDE.md with bash, so project context
    // arrives as ordinary (prunable) tool observations, not a hook block.
    // Kept as a named choke point so `/context` stays honest if a hook is
    // ever added - and so the call in `repl::status` keeps compiling.
    let _ = cwd;
    None
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
    async fn skills_context_matches_fresh_discovery_and_rescans_on_change() {
        use gray_plugin::Plugin;
        // Isolate from the user's real skills (see the test above for why
        // this save/set/restore dance exists).
        let prev_home = std::env::var("HOME").ok();
        let prev_gray = std::env::var("GRAY_HOME").ok();
        let prev_xdg = std::env::var("XDG_CONFIG_HOME").ok();
        let iso = tempfile::tempdir().unwrap();
        unsafe {
            std::env::set_var("HOME", iso.path());
            std::env::set_var("GRAY_HOME", iso.path().join(".gray"));
            std::env::set_var("XDG_CONFIG_HOME", iso.path().join(".config"));
        }
        let restore = || unsafe {
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
        };

        let work = tempfile::tempdir().unwrap();
        let cwd = work.path().to_str().unwrap().to_string();
        let fresh_block = || {
            let found = crate::skills::discover_skills(work.path());
            let block = crate::skills::format_skills_for_prompt(&found.skills);
            if block.trim().is_empty() {
                None
            } else {
                Some(block)
            }
        };
        let write_skill = |name: &str, description: &str| {
            let dir = work.path().join(".gray/skills").join(name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("SKILL.md"),
                format!("---\ndescription: {description}\n---\nBody"),
            )
            .unwrap();
        };

        let plugin = SkillsPlugin::default();
        // Fingerprint is stable with no changes…
        let fp1 = crate::skills::discovery_fingerprint(work.path());
        assert_eq!(fp1, crate::skills::discovery_fingerprint(work.path()));

        write_skill("demo-a", "first demo skill");
        // …moves when a skill appears…
        let fp2 = crate::skills::discovery_fingerprint(work.path());
        assert_ne!(fp1, fp2, "fingerprint must move on new skill");
        // …and the served block matches a fresh discovery exactly (no
        // behavior change vs per-turn rediscovery)…
        let a = plugin.prompt_context(&cwd).await;
        assert_eq!(a, fresh_block());
        assert!(a.unwrap().contains("first demo skill"));
        // …is stable across turns…
        assert_eq!(plugin.prompt_context(&cwd).await, fresh_block());

        // …moves on in-place edits (dir mtime alone would miss these)…
        write_skill("demo-a", "edited demo description");
        let fp3 = crate::skills::discovery_fingerprint(work.path());
        assert_ne!(fp2, fp3, "fingerprint must move on content edit");
        let b = plugin.prompt_context(&cwd).await;
        assert_eq!(b, fresh_block());
        assert!(b.unwrap().contains("edited demo description"));

        // …and on removal.
        std::fs::remove_dir_all(work.path().join(".gray/skills/demo-a")).unwrap();
        let fp4 = crate::skills::discovery_fingerprint(work.path());
        assert_ne!(fp3, fp4, "fingerprint must move on removal");
        assert_eq!(plugin.prompt_context(&cwd).await, fresh_block());

        restore();
    }

    #[tokio::test]
    async fn skills_plugin_is_context_only_and_serves_block() {
        use gray_plugin::Plugin;
        let plugin = SkillsPlugin::default();
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
