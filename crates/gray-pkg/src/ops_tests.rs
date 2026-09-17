#![allow(clippy::await_holding_lock)]
use super::*;

// Serializes the process-global GRAY_HOME mutation within this test
// binary (cargo runs tests in one process on multiple threads).
// Shared with `errors::tests` (same process-global env).
pub(crate) static ENV_GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// One panicked test holding ENV_GUARD must not poison ~30 unrelated
/// suites (CI cascade). Recover the guard instead of propagating the
/// panic; the panicking test still fails on its own assertion.
pub(crate) fn env_guard() -> std::sync::MutexGuard<'static, ()> {
    ENV_GUARD.lock().unwrap_or_else(|e| e.into_inner())
}

#[test]
fn default_entry_is_enabled() {
    assert!(LockEntry::default().enabled);
}

#[test]
fn spec_splits_names_and_urls() {
    assert!(matches!(parse_spec("foo"), NameOrUrl::Name(_)));
    assert!(matches!(parse_spec("  foo  "), NameOrUrl::Name(_)));
    assert!(matches!(
        parse_spec("https://h/x.tar.gz"),
        NameOrUrl::Url(_)
    ));
    assert!(matches!(parse_spec("http://h/x.tar.gz"), NameOrUrl::Url(_)));
    assert_eq!(name_from_url("https://h/plugins/foo.tar.gz?x=1"), "foo");
}

#[test]
fn spec_parses_npm_forms() {
    match parse_spec("npm:pi-foo") {
        NameOrUrl::Npm { name, version } => {
            assert_eq!(name, "pi-foo");
            assert_eq!(version, None);
        }
        other => panic!("unexpected spec: {other:?}"),
    }
    match parse_spec("npm:@scope/bar@1.2.3") {
        NameOrUrl::Npm { name, version } => {
            assert_eq!(name, "@scope/bar");
            assert_eq!(version, Some("1.2.3".to_string()));
        }
        other => panic!("unexpected spec: {other:?}"),
    }
}

#[test]
fn npm_spec_edge_cases() {
    // Scoped without version stays unpinned.
    match parse_spec("npm:@scope/bar") {
        NameOrUrl::Npm { name, version } => {
            assert_eq!(name, "@scope/bar");
            assert_eq!(version, None);
        }
        other => panic!("unexpected spec: {other:?}"),
    }
    // Trailing `@` is unpinned, not an empty version.
    match parse_spec("npm:foo@") {
        NameOrUrl::Npm { name, version } => {
            assert_eq!(name, "foo");
            assert_eq!(version, None);
        }
        other => panic!("unexpected spec: {other:?}"),
    }
    // Pinned unscoped.
    match parse_spec("npm:pi-foo@0.2.0") {
        NameOrUrl::Npm { name, version } => {
            assert_eq!(name, "pi-foo");
            assert_eq!(version, Some("0.2.0".to_string()));
        }
        other => panic!("unexpected spec: {other:?}"),
    }
}

#[test]
fn spec_parses_git_forms() {
    // `git:` prefix with and without ref.
    match parse_spec("git:https://host/o/r.git@main") {
        NameOrUrl::Git { url, git_ref } => {
            assert_eq!(url, "https://host/o/r.git");
            assert_eq!(git_ref, Some("main".to_string()));
        }
        other => panic!("unexpected spec: {other:?}"),
    }
    match parse_spec("git:https://host/o/r.git") {
        NameOrUrl::Git { url, git_ref } => {
            assert_eq!(url, "https://host/o/r.git");
            assert_eq!(git_ref, None);
        }
        other => panic!("unexpected spec: {other:?}"),
    }
    // Raw forms are git by shape (R15); raw https stays tarball Url.
    assert!(matches!(
        parse_spec("ssh://git@host/o/r.git"),
        NameOrUrl::Git { .. }
    ));
    assert!(matches!(
        parse_spec("git://host/o/r.git"),
        NameOrUrl::Git { .. }
    ));
    assert!(matches!(
        parse_spec("git@host:o/r.git"),
        NameOrUrl::Git { .. }
    ));
    assert!(matches!(
        parse_spec("https://host/o/r.git"),
        NameOrUrl::Git { .. }
    ));
    assert!(matches!(
        parse_spec("https://h/x.tar.gz"),
        NameOrUrl::Url(_)
    ));
    // `@ref` alone never flips a tarball URL to git (R15).
    assert!(matches!(
        parse_spec("https://h/x.tar.gz@main"),
        NameOrUrl::Url(_)
    ));
    // Bare github.com repo pages (what users paste) are git sources.
    match parse_spec("https://github.com/DietrichGebert/ponytail") {
        NameOrUrl::Git { url, git_ref } => {
            assert_eq!(url, "https://github.com/DietrichGebert/ponytail");
            assert_eq!(git_ref, None);
        }
        other => panic!("unexpected spec: {other:?}"),
    }
    // Trailing slash / `.git` / case variants stay git; deeper paths
    // (release tarballs, trees, blobs) stay tarball Url.
    assert!(matches!(
        parse_spec("https://github.com/o/r/"),
        NameOrUrl::Git { .. }
    ));
    assert!(matches!(
        parse_spec("https://github.com/o/r.git"),
        NameOrUrl::Git { .. }
    ));
    assert!(matches!(
        parse_spec("https://GitHub.com/o/r"),
        NameOrUrl::Git { .. }
    ));
    assert!(matches!(
        parse_spec("https://github.com/o/r/releases/download/v1/x.tar.gz"),
        NameOrUrl::Url(_)
    ));
    assert!(matches!(
        parse_spec("https://github.com/o/r/tree/main/skills"),
        NameOrUrl::Url(_)
    ));
    assert!(matches!(
        parse_spec("https://example.com/o/r"),
        NameOrUrl::Url(_)
    ));
}

#[test]
fn npm_metadata_url_encodes_scope() {
    assert_eq!(
        npm_metadata_url("https://registry.npmjs.org", "@scope/bar"),
        "https://registry.npmjs.org/@scope%2Fbar"
    );
    assert_eq!(
        npm_metadata_url("http://127.0.0.1:1/", "pi-foo"),
        "http://127.0.0.1:1/pi-foo"
    );
}

// --- npm registry stub helpers (loopback only, no live registry) ---

