/// Single slash-command registry driving `/help`, completion, and parsing.
pub(crate) struct CmdDef {
    pub(crate) name: &'static str,
    pub(crate) desc: &'static str,
    pub(crate) aliases: &'static [&'static str],
}

pub(crate) const REGISTRY: &[CmdDef] = &[
    CmdDef {
        name: "connect",
        desc: "setup provider & API key",
        aliases: &["keys", "key", "providers", "provider", "login"],
    },
    CmdDef {
        name: "model",
        desc: "switch model",
        aliases: &[],
    },
    CmdDef {
        name: "thinking",
        desc: "reasoning effort",
        aliases: &["effort", "reasoning"],
    },
    CmdDef {
        name: "context",
        desc: "set context window",
        aliases: &[],
    },
    CmdDef {
        name: "resume",
        desc: "resume conversation",
        aliases: &[],
    },
    CmdDef {
        name: "new",
        desc: "new conversation",
        aliases: &["clear", "reset"],
    },
    CmdDef {
        name: "compact",
        desc: "summarize context",
        aliases: &["compress"],
    },
    CmdDef {
        name: "usage",
        desc: "session tokens & cost",
        aliases: &["cost"],
    },
    CmdDef {
        name: "cron",
        desc: "list cron jobs (read-only)",
        aliases: &[],
    },
    CmdDef {
        name: "copy",
        desc: "copy last assistant response",
        aliases: &[],
    },
    CmdDef {
        name: "doctor",
        desc: "health checks (config, store, provider)",
        aliases: &[],
    },
    CmdDef {
        name: "feedback",
        desc: "send feedback",
        aliases: &[],
    },
    CmdDef {
        name: "agentsmd",
        desc: "edit system prompt",
        aliases: &["sys"],
    },
    CmdDef {
        name: "skills",
        desc: "list skills (/skills [name] [args] to run one)",
        aliases: &["skill"],
    },
    CmdDef {
        name: "plugin",
        desc: "manage plugins",
        aliases: &["plugins"],
    },
    CmdDef {
        name: "marketplace",
        desc: "browse and install plugins/skills",
        aliases: &[],
    },
    CmdDef {
        name: "help",
        desc: "show commands",
        aliases: &[],
    },
    CmdDef {
        name: "quit",
        desc: "exit",
        aliases: &["exit"],
    },
];

/// Help line with aliases inline (Bug2: `/exit` worked but was unlisted;
/// welcome hinted `/provider` while `/help` showed only `/connect`).
/// Single alias → `/quit (alias: /exit)`; several → `/connect (aliases: …)`;
/// no aliases keeps the legacy `  /name desc` shape. Reads the existing
/// `aliases` arrays so `/help` can never drift from `resolve`.
pub(crate) fn format_help_line(d: &CmdDef) -> String {
    if d.aliases.is_empty() {
        format!("  /{:<10} {}", d.name, d.desc)
    } else if d.aliases.len() == 1 {
        format!("  /{} (alias: /{}) {}", d.name, d.aliases[0], d.desc)
    } else {
        let list = d
            .aliases
            .iter()
            .map(|a| format!("/{a}"))
            .collect::<Vec<_>>()
            .join(", ");
        format!("  /{} (aliases: {}) {}", d.name, list, d.desc)
    }
}

/// Canonical lookup: strip one leading `/`, lowercase, exact wins then aliases.
pub(crate) fn resolve(name: &str) -> Option<&'static CmdDef> {
    let n = name.strip_prefix('/').unwrap_or(name).to_lowercase();
    if let Some(d) = REGISTRY.iter().find(|d| d.name == n) {
        return Some(d);
    }
    REGISTRY.iter().find(|d| d.aliases.iter().any(|a| *a == n))
}

