use clap::{CommandFactory, Parser};
use gray::Cli;
use gray::config::Config;
use gray::print::run_print_mode_with_session;
use gray::repl::run_repl_mode;
use gray_core::input::{InputEnvelope, InputError, MAX_INPUT_BYTES};
use std::io::Read;
use std::io::Write as _;
use std::path::Path;

fn read_structured_input(path: &Path) -> Result<Vec<u8>, InputError> {
    let mut bytes = Vec::new();
    if path == Path::new("-") {
        std::io::stdin()
            .take((MAX_INPUT_BYTES as u64) + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| InputError::Io)?;
    } else {
        std::fs::File::open(path)
            .map_err(|_| InputError::Io)?
            .take((MAX_INPUT_BYTES as u64) + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| InputError::Io)?;
    }
    if bytes.len() > MAX_INPUT_BYTES {
        return Err(InputError::TooLarge);
    }
    Ok(bytes)
}

fn main() -> anyhow::Result<()> {
    #[cfg(unix)]
    if let Some(tool) = gray_tools::search_shim::invoked_as() {
        gray_tools::search_shim::run(&tool);
    }
    gray_main()
}

#[tokio::main]
async fn gray_main() -> anyhow::Result<()> {
    let result = run().await;

    if let Err(e) = &result
        && let Some(gray::session_store::SessionError::Locked { id, pid }) =
            e.downcast_ref::<gray::session_store::SessionError>()
    {
        eprintln!("{}", gray::resume::locked_session_card(id, *pid));
        std::process::exit(1);
    }
    result
}

