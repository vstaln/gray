use super::*;

#[test]
fn shell_quote_survives_apostrophes() {
    assert_eq!(shell_quote("/a/b"), "'/a/b'");
    assert_eq!(shell_quote("/it's"), r"'/it'\''s'");
}

#[test]
fn runit_run_script_exports_home_and_execs_the_gateway() {
    let body = runit_run_script(Path::new("/tmp/it's home"), Path::new("/usr/bin/gray"));
    assert!(
        body.contains(r"export GRAY_HOME='/tmp/it'\''s home'"),
        "{body}"
    );
    assert!(body.contains("exec '/usr/bin/gray' gateway run"), "{body}");
    assert!(body.starts_with("#!/bin/sh"), "{body}");
}

#[test]
fn runit_log_script_points_svlogd_at_the_home() {
    let body = runit_log_script(Path::new("/tmp/gh"));
    assert!(body.contains("mkdir -p '/tmp/gh/logs/gateway'"), "{body}");
    assert!(
        body.contains("exec svlogd -tt '/tmp/gh/logs/gateway'"),
        "{body}"
    );
}

#[test]
fn systemd_unit_restarts_on_failure_and_waits_out_drains() {
    let body = systemd_unit_body(Path::new("/tmp/gh"), Path::new("/usr/bin/gray"));
    assert!(
        body.contains("ExecStart=\"/usr/bin/gray\" gateway run"),
        "{body}"
    );
    assert!(body.contains("Environment=GRAY_HOME=/tmp/gh"), "{body}");
    assert!(body.contains("Restart=always"), "{body}");
    assert!(body.contains("TimeoutStopSec=75"), "{body}");
}

#[test]
fn pid_parser_reads_runit_status_lines() {
    assert_eq!(
        parse_pid("run: gray-gateway: (pid 1234) 42s; run: log: (pid 1235) 42s"),
        Some(1234)
    );
    assert_eq!(parse_pid("down: gray-gateway: 1s, normally up"), None);
}
#[test]
fn linger_warning_fires_only_without_linger_yes() {
    assert_eq!(linger_warning_for("Linger=yes\n"), None);
    assert!(linger_warning_for("Linger=no\n").is_some());
    assert!(linger_warning_for("").is_some());
}