/// Commands matching `filter` (the text after '/'), auto-sorted by relevance.
pub(crate) fn completion_matches(filter: &str) -> Vec<(&'static str, &'static str)> {
    let f = filter.to_lowercase();
    let mut matches: Vec<(u8, &'static str, &'static str)> = Vec::new();
    for d in REGISTRY {
        let is_match = f.is_empty()
            || d.name.to_lowercase().contains(&f)
            || d.desc.to_lowercase().contains(&f)
            || d.aliases.iter().any(|a| a.contains(f.as_str()));
        if !is_match {
            continue;
        }
        let nl = d.name.to_lowercase();
        let rank = if nl == f {
            0
        } else if nl.starts_with(&f) {
            1
        } else if d.aliases.contains(&f.as_str()) {
            2
        } else if d.aliases.iter().any(|a| a.starts_with(f.as_str())) {
            3
        } else {
            4
        };
        matches.push((rank, d.name, d.desc));
    }
    matches.sort_by_key(|(rank, _, _)| *rank);
    matches
        .into_iter()
        .map(|(_, name, desc)| (name, desc))
        .collect()
}

/// Completion for the composer prompt: static commands, skill names after
/// `/skills ` (alias `/skill `), or per-command suffixes after `/cmd `
/// (Minecraft-style). Owned here so every read_loop call site stays in sync.
pub(crate) fn completion_matches_dyn(
    cur_text: &str,
    cwd: &std::path::Path,
) -> Vec<(String, String)> {
    if let Some(inner) = cur_text.strip_prefix('/') {
        if let Some(idx) = inner.find(char::is_whitespace) {
            let (cmd, _) = inner.split_at(idx);
            if cmd.is_empty() || cmd.contains(':') {
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

/// Fill text for an accepted popup row. Built-ins and `cmd args` rows
/// fill `/{name} ` as before; a bare skill name fills `/skills {name} ` so
/// the existing skill dispatch runs it — skills never become real
/// top-level commands.
pub(crate) fn completion_fill(name: &str) -> String {
    if name.contains(' ') || resolve(name).is_some() {
        return format!("/{name} ");
    }
    format!("/skills {name} ")
}

/// Universal per-command suffix completion hook.
///
/// `cmd` is lowercased without the leading `/`; `arg_text` is everything
/// after it with leading spaces trimmed (trailing space preserved to detect
/// `/cmd sub ` vs `/cmd sub`). Returns full `cmd + args` names (no slash)
/// so the existing `/{name} ` fill just works. Add new commands here.
///
/// First page (`/cmd ` with an empty arg box) leads with the bare command
/// itself, so Enter runs it instead of forcing a suffix. Filtered and L2
/// pages list suffixes only — a bare row there would wipe the typed args
/// on fill.
pub(crate) fn complete_command_args(
    cmd: &str,
    arg_text: &str,
    cwd: &std::path::Path,
) -> Vec<(String, String)> {
    let mut out = match cmd {
        "context" => complete_context_args(arg_text),
        "plugin" | "plugins" => complete_plugin_args(cmd, arg_text, cwd),
        "thinking" | "effort" | "reasoning" => complete_thinking_args(cmd, arg_text),
        "resume" => complete_resume_args(cmd, arg_text),
        "skill" | "skills" => complete_skill_args(cmd, arg_text, cwd),
        "agentsmd" | "sys" => complete_agentsmd_args(cmd, arg_text),
        "model" => complete_model_args(cmd, arg_text),
        _ => Vec::new(),
    };
    if arg_text.trim().is_empty()
        && let Some(d) = resolve(cmd)
    {
        out.insert(0, (cmd.to_string(), d.desc.to_string()));
    }
    out
}

/// Filter a static `(name, description)` table by the lowercased typed args;
/// empty filter lists every row. Rows fill as `{cmd} {name}`.
fn complete_from_table(cmd: &str, arg_text: &str, table: &[(&str, &str)]) -> Vec<(String, String)> {
    let f = arg_text.to_lowercase();
    table
        .iter()
        .filter(|(s, _)| f.is_empty() || s.contains(f.as_str()))
        .map(|(s, d)| (format!("{cmd} {s}"), d.to_string()))
        .collect()
}

/// Suffixes for `/thinking` (aliases `/effort`, `/reasoning`): global catalog.
/// Direct sets validate against the per-model [`crate::setup::supported_thinking_levels`].
fn complete_thinking_args(cmd: &str, arg_text: &str) -> Vec<(String, String)> {
    complete_from_table(cmd, arg_text, crate::setup::THINKING_LEVELS)
}

/// Suffixes for `/resume`: session picker flags.
fn complete_resume_args(cmd: &str, arg_text: &str) -> Vec<(String, String)> {
    const FLAGS: &[(&str, &str)] = &[
        ("--last", "resume most recent session"),
        ("--all", "include other directories"),
    ];
    complete_from_table(cmd, arg_text, FLAGS)
}

/// Suffixes for `/skills` (alias `/skill`): discovered skill names;
/// rows fill `/skills <name> ` (or `/skill <name> `).
fn complete_skill_args(cmd: &str, arg_text: &str, cwd: &std::path::Path) -> Vec<(String, String)> {
    let f = arg_text.trim().to_lowercase();
    crate::skills::discover_skills(cwd)
        .skills
        .iter()
        .filter(|s| f.is_empty() || s.name.contains(f.as_str()))
        .map(|s| (format!("{cmd} {}", s.name), s.description.clone()))
        .collect()
}

/// Suffixes for `/agentsmd` (alias `/sys`).
fn complete_agentsmd_args(cmd: &str, arg_text: &str) -> Vec<(String, String)> {
    const SUBS: &[(&str, &str)] = &[("show", "print prompt file"), ("reset", "restore default")];
    complete_from_table(cmd, arg_text, SUBS)
}

/// Suffixes for `/model`: ids from the in-memory model cache (empty until
/// models are fetched; the picker covers discovery).
fn complete_model_args(cmd: &str, arg_text: &str) -> Vec<(String, String)> {
    let f = arg_text.to_lowercase();
    crate::setup::cached_model_ids()
        .into_iter()
        .filter(|id| f.is_empty() || id.to_lowercase().contains(&f))
        .map(|id| (format!("{cmd} {id}"), "cached model".to_string()))
        .collect()
}

/// Suffixes for `/plugin` (alias `/plugins`): plugin manager subcommands.
fn complete_plugin_args(
    cmd: &str,
    arg_text: &str,
    _cwd: &std::path::Path,
) -> Vec<(String, String)> {
    const SUBS: &[(&str, &str)] = &[
        ("list", "list installed plugins"),
        ("search", "search Gray Index"),
        ("install", "install a plugin"),
        ("remove", "remove a plugin"),
        ("update", "update plugins"),
        ("enable", "enable a plugin"),
        ("disable", "disable a plugin"),
        ("check", "run conformance checks on a plugin dir"),
    ];
    complete_from_table(cmd, arg_text, SUBS)
}

/// Suffixes for `/context`: L1 (`[number]|auto|status|reserve|keep`) and L2
/// (`reserve <16k|auto>`, `keep <20k|auto|off>`).
fn complete_context_args(arg_text: &str) -> Vec<(String, String)> {
    const L1: &[(&str, &str)] = &[
        ("[number]", "set window"),
        ("auto", "clear override → auto"),
        ("status", "show breakdown"),
        ("reserve", "set reserve…"),
        ("keep", "set keep tail…"),
    ];
    const RESERVE_VALS: &[(&str, &str)] =
        &[("16k", "reserve 16k"), ("auto", "clear reserve → default")];
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
        let vals = if head == "reserve" {
            RESERVE_VALS
        } else {
            KEEP_VALS
        };
        let f = if parts.len() >= 2 {
            parts[1].to_lowercase()
        } else {
            String::new()
        };
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
    /// Set reasoning effort (`/thinking [level]`, `/effort`, `/reasoning`; bare toggles hide/show).
    Thinking(Option<String>),
    /// Print the command list (`/help`).
    Help,
    /// Open the model picker (`/model`) or set directly (`/model provider/id`).
    Model(Option<String>),
    /// Set context window (`/context [128k|auto|reserve 16k|keep 20k|status]`).
    ContextWindow(Option<String>),
    /// Session token + cost totals (`/usage` or `/cost`).
    Usage,
    /// List cron jobs (read-only; manage via `gray cron` CLI).
    CronJobs(Option<String>),
    /// Copy the last assistant response to the clipboard (`/copy`).
    Copy,
    /// Health checks: config, session store, provider reachability (`/doctor`).
    Doctor,
    /// Send feedback (`/feedback <what happened>`): saves locally, opens a prefilled issue.
    Feedback(Option<String>),
    /// Unknown slash command (`/word`).
    Unknown(String),
    /// Plugin manager: /plugin <list|search|install|remove|update|enable|disable|check>.
    /// `/plugins` is an alias.
    Plugin(String),
    /// Store: /marketplace browses+installs plugins/skills.
    Marketplace(String),
    /// Skills: bare /skills lists discovered; /skills <name> [args] (alias /skill <name>) runs one
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

/// Parses a line of input into a [`ReplCommand`]: resolve the first token
/// to its canonical registry name, then match on canonical only.
pub fn parse_command(line: &str) -> ReplCommand {
    let t = line.trim();
    if t.is_empty() {
        return ReplCommand::Empty;
    }
    if !t.starts_with('/') {
        return ReplCommand::Prompt(t.to_string());
    }
    let (cmd, rest) = match t.split_once(' ') {
        Some((c, r)) => (c, r.trim()),
        None => (t, ""),
    };
    let opt = |s: &str| (!s.is_empty()).then(|| s.to_string());
    let lower_t = t.to_lowercase();
    let lower_cmd = cmd.to_lowercase();
    let canon: Option<&str> = if lower_cmd == "/skills" || lower_cmd == "/skill" {
        Some("skills")
    } else if let Some(d) = resolve(cmd) {
        Some(d.name)
    } else if lower_t.starts_with("/model") {
        Some("model")
    } else {
        None
    };
    match canon {
        Some("quit") => ReplCommand::Quit,
        Some("resume") => ReplCommand::Resume(if rest.is_empty() {
            ResumeArgs {
                target: None,
                last: false,
                all: false,
            }
        } else {
            parse_resume_args(rest)
        }),
        Some("agentsmd") => match rest {
            "" => ReplCommand::Sys(SysAction::Edit),
            "show" => ReplCommand::Sys(SysAction::Show),
            "reset" => ReplCommand::Sys(SysAction::Reset),
            _ => ReplCommand::Unknown(t.to_string()),
        },
        Some("new") => ReplCommand::New(opt(rest)),
        Some("compact") => ReplCommand::Compact(opt(rest)),
        Some("thinking") => ReplCommand::Thinking(opt(rest)),
        Some("context") => ReplCommand::ContextWindow(opt(rest)),
        Some("usage") => ReplCommand::Usage,
        Some("cron") => ReplCommand::CronJobs(opt(rest)),
        Some("copy") => ReplCommand::Copy,
        Some("doctor") => ReplCommand::Doctor,
        Some("feedback") => ReplCommand::Feedback(opt(rest)),
        Some("help") => ReplCommand::Help,
        // Every connect alias accepts optional args like `/key openrouter`
        // (args are advisory; the provider menu always opens).
        Some("connect") => ReplCommand::Provider,
        Some("model") => ReplCommand::Model(opt(t[6..].trim())),
        Some("plugin") => ReplCommand::Plugin(t.to_string()),
        Some("marketplace") => ReplCommand::Marketplace(t.to_string()),
        Some("skills") => {
            if rest.is_empty() {
                ReplCommand::Skill(None)
            } else {
                // `/skills <name> [args]` (or the `/skill` alias): identical
                // payload shape so expansion/validation match exactly.
                ReplCommand::Skill(Some(rest.to_string()))
            }
        }
        _ => {
            // Codex port: a slash-name containing '/' is plain text, not an
            // unknown command — e.g. `///` doc comments, `//` comments,
            // `/tmp/foo` paths. Bare `/` (empty name) is text too.
            let name = t[1..].split_whitespace().next().unwrap_or("");
            if name.is_empty() || name.contains('/') {
                ReplCommand::Prompt(t.to_string())
            } else {
                ReplCommand::Unknown(t.to_string())
            }
        }
    }
}

#[path = "commands_tests.rs"]
#[cfg(test)]
mod tests;
