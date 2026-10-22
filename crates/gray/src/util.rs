use std::fs::OpenOptions;
use std::path::Path;

pub fn now_secs() -> i64 {
    gray_core::spill::now_secs()
}

pub fn atomic_write_json(path: &Path, value: &impl serde::Serialize) -> anyhow::Result<()> {
    use std::io::Write as _;
    let body = serde_json::to_string_pretty(value)?;
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;

    let tmp = parent.join(format!(".gray-{}.tmp", uuid::Uuid::new_v4()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| -> anyhow::Result<()> {
        let mut file = options.open(&tmp)?;
        file.write_all(body.as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&tmp, path)?;
        #[cfg(unix)]
        std::fs::File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}


#[path = "util_tests.rs"]
#[cfg(test)]
mod tests;

/// Open `url` in the system browser, best-effort (device-flow OAuth
/// verification pages; was `feedback::open_in_browser`).
pub fn open_in_browser(url: &str) {
    let spawn = |prog: &str, args: &[&str]| {
        std::process::Command::new(prog).args(args).spawn().is_ok()
    };
    if cfg!(target_os = "macos") {
        let _ = spawn("open", &[url]);
    } else if cfg!(target_os = "windows") {
        let _ = spawn("cmd", &["/c", "start", url]);
    } else {
        let _ = spawn("xdg-open", &[url]);
    }
}
