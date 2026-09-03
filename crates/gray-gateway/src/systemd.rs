use std::path::PathBuf;
pub fn systemd_unit_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(home).join(".config/systemd/user/gray-gateway.service")
}
pub fn generate_unit(gray_bin: &PathBuf) -> String {
    let gray_home = crate::config::gray_home_dir().map(|p| p.display().to_string()).unwrap_or_else(|_| format!("{}/.gray", std::env::var("HOME").unwrap_or_default()));
    format!("[Unit]\nDescription=Gray Gateway\nAfter=network.target\n\n[Service]\nExecStart={} gateway run\nRestart=always\nRestartSec=5\nEnvironment=GRAY_HOME={}\n\n[Install]\nWantedBy=default.target\n", gray_bin.display(), gray_home)
}
pub fn install() -> anyhow::Result<()> {
    let bin = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("gray"));
    let path = systemd_unit_path();
    if let Some(p) = path.parent() { std::fs::create_dir_all(p)?; }
    std::fs::write(&path, generate_unit(&bin))?;
    let _ = std::process::Command::new("systemctl").args(["--user","daemon-reload"]).status();
    let _ = std::process::Command::new("systemctl").args(["--user","enable","--now","gray-gateway.service"]).status();
    println!("installed {}", path.display());
    Ok(())
}
pub fn uninstall() -> anyhow::Result<()> {
    let _ = std::process::Command::new("systemctl").args(["--user","disable","--now","gray-gateway.service"]).status();
    let path = systemd_unit_path();
    let _ = std::fs::remove_file(&path);
    println!("uninstalled gray-gateway");
    Ok(())
}
pub fn status() -> anyhow::Result<()> {
    let out = std::process::Command::new("systemctl").args(["--user","is-active","gray-gateway.service"]).output()?;
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    println!("gray-gateway: {s}");
    Ok(())
}
