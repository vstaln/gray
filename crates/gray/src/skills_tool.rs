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
//!
//! [`ProjectContextPlugin`] rides the same hook to serve the per-turn
//! `<project_context>` block: the nearest project AGENTS.md / CLAUDE.md
//! above the turn cwd, so project rules stay in the permanent prompt
//! instead of arriving as prunable tool observations.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Cache entry: owning cwd, discovery fingerprint, auto-switch snapshot,
/// disabled-set snapshot, served block. The toggles flip no file stat —
/// without them a toggle would serve the stale block.
type SkillsCache = Option<(String, u64, bool, BTreeSet<String>, Option<String>)>;

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
    cache: std::sync::Mutex<SkillsCache>,
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
        // Toggles flip no file stat, so the auto switch + disabled set ride the
        // cache key — otherwise a toggle would keep serving the stale block.
        let auto = crate::setup::skills_auto_enabled();
        let disabled = crate::setup::disabled_skill_names();
        if let Ok(guard) = self.cache.lock()
            && let Some((cached_cwd, cached_fp, cached_auto, cached_disabled, cached_block)) =
                guard.as_ref()
            && *cached_cwd == cwd
            && *cached_fp == fingerprint
            && *cached_auto == auto
            && *cached_disabled == disabled
        {
            return cached_block.clone();
        }
        let found = crate::skills::discover_skills(Path::new(cwd));
        let block = crate::skills::format_skills_for_prompt(&found.skills, auto, &disabled);
        let out = if block.trim().is_empty() {
            None
        } else {
            Some(block)
        };
        if let Ok(mut guard) = self.cache.lock() {
            *guard = Some((cwd.to_string(), fingerprint, auto, disabled, out.clone()));
        }
        out
    }
}
/// Rule file names probed at each directory level, nearest level first.
const PROJECT_RULES_NAMES: [&str; 2] = ["AGENTS.md", "CLAUDE.md"];

/// Cap on served rule bytes. An AGENTS.md is normally a few KB; a
/// pathological one must not be able to eat the context window, so the tail
/// is cut with a pointer back to the file.
const PROJECT_RULES_MAX_CHARS: usize = 32_768;

/// Read the exact `<project_context>` block the prompt hook serves for `cwd`:
/// the nearest `AGENTS.md` / `CLAUDE.md` at or above `cwd`. `None` when no
/// file is found or it is empty — the caller (`repl::status` for `/context`,
/// [`ProjectContextPlugin`] for the prompt) shares this one implementation.
///
/// The gray-home `AGENTS.md` is the stored system prompt itself; serving it
/// as project context would duplicate the whole prompt every turn, so that
/// level is skipped while the walk continues above it. Nearest ancestor
/// only: a monorepo's nested rule files wait for the model to read them,
/// which keeps the per-turn budget bounded.
pub fn project_context_block(cwd: &Path) -> Option<String> {
    let gray_home = crate::setup::gray_home()
        .ok()
        .and_then(|p| p.canonicalize().ok());
    let path = find_project_rules(cwd, gray_home.as_deref())?;
    let body = std::fs::read_to_string(&path).ok()?;
    let body = body.trim();
    if body.is_empty() {
        return None;
    }
    let mut body = body.to_string();
    if body.chars().count() > PROJECT_RULES_MAX_CHARS {
        let cut = body
            .char_indices()
            .nth(PROJECT_RULES_MAX_CHARS)
            .map(|(i, _)| i)
            .unwrap_or(body.len());
        body.truncate(cut);
        body.push_str(&format!(
            "\n…[truncated — read {} for the full file]",
            path.display()
        ));
    }
    // The block carries its own semantics, like the skills block does: a
    // stored prompt that never mentions <project_context> still learns what
    // it is and how to weigh it.
    Some(format!(
        "<project_context source=\"{}\">\nProject rules for this working directory, served automatically each turn. Follow them; they outrank general defaults.\n\n{body}\n</project_context>",
        path.display()
    ))
}

/// Nearest-ancestor search for a rule file. Each level prefers `AGENTS.md`
/// over `CLAUDE.md`. The gray-home level is skipped (it holds the stored
/// system prompt, not project rules); levels above it still count.
fn find_project_rules(cwd: &Path, gray_home: Option<&Path>) -> Option<PathBuf> {
    let mut dir = Some(cwd);
    while let Some(d) = dir {
        let is_gray_home = gray_home
            .map(|h| d.canonicalize().ok().as_deref() == Some(h))
            .unwrap_or(false);
        if !is_gray_home {
            for name in PROJECT_RULES_NAMES {
                let candidate = d.join(name);
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
        dir = d.parent();
    }
    None
}

/// Context-only builtin plugin serving the per-turn `<project_context>`
/// block (nearest project AGENTS.md / CLAUDE.md above the turn cwd).
///
/// - `tools()` is empty: the tool surface stays bash-only.
/// - `prompt_context()` returns [`project_context_block`], `None` when no
///   rule file exists (system prefix stays byte-stable for prefix caching).
///
/// The walk is a handful of stats, and the hook already runs once per turn
/// (not per round), so there is deliberately no cache to go stale — every
/// turn observes the file as it is on disk right now.
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

#[path = "skills_tool_tests.rs"]
#[cfg(test)]
mod tests;
