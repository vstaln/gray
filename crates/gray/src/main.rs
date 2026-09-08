//! Gray binary entry point.

use clap::Parser;
use gray::Cli;
use gray::config::Config;
use gray::print::run_print_mode_with_session;
use gray::repl::run_repl_mode;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    gray::logging::init();
    install_panic_hook();
    let _ = crossterm::terminal::disable_raw_mode();
    let cli = Cli::parse();
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
    let mut config = Config::resolve(&cli)?;
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
            gray::Commands::Update => {
                return gray::update::update_now().await;
            }
            gray::Commands::Gateway { cmd } => {
                return run_gateway(cmd).await;
            }
            gray::Commands::Plugin { cmd } => {
                return run_plugin(cmd).await;
            }
            gray::Commands::Cron { cmd } => {
                return run_cron(cmd).await;
            }
            gray::Commands::Send { target, text } => {
                return run_send(&target, &text).await;
            }
        }
    }
    if let Some(prompt) = cli.print.as_deref() {
        if let Some(agent) = cli.acp.as_deref() {
            return run_acp_print_mode(agent, prompt).await;
        }
        run_print_mode_with_session(&config, prompt, cli.session.as_deref(), cli.continue_last)
            .await?;
    } else {
        gray::update::startup_check().await;
        run_repl_mode(&mut config, cli.continue_last, cli.session.as_deref()).await?;
    }
    Ok(())
}

async fn run_acp_print_mode(agent: &str, prompt: &str) -> anyhow::Result<()> {
    let home = gray_acp::gray_home_dir();
    let Some(spec) = gray_acp::resolve(agent, Some(home.as_path())) else {
        anyhow::bail!("unknown acp agent '{agent}' (try /acp list in the REPL)");
    };
    if !gray_acp::installed(&spec) {
        anyhow::bail!("agent '{}' not installed ({})", spec.key, spec.install_hint);
    }
    let cwd = std::env::current_dir()?;
    let auto_approve = std::env::var("GRAY_ACP_AUTO_APPROVE").as_deref() == Ok("1");
    let display = if spec.display.is_empty() {
        spec.key.to_string()
    } else {
        spec.display.to_string()
    };
    let opts = gray_acp::AcpSessionOptions {
        spec,
        cwd,
        resume_session_id: None,
        auto_approve,
        permission_prompt: std::sync::Arc::new(gray_acp::DenyAllPrompt),
        display,
    };
    let mut session = gray_acp::AcpSession::start(opts).await?;
    let mut on_event = |ev: &gray_core::event::AgentEvent| {
        use gray_core::event::AgentEvent;
        if let AgentEvent::TextDelta { delta } = ev {
            print!("{delta}");
            let _ = std::io::Write::flush(&mut std::io::stdout());
        }
    };
    let res = session.prompt(prompt, &mut on_event).await;
    session.shutdown().await;
    println!();
    res?;
    Ok(())
}

