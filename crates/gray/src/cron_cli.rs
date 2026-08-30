use clap::{Parser, Subcommand};

#[derive(Parser, Debug, Clone)]
pub struct CronArgs {
    #[command(subcommand)]
    pub cmd: Option<CronCmd>,
}

#[derive(Debug, Clone, Subcommand)]
pub enum CronCmd {
    /// List cron jobs
    List,
    /// Create a cron job: gray cron create --schedule "every 30m" --prompt "check inbox"
    Create {
        #[arg(long)] schedule: String,
        #[arg(long)] prompt: String,
        #[arg(long)] name: Option<String>,
    },
    /// Shorthand: gray cron add "check inbox every 30m" / "remind me in 10m"
    Add {
        /// Human input like "check inbox every 30m" — schedule auto-extracted
        input: String,
        #[arg(long)] name: Option<String>,
    },
    /// Remove a job by id or name
    Remove { id: String },
    /// Show a job
    Show { id: String },
    /// Run the scheduler daemon (checks due jobs every 60s and runs them)
    Run,
}

pub fn run_cron(args: CronArgs) -> anyhow::Result<()> {
    let cmd = args.cmd.unwrap_or(CronCmd::List);
    match cmd {
        CronCmd::List => {
            let jobs = gray_cron::list_jobs();
            if jobs.is_empty() {
                println!("no cron jobs — create one with: gray cron create --schedule \"every 30m\" --prompt \"...\"");
                return Ok(());
            }
            println!("{:<10} {:<20} {:<16} {}", "ID", "NAME", "SCHEDULE", "NEXT RUN");
            println!("{}", "-".repeat(70));
            for j in jobs {
                let next = j
                    .next_run
                    .map(|t| t.format("%Y-%m-%d %H:%M UTC").to_string())
                    .unwrap_or_else(|| "-".to_string());
                let enabled = if j.enabled { "" } else { " (paused)" };
                println!(
                    "{:<10} {:<20} {:<16} {}{}",
                    j.id, j.name, j.schedule, next, enabled
                );
            }
        }
        CronCmd::Create { schedule, prompt, name } => {
            let n = name.unwrap_or_else(|| format!("job-{}", &prompt.chars().take(12).collect::<String>()));
            // Validate
            gray_cron::parse_schedule(&schedule)?;
            let job = gray_cron::create_job(n.clone(), schedule.clone(), prompt.clone())?;
            println!("created cron job {} (\"{}\") — schedule: {} — next: {}",
                job.id,
                job.name,
                job.schedule,
                job.next_run.map(|t| t.to_string()).unwrap_or_else(|| "-".to_string())
            );
        }
        CronCmd::Add { input, name } => {
            let (schedule, prompt) = gray_cron::schedule::split_human_input(&input)
                .ok_or_else(|| anyhow::anyhow!("could not parse schedule from '{input}' — try 'check inbox every 30m' or 'remind me in 10m' or cron '0 9 * * *'"))?;
            let n = name.clone().unwrap_or_else(|| format!("job-{}", &prompt.chars().take(12).collect::<String>()));
            let job = gray_cron::create_job(n.clone(), schedule.clone(), prompt.clone())?;
            println!("created cron job {} (\"{}\") — schedule: {} — next: {}",
                job.id, job.name, job.schedule,
                job.next_run.map(|t| t.to_string()).unwrap_or_else(|| "-".to_string())
            );
        }
        CronCmd::Remove { id } => {
            if gray_cron::remove_job(&id)? {
                println!("removed {id}");
            } else {
                println!("no job found for '{id}'");
            }
        }
        CronCmd::Show { id } => {
            if let Some(j) = gray_cron::find_job(&id) {
                println!("{}", serde_json::to_string_pretty(&j)?);
            } else {
                println!("no job found for '{id}'");
            }
        }
        CronCmd::Run => {
            use gray_cron::Scheduler;
            use std::sync::Arc;
            println!("gray cron daemon running — checking every 60s (Ctrl-C to stop)...");
            let scheduler = Scheduler::from_active();
            let guard = Arc::new(gray_cron::InflightGuard::new());
            loop {
                match scheduler.scan_due_jobs() {
                    Ok(due) => {
                        for job in due {
                            if !guard.try_register_running_job(&job.id) {
                                continue;
                            }
                            println!("→ running cron job {} (\"{}\"): {}", job.id, job.name, job.prompt);
                            let _ = gray_cron::store::update_job_run(&job.id, chrono::Utc::now());
                            // Daemon headless: for now log + update; REPL background will run Agent.
                            // One-shot jobs: next_run=None after update, so they won't refire.
                            guard.release_running_job(&job.id);
                        }
                    }
                    Err(e) => eprintln!("cron scan failed: {e}"),
                }
                std::thread::sleep(std::time::Duration::from_secs(60));
            }
        }
    }
    Ok(())
}
