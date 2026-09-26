//! Gray binary entry point.

use clap::Parser;
use gray::Cli;
use gray::config::Config;
use gray::print::run_print_mode_with_session;
use gray::repl::run_repl_mode;
use gray_core::input::{InputEnvelope, InputError, MAX_INPUT_BYTES};
use std::io::Read;
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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    gray::logging::init();
    install_panic_hook();
    let _ = crossterm::terminal::disable_raw_mode();
    let cli = Cli::parse();
    if let Some(gray::Commands::Memory(args)) = &cli.command {
        return gray::memory::run_cli(args);
    }
    // Same reason as memory: a local file is nobody's provider concern. The
    // bash tool also claims `gray view <path>` before the shell runs, so in
    // an agent session the image is attached as a vision block and this only
    // prints when a human runs it.
    if let Some(gray::Commands::View {
        paths,
        frames,
        native,
    }) = &cli.command
    {
        return gray::view::run_cli(paths, *frames, *native);
    }
    // Same reason as view: a local search is nobody's provider concern, and
    // the bash tool claims `gray find`/`gray grep` so a model does not have to
    // know `gray` is on its PATH.
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
    // Account commands run before provider configuration: enrolling a fresh
    // machine must not require a model and a key first.
    match &cli.command {
        Some(gray::Commands::Login { code }) => {
            return gray::account::run_login(code.as_deref()).await;
        }
        Some(gray::Commands::Whoami) => {
            return gray::account::run_whoami().await;
        }
        Some(gray::Commands::Logout) => {
            return gray::account::run_logout().await;
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
    // Plugin terminal commands do not depend on provider configuration.
    match &cli.command {
        Some(gray::Commands::Install {
            cmd: gray::InstallCmd::Plugin { name, force },
        }) => {
            return gray::plugin_cli::install(&gray::plugin_cli::home()?, name, *force).await;
        }
        Some(gray::Commands::External(args)) => {
            let (name, rest) = args
                .split_first()
                .expect("clap external command is nonempty");
            return gray::plugin_cli::forward(&gray::plugin_cli::home()?, name, rest);
        }
        _ => {}
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
    gray::turn_caps::init_process_start();
    gray::setup::set_user_context_window(config.context_window);
    gray::setup::set_user_reserve_tokens(config.context_reserve);
    gray::setup::set_user_keep_recent_tokens(config.context_keep);
    if let Some(cmd) = cli.command {
        match cmd {
            gray::Commands::Memory(_) => unreachable!("handled before provider configuration"),
            gray::Commands::Resume {
                session_id,
                last,
                all,
            } => {
                return run_resume_subcommand(&mut config, session_id.as_deref(), last, all).await;
            }
            gray::Commands::Update => {
                return gray::update::update_now().await;
            }
            gray::Commands::Plugin { cmd } => {
                return run_plugin(cmd).await;
            }
            gray::Commands::Cron { cmd } => {
                return run_cron(cmd, &config).await;
            }
            gray::Commands::Gateway { cmd } => {
                return gray::gateway::run_cli(cmd, &config).await;
            }
            gray::Commands::Sessions { cmd } => {
                return run_sessions(cmd).await;
            }
            // Handled before Config::resolve so plugin commands work with no
            // provider configured (fresh machine, venv-only install).
            gray::Commands::Install { .. } | gray::Commands::External(_) => {
                unreachable!("plugin CLI dispatch happens before configuration")
            }
            // Same reason, one step earlier still: no provider needed to log in.
            gray::Commands::Login { .. } | gray::Commands::Whoami | gray::Commands::Logout => {
                unreachable!("account CLI dispatch happens before configuration")
            }
            gray::Commands::View { .. }
            | gray::Commands::Find { .. }
            | gray::Commands::Grep { .. } => {
                unreachable!("view/find/grep CLI dispatch happens before configuration")
            }
        }
    }
    if let Some(prompt) = cli.print.as_deref() {
        if cli.json {
            gray::print::run_print_mode_json(
                &config,
                prompt,
                cli.session.as_deref(),
                cli.continue_last,
                cli.max_requests,
                cli.input_price,
                cli.output_price,
            )
            .await?;
        } else {
            run_print_mode_with_session(&config, prompt, cli.session.as_deref(), cli.continue_last)
                .await?;
        }
    } else if let Some(input) = structured_input.as_ref() {
        gray::print::run_print_mode_json_input(
            &config,
            input,
            cli.session.as_deref(),
            cli.continue_last,
            cli.max_requests,
            cli.input_price,
            cli.output_price,
        )
        .await?;
    } else {
        gray::update::startup_check().await;
        run_repl_mode(&mut config, cli.continue_last, cli.session.as_deref()).await?;
    }
    Ok(())
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
        // Shared with `-p --session`: one validation, one error message.
        gray::resume::resolve_session_strict(&store, raw, all).await?
    } else if last {
        let cwd = std::env::current_dir().ok();
        // Recall-first for the cwd-scoped case (`--all` keeps the scan).
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
            // Scripts/pipes: the picker needs a real terminal — list instead.
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
        match gray::resume::run_resume_picker(all, None).await? {
            Some(id) => id,
            None => return Ok(()),
        }
    };
    // Non-TTY (`resume <id> < /dev/null`, scripts): the REPL would hit EOF
    // and exit silently, so announce what was resumed first — never exit 0
    // with no output. Interactive terminals skip this (the TUI owns the screen).
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
    // NOTE: an earlier `PROMPT` positional was deleted —
    // it was accepted and then discarded. To send a first message on resume,
    // pipe it in or type it after the REPL opens.
    run_repl_mode(config, false, Some(target_id.as_str())).await?;
    Ok(())
}

async fn run_plugin(cmd: gray::PluginCmd) -> anyhow::Result<()> {
    // Uniform with `Check`: user-facing errors go to stderr as
    // `error: …` with exit 1 (not anyhow's `Error: …` dump).
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
        PluginCmd::Capabilities { name } => {
            gray::plugin_cli::print_capabilities(name.as_deref())?;
            Ok(())
        }
        PluginCmd::List => {
            // Merged view: `lock.json` sidecars + `commands.json` native/CLI
            // commands (`install plugin` wrote there, `plugin list` never
            // looked). CLI rows carry a `[command]` tag since `update` and
            // `install <spec>` stay sidecar-only.
            let rows = gray::plugin_cli::list_rows()?;
            if rows.is_empty() {
                println!("no plugins installed");
            }
            for r in &rows {
                let state = if r.on { "" } else { " [disabled]" };
                let kind = if r.cli { " [command]" } else { "" };
                println!("{} {} ({}){state}{kind}", r.name, r.version, r.scope);
            }
            Ok(())
        }
        PluginCmd::Install { spec, force: _ } => {
            let r = gray_pkg::ops::install(spec, gray_pkg::ops::InstallOpts::default()).await?;
            println!("installed {} {} at {}", r.name, r.version, r.path.display());
            Ok(())
        }
        PluginCmd::Remove { name } => {
            // `install plugin` entries live in `commands.json`, not the
            // sidecar lock: route to whichever registry owns the name.
            gray::plugin_cli::remove_managed(&name)?;
            println!("removed {name}");
            Ok(())
        }
        PluginCmd::Update { target } => {
            // `update` only knows sidecar sources: `commands.json` entries
            // warn and no-op instead of failing as "not installed".
            let reports = gray::plugin_cli::update_managed(&target).await?;
            if reports.is_empty() {
                println!("up to date");
            }
            for r in reports {
                println!("updated {} {}", r.name, r.version);
            }
            Ok(())
        }
        PluginCmd::Enable { name } => {
            // Same routing as remove: the name may live in either registry.
            gray::plugin_cli::set_managed_enabled(&name, true)?;
            println!("enabled {name}");
            Ok(())
        }
        PluginCmd::Disable { name } => {
            gray::plugin_cli::set_managed_enabled(&name, false)?;
            println!("disabled {name}");
            Ok(())
        }
    }
}

