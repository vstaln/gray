#[test]
fn empty_consent_is_not_grandfathered() {
    let record = super::policy::grant(&[]);
    assert!(record.granted.is_empty());
    assert_eq!(record.hash, gray_plugin::capabilities::consent_hash(&[]));
}

#[test]
fn unknown_capabilities_are_not_granted() {
    let record = super::policy::grant(&[
        gray_plugin::capabilities::HOST_ASK.to_string(),
        "invented.capability".into(),
    ]);
    assert_eq!(
        record.granted,
        vec![gray_plugin::capabilities::HOST_ASK.to_string()]
    );
}

#[test]
fn consent_normalizes_known_ids_and_displays_unknowns() {
    let declared = vec![" host.ask ".into(), "host.ask".into(), "invented".into()];
    let consent = super::policy::grant(&declared);
    assert_eq!(consent.granted, ["host.ask"]);
    assert_eq!(
        consent.hash,
        gray_plugin::capabilities::consent_hash(&["host.ask".into()])
    );
    let lines = super::policy::capability_lines(&declared);
    assert_eq!(lines.len(), 3);
    assert!(lines[0].grantable);
    assert_eq!(
        lines[0].description,
        gray_plugin::capabilities::spec("host.ask")
            .unwrap()
            .description
    );
    assert_eq!(lines[2].id, "invented");
    assert!(!lines[2].grantable);
}

#[test]
fn scan_blocks_only_dangerous_and_retains_caution_findings() {
    use gray_plugin::scan::{Finding, Report, Severity};
    let mut report = Report::default();
    super::policy::require_installable_scan(&report).unwrap();
    report.findings.push(Finding {
        rule: "fixture",
        severity: Severity::Caution,
        path: "plugin.sh".into(),
        line: 1,
        detail: "review this finding".into(),
    });
    super::policy::require_installable_scan(&report).unwrap();
    assert!(report.render().contains("review this finding"));
    report.findings[0].severity = Severity::Critical;
    let error = super::policy::require_installable_scan(&report).unwrap_err();
    assert!(error.to_string().contains("blocked installation"));
}

#[cfg(unix)]
mod staged {
    use super::super as install;
    use gray_pkg::staged::{ApprovedEntry, InstallStage};
    use sha2::{Digest, Sha256};
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio_util::sync::CancellationToken;

    fn executable(root: &Path, script: &str) -> PathBuf {
        let path = root.join("plugin.sh");
        std::fs::write(&path, script).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    const COUNTER: &str = r#"#!/bin/sh
printf x >> "$0.count"
printf '%s\n' '{"name":"runtime-demo","version":"1.0.0","protocol":"1.1","capabilities":["host.ask"],"commands":["/demo"]}'
"#;

    #[tokio::test]
    async fn manifest_probe_runs_once() {
        let root = tempfile::tempdir().unwrap();
        let exe = executable(root.path(), COUNTER);
        let raw = super::super::probe::manifest(&exe, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(raw["name"], "runtime-demo");
        assert_eq!(
            std::fs::read_to_string(exe.with_extension("sh.count")).unwrap(),
            "x"
        );
    }

    async fn wait_for_file(path: &Path) {
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while !std::fs::read_to_string(path)
                .ok()
                .is_some_and(|s| s.trim().parse::<i32>().is_ok())
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    fn assert_reaped(pid_file: &Path) {
        let pid = std::fs::read_to_string(pid_file)
            .unwrap()
            .trim()
            .parse::<i32>()
            .unwrap();
        // The owned probe must wait, not merely signal the child.
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        );
        assert_eq!(
            unsafe { libc::waitpid(pid, std::ptr::null_mut(), libc::WNOHANG) },
            -1
        );
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ECHILD)
        );
    }

