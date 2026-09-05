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

/// Writes one JSON receipt doc `{ts, channel, from, to, rc, reason}`.
/// `latest.json` is overwritten with the single latest doc (whole-file JSON);
/// history appends to sibling `history.jsonl`, trimmed to the last 200 lines.
/// Best effort: receipt failures never fail the update.
pub(crate) fn write_update_receipt_to(
    path: &Path,
    channel: &str,
    from: &str,
    to: &str,
    rc: i32,
    reason: &str,
) {
    const HISTORY_LINES: usize = 200;
    let _ = (|| -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let receipt = serde_json::json!({
            "ts": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            "channel": channel, "from": from, "to": to, "rc": rc, "reason": reason,
        });
        std::fs::write(path, format!("{receipt}\n"))?;
        let hist = path
            .parent()
            .map(|p| p.join("history.jsonl"))
            .unwrap_or_else(|| PathBuf::from("history.jsonl"));
        let mut f = OpenOptions::new().create(true).append(true).open(&hist)?;
        use std::io::Write as _;
        writeln!(f, "{receipt}")?;
        drop(f);
        let text = std::fs::read_to_string(&hist)?;
        let lines: Vec<&str> = text.lines().collect();
        if lines.len() > HISTORY_LINES {
            std::fs::write(
                &hist,
                lines[lines.len() - HISTORY_LINES..].join("\n") + "\n",
            )?;
        }
        Ok(())
    })();
}

fn write_update_receipt(channel: &str, from: &str, to: &str, rc: i32, reason: &str) {
    let path = crate::setup::gray_home()
        .map(|h| h.join("logs").join("update_receipts").join("latest.json"))
        .unwrap_or_else(|_| {
            std::env::temp_dir()
                .join("gray-update-receipts")
                .join("latest.json")
        });
    write_update_receipt_to(&path, channel, from, to, rc, reason);
}

/// Manual `gray update`: run the installer unconditionally, then exit hint.
pub async fn update_now() -> anyhow::Result<()> {
    let from = env!("CARGO_PKG_VERSION");
    println!("→ updating gray ({CHANNEL})...");
    let res = run_installer_locked();
    match &res {
        Ok(()) => write_update_receipt(CHANNEL, from, "", 0, "ok"),
        Err(e) => write_update_receipt(CHANNEL, from, "", 1, &e.to_string()),
    }
    res?;
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
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
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
    if std::env::var("GRAY_AUTO_UPDATE").as_deref() == Ok("1") {
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
            write_update_receipt(
                CHANNEL,
                current,
                &latest,
                if ok { 0 } else { 1 },
                if ok { "ok" } else { "installer failed" },
            );
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
                write_update_receipt(CHANNEL, current, &latest, 0, "ok");
                println!("✓ updated. restart gray to use {latest}.");
                std::process::exit(0);
            }
            Err(e) => {
                write_update_receipt(CHANNEL, current, &latest, 1, &e.to_string());
                eprintln!("update failed: {e}");
            }
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

    #[test]
    fn receipt_latest_overwrites_and_history_appends() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("update_receipts").join("latest.json");
        write_update_receipt_to(&path, "beta", "0.1.0", "0.2.0", 0, "ok");
        write_update_receipt_to(&path, "beta", "0.2.0", "", 1, "boom");
        // latest.json holds exactly the newest receipt doc (whole-file JSON).
        let text = std::fs::read_to_string(&path).unwrap();
        let v: serde_json::Value = serde_json::from_str(text.trim()).unwrap();
        assert_eq!(v["channel"], "beta");
        assert_eq!(v["from"], "0.2.0");
        assert_eq!(v["to"], "");
        assert_eq!(v["rc"], 1);
        assert_eq!(v["reason"], "boom");
        assert!(v["ts"].is_string());
        // history.jsonl keeps every receipt.
        let hist =
            std::fs::read_to_string(dir.path().join("update_receipts").join("history.jsonl"))
                .unwrap();
        let lines: Vec<&str> = hist.lines().collect();
        assert_eq!(lines.len(), 2);
        let h0: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(h0["from"], "0.1.0");
        let h1: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(h1["reason"], "boom");
    }

    #[test]
    fn receipt_history_trims_to_last_200() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("update_receipts").join("latest.json");
        for i in 0..205 {
            write_update_receipt_to(&path, "beta", "0.1.0", "0.2.0", 0, &format!("run-{i}"));
        }
        let hist =
            std::fs::read_to_string(dir.path().join("update_receipts").join("history.jsonl"))
                .unwrap();
        let lines: Vec<&str> = hist.lines().collect();
        assert_eq!(lines.len(), 200);
        let first: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(first["reason"], "run-5");
        let last: serde_json::Value = serde_json::from_str(lines[199]).unwrap();
        assert_eq!(last["reason"], "run-204");
        // latest.json still a single doc with the newest receipt.
        let v: serde_json::Value =
            serde_json::from_str(std::fs::read_to_string(&path).unwrap().trim()).unwrap();
        assert_eq!(v["reason"], "run-204");
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
