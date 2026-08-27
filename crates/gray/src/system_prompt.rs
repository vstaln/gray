//! System prompt construction — port of `packages/coding-agent/src/core/system-prompt.ts` (169 LOC).
//!
//! Literal port of `buildSystemPrompt` semantics:
//! - tools list only includes tools WITH `promptSnippet`
//! - guidelines deduped via insertion-ordered set
//! - cwd appended last
//! - skills section gated on `read` tool presence
//! - customPrompt replaces default prompt; project_context + skills still appended
//! - project_context blocks from AGENTS.md / CLAUDE.md discovery (walk up to git root)

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::skills::{format_skills_for_prompt, Skill};

// ---------------------------------------------------------------------------
// AGENTS.md / CLAUDE.md discovery — walk up to git root
// ---------------------------------------------------------------------------

fn find_git_root(start: &Path) -> Option<PathBuf> {
    let mut cur = if start.is_file() {
        start.parent().map(PathBuf::from)
    } else {
        Some(start.to_path_buf())
    };
    while let Some(dir) = cur {
        if dir.join(".git").exists() {
            return Some(dir);
        }
        cur = dir.parent().map(|p| p.to_path_buf());
    }
    None
}

/// Context files discovered by walking `cwd` up to git root (or filesystem root).
/// Looks for `AGENTS.md` and `CLAUDE.md` at each ancestor.
#[derive(Debug, Clone)]
pub struct ContextFile {
    pub path: PathBuf,
    pub content: String,
}

