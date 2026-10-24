use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn base_url() -> String {
    std::env::var("GRAY_CDN_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "https://gray.alignment.id/dl".to_string())
}
pub const CHANNEL: &str = env!("GRAY_CHANNEL");

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

pub(crate) fn update_available(channel: &str, latest: &str, current: &str, build: &str) -> bool {
    if channel == "beta" {
        !latest.trim().is_empty() && latest.trim() != build.trim()
    } else {
        is_newer(latest, current)
    }
}

pub(crate) async fn latest_version() -> anyhow::Result<String> {
    let suffix = if CHANNEL == "beta" { "-build" } else { "" };
    let url = format!("{}/latest-{CHANNEL}{suffix}.txt", base_url());
    let txt = reqwest::get(&url).await?.error_for_status()?.text().await?;
    Ok(txt.trim().to_string())
}

fn installer_pin() -> Option<String> {
    std::env::var("GRAY_INSTALLER_SHA256")
        .ok()
        .map(|s| s.trim().to_lowercase())
        .filter(|s| s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit()))
}

#[cfg(not(windows))]
fn install_command() -> String {
    match CHANNEL {
        "stable" => "sh -c 'curl -fsSL https://gray.alignment.id/install.sh | sh'".into(),
        ch => format!("sh -c 'curl -fsSL https://gray.alignment.id/install.sh | sh -s -- {ch}'"),
    }
}

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

fn run_step(cmd: &mut Command, quiet: bool, what: &str) -> anyhow::Result<()> {
    if !quiet {
        anyhow::ensure!(cmd.status()?.success(), "{what} failed");
        return Ok(());
    }
    let out = cmd.stdin(Stdio::null()).output()?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let tail = err.trim().lines().last().unwrap_or("");
        anyhow::bail!("{what} failed: {tail}");
    }
    Ok(())
}

#[cfg(not(windows))]
fn run_pinned_installer(pin: &str, base: &str, quiet: bool) -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("install.sh");
    run_step(
        Command::new("sh").arg("-c").arg(format!(
            "curl -fsSL {} > {}",
            shell_escape(&format!("{base}/../install.sh")),
            shell_escape(&file.to_string_lossy())
        )),
        quiet,
        "installer download",
    )?;
    let bytes = std::fs::read(&file)?;
    let actual = sha256_hex(&bytes);
    anyhow::ensure!(
        actual == pin,
        "installer checksum mismatch (expected {pin}, got {actual}) — refusing to run"
    );
    run_step(Command::new("sh").arg(&file), quiet, "installer")
}

fn run_installer() -> anyhow::Result<()> {
    run_installer_with(false)
}

fn run_installer_with(quiet: bool) -> anyhow::Result<()> {
    #[cfg(windows)]
    {
        windows_install(quiet)
    }
    #[cfg(not(windows))]
    {
        if let Some(pin) = installer_pin() {
            return run_pinned_installer(&pin, &base_url(), quiet);
        }
        run_step(
            Command::new("sh").arg("-c").arg(install_command()),
            quiet,
            "installer",
        )
    }
}

#[cfg(not(windows))]
fn shell_escape(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest as _;
    format!("{:x}", sha2::Sha256::digest(bytes))
}

fn update_lock_path() -> PathBuf {
    crate::setup::gray_home()
        .map(|h| h.join("logs").join("update.lock"))
        .unwrap_or_else(|_| std::env::temp_dir().join("gray-update.lock"))
}

pub(crate) struct UpdateLock(std::fs::File);

impl Drop for UpdateLock {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

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

pub(crate) fn try_acquire_update_lock() -> Option<UpdateLock> {
    let path = update_lock_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok()?;
    }
    let f = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .ok()?;
    match f.try_lock() {
        Ok(()) => Some(UpdateLock(f)),
        Err(_) => None,
    }
}

pub(crate) fn run_installer_locked() -> anyhow::Result<()> {
    let _lock = acquire_update_lock()?;
    run_installer()
}

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

#[cfg(windows)]
fn download_to(url: &str, dest: &Path, quiet: bool, what: &str) -> anyhow::Result<()> {
    run_step(
        Command::new("curl")
            .arg("-fsSL")
            .arg(url)
            .arg("-o")
            .arg(dest),
        quiet,
        what,
    )
}

#[cfg(any(windows, test))]
fn sums_digest(sums: &str, name: &str) -> anyhow::Result<String> {
    let found: Vec<&str> = sums
        .lines()
        .filter_map(|line| {
            let (digest, file) = line.trim().split_once(char::is_whitespace)?;
            (digest.len() == 64
                && digest.bytes().all(|b| b.is_ascii_hexdigit())
                && file.trim().trim_start_matches('*') == name)
                .then_some(digest)
        })
        .collect();
    anyhow::ensure!(found.len() == 1, "missing or invalid archive checksum");
    Ok(found[0].to_ascii_lowercase())
}