async fn run() -> anyhow::Result<()> {
    gray::turn_caps::init_process_start();
    gray::session_store::capture_inherited_session_env();
    gray::logging::init();
    install_panic_hook();
    let _ = crossterm::terminal::disable_raw_mode();
    let cli = Cli::parse();

    if cli.skill {
        print!("{}", include_str!("../gray-skill.md"));
        return Ok(());
    }

    if cli.command.is_some() && (cli.print.is_some() || cli.input_json.is_some() || cli.json) {
        Cli::command()
            .error(
                clap::error::ErrorKind::UnknownArgument,
                "-p/--input-json/--json don't combine with a subcommand or plugin command",
            )
            .exit();
    }

    if let Some(gray::Commands::Spill { cmd }) = &cli.command {
        return gray::spill::run_cli(cmd);
    }
    #[cfg(target_os = "linux")]
    if let Some(gray::Commands::SearchServe {
        root,
        sock,
        index_dir,
    }) = &cli.command
    {
        return Ok(gray_tools::search_serve::serve(
            root,
            sock,
            index_dir.as_deref(),
        )?);
    }
    match &cli.command {
        Some(gray::Commands::Find {
            pattern,
            path,
            limit,
        }) => return gray::search::run_find(pattern, path.as_deref(), *limit).await,
        Some(gray::Commands::Grep {
            pattern,
            path,
            limit,
            glob,
            ignore_case,
            literal,
            context,
        }) => {
            return gray::search::run_grep(
                pattern,
                path.as_deref(),
                *limit,
                glob.as_deref(),
                *ignore_case,
                *literal,
                *context,
            )
            .await;
        }
        _ => {}
    }
    if cli.dump_manifest {
        match gray::build_registry().await {
            Ok((_registry, manifests, fallback)) => {
                if fallback {
                    eprintln!(
                        "note: gray.yml profile missing/unresolvable — showing builtin manifests"
                    );
                }
                for w in gray::take_profile_warnings() {
                    eprintln!("warning: {w}");
                }
                println!("{}", serde_json::to_string_pretty(&manifests)?);
                return Ok(());
            }
            Err(e) => {
                eprintln!("error: gray.yml: {e:#}");
                std::process::exit(1);
            }
        }
    }

    if let Some(gray::Commands::Send {
        session,
        text,
        attach,
        from,
        wait,
    }) = &cli.command
    {
        return gray::session_send::run_cli(session, text, attach, from.as_deref(), *wait).await;
    }

    if let Some(gray::Commands::Read {
        session,
        live,
        last,
        json,
    }) = &cli.command
    {
        return gray::session_read::run_cli(session.as_deref(), *live, *last, *json).await;
    }

    if let Some(gray::Commands::Gateway { args }) = &cli.command {
        return gray::plugin_cli::forward(&gray::plugin_cli::home()?, "gateway", args).await;
    }

    if let Some(gray::Commands::Install {
        cmd: gray::InstallCmd::Plugin { name },
    }) = &cli.command
    {
        return gray::plugin_cli::install(&gray::plugin_cli::home()?, name).await;
    }
    if let Some(gray::Commands::External(args)) = &cli.command {
        let (name, rest) = args
            .split_first()
            .expect("clap external command is nonempty");
        return gray::plugin_cli::forward(&gray::plugin_cli::home()?, name, rest).await;
    }
    let structured_input = if let Some(path) = cli.input_json.as_deref() {
        let bytes = match read_structured_input(path) {
            Ok(bytes) => bytes,
            Err(error) => {
                gray::print::write_structured_input_error(&error);
                std::process::exit(1);
            }
        };
        match InputEnvelope::from_json(&bytes) {
            Ok(input) => Some(input),
            Err(error) => {
                gray::print::write_structured_input_error(&error);
                std::process::exit(1);
            }
        }
    } else {
        None
    };

    let mut config = Config::resolve(&cli)?;
    log::debug!(target: "gray_timing", "spawn config_loaded elapsed_ms={}", gray::turn_caps::process_start().elapsed().as_millis());
    if !config.bare {
        let saved_theme = gray::setup::saved_config_path()
            .ok()
            .and_then(|p| gray::setup::load_saved_config_at(&p).theme);
        gray::theme::init_from_saved(saved_theme.as_deref());
        gray::statusline::init_from_saved_config();
        if let Ok(home) = gray::setup::gray_home() {
            for w in gray::keymap::load_into_active(&home) {
                eprintln!("gray: keybindings.json: {w}");
            }
        }
        gray::win_paste::ensure_ctrl_v_passthrough();
    }
    gray::setup::set_user_context_window(config.context_window);
    gray::setup::set_user_reserve_tokens(config.context_reserve);
    gray::setup::set_user_keep_recent_tokens(config.context_keep);
    if let Some(cmd) = cli.command {
        match cmd {
            gray::Commands::Resume {
                session_id,
                last,
                all,
            } => {
                return run_resume_subcommand(&mut config, session_id.as_deref(), last, all).await;
            }
            gray::Commands::Update { auto, everything } => {
                if let Some(v) = auto.as_deref() {
                    return gray::auto_update::auto_cli(v);
                }
                if everything {
                    return gray::quality_of_life::update::cli().await;
                }
                return gray::update::update_now().await;
            }
            gray::Commands::Plugin { cmd } => {
                return run_plugin(cmd).await;
            }
            gray::Commands::Send { .. } => {
                unreachable!("send dispatch happens before configuration")
            }
            gray::Commands::Read { .. } => {
                unreachable!("read dispatch happens before configuration")
            }
            gray::Commands::Trust { dir, revoke } => {
                return gray::project_trust::run_cli(dir, revoke);
            }
            gray::Commands::Sessions { cmd } => {
                return run_sessions(cmd).await;
            }
            gray::Commands::Cmd { cmd } => {
                return gray::cmd::run_cli(cmd, &mut config).await;
            }

            gray::Commands::External(_) | gray::Commands::Install { .. } => {
                unreachable!("plugin CLI dispatch happens before configuration")
            }
            gray::Commands::Gateway { .. } => {
                unreachable!("gateway CLI dispatch happens before configuration")
            }
            gray::Commands::Find { .. }
            | gray::Commands::Grep { .. }
            | gray::Commands::Spill { .. } => {
                unreachable!("find/grep/spill CLI dispatch happens before configuration")
            }
            #[cfg(target_os = "linux")]
            gray::Commands::SearchServe { .. } => {
                unreachable!("search-serve CLI dispatch happens before configuration")
            }
        }
    }

    if let Some(None) = &cli.resume {
        return run_resume_subcommand(&mut config, None, false, false).await;
    }
    let session_arg = cli
        .resume
        .as_ref()
        .and_then(|r| r.as_deref())
        .or(cli.session.as_deref());

    if let Some(prompt) = cli.print.as_deref() {
        let piped;
        let prompt = if prompt == "-" {
            piped = std::io::read_to_string(std::io::stdin())?;
            piped.as_str()
        } else {
            prompt
        };
        let result = if cli.json {
            gray::print::run_print_mode_json(
                &config,
                prompt,
                session_arg,
                cli.continue_last,
                cli.max_requests,
                cli.input_price,
                cli.output_price,
            )
            .await
        } else {
            run_print_mode_with_session(&config, prompt, session_arg, cli.continue_last).await
        };
        exit_after_print(result);
    } else if let Some(input) = structured_input.as_ref() {
        let result = gray::print::run_print_mode_json_input(
            &config,
            input,
            session_arg,
            cli.continue_last,
            cli.max_requests,
            cli.input_price,
            cli.output_price,
        )
        .await;
        exit_after_print(result);
    } else {
        if !config.bare {
            gray::update::startup_check().await;
        }
        run_repl_mode(&mut config, cli.continue_last, session_arg).await?;

        let _ = std::io::stdout().flush();
        let _ = std::io::stderr().flush();
        log::logger().flush();
        std::process::exit(0);
    }
}

