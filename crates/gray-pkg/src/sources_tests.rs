#![allow(clippy::await_holding_lock)]
use super::*;

#[test]
fn source_labels_are_exact() {
    assert_eq!(Source::GrayIndex.label(), "Gray Index");
    assert_eq!(Source::PiGallery.label(), "Pi Gallery (preview)");
    assert_eq!(Source::ClawHub.label(), "ClawHub");
    assert_eq!(Source::ClaudeRepo.label(), "Claude");
}

#[test]
fn clawhub_key_input_qualifies_owner() {
    assert_eq!(clawhub_key_input("arein", "test"), "arein/test");
    assert_eq!(clawhub_key_input("", "test"), "test");
    // Downstream sanitize turns it into the collision-free lock key.
    assert_eq!(
        crate::ops::sanitize_npm_key(&clawhub_key_input("arein", "test")),
        "arein-test"
    );
}

#[test]
fn clawhub_slug_split_cases() {
    assert_eq!(
        split_clawhub_slug("arein/test"),
        (Some("arein".to_string()), "test".to_string())
    );
    assert_eq!(split_clawhub_slug("test"), (None, "test".to_string()));
    assert_eq!(split_clawhub_slug("  "), (None, String::new()));
}

#[test]
fn marketplace_fixture_covers_all_six_plus_command() {
    let raw = r#"{
            "name": "fixture-market",
            "plugins": [
                {"name":"p-path","description":"d","version":"1.0.0","source":"./plugins/p-path"},
                {"name":"p-github","description":"d","source":{"source":"github","repo":"o/r","ref":"main","sha":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}},
                {"name":"p-url","description":"d","source":{"source":"url","url":"https://example.com/r.git"}},
                {"name":"p-subdir","description":"d","source":{"source":"git-subdir","url":"o/mono","path":"tools/p"}},
                {"name":"p-npm","description":"d","source":{"source":"npm","package":"@o/p","version":"2.0.0"}},
                {"name":"p-archive","description":"d","source":{"source":"archive","url":"https://example.com/p.zip","sha256":"abc123"}},
                {"name":"p-command","description":"d","source":{"source":"command","command":"make plugin"}},
                {"name":"","description":"nameless dropped","source":"./x"},
                {"name":"p-bogus","description":"unknown shape dropped","source":{"source":"teleport"}}
            ]
        }"#;
    let cat = parse_marketplace_json(raw).unwrap();
    assert_eq!(cat.name, "fixture-market");
    assert_eq!(cat.plugins.len(), 7);
    assert!(matches!(cat.plugins[0].source, PluginSource::Path(_)));
    assert!(matches!(cat.plugins[1].source, PluginSource::Github { .. }));
    assert!(matches!(cat.plugins[2].source, PluginSource::Url { .. }));
    assert!(matches!(
        cat.plugins[3].source,
        PluginSource::GitSubdir { .. }
    ));
    assert!(matches!(cat.plugins[4].source, PluginSource::Npm { .. }));
    assert!(matches!(
        cat.plugins[5].source,
        PluginSource::Archive { .. }
    ));
    // `command` parses but is never executed: the install arm refuses it.
    assert!(matches!(
        cat.plugins[6].source,
        PluginSource::Command { .. }
    ));
    assert_eq!(claude_qualifier(&cat.plugins[4].source), "npm:@o/p@2.0.0");
}

#[test]
fn marketplace_specs_parse_and_reject() {
    assert_eq!(
        marketplace_repo_parts("anthropics/claude-plugins-official").unwrap(),
        (
            "anthropics".to_string(),
            "claude-plugins-official".to_string()
        )
    );
    assert!(marketplace_repo_parts("justname").is_err());
    assert!(marketplace_repo_parts("a/b/c").is_err());
    assert!(marketplace_repo_parts("https://github.com/a/b").is_err());
}

