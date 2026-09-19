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
    cache: std::sync::Mutex<
        Option<(
            String,
            u64,
            std::collections::BTreeSet<String>,
            Option<String>,
        )>,
    >,
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
        // A toggle flips no file stat, so the disabled set rides the cache
        // key — otherwise a disable would keep serving the stale block.
        let disabled = crate::setup::disabled_skill_names();
        if let Ok(guard) = self.cache.lock()
            && let Some((cached_cwd, cached_fp, cached_disabled, cached_block)) = guard.as_ref()
            && *cached_cwd == cwd
            && *cached_fp == fingerprint
            && *cached_disabled == disabled
        {
            return cached_block.clone();
        }
        let found = crate::skills::discover_skills(Path::new(cwd));
        let block = crate::skills::format_skills_for_prompt(&found.skills, &disabled);
        let out = if block.trim().is_empty() {
            None
        } else {
            Some(block)
        };
        if let Ok(mut guard) = self.cache.lock() {
            *guard = Some((cwd.to_string(), fingerprint, disabled, out.clone()));
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

#[path = "skills_tool_tests.rs"]
#[cfg(test)]
mod tests;
