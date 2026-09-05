//! Task 2.2: fetch integration tests — hash verify, https policy,
//! tar traversal guard, 64 MiB cap. Hermetic: loopback servers only.

// Deliberate: the env lock must span each whole test (env is read inside
// the awaited calls), so holding a std guard across await is the point.
#![allow(clippy::await_holding_lock)]

use std::sync::Mutex;

use axum::{Router, routing::get};
use sha2::{Digest, Sha256};

static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Point `GRAY_HOME` at a fresh tempdir (isolates `plugins/tmp`).
/// Returns the tempdir (must stay alive for the test's duration).
fn use_env() -> tempfile::TempDir {
    let home = tempfile::tempdir().unwrap();
    // SAFETY: under ENV_LOCK; no other test thread touches env concurrently.
    unsafe {
        std::env::set_var("GRAY_HOME", home.path());
    }
    home
}

fn tmp_dir(home: &tempfile::TempDir) -> std::path::PathBuf {
    home.path().join("plugins/tmp")
}

fn tmp_is_empty(home: &tempfile::TempDir) -> bool {
    let dir = tmp_dir(home);
    !dir.exists() || std::fs::read_dir(&dir).unwrap().count() == 0
}

async fn spawn_bytes(body: Vec<u8>) -> String {
    let router = Router::new().route(
        "/x.tar.gz",
        get(move || {
            let body = body.clone();
            async move { body }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    format!("http://127.0.0.1:{port}/x.tar.gz")
}

/// Raw-HTTP server streaming `total` zero bytes (avoids buffering 64 MiB
/// in axum and needs no extra stream deps). `download` enforces its cap by
/// counting streamed chunks, so this must exceed `MAX_BYTES` for real.
async fn spawn_big_server(total: u64) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        // Consume request headers first.
        let mut buf = vec![0u8; 4096];
        let mut seen = Vec::new();
        loop {
            match sock.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    seen.extend_from_slice(&buf[..n]);
                    if seen.windows(4).any(|w| w == b"\r\n\r\n") || seen.len() > 65536 {
                        break;
                    }
                }
            }
        }
        let header =
            format!("HTTP/1.1 200 OK\r\ncontent-length: {total}\r\nconnection: close\r\n\r\n");
        if sock.write_all(header.as_bytes()).await.is_err() {
            return;
        }
        let chunk = vec![0u8; 1024 * 1024];
        let mut left = total;
        while left > 0 {
            let n = left.min(chunk.len() as u64) as usize;
            // The client bails after the cap; a broken pipe then is expected.
            if sock.write_all(&chunk[..n]).await.is_err() {
                break;
            }
            left -= n as u64;
        }
    });
    format!("http://127.0.0.1:{port}/big.tar.gz")
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    format!("sha256:{:x}", h.finalize())
}

#[tokio::test]
async fn hash_mismatch_errors_and_cleans_up() {
    let _lock = ENV_LOCK.lock().unwrap();
    let home = use_env();
    let url = spawn_bytes(b"hello plugin".to_vec()).await;

    let client = gray_pkg::fetch::client().unwrap();
    let wrong = gray_pkg::index::HashSpec::Single(format!("sha256:{}", "0".repeat(64)));
    let err = gray_pkg::fetch::download(&client, &url, Some(&wrong))
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("hash mismatch"), "unexpected error: {err}");
    assert!(tmp_is_empty(&home), "temp file must be removed on failure");
}

#[tokio::test]
async fn non_loopback_http_rejected() {
    let _lock = ENV_LOCK.lock().unwrap();
    let _home = use_env();

    let client = gray_pkg::fetch::client().unwrap();
    let err = gray_pkg::fetch::download(&client, "http://example.com/x.tar.gz", None)
        .await
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("refusing non-https"),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn loopback_http_allowed_with_correct_hash() {
    let _lock = ENV_LOCK.lock().unwrap();
    let home = use_env();
    let body = b"plugin bytes".to_vec();
    let url = spawn_bytes(body.clone()).await;

    let client = gray_pkg::fetch::client().unwrap();
    let want = gray_pkg::index::HashSpec::Single(sha256_hex(&body));
    let path = gray_pkg::fetch::download(&client, &url, Some(&want))
        .await
        .unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), body);
    assert!(path.starts_with(tmp_dir(&home)));
    std::fs::remove_file(&path).unwrap();
}

/// Hand-crafted ustar (the tar builder itself refuses `..` at write time).
fn evil_tar_gz(name: &str) -> Vec<u8> {
    use std::io::Write;
    let mut hdr = [0u8; 512];
    hdr[..name.len()].copy_from_slice(name.as_bytes());
    hdr[100..108].copy_from_slice(b"0000777\0");
    hdr[124..136].copy_from_slice(b"00000000004\0");
    hdr[148..156].copy_from_slice(b"        ");
    hdr[156] = b'0';
    hdr[257..262].copy_from_slice(b"ustar");
    hdr[263..265].copy_from_slice(b"00");
    let sum: u32 = hdr.iter().map(|&b| b as u32).sum();
    hdr[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
    let mut raw = Vec::new();
    raw.extend_from_slice(&hdr);
    let mut data = [0u8; 512];
    data[..4].copy_from_slice(b"evil");
    raw.extend_from_slice(&data);
    raw.extend_from_slice(&[0u8; 1024]);
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    enc.write_all(&raw).unwrap();
    enc.finish().unwrap()
}

#[test]
fn dotdot_entry_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let archive = dir.path().join("evil.tar.gz");
    std::fs::write(&archive, evil_tar_gz("../evil")).unwrap();
    let dest = dir.path().join("out");
    std::fs::create_dir(&dest).unwrap();
    assert!(gray_pkg::fetch::unpack_tar_gz(&archive, &dest).is_err());
    assert!(!dir.path().join("evil").exists());
}

#[tokio::test]
async fn over_cap_download_aborted() {
    let _lock = ENV_LOCK.lock().unwrap();
    let home = use_env();
    let url = spawn_big_server(gray_pkg::fetch::MAX_BYTES + 1024 * 1024).await;

    let client = gray_pkg::fetch::client().unwrap();
    let err = gray_pkg::fetch::download(&client, &url, None)
        .await
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("exceeds 64 MiB cap"),
        "unexpected error: {err}"
    );
    assert!(tmp_is_empty(&home), "partial file must be removed");
}
