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

/// Point `GRAY_HOME` at a fresh tempdir. Must be called under `ENV_GUARD`.
fn use_home_env() -> tempfile::TempDir {
    let home = tempfile::tempdir().unwrap();
    // SAFETY: serialized by ENV_GUARD.
    unsafe {
        std::env::set_var("GRAY_HOME", home.path());
    }
    home
}

#[tokio::test]
async fn download_verifies_sha512_base64() {
    let _guard = env_guard();
    let _home = use_home_env();
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
    let _home = use_home_env();
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
    let _home = use_home_env();
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
    let _home = use_home_env();
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
fn install_key_confines_dotdot_and_backslash() {
    for bad in ["..", ".", "a/b", "a\\b", ""] {
        assert!(validate_install_key(bad).is_err(), "{bad:?}");
    }
    assert!(validate_install_key("pi-foo").is_ok());
}

#[test]
fn remove_deletes_pi_subdir() {
    let _guard = env_guard();
    let _home = use_home_env();
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
            cli_argv: None,
            extra: Default::default(),
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
fn lock_cli_argv_roundtrips() {
    let mut lock = LockFile::default();
    lock.plugins.insert(
        "demo".to_string(),
        LockEntry {
            cli_argv: Some(vec!["/usr/bin/demo".into()]),
            ..LockEntry::default()
        },
    );
    let back: LockFile = serde_json::from_str(&serde_json::to_string(&lock).unwrap()).unwrap();
    assert_eq!(
        back.plugins["demo"].cli_argv.as_deref(),
        Some(&["/usr/bin/demo".to_string()][..])
    );
    // Sidecar-only entries keep serializing without the field.
    let plain: serde_json::Value =
        serde_json::from_str(&serde_json::to_string(&LockEntry::default()).unwrap()).unwrap();
    assert!(plain.get("cli_argv").is_none());
}

#[test]
fn set_enabled_flips_flag_and_bails_on_miss() {
    // GRAY_HOME points at a tempdir so the real lockfile is never
    // disturbed (serialized by ENV_GUARD).
    let _guard = env_guard();
    let _home = use_home_env();
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
fn set_enabled_preserves_plugin_owned_lock_fields() {
    // gray_plugin::lock owns fields this crate does not model (grants,
    // consent hash, runtime role). A gray-pkg rewrite must carry them or a
    // disable/enable cycle silently re-grants every declared capability.
    let _guard = env_guard();
    let _home = use_home_env();
    let mut extra = serde_json::Map::new();
    extra.insert(
        "granted_capabilities".to_string(),
        serde_json::json!(["fs.read"]),
    );
    extra.insert("capabilities_hash".to_string(), serde_json::json!("abc123"));
    extra.insert(
        "runtime_role".to_string(),
        serde_json::json!("provider_only"),
    );
    let mut lock = LockFile::default();
    lock.plugins.insert(
        "demo".to_string(),
        LockEntry {
            ecosystem: "gray-native".into(),
            version: "1.0.0".into(),
            enabled: true,
            extra,
            ..LockEntry::default()
        },
    );
    write_lock(&lock).unwrap();
    set_enabled("demo", false).unwrap();
    let entry = &list().unwrap()["demo"];
    assert!(!entry.enabled);
    assert_eq!(
        entry.extra["granted_capabilities"],
        serde_json::json!(["fs.read"])
    );
    assert_eq!(
        entry.extra["capabilities_hash"],
        serde_json::json!("abc123")
    );
    assert_eq!(
        entry.extra["runtime_role"],
        serde_json::json!("provider_only")
    );
}

#[test]
fn remove_rejects_traversal_and_empty_names() {
    let _guard = env_guard();
    let _home = use_home_env();
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
    let _home = use_home_env();
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

#[tokio::test]
async fn failed_reinstall_preserves_existing_plugin() {
    let _guard = env_guard();
    let _home = use_home_env();
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
    let home = use_home_env();
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
    let home = use_home_env();
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

#[test]
fn registry_lock_times_out_when_held() {
    use crate::ops::hold_registry_lock_timeout;
    let home = tempfile::tempdir().unwrap();
    let _g = ENV_GUARD.lock().unwrap();
    // SAFETY: serialized by ENV_GUARD.
    unsafe { std::env::set_var("GRAY_HOME", home.path()) };
    // Somebody else holds the registry lock for longer than the caller's patience.
    let lock_path = crate::plugins_dir().join(".registry.lock");
    std::fs::create_dir_all(crate::plugins_dir()).unwrap();
    let holder = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .open(&lock_path)
        .unwrap();
    holder.try_lock().unwrap();
    let err = hold_registry_lock_timeout(std::time::Duration::from_millis(50))
        .err()
        .expect("a contended registry lock must fail");
    assert!(
        err.to_string().contains("another plugin operation"),
        "{err}"
    );
    drop(holder);
    assert!(hold_registry_lock_timeout(std::time::Duration::from_millis(50)).is_ok());
}

#[test]
fn remove_keeps_files_when_the_registry_write_fails() {
    let _guard = env_guard();
    let _home = use_home_env();
    let plugin_dir = crate::plugins_dir().join("demo");
    std::fs::create_dir_all(&plugin_dir).unwrap();
    std::fs::write(plugin_dir.join("marker"), "here").unwrap();
    let mut lock = LockFile::default();
    lock.plugins.insert(
        "demo".to_string(),
        LockEntry {
            ecosystem: "gray-native".to_string(),
            version: "1.0.0".to_string(),
            hash: String::new(),
            source: String::new(),
            argv: vec![],
            adapter_version: "1".to_string(),
            installed_at: "0".to_string(),
            scope: "user".to_string(),
            enabled: true,
            cli_argv: None,
            extra: Default::default(),
        },
    );
    write_lock(&lock).unwrap();
    // Make the registry file unwritable so the write fails mid-remove.
    let registry = crate::plugins_dir().join("lock.json");
    std::fs::write(&registry, "sentinel").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&registry, std::fs::Permissions::from_mode(0o400)).unwrap();
    }
    let err = remove("demo")
        .err()
        .expect("a failed registry write must propagate");
    assert!(!err.to_string().is_empty());
    // The files are still there: the registry is committed only after it
    // can be written, so a failed remove leaves a re-removable plugin.
    assert!(
        plugin_dir.join("marker").exists(),
        "files must survive a failed remove"
    );
}
