//! WP0 baseline: records today's `BashTool` output per shell fixture into
//! `tests/snapshots/before/<name>.txt`.
//!
//! These tests record behaviour only — no assertions beyond "did not panic".
//! Later WPs diff `before/` against `after/`. Do not add assertions here.

use std::path::PathBuf;

use gray_core::agent::ToolContext;
use gray_tools::{BashTool, Tool};
use serde_json::json;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/shell")
        .join(name)
}

async fn run_and_snapshot(name: &str, command: String, timeout: Option<u64>) {
    let args = match timeout {
        Some(t) => json!({"command": command.clone(), "timeout": t}),
        None => json!({"command": command.clone()}),
    };
    let out = BashTool.execute(&ToolContext::default(), args).await;
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots/before");
    std::fs::create_dir_all(&dir).expect("create tests/snapshots/before");
    let body = format!(
        "is_error: {}\ncommand: {command}\n---\n{}",
        out.is_error, out.content
    );
    std::fs::write(dir.join(format!("{name}.txt")), body).expect("write snapshot");
}

#[tokio::test]
async fn baseline_burst() {
    let cmd = format!("sh {}", fixture("burst.sh").display());
    run_and_snapshot("burst", cmd, None).await;
}

#[tokio::test]
async fn baseline_wide() {
    let cmd = format!("sh {}", fixture("wide.sh").display());
    run_and_snapshot("wide", cmd, None).await;
}

#[tokio::test]
async fn baseline_slow() {
    let cmd = format!("sh {}", fixture("slow.sh").display());
    run_and_snapshot("slow", cmd, Some(10)).await;
}

#[tokio::test]
async fn baseline_oom() {
    let cmd = format!("sh {}", fixture("oom.sh").display());
    run_and_snapshot("oom", cmd, None).await;
}

#[tokio::test]
async fn baseline_server() {
    let cmd = format!("PORT=18080 sh {}", fixture("server.sh").display());
    run_and_snapshot("server", cmd, Some(10)).await;
}

#[tokio::test]
async fn baseline_silent() {
    let cmd = format!("sh {}", fixture("silent.sh").display());
    run_and_snapshot("silent", cmd, Some(10)).await;
}

#[tokio::test]
async fn baseline_grep_miss() {
    let cmd = format!(
        "grep zzz_no_such_match_xyz {}",
        fixture("silent.sh").display()
    );
    run_and_snapshot("grep-miss", cmd, None).await;
}
