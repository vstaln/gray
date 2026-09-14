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

/// Build the system prompt: the file text, verbatim, minus HTML comments.
#[derive(Debug, Clone, Default)]
pub struct BuildSystemPromptOptions {
    /// The full prompt (the `~/.gray/AGENTS.md` body). `None`/empty → "".
    pub custom_prompt: Option<String>,
}

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

/// Build the system prompt.
pub fn build_system_prompt(options: BuildSystemPromptOptions) -> String {
    strip_comments(&options.custom_prompt.unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(prompt: &str) -> BuildSystemPromptOptions {
        BuildSystemPromptOptions {
            custom_prompt: Some(prompt.to_string()),
        }
    }

    #[test]
    fn rebuild_is_byte_stable() {
        // Prefix-cache invariant: identical inputs must rebuild to identical
        // bytes, or providers rebill the whole prefix every turn.
        let a = build_system_prompt(opts("You are gray."));
        let b = build_system_prompt(opts("You are gray."));
        assert_eq!(a, b, "system prompt rebuild diverged");
    }

    #[test]
    fn prompt_is_verbatim_after_comment_strip() {
        let p = build_system_prompt(opts("You are gray.\n\nFollow the rules."));
        assert_eq!(p, "You are gray.\n\nFollow the rules.");
    }

    #[test]
    fn html_comments_are_stripped_including_multiline() {
        let p = build_system_prompt(opts("A\n<!-- secret note\nspanning lines -->\nB\n"));
        assert_eq!(p, "A\n\nB");
        assert!(!p.contains("secret note"), "{p}");
    }

    #[test]
    fn unclosed_comment_swallows_tail() {
        assert_eq!(strip_comments("keep <!-- drop"), "keep");
        assert_eq!(
            strip_comments("only a comment <!-- x -->"),
            "only a comment"
        );
    }
}
