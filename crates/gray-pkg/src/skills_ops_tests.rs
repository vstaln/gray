#![allow(clippy::await_holding_lock)]
use super::*;

const FIXTURE_SKILL: &str = "---\nname: demo-skill\ndescription: demo skill for tests\n---\nBody\n";

/// Point `GRAY_HOME` at a fresh tempdir. Must run under `ENV_GUARD`.
fn use_skills_env() -> tempfile::TempDir {
    let home = tempfile::tempdir().unwrap();
    // SAFETY: serialized by ENV_GUARD.
    unsafe {
        std::env::set_var("GRAY_HOME", home.path());
    }
    home
}

/// Fixture bundle: `<root>/demo-skill/{SKILL.md, references/notes.md}`.
fn fixture_bundle() -> (tempfile::TempDir, PathBuf) {
    let root = tempfile::tempdir().unwrap();
    let bundle = root.path().join("demo-skill");
    std::fs::create_dir_all(bundle.join("references")).unwrap();
    std::fs::write(bundle.join("SKILL.md"), FIXTURE_SKILL).unwrap();
    std::fs::write(bundle.join("references/notes.md"), "# notes\n").unwrap();
    (root, bundle)
}

#[test]
fn spec_identity_routes_each_scheme() {
    assert_eq!(
        spec_identity("clawhub:owner/slug"),
        ("clawhub".to_string(), "clawhub:owner/slug".to_string())
    );
    assert_eq!(
        spec_identity("claude:grep-skills"),
        ("claude".to_string(), "claude:grep-skills".to_string())
    );
    assert_eq!(
        spec_identity("claude:grep-skills@dogfood-fix"),
        (
            "claude".to_string(),
            "claude:grep-skills@dogfood-fix".to_string()
        )
    );
    assert_eq!(
        spec_identity("github:owner/repo"),
        ("github".to_string(), "github:owner/repo".to_string())
    );
    assert_eq!(
        spec_identity("url:https://h/x-skill/SKILL.md"),
        (
            "url".to_string(),
            "url:https://h/x-skill/SKILL.md".to_string()
        )
    );
    assert_eq!(
        spec_identity("/tmp/demo-skill"),
        ("local".to_string(), "/tmp/demo-skill".to_string())
    );
}

#[test]
fn frontmatter_parses_name_and_description() {
    assert_eq!(
        parse_skill_frontmatter(FIXTURE_SKILL),
        ("demo-skill".to_string(), "demo skill for tests".to_string())
    );
    assert_eq!(
        parse_skill_frontmatter("---\nname: 'q'\ndescription: \"d\"\n---\n"),
        ("q".to_string(), "d".to_string())
    );
    // Absent/unclosed frontmatter degrades to empty (callers refuse).
    assert_eq!(
        parse_skill_frontmatter("no frontmatter\n"),
        (String::new(), String::new())
    );
    assert_eq!(
        parse_skill_frontmatter("---\nname: x\n"),
        (String::new(), String::new())
    );
}

#[test]
fn github_spec_parses_and_rejects() {
    assert_eq!(
        parse_github_spec("owner/repo").unwrap(),
        ("owner".to_string(), "repo".to_string(), None)
    );
    assert_eq!(
        parse_github_spec("owner/repo/path/to/skill").unwrap(),
        (
            "owner".to_string(),
            "repo".to_string(),
            Some("path/to/skill".to_string())
        )
    );
    for bad in ["", "justname", "a/../b", "a/./b", "a//b", "a/b\\c"] {
        assert!(parse_github_spec(bad).is_err(), "accepted {bad:?}");
    }
}

#[test]
fn url_slugs_derive_from_skill_path() {
    assert_eq!(
        slug_from_url("https://h/skills/my-skill/SKILL.md").unwrap(),
        "my-skill"
    );
    assert_eq!(
        slug_from_url("http://127.0.0.1:9/x-skill/SKILL.md").unwrap(),
        "x-skill"
    );
    assert_eq!(slug_from_url("https://h/x/foo.md?tok=1").unwrap(), "foo");
    assert!(slug_from_url("https://h/").is_err());
}

#[tokio::test]
async fn install_local_copies_bundle_and_writes_origin() {
    let _guard = crate::ops::tests::ENV_GUARD.lock().unwrap();
    let (_root, bundle) = fixture_bundle();
    let _home = use_skills_env();

    let report = install(bundle.to_str().unwrap()).await.unwrap();
    assert_eq!(report.name, "demo-skill");
    assert_eq!(report.version, "0.0.0");
    let dest = skills_dir().join("demo-skill");
    assert_eq!(report.path, dest);
    assert_eq!(
        std::fs::read(dest.join("SKILL.md")).unwrap(),
        FIXTURE_SKILL.as_bytes()
    );
    assert_eq!(
        std::fs::read(dest.join("references/notes.md")).unwrap(),
        b"# notes\n"
    );
    let origin: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dest.join(ORIGIN_FILE)).unwrap()).unwrap();
    assert_eq!(origin["version"], 1);
    assert_eq!(origin["registry"], "local");
    assert_eq!(origin["slug"], "demo-skill");
    assert_eq!(origin["installed_version"], "0.0.0");
    assert!(!origin["installed_at"].as_str().unwrap_or("").is_empty());
    assert_eq!(origin["source_url"], bundle.to_str().unwrap());
}

