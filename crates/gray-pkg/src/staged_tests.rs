fn approved_demo() -> super::ApprovedEntry {
    super::ApprovedEntry::new(
        "demo".into(),
        serde_json::from_value(serde_json::json!({
            "ecosystem": "gray-native", "kind": "plugin", "version": "1.0.0",
            "source": {"type": "tarball", "url": "https://example.invalid/demo.tgz"},
            "hash": format!("sha256:{}", "0".repeat(64))
        }))
        .unwrap(),
    )
    .unwrap()
}

#[tokio::test]
async fn cancelled_prepare_has_no_install_side_effects() {
    let home = tempfile::tempdir().unwrap();
    let cancel = tokio_util::sync::CancellationToken::new();
    cancel.cancel();
    let result = super::prepare(
        home.path(),
        approved_demo(),
        &gray_pkg_client(),
        cancel,
        None,
    )
    .await;
    assert!(result.is_err());
    assert!(!home.path().join("plugins/demo").exists());
    assert!(!home.path().join("plugins/lock.json").exists());
}

fn gray_pkg_client() -> reqwest::Client {
    crate::fetch::client().unwrap()
}

fn archive_bytes(path: &str) -> Vec<u8> {
    let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    let mut archive = tar::Builder::new(encoder);
    let body = b"#!/bin/sh\nprintf '{}\\n'\n";
    let mut header = tar::Header::new_gnu();
    header.set_size(body.len() as u64);
    header.set_mode(0o755);
    // Set raw bytes so the traversal fixture reaches the unpacker, not tar's builder guard.
    header.as_mut_bytes()[..path.len()].copy_from_slice(path.as_bytes());
    header.set_cksum();
    archive.append(&header, &body[..]).unwrap();
    archive.into_inner().unwrap().finish().unwrap()
}

async fn serve(router: axum::Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    format!("http://{address}/demo.tgz")
}

async fn archive_entry(bytes: Vec<u8>) -> super::ApprovedEntry {
    use sha2::Digest;
    let hash = format!("sha256:{:x}", sha2::Sha256::digest(&bytes));
    let url = serve(axum::Router::new().route(
        "/demo.tgz",
        axum::routing::get(move || {
            let bytes = bytes.clone();
            async move { bytes }
        }),
    ))
    .await;
    let mut entry = approved_demo().entry().clone();
    entry.source.url = url;
    entry.hash = crate::index::HashSpec::Single(hash);
    super::ApprovedEntry::new("demo".into(), entry).unwrap()
}

async fn prepared_demo(home: &std::path::Path) -> super::PreparedPackage {
    let entry = archive_entry(archive_bytes("plugin.sh")).await;
    super::prepare(
        home,
        entry,
        &gray_pkg_client(),
        tokio_util::sync::CancellationToken::new(),
        None,
    )
    .await
    .unwrap()
}

fn assert_clean(home: &std::path::Path) {
    let plugins = home.join("plugins");
    if !plugins.exists() {
        return;
    }
    for path in std::fs::read_dir(plugins).unwrap() {
        let path = path.unwrap().path();
        assert_eq!(
            path.file_name().unwrap(),
            "tmp",
            "unexpected leftover: {}",
            path.display()
        );
        assert_eq!(std::fs::read_dir(path).unwrap().count(), 0);
    }
}

#[tokio::test]
async fn verified_preparation_is_owned_and_not_installed() {
    let home = tempfile::tempdir().unwrap();
    let prepared = prepared_demo(home.path()).await;
    assert_eq!(prepared.home(), home.path());
    assert_eq!(prepared.approved().name(), "demo");
    assert!(prepared.root().join("plugin.sh").is_file());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_ne!(
            std::fs::metadata(prepared.root().join("plugin.sh"))
                .unwrap()
                .permissions()
                .mode()
                & 0o111,
            0
        );
    }
    assert!(!home.path().join("plugins/demo").exists());
    assert!(!home.path().join("plugins/lock.json").exists());
    let stage = prepared.root().parent().unwrap().to_path_buf();
    drop(prepared);
    assert!(!stage.exists());
    assert_clean(home.path());
}

