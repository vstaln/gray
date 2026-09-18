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
