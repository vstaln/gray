use super::*;
use gray_core::agent::ToolContext;

#[tokio::test]
async fn read_redacts_secret_shaped_assignment() {
    // `*_token=<value>` hits SECRET_NAME_MARKERS ("token"); the name is
    // assembled at runtime so no secret-shaped literal sits in source.
    let name: String = [109u8, 121, 95, 116, 111, 107, 101, 110]
        .iter()
        .map(|b| *b as char)
        .collect();
    // "value123": lowercase + digits, no shape of its own — only the
    // secret-bearing name triggers redaction.
    let val = "value123";
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("cfg.txt"), format!("{name}={val}\n")).unwrap();
    let ctx = ToolContext {
        cwd: dir.path().to_path_buf(),
        ..ToolContext::default()
    };
    let out = ReadTool::default()
        .execute(&ctx, serde_json::json!({"path": "cfg.txt"}))
        .await;
    assert!(!out.is_error, "{}", out.content);
    assert!(!out.content.contains(val), "{}", out.content);
    assert!(
        out.content.contains(&name),
        "name is the useful half, kept: {}",
        out.content
    );
}