#[tokio::test]
async fn corrupt_digest_and_traversal_cleanup_staging() {
    for corrupt_hash in [true, false] {
        let home = tempfile::tempdir().unwrap();
        let approved = archive_entry(archive_bytes(if corrupt_hash {
            "plugin.sh"
        } else {
            "../escape"
        }))
        .await;
        let approved = if corrupt_hash {
            let mut entry = approved.entry().clone();
            entry.hash = approved_demo().entry().hash.clone();
            super::ApprovedEntry::new("demo".into(), entry).unwrap()
        } else {
            approved
        };
        let error = super::prepare(
            home.path(),
            approved,
            &gray_pkg_client(),
            tokio_util::sync::CancellationToken::new(),
            None,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains(if corrupt_hash {
            "hash mismatch"
        } else {
            "unsafe archive"
        }));
        assert!(!home.path().join("escape").exists());
        assert_clean(home.path());
    }
}

#[tokio::test]
async fn cancellation_during_download_cleans_owned_archive() {
    let home = tempfile::tempdir().unwrap();
    let (started, received) = tokio::sync::oneshot::channel();
    let started = std::sync::Arc::new(std::sync::Mutex::new(Some(started)));
    let url = serve(axum::Router::new().route(
        "/demo.tgz",
        axum::routing::get(move || {
            let started = started.clone();
            async move {
                let _ = started.lock().unwrap().take().unwrap().send(());
                std::future::pending::<Vec<u8>>().await
            }
        }),
    ))
    .await;
    let mut entry = approved_demo().entry().clone();
    entry.source.url = url;
    let approved = super::ApprovedEntry::new("demo".into(), entry).unwrap();
    let cancel = tokio_util::sync::CancellationToken::new();
    let client = gray_pkg_client();
    let future = super::prepare(home.path(), approved, &client, cancel.clone(), None);
    tokio::pin!(future);
    tokio::select! {
        result = &mut future => panic!("preparation completed unexpectedly: {result:?}"),
        _ = received => {}
    }
    cancel.cancel();
    assert!(future.await.unwrap_err().to_string().contains("cancelled"));
    assert_clean(home.path());
}

#[tokio::test]
async fn cancellation_after_response_body_starts_removes_partial_download() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let home = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/demo.tgz", listener.local_addr().unwrap());
    let (started, received) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = [0; 1024];
        assert!(socket.read(&mut request).await.unwrap() > 0);
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4096\r\n\r\npartial archive")
            .await
            .unwrap();
        let _ = started.send(());
        std::future::pending::<()>().await;
    });
    let mut entry = approved_demo().entry().clone();
    entry.source.url = url;
    let cancel = tokio_util::sync::CancellationToken::new();
    let client = gray_pkg_client();
    let future = super::prepare(
        home.path(),
        super::ApprovedEntry::new("demo".into(), entry).unwrap(),
        &client,
        cancel.clone(),
        None,
    );
    tokio::pin!(future);
    tokio::select! {
        result = &mut future => panic!("unexpected preparation result: {result:?}"),
        _ = async {
            received.await.unwrap();
            tokio::time::timeout(std::time::Duration::from_secs(2), async {
                loop {
                    if std::fs::read_dir(home.path().join("plugins/tmp"))
                        .is_ok_and(|mut entries| entries.next().is_some())
                    {
                        break;
                    }
                    tokio::task::yield_now().await;
                }
            }).await.unwrap();
        } => {}
    }
    cancel.cancel();
    assert!(future.await.unwrap_err().to_string().contains("cancelled"));
    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());
    assert_clean(home.path());
}

#[tokio::test]
async fn cancellation_at_unpack_stage_waits_for_owned_cleanup() {
    let home = tempfile::tempdir().unwrap();
    let approved = archive_entry(archive_bytes("plugin.sh")).await;
    let cancel = tokio_util::sync::CancellationToken::new();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let client = gray_pkg_client();
    let future = super::prepare(home.path(), approved, &client, cancel.clone(), Some(tx));
    tokio::pin!(future);
    let mut stages = Vec::new();
    loop {
        tokio::select! {
            biased;
            stage = rx.recv() => {
                let stage = stage.unwrap();
                stages.push(stage);
                if stage == super::InstallStage::Unpacking {
                    cancel.cancel();
                    break;
                }
            }
            result = &mut future => panic!("unexpected preparation result: {result:?}"),
        }
    }
    assert!(future.await.unwrap_err().to_string().contains("cancelled"));
    assert_eq!(
        stages,
        [
            super::InstallStage::Downloading,
            super::InstallStage::Unpacking
        ]
    );
    assert_clean(home.path());
}