fn b64_encode(bytes: &[u8]) -> String {
    const ALPH: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::new();
    for ch in bytes.chunks(3) {
        let mut n: u32 = 0;
        for &b in ch {
            n = (n << 8) | u32::from(b);
        }
        n <<= 8 * (3 - ch.len());
        s.push(ALPH[((n >> 18) & 63) as usize] as char);
        s.push(ALPH[((n >> 12) & 63) as usize] as char);
        s.push(if ch.len() > 1 {
            ALPH[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        s.push(if ch.len() > 2 {
            ALPH[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    s
}

fn sha512_integrity(bytes: &[u8]) -> String {
    use sha2::Digest;
    let mut h = sha2::Sha512::new();
    h.update(bytes);
    format!("sha512-{}", b64_encode(&h.finalize()))
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    let mut h = sha2::Sha256::new();
    h.update(bytes);
    format!("sha256:{:x}", h.finalize())
}

fn tiny_tgz() -> Vec<u8> {
    use std::io::Write;
    let bytes = br#"{"name":"pi-foo"}"#;
    let mut hdr = tar::Header::new_gnu();
    hdr.set_size(bytes.len() as u64);
    hdr.set_mode(0o644);
    hdr.set_mtime(0);
    hdr.set_cksum();
    let mut ar = tar::Builder::new(Vec::new());
    ar.append_data(&mut hdr, "package/package.json", &bytes[..])
        .unwrap();
    let tar_bytes = ar.into_inner().unwrap();
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    enc.write_all(&tar_bytes).unwrap();
    enc.finish().unwrap()
}

async fn spawn_tarball(tgz: Vec<u8>) -> String {
    use axum::{Router, routing::get};
    let router = Router::new().route(
        "/pi-foo.tgz",
        get(move || {
            let tgz = tgz.clone();
            async move { tgz }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    format!("http://127.0.0.1:{port}/pi-foo.tgz")
}

async fn spawn_registry(meta: serde_json::Value) -> String {
    use axum::{Json, Router, routing::get};
    let router = Router::new().route(
        "/pi-foo",
        get(move || {
            let meta = meta.clone();
            async move { Json(meta) }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    format!("http://127.0.0.1:{port}")
}

/// Point `GRAY_HOME` at a fresh tempdir and the npm registry at the stub.
/// Must be called under `ENV_GUARD`.
fn use_npm_env(registry_base: &str) -> tempfile::TempDir {
    let home = tempfile::tempdir().unwrap();
    // SAFETY: serialized by ENV_GUARD.
    unsafe {
        std::env::set_var("GRAY_HOME", home.path());
        std::env::set_var(NPM_REGISTRY_ENV, registry_base);
    }
    home
}

#[tokio::test]
async fn npm_resolve_picks_exact_version() {
    let _guard = env_guard();
    let tarball = spawn_tarball(tiny_tgz()).await;
    let meta = serde_json::json!({
        "dist-tags": {"latest": "2.0.0"},
        "versions": {
            "1.0.0": {"dist": {"tarball": tarball, "integrity": sha512_integrity(b"old")}},
            "2.0.0": {"dist": {"tarball": tarball, "integrity": sha512_integrity(b"new")}},
        }
    });
    let base = spawn_registry(meta).await;
    let _home = use_npm_env(&base);

    let client = crate::fetch::client().unwrap();
    let resolved = npm_resolve(&client, "pi-foo", Some("1.0.0")).await.unwrap();
    assert_eq!(resolved.tarball, tarball);
    assert_eq!(resolved.integrity, sha512_integrity(b"old"));
    assert_eq!(resolved.version, "1.0.0");
}

#[tokio::test]
async fn npm_resolve_falls_back_to_latest() {
    let _guard = env_guard();
    let tarball = spawn_tarball(tiny_tgz()).await;
    let meta = serde_json::json!({
        "dist-tags": {"latest": "2.0.0"},
        "versions": {
            "1.0.0": {"dist": {"tarball": tarball, "integrity": sha512_integrity(b"old")}},
            "2.0.0": {"dist": {"tarball": tarball, "integrity": sha512_integrity(b"new")}},
        }
    });
    let base = spawn_registry(meta).await;
    let _home = use_npm_env(&base);

    let client = crate::fetch::client().unwrap();
    let resolved = npm_resolve(&client, "pi-foo", None).await.unwrap();
    assert_eq!(resolved.tarball, tarball);
    assert_eq!(resolved.integrity, sha512_integrity(b"new"));
    assert_eq!(resolved.version, "2.0.0");
}

#[tokio::test]
async fn npm_resolve_shasum_fallback() {
    let _guard = env_guard();
    let tarball = spawn_tarball(tiny_tgz()).await;
    let meta = serde_json::json!({
        "dist-tags": {"latest": "1.0.0"},
        "versions": {
            "1.0.0": {"dist": {"tarball": tarball, "shasum": "abc123"}},
        }
    });
    let base = spawn_registry(meta).await;
    let _home = use_npm_env(&base);

    let client = crate::fetch::client().unwrap();
    let error = npm_resolve(&client, "pi-foo", None).await.unwrap_err();
    assert!(error.to_string().contains("SHA-1"));
}

#[tokio::test]
async fn npm_resolve_missing_version_bails() {
    let _guard = env_guard();
    let tarball = spawn_tarball(tiny_tgz()).await;
    let meta = serde_json::json!({
        "dist-tags": {"latest": "1.0.0"},
        "versions": {
            "1.0.0": {"dist": {"tarball": tarball, "shasum": "abc123"}},
        }
    });
    let base = spawn_registry(meta).await;
    let _home = use_npm_env(&base);

    let client = crate::fetch::client().unwrap();
    let err = npm_resolve(&client, "pi-foo", Some("9.9.9"))
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("9.9.9"), "unexpected error: {err}");
}

#[tokio::test]
async fn npm_resolve_missing_integrity_bails() {
    let _guard = env_guard();
    let tarball = spawn_tarball(tiny_tgz()).await;
    let meta = serde_json::json!({
        "dist-tags": {"latest": "1.0.0"},
        "versions": {
            "1.0.0": {"dist": {"tarball": tarball}},
        }
    });
    let base = spawn_registry(meta).await;
    let _home = use_npm_env(&base);

    let client = crate::fetch::client().unwrap();
    let err = npm_resolve(&client, "pi-foo", None)
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("no integrity"), "unexpected error: {err}");
}

#[tokio::test]
async fn stage_npm_package_downloads_verifies_unpacks() {
    let _guard = env_guard();
    let tgz = tiny_tgz();
    let tarball = spawn_tarball(tgz.clone()).await;
    let integrity = sha512_integrity(&tgz);
    let meta = serde_json::json!({
        "dist-tags": {"latest": "1.0.0"},
        "versions": {
            "1.0.0": {"dist": {"tarball": tarball, "integrity": integrity}},
        }
    });
    let base = spawn_registry(meta).await;
    let _home = use_npm_env(&base);

    let client = crate::fetch::client().unwrap();
    let staged = stage_npm_package(&client, "pi-foo", None).await.unwrap();
    assert_eq!(staged.name, "pi-foo");
    assert_eq!(staged.version, "1.0.0");
    assert_eq!(staged.integrity, integrity);
    assert_eq!(
        std::fs::read(staged.dir.path().join("package/package.json")).unwrap(),
        br#"{"name":"pi-foo"}"#
    );
}

#[tokio::test]
async fn stage_npm_package_hash_mismatch_bails() {
    let _guard = env_guard();
    let tarball = spawn_tarball(tiny_tgz()).await;
    let meta = serde_json::json!({
        "dist-tags": {"latest": "1.0.0"},
        "versions": {
            "1.0.0": {"dist": {"tarball": tarball, "integrity": sha512_integrity(b"other bytes")}},
        }
    });
    let base = spawn_registry(meta).await;
    let _home = use_npm_env(&base);

    let client = crate::fetch::client().unwrap();
    let err = stage_npm_package(&client, "pi-foo", None)
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("hash mismatch"), "unexpected error: {err}");
}

#[tokio::test]
async fn download_verifies_sha512_base64() {
    let _guard = env_guard();
    let _home = use_npm_env("http://127.0.0.1:1");
    let body = b"plugin bytes".to_vec();
    let url = spawn_tarball(body.clone()).await;

    let client = crate::fetch::client().unwrap();
    let want = crate::index::HashSpec::Single(sha512_integrity(&body));
    let path = crate::fetch::download(&client, &url, Some(&want))
        .await
        .unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), body);
    std::fs::remove_file(&path).unwrap();
}

#[tokio::test]
async fn download_verifies_sha256_hex() {
    let _guard = env_guard();
    let _home = use_npm_env("http://127.0.0.1:1");
    let body = b"plugin bytes".to_vec();
    let url = spawn_tarball(body.clone()).await;

    let client = crate::fetch::client().unwrap();
    let want = crate::index::HashSpec::Single(sha256_hex(&body));
    let path = crate::fetch::download(&client, &url, Some(&want))
        .await
        .unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), body);
    std::fs::remove_file(&path).unwrap();
}

#[tokio::test]
async fn download_rejects_sha512_mismatch() {
    let _guard = env_guard();
    let _home = use_npm_env("http://127.0.0.1:1");
    let url = spawn_tarball(b"plugin bytes".to_vec()).await;

    let client = crate::fetch::client().unwrap();
    let want = crate::index::HashSpec::Single(sha512_integrity(b"other bytes"));
    let err = crate::fetch::download(&client, &url, Some(&want))
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("hash mismatch"), "unexpected error: {err}");
}

#[tokio::test]
async fn download_rejects_unknown_algorithm() {
    let _guard = env_guard();
    let _home = use_npm_env("http://127.0.0.1:1");
    let url = spawn_tarball(b"plugin bytes".to_vec()).await;

    let client = crate::fetch::client().unwrap();
    let want = crate::index::HashSpec::Single("md5:abc".to_string());
    let err = crate::fetch::download(&client, &url, Some(&want))
        .await
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("unsupported hash algorithm"),
        "unexpected error: {err}"
    );
}

#[test]
fn sanitize_npm_key_cases() {
    assert_eq!(sanitize_npm_key("pi-foo"), "pi-foo");
    assert_eq!(sanitize_npm_key("@scope/bar"), "scope-bar");
    // Sanitized keys can never trip `remove`'s traversal rejection.
    for key in [sanitize_npm_key("pi-foo"), sanitize_npm_key("@scope/bar")] {
        assert!(!key.contains('/'));
        assert!(!key.contains(".."));
        assert!(!key.is_empty());
    }
}

#[test]
fn install_key_confines_dotdot_and_backslash() {
    // `..`, `a/../b`, `foo..bar` sanitize to safe keys …
    for raw in ["..", "a/../b", "foo..bar"] {
        let key = sanitize_npm_key(raw);
        assert!(!key.contains(".."), "raw {raw:?} → {key:?}");
        assert!(!key.contains('/'), "raw {raw:?} → {key:?}");
        assert!(!key.contains('\\'), "raw {raw:?} → {key:?}");
        assert!(!key.is_empty(), "raw {raw:?}");
        assert_ne!(key, "..");
        assert_ne!(key, ".");
        // … and the shared validator accepts the sanitized form.
        validate_install_key(&key).unwrap();
        install_key(raw).unwrap();
    }
    // … while empty, `.`, and backslash keys are rejected, not mangled.
    assert!(install_key("").is_err());
    assert!(install_key(".").is_err());
    assert!(validate_install_key("..").is_err());
    assert!(validate_install_key("a/b").is_err());
    assert!(validate_install_key("a\\b").is_err());
    assert!(install_key("a\\b").is_err());
    // `remove()` works on the sanitized forms (no traversal trap).
    let _guard = env_guard();
    let home = tempfile::tempdir().unwrap();
    // SAFETY: serialized by ENV_GUARD.
    unsafe {
        std::env::set_var("GRAY_HOME", home.path());
    }
    for raw in ["..", "a/../b", "foo..bar"] {
        let key = sanitize_npm_key(raw);
        let mut lock = LockFile::default();
        lock.plugins.insert(
            key.clone(),
            LockEntry {
                ecosystem: "pi-gallery".into(),
                version: "1.0.0".into(),
                ..LockEntry::default()
            },
        );
        write_lock(&lock).unwrap();
        let dir = crate::plugins_dir().join("pi").join(&key);
        std::fs::create_dir_all(&dir).unwrap();
        remove(&key).unwrap();
        assert!(!list().unwrap().contains_key(&key));
        assert!(!dir.exists());
    }
}

/// Build a fixture tarball with `package/`-prefixed entries (npm layout).
fn skill_tgz(files: &[(&str, &str)]) -> Vec<u8> {
    use std::io::Write;
    let mut ar = tar::Builder::new(Vec::new());
    for (name, content) in files {
        let mut hdr = tar::Header::new_gnu();
        hdr.set_size(content.len() as u64);
        hdr.set_mode(0o644);
        hdr.set_mtime(0);
        hdr.set_cksum();
        ar.append_data(&mut hdr, format!("package/{name}"), content.as_bytes())
            .unwrap();
    }
    let tar_bytes = ar.into_inner().unwrap();
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    enc.write_all(&tar_bytes).unwrap();
    enc.finish().unwrap()
}

/// Registry stub serving one metadata doc on ANY path (scoped names).
async fn spawn_registry_any(meta: serde_json::Value) -> String {
    use axum::{Json, Router, routing::get};
    let router = Router::new().route(
        "/*rest",
        get(move || {
            let meta = meta.clone();
            async move { Json(meta) }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    format!("http://127.0.0.1:{port}")
}

const MULCH_SKILL: &str = "---\ndescription: mulch skill\n---\nMulch body";
const TMUX_SKILL: &str = "---\ndescription: tmux skill\n---\nTmux body";

fn pi_foo_meta(tarball: &str, integrity: &str) -> serde_json::Value {
    serde_json::json!({
        "dist-tags": {"latest": "1.0.0"},
        "versions": {
            "1.0.0": {"dist": {"tarball": tarball, "integrity": integrity}},
        }
    })
}

#[tokio::test]
async fn install_npm_extracts_skills_and_writes_lock() {
    let _guard = env_guard();
    let tgz = skill_tgz(&[
        (
            "package.json",
            r#"{"name":"pi-foo","version":"1.0.0","pi":{"extensions":["./dist/extension.js"]}}"#,
        ),
        ("skills/mulch/SKILL.md", MULCH_SKILL),
        ("skills/run-in-tmux/SKILL.md", TMUX_SKILL),
        (
            "skills/run-in-tmux/scripts/run-in-tmux",
            "#!/bin/sh\necho hi\n",
        ),
        ("extensions/mulch.ts", "export const x = 1;\n"),
        ("themes/dark.css", "body {}\n"),
        ("README.md", "# pi-foo\n"),
    ]);
    let tarball = spawn_tarball(tgz.clone()).await;
    let integrity = sha512_integrity(&tgz);
    let base = spawn_registry(pi_foo_meta(&tarball, &integrity)).await;
    let _home = use_npm_env(&base);

    let report = install(parse_spec("npm:pi-foo"), InstallOpts::default())
        .await
        .unwrap();
    assert_eq!(report.name, "pi-foo");
    assert_eq!(report.version, "1.0.0");

    // `.md` only: skills + top-level doc land, code never does.
    assert_eq!(
        std::fs::read(report.path.join("mulch/SKILL.md")).unwrap(),
        MULCH_SKILL.as_bytes()
    );
    assert!(report.path.join("run-in-tmux/SKILL.md").is_file());
    assert!(report.path.join("README.md").is_file());
    assert!(!report.path.join("run-in-tmux/scripts/run-in-tmux").exists());
    assert!(!report.path.join("extensions").exists());
    assert!(!report.path.join("package.json").exists());

    // ONE lock entry, exact shape.
    let entry = list().unwrap().remove("pi-foo").expect("lock entry");
    assert_eq!(entry.ecosystem, "pi-gallery");
    assert_eq!(entry.version, "1.0.0");
    assert_eq!(entry.hash, integrity);
    assert_eq!(entry.source, tarball);
    assert_eq!(entry.scope, "user");
    assert!(entry.enabled);
}

#[tokio::test]
async fn install_npm_scoped_name_sanitizes_key_and_dir() {
    let _guard = env_guard();
    let tgz = skill_tgz(&[("skills/a/SKILL.md", MULCH_SKILL)]);
    let tarball = spawn_tarball(tgz.clone()).await;
    let integrity = sha512_integrity(&tgz);
    let meta = serde_json::json!({
        "dist-tags": {"latest": "2.0.0"},
        "versions": {
            "2.0.0": {"dist": {"tarball": tarball, "integrity": integrity}},
        }
    });
    let base = spawn_registry_any(meta).await;
    let _home = use_npm_env(&base);

    let report = install(parse_spec("npm:@scope/bar"), InstallOpts::default())
        .await
        .unwrap();
    assert_eq!(report.name, "scope-bar");
    assert!(
        report.path.ends_with("pi/scope-bar"),
        "{}",
        report.path.display()
    );
    assert!(report.path.join("a/SKILL.md").is_file());
    let entry = list().unwrap().remove("scope-bar").expect("lock entry");
    assert_eq!(entry.ecosystem, "pi-gallery");
    assert_eq!(entry.version, "2.0.0");
}

#[tokio::test]
async fn install_npm_honors_manifest_skill_globs() {
    let _guard = env_guard();
    let tgz = skill_tgz(&[
        (
            "package.json",
            r#"{"name":"pi-foo","version":"1.0.0","pi":{"skills":["./custom"]}}"#,
        ),
        ("custom/weird/SKILL.md", MULCH_SKILL),
    ]);
    let tarball = spawn_tarball(tgz.clone()).await;
    let integrity = sha512_integrity(&tgz);
    let base = spawn_registry(pi_foo_meta(&tarball, &integrity)).await;
    let _home = use_npm_env(&base);

    let report = install(parse_spec("npm:pi-foo"), InstallOpts::default())
        .await
        .unwrap();
    assert_eq!(report.name, "pi-foo");
    assert!(report.path.join("weird/SKILL.md").is_file());
}

#[test]
fn manifest_glob_bases_stay_in_staging_root() {
    assert!(glob_base_is_safe("skills"));
    assert!(glob_base_is_safe("custom/weird"));
    assert!(!glob_base_is_safe("/etc/x.md"));
    assert!(!glob_base_is_safe("../../evil.md"));
    assert!(!glob_base_is_safe("a/../../b"));
    assert!(!glob_base_is_safe(""));
}

#[test]
fn manifest_escape_globs_are_skipped_but_legit_match_survives() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("skills/legit")).unwrap();
    std::fs::write(root.path().join("skills/legit/SKILL.md"), MULCH_SKILL).unwrap();
    // Sibling outside the staging root that an escape glob would target.
    let outside = root.path().join("evil.md");
    std::fs::write(&outside, "evil").unwrap();
    let matches = collect_skill_matches(
        root.path(),
        &[
            "/etc/x.md".to_string(),
            "../../evil.md".to_string(),
            "./skills".to_string(),
        ],
    );
    let labels: Vec<_> = matches.iter().map(|m| m.label.clone()).collect();
    assert!(
        labels.contains(&"legit".to_string()),
        "legit skill survives: {labels:?}"
    );
    // Every match still resolves under the staging root.
    for m in &matches {
        assert!(
            m.src.strip_prefix(root.path()).is_ok(),
            "escape: {}",
            m.src.display()
        );
    }
}

// --- P2-4 git fixture repos (local commits only, no network) ---

/// `git` fixture repo with `files` committed on the default branch.
/// Returns the fixture dir (deleted on drop) and its `file://` URL.
fn init_git_fixture(files: &[(&str, &str)]) -> (tempfile::TempDir, String) {
    let dir = tempfile::tempdir().unwrap();
    for (name, content) in files {
        let path = dir.path().join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&path, content).unwrap();
    }
    let run = |args: &[&str]| {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(dir.path())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} failed");
    };
    run(&["init", "-q"]);
    run(&["add", "-A"]);
    run(&["commit", "-qm", "fixture"]);
    let url = format!("file://{}", dir.path().display());
    (dir, url)
}

fn git_fixture_sha(dir: &tempfile::TempDir) -> String {
    let out = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(out.status.success());
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

/// `GRAY_HOME` at a fresh tempdir (git installs need no registry stub).
/// Must be called under `ENV_GUARD`.
fn use_git_env() -> tempfile::TempDir {
    let home = tempfile::tempdir().unwrap();
    // SAFETY: serialized by ENV_GUARD.
    unsafe {
        std::env::set_var("GRAY_HOME", home.path());
    }
    home
}

fn git_fixture_key(dir: &tempfile::TempDir) -> String {
    sanitize_npm_key(dir.path().file_name().unwrap().to_str().unwrap())
}

#[tokio::test]
async fn install_git_clones_and_extracts_skills() {
    let _guard = env_guard();
    let (repo, url) = init_git_fixture(&[
        ("skills/mulch/SKILL.md", MULCH_SKILL),
        ("extensions/mulch.ts", "export const x = 1;\n"),
        ("README.md", "# demo\n"),
    ]);
    let sha = git_fixture_sha(&repo);
    let key = git_fixture_key(&repo);
    let _home = use_git_env();

    let report = install(parse_spec(&format!("git:{url}")), InstallOpts::default())
        .await
        .unwrap();
    assert_eq!(report.name, key);
    assert_eq!(report.version, "0.0.0");

    assert_eq!(
        std::fs::read(report.path.join("mulch/SKILL.md")).unwrap(),
        MULCH_SKILL.as_bytes()
    );
    assert!(report.path.join("README.md").is_file());
    assert!(!report.path.join("extensions").exists());

    // R17 lock shape: raw 40-hex sha, source is the clone URL.
    assert_eq!(sha.len(), 40);
    let entry = list().unwrap().remove(&key).expect("lock entry");
    assert_eq!(entry.ecosystem, "pi-gallery");
    assert_eq!(entry.version, "0.0.0");
    assert_eq!(entry.hash, sha);
    assert_eq!(entry.source, url);
    assert_eq!(entry.scope, "user");
    assert!(entry.enabled);
}

#[tokio::test]
async fn install_git_pinned_ref_records_version() {
    let _guard = env_guard();
    let (repo, url) = init_git_fixture(&[("skills/a/SKILL.md", MULCH_SKILL)]);
    let run = |args: &[&str]| {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(repo.path())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} failed");
    };
    run(&["checkout", "-qb", "feature"]);
    std::fs::create_dir_all(repo.path().join("skills/b")).unwrap();
    std::fs::write(repo.path().join("skills/b/SKILL.md"), TMUX_SKILL).unwrap();
    run(&["add", "-A"]);
    run(&["commit", "-qm", "feature"]);
    let sha = git_fixture_sha(&repo);
    let key = git_fixture_key(&repo);
    let _home = use_git_env();

    let report = install(
        parse_spec(&format!("git:{url}@feature")),
        InstallOpts::default(),
    )
    .await
    .unwrap();
    assert_eq!(report.name, key);
    assert_eq!(report.version, "feature");
    assert!(report.path.join("b/SKILL.md").is_file());
    let entry = list().unwrap().remove(&key).expect("lock entry");
    assert_eq!(entry.version, "feature");
    assert_eq!(entry.hash, sha);
    assert_eq!(entry.source, url);
}

#[tokio::test]
async fn install_git_without_skills_bails_without_half_state() {
    let _guard = env_guard();
    let (repo, _url) = init_git_fixture(&[("notes.txt", "no skills here\n")]);
    let key = git_fixture_key(&repo);
    let _home = use_git_env();

    let err = install(
        parse_spec(&format!("git:file://{}", repo.path().display())),
        InstallOpts::default(),
    )
    .await
    .unwrap_err()
    .to_string();
    assert!(err.contains("no skills"), "honest reason: {err}");
    assert!(list().unwrap().is_empty());
    assert!(!crate::plugins_dir().join("pi").join(&key).exists());
}

#[tokio::test]
async fn install_npm_without_skills_bails_without_half_state() {
    let _guard = env_guard();
    let tgz = tiny_tgz();
    let tarball = spawn_tarball(tgz.clone()).await;
    let integrity = sha512_integrity(&tgz);
    let base = spawn_registry(pi_foo_meta(&tarball, &integrity)).await;
    let _home = use_npm_env(&base);

    let err = install(parse_spec("npm:pi-foo"), InstallOpts::default())
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("pi-foo"), "names package: {err}");
    assert!(err.contains("no skills"), "honest reason: {err}");
    // Nothing recorded, nothing left behind.
    assert!(list().unwrap().is_empty());
    assert!(!crate::plugins_dir().join("pi").exists());
}

#[test]
fn remove_deletes_pi_subdir() {
    let _guard = env_guard();
    let home = tempfile::tempdir().unwrap();
    // SAFETY: serialized by ENV_GUARD.
    unsafe {
        std::env::set_var("GRAY_HOME", home.path());
    }
    let mut lock = LockFile::default();
    lock.plugins.insert(
        "scope-bar".to_string(),
        LockEntry {
            ecosystem: "pi-gallery".into(),
            version: "2.0.0".into(),
            ..LockEntry::default()
        },
    );
    write_lock(&lock).unwrap();
    let dir = crate::plugins_dir().join("pi").join("scope-bar");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("SKILL.md"), "x").unwrap();
    remove("scope-bar").unwrap();
    assert!(!list().unwrap().contains_key("scope-bar"));
    assert!(!dir.exists());
}

#[test]
fn lock_roundtrips_exact_shape() {
    let mut lock = LockFile {
        schema: 1,
        plugins: BTreeMap::new(),
    };
    lock.plugins.insert(
        "demo".to_string(),
        LockEntry {
            ecosystem: "gray-native".into(),
            version: "1.0.0".into(),
            hash: "sha256:abc".into(),
            source: "https://h/demo.tar.gz".into(),
            argv: vec![],
            adapter_version: "0.1.0".into(),
            installed_at: "1".into(),
            scope: "user".into(),
            enabled: true,
        },
    );
    let v: serde_json::Value =
        serde_json::from_str(&serde_json::to_string(&lock).unwrap()).unwrap();
    assert_eq!(v["schema"], 1);
    let e = &v["plugins"]["demo"];
    for k in [
        "ecosystem",
        "version",
        "hash",
        "source",
        "argv",
        "adapter_version",
        "installed_at",
        "scope",
    ] {
        assert!(e.get(k).is_some(), "missing {k}");
    }
}

#[test]
fn lock_enabled_defaults_true_and_roundtrips_false() {
    // Old locks without `enabled` load as enabled.
    let old: LockFile = serde_json::from_str(
            r#"{"schema":1,"plugins":{"demo":{"ecosystem":"gray-native","version":"1.0.0","hash":"sha256:abc","source":"https://h/demo.tar.gz","argv":[],"adapter_version":"0.1.0","installed_at":"1","scope":"user"}}}"#,
        )
        .unwrap();
    assert!(old.plugins["demo"].enabled);
    // New locks round-trip an explicit `false`.
    let mut lock = old.clone();
    lock.plugins.get_mut("demo").unwrap().enabled = false;
    let back: LockFile = serde_json::from_str(&serde_json::to_string(&lock).unwrap()).unwrap();
    assert!(!back.plugins["demo"].enabled);
}

#[test]
fn set_enabled_flips_flag_and_bails_on_miss() {
    // GRAY_HOME points at a tempdir so the real lockfile is never
    // disturbed (serialized by ENV_GUARD).
    let _guard = env_guard();
    let home = tempfile::tempdir().unwrap();
    // SAFETY: serialized by ENV_GUARD.
    unsafe {
        std::env::set_var("GRAY_HOME", home.path());
    }
    let mut lock = LockFile::default();
    lock.plugins.insert(
        "demo".to_string(),
        LockEntry {
            ecosystem: "gray-native".into(),
            version: "1.0.0".into(),
            enabled: true,
            ..LockEntry::default()
        },
    );
    write_lock(&lock).unwrap();
    set_enabled("demo", false).unwrap();
    assert!(!list().unwrap()["demo"].enabled);
    set_enabled("demo", true).unwrap();
    assert!(list().unwrap()["demo"].enabled);
    let err = set_enabled("nope", false).unwrap_err();
    assert_eq!(err.to_string(), "not installed: nope");
}

#[test]
fn remove_rejects_traversal_and_empty_names() {
    let _guard = env_guard();
    let home = tempfile::tempdir().unwrap();
    // SAFETY: serialized by ENV_GUARD.
    unsafe {
        std::env::set_var("GRAY_HOME", home.path());
    }
    let mut lock = LockFile::default();
    lock.plugins.insert(
        "demo".to_string(),
        LockEntry {
            ecosystem: "gray-native".into(),
            version: "1.0.0".into(),
            ..LockEntry::default()
        },
    );
    write_lock(&lock).unwrap();
    for bad in ["../evil", "a/b", "..", ""] {
        let err = remove(bad).unwrap_err();
        assert_eq!(
            err.to_string(),
            format!("not installed: {bad}"),
            "name {bad:?}"
        );
    }
    // The lock entry and the plugins dir survive the rejections.
    assert!(list().unwrap().contains_key("demo"));
}

#[test]
fn remove_deletes_entry_and_dir() {
    let _guard = env_guard();
    let home = tempfile::tempdir().unwrap();
    // SAFETY: serialized by ENV_GUARD.
    unsafe {
        std::env::set_var("GRAY_HOME", home.path());
    }
    let mut lock = LockFile::default();
    lock.plugins.insert(
        "demo".to_string(),
        LockEntry {
            ecosystem: "gray-native".into(),
            version: "1.0.0".into(),
            ..LockEntry::default()
        },
    );
    write_lock(&lock).unwrap();
    let dir = crate::plugins_dir().join("demo");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("plugin.sh"), "#!/bin/sh\n").unwrap();
    remove("demo").unwrap();
    assert!(!list().unwrap().contains_key("demo"));
    assert!(!dir.exists());
}

