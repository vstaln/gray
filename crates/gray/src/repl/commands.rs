/// Static slash-command table driving both `/help` and the autocomplete panel.
pub(crate) const COMMANDS: &[(&str, &str)] = &[
    ("connect", "setup provider & API key"),
    ("model", "switch model"),
    ("thinking", "reasoning effort"),
    ("context", "set context window"),
    ("resume", "resume conversation"),
    ("new", "new conversation"),
    ("compact", "summarize context"),
    ("usage", "session tokens & cost"),
    ("cron", "cron jobs"),
    ("proxy", "share Codex/Grok/OpenRouter via :8645"),
    ("gateway", "messaging gateway (Telegram/Discord/Slack)"),
    ("portal", "portal status"),
    ("agentsmd", "edit system prompt"),
    ("skills", "list skills (/skills:<name> [args] to run one)"),
    ("help", "show commands"),
    ("quit", "exit"),
];

pub(crate) const ALIASES: &[(&str, &str)] = &[
    ("clear", "new"),
    ("reset", "new"),
    ("exit", "quit"),
    ("keys", "connect"),
    ("key", "connect"),
    ("providers", "connect"),
    ("provider", "connect"),
    ("login", "connect"),
    ("effort", "thinking"),
    ("compress", "compact"),
    ("sys", "agentsmd"),
    ("portal", "proxy"),
    ("gw", "gateway"),
    ("cost", "usage"),
];

/// Commands matching `filter` (the text after '/'), auto-sorted by relevance.
pub(crate) fn completion_matches(filter: &str) -> Vec<(&'static str, &'static str)> {
    let f = filter.to_lowercase();
    let mut matches: Vec<(&'static str, &'static str)> = Vec::new();

    for &(name, desc) in COMMANDS {
        let is_match = f.is_empty()
            || name.to_lowercase().contains(&f)
            || desc.to_lowercase().contains(&f)
            || ALIASES.iter().any(|(alias, target)| *target == name && alias.contains(&f));

        if is_match {
            matches.push((name, desc));
        }
    }

    matches.sort_by_key(|(n, _)| {
        let nl = n.to_lowercase();
        if nl == f {
            0
        } else if nl.starts_with(&f) {
            1
        } else if ALIASES.iter().any(|(alias, target)| *target == *n && *alias == f) {
            2
        } else if ALIASES.iter().any(|(alias, target)| *target == *n && alias.starts_with(&f)) {
            3
        } else {
            4
        }
    });

    matches
}


/// Completion for the composer prompt: static commands, skill names after
/// `/skills:`, or per-command suffixes after `/cmd ` (Minecraft-style).
/// Owned here so every read_loop call site stays in sync.
pub(crate) fn completion_matches_dyn(cur_text: &str, cwd: &std::path::Path) -> Vec<(String, String)> {
    if cur_text.starts_with("/skills:") && !cur_text[8..].contains(char::is_whitespace) {
        let filter = &cur_text[8..];
        return crate::skills::discover_skills(cwd)
            .skills
            .iter()
            .filter(|s| s.name.contains(filter))
            .map(|s| (format!("skills:{}", s.name), s.description.clone()))
            .collect();
    }
    if cur_text.starts_with('/') {
        let inner = &cur_text[1..];
        if let Some(idx) = inner.find(char::is_whitespace) {
            let (cmd, _) = inner.split_at(idx);
            if cmd.contains(':') {
                return Vec::new();
            }
            // Everything after `<cmd>`, leading spaces trimmed, trailing kept
            // for level detection (`/context reserve ` vs `/context reserve`).
            let after = inner[cmd.len()..].trim_start_matches(char::is_whitespace);
            // Reconstruct trailing-space info from the raw line.
            let trailing = cur_text.ends_with(char::is_whitespace);
            let full_after = if trailing && !after.ends_with(char::is_whitespace) {
                format!("{after} ")
            } else {
                after.to_string()
            };
            return complete_command_args(&cmd.to_lowercase(), &full_after, cwd);
        }
        return completion_matches(inner)
            .into_iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect();
    }
    Vec::new()
}