#[cfg(any(windows, test))]
fn zip_members(listing: &str) -> anyhow::Result<Vec<String>> {
    let mut names: Vec<String> = listing
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect();
    names.sort_unstable();
    anyhow::ensure!(
        names == ["LICENSE", "THIRD_PARTY_NOTICES.md", "gray.exe"],
        "update archive must contain exactly gray.exe, LICENSE and THIRD_PARTY_NOTICES.md"
    );
    Ok(names)
}

// bsdtar (System32\tar.exe, Windows 10+) reads zip directly; members are the
// install-native.ps1 allowlist, enforced exactly before extraction.
#[cfg(windows)]
fn extract_update_zip(archive: &Path, dest: &Path) -> anyhow::Result<()> {
    let out = Command::new("tar").arg("-tf").arg(archive).output()?;
    anyhow::ensure!(out.status.success(), "cannot list update archive");
    let members = zip_members(&String::from_utf8_lossy(&out.stdout))?;
    let mut total = 0u64;
    for member in members {
        run_step(
            Command::new("tar")
                .arg("-xf")
                .arg(archive)
                .arg("-C")
                .arg(dest)
                .arg(&member),
            true,
            "archive extraction",
        )?;
        total += std::fs::metadata(dest.join(member))?.len();
        anyhow::ensure!(
            total <= 256 * 1024 * 1024,
            "expanded archive exceeds 256 MiB"
        );
    }
    Ok(())
}

#[cfg(windows)]
fn windows_install(quiet: bool) -> anyhow::Result<()> {
    let base = base_url();
    let name = format!("gray-{CHANNEL}-x86_64-windows.zip");
    let tmp = tempfile::tempdir()?;
    let archive = tmp.path().join(&name);
    download_to(
        &format!("{base}/{name}"),
        &archive,
        quiet,
        "update download",
    )?;
    let want = match installer_pin() {
        Some(pin) => pin,
        None => {
            let sums = tmp.path().join("SHA256SUMS");
            download_to(
                &format!("{base}/SHA256SUMS-{CHANNEL}"),
                &sums,
                quiet,
                "checksum download",
            )?;
            sums_digest(&std::fs::read_to_string(&sums)?, &name)?
        }
    };
    let got = sha256_hex(&std::fs::read(&archive)?);
    anyhow::ensure!(
        got == want,
        "update checksum mismatch (expected {want}, got {got}) — refusing to install"
    );
    let stage = tmp.path().join("stage");
    std::fs::create_dir(&stage)?;
    extract_update_zip(&archive, &stage)?;
    anyhow::ensure!(
        binary_version(&stage.join("gray.exe")).is_some(),
        "downloaded gray.exe failed its --version probe"
    );
    let dest =
        installer_dest().ok_or_else(|| anyhow::anyhow!("no install dir — set GRAY_INSTALL_DIR"))?;
    std::fs::create_dir_all(&dest)?;
    for notice in ["LICENSE", "THIRD_PARTY_NOTICES.md"] {
        std::fs::copy(stage.join(notice), dest.join(notice))?;
    }
    // Stage inside dest so the final rename stays on one volume; a running
    // gray.exe can't be overwritten but can be renamed aside.
    let staged = dest.join(".gray-new.exe");
    std::fs::copy(stage.join("gray.exe"), &staged)?;
    let exe = dest.join("gray.exe");
    let old = dest.join("gray.exe.old");
    if exe.exists() {
        let _ = std::fs::remove_file(&old);
        std::fs::rename(&exe, &old)
            .map_err(|e| anyhow::anyhow!("cannot move the running gray.exe aside: {e}"))?;
        if let Err(e) = std::fs::rename(&staged, &exe) {
            let _ = std::fs::rename(&old, &exe);
            anyhow::bail!("cannot place the new gray.exe: {e}");
        }
        // Still locked while the old build runs; cleaned at next startup.
        let _ = std::fs::remove_file(&old);
    } else {
        std::fs::rename(&staged, &exe)?;
    }
    Ok(())
}

#[cfg(windows)]
fn clean_stale_update_files() {
    let Some(dest) = installer_dest() else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(&dest) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "old") || entry.file_name() == ".gray-new.exe" {
            let _ = std::fs::remove_file(&path);
        }
    }
}

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

fn first_gray_in(entries: &[PathBuf]) -> Option<PathBuf> {
    entries.iter().find_map(|d| {
        gray_names()
            .iter()
            .map(|name| d.join(name))
            .find(|c| is_launcher(c))
    })
}

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

fn first_gray_on_path() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let entries: Vec<PathBuf> = std::env::split_paths(&path).collect();
    first_gray_in(&entries)
}

fn cargo_shadow(path: &Path) -> bool {
    let Some(home) = std::env::var_os("HOME") else {
        return false;
    };
    path == PathBuf::from(home).join(".cargo/bin/gray")
}

