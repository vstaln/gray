use super::*;
use gray_core::agent::ToolContext;

#[tokio::test]
async fn huge_line_count_uses_count_skipped_wording() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("huge.txt"), "x\n".repeat(150_000)).unwrap();
    let ctx = ToolContext {
        cwd: dir.path().to_path_buf(),
        ..ToolContext::default()
    };
    let out = ReadTool::default()
        .execute(&ctx, serde_json::json!({"path": "huge.txt", "limit": 10}))
        .await;
    assert!(!out.is_error, "{}", out.content);
    assert!(
        out.content.contains("count skipped"),
        "{}",
        tail(&out.content)
    );
    assert!(out.content.contains("offset=11"), "{}", tail(&out.content));
    assert!(!out.content.contains("of 150000"), "{}", tail(&out.content));
}

fn tail(s: &str) -> &str {
    &s[s.len().saturating_sub(500)..]
}