/// `gray cron ...` (cron plan Task 5).
///
/// File-only surface: the store lives at `$GRAY_HOME/cron/jobs.json`.
/// Delivery targets ride the record opaquely; no backend interprets them yet.
fn cron_store() -> anyhow::Result<gray::cron::CronStore> {
    let home = gray::setup::gray_home()?;
    gray::cron::CronStore::open(home.join("cron"))
}

fn fmt_ts(ts: i64) -> String {
    chrono::DateTime::from_timestamp(ts, 0)
        .map(|d| d.format("%Y-%m-%dT%H:%M:%SZ").to_string())
        .unwrap_or_else(|| ts.to_string())
}

fn fmt_ts_opt(ts: Option<i64>) -> String {
    ts.map(fmt_ts).unwrap_or_else(|| "-".to_string())
}

fn fmt_dur(mut secs: u64) -> String {
    if secs.is_multiple_of(86400) {
        return format!("{}d", secs / 86400);
    }
    if secs.is_multiple_of(3600) {
        return format!("{}h", secs / 3600);
    }
    // Interval floor is 60s, so minutes are exact here.
    secs /= 60;
    format!("{secs}m")
}

fn fmt_schedule(s: &gray::cron::Schedule) -> String {
    match s {
        gray::cron::Schedule::Interval { secs } => format!("every {}", fmt_dur(*secs)),
        gray::cron::Schedule::Cron { expr } => expr.clone(),
        gray::cron::Schedule::Once { at } => format!("once {}", fmt_ts(*at)),
    }
}

