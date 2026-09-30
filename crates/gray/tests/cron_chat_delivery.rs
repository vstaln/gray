//! A job added from a chat comes back to that chat: the host declares the
//! origin in the environment, and `gray cron tick --json` hands the frame
//! back for routing. Core renders; the platform carries.

use std::path::Path;
use std::process::{Command, Output};

fn run(home: &Path, env_origin: Option<&str>, args: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_gray"));
    cmd.env("GRAY_HOME", home).current_dir(home).args(args);
    match env_origin {
        Some(v) => {
            cmd.env("GRAY_CRON_ORIGIN", v);
        }
        None => {
            cmd.env_remove("GRAY_CRON_ORIGIN");
        }
    }
    cmd.output().unwrap()
}

fn ok(home: &Path, env_origin: Option<&str>, args: &[&str]) -> String {
    let out = run(home, env_origin, args);
    assert!(
        out.status.success(),
        "{:?}: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

fn home() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("cron")).unwrap();
    tmp
}

#[test]
fn a_host_origin_binds_the_job_without_the_model_knowing_ids() {
    let tmp = home();
    let h = tmp.path();
    let origin = r#"{"platform":"discord","chat":"4f3c2b1a","route":"1234567890"}"#;
    ok(
        h,
        Some(origin),
        &["cron", "add", "every 1h", "check the deploy"],
    );
    let shown = ok(h, None, &["cron", "list"]);
    let id = shown
        .lines()
        .find(|l| l.contains("check the deploy"))
        .and_then(|l| l.split_whitespace().next())
        .unwrap()
        .to_string();
    let record = ok(h, None, &["cron", "show", &id]);
    assert!(record.contains("deliver: origin"), "{record}");
    assert!(record.contains("origin: discord:4f3c2b1a"), "{record}");
    assert!(record.contains("route:1234567890"), "{record}");
}

#[test]
fn an_explicit_flag_beats_the_environment() {
    let tmp = home();
    let h = tmp.path();
    let origin = r#"{"platform":"discord","chat":"4f3c2b1a","route":"1234567890"}"#;
    ok(
        h,
        Some(origin),
        &["cron", "add", "every 1h", "local one", "--deliver", "local"],
    );
    let listed = ok(h, None, &["cron", "list"]);
    assert!(
        !listed.contains("local one")
            || !ok(h, None, &["cron", "show", "local one"]).contains("deliver: origin"),
        "an explicit --deliver local must not be overridden by the host origin"
    );
}

#[test]
fn without_a_host_origin_a_job_is_just_local() {
    let tmp = home();
    let h = tmp.path();
    ok(h, None, &["cron", "add", "every 1h", "plain job"]);
    let record = ok(h, None, &["cron", "show", "plain job"]);
    assert!(record.contains("deliver: local"), "plain -> {record}");
    assert!(!record.contains("origin:"), "plain -> {record}");
}

#[test]
fn a_junk_host_origin_is_not_a_declaration() {
    let tmp = home();
    let h = tmp.path();
    for (n, junk) in [
        "",
        "not json",
        "{}",
        r#"{"platform":"discord"}"#,
        r#"{"chat":"x"}"#,
    ]
    .iter()
    .enumerate()
    {
        let name = format!("junkjob{n}");
        ok(h, Some(junk), &["cron", "add", "every 1h", &name]);
        let record = ok(h, None, &["cron", "show", &name]);
        assert!(
            record.contains("deliver: local"),
            "junk {junk:?} -> {record}"
        );
        assert!(!record.contains("origin:"), "junk {junk:?} -> {record}");
    }
}

#[test]
fn tick_json_reports_the_tick_even_with_nothing_due() {
    let tmp = home();
    let h = tmp.path();
    // Native Windows has no cron execution host by design (WSL instead):
    // assert the refusal, not the tick.
    #[cfg(windows)]
    {
        let out = run(h, None, &["cron", "tick", "--json"]);
        assert!(!out.status.success());
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("WSL"),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    #[cfg(not(windows))]
    {
        let out = ok(h, None, &["cron", "tick", "--json"]);
        let last: serde_json::Value = out
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .next_back()
            .expect("a tick summary line");
        assert_eq!(last["type"], "cron_tick");
        assert_eq!(last["fired"], 0);
    }
}