#[test]
fn approved_identity_rejects_unsafe_or_unsupported_entries() {
    for name in [
        "",
        "../escape",
        "Demo",
        "demo.plugin",
        "tmp",
        "pi",
        "a/b",
        "a\\b",
    ] {
        assert!(super::ApprovedEntry::new(name.into(), approved_demo().entry().clone()).is_err());
    }
    for (ecosystem, kind, source) in [
        ("other", "plugin", "tarball"),
        ("gray-native", "skill", "tarball"),
        ("gray-native", "plugin", "git"),
    ] {
        let mut entry = approved_demo().entry().clone();
        entry.ecosystem = ecosystem.into();
        entry.kind = kind.into();
        entry.source.type_ = source.into();
        assert!(super::ApprovedEntry::new("demo".into(), entry).is_err());
    }
    let mut entry = approved_demo().entry().clone();
    entry.hash = crate::index::HashSpec::Single(String::new());
    assert!(super::ApprovedEntry::new("demo".into(), entry).is_err());
}

#[test]
fn absent_dependencies_fail_closed_by_name() {
    let home = tempfile::tempdir().unwrap();
    let mut entry = approved_demo().entry().clone();
    entry.requires_plugins = vec!["questions".into(), "ledger".into()];
    let approved = super::ApprovedEntry::new("demo".into(), entry).unwrap();
    let error = super::check_constraints(home.path(), &approved)
        .unwrap_err()
        .to_string();
    assert!(error.contains("questions"));
    assert!(error.contains("ledger"));
    assert!(!home.path().join("plugins/lock.json").exists());
}

fn consent_metadata() -> super::CommitMetadata {
    super::CommitMetadata {
        extra: serde_json::from_value(serde_json::json!({
            "granted_capabilities": [],
            "capabilities_hash": "consented-empty",
            "runtime_role": "tool"
        }))
        .unwrap(),
        ..Default::default()
    }
}

fn registry(home: &std::path::Path, names: &[&str]) {
    let mut lock = crate::ops::LockFile::default();
    for name in names {
        lock.plugins.insert(
            (*name).into(),
            crate::ops::LockEntry {
                enabled: false,
                ..Default::default()
            },
        );
    }
    crate::ops::write_lock_in(&home.join("plugins"), &lock).unwrap();
}

#[tokio::test]
async fn constraints_fail_before_download_or_directory_creation() {
    let home = tempfile::tempdir().unwrap();
    let mut entry = approved_demo().entry().clone();
    entry.requires_plugins = vec!["questions".into(), "ledger".into()];
    let error = super::prepare(
        home.path(),
        super::ApprovedEntry::new("demo".into(), entry).unwrap(),
        &gray_pkg_client(),
        tokio_util::sync::CancellationToken::new(),
        None,
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("questions, ledger"));
    assert!(!home.path().join("plugins").exists());
}

#[test]
fn disabled_dependencies_count_present_and_corrupt_registry_fails_closed() {
    let home = tempfile::tempdir().unwrap();
    registry(home.path(), &["questions"]);
    let mut entry = approved_demo().entry().clone();
    entry.requires_plugins = vec!["questions".into()];
    let approved = super::ApprovedEntry::new("demo".into(), entry).unwrap();
    super::check_constraints(home.path(), &approved).unwrap();
    std::fs::write(home.path().join("plugins/lock.json"), b"corrupt").unwrap();
    assert!(super::check_constraints(home.path(), &approved).is_err());
}

#[tokio::test]
async fn commit_installs_exact_approved_identity_and_consent() {
    let home = tempfile::tempdir().unwrap();
    let prepared = prepared_demo(home.path()).await;
    let approved = prepared.approved().clone();
    let stage = prepared.root().parent().unwrap().to_path_buf();
    let executable = home
        .path()
        .join("plugins/demo/plugin.sh")
        .to_string_lossy()
        .into_owned();
    let mut metadata = consent_metadata();
    metadata.argv = vec![executable.clone(), "serve".into()];
    metadata.cli_argv = Some(vec![executable.clone()]);
    let outcome = prepared.commit(metadata).unwrap();
    let super::CommitOutcome::Installed(report) = outcome else {
        panic!("not installed")
    };
    assert_eq!(report.name, approved.name());
    assert_eq!(report.version, approved.entry().version);
    assert_eq!(report.path, home.path().join("plugins/demo"));
    assert!(report.deps.is_empty());
    assert!(report.path.join("plugin.sh").is_file());
    assert!(!stage.exists());
    let lock = crate::ops::read_lock_in(&home.path().join("plugins"))
        .unwrap()
        .unwrap();
    assert_eq!(lock.schema, 1);
    assert_eq!(lock.plugins.len(), 1);
    let row = &lock.plugins["demo"];
    assert_eq!(row.ecosystem, approved.entry().ecosystem);
    assert_eq!(row.version, approved.entry().version);
    assert_eq!(row.hash, approved.entry().hash.primary().unwrap());
    assert_eq!(row.source, approved.entry().source.url);
    assert_eq!(row.scope, "user");
    assert!(row.enabled);
    assert_eq!(row.argv, [executable.clone(), "serve".into()]);
    assert_eq!(row.cli_argv, Some(vec![executable]));
    assert_eq!(row.extra["granted_capabilities"], serde_json::json!([]));
    assert_eq!(row.extra["capabilities_hash"], "consented-empty");
    assert_eq!(row.extra["runtime_role"], "tool");
    assert_eq!(row.adapter_version, env!("CARGO_PKG_VERSION"));
    assert!(!row.installed_at.is_empty());
}

