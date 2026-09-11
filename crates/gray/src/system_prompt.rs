//! System prompt construction.
//!
//! System prompt rules:
//! - customPrompt replaces the built-in prompt; project_context + skills still appended
//! - skills section gated on `read` tool presence
//! - cwd appended last
//! - project_context blocks from AGENTS.md / CLAUDE.md discovery (walk up to git root)

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::skills::{Skill, format_skills_for_prompt};

// ---------------------------------------------------------------------------
// AGENTS.md / CLAUDE.md discovery — walk up to git root
// ---------------------------------------------------------------------------

/// Context files discovered by walking `cwd` up to git root (or filesystem root).
/// Looks for `AGENTS.md` and `CLAUDE.md` at each ancestor.
#[derive(Debug, Clone)]
pub struct ContextFile {
    pub path: PathBuf,
    pub content: String,
}

pub fn discover_context_files(cwd: &Path) -> Vec<ContextFile> {
    let git_root = crate::skills::find_git_root(cwd);
    let mut out = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut cur = Some(cwd.to_path_buf());
    // collect ancestors from cwd up to git root (or filesystem root)
    let mut ancestors: Vec<PathBuf> = Vec::new();
    while let Some(dir) = cur {
        ancestors.push(dir.clone());
        if let Some(root) = &git_root
            && &dir == root
        {
            break;
        }
        cur = dir.parent().map(|p| p.to_path_buf());
    }
    // walk from root down to cwd so root files come first (stable ordering)
    for dir in ancestors.iter().rev() {
        for name in ["AGENTS.md", "CLAUDE.md"] {
            let p = dir.join(name);
            if !seen.insert(p.clone()) {
                continue;
            }
            if let Ok(content) = fs::read_to_string(&p)
                && !content.trim().is_empty()
            {
                out.push(ContextFile { path: p, content });
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// buildSystemPrompt
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct BuildSystemPromptOptions {
    /// Custom system prompt (replaces the built-in default).
    pub custom_prompt: Option<String>,
    /// Selected tool names; the skills section is gated on `read` presence.
    pub selected_tools: Option<Vec<String>>,
    /// Working directory (used for cwd line + context file discovery if not provided).
    pub cwd: PathBuf,
    /// Pre-loaded context files (if None, discovered via AGENTS.md/CLAUDE.md).
    pub context_files: Option<Vec<ContextFile>>,
    /// Pre-loaded skills (if None, empty).
    pub skills: Option<Vec<Skill>>,
}

/// Build the system prompt.
pub fn build_system_prompt(options: BuildSystemPromptOptions) -> String {
    let cwd = options.cwd.clone();
    let prompt_cwd = cwd.to_string_lossy().replace('\\', "/");

    // Resolve context files: use provided, else discover
    let context_files: Vec<ContextFile> = if let Some(cf) = options.context_files {
        cf
    } else {
        discover_context_files(&cwd)
    };

    let skills: Vec<Skill> = options.skills.unwrap_or_default();

    // The custom prompt (user's AGENTS.md) replaces the built-in prompt;
    // project_context + skills still append.
    let mut prompt = options.custom_prompt.unwrap_or_default();
    if !context_files.is_empty() {
        prompt.push_str("\n\n<project_context>\n\n");
        prompt.push_str("Project-specific instructions and guidelines:\n\n");
        for cf in &context_files {
            prompt.push_str(&format!(
                "<project_instructions path=\"{}\">\n{}\n</project_instructions>\n\n",
                cf.path.display(),
                cf.content
            ));
        }
        prompt.push_str("</project_context>\n");
    }
    let selected = options.selected_tools.clone();
    // Skills are loadable whenever the model has a shell or a reader: both can
    // read `SKILL.md`. Under the default `tools-minimal` surface that is `bash`.
    let has_reader = selected
        .as_ref()
        .map(|t| t.iter().any(|n| n == "read" || n == "bash"))
        .unwrap_or(true);
    if has_reader && !skills.is_empty() {
        prompt.push_str(&format_skills_for_prompt(&skills));
    }
    prompt.push_str(&format!("\nCurrent working directory: {prompt_cwd}\n"));
    prompt
}

#[cfg(test)]
mod tests {
    use super::*;

    fn custom_opts(prompt: &str) -> BuildSystemPromptOptions {
        BuildSystemPromptOptions {
            custom_prompt: Some(prompt.to_string()),
            cwd: PathBuf::from("/tmp"),
            context_files: Some(vec![]),
            skills: Some(vec![]),
            ..Default::default()
        }
    }

    #[test]
    fn rebuild_is_byte_stable() {
        // Prefix-cache invariant: identical inputs must rebuild to identical
        // bytes, or providers rebill the whole prefix every turn.
        let a = build_system_prompt(custom_opts("You are gray."));
        let b = build_system_prompt(custom_opts("You are gray."));
        assert_eq!(a, b, "system prompt rebuild diverged");
    }

    #[test]
    fn custom_prompt_forbids_decline_workarounds() {
        let guide = "When a tool call is declined, do NOT re-attempt it via write/edit/bash workarounds; ask the user instead";
        let prompt = build_system_prompt(custom_opts(guide));
        assert!(prompt.contains("do NOT re-attempt"), "{prompt}");
        assert!(prompt.contains("write/edit/bash"), "{prompt}");
        assert!(prompt.contains("ask the user instead"), "{prompt}");
        assert!(
            !prompt.contains("operating inside pi"),
            "dead default prompt must not leak: {prompt}"
        );
    }

    #[test]
    fn custom_prompt_warns_on_secret_files() {
        let guide = "Treat secret-bearing files as sensitive: never print their values, redact secrets when quoting";
        let prompt = build_system_prompt(custom_opts(guide));
        assert!(prompt.contains("secret-bearing"), "{prompt}");
        assert!(prompt.contains("never print"), "{prompt}");
        assert!(prompt.contains("redact"), "{prompt}");
    }
}
