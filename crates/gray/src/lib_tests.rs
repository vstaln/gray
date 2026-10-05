use super::*;
// UNRUN (cargo test banned under X): run in TTY/CI.

#[test]
fn plugin_cli_alias_plugins_resolves() {
    // `gray plugins ...` is the visible alias of `gray plugin ...`: both
    // spellings parse to the identical subcommand.
    for (a, b) in [
        (["gray", "plugin", "list"], ["gray", "plugins", "list"]),
        (["gray", "plugin", "update"], ["gray", "plugins", "update"]),
    ] {
        let cli = Cli::try_parse_from(a).unwrap();
        let aliased = Cli::try_parse_from(b).unwrap();
        assert!(
            matches!(cli.command, Some(Commands::Plugin { .. })),
            "{a:?}"
        );
        assert_eq!(
            format!("{:?}", cli.command),
            format!("{:?}", aliased.command),
            "{a:?} vs {b:?}"
        );
    }
}

#[test]
fn cron_cli_parses_add_shapes() {
    // `add` takes schedule + prompt positionally (`--` separates a
    // dash-leading prompt); flags are optional.
    let cli = Cli::try_parse_from([
        "gray",
        "cron",
        "add",
        "every 1h",
        "--",
        "check CI and report",
    ])
    .unwrap();
    assert!(matches!(
        cli.command,
        Some(Commands::Cron {
            cmd: CronCmd::Add { .. },
        })
    ));

    let cli = Cli::try_parse_from([
        "gray",
        "cron",
        "add",
        "0 9 * * *",
        "--deliver",
        "telegram:123",
        "--name",
        "morn",
        "--in",
        "/tmp",
        "ping",
    ])
    .unwrap();
    match cli.command {
        Some(Commands::Cron {
            cmd:
                CronCmd::Add {
                    schedule,
                    prompt,
                    deliver,
                    name,
                    workdir,
                    ..
                },
        }) => {
            assert_eq!(schedule, "0 9 * * *");
            assert_eq!(prompt, "ping");
            assert_eq!(deliver.as_deref(), Some("telegram:123"));
            assert_eq!(name.as_deref(), Some("morn"));
            assert_eq!(workdir, Some(PathBuf::from("/tmp")));
        }
        other => panic!("unexpected {other:?}"),
    }

    let cli = Cli::try_parse_from(["gray", "cron", "list"]).unwrap();
    assert!(matches!(
        cli.command,
        Some(Commands::Cron { cmd: CronCmd::List })
    ));
    let cli = Cli::try_parse_from(["gray", "cron", "show", "abc123"]).unwrap();
    assert!(matches!(
        cli.command,
        Some(Commands::Cron {
            cmd: CronCmd::Show { .. },
        })
    ));
    let cli = Cli::try_parse_from(["gray", "cron", "remove", "abc123"]).unwrap();
    assert!(matches!(
        cli.command,
        Some(Commands::Cron {
            cmd: CronCmd::Remove { .. },
        })
    ));
}

