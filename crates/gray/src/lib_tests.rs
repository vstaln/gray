use super::*;

#[test]
fn plugin_cli_alias_plugins_resolves() {
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
fn cache_key_prefers_session_id() {
    assert_eq!(
        provider_cache_key(Some("cc5d154d-4c24-42ee-b8a8-6a5735bdcfc9")),
        "cc5d154d-4c24-42ee-b8a8-6a5735bdcfc9"
    );
}

#[test]
fn cache_key_fallback_is_stable_per_process() {
    assert_eq!(provider_cache_key(None), provider_cache_key(None));
    assert_eq!(provider_cache_key(Some("")), provider_cache_key(None));
}

#[test]
fn cache_key_clamped_to_64_chars() {
    let long = "s".repeat(100);
    assert_eq!(provider_cache_key(Some(&long)).len(), 64);
}

const SKILL_MD: &str = include_str!("../gray-skill.md");

fn flags_named_in(md: &str) -> Vec<String> {
    md.split_whitespace()
        .map(|t| t.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-'))
        .filter(|t| t.len() > 1 && t.starts_with('-') && !t.trim_matches('-').is_empty())
        .map(str::to_string)
        .collect()
}

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
        fast_mode: None,
        model_parts: Default::default(),
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
        lean: false,
        background_compaction: false,
        compact_at: None,
    }
}

#[test]
fn cache_warm_covers_openai_compatible_hosts() {
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
                false,
                5 * 60
            )
            .is_some(),
            "warms on {host}"
        );
        assert!(
            cache_warm_policy(
                &warm_config(host, None, false),
                "openai/gpt-5",
                false,
                5 * 60
            )
            .is_some(),
            "warms on {host} without effort"
        );
    }
}

#[test]
fn cache_warm_stays_off_for_plugin_creds_but_covers_claude_thinking() {
    let url = "https://api.openai.com/v1";

    assert!(
        cache_warm_policy(
            &warm_config(url, Some("off"), true),
            "openai/gpt-5",
            false,
            5 * 60
        )
        .is_none()
    );
    if std::env::var_os("GRAY_NO_CACHE_WARM").is_some() {
        return;
    }

    assert!(
        cache_warm_policy(
            &warm_config(url, Some("off"), true),
            "openai/gpt-5",
            true,
            5 * 60
        )
        .is_some()
    );

    assert!(
        cache_warm_policy(
            &warm_config("https://api.anthropic.com/v1", Some("high"), false),
            "claude-sonnet-4-5",
            false,
            5 * 60
        )
        .is_some()
    );
    assert!(
        cache_warm_policy(
            &warm_config("https://openrouter.ai/api/v1", Some("max"), false),
            "anthropic/claude-opus-4.5",
            false,
            5 * 60
        )
        .is_some()
    );
}

#[test]
fn cache_warm_runs_with_reasoning_effort_off_claude() {
    if std::env::var_os("GRAY_NO_CACHE_WARM").is_some() {
        return;
    }

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
            cache_warm_policy(&warm_config(url, Some(effort), false), model, false, 5 * 60)
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

#[test]
fn managed_prompt_regenerates_only_while_pristine() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("AGENTS.md");

    let body = crate::load_or_create_system_prompt_at(&path).unwrap();
    assert_eq!(body, crate::DEFAULT_SYS_PROMPT);
    let on_disk = std::fs::read_to_string(&path).unwrap();
    assert!(
        on_disk.starts_with("<!-- gray:managed sha256:"),
        "{on_disk}"
    );

    let stale = crate::stamped_system_prompt("old default text");
    std::fs::write(&path, &stale).unwrap();
    let body = crate::load_or_create_system_prompt_at(&path).unwrap();
    assert_eq!(
        body,
        crate::DEFAULT_SYS_PROMPT,
        "pristine stale default refreshes"
    );
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .contains(&crate::DEFAULT_SYS_PROMPT[..40])
    );

    std::fs::write(&path, "my custom rules").unwrap();
    let body = crate::load_or_create_system_prompt_at(&path).unwrap();
    assert_eq!(body, "my custom rules", "edited file is never touched");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "my custom rules");

    std::fs::write(&path, crate::LEGACY_SYS_PROMPT).unwrap();
    let body = crate::load_or_create_system_prompt_at(&path).unwrap();
    assert_eq!(body, crate::LEGACY_SYS_PROMPT);
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .starts_with("<!-- gray:managed sha256:"),
        "legacy pristine default gets stamped in place"
    );
}
