use clap::{Parser, Subcommand};
use gray_heartbeat::{config, enable, goal, job};

#[derive(Parser)]
#[command(name = "gray-heartbeat", about = "gray's 24/7 standing-goal heartbeat")]
struct Cli {
    /// Run as a gray sidecar plugin (NDJSON over stdio) instead of a one-shot CLI.
    #[arg(long)]
    plugin: bool,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Turn the heartbeat on and create/refresh its cron job.
    On {
        /// Schedule, e.g. "every 30m" or "every 1h".
        #[arg(long)]
        every: Option<String>,
        /// Delivery target: local | origin | <target>.
        #[arg(long)]
        deliver: Option<String>,
    },
    /// Turn the heartbeat off (removes the cron job).
    Off,
    /// Show whether the heartbeat is enabled and its next run.
    Status,
    /// Print the standing goal, or set it with `goal set <text...>`.
    Goal {
        #[command(subcommand)]
        action: Option<GoalAction>,
    },
    /// Re-render the cron job from the current goal and config.
    Sync,
}

#[derive(Subcommand)]
enum GoalAction {
    /// Set the standing goal.
    Set { text: Vec<String> },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    if cli.plugin {
        return gray_heartbeat::plugin::serve_plugin();
    }
    let Some(command) = cli.command else {
        anyhow::bail!("no command given (try --help)");
    };
    match command {
        Command::On { every, deliver } => {
            let mut cfg = config::load_config()?;
            enable(&mut cfg, every, deliver)?;
            println!("heartbeat on ({}), deliver={}", cfg.schedule, cfg.deliver);
        }
        Command::Off => {
            let mut cfg = config::load_config()?;
            cfg.enabled = false;
            config::save_config(&cfg)?;
            job::sync_job(&cfg, &goal::read_goal()?)?;
            println!("heartbeat off");
        }
        Command::Status => {
            let cfg = config::load_config()?;
            match job::job_status(&cfg)? {
                job::JobStatus::Disabled => println!("disabled"),
                job::JobStatus::Missing => println!("enabled (job missing — run sync)"),
                job::JobStatus::Live { next_run_at } => match next_run_at {
                    Some(t) => println!("enabled, next run {}", fmt_ts(t)),
                    None => println!("enabled"),
                },
            }
        }
        Command::Goal { action } => match action {
            None => print!("{}", goal::read_goal()?),
            Some(GoalAction::Set { text }) => {
                goal::write_goal(&text.join(" "))?;
                println!("goal set");
            }
        },
        Command::Sync => {
            job::sync_job(&config::load_config()?, &goal::read_goal()?)?;
            println!("synced");
        }
    }
    Ok(())
}

fn fmt_ts(epoch: i64) -> String {
    match chrono::DateTime::from_timestamp(epoch, 0) {
        Some(dt) => dt.format("%Y-%m-%d %H:%M UTC").to_string(),
        None => epoch.to_string(),
    }
}
