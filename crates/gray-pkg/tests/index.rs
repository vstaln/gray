//! Task 2.2: index client integration tests over a loopback axum server.
//!
//! Hermetic: ephemeral ports, fresh `TempDir` per test for `GRAY_HOME`
//! (isolates the index cache), no external network. Env-touching tests
//! hold the process-wide `ENV_LOCK` (Rust runs tests in one process).

// Deliberate: the env lock must span each whole test (env is read inside
// the awaited calls), so holding a std guard across await is the point.
#![allow(clippy::await_holding_lock)]

use std::sync::Mutex;

use axum::{
    Json, Router,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::get,
};

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn minimal_index() -> serde_json::Value {
    serde_json::json!({
        "schema": 1,
        "generated": "test",
        "plugins": {
            "demo": {
                "ecosystem": "gray-native",
                "version": "1.0.0",
                "source": {"type": "tarball", "url": "https://example.com/demo.tar.gz"},
                "hash": "sha256:abc"
            }
        }
    })
}

async fn spawn(router: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    format!("http://127.0.0.1:{port}/index.json")
}

/// Point `GRAY_HOME` at a fresh tempdir and `GRAY_PLUGIN_INDEX` at `url`.
/// Returns the tempdir (must stay alive for the test's duration).
fn use_env(url: &str) -> tempfile::TempDir {
    let home = tempfile::tempdir().unwrap();
    // SAFETY: under ENV_LOCK; no other test thread touches env concurrently.
    unsafe {
        std::env::set_var("GRAY_HOME", home.path());
        std::env::set_var(gray_pkg::index::INDEX_URL_ENV, url);
    }
    home
}

#[tokio::test]
async fn fetch_parses_loopback_index() {
    let _lock = ENV_LOCK.lock().unwrap();
    let url =
        spawn(Router::new().route("/index.json", get(|| async { Json(minimal_index()) }))).await;
    let _home = use_env(&url);

    let client = gray_pkg::fetch::client().unwrap();
    let index = gray_pkg::index::fetch_index(&client).await.unwrap();
    let entry = gray_pkg::index::lookup(&index, "demo").unwrap();
    assert_eq!(entry.version, "1.0.0");
}

#[tokio::test]
async fn lookup_miss_error_is_exact() {
    let _lock = ENV_LOCK.lock().unwrap();
    let url =
        spawn(Router::new().route("/index.json", get(|| async { Json(minimal_index()) }))).await;
    let _home = use_env(&url);

    let client = gray_pkg::fetch::client().unwrap();
    let index = gray_pkg::index::fetch_index(&client).await.unwrap();
    let err = gray_pkg::index::lookup(&index, "nope")
        .unwrap_err()
        .to_string();
    assert_eq!(err, "not in index: nope (try /plugin install <https-url>)");
}

#[tokio::test]
async fn etag_304_revalidates() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    let _lock = ENV_LOCK.lock().unwrap();
    let hits = Arc::new(AtomicUsize::new(0));
    let router = {
        let hits = hits.clone();
        Router::new().route(
            "/index.json",
            get(move |headers: HeaderMap| {
                let hits = hits.clone();
                async move {
                    hits.fetch_add(1, Ordering::SeqCst);
                    if headers
                        .get(axum::http::header::IF_NONE_MATCH)
                        .is_some_and(|v| v == "\"v1\"")
                    {
                        StatusCode::NOT_MODIFIED.into_response()
                    } else {
                        ([("etag", "\"v1\"")], Json(minimal_index())).into_response()
                    }
                }
            }),
        )
    };
    let url = spawn(router).await;
    let home = use_env(&url);

    let client = gray_pkg::fetch::client().unwrap();
    let first = gray_pkg::index::fetch_index(&client).await.unwrap();
    assert_eq!(hits.load(Ordering::SeqCst), 1);

    // Age the cache past its TTL so the second fetch must revalidate.
    let cache = home.path().join("plugins/index-cache.json");
    let mut v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cache).unwrap()).unwrap();
    v["fetched_at"] = serde_json::json!(0);
    std::fs::write(&cache, serde_json::to_string(&v).unwrap()).unwrap();

    let second = gray_pkg::index::fetch_index(&client).await.unwrap();
    assert_eq!(
        hits.load(Ordering::SeqCst),
        2,
        "second fetch must hit network"
    );
    assert!(second.plugins.contains_key("demo"));
    assert_eq!(
        first.plugins["demo"].version,
        second.plugins["demo"].version
    );
}

#[tokio::test]
async fn env_override_selects_loopback() {
    let _lock = ENV_LOCK.lock().unwrap();
    let url =
        spawn(Router::new().route("/index.json", get(|| async { Json(minimal_index()) }))).await;
    let _home = use_env(&url);

    assert_eq!(gray_pkg::index::index_url(), url);
    assert_ne!(url, gray_pkg::index::DEFAULT_INDEX_URL);
}