/// Universal per-command suffix completion hook.
///
/// `cmd` is lowercased without the leading `/`; `arg_text` is everything
/// after it with leading spaces trimmed (trailing space preserved to detect
/// `/cmd sub ` vs `/cmd sub`). Returns full `cmd + args` names (no slash)
/// so the existing `/{name} ` fill just works. Add new commands here.
pub(crate) fn complete_command_args(
    cmd: &str,
    arg_text: &str,
    _cwd: &std::path::Path,
) -> Vec<(String, String)> {
    match cmd {
        "context" => complete_context_args(arg_text),
        _ => Vec::new(),
    }
}

/// Suffixes for `/context`: L1 (`128k|auto|status|reserve|keep`) and L2
/// (`reserve <16k|auto>`, `keep <20k|auto|off>`).
fn complete_context_args(arg_text: &str) -> Vec<(String, String)> {
    const L1: &[(&str, &str)] = &[
        ("128k", "set window — e.g. 128k, 1m"),
        ("auto", "clear override → auto"),
        ("status", "show breakdown"),
        ("reserve", "set reserve…"),
        ("keep", "set keep tail…"),
    ];
    const RESERVE_VALS: &[(&str, &str)] = &[
        ("16k", "reserve 16k"),
        ("auto", "clear reserve → default"),
    ];
    const KEEP_VALS: &[(&str, &str)] = &[
        ("20k", "keep 20k tail"),
        ("auto", "clear keep → default"),
        ("off", "summary only"),
    ];
    let trailing = arg_text.ends_with(char::is_whitespace);
    let parts: Vec<&str> = arg_text.split_whitespace().collect();
    // `/context ` → all L1
    if parts.is_empty() {
        return L1
            .iter()
            .map(|(s, d)| (format!("context {s}"), d.to_string()))
            .collect();
    }
    let head = parts[0].to_lowercase();
    if head == "reserve" || head == "keep" {
        // `/context reserve` (no space) → still L1 filtering, so Tab picks `reserve ` first
        if parts.len() == 1 && !trailing {
            let f = parts[0].to_lowercase();
            return L1
                .iter()
                .filter(|(s, _)| s.to_lowercase().contains(&f))
                .map(|(s, d)| (format!("context {s}"), d.to_string()))
                .collect();
        }
        let vals = if head == "reserve" { RESERVE_VALS } else { KEEP_VALS };
        let f = if parts.len() >= 2 { parts[1].to_lowercase() } else { String::new() };
        return vals
            .iter()
            .filter(|(s, _)| f.is_empty() || s.to_lowercase().contains(&f))
            .map(|(s, d)| (format!("context {head} {s}"), d.to_string()))
            .collect();
    }
    // L1 leaf with trailing space takes nothing further.
    if parts.len() > 1 || trailing {
        return Vec::new();
    }
    let f = parts[0].to_lowercase();
    L1.iter()
        .filter(|(s, _)| s.to_lowercase().contains(&f))
        .map(|(s, d)| (format!("context {s}"), d.to_string()))
        .collect()
}