fn drop_cargo_shadow(resolved: PathBuf, other: &Path) -> PathBuf {
    if cargo_shadow(&resolved)
        && !same_binary(&resolved, other)
        && std::fs::remove_file(&resolved).is_ok()
    {
        log::info!("removed stale cargo-install shadow {}", resolved.display());
        if let Some(next) = first_gray_on_path() {
            return next;
        }
        return resolved;
    }
    resolved
}

fn same_binary(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

fn shadow_warning(installed: &Path, installed_version: &str, resolved: &Path) -> Option<String> {
    if same_binary(resolved, installed) {
        return None;
    }
    let rm = if cfg!(windows) { "del" } else { "rm" };
    Some(format!(
        "⚠ {} comes first on PATH and shadows the updated {} (gray {installed_version}) — new shells keep launching the old build\n  fix:  {rm} {}   (then open a new shell)",
        resolved.display(),
        installed.display(),
        resolved.display(),
    ))
}

fn post_update_shadow_warning() -> Option<String> {
    let bin = if cfg!(windows) { "gray.exe" } else { "gray" };
    let installed = installer_dest()?.join(bin);

    let installed_version = binary_version(&installed)?;
    let resolved = drop_cargo_shadow(first_gray_on_path()?, &installed);
    shadow_warning(&installed, &installed_version, &resolved)
}

pub(crate) fn warn_on_shadow() {
    if let Some(w) = post_update_shadow_warning() {
        eprintln!("{w}");
    }
}

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

fn path_divergence_warning() -> Option<String> {
    let current = std::env::current_exe().ok()?;
    let resolved = drop_cargo_shadow(first_gray_on_path()?, &current);
    divergence_warning(&current, env!("CARGO_PKG_VERSION"), &resolved)
}

fn warn_on_divergence() {
    if let Some(w) = path_divergence_warning() {
        eprintln!("{w}");
    }
}

pub async fn update_now() -> anyhow::Result<()> {
    match latest_version().await {
        Ok(latest)
            if !latest.is_empty()
                && !update_available(
                    CHANNEL,
                    &latest,
                    env!("CARGO_PKG_VERSION"),
                    env!("GRAY_BUILD_ID"),
                ) =>
        {
            println!(
                "gray {} is current ({CHANNEL}, latest {latest})",
                env!("CARGO_PKG_VERSION")
            );
            warn_on_shadow();
            return Ok(());
        }
        Ok(_) => {}
        Err(e) => log::debug!("version check failed ({e:#}); installing anyway"),
    }
    println!(
        "{} updating gray ({CHANNEL})...",
        crate::tui::glyph("→", ">")
    );
    install()?;
    println!(
        "{} updated. restart gray to use the new version.",
        crate::tui::glyph("✓", "v")
    );
    warn_on_shadow();
    Ok(())
}

pub(crate) fn install_unlocked() -> anyhow::Result<()> {
    run_installer_with(true)
}

pub(crate) fn install() -> anyhow::Result<()> {
    anyhow::ensure!(
        !cfg!(debug_assertions),
        "this is a debug build — self-update would overwrite the installed gray with the release build; use scripts/install-local.sh instead"
    );
    run_installer_locked()
}

const CHECK_INTERVAL_SECS: u64 = 24 * 3600;

fn update_check_due(last_check_secs: Option<u64>, now_secs: u64) -> bool {
    match last_check_secs {
        None => true,

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

fn auto_update_allowed(channel: &str, flag: Option<&str>) -> bool {
    channel == "stable" && flag == Some("1")
}

pub async fn startup_check() {
    #[cfg(windows)]
    clean_stale_update_files();
    let current = env!("CARGO_PKG_VERSION");
    if std::env::var("GRAY_NO_UPDATE_CHECK").as_deref() == Ok("1") || crate::config::offline() {
        return;
    }
    if cfg!(debug_assertions) || current == "0.0.0" {
        return;
    }
    let now = gray_core::spill::now_secs() as u64;
    if !update_check_due(read_last_check(), now) {
        return;
    }
    write_last_check(now);

    warn_on_divergence();
    let Ok(Ok(latest)) =
        tokio::time::timeout(std::time::Duration::from_millis(1500), latest_version()).await
    else {
        return;
    };
    if !update_available(CHANNEL, &latest, current, env!("GRAY_BUILD_ID")) {
        return;
    }
    let auto_flag = std::env::var("GRAY_AUTO_UPDATE").ok();
    if auto_update_allowed(CHANNEL, auto_flag.as_deref()) {
        let latest = latest.clone();
        tokio::task::spawn_blocking(move || {
            let Ok(_lock) = acquire_update_lock() else {
                return;
            };
            let ok = install_unlocked().is_ok();
            if ok {
                crate::profile::queue_profile_warning(format!(
                    "gray {latest} installed in the background — restart to apply"
                ));

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
                println!(
                    "{} updated. restart gray to use {latest}.",
                    crate::tui::glyph("✓", "v")
                );
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