// --- P2-3 search fan-out fixtures (loopback only, no live network) ---

async fn spawn_index_stub(index: serde_json::Value) -> String {
    use axum::{Json, Router, routing::get};
    let router = Router::new().route(
        "/index.json",
        get(move || {
            let index = index.clone();
            async move { Json(index) }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    format!("http://127.0.0.1:{port}/index.json")
}

fn index_fixture(names: &[(&str, &str)]) -> serde_json::Value {
    let mut plugins = serde_json::Map::new();
    for (name, version) in names {
        plugins.insert(
            (*name).to_string(),
            serde_json::json!({
                "ecosystem": "gray-native",
                "version": version,
                "source": {"type": "https", "url": "https://h/x.tar.gz"},
                "hash": "sha256:abc",
            }),
        );
    }
    serde_json::json!({"schema": 1, "generated": "", "plugins": plugins})
}

/// Full install env: index + npm registry + ClawHub base + Claude
/// marketplace specs. Must be called under `ENV_GUARD`.
fn use_market_env(
    index_url: &str,
    registry_base: &str,
    clawhub_base: &str,
    claude_markets: &str,
) -> tempfile::TempDir {
    let home = tempfile::tempdir().unwrap();
    // SAFETY: serialized by ENV_GUARD.
    unsafe {
        std::env::set_var("GRAY_HOME", home.path());
        std::env::set_var(crate::index::INDEX_URL_ENV, index_url);
        std::env::set_var(NPM_REGISTRY_ENV, registry_base);
        std::env::set_var(crate::sources::CLAWHUB_BASE_ENV, clawhub_base);
        std::env::set_var(crate::sources::CLAUDE_MARKETPLACES_ENV, claude_markets);
    }
    home
}

#[test]
fn spec_parses_clawhub_and_claude_forms() {
    match parse_spec("clawhub:arein/test") {
        NameOrUrl::ClawHub { slug } => assert_eq!(slug, "arein/test"),
        other => panic!("unexpected spec: {other:?}"),
    }
    match parse_spec("clawhub:test") {
        NameOrUrl::ClawHub { slug } => assert_eq!(slug, "test"),
        other => panic!("unexpected spec: {other:?}"),
    }
    match parse_spec("claude:my-plugin@fixture-market") {
        NameOrUrl::Claude {
            plugin,
            marketplace,
        } => {
            assert_eq!(plugin, "my-plugin");
            assert_eq!(marketplace, Some("fixture-market".to_string()));
        }
        other => panic!("unexpected spec: {other:?}"),
    }
    // No `@marketplace` stays unpinned; a trailing `@` is unpinned too.
    match parse_spec("claude:my-plugin") {
        NameOrUrl::Claude {
            plugin,
            marketplace,
        } => {
            assert_eq!(plugin, "my-plugin");
            assert_eq!(marketplace, None);
        }
        other => panic!("unexpected spec: {other:?}"),
    }
    match parse_spec("claude:my-plugin@") {
        NameOrUrl::Claude {
            plugin,
            marketplace,
        } => {
            assert_eq!(plugin, "my-plugin");
            assert_eq!(marketplace, None);
        }
        other => panic!("unexpected spec: {other:?}"),
    }
}

// --- Task 2 install fixtures (loopback + local dirs only) ---

/// Minimal stored-ZIP builder (ClawHub serves ZIPs, not tarballs).
fn skill_zip(files: &[(&str, &str)]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, content) in files {
        let data = content.as_bytes();
        let off = out.len() as u32;
        out.extend_from_slice(b"PK\x03\x04");
        for _ in 0..5 {
            out.extend_from_slice(&0u16.to_le_bytes());
        }
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(data);
        central.extend_from_slice(b"PK\x01\x02");
        for _ in 0..3 {
            central.extend_from_slice(&0u16.to_le_bytes());
        }
        central.extend_from_slice(&0u16.to_le_bytes());
        for _ in 0..2 {
            central.extend_from_slice(&0u16.to_le_bytes());
        }
        central.extend_from_slice(&0u32.to_le_bytes());
        central.extend_from_slice(&(data.len() as u32).to_le_bytes());
        central.extend_from_slice(&(data.len() as u32).to_le_bytes());
        central.extend_from_slice(&(name.len() as u16).to_le_bytes());
        for _ in 0..4 {
            central.extend_from_slice(&0u16.to_le_bytes());
        }
        central.extend_from_slice(&0u32.to_le_bytes());
        central.extend_from_slice(&off.to_le_bytes());
        central.extend_from_slice(name.as_bytes());
    }
    let cd_off = out.len() as u32;
    let cd_size = central.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(b"PK\x05\x06");
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&cd_size.to_le_bytes());
    out.extend_from_slice(&cd_off.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

/// ClawHub stub: fixed `/search` results plus a `fixture/demo` skill
/// (detail + empty version files + ZIP download). The download 429s
/// once (with `Retry-After: 0`) to prove the retry routing; the
/// verdicts endpoint answers for `claw-foo` only, so `claw-bar` pins
/// the search-payload fallback path.
#[derive(Clone)]
struct ClawStub {
    zip: Vec<u8>,
    downloads: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

async fn spawn_clawhub_stub(zip: Vec<u8>) -> String {
    use axum::{
        Json, Router,
        extract::State,
        http::{HeaderMap, StatusCode},
        routing::{get, post},
    };
    use std::sync::atomic::Ordering;
    // `claw-foo` carries NO payload trust: its `scan:clean` must come
    // from the verdicts batch or the test fails.
    let search = serde_json::json!({"results": [
        {"slug": "claw-foo", "displayName": "Claw Foo",
         "summary": "does claw things", "version": "4.0.0",
         "ownerHandle": "fixture", "official": false},
        {"slug": "claw-bar", "displayName": "Claw Bar",
         "summary": "payload trust only", "version": "1.0.0",
         "official": false, "trust": {"clawHubVerdict": "clean"}},
        {"slug": "gray-foo", "displayName": "Gray Copy",
         "summary": "suppressed duplicate", "version": "9.9.9"},
    ]});
    let verdicts = serde_json::json!({"items": [
        {"ok": true, "decision": "pass", "requestedSlug": "claw-foo",
         "requestedOwnerHandle": "fixture", "requestedVersion": "4.0.0",
         "version": "4.0.0", "security": {"status": "clean", "passed": true}},
    ]});
    let detail = serde_json::json!({
        "skill": {"slug": "demo", "displayName": "Demo", "summary": "demo skill"},
        "latestVersion": {"version": "1.0.0"},
        "owner": {"handle": "fixture"},
    });
    let versions = serde_json::json!(
        {"version": {"version": "1.0.0", "files": [], "security": {"status": "clean"}}}
    );
    let router = Router::new()
        .route(
            "/api/v1/search",
            get(move || {
                let search = search.clone();
                async move { Json(search) }
            }),
        )
        .route(
            "/api/v1/skills/demo",
            get(move || {
                let detail = detail.clone();
                async move { Json(detail) }
            }),
        )
        .route(
            "/api/v1/skills/demo/versions/1.0.0",
            get(move || {
                let versions = versions.clone();
                async move { Json(versions) }
            }),
        )
        .route(
            "/api/v1/download",
            get(|State(st): State<ClawStub>| async move {
                if st.downloads.fetch_add(1, Ordering::SeqCst) == 0 {
                    let mut h = HeaderMap::new();
                    h.insert("retry-after", "0".parse().unwrap());
                    return (StatusCode::TOO_MANY_REQUESTS, h, Vec::new());
                }
                (StatusCode::OK, HeaderMap::new(), st.zip.clone())
            }),
        )
        .route(
            "/api/v1/skills/-/security-verdicts",
            post(move || {
                let verdicts = verdicts.clone();
                async move { Json(verdicts) }
            }),
        )
        .with_state(ClawStub {
            zip,
            downloads: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    format!("http://127.0.0.1:{port}/api/v1")
}

/// Local Claude marketplace fixture (no network): one path plugin
/// with a skill plus one command plugin (install must refuse it).
fn init_claude_fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".claude-plugin")).unwrap();
    std::fs::write(
        dir.path().join(".claude-plugin/marketplace.json"),
        r#"{"name":"fixture-market","plugins":[
                {"name":"claude-foo","description":"does claude things","version":"3.0.0",
                 "source":"./plugins/claude-foo"},
                {"name":"claude-cmd","description":"command thing",
                 "source":{"source":"command","command":"make plugin"}}
            ]}"#,
    )
    .unwrap();
    std::fs::create_dir_all(dir.path().join("plugins/claude-foo/skills/greeter")).unwrap();
    std::fs::write(
        dir.path()
            .join("plugins/claude-foo/skills/greeter/SKILL.md"),
        MULCH_SKILL,
    )
    .unwrap();
    dir
}

#[tokio::test]
async fn install_clawhub_downloads_and_writes_lock() {
    let _guard = env_guard();
    let zip = skill_zip(&[
        ("skills/greeter/SKILL.md", MULCH_SKILL),
        ("README.md", "# demo\n"),
    ]);
    let clawhub = spawn_clawhub_stub(zip).await;
    let market = init_claude_fixture();
    let markets = format!("file://{}", market.path().display());
    let index_url = spawn_index_stub(index_fixture(&[])).await;
    let _home = use_market_env(&index_url, "http://127.0.0.1:1", &clawhub, &markets);

    let report = install(parse_spec("clawhub:fixture/demo"), InstallOpts::default())
        .await
        .unwrap();
    // Owner-qualified key: no collision with another owner's `demo`.
    assert_eq!(report.name, "fixture-demo");
    assert_eq!(report.version, "1.0.0");
    // No version file list from the stub → honest unverified path.
    // (The stub 429s the download once: success proves the retry.)
    assert!(report.path.join("greeter/SKILL.md").is_file());
    let entry = list().unwrap().remove("fixture-demo").expect("lock entry");
    assert_eq!(entry.ecosystem, "clawhub");
    assert_eq!(entry.version, "1.0.0");
    assert_eq!(entry.source, "https://clawhub.ai/fixture/skills/demo");
}

#[tokio::test]
async fn install_claude_path_source_and_command_refusal() {
    let _guard = env_guard();
    let market = init_claude_fixture();
    let markets = format!("file://{}", market.path().display());
    let index_url = spawn_index_stub(index_fixture(&[])).await;
    let _home = use_market_env(
        &index_url,
        "http://127.0.0.1:1",
        "http://127.0.0.1:1",
        &markets,
    );

    let report = install(
        parse_spec("claude:claude-foo@fixture-market"),
        InstallOpts::default(),
    )
    .await
    .unwrap();
    assert_eq!(report.name, "claude-foo");
    assert_eq!(report.version, "3.0.0");
    assert!(report.path.join("greeter/SKILL.md").is_file());
    let entry = list().unwrap().remove("claude-foo").expect("lock entry");
    assert_eq!(entry.ecosystem, "claude");

    // `command` sources refuse with the warning string (never exec)…
    let err = install(parse_spec("claude:claude-cmd"), InstallOpts::default())
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("command source"), "warns honestly: {err}");
    // …and the failure is recorded (Task 1 ruling holds for new arms).
    let recorded = crate::errors::list();
    assert!(
        recorded
            .iter()
            .any(|e| e.source == "claude" && e.item == "claude-cmd"),
        "registry keeps the failure: {recorded:?}"
    );
    // Unknown marketplace filters miss honestly too.
    let err = install(
        parse_spec("claude:claude-foo@no-such-market"),
        InstallOpts::default(),
    )
    .await
    .unwrap_err()
    .to_string();
    assert!(err.contains("not in claude marketplaces"), "miss: {err}");
}