#[tokio::test]
async fn install_from_skill_md_file_uses_parent_bundle() {
    let _guard = crate::ops::tests::ENV_GUARD.lock().unwrap();
    let (_root, bundle) = fixture_bundle();
    let _home = use_skills_env();

    let skill_md = bundle.join("SKILL.md");
    let report = install(skill_md.to_str().unwrap()).await.unwrap();
    assert_eq!(report.name, "demo-skill");
    assert!(report.path.join("references/notes.md").is_file());
}

#[tokio::test]
async fn list_round_trips_install() {
    let _guard = crate::ops::tests::ENV_GUARD.lock().unwrap();
    let (_root, bundle) = fixture_bundle();
    let _home = use_skills_env();
    assert!(list().unwrap().is_empty());

    install(bundle.to_str().unwrap()).await.unwrap();
    let skills = list().unwrap();
    assert_eq!(skills.len(), 1);
    assert_eq!(skills[0].name, "demo-skill");
    assert_eq!(skills[0].version, "0.0.0");
    assert_eq!(skills[0].source, "local");
    let origin = skills[0].origin.as_ref().expect("origin");
    assert_eq!(origin.slug, "demo-skill");
    assert_eq!(origin.installed_version, "0.0.0");
}

#[tokio::test]
async fn remove_deletes_dir_and_miss_matches_ops_voice() {
    let _guard = crate::ops::tests::ENV_GUARD.lock().unwrap();
    let (_root, bundle) = fixture_bundle();
    let _home = use_skills_env();

    install(bundle.to_str().unwrap()).await.unwrap();
    remove("demo-skill").unwrap();
    assert!(list().unwrap().is_empty());
    assert!(!skills_dir().join("demo-skill").exists());
    for bad in ["demo-skill", "../evil", "a/b", ""] {
        let err = remove(bad).unwrap_err();
        assert_eq!(err.to_string(), format!("not installed: {bad}"));
    }
}

#[tokio::test]
async fn install_refuses_bundle_without_skill_md() {
    let _guard = crate::ops::tests::ENV_GUARD.lock().unwrap();
    let root = tempfile::tempdir().unwrap();
    let bundle = root.path().join("empty-skill");
    std::fs::create_dir_all(&bundle).unwrap();
    std::fs::write(bundle.join("notes.txt"), "no skill here\n").unwrap();
    let _home = use_skills_env();

    let err = install(bundle.to_str().unwrap())
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("no SKILL.md"), "clear reason: {err}");
    assert!(!skills_dir().join("empty-skill").exists());
    assert!(list().unwrap().is_empty());
}

#[tokio::test]
async fn install_refuses_missing_description() {
    let _guard = crate::ops::tests::ENV_GUARD.lock().unwrap();
    let root = tempfile::tempdir().unwrap();
    let bundle = root.path().join("nodesc");
    std::fs::create_dir_all(&bundle).unwrap();
    std::fs::write(bundle.join("SKILL.md"), "---\nname: nodesc\n---\nBody\n").unwrap();
    let _home = use_skills_env();

    let err = install(bundle.to_str().unwrap())
        .await
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("description"),
        "names the missing field: {err}"
    );
    assert!(!skills_dir().join("nodesc").exists());
}

#[tokio::test]
async fn install_refuses_missing_name() {
    let _guard = crate::ops::tests::ENV_GUARD.lock().unwrap();
    let root = tempfile::tempdir().unwrap();
    let bundle = root.path().join("noname");
    std::fs::create_dir_all(&bundle).unwrap();
    std::fs::write(
        bundle.join("SKILL.md"),
        "---\ndescription: has desc\n---\nBody\n",
    )
    .unwrap();
    let _home = use_skills_env();

    let err = install(bundle.to_str().unwrap())
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("'name'"), "names the missing field: {err}");
    assert!(!skills_dir().join("noname").exists());
}

#[tokio::test]
async fn install_warns_not_refuses_on_name_dir_mismatch() {
    let _guard = crate::ops::tests::ENV_GUARD.lock().unwrap();
    let root = tempfile::tempdir().unwrap();
    let bundle = root.path().join("dir-name");
    std::fs::create_dir_all(&bundle).unwrap();
    std::fs::write(
        bundle.join("SKILL.md"),
        "---\nname: other-name\ndescription: d\n---\nBody\n",
    )
    .unwrap();
    let _home = use_skills_env();

    // pi leniency: installs under the dir slug, warning on stderr.
    let report = install(bundle.to_str().unwrap()).await.unwrap();
    assert_eq!(report.name, "dir-name");
    assert!(report.path.join("SKILL.md").is_file());
}

#[tokio::test]
async fn install_url_downloads_skill_md_over_loopback() {
    let _guard = crate::ops::tests::ENV_GUARD.lock().unwrap();
    use axum::{Router, routing::get};
    let router = Router::new().route("/x-skill/SKILL.md", get(|| async { FIXTURE_SKILL }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let _home = use_skills_env();

    // Fixture frontmatter names `demo-skill` while the URL slug is
    // `x-skill`: mismatch warns, install lands on the URL slug.
    let report = install(&format!("url:http://127.0.0.1:{port}/x-skill/SKILL.md"))
        .await
        .unwrap();
    assert_eq!(report.name, "x-skill");
    assert_eq!(
        std::fs::read(report.path.join("SKILL.md")).unwrap(),
        FIXTURE_SKILL.as_bytes()
    );
    let skills = list().unwrap();
    assert_eq!(skills.len(), 1);
    assert_eq!(skills[0].source, "url");
}
