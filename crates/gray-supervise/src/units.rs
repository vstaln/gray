//! Service units: hardened systemd user unit + macOS launchd plist. No IO here.
use std::path::Path;

/// Paths are operator-controlled, but they are interpolated as unit syntax
/// (systemd) and XML (launchd): quote the XML, and refuse non-plain paths
/// for systemd rather than emitting directives from a path.
fn xml_text(path: &Path) -> String {
    path.to_string_lossy()
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Absolute UTF-8 paths without spaces, newlines, `%` specifiers, or shell
/// metacharacters: safe to interpolate into a unit file verbatim.
fn plain_unit_path(path: &Path) -> bool {
    path.is_absolute()
        && path.to_str().is_some_and(|s| {
            s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"/_-.".contains(&b))
        })
}

/// Hardened user unit: always restart, no start-limit stall, 90s stop budget
/// (30s drain + headroom), exit-code contract, linger-friendly target.
/// (`StartLimitIntervalSec` lives in `[Unit]`; in `[Service]` systemd
/// ignores it.)
pub fn generate_systemd_unit(gray_bin: &Path, gray_home: &Path) -> String {
    if !plain_unit_path(gray_bin) || !plain_unit_path(gray_home) {
        // Explicitly inert rather than emitting a path as service syntax.
        return "[Unit]\nDescription=Gray: unsupported service path\nConditionPathExists=/dev/null/gray-invalid-path\n"
            .into();
    }
    format!(
        "[Unit]\nDescription=Gray Gateway\nAfter=network.target\nStartLimitIntervalSec=0\n\n[Service]\nExecStart={} gateway run\nRestart=always\nRestartSec=5\nTimeoutStopSec=90\nRestartForceExitStatus={}\nRestartPreventExitStatus={}\nEnvironment=GRAY_HOME={}\n\n[Install]\nWantedBy=default.target\n",
        gray_bin.display(),
        crate::exit::EXIT_RESTART,
        crate::exit::EXIT_FATAL,
        gray_home.display()
    )
}

/// macOS agent plist: `~/Library/LaunchAgents/ai.gray.gateway.plist`.
pub fn generate_launchd_plist(gray_bin: &Path, gray_home: &Path) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n\t<key>Label</key><string>ai.gray.gateway</string>\n\t<key>ProgramArguments</key><array><string>{}</string><string>gateway</string><string>run</string></array>\n\t<key>EnvironmentVariables</key><dict><key>GRAY_HOME</key><string>{}</string></dict>\n\t<key>RunAtLoad</key><true/>\n\t<key>KeepAlive</key><true/>\n\t<key>ThrottleInterval</key><integer>30</integer>\n\t<key>StandardOutPath</key><string>{}/logs/gateway.out.log</string>\n\t<key>StandardErrorPath</key><string>{}/logs/gateway.err.log</string>\n</dict>\n</plist>\n",
        xml_text(gray_bin),
        xml_text(gray_home),
        xml_text(&gray_home.join("logs/gateway.out.log")),
        xml_text(&gray_home.join("logs/gateway.err.log"))
    )
}

/// Human hint when systemd lingering is off (checked by the caller via loginctl).
pub fn linger_hint() -> &'static str {
    "tip: run `loginctl enable-linger $USER` so the gateway survives logout"
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    #[test]
    fn systemd_unit_has_restart_contract_and_drain() {
        let u = generate_systemd_unit(
            Path::new("/home/u/.local/bin/gray"),
            Path::new("/home/u/.gray"),
        );
        assert!(
            u.contains("ExecStart=/home/u/.local/bin/gray gateway run"),
            "got:\n{u}"
        );
        assert!(u.contains("Restart=always"));
        assert!(u.contains("RestartSec=5"));
        assert!(u.contains("StartLimitIntervalSec=0"));
        assert!(u.contains("TimeoutStopSec=90"));
        assert!(u.contains("RestartForceExitStatus=75"));
        assert!(u.contains("RestartPreventExitStatus=78"));
        assert!(u.contains("Environment=GRAY_HOME=/home/u/.gray"));
    }
    #[test]
    fn launchd_plist_keeps_alive_and_runs_at_load() {
        let p = generate_launchd_plist(
            Path::new("/opt/homebrew/bin/gray"),
            Path::new("/Users/u/.gray"),
        );
        assert!(p.contains("RunAtLoad"));
        assert!(p.contains("<true/>"));
        assert!(p.contains("KeepAlive"));
        assert!(p.contains("/opt/homebrew/bin/gray"));
    }
    #[test]
    fn hostile_paths_never_become_syntax() {
        let evil_bin = Path::new("/tmp/my gray/bin");
        let evil_home = Path::new("/home/u/.gray\nInjected=1");
        let u = generate_systemd_unit(evil_bin, Path::new("/home/u/.gray"));
        assert!(
            u.contains("unsupported service path"),
            "spaced binary must yield inert unit, got:\n{u}"
        );
        assert!(!u.contains("ExecStart="));
        let u = generate_systemd_unit(Path::new("/usr/bin/gray"), evil_home);
        assert!(u.contains("unsupported service path"));
        let p = generate_launchd_plist(
            Path::new("/bin/gray&a"),
            Path::new("/h<m>e"),
        );
        assert!(p.contains("/bin/gray&amp;a"), "got:\n{p}");
        assert!(p.contains("/h&lt;m&gt;e"), "got:\n{p}");
    }
}
