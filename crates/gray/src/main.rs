//! Gray binary entry point.

use clap::Parser;
use gray::config::Config;
use gray::print::run_print_mode;
use gray::repl::run_repl_mode;
use gray::Cli;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    gray::logging::init();
    install_panic_hook();
    let _ = crossterm::terminal::disable_raw_mode();
    let cli = Cli::parse();
    let mut config = Config::resolve(&cli)?;
    // Best-effort: when the saved config is oauth-mode, pull a fresh access
    // token from ~/.gray/auth.json into the session (refreshing if stale).
    gray::oauth::apply_saved_oauth(&mut config).await;
    if let Some(prompt) = cli.print.as_deref() {
        run_print_mode(&config, prompt).await?;
    } else {
        run_repl_mode(&mut config, cli.continue_last, cli.session.as_deref()).await?;
    }
    Ok(())
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
