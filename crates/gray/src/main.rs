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
    gray::oauth::apply_saved_oauth(&mut config).await;
    if let Some(gray::Commands::Resume { session_id, last, all, prompt }) = cli.command {
        return run_resume_subcommand(&mut config, session_id.as_deref(), last, all, prompt.as_deref()).await;
    }
    if let Some(prompt) = cli.print.as_deref() {
        run_print_mode(&config, prompt).await?;
    } else {
        run_repl_mode(&mut config, cli.continue_last, cli.session.as_deref()).await?;
    }
    Ok(())
}

async fn run_resume_subcommand(
    config: &mut Config,
    session_id: Option<&str>,
    last: bool,
    all: bool,
    prompt: Option<&str>,
) -> anyhow::Result<()> {
    use gray_session::{default_root, JsonlSessionStore, SessionStore};
    let Some(root) = default_root() else {
        anyhow::bail!("cannot resolve home");
    };
    let store = JsonlSessionStore::new(root);
    let target_id = if let Some(raw) = session_id {
        if let Some(resolved) = gray::resume::resolve_prefix(&store, raw, all).await {
            resolved
        } else {
            match store.load(&gray_session::SessionId::new(raw)).await {
                Ok(_) => gray_session::SessionId::new(raw),
                Err(e) => anyhow::bail!("no session matching '{raw}': {e}"),
            }
        }
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
    let _ = crossterm::terminal::disable_raw_mode();
    run_repl_mode(config, false, Some(target_id.as_str())).await?;
    if let Some(p) = prompt {
        let _ = p;
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
