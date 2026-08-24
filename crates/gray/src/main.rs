//! Gray binary entry point.

use clap::Parser;
use gray::config::Config;
use gray::print::run_print_mode;
use gray::repl::run_repl_mode;
use gray::Cli;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let mut config = Config::resolve(&cli)?;
    if let Some(prompt) = cli.print.as_deref() {
        run_print_mode(&config, prompt).await?;
    } else {
        run_repl_mode(&mut config).await?;
    }
    Ok(())
}