    #[tokio::test]
    async fn cancelling_manifest_probe_kills_and_reaps_child() {
        let root = tempfile::tempdir().unwrap();
        let exe = executable(
            root.path(),
            "#!/bin/sh\necho $$ > \"$0.pid\"\nexec sleep 30\n",
        );
        let cancel = CancellationToken::new();
        let token = cancel.clone();
        let task = tokio::spawn(async move { super::super::probe::manifest(&exe, token).await });
        let pid = root.path().join("plugin.sh.pid");
        wait_for_file(&pid).await;
        cancel.cancel();
        let error = task.await.unwrap().unwrap_err();
        assert!(error.to_string().contains("cancelled"));
        assert_reaped(&pid);
    }

    const WIRE: &str = r#"#!/bin/sh
if [ $# -gt 0 ]; then exit 1; fi
while IFS= read -r line; do
  case "$line" in
    *plugin/shutdown*) exit 0 ;;
    *plugin/manifest*)
      printf '%s\n' '{"id":1,"result":{"name":"runtime-demo","version":"1.0.0","protocol":"1.1","tools":[],"capabilities":["host.ask","invented"],"commands":["/demo"]}}'
      ;;
  esac
done
"#;

    #[tokio::test]
    async fn wire_fallback_matches_existing_sidecar_manifest() {
        use gray_plugin::Plugin as _;
        let root = tempfile::tempdir().unwrap();
        let exe = executable(root.path(), WIRE);
        let prepared = install::probe::prepared(root.path(), CancellationToken::new())
            .await
            .unwrap();
        assert!(!prepared.cli);
        assert_eq!(prepared.declared, ["host.ask", "invented"]);
        let old = gray_plugin::SidecarPlugin::spawn(vec![exe.to_string_lossy().into_owned()])
            .await
            .unwrap();
        let expected = serde_json::to_value(old.manifest()).unwrap();
        old.shutdown(std::time::Duration::from_secs(2)).await;
        assert_eq!(
            serde_json::to_value(gray_plugin::Manifest::from_result(&prepared.raw)).unwrap(),
            expected
        );
    }

    #[tokio::test]
    async fn cancelling_wire_handshake_kills_and_reaps_child() {
        let root = tempfile::tempdir().unwrap();
        executable(
            root.path(),
            "#!/bin/sh\nif [ $# -gt 0 ]; then exit 1; fi\necho $$ > \"$0.pid\"\nexec sleep 30\n",
        );
        let cancel = CancellationToken::new();
        let token = cancel.clone();
        let path = root.path().to_owned();
        let task = tokio::spawn(async move { install::probe::prepared(&path, token).await });
        let pid = root.path().join("plugin.sh.pid");
        wait_for_file(&pid).await;
        cancel.cancel();
        assert!(format!("{:#}", task.await.unwrap().unwrap_err()).contains("cancelled"));
        assert_reaped(&pid);
    }

    #[tokio::test]
    async fn cancelled_cli_probe_never_starts_wire_fallback() {
        let root = tempfile::tempdir().unwrap();
        executable(
            root.path(),
            "#!/bin/sh\nif [ $# -eq 0 ]; then touch \"$0.wire\"; exit 1; fi\necho $$ > \"$0.pid\"\nexec sleep 30\n",
        );
        let cancel = CancellationToken::new();
        let token = cancel.clone();
        let path = root.path().to_owned();
        let task = tokio::spawn(async move { install::probe::prepared(&path, token).await });
        let pid = root.path().join("plugin.sh.pid");
        wait_for_file(&pid).await;
        cancel.cancel();
        assert!(task.await.unwrap().is_err());
        assert!(!root.path().join("plugin.sh.wire").exists());
        assert_reaped(&pid);
    }

    #[tokio::test]
    async fn manifest_timeout_and_oversized_output_reap_children() {
        let root = tempfile::tempdir().unwrap();
        let exe = executable(
            root.path(),
            "#!/bin/sh\necho $$ > \"$0.pid\"\nexec sleep 30\n",
        );
        let error = install::probe::manifest(&exe, CancellationToken::new())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("timed out"));
        assert_reaped(&root.path().join("plugin.sh.pid"));
        executable(
            root.path(),
            "#!/bin/sh\necho $$ > \"$0.pid\"\nwhile :; do printf '01234567890123456789012345678901'; done\n",
        );
        let error = install::probe::manifest(&exe, CancellationToken::new())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("64 KiB"));
        assert_reaped(&root.path().join("plugin.sh.pid"));
    }

    #[tokio::test]
    async fn wire_oversized_output_reaps_child() {
        let root = tempfile::tempdir().unwrap();
        let exe = executable(
            root.path(),
            "#!/bin/sh\necho $$ > \"$0.pid\"\nwhile :; do printf '01234567890123456789012345678901'; done\n",
        );
        let error = install::probe::wire_manifest(&exe, CancellationToken::new())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("256 KiB"));
        assert_reaped(&root.path().join("plugin.sh.pid"));
    }

    struct Fixture {
        home: tempfile::TempDir,
        approved: ApprovedEntry,
        server: tokio::task::JoinHandle<()>,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            self.server.abort();
        }
    }

    async fn fixture(script: &str, extra: Option<&str>) -> Fixture {
        let source = tempfile::tempdir().unwrap();
        executable(source.path(), script);
        let archive = source.path().join("fixture.tgz");
        let mut tar = std::process::Command::new("tar");
        tar.env("COPYFILE_DISABLE", "1")
            .arg("--format=ustar")
            .arg("-czf")
            .arg(&archive)
            .arg("-C")
            .arg(source.path())
            .arg("plugin.sh");
        if let Some(extra) = extra {
            std::fs::write(source.path().join("payload.sh"), extra).unwrap();
            tar.arg("payload.sh");
        }
        assert!(tar.status().unwrap().success());
        let bytes = std::fs::read(archive).unwrap();
        let hash = format!("sha256:{:x}", Sha256::digest(&bytes));
        let (url, server) = serve_bytes(bytes).await;
        let approved = ApprovedEntry::new(
            "demo".into(),
            serde_json::from_value(serde_json::json!({
                "ecosystem": "gray-native", "kind": "plugin", "version": "catalog-1",
                "source": {"type": "tarball", "url": url}, "hash": hash
            }))
            .unwrap(),
        )
        .unwrap();
        Fixture {
            home: tempfile::tempdir().unwrap(),
            approved,
            server,
        }
    }

    async fn serve_bytes(bytes: Vec<u8>) -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/fixture", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0; 4096];
            socket.read(&mut request).await.unwrap();
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                bytes.len()
            );
            socket.write_all(head.as_bytes()).await.unwrap();
            socket.write_all(&bytes).await.unwrap();
        });
        (url, server)
    }

    fn assert_clean(home: &Path) {
        assert!(!home.join("plugins/lock.json").exists());
        assert!(!home.join("plugins/demo").exists());
        for item in std::fs::read_dir(home.join("plugins")).unwrap() {
            let path = item.unwrap().path();
            assert_eq!(path.file_name().unwrap(), "tmp");
            assert_eq!(std::fs::read_dir(path).unwrap().count(), 0);
        }
    }

    #[tokio::test]
    async fn preparation_keeps_exact_manifest_and_cleans_up_when_dropped() {
        let fixture = fixture(COUNTER, None).await;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let prepared = install::prepare(
            fixture.home.path(),
            fixture.approved.clone(),
            CancellationToken::new(),
            Some(tx),
        )
        .await
        .unwrap();
        assert_eq!(prepared.package.approved().entry().version, "catalog-1");
        assert_eq!(prepared.manifest.raw["name"], "runtime-demo");
        assert_eq!(
            prepared.manifest.executable_relative,
            Path::new("plugin.sh")
        );
        assert_eq!(prepared.manifest.commands, ["/demo"]);
        assert_eq!(prepared.manifest.declared, ["host.ask"]);
        assert!(prepared.manifest.cli);
        assert_eq!(
            std::fs::read_to_string(prepared.package.root().join("plugin.sh.count")).unwrap(),
            "x"
        );
        let mut stages = Vec::new();
        while let Ok(stage) = rx.try_recv() {
            stages.push(stage);
        }
        assert_eq!(
            stages,
            [
                InstallStage::Downloading,
                InstallStage::Unpacking,
                InstallStage::Scanning,
                InstallStage::ReadingManifest
            ]
        );
        drop(prepared);
        assert_clean(fixture.home.path());
    }

    #[tokio::test]
    async fn dangerous_scan_blocks_before_any_executable_probe() {
        let marker = tempfile::tempdir().unwrap();
        let path = marker.path().join("probe-count");
        let script = format!("#!/bin/sh\nprintf x >> '{}'\n", path.display());
        let fixture = fixture(&script, Some("#!/bin/sh\nrm -rf /\n")).await;
        let error = install::prepare(
            fixture.home.path(),
            fixture.approved.clone(),
            CancellationToken::new(),
            None,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("scan blocked"));
        assert!(!path.exists());
        assert_clean(fixture.home.path());
    }

    #[tokio::test]
    async fn invalid_manifest_drops_staging_and_does_not_install() {
        let fixture = fixture("#!/bin/sh\nprintf 'not-json\\n'\n", None).await;
        let error = install::prepare(
            fixture.home.path(),
            fixture.approved.clone(),
            CancellationToken::new(),
            None,
        )
        .await
        .unwrap_err();
        assert!(format!("{error:#}").contains("invalid plugin manifest JSON"));
        assert_clean(fixture.home.path());
    }

    #[tokio::test]
    async fn preparation_error_keeps_exit_status_without_plugin_stdout() {
        let fixture = fixture(
            "#!/bin/sh\nprintf 'private-plugin-output\\n'\nexit 9\n",
            None,
        )
        .await;
        let error = install::prepare(
            fixture.home.path(),
            fixture.approved.clone(),
            CancellationToken::new(),
            None,
        )
        .await
        .unwrap_err();
        let display = format!("{error:#}");
        assert!(!display.contains("private-plugin-output"), "{display}");
        assert!(display.contains("exit status: 9"), "{display}");
        assert_clean(fixture.home.path());
    }

    #[tokio::test]
    async fn caution_findings_survive_preparation_for_consent() {
        let fixture = fixture(COUNTER, Some("# review .gray/auth.json\n")).await;
        let prepared = install::prepare(
            fixture.home.path(),
            fixture.approved.clone(),
            CancellationToken::new(),
            None,
        )
        .await
        .unwrap();
        assert_eq!(prepared.scan.verdict(), gray_plugin::scan::Verdict::Caution);
        assert!(prepared.scan.render().contains("credential_store_read"));
        drop(prepared);
        assert_clean(fixture.home.path());
    }

    #[tokio::test]
    async fn cancellation_during_staged_probe_cleans_up_before_return() {
        let marker = tempfile::tempdir().unwrap();
        let pid = marker.path().join("pid");
        let script = format!("#!/bin/sh\necho $$ > '{}'\nexec sleep 30\n", pid.display());
        let fixture = fixture(&script, None).await;
        let home = fixture.home.path().to_owned();
        let approved = fixture.approved.clone();
        let cancel = CancellationToken::new();
        let token = cancel.clone();
        let task =
            tokio::spawn(async move { install::prepare(&home, approved, token, None).await });
        wait_for_file(&pid).await;
        cancel.cancel();
        assert!(task.await.unwrap().is_err());
        assert_reaped(&pid);
        assert_clean(fixture.home.path());
    }

    #[tokio::test]
    async fn consented_commit_round_trips_host_lock_without_reprobing() {
        let fixture = fixture(COUNTER, None).await;
        let prepared = install::prepare(
            fixture.home.path(),
            fixture.approved.clone(),
            CancellationToken::new(),
            None,
        )
        .await
        .unwrap();
        let consent = install::policy::grant(&prepared.manifest.declared);
        let result = install::commit(prepared, consent.clone()).await.unwrap();
        assert!(result.installed);
        assert_eq!(result.name, "demo");
        assert!(result.warnings.is_empty(), "{:?}", result.warnings);
        let lock =
            gray_plugin::lock::LockFile::load(&gray_plugin::lock::lock_path(fixture.home.path()))
                .unwrap();
        let row = &lock.plugins["demo"];
        assert_eq!(
            row.capabilities_hash.as_deref(),
            Some(consent.hash.as_str())
        );
        assert_eq!(row.granted_capabilities, consent.granted);
        let executable = fixture.home.path().join("plugins/demo/plugin.sh");
        assert_eq!(Path::new(&row.argv[0]), executable);
        assert_eq!(row.cli_argv.as_ref().unwrap(), &row.argv);
        assert_eq!(
            std::fs::read_to_string(executable.with_extension("sh.count")).unwrap(),
            "x"
        );
        let metadata: serde_json::Value = serde_json::from_slice(
            &std::fs::read(fixture.home.path().join("plugins/demo-manifest.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(metadata["name"], "runtime-demo");
        assert_eq!(row.version, fixture.approved.entry().version);
        assert_eq!(row.hash, fixture.approved.entry().hash.primary().unwrap());
        assert_eq!(row.source, fixture.approved.entry().source.url);
        assert_eq!(lock.plugins.len(), 1);
    }

    #[tokio::test]
    async fn adoption_error_is_installed_with_warning_and_consent_intact() {
        let fixture = fixture(COUNTER, None).await;
        let prepared = install::prepare(
            fixture.home.path(),
            fixture.approved.clone(),
            CancellationToken::new(),
            None,
        )
        .await
        .unwrap();
        let consent = install::policy::grant(&prepared.manifest.declared);
        let result = install::commit_with(prepared, consent.clone(), |_, _, _, _, _| {
            anyhow::bail!("provider refresh failed")
        })
        .await
        .unwrap();
        assert!(result.installed);
        assert_eq!(result.warnings.len(), 1);
        assert!(result.warnings[0].contains("provider refresh failed"));
        assert!(fixture.home.path().join("plugins/demo/plugin.sh").is_file());
        let lock =
            gray_plugin::lock::LockFile::load(&gray_plugin::lock::lock_path(fixture.home.path()))
                .unwrap();
        assert_eq!(lock.plugins["demo"].capabilities_hash, Some(consent.hash));
        assert_eq!(lock.plugins["demo"].granted_capabilities, consent.granted);
    }

    #[tokio::test]
    async fn empty_consent_and_sidecar_args_are_committed_even_without_progress_receiver() {
        let script = COUNTER.replace(
            "\"capabilities\":[\"host.ask\"]",
            "\"capabilities\":[],\"sidecar_args\":[\"serve\",\"--test\"]",
        );
        let fixture = fixture(&script, None).await;
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        drop(rx);
        let prepared = install::prepare(
            fixture.home.path(),
            fixture.approved.clone(),
            CancellationToken::new(),
            Some(tx),
        )
        .await
        .unwrap();
        let result = install::commit(prepared, install::policy::grant(&[]))
            .await
            .unwrap();
        assert!(result.installed);
        let lock =
            gray_plugin::lock::LockFile::load(&gray_plugin::lock::lock_path(fixture.home.path()))
                .unwrap();
        let row = &lock.plugins["demo"];
        assert_eq!(
            row.capabilities_hash,
            Some(gray_plugin::capabilities::consent_hash(&[]))
        );
        assert!(row.granted_capabilities.is_empty());
        assert_eq!(&row.argv[1..], ["serve", "--test"]);
        assert_eq!(row.cli_argv.as_ref().unwrap(), &row.argv[..1]);
    }

    #[tokio::test]
    async fn mismatched_consent_drops_staging_without_commit() {
        for alter_hash in [true, false] {
            let fixture = fixture(COUNTER, None).await;
            let prepared = install::prepare(
                fixture.home.path(),
                fixture.approved.clone(),
                CancellationToken::new(),
                None,
            )
            .await
            .unwrap();
            let mut consent = install::policy::grant(&prepared.manifest.declared);
            if alter_hash {
                consent.hash.push('x');
            } else {
                consent.granted.push("invented".into());
            }
            let error = install::commit(prepared, consent).await.unwrap_err();
            assert!(error.to_string().contains("consent does not match"));
            assert_clean(fixture.home.path());
        }
    }

    #[tokio::test]
    async fn concurrent_existing_target_is_not_adopted_or_reported_as_new_install() {
        let fixture = fixture(COUNTER, None).await;
        let prepared = install::prepare(
            fixture.home.path(),
            fixture.approved.clone(),
            CancellationToken::new(),
            None,
        )
        .await
        .unwrap();
        let consent = install::policy::grant(&prepared.manifest.declared);
        let path = gray_plugin::lock::lock_path(fixture.home.path());
        let mut lock = gray_plugin::lock::LockFile::load(&path).unwrap();
        lock.plugins.insert(
            "demo".into(),
            gray_plugin::lock::LockEntry {
                version: "concurrent".into(),
                ..Default::default()
            },
        );
        lock.save(&path).unwrap();
        let result = install::commit_with(prepared, consent, |_, _, _, _, _| {
            panic!("a concurrent install must never adopt our staged manifest")
        })
        .await
        .unwrap();
        assert!(!result.installed);
        assert_eq!(result.name, "demo");
        assert!(result.warnings.is_empty());
        assert_eq!(gray_plugin::lock::LockFile::load(&path).unwrap(), lock);
        assert!(!fixture.home.path().join("plugins/demo").exists());
        assert!(
            !fixture
                .home
                .path()
                .join("plugins/demo-manifest.json")
                .exists()
        );
    }

    #[tokio::test]
    async fn wire_commit_publishes_manifest_and_grants_only_known_ids_without_reprobing() {
        let script = WIRE.replace("while IFS=", "printf x >> \"$0.count\"\nwhile IFS=");
        let fixture = fixture(&script, None).await;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let prepared = install::prepare(
            fixture.home.path(),
            fixture.approved.clone(),
            CancellationToken::new(),
            Some(tx),
        )
        .await
        .unwrap();
        let consent = install::policy::grant(&prepared.manifest.declared);
        assert_eq!(consent.granted, ["host.ask"]);
        let result = install::commit(prepared, consent).await.unwrap();
        assert!(result.installed);
        let mut stages = Vec::new();
        while let Some(stage) = rx.recv().await {
            stages.push(stage);
        }
        assert_eq!(stages.last(), Some(&InstallStage::Committing));
        let lock =
            gray_plugin::lock::LockFile::load(&gray_plugin::lock::lock_path(fixture.home.path()))
                .unwrap();
        assert!(lock.plugins["demo"].cli_argv.is_none());
        assert_eq!(
            std::fs::read_to_string(fixture.home.path().join("plugins/demo/plugin.sh.count"))
                .unwrap(),
            "x"
        );
        let manifest: serde_json::Value = serde_json::from_slice(
            &std::fs::read(fixture.home.path().join("plugins/demo-manifest.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            manifest["capabilities"],
            serde_json::json!(["host.ask", "invented"])
        );
    }

    #[tokio::test]
    async fn provider_role_and_discovery_reuse_prepared_manifest() {
        let raw = serde_json::json!({
            "name": "runtime-provider", "version": "runtime-1", "protocol": gray_plugin::PROVIDER_PROTOCOL,
            "capabilities": [gray_plugin::PROVIDER_CREDENTIALS],
            "providers": [{
                "id": "prov", "name": "Prov",
                "transport": {"kind": "openai-responses", "base_url": "https://127.0.0.1:1/",
                    "authorization": {"kind": "bearer", "secret_name": "k"}},
                "auth_methods": [{"id": "a", "kind": "api_key", "name": "A", "operations": ["models"]}]
            }]
        });
        let script = format!("#!/bin/sh\nprintf x >> \"$0.count\"\nprintf '%s\\n' '{raw}'\n");
        let fixture = fixture(&script, None).await;
        let prepared = install::prepare(
            fixture.home.path(),
            fixture.approved.clone(),
            CancellationToken::new(),
            None,
        )
        .await
        .unwrap();
        let consent = install::policy::grant(&prepared.manifest.declared);
        let result = install::commit(prepared, consent).await.unwrap();
        assert!(result.installed);
        assert!(result.warnings.is_empty(), "{:?}", result.warnings);
        let lock =
            gray_plugin::lock::LockFile::load(&gray_plugin::lock::lock_path(fixture.home.path()))
                .unwrap();
        assert_eq!(
            lock.plugins["demo"].runtime_role.as_deref(),
            Some("provider_only")
        );
        let registry = crate::providers::ProviderRegistry::load_cached(fixture.home.path());
        assert_eq!(registry.cache().plugins["demo"].providers[0].id, "prov");
        assert_eq!(
            std::fs::read_to_string(fixture.home.path().join("plugins/demo/plugin.sh.count"))
                .unwrap(),
            "x"
        );
    }

    #[tokio::test]
    async fn approved_entry_ignores_shadow_paths_in_isolated_process() {
        const ENV: &str = "GRAY_MARKETPLACE_SHADOW_FIXTURE";
        if let Some(shadow) = std::env::var_os(ENV) {
            let fixture = fixture(COUNTER, None).await;
            let prepared = install::prepare(
                fixture.home.path(),
                fixture.approved.clone(),
                CancellationToken::new(),
                None,
            )
            .await
            .unwrap();
            // A later catalog snapshot cannot change the already-prepared package.
            let mut changed = fixture.approved.entry().clone();
            changed.version = "catalog-2".into();
            changed.source.url = "https://example.invalid/replaced.tgz".into();
            changed.hash = serde_json::from_value(serde_json::json!("sha256:changed")).unwrap();
            let (url, server) = serve_bytes(
                serde_json::to_vec(&serde_json::json!({
                    "plugins": {"demo": changed}
                }))
                .unwrap(),
            )
            .await;
            let refreshed: gray_pkg::index::Index = gray_pkg::fetch::client()
                .unwrap()
                .get(url)
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            server.await.unwrap();
            let changed =
                ApprovedEntry::new("demo".into(), refreshed.plugins["demo"].clone()).unwrap();
            let consent = install::policy::grant(&prepared.manifest.declared);
            assert!(install::commit(prepared, consent).await.unwrap().installed);
            let lock = gray_plugin::lock::LockFile::load(&gray_plugin::lock::lock_path(
                fixture.home.path(),
            ))
            .unwrap();
            assert_eq!(
                lock.plugins["demo"].version,
                fixture.approved.entry().version
            );
            assert_ne!(lock.plugins["demo"].version, changed.entry().version);
            assert_eq!(
                lock.plugins["demo"].source,
                fixture.approved.entry().source.url
            );
            assert_eq!(
                lock.plugins["demo"].hash,
                fixture.approved.entry().hash.primary().unwrap()
            );
            for entry in std::fs::read_dir(shadow).unwrap() {
                assert!(
                    !entry
                        .unwrap()
                        .path()
                        .to_string_lossy()
                        .ends_with(".invoked")
                );
            }
            return;
        }
        // Scope environment changes to a subprocess, not the parallel test runner.
        let shadow = tempfile::tempdir().unwrap();
        let exe = executable(
            shadow.path(),
            "#!/bin/sh\nprintf x >> \"$0.invoked\"\nexit 1\n",
        );
        std::fs::copy(&exe, shadow.path().join("demo")).unwrap();
        std::fs::copy(&exe, shadow.path().join("gray-demo")).unwrap();
        let mut paths = vec![shadow.path().to_owned()];
        paths.extend(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        ));
        let output = tokio::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "plugin_install::tests::staged::approved_entry_ignores_shadow_paths_in_isolated_process"])
            .env(ENV, shadow.path()).env("GRAY_PLUGIN_PATH", shadow.path())
            .env("PATH", std::env::join_paths(paths).unwrap())
            .output().await.unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
    }
}
