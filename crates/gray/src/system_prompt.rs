//! System prompt construction.
//!
//! The stored system prompt is the user's `~/.gray/AGENTS.md` file, sent to
//! the model verbatim minus HTML comments (`<!-- ... -->`). The file itself
//! carries no discovered project files and no working-directory line —
//! bash-only tools: the model inspects those itself via bash.
//!
//! Skills are the one ephemeral addition, and they live outside this module:
//! the context-only [`crate::skills_tool::SkillsPlugin`] serves the per-turn
//! `<available_skills>` list through the `prompt/context` hook (fresh
//! discovery for the turn cwd, `None` when empty so the prefix stays
//! byte-stable). No skill tool — the model reads matches with bash (`cat`).
//! This module stays pure file-text so its byte-stability unit test keeps
//! meaning something.

/// Strip `<!-- ... -->` spans (multi-line allowed) and trailing whitespace.
/// Comments stay in the editable file; the model never sees them. An unclosed
/// comment swallows the rest of the file.
pub fn strip_comments(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find("<!--") {
        out.push_str(&rest[..start]);
        match rest[start + 4..].find("-->") {
            Some(end) => rest = &rest[start + 4 + end + 3..],
            None => return out.trim_end().to_string(),
        }
    }
    out.push_str(rest);
    out.trim_end().to_string()
}

/// Build the system prompt: the file text, verbatim, minus HTML comments.
/// `None`/empty → "".
pub fn build_system_prompt(custom_prompt: Option<String>) -> String {
    strip_comments(custom_prompt.as_deref().unwrap_or_default())
}

#[path = "system_prompt_tests.rs"]
#[cfg(test)]
mod tests;
