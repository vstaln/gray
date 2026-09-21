//! Startup update check + self-update via the gray.alignment.id installer.
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::process::Command;

fn base_url() -> String {
    std::env::var("GRAY_CDN_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "https://gray.alignment.id/dl".to_string())
}
pub const CHANNEL: &str = env!("GRAY_CHANNEL");

/// Numeric triple plus prerelease identifiers, build metadata dropped.
fn parse_version(v: &str) -> Option<(u64, u64, u64, Option<String>)> {
    let v = v.trim();
    let (core, pre) = match v.split_once('-') {
        Some((core, pre)) => (core, Some(pre.split('+').next()?.to_string())),
        None => (v.split('+').next()?, None),
    };
    let mut it = core.split('.');
    Some((
        it.next()?.parse().ok()?,
        it.next()?.parse().ok()?,
        it.next()?.parse().ok()?,
        pre.filter(|p| !p.is_empty()),
    ))
}

fn is_newer(latest: &str, current: &str) -> bool {
    match (parse_version(latest), parse_version(current)) {
        (Some(l), Some(c)) => version_cmp(&l, &c) == std::cmp::Ordering::Greater,
        _ => false,
    }
}

/// Full SemVer precedence (§ 11): a prerelease sorts below its release;
/// identifiers compare numerically when both numeric, else lexically, with
/// numeric below alphanumeric and fewer fields below more.
fn version_cmp(
    a: &(u64, u64, u64, Option<String>),
    b: &(u64, u64, u64, Option<String>),
) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let core = (a.0, a.1, a.2).cmp(&(b.0, b.1, b.2));
    if core != Ordering::Equal {
        return core;
    }
    match (&a.3, &b.3) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(x), Some(y)) => cmp_prerelease(x, y),
    }
}

fn cmp_prerelease(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let mut ai = a.split('.');
    let mut bi = b.split('.');
    loop {
        match (ai.next(), bi.next()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) => match (x.parse::<u64>(), y.parse::<u64>()) {
                (Ok(nx), Ok(ny)) => match nx.cmp(&ny) {
                    Ordering::Equal => continue,
                    other => return other,
                },
                (Ok(_), Err(_)) => return Ordering::Less,
                (Err(_), Ok(_)) => return Ordering::Greater,
                (Err(_), Err(_)) => match x.cmp(y) {
                    Ordering::Equal => continue,
                    other => return other,
                },
            },
        }
    }
}

fn update_available(channel: &str, latest: &str, current: &str, build: &str) -> bool {
    if channel == "beta" {
        !latest.trim().is_empty() && latest.trim() != build.trim()
    } else {
        is_newer(latest, current)
    }
}

async fn latest_version() -> anyhow::Result<String> {
    let suffix = if CHANNEL == "beta" { "-build" } else { "" };
    let url = format!("{}/latest-{CHANNEL}{suffix}.txt", base_url());
    let txt = reqwest::get(&url).await?.error_for_status()?.text().await?;
    Ok(txt.trim().to_string())
}

/// Optional installer pin: when `GRAY_INSTALLER_SHA256` is set, the update
/// path downloads install.sh first, verifies sha256, then executes the
/// verified bytes instead of `curl|sh` on a mutable script.
fn installer_pin() -> Option<String> {
    std::env::var("GRAY_INSTALLER_SHA256")
        .ok()
        .map(|s| s.trim().to_lowercase())
        .filter(|s| s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit()))
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
    anyhow::ensure!(
        !cfg!(windows),
        "self-update is not supported on native Windows: close Gray and rerun install-native.ps1 with the verified preview ZIP and checksum"
    );
    // Pinned path: download install.sh, check sha256 against
    // GRAY_INSTALLER_SHA256, execute the verified bytes.
    if let Some(pin) = installer_pin() {
        let dir = std::env::temp_dir().join(format!("gray-installer-{}", std::process::id()));
        std::fs::create_dir_all(&dir)?;
        let file = dir.join("install.sh");
        let dl = Command::new("sh")
            .arg("-c")
            .arg(format!(
                "curl -fsSL {} > {}",
                shell_escape(&format!("{}/../install.sh", base_url())),
                shell_escape(&file.to_string_lossy())
            ))
            .status()?;
        anyhow::ensure!(dl.success(), "installer download failed");
        let bytes = std::fs::read(&file)?;
        let actual = sha256_hex(&bytes);
        anyhow::ensure!(
            actual == pin,
            "installer checksum mismatch (expected {pin}, got {actual}) — refusing to run"
        );
        let status = Command::new("sh").arg(&file).status()?;
        let _ = std::fs::remove_dir_all(&dir);
        anyhow::ensure!(status.success(), "installer failed");
        return Ok(());
    }
    let status = Command::new("sh")
        .arg("-c")
        .arg(install_command())
        .status()?;
    anyhow::ensure!(status.success(), "installer failed");
    Ok(())
}