fn exit_after_print(result: anyhow::Result<()>) -> ! {
    let code = match result {
        Ok(()) => 0,
        Err(error) => {
            if let Some(gray::session_store::SessionError::Locked { id, pid }) =
                error.downcast_ref::<gray::session_store::SessionError>()
            {
                eprintln!("{}", gray::resume::locked_session_card(id, *pid));
            } else {
                eprintln!("Error: {error:?}");
            }
            1
        }
    };
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    log::logger().flush();
    std::process::exit(code);
}

async fn run_resume_subcommand(
    config: &mut Config,
    session_id: Option<&str>,
    last: bool,
    all: bool,
) -> anyhow::Result<()> {
    use gray::session_store::{JsonlSessionStore, default_root};
    let Some(root) = default_root() else {
        anyhow::bail!("cannot resolve home");
    };
    let store = JsonlSessionStore::new(root);
    let target_id = if let Some(raw) = session_id {
        gray::resume::resolve_session_strict(&store, raw, all).await?
    } else if last {
        let cwd = std::env::current_dir().ok();

        if !all
            && let Some(c) = cwd.as_deref()
            && let Some(rid) = store.recall_validated(c).await
            && store.load(&rid).await.is_ok()
        {
            rid
        } else {
            let summaries = store.list().await;
            let cwd_filter = if all { None } else { cwd.as_deref() };
            match gray::resume::latest_summary(&summaries, cwd_filter) {
                Some(s) => s.id.clone(),
                None => {
                    if all {
                        anyhow::bail!("no saved sessions")
                    } else {
                        anyhow::bail!("no saved sessions in this directory (try --all)")
                    }
                }
            }
        }
    } else {
        use std::io::IsTerminal as _;
        if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
            let summaries = gray::resume::recent_summaries(&store, all).await;
            if summaries.is_empty() {
                if all {
                    anyhow::bail!("no saved sessions")
                } else {
                    anyhow::bail!("no saved sessions in this directory (try --all)")
                }
            }
            for s in &summaries {
                println!("{}", gray::resume::format_summary_row(s));
            }
            return Ok(());
        }
        match gray::resume::run_resume_picker(all, None, None).await? {
            Some(id) => id,
            None => return Ok(()),
        }
    };

    let _open_guard = store.acquire_open(&target_id).await?;

    {
        use std::io::IsTerminal;
        if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
            println!(
                "{}",
                gray::resume::resumed_session_line(&store, &target_id).await?
            );
        }
    }
    let _ = crossterm::terminal::disable_raw_mode();

    run_repl_mode(config, false, Some(target_id.as_str())).await?;

    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    log::logger().flush();
    std::process::exit(0);
}

