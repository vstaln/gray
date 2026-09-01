/// Static slash-command table driving both `/help` and the autocomplete panel.
pub(crate) const COMMANDS: &[(&str, &str)] = &[
    ("connect", "setup provider & API key"),
    ("model", "switch model"),
    ("thinking", "reasoning effort"),
    ("resume", "resume conversation"),
    ("new", "new conversation"),
    ("compact", "summarize context"),
    ("cron", "cron jobs"),
    ("proxy", "share Codex/Grok/OpenRouter via :8645"),
    ("portal", "portal status"),
    ("agentsmd", "edit system prompt"),
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
            if t.starts_with('/') {
                return ReplCommand::Unknown(t.to_string());
            }
            ReplCommand::Prompt(t.to_string())
        }
    }
}

