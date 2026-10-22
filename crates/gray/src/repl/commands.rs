pub(crate) struct CmdDef {
    pub(crate) name: &'static str,
    pub(crate) desc: &'static str,
    pub(crate) aliases: &'static [&'static str],

    pub(crate) scope: CmdScope,

    pub(crate) surface: CmdSurface,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CmdScope {
    Conversation,
    Global,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CmdSurface {
    Interactive,
    Text,
    Local,
}

pub(crate) const REGISTRY: &[CmdDef] = &[
    CmdDef {
        name: "models",
        desc: "switch provider, model & effort",
        aliases: &["model", "connect", "provider"],
        scope: CmdScope::Conversation,
        surface: CmdSurface::Interactive,
    },
    CmdDef {
        name: "context",
        desc: "set context window",
        aliases: &[],
        scope: CmdScope::Conversation,
        surface: CmdSurface::Text,
    },
    CmdDef {
        name: "sessions",
        desc: "browse, resume & manage past sessions",
        aliases: &["resume"],
        scope: CmdScope::Conversation,
        surface: CmdSurface::Text,
    },
    CmdDef {
        name: "new",
        desc: "new conversation",
        aliases: &["clear", "reset"],
        scope: CmdScope::Conversation,
        surface: CmdSurface::Text,
    },
    CmdDef {
        name: "compact",
        desc: "summarize context",
        aliases: &["compress"],
        scope: CmdScope::Conversation,
        surface: CmdSurface::Text,
    },
    CmdDef {
        name: "undo",
        desc: "drop the last exchange",
        aliases: &[],
        scope: CmdScope::Conversation,
        surface: CmdSurface::Text,
    },
    CmdDef {
        name: "retry",
        desc: "drop the last exchange and ask again",
        aliases: &[],
        scope: CmdScope::Conversation,
        surface: CmdSurface::Text,
    },
    CmdDef {
        name: "update",
        desc: "install a new gray build (everything: + plugins & provider CLIs)",
        aliases: &["upgrade"],
        scope: CmdScope::Global,
        surface: CmdSurface::Text,
    },
    CmdDef {
        name: "restart",
        desc: "restart the gateway & this session on the new build",
        aliases: &["reboot"],
        scope: CmdScope::Global,
        surface: CmdSurface::Local,
    },
    CmdDef {
        name: "usage",
        desc: "session tokens & cost",
        aliases: &["cost"],
        scope: CmdScope::Conversation,
        surface: CmdSurface::Text,
    },
    CmdDef {
        name: "jobs",
        desc: "background jobs (u stops)",
        aliases: &[],
        scope: CmdScope::Conversation,
        surface: CmdSurface::Text,
    },
    CmdDef {
        name: "copy",
        desc: "copy last assistant response",
        aliases: &[],
        scope: CmdScope::Conversation,
        surface: CmdSurface::Local,
    },
    CmdDef {
        name: "feedback",
        desc: "send feedback",
        aliases: &[],
        scope: CmdScope::Global,
        surface: CmdSurface::Text,
    },
    CmdDef {
        name: "theme",
        desc: "pick a color theme (~/.gray/themes/*.json; /theme new <name>)",
        aliases: &["themes"],
        scope: CmdScope::Global,
        surface: CmdSurface::Local,
    },
    CmdDef {
        name: "hotkeys",
        desc: "show keybindings (~/.gray/keybindings.json; /hotkeys reload)",
        aliases: &["keybindings", "bindings"],
        scope: CmdScope::Global,
        surface: CmdSurface::Local,
    },
    CmdDef {
        name: "reload",
        desc: "re-read theme, keybindings and prompt templates",
        aliases: &[],
        scope: CmdScope::Global,
        surface: CmdSurface::Text,
    },
    CmdDef {
        name: "agentsmd",
        desc: "edit system prompt",
        aliases: &["sys"],
        scope: CmdScope::Global,
        surface: CmdSurface::Text,
    },
    CmdDef {
        name: "skills",
        desc: "list skills (/skills [name] [args] to run one, /skills on|off for auto-context, /skills enable|disable <name>)",
        aliases: &["skill"],
        scope: CmdScope::Conversation,
        surface: CmdSurface::Text,
    },
    CmdDef {
        name: "gateway",
        desc: "connections: apps + daemon/cron/memory (space toggles)",
        aliases: &["gw"],
        scope: CmdScope::Global,
        surface: CmdSurface::Text,
    },
    CmdDef {
        name: "plugins",
        desc: "plugin manager (list, enable, update)",
        aliases: &[],
        scope: CmdScope::Global,
        surface: CmdSurface::Interactive,
    },
    CmdDef {
        name: "hehe",
        desc: "something funny",
        aliases: &[],
        scope: CmdScope::Conversation,
        surface: CmdSurface::Local,
    },
    CmdDef {
        name: "help",
        desc: "show commands",
        aliases: &[],
        scope: CmdScope::Global,
        surface: CmdSurface::Text,
    },
    CmdDef {
        name: "quit",
        desc: "exit",
        aliases: &["exit"],
        scope: CmdScope::Conversation,
        surface: CmdSurface::Local,
    },
];

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

const HIDDEN_ALIASES: &[(&str, &str)] = &[("plugin", "plugins")];

pub(crate) const TUI_COMMANDS: &[CmdDef] = &[
    CmdDef {
        name: "intro",
        desc: "logo intro enable|disable",
        aliases: &[],
        scope: CmdScope::Global,
        surface: CmdSurface::Local,
    },
];

pub(crate) fn resolve(name: &str) -> Option<&'static CmdDef> {
    let n = name.strip_prefix('/').unwrap_or(name).to_lowercase();
    if let Some(d) = REGISTRY.iter().chain(TUI_COMMANDS).find(|d| d.name == n) {
        return Some(d);
    }
    if let Some(&(_, target)) = HIDDEN_ALIASES.iter().find(|(a, _)| *a == n) {
        return REGISTRY.iter().find(|d| d.name == target);
    }
    REGISTRY
        .iter()
        .chain(TUI_COMMANDS)
        .find(|d| d.aliases.iter().any(|a| *a == n))
}

