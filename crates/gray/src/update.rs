//! Startup update check + self-update via the gray.alignment.id installer.
use std::process::Command;

const BASE: &str = "https://gray.alignment.id/dl";
pub const CHANNEL: &str = env!("GRAY_CHANNEL");

fn parse_version(v: &str) -> Option<(u64, u64, u64)> {
    let mut it = v.trim().split('.');
    Some((it.next()?.parse().ok()?, it.next()?.parse().ok()?, it.next()?.parse().ok()?))
}

fn is_newer(latest: &str, current: &str) -> bool {
    match (parse_version(latest), parse_version(current)) {
        (Some(l), Some(c)) => l > c,
        _ => false,
    }
}

async fn latest_version() -> anyhow::Result<String> {
    let url = format!("{BASE}/latest-{CHANNEL}.txt");
    let txt = reqwest::get(&url).await?.error_for_status()?.text().await?;
    Ok(txt.trim().to_string())
}

/// curl -fsSL https://gray.alignment.id/install.sh | sh [- beta]
fn install_command() -> String {
    match CHANNEL {
        "stable" => "sh -c 'curl -fsSL https://gray.alignment.id/install.sh | sh'".into(),
        ch => format!("sh -c 'curl -fsSL https://gray.alignment.id/install.sh | sh -s -- {ch}'"),
    }
}

/// Ask y/n in raw mode. Returns true on y/Y.
fn confirm() -> bool {
    use crossterm::event::{self, Event, KeyCode, KeyEvent};
    if crossterm::terminal::enable_raw_mode().is_err() {
        return false;
    }
    let yes = matches!(event::read(), Ok(Event::Key(KeyEvent { code: KeyCode::Char('y') | KeyCode::Char('Y'), .. })));
    let _ = crossterm::terminal::disable_raw_mode();
    yes
}

fn run_installer() -> anyhow::Result<()> {
    let status = Command::new("sh").arg("-c").arg(install_command()).status()?;
    anyhow::ensure!(status.success(), "installer failed");
    Ok(())
}

/// Manual `gray update`: run the installer unconditionally, then exit hint.
pub async fn update_now() -> anyhow::Result<()> {
    println!("→ updating gray ({CHANNEL})...");
    run_installer()?;
    println!("✓ updated. restart gray to use the new version.");
    Ok(())
}

/// Called before the REPL starts. Checks for a newer release, prompts y/n.
/// Errors are silent — update checks must never break startup.
pub async fn startup_check() {
    let current = env!("CARGO_PKG_VERSION");
    if cfg!(debug_assertions) || current == "0.0.0" {
        return;
    }
    // ponytail: blocks startup up to ~1.5s waiting for the version check; add a
    // file cache like codex's updates_cache.rs if that ever annoys anyone.
    let Ok(Ok(latest)) = tokio::time::timeout(std::time::Duration::from_millis(1500), latest_version()).await else { return };
    if !is_newer(&latest, current) {
        return;
    }
    if std::env::var("GRAY_AUTO_UPDATE").as_deref() == Ok("1") {
        // ponytail: fire-and-forget spawn — installer output is lost; run `gray update`
        // to see it. Swap for a status line in the TUI if this ever matters.
        let latest = latest.clone();
        tokio::spawn(async move {
            if Command::new("sh").arg("-c").arg(install_command()).output().is_ok_and(|o| o.status.success()) {
                eprintln!("\x1b[2mgray {latest} installed in the background — restart to apply\x1b[0m");
            }
        });
        return;
    }
    println!("\x1b[1mgray {latest} available\x1b[0m \x1b[2m(you have {current})\x1b[0m — update now? \x1b[2m[y/N]\x1b[0m");
    if confirm() {
        if let Err(e) = run_installer() {
            eprintln!("update failed: {e}");
        } else {
            println!("✓ updated. restart gray to use {latest}.");
            std::process::exit(0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semver_compare() {
        assert!(is_newer("0.2.0", "0.1.9"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.2.0"));
    }

    #[test]
    fn bad_versions_never_newer() {
        assert!(!is_newer("garbage", "0.1.0"));
        assert!(!is_newer("0.1.0-beta.1", "0.1.0"));
        assert!(!is_newer(" 1.2.3 ", "1.2.3"));
        assert!(is_newer(" 1.2.3 ", "1.2.2"));
    }

    #[test]
    fn install_command_carries_channel() {
        assert!(install_command().contains("gray.alignment.id/install.sh"));
        if CHANNEL == "beta" {
            assert!(install_command().ends_with("beta'"));
        }
    }
}