fn shell_escape(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn sha256_hex(bytes: &[u8]) -> String {
    // ponytail: shell out to sha256sum/shasum, no new dep for one check
    let tmp = std::env::temp_dir().join(format!("gray-hash-{}", std::process::id()));
    if std::fs::write(&tmp, bytes).is_err() {
        return String::new();
    }
    for prog in ["sha256sum", "shasum"] {
        let args: &[&str] = if prog == "shasum" {
            &["-a", "256"]
        } else {
            &[]
        };
        if let Ok(out) = Command::new(prog).args(args).arg(&tmp).output()
            && out.status.success()
        {
            let hex: String = String::from_utf8_lossy(&out.stdout)
                .split_whitespace()
                .next()
                .unwrap_or_default()
                .to_lowercase();
            let _ = std::fs::remove_file(&tmp);
            if hex.len() == 64 {
                return hex;
            }
        }
    }
    let _ = std::fs::remove_file(&tmp);
    String::new()
}

/// Exclusive-update lock path: `<gray-home>/logs/update.lock` (temp fallback).
fn update_lock_path() -> PathBuf {
    crate::setup::gray_home()
        .map(|h| h.join("logs").join("update.lock"))
        .unwrap_or_else(|_| std::env::temp_dir().join("gray-update.lock"))
}

pub(crate) struct UpdateLock(std::fs::File);

impl Drop for UpdateLock {
    fn drop(&mut self) {
        // Closing alone leaves flock held if a concurrent fork inherited the
        // descriptor. Explicit unlock ends ownership when this guard ends.
        let _ = self.0.unlock();
    }
}

/// Acquires the exclusive update lock; explicitly released when the guard drops.
pub(crate) fn acquire_update_lock_at(path: &Path) -> std::io::Result<UpdateLock> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let f = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)?;
    f.lock()?;
    Ok(UpdateLock(f))
}

fn acquire_update_lock() -> std::io::Result<UpdateLock> {
    acquire_update_lock_at(&update_lock_path())
}

/// Installer run under mutual exclusion.
fn run_installer_locked() -> anyhow::Result<()> {
    let _lock = acquire_update_lock()?;
    run_installer()
}

/// Where the installer writes: `$GRAY_INSTALL_DIR`, else the per-OS default —
/// `/usr/local/bin` as root and `~/.local/bin` otherwise on Unix (what
/// `dist/install.sh` does), `%LOCALAPPDATA%\Programs\gray\bin` on Windows
/// (what `dist/install-native.ps1` does).
fn installer_dest() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("GRAY_INSTALL_DIR").filter(|s| !s.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    #[cfg(unix)]
    if let Some(dest) = unix_installer_dest() {
        return Some(dest);
    }
    #[cfg(windows)]
    if let Some(dest) = windows_installer_dest() {
        return Some(dest);
    }
    None
}

#[cfg(unix)]
fn unix_installer_dest() -> Option<PathBuf> {
    // Root first: a root shell with no HOME still installs to /usr/local/bin,
    // and the shadow guard must not lose the destination over it.
    // SAFETY: geteuid takes no arguments and cannot fail.
    if unsafe { libc::geteuid() } == 0 {
        return Some(PathBuf::from("/usr/local/bin"));
    }
    let home = std::env::var_os("HOME").filter(|s| !s.is_empty())?;
    Some(PathBuf::from(home).join(".local").join("bin"))
}

#[cfg(windows)]
fn windows_installer_dest() -> Option<PathBuf> {
    let local = std::env::var_os("LOCALAPPDATA").filter(|s| !s.is_empty())?;
    Some(
        PathBuf::from(local)
            .join("Programs")
            .join("gray")
            .join("bin"),
    )
}

/// `gray --version` of the binary at `path` ("gray 0.1.1" -> "0.1.1"). Only
/// ever called on the installer's own destination: a PATH-resolved candidate
/// is untrusted search-path output and is never executed (CWE-426).
fn binary_version(path: &Path) -> Option<String> {
    let out = Command::new(path).arg("--version").output().ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .nth(1)
        .map(str::to_string)
}

/// Launcher names a shell may resolve in a PATH entry: Windows runs
/// `gray.exe`, Unix `gray`.
fn gray_names() -> &'static [&'static str] {
    #[cfg(windows)]
    {
        &["gray.exe", "gray"]
    }
    #[cfg(not(windows))]
    {
        &["gray"]
    }
}

/// First `gray` among `entries`, in PATH order, the way a shell resolves it.
fn first_gray_in(entries: &[PathBuf]) -> Option<PathBuf> {
    entries.iter().find_map(|d| {
        gray_names()
            .iter()
            .map(|name| d.join(name))
            .find(|c| is_launcher(c))
    })
}

/// A launcher the shell would actually run. On Unix that means the executable
/// bit: a plain data file named `gray` cannot shadow anything, so it must not
/// raise a warning either. On Windows the name list carries the check
/// (`gray.exe`, then `gray`).
fn is_launcher(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::metadata(path).is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// The `gray` the next shell will actually launch.
fn first_gray_on_path() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let entries: Vec<PathBuf> = std::env::split_paths(&path).collect();
    first_gray_in(&entries)
}