pub(crate) fn is_builtin_command(name: &str) -> bool {
    resolve(name).is_some()
}

pub(crate) fn provider_shortcuts() -> Vec<(String, String, String)> {
    let Ok(home) = crate::setup::gray_home() else {
        return Vec::new();
    };
    let mut out: Vec<(String, String, String)> = Vec::new();
    for p in crate::providers::ProviderRegistry::load_cached(&home).installed() {
        for cmd in crate::plugin_cli::declared_commands(&home, &p.plugin) {
            let short = cmd.trim_start_matches('/');
            if short.is_empty()
                || resolve(short).is_some()
                || out.iter().any(|(s, _, _)| s == short)
            {
                continue;
            }
            out.push((short.to_string(), p.provider_id(), p.provider.name.clone()));
        }
    }
    out
}

pub(crate) fn completion_matches(filter: &str) -> Vec<(&'static str, &'static str)> {
    let f = filter.to_lowercase();
    let mut matches: Vec<(u8, &'static str, &'static str)> = Vec::new();
    for d in REGISTRY.iter().chain(TUI_COMMANDS) {
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

            let after = inner[cmd.len()..].trim_start_matches(char::is_whitespace);

            let trailing = cur_text.ends_with(char::is_whitespace);
            let full_after = if trailing && !after.ends_with(char::is_whitespace) {
                format!("{after} ")
            } else {
                after.to_string()
            };
            let mut matches = complete_command_args(&cmd.to_lowercase(), &full_after, cwd);
            matches.extend(crate::plugin_cli::completions(inner));
            return matches;
        }
        let mut matches: Vec<_> = completion_matches(inner)
            .into_iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect();
        let f = inner.to_lowercase();

        let templates = crate::prompt_templates::discover(cwd);
        let mut tpl_rows: Vec<(u8, String, String)> = templates
            .iter()
            .filter_map(|t| {
                let n = t.name.to_lowercase();
                let rank = if n.starts_with(&f) {
                    0
                } else if n.contains(&f) || t.description.to_lowercase().contains(&f) {
                    1
                } else {
                    return None;
                };
                let desc = match &t.argument_hint {
                    Some(h) => format!("{} {h}", t.description),
                    None => t.description.clone(),
                };
                Some((rank, t.name.clone(), desc))
            })
            .collect();
        tpl_rows.sort_by_key(|(rank, _, _)| *rank);
        matches.extend(tpl_rows.into_iter().map(|(_, n, d)| (n, d)));
        let shortcuts = provider_shortcuts();
        matches.extend(
            shortcuts
                .iter()
                .filter(|(short, _, _)| short.starts_with(&f))
                .map(|(short, _, name)| (short.clone(), format!("log in to {name}"))),
        );

        matches.extend(
            crate::plugin_cli::completions(inner)
                .into_iter()
                .filter(|(n, _)| !shortcuts.iter().any(|(s, _, _)| s == n)),
        );
        return matches;
    }
    Vec::new()
}

pub(crate) fn completion_fill(name: &str) -> String {
    if name.contains(' ')
        || resolve(name).is_some()
        || crate::prompt_templates::is_cached_name(name)
        || provider_shortcuts().iter().any(|(s, _, _)| s == name)
        || crate::plugin_cli::completions(name)
            .iter()
            .any(|(n, _)| n == name)
    {
        return format!("/{name} ");
    }
    format!("/skills {name} ")
}

