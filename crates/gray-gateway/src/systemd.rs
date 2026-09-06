use std::path::{Path, PathBuf};
pub fn systemd_unit_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(home).join(".config/systemd/user/gray-gateway.service")
}
pub fn generate_unit(gray_bin: &Path) -> String {
    let gray_home = crate::config::gray_home_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| format!("{}/.gray", std::env::var("HOME").unwrap_or_default()));
    gray_supervise::units::generate_systemd_unit(gray_bin, Path::new(&gray_home))
}
pub fn install() -> anyhow::Result<()> {
    let bin = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("gray"));
    let path = systemd_unit_path();
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p)?;
    }
    if path.exists() && !path.with_extension("service.bak").exists() {
        let _ = std::fs::copy(&path, path.with_extension("service.bak"));
    }
    std::fs::write(&path, generate_unit(&bin))?;
    let _ = std::process::Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .status();
    let _ = std::process::Command::new("systemctl")
        .args(["--user", "enable", "--now", "gray-gateway.service"])
        .status();
    println!("installed {}", path.display());
    if !std::process::Command::new("loginctl")
        .args([
            "show-user",
            &std::env::var("USER").unwrap_or_default(),
            "-p",
            "Linger",
        ])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains("Linger=yes"))
        .unwrap_or(true)
    {
        println!("{}", gray_supervise::units::linger_hint());
    }
    Ok(())
}
pub fn uninstall() -> anyhow::Result<()> {
    let _ = std::process::Command::new("systemctl")
        .args(["--user", "disable", "--now", "gray-gateway.service"])
        .status();
    let path = systemd_unit_path();
    let _ = std::fs::remove_file(&path);
    println!("uninstalled gray-gateway");
    Ok(())
}
pub fn status() -> anyhow::Result<()> {
    status_with_output(|| {
        std::process::Command::new("systemctl")
            .args(["--user", "is-active", "gray-gateway.service"])
            .output()
    })
}

pub(crate) fn status_with_output(
    run: impl FnOnce() -> std::io::Result<std::process::Output>,
) -> anyhow::Result<()> {
    let out = run().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            anyhow::anyhow!(
                "systemctl not found — gateway service status unavailable (is systemd installed?)"
            )
        } else {
            anyhow::anyhow!("gateway service status unavailable: {e}")
        }
    })?;
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&out.stderr);
    if stdout == "unknown"
        || stdout == "not-found"
        || stderr.contains("could not be found")
        || stderr.contains("No such file")
    {
        anyhow::bail!("service gray-gateway.service not installed");
    }
    println!("gray-gateway: {stdout}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_systemctl_names_the_missing_thing() {
        let err = status_with_output(|| {
            Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "No such file or directory (os error 2)",
            ))
        })
        .unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("systemctl not found"),
            "must name systemctl, got: {msg}"
        );
        assert!(
            !msg.contains("os error 2"),
            "must not leak bare os error, got: {msg}"
        );
    }

    #[test]
    fn absent_service_names_the_service() {
        use std::os::unix::process::ExitStatusExt;
        let out = std::process::Output {
            status: std::process::ExitStatus::from_raw(3 << 8),
            stdout: b"unknown\n".to_vec(),
            stderr: b"Unit gray-gateway.service could not be found.\n".to_vec(),
        };
        let err = status_with_output(|| Ok(out)).unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("gray-gateway.service not installed"),
            "must name service, got: {msg}"
        );
    }

    #[test]
    fn active_service_prints_status() {
        use std::os::unix::process::ExitStatusExt;
        let out = std::process::Output {
            status: std::process::ExitStatus::from_raw(0),
            stdout: b"active\n".to_vec(),
            stderr: Vec::new(),
        };
        assert!(status_with_output(|| Ok(out)).is_ok());
    }
}