/// Same binary under two paths (`~/.local/bin` is often a symlink farm).
fn same_binary(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

/// Decision half of the post-update guard: `installed` is the build the
/// installer just wrote, `resolved` the one a new shell launches instead.
fn shadow_warning(installed: &Path, installed_version: &str, resolved: &Path) -> Option<String> {
    if same_binary(resolved, installed) {
        return None;
    }
    Some(format!(
        "⚠ {} comes first on PATH and shadows the updated {} (gray {installed_version}) — new shells keep launching the old build\n  fix:  rm {}   (then open a new shell)",
        resolved.display(),
        installed.display(),
        resolved.display(),
    ))
}

/// Post-update guard. The installer writes one path; the shell may resolve
/// another. A stale copy earlier on PATH (an old `cargo install --path`, say)
/// keeps launching the previous build, so "updated" would be a lie and the
/// banner would never move.
fn post_update_shadow_warning() -> Option<String> {
    let installed = installer_dest()?.join("gray");
    // The installed build is the one the installer just wrote, so asking it
    // for its version is the same trust the update itself already carries.
    let installed_version = binary_version(&installed)?;
    let resolved = first_gray_on_path()?;
    shadow_warning(&installed, &installed_version, &resolved)
}

/// Report a shadowed install: the update landed, but the next launch would not
/// pick it up.
fn warn_on_shadow() {
    if let Some(w) = post_update_shadow_warning() {
        eprintln!("{w}");
    }
}

/// Decision half of the startup guard: `current` is this process's binary,
/// `resolved` the one a new shell launches instead.
fn divergence_warning(current: &Path, running: &str, resolved: &Path) -> Option<String> {
    if same_binary(resolved, current) {
        return None;
    }
    Some(format!(
        "⚠ {} comes first on PATH and is not this build (gray {running}, {}) — a new shell may launch a different gray\n  relaunch `gray` to run it",
        resolved.display(),
        current.display(),
    ))
}

/// Startup guard for the same hazard without an update in the picture: this
/// process runs one build while PATH resolves a newer one, so every new shell
/// silently lands on a different gray than the one already open.
fn path_divergence_warning() -> Option<String> {
    let current = std::env::current_exe().ok()?;
    let resolved = first_gray_on_path()?;
    divergence_warning(&current, env!("CARGO_PKG_VERSION"), &resolved)
}

/// Report a PATH/build divergence. Cadence-free: callers gate it behind the
/// daily update check.
fn warn_on_divergence() {
    if let Some(w) = path_divergence_warning() {
        eprintln!("{w}");
    }
}

/// Manual `gray update`: run the installer unconditionally, then exit hint.
pub async fn update_now() -> anyhow::Result<()> {
    // Windows locks its running executable. Never launch the Unix installer
    // or advertise an update we could not install; external reinstall only.
    anyhow::ensure!(
        !cfg!(windows),
        "self-update is not supported on native Windows: close Gray and rerun install-native.ps1 with the verified preview ZIP and checksum"
    );
    println!("→ updating gray ({CHANNEL})...");
    run_installer_locked()?;
    println!("✓ updated. restart gray to use the new version.");
    warn_on_shadow();
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
    if cfg!(windows) && std::env::var("GRAY_AUTO_UPDATE").as_deref() == Ok("1") {
        eprintln!(
            "Automatic installation is not supported on native Windows; close Gray and reinstall externally."
        );
    }
    if cfg!(debug_assertions) || current == "0.0.0" {
        return;
    }
    let now = now_secs();
    if !update_check_due(read_last_check(), now) {
        return;
    }
    write_last_check(now);
    // No update needed for this to matter: a stale copy first on PATH keeps
    // every new shell on the previous build, and nothing else would say so.
    warn_on_divergence();
    let Ok(Ok(latest)) =
        tokio::time::timeout(std::time::Duration::from_millis(1500), latest_version()).await
    else {
        return;
    };
    if !update_available(CHANNEL, &latest, current, env!("GRAY_BUILD_ID")) {
        return;
    }
    if cfg!(windows) {
        println!(
            "gray {latest} available; on native Windows close Gray and rerun install-native.ps1 externally."
        );
        return;
    }
    let auto_flag = std::env::var("GRAY_AUTO_UPDATE").ok();
    if auto_update_allowed(CHANNEL, auto_flag.as_deref()) {
        let latest = latest.clone();
        tokio::task::spawn_blocking(move || {
            let Ok(_lock) = acquire_update_lock() else {
                return;
            };
            let ok = Command::new("sh")
                .arg("-c")
                .arg(install_command())
                .output()
                .is_ok_and(|o| o.status.success());
            if ok {
                crate::profile::queue_profile_warning(format!(
                    "gray {latest} installed in the background — restart to apply"
                ));
                // Same hazard as a manual update, and this path is the one that
                // never asked: a stale copy earlier on PATH keeps every new
                // shell on the previous build.
                if let Some(w) = post_update_shadow_warning() {
                    crate::profile::queue_profile_warning(w);
                }
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
                warn_on_shadow();
                std::process::exit(0);
            }
            Err(e) => eprintln!("update failed: {e}"),
        }
    }
}

#[path = "update_tests.rs"]
#[cfg(test)]
mod tests;