async fn run_resume_subcommand(
    config: &mut Config,
    session_id: Option<&str>,
    last: bool,
    all: bool,
) -> anyhow::Result<()> {
    use gray_session::{JsonlSessionStore, default_root};
    let Some(root) = default_root() else {
        anyhow::bail!("cannot resolve home");
    };
    let store = JsonlSessionStore::new(root);
    let target_id = if let Some(raw) = session_id {
        // Shared with `-p --session`: one validation, one error message.
        gray::resume::resolve_session_strict(&store, raw, all).await?
    } else if last {
        let cwd = std::env::current_dir().ok();
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
    } else {
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

async fn run_gateway(cmd: Option<gray::GatewayCmd>) -> anyhow::Result<()> {
    use gray::GatewayCmd;
    match cmd {
        None | Some(GatewayCmd::Status { probe: false }) => gray_gateway::systemd::status(),
        Some(GatewayCmd::Status { probe: true }) => {
            let home = std::env::var("GRAY_HOME")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|_| {
                    std::env::var("HOME")
                        .map(|h| std::path::PathBuf::from(h).join(".gray"))
                        .unwrap_or_else(|_| std::path::PathBuf::from("/tmp/.gray"))
                });
            let h = gray_supervise::health::probe_full(
                &home,
                gray_gateway::status::read_board_healthy(&home),
                gray_gateway::status::gateway_config_parses(&home),
            );
            println!("{}", h.reason);
            if !h.healthy {
                std::process::exit(1);
            }
            Ok(())
        }
        Some(GatewayCmd::Run) => match gray_gateway::daemon::run_gateway().await {
            Err(e) if format!("{e:#}").contains("no gateway platforms enabled") => {
                eprintln!("{e:#}");
                std::process::exit(gray_supervise::exit::EXIT_FATAL);
            }
            r => r,
        },
        Some(GatewayCmd::Install) => gray_gateway::systemd::install(),
        Some(GatewayCmd::Uninstall) => gray_gateway::systemd::uninstall(),
        Some(GatewayCmd::Invite { platform }) => print_invite(&platform),
        Some(GatewayCmd::Pairing { cmd }) => run_pairing(cmd),
    }
}

fn run_pairing(cmd: gray::PairingCmd) -> anyhow::Result<()> {
    use gray::PairingCmd;
    use gray_gateway::pairing::{pairing_approve, pairing_list, pairing_revoke};
    match cmd {
        PairingCmd::Approve { platform, code } => {
            println!("{}", pairing_approve(&platform, &code)?)
        }
        PairingCmd::List { platform } => println!("{}", pairing_list(Some(&platform))?),
        PairingCmd::Revoke { platform, user } => println!("{}", pairing_revoke(&platform, &user)?),
    }
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
        PluginCmd::Check { dir } => {
            gray::plugin_check::check_plugin_dir(&dir).await?;
            Ok(())
        }
        PluginCmd::List => {
            let plugins = gray_pkg::ops::list()?;
            if plugins.is_empty() {
                println!("no plugins installed");
            }
            for (name, e) in &plugins {
                let state = if e.enabled { "" } else { " [disabled]" };
                println!("{} {} ({}){state}", name, e.version, e.scope);
            }
            Ok(())
        }
        PluginCmd::Search { query } => {
            let out = gray_pkg::ops::search_all(&query).await?;
            if out.hits.is_empty()
                && !out.pi_unreachable
                && !out.gray_unreachable
                && !out.clawhub_unreachable
                && !out.claude_unreachable
            {
                anyhow::bail!("not in index: {query} (try /plugin install <https-url>)");
            }
            for hit in &out.hits {
                println!("{}", gray_pkg::ops::format_search_hit(hit));
            }
            if out.gray_unreachable {
                println!("{}", gray_pkg::ops::GRAY_UNREACHABLE_LINE);
            }
            if out.pi_unreachable {
                println!("{}", gray_pkg::ops::PI_UNREACHABLE_LINE);
            }
            if out.clawhub_unreachable {
                println!("{}", gray_pkg::ops::CLAWHUB_UNREACHABLE_LINE);
            }
            if out.claude_unreachable {
                println!("{}", gray_pkg::ops::CLAUDE_UNREACHABLE_LINE);
            }
            Ok(())
        }
        PluginCmd::Install { spec } => {
            let r = gray_pkg::ops::install(spec, gray_pkg::ops::InstallOpts::default()).await?;
            println!("installed {} {} at {}", r.name, r.version, r.path.display());
            Ok(())
        }
        PluginCmd::Remove { name } => {
            gray_pkg::ops::remove(&name)?;
            println!("removed {name}");
            Ok(())
        }
        PluginCmd::Update { target } => {
            let reports = gray_pkg::ops::update(&target).await?;
            if reports.is_empty() {
                println!("up to date");
            }
            for r in reports {
                println!("updated {} {}", r.name, r.version);
            }
            Ok(())
        }
        PluginCmd::Enable { name } => {
            gray_pkg::ops::set_enabled(&name, true)?;
            println!("enabled {name}");
            Ok(())
        }
        PluginCmd::Disable { name } => {
            gray_pkg::ops::set_enabled(&name, false)?;
            println!("disabled {name}");
            Ok(())
        }
    }
}

