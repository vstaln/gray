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
            println!("gray cron daemon running — checking every 60s (Ctrl-C to stop)...");
            // Simple blocking loop; in real use you'd spawn this via `gray cron run &` or systemd
            loop {
                let jobs = gray_cron::list_jobs();
                let due = gray_cron::due_jobs(&jobs);
                for job in due {
                    println!("→ running cron job {} (\"{}\"): {}", job.id, job.name, job.prompt);
                    let _ = gray_cron::store::update_job_run(&job.id, chrono::Utc::now());
                    // TODO: actually spawn an agent run for the prompt when daemon is wired to gray-core
                }
                std::thread::sleep(std::time::Duration::from_secs(60));
            }
        }
    }
    Ok(())
}
