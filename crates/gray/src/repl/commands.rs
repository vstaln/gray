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
        name: "permissions",
        desc: "choose what gray is allowed to do",
        aliases: &["perms", "access"],
    },
    CmdDef {
        name: "feedback",
        desc: "send feedback",
        aliases: &[],
    },
    CmdDef {
        name: "acp",
        desc: "run as an external ACP agent (claude, codex, cursor…)",
        aliases: &[],
    },
    CmdDef {
        name: "agentsmd",
        desc: "edit system prompt",
        aliases: &["sys"],
    },
    CmdDef {
        name: "skills",
        desc: "manage installed skills (/skill <name> [args] or /skills:<name> [args] to run one)",
        aliases: &[],
    },
    CmdDef {
        name: "plugin",
        desc: "manage plugins",
        aliases: &["plugins"],
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
    let mut matches: Vec<(&'static str, &'static str)> = Vec::new();
    for d in REGISTRY {
        let is_match = f.is_empty()
            || d.name.to_lowercase().contains(&f)
            || d.desc.to_lowercase().contains(&f)
            || d.aliases.iter().any(|a| a.contains(f.as_str()));
        if is_match {
            matches.push((d.name, d.desc));
        }
    }
    matches.sort_by_key(|(n, _)| {
        let nl = n.to_lowercase();
        if nl == f {
            0
        } else if nl.starts_with(&f) {
            1
        } else if REGISTRY
            .iter()
            .any(|d| d.name == *n && d.aliases.contains(&f.as_str()))
        {
            2
        } else if REGISTRY
            .iter()
            .any(|d| d.name == *n && d.aliases.iter().any(|a| a.starts_with(f.as_str())))
        {
            3
        } else {
            4
        }
    });
    matches
}

