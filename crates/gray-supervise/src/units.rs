//! Service units: hardened systemd user unit. No IO here.
use std::path::Path;

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
    }
}
