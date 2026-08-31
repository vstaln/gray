/// Static slash-command table driving both `/help` and the autocomplete panel.
pub(crate) const COMMANDS: &[(&str, &str)] = &[
    ("connect", "setup provider & API key"),
    ("model", "switch model"),
    ("thinking", "reasoning effort"),
    ("resume", "resume conversation"),
    ("new", "new conversation"),
    ("compact", "summarize context"),
    ("cron", "cron jobs"),
    ("proxy", "local proxy"),
    ("portal", "portal status"),
    ("sys", "edit system prompt"),
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
    ("portal", "proxy"),
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


/// Parsed command or input from the REPL prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplCommand {
    /// Exit the REPL cleanly (`/quit` or `/exit`).
    Quit,
    /// Open the system-prompt file in `$EDITOR` (`/sys`), print it (`/sys show`),
    /// or restore the default (`/sys reset`).
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
    /// Unknown slash command (`/word`).
    Unknown(String),
    /// Cron jobs: /cron, /cron list, /cron create --schedule ... --prompt ...
    Cron(String),
    /// Local proxy: /proxy start|stop|status, /portal alias
    Proxy(String),
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

/// What to do when the user types `/sys`.
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
    let trimmed = line.trim();
    if trimmed.is_empty() {
        ReplCommand::Empty
    } else if trimmed == "/quit" || trimmed == "/exit" {
        ReplCommand::Quit
    } else if trimmed == "/resume" {
        ReplCommand::Resume(ResumeArgs { target: None, last: false, all: false })
    } else if let Some(rest) = trimmed.strip_prefix("/resume ") {
        ReplCommand::Resume(parse_resume_args(rest))
    } else if trimmed == "/sys" {
        ReplCommand::Sys(SysAction::Edit)
    } else if trimmed == "/sys show" {
        ReplCommand::Sys(SysAction::Show)
    } else if trimmed == "/sys reset" {
        ReplCommand::Sys(SysAction::Reset)
    } else if trimmed == "/new" || trimmed == "/clear" || trimmed == "/reset" {
        ReplCommand::New(None)
    } else if let Some(rest) = trimmed.strip_prefix("/new ") {
        let arg = rest.trim();
        ReplCommand::New((!arg.is_empty()).then(|| arg.to_string()))
    } else if let Some(rest) = trimmed.strip_prefix("/clear ") {
        let arg = rest.trim();
        ReplCommand::New((!arg.is_empty()).then(|| arg.to_string()))
    } else if let Some(rest) = trimmed.strip_prefix("/reset ") {
        let arg = rest.trim();
        ReplCommand::New((!arg.is_empty()).then(|| arg.to_string()))
    } else if trimmed == "/compact" || trimmed == "/compress" {
        ReplCommand::Compact(None)
    } else if let Some(rest) = trimmed.strip_prefix("/compact ") {
        let arg = rest.trim();
        ReplCommand::Compact((!arg.is_empty()).then(|| arg.to_string()))
    } else if let Some(rest) = trimmed.strip_prefix("/compress ") {
        let arg = rest.trim();
        ReplCommand::Compact((!arg.is_empty()).then(|| arg.to_string()))
    } else if trimmed == "/thinking" || trimmed == "/effort" {
        ReplCommand::Thinking(None)
    } else if let Some(rest) = trimmed.strip_prefix("/thinking ") {
        let arg = rest.trim();
        ReplCommand::Thinking((!arg.is_empty()).then(|| arg.to_string()))
    } else if let Some(rest) = trimmed.strip_prefix("/effort ") {
        let arg = rest.trim();
        ReplCommand::Thinking((!arg.is_empty()).then(|| arg.to_string()))
    } else if trimmed == "/connect" || trimmed == "/provider" || trimmed == "/providers" || trimmed == "/login" || trimmed == "/key" || trimmed == "/keys" || trimmed.starts_with("/key ") {
        ReplCommand::Provider
    } else if trimmed == "/help" {
        ReplCommand::Help
    } else if let Some(rest) = trimmed.strip_prefix("/model") {
        let arg = rest.trim();
        ReplCommand::Model((!arg.is_empty()).then(|| arg.to_string()))
    } else if trimmed.starts_with("/cron") {
        ReplCommand::Cron(trimmed.to_string())
    } else if trimmed.starts_with("/proxy") || trimmed.starts_with("/portal") {
        ReplCommand::Proxy(trimmed.to_string())
    } else if trimmed.starts_with('/') {
        ReplCommand::Unknown(trimmed.to_string())
    } else {
        ReplCommand::Prompt(trimmed.to_string())
    }
}