#[test]
fn git_names_allow_trailing_slashes() {
    for url in [
        "https://github.com/owner/repo/",
        "https://github.com/owner/repo.git/",
        "git@github.com:owner/repo.git/",
    ] {
        assert_eq!(name_from_git_url(url), "repo");
    }
    // Windows file:// fixtures carry a drive letter: the name is still the
    // last path segment, and the drive colon must not read as a host split.
    #[cfg(windows)]
    assert_eq!(
        name_from_git_url("file://C:\\Users\\someone\\AppData\\Local\\Temp\\fixture"),
        "fixture"
    );
}

#[tokio::test]
async fn install_git_trailing_slash_extracts_and_records() {
    let _guard = env_guard();
    let (repo, url) = init_git_fixture(&[("skills/a/SKILL.md", MULCH_SKILL)]);
    let _home = use_git_env();
    let url = format!("{url}/");
    let report = install(parse_spec(&format!("git:{url}")), InstallOpts::default())
        .await
        .unwrap();
    assert_eq!(report.name, git_fixture_key(&repo));
    assert_eq!(
        std::fs::read(report.path.join("a/SKILL.md")).unwrap(),
        MULCH_SKILL.as_bytes()
    );
    assert_eq!(list().unwrap()[&report.name].source, url);
}

#[tokio::test]
async fn failed_reinstall_preserves_existing_plugin() {
    let _guard = env_guard();
    let _home = use_git_env();
    let url = spawn_tarball(b"not an archive".to_vec()).await;
    let dest = crate::plugins_dir().join("pi-foo");
    std::fs::create_dir_all(&dest).unwrap();
    std::fs::write(dest.join("keep.txt"), b"old install").unwrap();
    let client = crate::fetch::client().unwrap();
    assert!(
        install_url(&client, &url, InstallOpts::default())
            .await
            .is_err()
    );
    assert_eq!(
        std::fs::read(dest.join("keep.txt")).unwrap(),
        b"old install"
    );
}