/// Parsed command or input from the REPL prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplCommand {
    /// Exit the REPL cleanly (`/quit` or `/exit`).
    Quit,
    /// Open the system-prompt file in `$EDITOR` (`/agentsmd`), print it (`/agentsmd show`),
    /// or restore the default (`/agentsmd reset`). `/sys` is an alias.
    Sys(SysAction),
    /// Open the provider selection menu (`/connect` or `/provider`).
    Provider,
    /// Start a fresh conversation (`/new` or `/clear [prompt]`).
    New(Option<String>),
    /// Resume a previous session (`/resume [id|--last|--all]`).
    Resume(ResumeArgs),
    /// Compress conversation context window (`/compact` or `/compress [instructions]`).
    Compact(Option<String>),
    /// Set reasoning effort (`/thinking [level]` or `/effort [level]`; bare toggles hide/show).
    Thinking(Option<String>),
    /// Print the command list (`/help`).
    Help,
    /// Open the model picker (`/model`) or set directly (`/model provider/id`).
    Model(Option<String>),
    /// Set context window (`/context [128k|auto|reserve 16k|keep 20k|status]`).
    ContextWindow(Option<String>),
    /// Session token + cost totals (`/usage` or `/cost`).
    Usage,
    /// Unknown slash command (`/word`).
    Unknown(String),
    /// Cron jobs: /cron, /cron list, /cron create --schedule ... --prompt ...
    Cron(String),
    /// Local proxy: /proxy start|stop|status, /portal alias
    Proxy(String),
    /// Messaging gateway: /gateway, /gateway status|run|install (bare opens picker like /proxy)
    Gateway(String),
    /// Skills: /skills lists; /skills:<name> [args] runs a skill
    Skill(Option<String>),
    /// Regular user prompt to feed to the agent.
    Prompt(String),
    /// Blank line, should be ignored.
    Empty,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumeArgs {
    pub target: Option<String>,
    pub last: bool,
    pub all: bool,
}

/// What to do when the user types `/agentsmd`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SysAction {
    /// Edit `~/.gray/AGENTS.md` in `$EDITOR`.
    Edit,
    /// Print the current prompt file contents and path.
    Show,
    /// Overwrite the file with the shipped default.
    Reset,
}

pub(crate) fn parse_resume_args(rest: &str) -> ResumeArgs {
    let tokens: Vec<&str> = rest.split_whitespace().collect();
    let mut target: Option<String> = None;
    let mut last = false;
    let mut all = false;
    for tok in tokens {
        match tok {
            "--last" => last = true,
            "--all" => all = true,
            s if s.starts_with("--") => {}
            s => {
                if target.is_none() {
                    target = Some(s.to_string());
                }
            }
        }
    }
    ResumeArgs { target, last, all }
}