pub fn discover_context_files(cwd: &Path) -> Vec<ContextFile> {
    let git_root = find_git_root(cwd);
    let mut out = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut cur = Some(cwd.to_path_buf());
    // collect ancestors from cwd up to git root (or filesystem root)
    let mut ancestors: Vec<PathBuf> = Vec::new();
    while let Some(dir) = cur {
        ancestors.push(dir.clone());
        if let Some(root) = &git_root {
            if &dir == root {
                break;
            }
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
            if let Ok(content) = fs::read_to_string(&p) {
                if !content.trim().is_empty() {
                    out.push(ContextFile { path: p, content });
                }
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// buildSystemPrompt — literal port
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct BuildSystemPromptOptions {
    /// Custom system prompt (replaces default).
    pub custom_prompt: Option<String>,
    /// Tools to include in prompt. Default: ["read", "bash", "edit", "write"]
    pub selected_tools: Option<Vec<String>>,
    /// One-line tool snippets keyed by tool name. Only tools WITH a snippet appear in Available tools.
    pub tool_snippets: Option<HashMap<String, String>>,
    /// Additional guideline bullets appended to default guidelines (deduped).
    pub prompt_guidelines: Option<Vec<String>>,
    /// Text appended to system prompt.
    pub append_system_prompt: Option<String>,
    /// Working directory (used for cwd line + context file discovery if not provided).
    pub cwd: PathBuf,
    /// Pre-loaded context files (if None, discovered via AGENTS.md/CLAUDE.md).
    pub context_files: Option<Vec<ContextFile>>,
    /// Pre-loaded skills (if None, empty).
    pub skills: Option<Vec<Skill>>,
}

fn default_selected_tools() -> Vec<String> {
    vec!["read".to_string(), "bash".to_string(), "edit".to_string(), "write".to_string()]
}

/// Build the system prompt — literal port of `buildSystemPrompt` in `system-prompt.ts`.
pub fn build_system_prompt(options: BuildSystemPromptOptions) -> String {
    let cwd = options.cwd.clone();
    let prompt_cwd = cwd.to_string_lossy().replace('\\', "/");

    let append_section = options
        .append_system_prompt
        .as_deref()
        .map(|s| format!("\n\n{s}"))
        .unwrap_or_default();

    // Resolve context files: use provided, else discover
    let context_files: Vec<ContextFile> = if let Some(cf) = options.context_files {
        cf
    } else {
        discover_context_files(&cwd)
    };

    let skills: Vec<Skill> = options.skills.unwrap_or_default();

    // Custom prompt branch — replaces default prompt
    if let Some(custom) = options.custom_prompt {
        let mut prompt = custom;
        if !append_section.is_empty() {
            prompt.push_str(&append_section);
        }
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
        let has_read = selected.as_ref().map(|t| t.iter().any(|n| n == "read")).unwrap_or(true);
        if has_read && !skills.is_empty() {
            prompt.push_str(&format_skills_for_prompt(&skills));
        }
        prompt.push_str(&format!("\nCurrent working directory: {prompt_cwd}\n"));
        return prompt;
    }

    // Default prompt branch
    let readme_path = get_readme_path();
    let docs_path = get_docs_path();
    let examples_path = get_examples_path();

    let tools = options.selected_tools.unwrap_or_else(default_selected_tools);
    let snippets = options.tool_snippets.unwrap_or_default();

    // Only tools WITH a snippet appear
    let visible_tools: Vec<&String> = tools.iter().filter(|name| snippets.contains_key(*name as &str)).collect();
    let tools_list = if visible_tools.is_empty() {
        "(none)".to_string()
    } else {
        visible_tools
            .iter()
            .map(|name| format!("- {}: {}", name, snippets[*name as &str]))
            .collect::<Vec<_>>()
            .join("\n")
    };

    // Guidelines — deduped insertion-ordered
    let mut guidelines_list: Vec<String> = Vec::new();
    let mut guidelines_set: HashSet<String> = HashSet::new();
    let mut add_guideline = |g: String| {
        if guidelines_set.insert(g.clone()) {
            guidelines_list.push(g);
        }
    };

    let has_bash = tools.iter().any(|t| t == "bash");
    let has_powershell = tools.iter().any(|t| t == "powershell");
    let has_grep = tools.iter().any(|t| t == "grep");
    let has_find = tools.iter().any(|t| t == "find");
    let has_ls = tools.iter().any(|t| t == "ls");
    let has_read = tools.iter().any(|t| t == "read");

    if (has_bash || has_powershell) && !has_grep && !has_find && !has_ls {
        if has_bash && has_powershell {
            add_guideline("Use bash or PowerShell for file operations like listing, searching, and finding files".to_string());
        } else if has_powershell {
            add_guideline("Use PowerShell for file operations like listing, searching, and finding files".to_string());
        } else {
            add_guideline("Use bash for file operations like ls, rg, find".to_string());
        }
    }

    for g in options.prompt_guidelines.unwrap_or_default() {
        let normalized = g.trim().to_string();
        if !normalized.is_empty() {
            add_guideline(normalized);
        }
    }

    add_guideline("Be concise in your responses".to_string());
    add_guideline("Show file paths clearly when working with files".to_string());

    let guidelines = guidelines_list.iter().map(|g| format!("- {g}")).collect::<Vec<_>>().join("\n");

    let mut prompt = format!(
        "You are an expert coding assistant operating inside pi, a coding agent harness. You help users by reading files, executing commands, editing code, and writing new files.\n\nAvailable tools:\n{tools_list}\n\nIn addition to the tools above, you may have access to other custom tools depending on the project.\n\nGuidelines:\n{guidelines}\n\nPi documentation (read only when the user asks about pi itself, its SDK, extensions, themes, skills, or TUI):\n- Main documentation: {readme_path}\n- Additional docs: {docs_path}\n- Examples: {examples_path} (extensions, custom tools, SDK)\n- When reading pi docs or examples, resolve docs/... under Additional docs and examples/... under Examples, not the current working directory\n- When asked about: extensions (docs/extensions.md, examples/extensions/), themes (docs/themes.md), skills (docs/skills.md), prompt templates (docs/prompt-templates.md), TUI components (docs/tui.md), keybindings (docs/keybindings.md), SDK integrations (docs/sdk.md), custom providers (docs/custom-provider.md), adding models (docs/models.md), pi packages (docs/packages.md), environment variables (docs/environment-variables.md)\n- When working on pi topics, read the docs and examples, and follow .md cross-references before implementing\n- Always read pi .md files completely and follow links to related docs (e.g., tui.md for TUI API details)"
    );

    if !append_section.is_empty() {
        prompt.push_str(&append_section);
    }

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

    if has_read && !skills.is_empty() {
        prompt.push_str(&format_skills_for_prompt(&skills));
    }

    prompt.push_str(&format!("\nCurrent working directory: {prompt_cwd}"));
    prompt
}

fn get_readme_path() -> String {
    // Try to resolve via env or fallback
    std::env::var("PI_README_PATH").unwrap_or_else(|_| "README.md".to_string())
}
fn get_docs_path() -> String {
    std::env::var("PI_DOCS_PATH").unwrap_or_else(|_| "docs".to_string())
}
fn get_examples_path() -> String {
    std::env::var("PI_EXAMPLES_PATH").unwrap_or_else(|_| "examples".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn gated_on_read_tool() {
        let skill = Skill {
            name: "x".into(),
            description: "does x".into(),
            file_path: PathBuf::from("/tmp/SKILL.md"),
            base_dir: PathBuf::from("/tmp"),
            disable_model_invocation: false,
            source: "user".into(),
        };
        let p = build_system_prompt(BuildSystemPromptOptions {
            cwd: PathBuf::from("/tmp"),
            selected_tools: Some(vec!["bash".into()]),
            skills: Some(vec![skill]),
            ..Default::default()
        });
        assert!(!p.contains("<available_skills>"));
    }
    #[test]
    fn tools_list_only_with_snippet() {
        let mut snippets = HashMap::new();
        snippets.insert("read".to_string(), "read files".to_string());
        let p = build_system_prompt(BuildSystemPromptOptions {
            cwd: PathBuf::from("/tmp"),
            tool_snippets: Some(snippets),
            ..Default::default()
        });
        assert!(p.contains("read: read files"));
        assert!(!p.contains("bash:"));
    }
}
