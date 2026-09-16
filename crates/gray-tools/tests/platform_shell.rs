//! Native shell release gate, run unchanged on Unix and Windows + Git Bash.
//!
//! Exit text alone is insufficient: the old Windows signal stub reports a
//! timeout while leaving descendants alive. A delayed write proves whether
//! cancellation actually stopped the tree. Fixtures expire on their own so a
//! failing implementation cannot leave a permanent process on the CI runner.

use std::time::Duration;

use gray_core::agent::{Tool, ToolContext};
use gray_tools::BashTool;
use serde_json::json;

#[tokio::test]
async fn shell_preserves_command_cwd_and_nonzero_exit() {
    // Deliberately exercise a native cwd containing spaces and Unicode. Pass
    // cwd through the process API, not shell interpolation or slash rewriting.
    let dir = tempfile::Builder::new()
        .prefix("gray shell café ")
        .tempdir()
        .unwrap();
    let ctx = ToolContext {
        cwd: dir.path().to_path_buf(),
        ..ToolContext::default()
    };
    let out = tokio::time::timeout(
        Duration::from_secs(15),
        BashTool.execute(
            &ctx,
            json!({"command": "printf 'exact bytes' > result.txt; printf 'hello'; exit 7"}),
        ),
    )
    .await
    .expect("shell execution must be bounded");
    assert!(!out.is_error, "{}", out.content);
    assert!(out.content.starts_with("exit 7"), "{}", out.content);
    assert!(out.content.contains("hello"), "{}", out.content);
    assert_eq!(
        std::fs::read(dir.path().join("result.txt")).unwrap(),
        b"exact bytes"
    );
}

async fn stopped_tree_cannot_write_later(cancel: bool) {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ToolContext {
        cwd: dir.path().to_path_buf(),
        ..ToolContext::default()
    };
    let token = ctx.cancel.clone();
    let cancellation = tokio::spawn(async move {
        if cancel {
            tokio::time::sleep(Duration::from_secs(1)).await;
            token.cancel();
        }
    });
    let out = tokio::time::timeout(
        Duration::from_secs(12),
        BashTool.execute(
            &ctx,
            json!({
                // Parent waits for a background subshell and its sleep child.
                // Killing only the top-level shell must fail this regression.
                "command": "printf 'ready\\n'; (sleep 4; printf escaped > escaped.txt) & wait",
                "timeout": if cancel { 10 } else { 1 }
            }),
        ),
    )
    .await
    .expect("timeout/cancellation must not hang waiting for surviving children");
    cancellation.await.unwrap();
    assert!(!out.is_error, "{}", out.content);
    assert!(
        out.content.contains("ready"),
        "partial output lost: {}",
        out.content
    );
    assert!(
        out.content
            .starts_with(if cancel { "cancelled" } else { "timed out" }),
        "{}",
        out.content
    );
    // Observe the side effect after the fixture's deadline even if the tool
    // returned quickly. Otherwise a root-only kill could look successful.
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert!(
        !dir.path().join("escaped.txt").exists(),
        "descendant survived {} and wrote after termination: {}",
        if cancel { "cancellation" } else { "timeout" },
        out.content
    );
}

#[tokio::test]
async fn timeout_stops_descendants() {
    stopped_tree_cannot_write_later(false).await;
}

#[tokio::test]
async fn cancellation_stops_descendants() {
    stopped_tree_cannot_write_later(true).await;
}