#[test]
fn cron_cli_parses_lifecycle() {
    for args in [
        vec!["gray", "cron", "tick"],
        vec!["gray", "cron", "serve"],
        vec!["gray", "cron", "pause", "abc"],
        vec!["gray", "cron", "resume", "abc"],
        vec!["gray", "cron", "run", "abc"],
    ] {
        let cli = Cli::try_parse_from(args.clone()).unwrap();
        assert!(
            matches!(cli.command, Some(Commands::Cron { .. })),
            "{args:?}"
        );
    }
    let cli = Cli::try_parse_from([
        "gray",
        "cron",
        "add",
        "every 1h",
        "--skills",
        "a,b",
        "--script",
        "/tmp/pre.sh",
        "do it",
    ])
    .unwrap();
    match cli.command {
        Some(Commands::Cron {
            cmd: CronCmd::Add { skills, script, .. },
        }) => {
            assert_eq!(skills.as_deref(), Some("a,b"));
            assert_eq!(script, Some(PathBuf::from("/tmp/pre.sh")));
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn cache_key_prefers_session_id() {
    assert_eq!(
        provider_cache_key(Some("cc5d154d-4c24-42ee-b8a8-6a5735bdcfc9")),
        "cc5d154d-4c24-42ee-b8a8-6a5735bdcfc9"
    );
}

#[test]
fn reload_path_keeps_session_cache_shard() {
    // Steady-state builds (prompt_turn) and the reload path
    // must resolve the identical key for one session id; the pre-fix
    // reload passed None, rotating to the fallback shard (~0% hits).
    let sid = "cc5d154d-4c24-42ee-b8a8-6a5735bdcfc9";
    assert_eq!(provider_cache_key(Some(sid)), provider_cache_key(Some(sid)));
    assert_ne!(provider_cache_key(Some(sid)), provider_cache_key(None));
}

#[test]
fn cache_key_fallback_is_stable_per_process() {
    // Rebuilds mid-session (reload, lazy builds) must not rotate the key.
    assert_eq!(provider_cache_key(None), provider_cache_key(None));
    assert_eq!(provider_cache_key(Some("")), provider_cache_key(None));
}

#[test]
fn cache_key_clamped_to_64_chars() {
    let long = "s".repeat(100);
    assert_eq!(provider_cache_key(Some(&long)).len(), 64);
}

#[test]
fn cron_cli_parses_a_reminder() {
    let cli = Cli::try_parse_from(["gray", "cron", "add", "in 2m", "--reminder", "clean my roo"])
        .unwrap();
    match cli.command {
        Some(Commands::Cron {
            cmd:
                CronCmd::Add {
                    schedule,
                    prompt,
                    reminder,
                    name,
                    ..
                },
        }) => {
            assert_eq!(schedule, "in 2m");
            assert_eq!(prompt, "clean my roo");
            assert!(reminder);
            assert!(name.is_none(), "a reminder needs no name");
        }
        other => panic!("unexpected {other:?}"),
    }
}

/// `gray --skill` is how another agent learns to drive gray, so the file has
/// to stay true: a flag it names that no longer exists is a command the next
/// agent runs and fails on.
const SKILL_MD: &str = include_str!("../gray-skill.md");

/// Every flag-looking token in the skill, `-p` and `--json` alike.
fn flags_named_in(md: &str) -> Vec<String> {
    md.split_whitespace()
        .map(|t| t.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-'))
        .filter(|t| t.len() > 1 && t.starts_with('-') && t.trim_matches('-').len() > 0)
        .map(str::to_string)
        .collect()
}

/// The whole command tree's help, so a flag on any subcommand counts.
fn all_help() -> String {
    fn walk(mut cmd: clap::Command, out: &mut String) {
        out.push_str(&cmd.render_long_help().to_string());
        for sub in cmd.get_subcommands() {
            walk(sub.clone(), out);
        }
    }
    let mut out = String::new();
    walk(<Cli as clap::CommandFactory>::command(), &mut out);
    out
}

#[test]
fn the_skill_is_a_loadable_skill_that_says_when_not_to_load_it() {
    let (fm, body) = crate::skills::parse_frontmatter(SKILL_MD).expect("skill frontmatter parses");
    let desc = fm
        .description
        .expect("a skill needs a description to trigger on");
    assert!(
        desc.to_lowercase().contains("do not use"),
        "without an anti-trigger the skill loads on every coding task"
    );
    assert!(
        body.contains("## Exit codes"),
        "body must carry the contract"
    );
}

#[test]
fn every_flag_the_skill_names_still_exists() {
    let help = all_help();
    for flag in flags_named_in(SKILL_MD) {
        assert!(
            help.contains(&flag),
            "gray-skill.md names `{flag}`, which the CLI does not have"
        );
    }
}

/// The examples are the part an agent copies, so each one must parse.
#[test]
fn every_command_the_skill_shows_actually_parses() {
    let examples: &[&[&str]] = &[
        &["gray", "-p", "fix the failing test"],
        &["gray", "-p", "...", "--json"],
        &["gray", "-p", "...", "--json", "--max-turns", "20"],
        &["gray", "-p", "...", "--json", "--max-requests", "40"],
        &["gray", "-p", "...", "--max-cost-usd", "2.00"],
        &["gray", "-p", "...", "--max-wall-secs", "900"],
        &["gray", "-c"],
        &["gray", "--session", "abc123"],
        &["gray", "resume"],
        &["gray", "sessions", "prune", "--older-than-days", "90"],
        &[
            "gray",
            "cron",
            "add",
            "0 9 * * 1-5",
            "summarize",
            "--deliver",
            "local",
        ],
        &["gray", "cron", "list"],
        &["gray", "cron", "show", "id1"],
        &["gray", "cron", "remove", "id1"],
        &["gray", "plugin", "list"],
        &["gray", "gateway", "status"],
        &["gray", "update"],
        &["gray", "--skill"],
        &["gray", "--json", "--input-json", "task.json"],
    ];
    for argv in examples {
        Cli::try_parse_from(*argv)
            .unwrap_or_else(|e| panic!("gray-skill.md shows `{}`: {e}", argv.join(" ")));
    }
}

fn warm_config(base_url: &str, effort: Option<&str>, plugin: bool) -> Config {
    Config {
        temperature: None,
        top_p: None,
        model: Some("openai/gpt-5".into()),
        base_url: base_url.into(),
        api_key: Some("sk-test".into()),
        provider_id: if plugin { "p".into() } else { String::new() },
        credential_source: if plugin {
            "plugin".into()
        } else {
            String::new()
        },
        auth_ref: if plugin { "auth".into() } else { String::new() },
        thinking_effort: effort.map(str::to_string),
        show_reasoning: None,
        context_window: None,
        context_reserve: None,
        context_keep: None,
        exec_prefix: None,
        max_turns: None,
        max_cost_micros: None,
        max_wall_secs: None,
        bare: false,
    }
}

#[test]
fn cache_warm_covers_openai_compatible_hosts() {
    // GRAY_NO_CACHE_WARM must be unset for the Some-legs; the None-legs hold either way.
    if std::env::var_os("GRAY_NO_CACHE_WARM").is_some() {
        return;
    }
    for host in [
        "https://api.openai.com/v1",
        "https://api.commandcode.ai/provider/v1",
        "https://openrouter.ai/api/v1",
        "https://api.anthropic.com/v1",
        "http://localhost:11434/v1",
    ] {
        assert!(
            cache_warm_policy(
                &warm_config(host, Some("off"), false),
                "openai/gpt-5",
                Some("off")
            )
            .is_some(),
            "warms on {host}"
        );
        assert!(
            cache_warm_policy(&warm_config(host, None, false), "openai/gpt-5", None).is_some(),
            "warms on {host} without effort"
        );
    }
}

#[test]
fn cache_warm_stays_off_for_plugin_creds_and_claude_thinking() {
    let url = "https://api.openai.com/v1";
    assert!(
        cache_warm_policy(
            &warm_config(url, Some("off"), true),
            "openai/gpt-5",
            Some("off")
        )
        .is_none()
    );
    // A Claude thinking budget is sized from the output cap: natively or
    // through a router, the replay would key a different cache entry.
    assert!(
        cache_warm_policy(
            &warm_config("https://api.anthropic.com/v1", Some("high"), false),
            "claude-sonnet-4-5",
            Some("high")
        )
        .is_none()
    );
    assert!(
        cache_warm_policy(
            &warm_config("https://openrouter.ai/api/v1", Some("max"), false),
            "anthropic/claude-opus-4.5",
            Some("max")
        )
        .is_none()
    );
}

#[test]
fn cache_warm_runs_with_reasoning_effort_off_claude() {
    if std::env::var_os("GRAY_NO_CACHE_WARM").is_some() {
        return;
    }
    // The reported case: Muse Spark at xhigh on an OpenAI-compatible host.
    for (url, model, effort) in [
        (
            "https://api.commandcode.ai/provider/v1",
            "meta/muse-spark-1.3-contributor",
            "xhigh",
        ),
        ("https://api.openai.com/v1", "openai/gpt-5", "high"),
        (
            "https://openrouter.ai/api/v1",
            "deepseek/deepseek-v3.2",
            "max",
        ),
    ] {
        assert!(
            cache_warm_policy(&warm_config(url, Some(effort), false), model, Some(effort))
                .is_some(),
            "warms {model} at {effort}"
        );
    }
}

#[test]
fn resume_flag_parses_bare_id_and_conflicts() {
    let cli = Cli::try_parse_from(["gray", "-r"]).unwrap();
    assert_eq!(cli.resume, Some(None));

    let cli = Cli::try_parse_from(["gray", "-r", "chiral-xenon-pulsar"]).unwrap();
    assert_eq!(cli.resume, Some(Some("chiral-xenon-pulsar".to_string())));

    let cli = Cli::try_parse_from(["gray", "--resume"]).unwrap();
    assert_eq!(cli.resume, Some(None));

    assert!(Cli::try_parse_from(["gray", "-r", "x", "--session", "y"]).is_err());
    assert!(Cli::try_parse_from(["gray", "-r", "x", "-c"]).is_err());
}
