//! Startup update check + self-update via the gray.alignment.id installer.
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::process::Command;

const BASE: &str = "https://gray.alignment.id/dl";
pub const CHANNEL: &str = env!("GRAY_CHANNEL");

fn parse_version(v: &str) -> Option<(u64, u64, u64)> {
    let mut it = v.trim().split('.');
    Some((
        it.next()?.parse().ok()?,
        it.next()?.parse().ok()?,
        it.next()?.parse().ok()?,
    ))
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
///
/// Trust contract: self-update executes the installer's mutable HTTPS script.
/// Independent verification or pinning of that script is not a goal here;
/// payload checksums do not authenticate the installer that serves them.
/// This path is not an independently verified update.
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
    let yes = matches!(
        event::read(),
        Ok(Event::Key(KeyEvent {
            code: KeyCode::Char('y') | KeyCode::Char('Y'),
            ..
        }))
    );
    let _ = crossterm::terminal::disable_raw_mode();
    yes
}

fn run_installer() -> anyhow::Result<()> {
    let status = Command::new("sh")
        .arg("-c")
        .arg(install_command())
        .status()?;
    anyhow::ensure!(status.success(), "installer failed");
    Ok(())
}

/// Exclusive-update lock path: `<gray-home>/logs/update.lock` (temp fallback).
fn update_lock_path() -> PathBuf {
    crate::setup::gray_home()
        .map(|h| h.join("logs").join("update.lock"))
        .unwrap_or_else(|_| std::env::temp_dir().join("gray-update.lock"))
}

/// Acquires the exclusive update lock; held until the returned `File` drops.
pub(crate) fn acquire_update_lock_at(path: &Path) -> std::io::Result<std::fs::File> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let f = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)?;
    fs2::FileExt::lock_exclusive(&f)?;
    Ok(f)
}

fn acquire_update_lock() -> std::io::Result<std::fs::File> {
    acquire_update_lock_at(&update_lock_path())
}

/// Installer run under mutual exclusion.
fn run_installer_locked() -> anyhow::Result<()> {
    let _lock = acquire_update_lock()?;
    run_installer()
}

/// Manual `gray update`: run the installer unconditionally, then exit hint.
pub async fn update_now() -> anyhow::Result<()> {
    println!("→ updating gray ({CHANNEL})...");
    run_installer_locked()?;
    println!("✓ updated. restart gray to use the new version.");
    Ok(())
}

/// Seconds between update checks.
const CHECK_INTERVAL_SECS: u64 = 24 * 3600;

fn update_check_due(last_check_secs: Option<u64>, now_secs: u64) -> bool {
    match last_check_secs {
        None => true,
        // Clock skew (last check in the future) never blocks: check is due.
        Some(t) => match now_secs.checked_sub(t) {
            None => true,
            Some(elapsed) => elapsed >= CHECK_INTERVAL_SECS,
        },
    }
}

fn last_check_path() -> Option<PathBuf> {
    crate::setup::gray_home()
        .ok()
        .map(|h| h.join("logs").join("last_update_check"))
}

fn read_last_check() -> Option<u64> {
    let p = last_check_path()?;
    std::fs::read_to_string(p).ok()?.trim().parse().ok()
}

fn write_last_check(now_secs: u64) {
    if let Some(p) = last_check_path() {
        if let Some(parent) = p.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(p, now_secs.to_string());
    }
}

fn now_secs() -> u64 {
    chrono::Utc::now().timestamp().try_into().unwrap_or(0)
}

/// Auto self-update is stable-channel only: beta redeploys on every push to
/// main, so GRAY_AUTO_UPDATE=1 on beta would be a per-commit curl|sh
/// subscription. Manual `gray update` stays unconditional.
fn auto_update_allowed(channel: &str, flag: Option<&str>) -> bool {
    channel == "stable" && flag == Some("1")
}

/// Called before the REPL starts. Checks for a newer release, prompts y/n.
/// Errors are silent — update checks must never break startup.
pub async fn startup_check() {
    let current = env!("CARGO_PKG_VERSION");
    if std::env::var("GRAY_NO_UPDATE_CHECK").as_deref() == Ok("1") {
        return;
    }
    if cfg!(debug_assertions) || current == "0.0.0" {
        return;
    }
    let now = now_secs();
    if !update_check_due(read_last_check(), now) {
        return;
    }
    write_last_check(now);
    let Ok(Ok(latest)) =
        tokio::time::timeout(std::time::Duration::from_millis(1500), latest_version()).await
    else {
        return;
    };
    if !is_newer(&latest, current) {
        return;
    }
    let auto_flag = std::env::var("GRAY_AUTO_UPDATE").ok();
    if auto_update_allowed(CHANNEL, auto_flag.as_deref()) {
        let latest = latest.clone();
        tokio::spawn(async move {
            let Ok(_lock) = acquire_update_lock() else {
                return;
            };
            let ok = Command::new("sh")
                .arg("-c")
                .arg(install_command())
                .output()
                .is_ok_and(|o| o.status.success());
            if ok {
                eprintln!(
                    "\x1b[2mgray {latest} installed in the background — restart to apply\x1b[0m"
                );
            }
        });
        return;
    }
    println!(
        "\x1b[1mgray {latest} available\x1b[0m \x1b[2m(you have {current})\x1b[0m — update now? \x1b[2m[y/N]\x1b[0m"
    );
    if confirm() {
        match run_installer_locked() {
            Ok(()) => {
                println!("✓ updated. restart gray to use {latest}.");
                std::process::exit(0);
            }
            Err(e) => eprintln!("update failed: {e}"),
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
    fn auto_update_refuses_beta_channel() {
        assert!(auto_update_allowed("stable", Some("1")));
        assert!(!auto_update_allowed("beta", Some("1")));
        assert!(!auto_update_allowed("stable", Some("0")));
        assert!(!auto_update_allowed("stable", None));
    }

    #[test]
    fn install_command_carries_channel() {
        assert!(install_command().contains("gray.alignment.id/install.sh"));
        if CHANNEL == "beta" {
            assert!(install_command().ends_with("beta'"));
        }
    }

    #[test]
    fn check_due_logic() {
        assert!(update_check_due(None, 1_000_000));
        assert!(!update_check_due(Some(1_000_000), 1_000_000 + 3600));
        assert!(update_check_due(Some(1_000_000), 1_000_000 + 24 * 3600));
        assert!(update_check_due(Some(2_000_000), 1_000_000)); // clock skew never blocks
    }

    #[test]
    fn update_lock_is_exclusive() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("update.lock");
        let guard = acquire_update_lock_at(&path).unwrap();
        let probe = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        assert!(
            fs2::FileExt::try_lock_exclusive(&probe).is_err(),
            "second exclusive lock must fail while held"
        );
        drop(guard);
        assert!(fs2::FileExt::try_lock_exclusive(&probe).is_ok());
        let _ = fs2::FileExt::unlock(&probe);
    }
}