pub(crate) fn complete_command_args(
    cmd: &str,
    arg_text: &str,
    cwd: &std::path::Path,
) -> Vec<(String, String)> {
    let mut out = match cmd {
        "context" => complete_context_args(arg_text),
        "plugin" | "plugins" => complete_plugin_args(cmd, arg_text, cwd),
        "sessions" | "resume" => complete_resume_args(cmd, arg_text),
        "skill" | "skills" => complete_skill_args(cmd, arg_text, cwd),
        "agentsmd" | "sys" => complete_agentsmd_args(cmd, arg_text),
        "models" => complete_model_args(cmd, arg_text),
        "theme" | "themes" => complete_theme_args(cmd, arg_text),
        "intro" => complete_from_table(
            cmd,
            arg_text,
            &[
                ("enable", "animate the logo at startup"),
                ("disable", "settle instantly"),
                ("on", "alias of enable"),
                ("off", "alias of disable"),
            ],
        ),
        "hotkeys" | "keybindings" | "bindings" => {
            complete_from_table(cmd, arg_text, &[("reload", "re-read keybindings.json")])
        }
        _ => Vec::new(),
    };
    if arg_text.trim().is_empty()
        && let Some(d) = resolve(cmd)
    {
        out.insert(0, (cmd.to_string(), d.desc.to_string()));
    }
    out
}

fn complete_from_table(cmd: &str, arg_text: &str, table: &[(&str, &str)]) -> Vec<(String, String)> {
    let f = arg_text.to_lowercase();
    table
        .iter()
        .filter(|(s, _)| f.is_empty() || s.contains(f.as_str()))
        .map(|(s, d)| (format!("{cmd} {s}"), d.to_string()))
        .collect()
}

fn complete_resume_args(cmd: &str, arg_text: &str) -> Vec<(String, String)> {
    const FLAGS: &[(&str, &str)] = &[
        ("--last", "resume most recent session"),
        ("--all", "include other directories"),
        ("all", "every directory"),
        ("dismiss", "stop flagging interrupted sessions here"),
    ];
    complete_from_table(cmd, arg_text, FLAGS)
}

fn complete_skill_args(cmd: &str, arg_text: &str, cwd: &std::path::Path) -> Vec<(String, String)> {
    let f = arg_text.trim().to_lowercase();
    crate::skills::discover_skills(cwd)
        .skills
        .iter()
        .filter(|s| f.is_empty() || s.name.contains(f.as_str()))
        .map(|s| (format!("{cmd} {}", s.name), s.description.clone()))
        .collect()
}

fn complete_theme_args(cmd: &str, arg_text: &str) -> Vec<(String, String)> {
    let mut rows: Vec<(String, String)> =
        vec![(crate::theme::BUILTIN_THEME_NAME.to_string(), String::new())];
    if let Ok(dir) = crate::theme::themes_dir() {
        for name in crate::theme::list_themes_in(&dir) {
            if !crate::theme::is_builtin_name(&name) {
                rows.push((name, String::new()));
            }
        }
    }
    rows.push((
        "new".into(),
        "write the current palette to a new theme file".into(),
    ));
    rows.push(("reload".into(), "re-read the active theme file".into()));
    let f = arg_text.trim().to_lowercase();
    rows.into_iter()
        .filter(|(n, _)| n.to_lowercase().starts_with(&f))
        .map(|(n, d)| (format!("{cmd} {n}"), d))
        .collect()
}

fn complete_agentsmd_args(cmd: &str, arg_text: &str) -> Vec<(String, String)> {
    const SUBS: &[(&str, &str)] = &[("show", "print prompt file"), ("reset", "restore default")];
    complete_from_table(cmd, arg_text, SUBS)
}