/// Parses a line of input into a [`ReplCommand`].
pub fn parse_command(line: &str) -> ReplCommand {
    let t = line.trim();
    if t.is_empty() {
        return ReplCommand::Empty;
    }
    let (cmd, rest) = match t.split_once(' ') {
        Some((c, r)) => (c, r.trim()),
        None => (t, ""),
    };
    let opt = |s: &str| (!s.is_empty()).then(|| s.to_string());

    match cmd {
        "/quit" | "/exit" => ReplCommand::Quit,
        "/resume" => ReplCommand::Resume(if rest.is_empty() {
            ResumeArgs { target: None, last: false, all: false }
        } else {
            parse_resume_args(rest)
        }),
        "/agentsmd" | "/sys" => match rest {
            "" => ReplCommand::Sys(SysAction::Edit),
            "show" => ReplCommand::Sys(SysAction::Show),
            "reset" => ReplCommand::Sys(SysAction::Reset),
            _ => ReplCommand::Unknown(t.to_string()),
        },
        "/new" | "/clear" | "/reset" => ReplCommand::New(opt(rest)),
        "/compact" | "/compress" => ReplCommand::Compact(opt(rest)),
        "/thinking" | "/effort" => ReplCommand::Thinking(opt(rest)),
        "/context" => ReplCommand::ContextWindow(opt(rest)),
        "/usage" | "/cost" => ReplCommand::Usage,
        "/help" => ReplCommand::Help,
        _ => {
            // preserve original edge cases: bare aliases exact, "/key foo" is Provider but "/keys foo" is Unknown, "/model*" prefix without space
            if t == "/connect"
                || t == "/provider"
                || t == "/providers"
                || t == "/login"
                || t == "/key"
                || t == "/keys"
                || t.starts_with("/key ")
            {
                return ReplCommand::Provider;
            }
            if t.starts_with("/model") {
                return ReplCommand::Model(opt(t[6..].trim()));
            }
            if t.starts_with("/cron") {
                return ReplCommand::Cron(t.to_string());
            }
            if t.starts_with("/proxy") || t.starts_with("/portal") {
                return ReplCommand::Proxy(t.to_string());
            }
            if t.starts_with("/gateway") || t.starts_with("/gw") {
                return ReplCommand::Gateway(t.to_string());
            }
            if t == "/skills" || t.starts_with("/skills:") {
                return ReplCommand::Skill(t.strip_prefix("/skills:").map(str::to_string));
            }
            if t.starts_with('/') {
                // Codex port (`reference/openai/codex/codex-rs/tui/src/bottom_pane/chat_composer/slash_input.rs`
                // `validate_submission`): a slash-name containing '/' is plain
                // text, not an unknown command — e.g. `///` doc comments, `//`
                // comments, `/tmp/foo` paths. Bare `/` (empty name) is text too.
                let name = t[1..].split_whitespace().next().unwrap_or("");
                if name.is_empty() || name.contains('/') {
                    return ReplCommand::Prompt(t.to_string());
                }
                return ReplCommand::Unknown(t.to_string());
            }
            ReplCommand::Prompt(t.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ReplCommand, parse_command};
    #[test]
    fn slash_name_with_slash_is_plain_prompt_like_codex() {
        // Reported bug: pasting Rust `///` doc comments said "unknown command".
        let pasted = "/// Default system prompt, shipped as markdown and materialized to `~/.gray/sys.md`\n/// on first run.";
        assert!(matches!(parse_command(pasted), ReplCommand::Prompt(_)));
        assert!(matches!(parse_command("// comment"), ReplCommand::Prompt(_)));
        assert!(matches!(parse_command("/tmp/foo"), ReplCommand::Prompt(_)));
        assert!(matches!(parse_command("/"), ReplCommand::Prompt(_)));
        // Genuinely unknown single-token commands still error.
        assert!(matches!(parse_command("/boguscmd"), ReplCommand::Unknown(_)));
        // Known commands unaffected.
        assert!(matches!(parse_command("/help"), ReplCommand::Help));
        assert!(matches!(parse_command("/model foo"), ReplCommand::Model(_)));
    }

    #[test]
    fn context_renamed_no_window_alias() {
        assert!(matches!(parse_command("/context"), ReplCommand::ContextWindow(None)));
        assert!(matches!(
            parse_command("/context 128k"),
            ReplCommand::ContextWindow(Some(_))
        ));
        assert!(matches!(parse_command("/context-window"), ReplCommand::Unknown(_)));
    }

    #[test]
    fn usage_command_and_cost_alias() {
        assert!(matches!(parse_command("/usage"), ReplCommand::Usage));
        assert!(matches!(parse_command("/cost"), ReplCommand::Usage));
        use std::path::Path;
        let cwd = Path::new(".");
        assert!(super::completion_matches_dyn("/us", cwd).iter().any(|(n, _)| n == "usage"));
        // `cost` resolves through the alias table
        assert!(super::completion_matches("cost").iter().any(|(n, _)| *n == "usage"));
    }

    #[test]
    fn context_arg_completion_levels() {
        use super::{complete_command_args, completion_matches_dyn};
        use std::path::Path;
        let cwd = Path::new(".");
        // bare suffix lists everything
        let all = complete_command_args("context", "", cwd);
        assert!(all.iter().any(|(n, _)| n == "context reserve"));
        assert!(all.iter().any(|(n, _)| n == "context auto"));
        // filtered L1
        let r = complete_command_args("context", "r", cwd);
        assert!(r.iter().any(|(n, _)| n == "context reserve"));
        // L2 after `reserve `
        let r2 = complete_command_args("context", "reserve ", cwd);
        assert!(r2.iter().any(|(n, _)| n == "context reserve 16k"));
        // unknown command has no suffixes (universal hook default)
        assert!(complete_command_args("model", "", cwd).is_empty());
        // dyn dispatch through the composer entry point
        let dyn_all = completion_matches_dyn("/context ", cwd);
        assert!(dyn_all.iter().any(|(n, _)| n == "context reserve"));
        assert!(completion_matches_dyn("/model ", cwd).is_empty());
        // command-name path unaffected
        assert!(completion_matches_dyn("/cont", cwd).iter().any(|(n, _)| n == "context"));
    }
}