/// `gray cron ...` + `gray send ...` (cron plan Task 5).
///
/// File-only surface: the store lives at `$GRAY_HOME/cron/jobs.json` and
/// `send_once` builds a throw-away send-only adapter from `gateway.yaml`,
/// so neither verb needs the daemon running.
fn cron_store() -> anyhow::Result<gray_cron::CronStore> {
    let home = gray_gateway::config::gray_home_dir()?;
    gray_cron::CronStore::open(home.join("cron"))
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

fn fmt_schedule(s: &gray_cron::Schedule) -> String {
    match s {
        gray_cron::Schedule::Interval { secs } => format!("every {}", fmt_dur(*secs)),
        gray_cron::Schedule::Cron { expr } => expr.clone(),
        gray_cron::Schedule::Once { at } => format!("once {}", fmt_ts(*at)),
    }
}

fn fmt_deliver(d: &gray_cron::Deliver) -> String {
    match d {
        gray_cron::Deliver::Origin => "origin".to_string(),
        gray_cron::Deliver::Local => "local".to_string(),
        gray_cron::Deliver::Target(s) => s.clone(),
    }
}

fn fmt_status(s: Option<gray_cron::RunStatus>) -> &'static str {
    match s {
        None => "-",
        Some(gray_cron::RunStatus::Ok) => "ok",
        Some(gray_cron::RunStatus::Error) => "error",
        Some(gray_cron::RunStatus::DeliveryFailed) => "delivery_failed",
    }
}

/// `--deliver` flag: `origin`/`local` keywords (case-insensitive), anything
/// else rides `Deliver::Target` and resolves at fire time (unknown shapes
/// fail safe to save-only in the daemon, never misdeliver).
fn parse_deliver_flag(raw: Option<&str>) -> gray_cron::Deliver {
    match raw.map(str::trim).unwrap_or("local") {
        s if s.eq_ignore_ascii_case("origin") => gray_cron::Deliver::Origin,
        s if s.eq_ignore_ascii_case("local") || s.is_empty() => gray_cron::Deliver::Local,
        s => gray_cron::Deliver::Target(s.to_string()),
    }
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

async fn run_cron(cmd: gray::CronCmd) -> anyhow::Result<()> {
    use gray::CronCmd;
    match cmd {
        CronCmd::List => {
            let jobs = cron_store()?.list()?;
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
            Ok(())
        }
        CronCmd::Add {
            schedule,
            prompt,
            deliver,
            name,
            workdir,
        } => {
            let store = cron_store()?;
            let name = name.unwrap_or_else(|| default_job_name(&prompt));
            let id = store.add_full(
                &name,
                &schedule,
                &prompt,
                parse_deliver_flag(deliver.as_deref()),
                None,
                workdir,
            )?;
            let next = store
                .get(&id)?
                .map(|j| fmt_ts_opt(j.next_run_at))
                .unwrap_or_else(|| "-".to_string());
            println!("added {id} next {next}");
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
                println!("origin: {}:{} thread:{thread}", o.platform, o.chat);
            }
            if let Some(w) = &j.workdir {
                println!("workdir: {}", w.display());
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
    }
}

async fn run_send(target: &str, text: &[String]) -> anyhow::Result<()> {
    let cfg = gray_gateway::config::load_gateway_config();
    gray_gateway::delivery::send_once(&cfg, target, &text.join(" ")).await?;
    println!("sent to {target}");
    Ok(())
}

fn print_invite(platform: &str) -> anyhow::Result<()> {
    match platform.to_ascii_lowercase().as_str() {
        "discord" => {
            let cfg = gray_gateway::config::load_gateway_config();
            let plat = cfg.platforms.get(&gray_gateway::config::Platform::Discord);
            // client_id is derived from the token.
            let id = plat
                .and_then(|c| c.token.clone())
                .and_then(|t| gray_gateway::discord::client_id_from_token(&t))
                .ok_or_else(|| {
                    anyhow::anyhow!("set platforms.discord.token in ~/.gray/gateway.yaml")
                })?;
            println!("{}", gray_gateway::discord::invite_url(&id)?);
            Ok(())
        }
        other => anyhow::bail!("no invite URL for platform {other:?} (only discord)"),
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