fn fmt_deliver(d: &gray::cron::Deliver) -> String {
    match d {
        gray::cron::Deliver::Origin => "origin".to_string(),
        gray::cron::Deliver::Local => "local".to_string(),
        gray::cron::Deliver::Target(s) => s.clone(),
    }
}

fn fmt_status(s: Option<gray::cron::RunStatus>) -> &'static str {
    match s {
        None => "-",
        Some(gray::cron::RunStatus::Ok) => "ok",
        Some(gray::cron::RunStatus::Error) => "error",
        Some(gray::cron::RunStatus::DeliveryFailed) => "delivery_failed",
    }
}

/// `--deliver` flag: `origin`/`local` keywords (case-insensitive), anything
/// else rides `Deliver::Target`, stored opaquely until a delivery backend exists.
fn parse_deliver_flag(raw: Option<&str>) -> gray::cron::Deliver {
    match raw.map(str::trim).unwrap_or("local") {
        s if s.eq_ignore_ascii_case("origin") => gray::cron::Deliver::Origin,
        s if s.eq_ignore_ascii_case("local") || s.is_empty() => gray::cron::Deliver::Local,
        s => gray::cron::Deliver::Target(s.to_string()),
    }
}

/// The chat a job belongs to, declared by the host that runs the turn
/// (`GRAY_CRON_ORIGIN={"platform":"discord","chat":"...","route":"..."}`).
/// Unparseable or empty is no declaration, never a failure: a job added
/// outside a chat is just a local job.
fn origin_from_env() -> Option<gray::cron::store::Origin> {
    let raw = std::env::var("GRAY_CRON_ORIGIN").ok()?;
    let v: serde_json::Value = serde_json::from_str(raw.trim()).ok()?;
    let platform = v.get("platform")?.as_str()?.trim().to_string();
    let chat = v.get("chat")?.as_str()?.trim().to_string();
    if platform.is_empty() || chat.is_empty() {
        return None;
    }
    let field = |k: &str| {
        v.get(k)
            .and_then(|x| x.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    Some(gray::cron::store::Origin {
        platform,
        chat,
        thread: field("thread"),
        route: field("route"),
    })
}

/// Default job name: prompt's first line, truncated to the store's 50-char cap.
fn default_job_name(prompt: &str) -> String {
    let name: String = prompt
        .lines()
        .next()
        .unwrap_or("job")
        .trim()
        .chars()
        .take(40)
        .collect();
    if name.trim().is_empty() {
        "job".to_string()
    } else {
        name.trim().to_string()
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

async fn run_cron(cmd: gray::CronCmd, config: &gray::config::Config) -> anyhow::Result<()> {
    use gray::CronCmd;
    if cfg!(windows)
        && matches!(
            &cmd,
            CronCmd::Tick { .. } | CronCmd::Serve | CronCmd::Run { .. }
        )
    {
        anyhow::bail!(
            "cron execution is not supported on native Windows; use a WSL execution host"
        );
    }
    match cmd {
        CronCmd::List => {
            let store = cron_store()?;
            let jobs = store.list()?;
            if jobs.is_empty() {
                println!("no cron jobs");
                return Ok(());
            }
            for j in &jobs {
                println!(
                    "{} {} {} next={} last={}",
                    j.id,
                    j.name,
                    fmt_schedule(&j.schedule),
                    fmt_ts_opt(j.next_run_at),
                    fmt_status(j.last_status)
                );
            }
            // A schedule nothing ticks looks identical to a live one in the
            // rows above; this line is the only place that says otherwise.
            let now = gray::cron::now_secs();
            println!(
                "{}",
                gray::cron_status::ticker_line(&store.health(now)?, now)
            );
            Ok(())
        }
        CronCmd::Add {
            schedule,
            prompt,
            deliver,
            origin_session,
            name,
            workdir,
            skills,
            script,
        } => {
            let store = cron_store()?;
            let name = name.unwrap_or_else(|| default_job_name(&prompt));
            let base: std::path::PathBuf = workdir
                .clone()
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
            let skill_names: Vec<String> = skills
                .as_deref()
                .unwrap_or("")
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect();
            for s in &skill_names {
                if gray::skills_tool::resolve_skill_name(&base, s).is_none() {
                    anyhow::bail!("unknown skill {s:?}");
                }
            }
            // A host (a chat plugin) declares the chat a job belongs to in
            // the environment, so the model can add a job with a plain
            // `gray cron add <schedule> <prompt>` and still have it come
            // back to the conversation. Explicit flags always win.
            let env_origin = origin_from_env();
            let deliver_kind = match (&deliver, &env_origin) {
                (None, Some(_)) => gray::cron::Deliver::Origin,
                _ => parse_deliver_flag(deliver.as_deref()),
            };
            let origin = if matches!(deliver_kind, gray::cron::Deliver::Origin) {
                match (origin_session.as_deref(), &env_origin) {
                    (Some(chat), _) if !chat.trim().is_empty() => Some(gray::cron::store::Origin {
                        platform: env_origin
                            .as_ref()
                            .map(|o| o.platform.clone())
                            .unwrap_or_else(|| "local".to_string()),
                        chat: chat.trim().to_string(),
                        thread: None,
                        route: env_origin.as_ref().and_then(|o| o.route.clone()),
                    }),
                    (_, Some(o)) => Some(o.clone()),
                    _ => {
                        return Err(anyhow::anyhow!(
                            "--deliver origin requires --origin-session <session-id>"
                        ));
                    }
                }
            } else {
                None
            };
            let id = store.add_full(
                &name,
                &schedule,
                &prompt,
                deliver_kind,
                origin,
                workdir,
                skill_names,
                script,
            )?;
            let next = store
                .get(&id)?
                .map(|j| fmt_ts_opt(j.next_run_at))
                .unwrap_or_else(|| "-".to_string());
            println!("added {id} next {next}");
            if cfg!(windows) {
                println!(
                    "Stored only: cron execution is not supported on native Windows. Use a supported execution host."
                );
            }
            let stamp = store.last_tick()?;
            if let Some(warn) =
                gray::cron_status::add_warning(stamp.as_ref(), gray::cron::now_secs())
            {
                println!("{warn}");
            }
            Ok(())
        }
        CronCmd::Show { id } => {
            let Some(j) = cron_store()?.get(&id)? else {
                anyhow::bail!("unknown cron job {id:?}");
            };
            println!("id: {}", j.id);
            println!("name: {}", j.name);
            println!("schedule: {}", fmt_schedule(&j.schedule));
            println!("deliver: {}", fmt_deliver(&j.deliver));
            println!("enabled: {} state: {:?}", j.enabled, j.state);
            println!("created: {}", fmt_ts(j.created_at));
            println!("next run: {}", fmt_ts_opt(j.next_run_at));
            println!(
                "last run: {} status: {}",
                fmt_ts_opt(j.last_run_at),
                fmt_status(j.last_status)
            );
            if let Some(e) = &j.last_error {
                println!("last error: {e}");
            }
            if let Some(e) = &j.last_delivery_error {
                println!("last delivery error: {e}");
            }
            if let Some(o) = &j.origin {
                let thread = o.thread.as_deref().unwrap_or("-");
                let route = o.route.as_deref().unwrap_or("-");
                println!(
                    "origin: {}:{} thread:{thread} route:{route}",
                    o.platform, o.chat
                );
            }
            if let Some(w) = &j.workdir {
                println!("workdir: {}", w.display());
            }
            if !j.skills.is_empty() {
                println!("skills: {}", j.skills.join(", "));
            }
            if let Some(s) = &j.script {
                println!("script: {}", s.display());
            }
            println!("prompt: {}", j.prompt);
            Ok(())
        }
        CronCmd::Remove { id } => {
            if cron_store()?.remove(&id)? {
                println!("removed {id}");
                Ok(())
            } else {
                anyhow::bail!("unknown cron job {id:?}");
            }
        }
        CronCmd::Tick { json } => {
            let store = cron_store()?;
            let home = gray::setup::gray_home()?;
            let runner = gray::cron_serve::HeadlessRunner {
                config: config.clone(),
                follow_switches: false,
            };
            let deliver = gray::cron_serve::SaveLocalDeliver { home };
            let rep = gray::cron_serve::tick_once(&store, &runner, &deliver, "cli").await?;
            // `--json`: one line per chat-bound delivery, for a host that
            // routes them (a chat plugin). Core renders the frame; the
            // platform only carries the bytes.
            for saved in rep.delivered.iter().filter(|d| d.to_chat) {
                if json {
                    let origin = store.get(&saved.id).ok().flatten().and_then(|j| j.origin);
                    println!(
                        "{}",
                        gray::cron_serve::delivery_json(saved, origin.as_ref())
                    );
                } else {
                    println!("{}", gray::cron_serve::format_fire_chat(saved));
                }
            }
            if json {
                println!(
                    "{}",
                    serde_json::json!({"type": "cron_tick", "fired": rep.fired, "errors": rep.errors})
                );
            } else {
                println!("tick: fired={} errors={}", rep.fired, rep.errors);
            }
            Ok(())
        }
        CronCmd::Serve => {
            let store = cron_store()?;
            let home = gray::setup::gray_home()?;
            let runner = gray::cron_serve::HeadlessRunner {
                config: config.clone(),
                follow_switches: true,
            };
            gray::cron_serve::serve_loop(store, gray::cron_serve::SaveLocalDeliver { home }, runner)
                .await
        }
        CronCmd::Pause { id } => {
            if cron_store()?.set_paused(&id, true)? {
                println!("paused {id}");
                Ok(())
            } else {
                anyhow::bail!("unknown cron job {id:?}");
            }
        }
        CronCmd::Resume { id } => {
            let store = cron_store()?;
            if store.set_paused(&id, false)? {
                let next = store
                    .get(&id)?
                    .map(|j| fmt_ts_opt(j.next_run_at))
                    .unwrap_or_else(|| "-".to_string());
                println!("resumed {id} next {next}");
                Ok(())
            } else {
                anyhow::bail!("unknown cron job {id:?}");
            }
        }
        CronCmd::Run { id } => {
            let store = cron_store()?;
            let home = gray::setup::gray_home()?;
            let now = gray::cron::now_secs();
            let owner = gray::cron_serve::owner_stamp();
            let Some(job) = store.claim_one(now, &owner, &id)? else {
                anyhow::bail!("job {id:?} is not runnable (unknown, paused, or already claimed)");
            };
            let runner = gray::cron_serve::HeadlessRunner {
                config: config.clone(),
                follow_switches: false,
            };
            let deliver = gray::cron_serve::SaveLocalDeliver { home };
            let (status, saved) =
                gray::cron_serve::fire_one(&store, &runner, job, now, &deliver).await;
            if let Some(saved) = saved.filter(|d| d.to_chat) {
                println!("{}", gray::cron_serve::format_fire_chat(&saved));
            }
            println!("ran {id} status={status:?}");
            Ok(())
        }
    }
}

/// Log panics (payload + location) before the default hook prints to stderr,
/// which the TUI's screen-clearing would otherwise swallow.
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
        log::error!("panic at {location}: {payload}");
        default(info);
    }));
}
