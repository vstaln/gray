/// Single slash-command registry driving `/help`, completion, and parsing.
/// `portal` is both a canonical row (display) and a `proxy` alias (dispatch
/// shares Proxy) — resolve() prefers the canonical exact match.
pub(crate) struct CmdDef {
    pub(crate) name: &'static str,
    pub(crate) desc: &'static str,
    pub(crate) aliases: &'static [&'static str],
    #[allow(dead_code)] // reserved for future per-command hints; empty keeps /help byte-identical
    pub(crate) args_hint: &'static str,
}

pub(crate) const REGISTRY: &[CmdDef] = &[
    CmdDef { name: "connect", desc: "setup provider & API key", aliases: &["keys", "key", "providers", "provider", "login"], args_hint: "" },
    CmdDef { name: "model", desc: "switch model", aliases: &[], args_hint: "" },
    CmdDef { name: "thinking", desc: "reasoning effort", aliases: &["effort", "reasoning"], args_hint: "" },
    CmdDef { name: "context", desc: "set context window", aliases: &[], args_hint: "" },
    CmdDef { name: "resume", desc: "resume conversation", aliases: &[], args_hint: "" },
    CmdDef { name: "new", desc: "new conversation", aliases: &["clear", "reset"], args_hint: "" },
    CmdDef { name: "compact", desc: "summarize context", aliases: &["compress"], args_hint: "" },
    CmdDef { name: "usage", desc: "session tokens & cost", aliases: &["cost"], args_hint: "" },
    CmdDef { name: "cron", desc: "cron jobs", aliases: &[], args_hint: "" },
    CmdDef { name: "proxy", desc: "share Codex/Grok/OpenRouter via :8645", aliases: &["portal"], args_hint: "" },
    CmdDef { name: "gateway", desc: "messaging gateway (Discord)", aliases: &["gw"], args_hint: "" },
    CmdDef { name: "portal", desc: "portal status", aliases: &[], args_hint: "" },
    CmdDef { name: "agentsmd", desc: "edit system prompt", aliases: &["sys"], args_hint: "" },
    CmdDef { name: "skills", desc: "list skills (/skills:<name> [args] to run one)", aliases: &[], args_hint: "" },
    CmdDef { name: "help", desc: "show commands", aliases: &[], args_hint: "" },
    CmdDef { name: "quit", desc: "exit", aliases: &["exit"], args_hint: "" },
];

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
        } else if REGISTRY.iter().any(|d| d.name == *n && d.aliases.contains(&f.as_str())) {
            2
        } else if REGISTRY.iter().any(|d| d.name == *n && d.aliases.iter().any(|a| a.starts_with(f.as_str()))) {
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
    let canon: Option<&str> = if lower_cmd == "/skills" || lower_t.starts_with("/skills:") {
        Some("skills")
    } else if let Some(d) = resolve(cmd) {
        Some(d.name)
    } else if lower_t.starts_with("/model") {
        Some("model")
    } else if lower_t.starts_with("/cron") {
        Some("cron")
    } else if lower_t.starts_with("/proxy") {
        Some("proxy")
    } else if lower_t.starts_with("/portal") {
        Some("portal")
    } else if lower_t.starts_with("/gateway") || lower_t.starts_with("/gw") {
        Some("gateway")
    } else {
        None
    };
    match canon {
        Some("quit") => ReplCommand::Quit,
        Some("resume") => ReplCommand::Resume(if rest.is_empty() {
            ResumeArgs { target: None, last: false, all: false }
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
        Some("help") => ReplCommand::Help,
        // Bare connect aliases exact; only `/key ...` carries args (legacy edge).
        Some("connect") => {
            if rest.is_empty() || lower_cmd == "/key" {
                ReplCommand::Provider
            } else {
                ReplCommand::Unknown(t.to_string())
            }
        }
        Some("model") => ReplCommand::Model(opt(t[6..].trim())),
        Some("cron") => ReplCommand::Cron(t.to_string()),
        Some("proxy") | Some("portal") => ReplCommand::Proxy(t.to_string()),
        Some("gateway") => ReplCommand::Gateway(t.to_string()),
        Some("skills") => {
            if lower_t == "/skills" {
                ReplCommand::Skill(None)
            } else if lower_t.starts_with("/skills:") {
                ReplCommand::Skill(Some(t[8..].to_string()))
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
    fn thinking_effort_and_reasoning_aliases() {
        assert!(matches!(parse_command("/thinking"), ReplCommand::Thinking(None)));
        assert!(matches!(parse_command("/effort"), ReplCommand::Thinking(None)));
        assert!(matches!(parse_command("/reasoning"), ReplCommand::Thinking(None)));
        assert!(matches!(parse_command("/reasoning max"), ReplCommand::Thinking(Some(_))));
        // `reasoning` resolves through the alias table
        assert!(super::completion_matches("reasoning").iter().any(|(n, _)| *n == "thinking"));
    }

    #[test]
    fn registry_resolve_canonical_and_aliases() {
        for name in [
            "connect", "model", "thinking", "context", "resume", "new", "compact", "usage",
            "cron", "proxy", "gateway", "portal", "agentsmd", "skills", "help", "quit",
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
            ("gw", "gateway"),
            ("cost", "usage"),
        ] {
            assert_eq!(super::resolve(alias).unwrap().name, target, "alias {alias}");
            assert_eq!(super::resolve(&format!("/{alias}")).unwrap().name, target);
        }
        // `portal` is both a canonical command and a legacy alias for `proxy`;
        // canonical exact wins in resolve(), dispatch still maps both to Proxy.
        assert_eq!(super::resolve("portal").unwrap().name, "portal");
        assert!(
            super::REGISTRY
                .iter()
                .find(|d| d.name == "proxy")
                .unwrap()
                .aliases
                .contains(&"portal")
        );
        assert!(super::resolve("boguscmd").is_none());
        assert!(super::resolve("/boguscmd").is_none());
        assert!(super::resolve("").is_none());
        assert!(super::resolve("/").is_none());
    }

    #[test]
    fn registry_help_covers_all_commands() {
        let names: Vec<_> = super::REGISTRY.iter().map(|d| d.name).collect();
        for expected in [
            "connect", "model", "thinking", "context", "resume", "new", "compact", "usage",
            "cron", "proxy", "gateway", "portal", "agentsmd", "skills", "help", "quit",
        ] {
            assert!(names.contains(&expected), "help missing {expected}");
        }
        assert_eq!(super::REGISTRY.len(), 16);
        // args_hint reserved for future per-command hints; empty keeps /help byte-identical.
        assert!(super::REGISTRY.iter().all(|d| d.args_hint.is_empty()));
        let all = super::completion_matches("");
        assert_eq!(all.len(), 16);
        for expected in names {
            assert!(all.iter().any(|(n, _)| *n == expected));
        }
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
            ("portal", "proxy"),
            ("gw", "gateway"),
            ("cost", "usage"),
        ] {
            assert!(
                super::completion_matches(alias).iter().any(|(n, _)| *n == target),
                "completion {alias} -> {target}"
            );
        }
        let m = super::completion_matches("portal");
        assert!(m.iter().any(|(n, _)| *n == "portal"));
        assert!(m.iter().any(|(n, _)| *n == "proxy"));
    }

    #[test]
    fn registry_parse_uses_canonical() {
        assert!(matches!(parse_command("/cost"), ReplCommand::Usage));
        assert!(matches!(parse_command("/COST"), ReplCommand::Usage));
        assert!(matches!(parse_command("/exit"), ReplCommand::Quit));
        assert!(matches!(parse_command("/portal"), ReplCommand::Proxy(_)));
        assert!(matches!(parse_command("/gw"), ReplCommand::Gateway(_)));
        assert!(matches!(parse_command("/keys foo"), ReplCommand::Unknown(_)));
        assert!(matches!(parse_command("/connect foo"), ReplCommand::Unknown(_)));
        assert!(matches!(parse_command("/key foo"), ReplCommand::Provider));
        assert!(matches!(parse_command("/skills foo"), ReplCommand::Unknown(_)));
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