#[test]
fn archive_replacement_removes_stale_files_and_rolls_back_lock_failure() {
    let _guard = env_guard();
    let home = use_git_env();
    let dest = crate::plugins_dir().join("demo");
    std::fs::create_dir_all(&dest).unwrap();
    std::fs::write(dest.join("stale.txt"), b"old").unwrap();
    let archive = home.path().join("archive.tgz");
    std::fs::write(&archive, tiny_tgz()).unwrap();
    assert!(replace_archive(&archive, "demo", || anyhow::bail!("lock failure")).is_err());
    assert_eq!(std::fs::read(dest.join("stale.txt")).unwrap(), b"old");
    std::fs::write(&archive, tiny_tgz()).unwrap();
    replace_archive(&archive, "demo", || Ok(())).unwrap();
    assert!(!dest.join("stale.txt").exists());
    assert!(dest.join("package/package.json").exists());
}

#[tokio::test]
async fn unsafe_url_names_rejected_before_download() {
    let client = crate::fetch::client().unwrap();
    for name in [".", ".."] {
        let error = install_url(
            &client,
            &format!("http://127.0.0.1:1/{name}"),
            InstallOpts::default(),
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("safe plugin name"), "{error}");
    }
}

#[test]
fn archive_rollback_preserves_backup_when_cleanup_fails() {
    let _guard = env_guard();
    let home = use_git_env();
    let root = crate::plugins_dir();
    let dest = root.join("demo");
    std::fs::create_dir_all(&dest).unwrap();
    std::fs::write(dest.join("keep.txt"), b"working install").unwrap();
    let archive = home.path().join("archive.tgz");
    std::fs::write(&archive, tiny_tgz()).unwrap();
    let error = replace_archive(&archive, "demo", || {
        // A concurrent replacement prevents rollback cleanup of the directory.
        std::fs::remove_dir_all(&dest)?;
        std::fs::write(&dest, b"concurrent file")?;
        anyhow::bail!("record failed")
    })
    .unwrap_err();
    let backups: Vec<_> = std::fs::read_dir(&root)
        .unwrap()
        .map(|entry| entry.unwrap().path().join("previous/keep.txt"))
        .filter(|path| path.is_file())
        .collect();
    assert_eq!(backups.len(), 1, "previous install was deleted: {error}");
    assert_eq!(std::fs::read(&backups[0]).unwrap(), b"working install");
    assert!(error.to_string().contains("saved at"), "{error}");
}
