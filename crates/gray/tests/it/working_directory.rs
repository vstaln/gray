//! Exercise the actual CLI/provider boundary, without a real API key or model.
//! The first request carries the saved prompt verbatim (comments stripped)
//! and no runtime context, launched from a non-ASCII, spaced directory.
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test]
async fn first_request_sends_saved_prompt_verbatim_without_changing_it() {
    check_first_request(false).await;
}

#[cfg(windows)]
#[tokio::test]
async fn native_binary_uses_profile_without_home_override() {
    check_first_request(true).await;
}

async fn check_first_request(native_profile: bool) {
    let root = tempfile::tempdir().unwrap();
    let cwd = root.path().join("project café with spaces");
    let profile = root.path().join("profile café with spaces");
    let home = if native_profile {
        profile.join(".gray")
    } else {
        root.path().join("home")
    };
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    let saved = "Custom instructions.\n<!-- private editor note -->\n";
    std::fs::write(home.join("AGENTS.md"), saved).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut bytes = Vec::new();
        let (header_end, length) = loop {
            let mut chunk = [0; 4096];
            let n = stream.read(&mut chunk).await.unwrap();
            assert!(n > 0, "request ended before headers");
            bytes.extend_from_slice(&chunk[..n]);
            if let Some(i) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&bytes[..i]).to_lowercase();
                let length = headers
                    .lines()
                    .find_map(|l| l.strip_prefix("content-length:"))
                    .unwrap()
                    .trim()
                    .parse::<usize>()
                    .unwrap();
                break (i + 4, length);
            }
        };
        while bytes.len() < header_end + length {
            let mut chunk = [0; 4096];
            let n = stream.read(&mut chunk).await.unwrap();
            assert!(n > 0, "request ended before body");
            bytes.extend_from_slice(&chunk[..n]);
        }
        let request: serde_json::Value =
            serde_json::from_slice(&bytes[header_end..header_end + length]).unwrap();
        let body = "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"ok\"},\"finish_reason\":null}]}\n\ndata: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).await.unwrap();
        request
    });
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_gray"));
    command
        .current_dir(&cwd)
        .env("GRAY_HOME", &home)
        .env("HOME", &home)
        .env("USERPROFILE", &home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("GRAY_NO_UPDATE_CHECK", "1")
        .args([
            "--model",
            "cwd-test",
            "--api-key",
            "test-only",
            "--base-url",
            &url,
            "--context-window",
            "128000",
            "-p",
            "hello",
        ])
        .kill_on_drop(true);
    if native_profile {
        command
            .env_remove("HOME")
            .env_remove("GRAY_HOME")
            .env("USERPROFILE", &profile);
    }
    let output = tokio::time::timeout(Duration::from_secs(30), command.output())
        .await
        .expect("CLI must finish")
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let request = tokio::time::timeout(Duration::from_secs(2), server)
        .await
        .unwrap()
        .unwrap();
    let system = request["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["role"] == "system")
        .unwrap()["content"]
        .as_str()
        .unwrap();
    // The saved prompt verbatim minus comments; no runtime context (memory,
    // a separate feature, may follow).
    assert!(system.starts_with("Custom instructions."), "{system}");
    assert!(!system.contains("private editor note"));
    assert!(!system.contains("Working directory"), "{system}");
    assert_eq!(
        std::fs::read_to_string(home.join("AGENTS.md")).unwrap(),
        saved
    );
}