#[tokio::test]
async fn registry_publication_failure_rolls_back_files() {
    let home = tempfile::tempdir().unwrap();
    let prepared = prepared_demo(home.path()).await;
    let stage = prepared.root().parent().unwrap().to_path_buf();
    let result = prepared.commit_with(consent_metadata(), |_, _| {
        anyhow::bail!("injected registry failure")
    });
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("injected registry failure")
    );
    assert!(!home.path().join("plugins/demo").exists());
    assert!(!home.path().join("plugins/lock.json").exists());
    assert!(!stage.exists());
}

#[tokio::test]
async fn rollback_failure_preserves_and_reports_actual_recovery_paths() {
    let home = tempfile::tempdir().unwrap();
    let prepared = prepared_demo(home.path()).await;
    let root = prepared.root().to_path_buf();
    let stage = root.parent().unwrap().to_path_buf();
    let destination = home.path().join("plugins/demo");
    let error = prepared
        .commit_with(consent_metadata(), |_, _| {
            std::fs::write(&root, b"block rollback")?;
            anyhow::bail!("injected registry failure")
        })
        .unwrap_err()
        .to_string();
    assert!(error.contains("rollback failed"));
    assert!(error.contains(destination.to_str().unwrap()));
    assert!(error.contains(stage.to_str().unwrap()));
    assert!(destination.join("plugin.sh").is_file());
    assert!(stage.exists());
    assert!(!home.path().join("plugins/lock.json").exists());
}

#[tokio::test]
async fn concurrent_install_is_not_replaced() {
    let home = tempfile::tempdir().unwrap();
    let first = prepared_demo(home.path()).await;
    let second = prepared_demo(home.path()).await;
    first.commit(consent_metadata()).unwrap();
    let marker = home.path().join("plugins/demo/first-install");
    std::fs::write(&marker, b"preserve").unwrap();
    let stage = second.root().parent().unwrap().to_path_buf();
    assert!(matches!(second.commit(consent_metadata()).unwrap(),
        super::CommitOutcome::AlreadyInstalled(name) if name == "demo"));
    assert_eq!(std::fs::read(marker).unwrap(), b"preserve");
    assert!(!stage.exists());
    assert_eq!(
        crate::ops::read_lock_in(&home.path().join("plugins"))
            .unwrap()
            .unwrap()
            .plugins
            .len(),
        1
    );
}

