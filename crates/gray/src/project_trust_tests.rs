use super::*;

fn repo(root: &Path) {
    std::fs::create_dir_all(root.join(".git")).unwrap();
}

#[test]
fn nothing_is_trusted_until_asked() {
    let home = tempfile::tempdir().unwrap();
    let proj = tempfile::tempdir().unwrap();
    assert!(!is_trusted(home.path(), proj.path()));
}

#[test]
fn trust_then_untrust_round_trips() {
    let home = tempfile::tempdir().unwrap();
    let proj = tempfile::tempdir().unwrap();
    trust(home.path(), proj.path()).unwrap();
    assert!(is_trusted(home.path(), proj.path()));
    assert!(untrust(home.path(), proj.path()).unwrap());
    assert!(!is_trusted(home.path(), proj.path()));
    assert!(!untrust(home.path(), proj.path()).unwrap());
}

#[test]
fn trusting_a_repo_covers_its_subdirectories_but_not_siblings() {
    let home = tempfile::tempdir().unwrap();
    let base = tempfile::tempdir().unwrap();
    let root = base.path().join("repo");
    repo(&root);
    std::fs::create_dir_all(root.join("src/deep")).unwrap();
    let other = base.path().join("other");
    std::fs::create_dir_all(&other).unwrap();

    trust(home.path(), &root.join("src/deep")).unwrap();
    assert!(is_trusted(home.path(), &root));
    assert!(is_trusted(home.path(), &root.join("src")));
    assert!(!is_trusted(home.path(), &other));
}

#[test]
fn a_corrupt_list_trusts_nothing() {
    let home = tempfile::tempdir().unwrap();
    let proj = tempfile::tempdir().unwrap();
    std::fs::write(home.path().join(FILE), "{not json").unwrap();
    assert!(!is_trusted(home.path(), proj.path()));
    assert!(trusted(home.path()).is_empty());
}

#[test]
fn trusting_twice_records_the_project_once() {
    let home = tempfile::tempdir().unwrap();
    let proj = tempfile::tempdir().unwrap();
    trust(home.path(), proj.path()).unwrap();
    trust(home.path(), proj.path()).unwrap();
    assert_eq!(trusted(home.path()).len(), 1);
}

#[test]
fn project_skills_load_only_once_the_project_is_trusted() {
    let home = tempfile::tempdir().unwrap();
    let proj = tempfile::tempdir().unwrap();
    repo(proj.path());
    let skill = proj.path().join(".gray/skills/commit");
    std::fs::create_dir_all(&skill).unwrap();
    std::fs::write(
        skill.join("SKILL.md"),
        "---\nname: commit\ndescription: commit changes\n---\nBody",
    )
    .unwrap();
    let names = |home: &Path| {
        crate::skills::load_skills(proj.path(), home)
            .skills
            .into_iter()
            .map(|s| s.name)
            .collect::<Vec<_>>()
    };
    assert!(!names(home.path()).contains(&"commit".to_string()));
    trust(home.path(), proj.path()).unwrap();
    assert!(names(home.path()).contains(&"commit".to_string()));
}