async fn run_plugin(cmd: gray::PluginCmd) -> anyhow::Result<()> {
    let res = run_plugin_inner(cmd).await;
    if let Err(e) = res {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
    Ok(())
}

async fn run_plugin_inner(cmd: gray::PluginCmd) -> anyhow::Result<()> {
    use gray::PluginCmd;
    match cmd {
        PluginCmd::Check { dir } => gray::plugin_check::check_plugin_dir(&dir).await,
        PluginCmd::Capabilities { name, all } => {
            if all {
                let name = name.as_deref().ok_or_else(|| {
                    anyhow::anyhow!(
                        "--all needs a plugin name: gray plugin capabilities <name> --all"
                    )
                })?;
                gray::plugin_cli::grant_all_capabilities(name)?;
            } else {
                gray::plugin_cli::print_capabilities(name.as_deref())?;
            }
            Ok(())
        }
        PluginCmd::List => {
            let rows = gray::plugin_cli::list_rows()?;
            if rows.is_empty() {
                println!("no plugins installed");
            }
            for r in &rows {
                let state = if r.on { "" } else { " [disabled]" };
                let kind = if r.cli { " [command]" } else { "" };
                let global = if r.global { " [global]" } else { "" };
                println!(
                    "{} {} ({}){state}{kind}{global}",
                    r.name, r.version, r.scope
                );
            }
            Ok(())
        }
        PluginCmd::Install { specs, force, yes } => {
            gray::plugin_cli::install_specs(&gray::plugin_cli::home()?, &specs, force, yes).await
        }
        PluginCmd::Remove { name } => {
            gray::plugin_cli::remove_managed(&name)?;
            println!("removed {name}");
            Ok(())
        }
        PluginCmd::Update { target, check } => {
            let reports = if check {
                gray::plugin_cli::check_managed(&target).await?
            } else {
                gray::plugin_cli::update_managed(&target).await?
            };
            let mut printed = false;
            for r in &reports {
                if matches!(r.action, gray_pkg::ops::UpdateAction::Current) {
                    continue;
                }
                printed = true;
                println!("{}: {}", r.name, r.detail());
            }
            if !printed {
                println!("up to date");
            }
            Ok(())
        }
        PluginCmd::Enable { name, global } => {
            gray::plugin_cli::set_managed_enabled(&name, true)?;
            if global {
                gray::plugin_cli::set_managed_global(&name, true)?;
                println!("enabled {name} [global]");
            } else {
                println!("enabled {name}");
            }
            Ok(())
        }
        PluginCmd::Disable { name } => {
            gray::plugin_cli::set_managed_enabled(&name, false)?;
            println!("disabled {name}");
            Ok(())
        }
    }
}

async fn run_sessions(cmd: gray::SessionsCmd) -> anyhow::Result<()> {
    match cmd {
        gray::SessionsCmd::Prune { older_than_days } => {
            let root = gray::session_store::default_root()
                .ok_or_else(|| anyhow::anyhow!("cannot resolve home"))?;
            let store = gray::session_store::JsonlSessionStore::new(root);
            let cutoff_ms = chrono::Utc::now()
                .timestamp_millis()
                .saturating_sub((older_than_days as i64).saturating_mul(86_400_000))
                .max(0) as u64;
            let removed = store.prune_before(cutoff_ms).await?;
            println!(
                "pruned {} session(s) older than {older_than_days}d",
                removed.len()
            );
            Ok(())
        }
    }
}

fn install_panic_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let location = info
            .location()
            .map(|l| l.to_string())
            .unwrap_or_else(|| "unknown".into());
        let payload = if let Some(s) = info.payload().downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "non-string panic payload".into()
        };
        if payload.starts_with("failed printing to stdout") && payload.contains("Broken pipe") {
            std::process::exit(141);
        }
        log::error!("panic at {location}: {payload}");
        default(info);
    }));
}