#[tokio::test]
async fn dependency_disappearance_between_prepare_and_commit_fails_closed() {
    let home = tempfile::tempdir().unwrap();
    registry(home.path(), &["questions"]);
    let approved = archive_entry(archive_bytes("plugin.sh")).await;
    let mut entry = approved.entry().clone();
    entry.requires_plugins = vec!["questions".into()];
    let prepared = super::prepare(
        home.path(),
        super::ApprovedEntry::new("demo".into(), entry).unwrap(),
        &gray_pkg_client(),
        tokio_util::sync::CancellationToken::new(),
        None,
    )
    .await
    .unwrap();
    registry(home.path(), &[]);
    let error = prepared.commit(consent_metadata()).unwrap_err().to_string();
    assert!(error.contains("missing required plugins: questions"));
    assert!(!home.path().join("plugins/demo").exists());
    assert!(
        crate::ops::read_lock_in(&home.path().join("plugins"))
            .unwrap()
            .unwrap()
            .plugins
            .is_empty()
    );
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn new_conflict_between_prepare_and_commit_fails_closed() {
    let _guard = crate::ops::tests::env_guard();
    let previous = std::env::var_os("GRAY_PLUGIN_FORCE");
    unsafe {
        std::env::remove_var("GRAY_PLUGIN_FORCE");
    }
    let home = tempfile::tempdir().unwrap();
    let approved = archive_entry(archive_bytes("plugin.sh")).await;
    let mut entry = approved.entry().clone();
    entry.conflicts = vec!["rival".into()];
    let approved = super::ApprovedEntry::new("demo".into(), entry).unwrap();
    let prepared = super::prepare(
        home.path(),
        approved.clone(),
        &gray_pkg_client(),
        tokio_util::sync::CancellationToken::new(),
        None,
    )
    .await
    .unwrap();
    registry(home.path(), &["rival"]);
    let error = prepared.commit(consent_metadata()).unwrap_err().to_string();
    assert!(error.contains("conflicts with installed plugins: rival"));
    assert!(!home.path().join("plugins/demo").exists());
    unsafe {
        std::env::set_var("GRAY_PLUGIN_FORCE", "1");
    }
    super::check_constraints(home.path(), &approved).unwrap();
    let mut entry = approved.entry().clone();
    entry.requires_plugins = vec!["missing".into()];
    let approved = super::ApprovedEntry::new("demo".into(), entry).unwrap();
    assert!(
        super::check_constraints(home.path(), &approved)
            .unwrap_err()
            .to_string()
            .contains("missing required plugins: missing")
    );
    unsafe {
        match previous {
            Some(value) => std::env::set_var("GRAY_PLUGIN_FORCE", value),
            None => std::env::remove_var("GRAY_PLUGIN_FORCE"),
        }
    }
}

#[tokio::test]
async fn corrupt_registry_at_commit_preserves_existing_contents() {
    let home = tempfile::tempdir().unwrap();
    let prepared = prepared_demo(home.path()).await;
    let lock = home.path().join("plugins/lock.json");
    std::fs::write(&lock, b"not json").unwrap();
    assert!(prepared.commit(consent_metadata()).is_err());
    assert!(!home.path().join("plugins/demo").exists());
    assert_eq!(std::fs::read(lock).unwrap(), b"not json");
}

#[tokio::test]
async fn registry_lock_contention_blocks_commit() {
    let home = tempfile::tempdir().unwrap();
    let prepared = prepared_demo(home.path()).await;
    let guard = crate::ops::hold_registry_lock_in(&home.path().join("plugins"))
        .unwrap()
        .expect("filesystem must support locking for this test");
    let error = tokio::task::spawn_blocking(move || prepared.commit(consent_metadata()))
        .await
        .unwrap()
        .unwrap_err()
        .to_string();
    assert!(error.contains("another plugin operation"));
    assert!(!home.path().join("plugins/demo").exists());
    assert!(!home.path().join("plugins/lock.json").exists());
    drop(guard);
}

#[tokio::test]
async fn unregistered_destination_is_preserved() {
    let home = tempfile::tempdir().unwrap();
    let prepared = prepared_demo(home.path()).await;
    let destination = home.path().join("plugins/demo");
    std::fs::create_dir(&destination).unwrap();
    std::fs::write(destination.join("user-data"), b"preserve").unwrap();
    let error = prepared.commit(consent_metadata()).unwrap_err().to_string();
    assert!(error.contains("unregistered"));
    assert_eq!(
        std::fs::read(destination.join("user-data")).unwrap(),
        b"preserve"
    );
    assert!(!home.path().join("plugins/lock.json").exists());
}

#[tokio::test]
async fn commit_requires_consent_and_rejects_shadowed_registry_fields() {
    for (key, value) in [
        ("capabilities_hash", serde_json::Value::Null),
        ("capabilities_hash", serde_json::json!([])),
        ("granted_capabilities", serde_json::json!("all")),
        ("granted_capabilities", serde_json::Value::Null),
        ("version", serde_json::json!("9.0.0")),
        ("hash", serde_json::json!("sha256:other")),
        ("source", serde_json::json!("https://other.invalid")),
        ("argv", serde_json::json!([])),
        ("enabled", serde_json::json!(false)),
    ] {
        let home = tempfile::tempdir().unwrap();
        let prepared = prepared_demo(home.path()).await;
        let mut metadata = consent_metadata();
        metadata.extra.insert(key.into(), value);
        assert!(
            prepared.commit(metadata).is_err(),
            "{key} must fail validation"
        );
        assert!(!home.path().join("plugins/demo").exists());
        assert!(!home.path().join("plugins/lock.json").exists());
    }
    let home = tempfile::tempdir().unwrap();
    let prepared = prepared_demo(home.path()).await;
    assert!(prepared.commit(super::CommitMetadata::default()).is_err());
    assert!(!home.path().join("plugins/demo").exists());
}