fn complete_model_args(cmd: &str, arg_text: &str) -> Vec<(String, String)> {
    let f = arg_text.to_lowercase();
    crate::setup::cached_model_ids()
        .into_iter()
        .filter(|id| f.is_empty() || id.to_lowercase().contains(&f))
        .map(|id| (format!("{cmd} {id}"), "cached model".to_string()))
        .collect()
}

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
    complete_from_table(cmd, arg_text, SUBS)
}

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

    if parts.is_empty() {
        return L1
            .iter()
            .map(|(s, d)| (format!("context {s}"), d.to_string()))
            .collect();
    }
    let head = parts[0].to_lowercase();
    if head == "reserve" || head == "keep" {
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

    if parts.len() > 1 || trailing {
        return Vec::new();
    }
    let f = parts[0].to_lowercase();
    L1.iter()
        .filter(|(s, _)| s.to_lowercase().contains(&f))
        .map(|(s, d)| (format!("context {s}"), d.to_string()))
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplCommand {
    Quit,

    Sys(SysAction),

    ProviderLogin(String),

    ProviderCommand {
        cmd: String,
        argv: Vec<String>,
        provider_id: String,
    },

    New(Option<String>),

    Resume(ResumeArgs),

    Compact(Option<String>),

    Undo,

    Retry,

    Help,

    Model(Option<String>),

    ModelPanes,

    ModelFocus(String),

    ContextWindow(Option<String>),

    Update(Option<String>),

    Restart,
    Usage,

    Jobs,

    Gateway(Option<String>),


    Copy,

    Feedback(Option<String>),

    Hehe,

    Theme(Option<String>),

    Intro(Option<String>),

    Keys(Option<String>),

    Reload,

    Unknown(String),

    Plugin(String),

    Plugins,

    Skill(Option<String>),

    Prompt(String),

    Empty,
}

impl ReplCommand {
    pub fn runs_local(&self) -> bool {
        match self {
            ReplCommand::Prompt(_)
            | ReplCommand::Empty
            | ReplCommand::Compact(_)
            | ReplCommand::Retry
            | ReplCommand::Unknown(_)
            | ReplCommand::ProviderCommand { .. } => false,

            ReplCommand::Skill(payload) => payload.is_none(),
            _ => true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumeArgs {
    pub target: Option<String>,
    pub last: bool,
    pub all: bool,

    pub dismiss: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SysAction {
    Edit,

    Show,

    Reset,
}

pub(crate) fn parse_resume_args(rest: &str) -> ResumeArgs {
    let tokens: Vec<&str> = rest.split_whitespace().collect();
    let mut target: Option<String> = None;
    let mut last = false;
    let mut all = false;
    let mut dismiss = false;
    for tok in tokens {
        match tok {
            "dismiss" => dismiss = true,
            "all" | "--all" => all = true,
            "--last" => last = true,
            s if s.starts_with("--") => {}
            s => {
                if target.is_none() {
                    target = Some(s.to_string());
                }
            }
        }
    }
    ResumeArgs {
        target,
        last,
        all,
        dismiss,
    }
}

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
    let lower_cmd = cmd.to_lowercase();
    let canon: Option<&str> = if lower_cmd == "/skills" || lower_cmd == "/skill" {
        Some("skills")
    } else if let Some(d) = resolve(cmd) {
        Some(d.name)
    } else {
        None
    };
    match canon {
        Some("quit") => ReplCommand::Quit,
        Some("sessions") => ReplCommand::Resume(if rest.is_empty() {
            ResumeArgs {
                target: None,
                last: false,
                all: false,
                dismiss: false,
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
        Some("undo") => ReplCommand::Undo,
        Some("retry") => ReplCommand::Retry,
        Some("context") => ReplCommand::ContextWindow(opt(rest)),
        Some("update") => ReplCommand::Update(opt(rest)),
        Some("restart") => ReplCommand::Restart,
        Some("usage") => ReplCommand::Usage,
        Some("jobs") => ReplCommand::Jobs,
        Some("gateway") => ReplCommand::Gateway(opt(rest)),
        Some("copy") => ReplCommand::Copy,
        Some("feedback") => ReplCommand::Feedback(opt(rest)),
        Some("help") => ReplCommand::Help,
        Some("hehe") => ReplCommand::Hehe,
        Some("theme") => ReplCommand::Theme(opt(rest)),
        Some("intro") => ReplCommand::Intro(opt(rest)),
        Some("hotkeys") => ReplCommand::Keys(opt(rest)),
        Some("reload") => ReplCommand::Reload,
        Some("models") => match opt(rest) {
            Some(id) => ReplCommand::Model(Some(id)),
            None => ReplCommand::ModelPanes,
        },

        Some("plugins") => match opt(rest) {
            None => ReplCommand::Plugins,
            Some(_) => ReplCommand::Plugin(t.to_string()),
        },
        Some("skills") => {
            if rest.is_empty() {
                ReplCommand::Skill(None)
            } else {
                ReplCommand::Skill(Some(rest.to_string()))
            }
        }
        _ => {
            let name = t[1..].split_whitespace().next().unwrap_or("");
            if name.is_empty() || name.contains('/') {
                ReplCommand::Prompt(t.to_string())
            } else if let Some((_, id, _)) = provider_shortcuts()
                .into_iter()
                .find(|(short, _, _)| short.eq_ignore_ascii_case(name))
            {
                match super::split_plugin_command(t) {
                    Some((cmd, argv)) => ReplCommand::ProviderCommand {
                        cmd,
                        argv,
                        provider_id: id,
                    },
                    None => ReplCommand::Unknown(t.to_string()),
                }
            } else {
                ReplCommand::Unknown(t.to_string())
            }
        }
    }
}

#[path = "commands_tests.rs"]
#[cfg(test)]
mod tests;