#[test]
fn claude_marketplaces_env_override() {
    let _guard = crate::ops::tests::env_guard();
    // SAFETY: serialized by ENV_GUARD.
    unsafe {
        std::env::remove_var(CLAUDE_MARKETPLACES_ENV);
    }
    assert_eq!(claude_marketplaces().len(), 2);
    // SAFETY: serialized by ENV_GUARD.
    unsafe {
        std::env::set_var(CLAUDE_MARKETPLACES_ENV, "foo/bar, file:///tmp/x ");
    }
    assert_eq!(claude_marketplaces(), vec!["foo/bar", "file:///tmp/x"]);
    // SAFETY: serialized by ENV_GUARD.
    unsafe {
        std::env::remove_var(CLAUDE_MARKETPLACES_ENV);
    }
}

#[test]
fn local_marketplace_dir_detects_dirs_only() {
    let dir = tempfile::tempdir().unwrap();
    assert!(local_marketplace_dir(dir.path().to_str().unwrap()).is_some());
    assert!(local_marketplace_dir(&format!("file://{}", dir.path().display())).is_some());
    assert!(local_marketplace_dir("anthropics/claude-plugins-official").is_none());
    assert!(local_marketplace_dir("").is_none());
}

#[tokio::test]
async fn git_source_checks_out_stale_pin() {
    // Upstream moved past the catalog pin: resolve must check out the
    // pinned commit (not fail), fully offline over file://.
    // SAFETY: serialized by ENV_GUARD (process-global env).
    let _guard = crate::ops::tests::env_guard();
    let home = tempfile::tempdir().unwrap();
    unsafe {
        std::env::set_var("GRAY_HOME", home.path());
    }
    let dir = tempfile::tempdir().unwrap();
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
    std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
    run(&["init", "-q"]);
    run(&["add", "-A"]);
    run(&["commit", "-qm", "one"]);
    let out = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let pin = String::from_utf8(out.stdout).unwrap().trim().to_string();
    std::fs::write(dir.path().join("b.txt"), "two\n").unwrap();
    run(&["add", "-A"]);
    run(&["commit", "-qm", "two"]);
    let url = format!("file://{}", dir.path().display());
    let p = MarketplacePlugin {
        name: "pin-test".to_string(),
        description: String::new(),
        version: String::new(),
        source: PluginSource::Path("x".to_string()),
    };
    let resolved = install_git_source(&url, "", &pin, None, &p, "pin-test")
        .await
        .expect("stale pin must resolve via checkout");
    assert_eq!(resolved.hash, pin);
    assert!(!resolved.unverified);
    assert!(resolved.root.join("a.txt").is_file());
    assert!(!resolved.root.join("b.txt").exists());
}

#[tokio::test]
async fn status_never_hard_fails() {
    // Closed loopback port: every source reports false, none panics.
    // SAFETY: serialized by ENV_GUARD (process-global env).
    let _guard = crate::ops::tests::env_guard();
    unsafe {
        std::env::set_var(CLAUDE_MARKETPLACES_ENV, "127.0.0.1:9/none");
        std::env::set_var(CLAWHUB_BASE_ENV, "http://127.0.0.1:9");
        std::env::set_var(crate::ops::NPM_REGISTRY_ENV, "http://127.0.0.1:9");
        std::env::set_var(crate::index::INDEX_URL_ENV, "http://127.0.0.1:9/i.json");
    }
    assert!(!status(Source::GrayIndex).await);
    assert!(!status(Source::PiGallery).await);
    assert!(!status(Source::ClawHub).await);
    assert!(!status(Source::ClaudeRepo).await);
    unsafe {
        std::env::remove_var(CLAUDE_MARKETPLACES_ENV);
        std::env::remove_var(CLAWHUB_BASE_ENV);
        std::env::remove_var(crate::ops::NPM_REGISTRY_ENV);
        std::env::remove_var(crate::index::INDEX_URL_ENV);
    }
}