/// Completion for the composer prompt: static commands, skill names after
/// `/skills:` or `/skill `, or per-command suffixes after `/cmd ` (Minecraft-style).
/// Owned here so every read_loop call site stays in sync.
pub(crate) fn completion_matches_dyn(
    cur_text: &str,
    cwd: &std::path::Path,
) -> Vec<(String, String)> {
    if cur_text.starts_with("/skills:") && !cur_text[8..].contains(char::is_whitespace) {
        let filter = &cur_text[8..];
        return crate::skills::discover_skills(cwd)
            .skills
            .iter()
            .filter(|s| s.name.contains(filter))
            .map(|s| (format!("skills:{}", s.name), s.description.clone()))
            .collect();
    }
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

/// Fill text for an accepted popup row. Built-ins, aliases and `cmd args`
/// rows fill `/{name} ` as before; anything else is a skill name and fills
/// `/skills:{name} ` so the existing skill dispatch runs it — skills never
/// become real top-level commands.
pub(crate) fn completion_fill(name: &str) -> String {
    if name == "skills" {
        return "/skills:".to_string();
    }
    if name.contains([' ', ':']) || resolve(name).is_some() {
        return format!("/{name} ");
    }
    format!("/skills:{name} ")
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
        "acp" => complete_acp_args(arg_text),
        "plugin" | "plugins" => complete_plugin_args(cmd, arg_text, cwd),
        "permissions" | "perms" | "access" => complete_permissions_args(cmd, arg_text),
        "thinking" | "effort" | "reasoning" => complete_thinking_args(cmd, arg_text),
        "resume" => complete_resume_args(cmd, arg_text),
        "skill" => complete_skill_args(cmd, arg_text, cwd),
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

/// Suffixes for `/thinking` (aliases `/effort`, `/reasoning`): levels from
/// [`crate::setup::THINKING_LEVELS`], the exact set `handle_thinking` accepts.
fn complete_thinking_args(cmd: &str, arg_text: &str) -> Vec<(String, String)> {
    let f = arg_text.to_lowercase();
    crate::setup::THINKING_LEVELS
        .iter()
        .filter(|(l, _)| f.is_empty() || l.contains(f.as_str()))
        .map(|(l, d)| (format!("{cmd} {l}"), d.to_string()))
        .collect()
}

/// Suffixes for `/resume`: session picker flags.
fn complete_resume_args(cmd: &str, arg_text: &str) -> Vec<(String, String)> {
    const FLAGS: &[(&str, &str)] = &[
        ("--last", "resume most recent session"),
        ("--all", "include other directories"),
    ];
    let f = arg_text.to_lowercase();
    FLAGS
        .iter()
        .filter(|(s, _)| f.is_empty() || s.contains(f.as_str()))
        .map(|(s, d)| (format!("{cmd} {s}"), d.to_string()))
        .collect()
}

/// Suffixes for `/skill`: installed skill names (space-separated alias for
/// the `/skills:<name>` prefix form; rows fill `/skill <name> `).
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
    let f = arg_text.to_lowercase();
    SUBS.iter()
        .filter(|(s, _)| f.is_empty() || s.contains(f.as_str()))
        .map(|(s, d)| (format!("{cmd} {s}"), d.to_string()))
        .collect()
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

/// Suffixes for `/acp`: subcommands plus installed agent names.
fn complete_acp_args(arg_text: &str) -> Vec<(String, String)> {
    #[cfg_attr(not(feature = "acp"), allow(unused_mut))]
    let mut out: Vec<(String, String)> = vec![
        ("acp list".to_string(), "list agents".to_string()),
        ("acp status".to_string(), "show ACP session".to_string()),
        ("acp off".to_string(), "back to native".to_string()),
    ];
    #[cfg(feature = "acp")]
    for spec in gray_acp::all_specs(None) {
        let status = if gray_acp::installed(&spec) {
            "installed"
        } else {
            "not found"
        };
        out.push((
            format!("acp {}", spec.key),
            format!("{} ({status})", spec.display),
        ));
    }
    let f = arg_text.to_lowercase();
    out.into_iter()
        .filter(|(n, _)| f.is_empty() || n.to_lowercase().contains(&f))
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
        ("install", "install a plugin"),
        ("remove", "remove a plugin"),
        ("update", "update plugins"),
        ("enable", "enable a plugin"),
        ("disable", "disable a plugin"),
        ("check", "run conformance checks on a plugin dir"),
    ];
    let f = arg_text.to_lowercase();
    SUBS.iter()
        .filter(|(s, _)| f.is_empty() || s.contains(f.as_str()))
        .map(|(s, d)| (format!("{cmd} {s}"), d.to_string()))
        .collect()
}

/// Suffixes for `/permissions` (aliases `/perms`, `/access`): approval modes.
fn complete_permissions_args(cmd: &str, arg_text: &str) -> Vec<(String, String)> {
    gray_core::approvals::permission_modes()
        .into_iter()
        .filter(|(id, _, _)| arg_text.is_empty() || id.contains(&arg_text.to_lowercase()))
        .map(|(id, label, _)| (format!("{cmd} {id}"), label.to_string()))
        .collect()
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
    /// Choose what gray is allowed to do (`/permissions [read-only|auto|full]`).
    Permissions(Option<String>),
    /// Send feedback (`/feedback <what happened>`): saves locally, opens a prefilled issue.
    Feedback(Option<String>),
    /// Unknown slash command (`/word`).
    Unknown(String),
    /// External ACP agent: /acp (picker), /acp <agent> switches sticky,
    /// /acp <agent> <prompt> delegates one-shot, /acp off|status|list
    Acp(String),
    /// Plugin manager: /plugin <list|install|remove|update|enable|disable|check>.
    /// `/plugins` is an alias.
    Plugin(String),
    /// Skills: /skills manages installed; /skills:<name> [args] or /skill <name> [args] runs a skill
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
    let canon: Option<&str> =
        if lower_cmd == "/skills" || lower_cmd == "/skill" || lower_t.starts_with("/skills:") {
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
        Some("permissions") => ReplCommand::Permissions(opt(rest)),
        Some("feedback") => ReplCommand::Feedback(opt(rest)),
        Some("help") => ReplCommand::Help,
        // Every connect alias accepts optional args like `/key openrouter`
        // (args are advisory; the provider menu always opens).
        Some("connect") => ReplCommand::Provider,
        Some("model") => ReplCommand::Model(opt(t[6..].trim())),
        Some("acp") => ReplCommand::Acp(t.to_string()),
        Some("plugin") => ReplCommand::Plugin(t.to_string()),
        Some("skills") => {
            if lower_t == "/skills" || lower_t == "/skill" {
                ReplCommand::Skill(None)
            } else if lower_t.starts_with("/skills:") {
                ReplCommand::Skill(Some(t[8..].to_string()))
            } else if lower_cmd == "/skill" {
                // Singular space-separated alias: identical payload shape as
                // `/skills:<name> [args]` so expansion/validation match exactly.
                ReplCommand::Skill(Some(rest.to_string()))
            } else {
                ReplCommand::Unknown(t.to_string())
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

#[cfg(test)]
mod tests {
    use super::{ReplCommand, parse_command};
    #[test]
    fn slash_name_with_slash_is_plain_prompt_like_codex() {
        // Reported bug: pasting Rust `///` doc comments said "unknown command".
        let pasted = "/// Default system prompt, shipped as markdown and materialized to `~/.gray/sys.md`\n/// on first run.";
        assert!(matches!(parse_command(pasted), ReplCommand::Prompt(_)));
        assert!(matches!(
            parse_command("// comment"),
            ReplCommand::Prompt(_)
        ));
        assert!(matches!(parse_command("/tmp/foo"), ReplCommand::Prompt(_)));
        assert!(matches!(parse_command("/"), ReplCommand::Prompt(_)));
        // Genuinely unknown single-token commands still error.
        assert!(matches!(
            parse_command("/boguscmd"),
            ReplCommand::Unknown(_)
        ));
        // Known commands unaffected.
        assert!(matches!(parse_command("/help"), ReplCommand::Help));
        assert!(matches!(parse_command("/model foo"), ReplCommand::Model(_)));
    }

    #[test]
    fn context_renamed_no_window_alias() {
        assert!(matches!(
            parse_command("/context"),
            ReplCommand::ContextWindow(None)
        ));
        assert!(matches!(
            parse_command("/context 128k"),
            ReplCommand::ContextWindow(Some(_))
        ));
        assert!(matches!(
            parse_command("/context-window"),
            ReplCommand::Unknown(_)
        ));
    }

    #[test]
    fn usage_command_and_cost_alias() {
        assert!(matches!(parse_command("/usage"), ReplCommand::Usage));
        assert!(matches!(parse_command("/cost"), ReplCommand::Usage));
        use std::path::Path;
        let cwd = Path::new(".");
        assert!(
            super::completion_matches_dyn("/us", cwd)
                .iter()
                .any(|(n, _)| n == "usage")
        );
        // `cost` resolves through the alias table
        assert!(
            super::completion_matches("cost")
                .iter()
                .any(|(n, _)| *n == "usage")
        );
    }

    #[test]
    fn thinking_effort_and_reasoning_aliases() {
        assert!(matches!(
            parse_command("/thinking"),
            ReplCommand::Thinking(None)
        ));
        assert!(matches!(
            parse_command("/effort"),
            ReplCommand::Thinking(None)
        ));
        assert!(matches!(
            parse_command("/reasoning"),
            ReplCommand::Thinking(None)
        ));
        assert!(matches!(
            parse_command("/reasoning max"),
            ReplCommand::Thinking(Some(_))
        ));
        // `reasoning` resolves through the alias table
        assert!(
            super::completion_matches("reasoning")
                .iter()
                .any(|(n, _)| *n == "thinking")
        );
    }

    #[test]
    fn empty_prompt_hides_slash_popup_like_codex() {
        // codex `command_under_cursor`: empty text / no leading slash / cursor
        // past the command name → no popup. Deleting `/` must close it, not
        // strand stale matches (ghost popup + double footer + scrollback growth).
        use std::path::Path;
        let cwd = Path::new(".");
        assert!(super::completion_matches_dyn("", cwd).is_empty());
        assert!(super::completion_matches_dyn("hello", cwd).is_empty());
        assert!(super::completion_matches_dyn("/ ", cwd).is_empty());
        // bare `/` opens the popup with every command.
        assert_eq!(
            super::completion_matches_dyn("/", cwd).len(),
            super::REGISTRY.len()
        );
    }

    #[test]
    fn registry_resolve_canonical_and_aliases() {
        for name in [
            "connect",
            "model",
            "thinking",
            "context",
            "resume",
            "new",
            "compact",
            "usage",
            "permissions",
            "feedback",
            "acp",
            "agentsmd",
            "skills",
            "plugin",
            "help",
            "quit",
        ] {
            let d = super::resolve(name).unwrap_or_else(|| panic!("resolve {name}"));
            assert_eq!(d.name, name);
            assert_eq!(super::resolve(&format!("/{name}")).unwrap().name, name);
            assert_eq!(super::resolve(&name.to_uppercase()).unwrap().name, name);
        }
        for (alias, target) in [
            ("clear", "new"),
            ("reset", "new"),
            ("exit", "quit"),
            ("keys", "connect"),
            ("key", "connect"),
            ("providers", "connect"),
            ("provider", "connect"),
            ("login", "connect"),
            ("effort", "thinking"),
            ("reasoning", "thinking"),
            ("compress", "compact"),
            ("sys", "agentsmd"),
            ("cost", "usage"),
            ("perms", "permissions"),
            ("access", "permissions"),
            ("plugins", "plugin"),
        ] {
            assert_eq!(super::resolve(alias).unwrap().name, target, "alias {alias}");
            assert_eq!(super::resolve(&format!("/{alias}")).unwrap().name, target);
        }
        assert!(super::resolve("yolo").is_none());
        assert!(super::resolve("/yolo").is_none());
        assert!(super::resolve("boguscmd").is_none());
        assert!(super::resolve("/boguscmd").is_none());
        assert!(super::resolve("").is_none());
        assert!(super::resolve("/").is_none());
    }

    #[test]
    fn registry_completion_covers_aliases() {
        for (alias, target) in [
            ("clear", "new"),
            ("reset", "new"),
            ("exit", "quit"),
            ("keys", "connect"),
            ("key", "connect"),
            ("providers", "connect"),
            ("provider", "connect"),
            ("login", "connect"),
            ("effort", "thinking"),
            ("reasoning", "thinking"),
            ("compress", "compact"),
            ("sys", "agentsmd"),
            ("cost", "usage"),
            ("perms", "permissions"),
            ("access", "permissions"),
            ("plugins", "plugin"),
        ] {
            assert!(
                super::completion_matches(alias)
                    .iter()
                    .any(|(n, _)| *n == target),
                "completion {alias} -> {target}"
            );
        }
        // `/plug` surfaces `plugin`; bare `/plugin ` leads with itself + all 7 subcommands.
        use std::path::Path;
        let cwd = Path::new(".");
        assert!(
            super::completion_matches_dyn("/plug", cwd)
                .iter()
                .any(|(n, _)| n == "plugin")
        );
        let plugin_all = super::complete_command_args("plugin", "", cwd);
        assert_eq!(plugin_all.len(), 8);
        assert_eq!(plugin_all[0].0, "plugin");
    }

    #[test]
    fn registry_parse_uses_canonical() {
        assert!(matches!(parse_command("/cost"), ReplCommand::Usage));
        assert!(matches!(parse_command("/COST"), ReplCommand::Usage));
        assert!(matches!(
            parse_command("/plugin list"),
            ReplCommand::Plugin(_)
        ));
        assert!(matches!(
            parse_command("/plugins list"),
            ReplCommand::Plugin(_)
        ));
        assert!(matches!(
            parse_command("/PLUGIN list"),
            ReplCommand::Plugin(_)
        ));
        assert!(matches!(
            parse_command("/marketplace"),
            ReplCommand::Unknown(_)
        ));
        assert!(matches!(parse_command("/exit"), ReplCommand::Quit));
        // gateway left the TUI: /gateway and /gw are unknown.
        assert!(matches!(parse_command("/gw"), ReplCommand::Unknown(_)));
        assert!(matches!(
            parse_command("/gateway status"),
            ReplCommand::Unknown(_)
        ));
        assert!(matches!(parse_command("/keys foo"), ReplCommand::Provider));
        assert!(matches!(
            parse_command("/connect foo"),
            ReplCommand::Provider
        ));
        assert!(matches!(parse_command("/key foo"), ReplCommand::Provider));
        assert!(matches!(
            parse_command("/provider openrouter"),
            ReplCommand::Provider
        ));
        assert!(matches!(
            parse_command("/login openrouter"),
            ReplCommand::Provider
        ));
        assert!(matches!(
            parse_command("/skills foo"),
            ReplCommand::Unknown(_)
        ));
    }

    #[test]
    fn permissions_parses_with_and_without_mode() {
        assert!(matches!(
            parse_command("/permissions"),
            ReplCommand::Permissions(None)
        ));
        assert!(matches!(
            parse_command("/permissions full"),
            ReplCommand::Permissions(Some(_))
        ));
        assert!(matches!(
            parse_command("/perms read-only"),
            ReplCommand::Permissions(Some(_))
        ));
        assert!(matches!(
            parse_command("/access"),
            ReplCommand::Permissions(None)
        ));
        assert!(matches!(
            parse_command("/access full"),
            ReplCommand::Permissions(Some(_))
        ));
        assert!(matches!(parse_command("/yolo"), ReplCommand::Unknown(_)));
    }

    #[test]
    fn feedback_parses_with_and_without_text() {
        assert!(matches!(
            parse_command("/feedback"),
            ReplCommand::Feedback(None)
        ));
        assert!(matches!(
            parse_command("/feedback broken x"),
            ReplCommand::Feedback(Some(_))
        ));
        assert!(matches!(
            parse_command("/FEEDBACK hi"),
            ReplCommand::Feedback(Some(_))
        ));
    }

    #[test]
    fn context_arg_completion_levels() {
        use super::{complete_command_args, completion_matches_dyn};
        use std::path::Path;
        let cwd = Path::new(".");
        // bare suffix lists everything, led by the command itself
        let all = complete_command_args("context", "", cwd);
        assert_eq!(all[0].0, "context");
        assert!(all.iter().any(|(n, _)| n == "context reserve"));
        assert!(all.iter().any(|(n, _)| n == "context auto"));
        // filtered L1 and L2 pages list suffixes only (bare row would wipe args on fill)
        let r = complete_command_args("context", "r", cwd);
        assert!(!r.iter().any(|(n, _)| n == "context"));
        assert!(r.iter().any(|(n, _)| n == "context reserve"));
        // L2 after `reserve `
        let r2 = complete_command_args("context", "reserve ", cwd);
        assert!(!r2.iter().any(|(n, _)| n == "context"));
        assert!(r2.iter().any(|(n, _)| n == "context reserve 16k"));
        // unknown command has no suffixes (universal hook default)
        assert!(complete_command_args("boguscmd", "", cwd).is_empty());
        // dyn dispatch through the composer entry point
        let dyn_all = completion_matches_dyn("/context ", cwd);
        assert_eq!(dyn_all[0].0, "context");
        assert!(dyn_all.iter().any(|(n, _)| n == "context reserve"));
        // command-name path unaffected
        assert!(
            completion_matches_dyn("/cont", cwd)
                .iter()
                .any(|(n, _)| n == "context")
        );
    }

    fn temp_skill_cwd(name: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join(".gray").join("skills").join(name);
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\ndescription: Temp skill for completion tests\n---\n# temp\n",
        )
        .unwrap();
        dir
    }

    #[test]
    fn top_level_query_never_surfaces_skills() {
        use super::completion_matches_dyn;
        let dir = temp_skill_cwd("commit");
        let cwd = dir.path();
        // sanity: the skill is discoverable
        assert!(
            crate::skills::discover_skills(cwd)
                .skills
                .iter()
                .any(|s| s.name == "commit")
        );
        // top-level `/` completion must not surface it…
        let top = completion_matches_dyn("/com", cwd);
        assert!(
            !top.iter().any(|(n, _)| n == "commit"),
            "skill must not appear in / completion: {top:?}"
        );
        // …but the /skills: prefix still completes it
        let scoped = completion_matches_dyn("/skills:com", cwd);
        assert!(
            scoped.iter().any(|(n, _)| n == "skills:commit"),
            "skill must complete under /skills:: {scoped:?}"
        );
    }

    #[test]
    fn thinking_effort_arg_completion() {
        use super::complete_command_args;
        use std::path::Path;
        let cwd = Path::new(".");
        for cmd in ["thinking", "effort", "reasoning"] {
            let all = complete_command_args(cmd, "", cwd);
            for level in ["off", "minimal", "low", "medium", "high", "xhigh", "max"] {
                assert!(
                    all.iter().any(|(n, _)| n == &format!("{cmd} {level}")),
                    "{cmd} must complete level {level}: {all:?}"
                );
            }
            let f = complete_command_args(cmd, "hi", cwd);
            assert!(f.iter().any(|(n, _)| n == &format!("{cmd} high")));
            assert!(f.iter().any(|(n, _)| n == &format!("{cmd} xhigh")));
        }
    }

    #[test]
    fn resume_and_agentsmd_arg_completion() {
        use super::complete_command_args;
        use std::path::Path;
        let cwd = Path::new(".");
        let r = complete_command_args("resume", "", cwd);
        assert!(r.iter().any(|(n, _)| n == "resume --last"));
        assert!(r.iter().any(|(n, _)| n == "resume --all"));
        for cmd in ["agentsmd", "sys"] {
            let a = complete_command_args(cmd, "", cwd);
            assert!(a.iter().any(|(n, _)| n == &format!("{cmd} show")));
            assert!(a.iter().any(|(n, _)| n == &format!("{cmd} reset")));
        }
    }

    #[test]
    fn model_completes_cached_ids() {
        use super::complete_command_args;
        use std::path::Path;
        let cwd = Path::new(".");
        crate::setup::cache_model_context("test-completion-model-xyz", 128000);
        let rows = complete_command_args("model", "", cwd);
        assert!(
            rows.iter()
                .any(|(n, _)| n == "model test-completion-model-xyz"),
            "cached model id must complete: {rows:?}"
        );
        let filtered = complete_command_args("model", "xyz", cwd);
        assert!(
            filtered
                .iter()
                .any(|(n, _)| n == "model test-completion-model-xyz")
        );
        // an impossible filter still yields nothing (deterministic even
        // when other tests pollute the process-global cache)
        assert!(complete_command_args("model", "no-such-model-xyz-123", cwd).is_empty());
    }

    #[test]
    fn skill_singular_is_alias_for_skills_colon() {
        use super::super::handlers::expand_skill_command;
        use super::completion_matches_dyn;
        let dir = temp_skill_cwd("commit");
        let cwd = dir.path();
        // Parse parity: identical payloads.
        assert_eq!(
            parse_command("/skill commit"),
            parse_command("/skills:commit")
        );
        assert_eq!(
            parse_command("/skill commit extra"),
            parse_command("/skills:commit extra")
        );
        assert_eq!(parse_command("/skill"), parse_command("/skills"));
        assert_eq!(parse_command("/SKILL"), parse_command("/skills"));
        assert_eq!(
            parse_command("/SKILL commit"),
            parse_command("/skills:commit")
        );
        // Expansion parity: same Prompt out.
        let a = expand_skill_command(parse_command("/skill commit"), cwd, None, false);
        let b = expand_skill_command(parse_command("/skills:commit"), cwd, None, false);
        assert!(matches!(a, ReplCommand::Prompt(_)));
        assert_eq!(a, b);
        // Bad args fail identically (skill takes no args): both expand to Empty.
        let a = expand_skill_command(parse_command("/skill commit bogus-arg"), cwd, None, false);
        let b = expand_skill_command(parse_command("/skills:commit bogus-arg"), cwd, None, false);
        assert_eq!(a, ReplCommand::Empty);
        assert_eq!(a, b);
        // Unknown skill fails identically.
        let a = expand_skill_command(parse_command("/skill nope"), cwd, None, false);
        let b = expand_skill_command(parse_command("/skills:nope"), cwd, None, false);
        assert_eq!(a, ReplCommand::Empty);
        assert_eq!(a, b);
        // `/skill <partial>` completes installed skill names…
        let rows = completion_matches_dyn("/skill com", cwd);
        assert!(
            rows.iter().any(|(n, _)| n == "skill commit"),
            "skill must complete under /skill : {rows:?}"
        );
        let rows = completion_matches_dyn("/skill ", cwd);
        assert!(rows.iter().any(|(n, _)| n == "skill commit"));
        // …while `/skills foo` (plural + space) stays unknown, as before.
        assert!(matches!(
            parse_command("/skills foo"),
            ReplCommand::Unknown(_)
        ));
    }
}
