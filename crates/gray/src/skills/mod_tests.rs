use super::*;

fn test_skill(name: &str, args: &[&str]) -> Skill {
    Skill {
        name: name.to_string(),
        description: "test".to_string(),
        file_path: PathBuf::from("/tmp/SKILL.md"),
        base_dir: PathBuf::from("/tmp"),
        disable_model_invocation: false,
        source: "path".to_string(),
        args: args.iter().map(|s| s.to_string()).collect(),
    }
}

#[test]
fn undeclared_args_pass_raw_text_through() {
    let s = test_skill("ponytail", &[]);
    assert!(validate_skill_args(&s, None).is_ok());
    assert!(validate_skill_args(&s, Some("")).is_ok());
    assert!(validate_skill_args(&s, Some("   ")).is_ok());
    assert!(validate_skill_args(&s, Some("ultra")).is_ok());
    assert!(validate_skill_args(&s, Some("ultra --fast x=1")).is_ok());
}

#[test]
fn declared_args_reject_unknown_naming_valid() {
    let s = test_skill("deploy", &["env", "force"]);
    assert!(validate_skill_args(&s, None).is_ok());
    assert!(validate_skill_args(&s, Some("")).is_ok());
    assert!(validate_skill_args(&s, Some("env")).is_ok());
    assert!(validate_skill_args(&s, Some("env force")).is_ok());
    assert!(validate_skill_args(&s, Some("--env --force")).is_ok());
    let err = validate_skill_args(&s, Some("bogus")).unwrap_err();
    assert!(err.contains("bogus"), "names unknown: {err}");
    assert!(err.contains("env"), "names valid: {err}");
    assert!(err.contains("force"), "names valid: {err}");
}

fn ranked_skill(name: &str, desc: &str) -> Skill {
    Skill {
        name: name.to_string(),
        description: desc.to_string(),
        file_path: PathBuf::from("/tmp/SKILL.md"),
        base_dir: PathBuf::from("/tmp"),
        disable_model_invocation: false,
        source: "path".to_string(),
        args: vec![],
    }
}

#[test]
fn rank_keeps_exact_names_rejects_weak_entries() {
    let skills = vec![
        ranked_skill("deploy", "deploy the app"),
        ranked_skill("deploy", "duplicate name loses"),
        ranked_skill("empty", ""),
        ranked_skill("blob", &"x".repeat(701)),
        ranked_skill("", "nameless loses"),
    ];
    let ranked = rank_skills_for_prompt(&skills, true, &BTreeSet::new());
    assert_eq!(ranked.len(), 1);
    assert_eq!(ranked[0].name, "deploy");
    assert_eq!(ranked[0].description, "deploy the app");
}

#[test]
fn rank_skips_user_disabled_skills() {
    let skills = vec![
        ranked_skill("keep", "keep me"),
        ranked_skill("off", "turned off"),
    ];
    let disabled: BTreeSet<String> = ["off".to_string()].into_iter().collect();
    let ranked = rank_skills_for_prompt(&skills, true, &disabled);
    assert_eq!(ranked.len(), 1);
    assert_eq!(ranked[0].name, "keep");
    let out = format_skills_for_prompt(&skills, true, &disabled);
    assert!(out.contains("<name>keep</name>"), "{out}");
    assert!(!out.contains("<name>off</name>"), "{out}");

    let none = BTreeSet::new();
    assert_eq!(rank_skills_for_prompt(&skills, true, &none).len(), 2);

    assert!(rank_skills_for_prompt(&skills, false, &none).is_empty());
    assert!(format_skills_for_prompt(&skills, false, &none).is_empty());
    assert!(format_skills_for_prompt(&skills, false, &disabled).is_empty());
}

#[test]
fn prompt_block_gives_proactive_skill_guidance() {
    let s = test_skill("anything", &[]);
    let out = format_skills_for_prompt(&[s], true, &BTreeSet::new());
    assert!(
        out.contains("`cat` its SKILL.md"),
        "load hint missing: {out}"
    );
    assert!(out.contains("<available_skills>"));
}

#[test]
fn prompt_block_caps_skill_list_and_says_so() {
    let skills: Vec<Skill> = (0..60)
        .map(|i| test_skill(&format!("skill-{i:02}"), &[]))
        .collect();
    let out = format_skills_for_prompt(&skills, true, &BTreeSet::new());
    assert_eq!(
        out.matches("<skill>").count(),
        12,
        "prompt list must be capped"
    );
    assert!(
        out.contains("more skills"),
        "omission must be visible to the model: {out}"
    );
}

#[test]
fn prompt_block_under_cap_lists_all_without_notice() {
    let skills: Vec<Skill> = (0..3)
        .map(|i| test_skill(&format!("skill-{i}"), &[]))
        .collect();
    let out = format_skills_for_prompt(&skills, true, &BTreeSet::new());
    assert_eq!(out.matches("<skill>").count(), 3);
    assert!(
        !out.contains("more skills"),
        "no notice under the cap: {out}"
    );
}

#[test]
fn prompt_cap_counts_only_eligible_unique_skills() {
    let mut skills = vec![test_skill("duplicate", &[]); 50];
    let mut disabled = test_skill("disabled", &[]);
    disabled.disable_model_invocation = true;
    skills.push(disabled);
    skills.push(ranked_skill("empty", ""));
    skills.extend((0..39).map(|i| test_skill(&format!("skill-{i}"), &[])));
    let out = format_skills_for_prompt(&skills, true, &BTreeSet::new());
    assert_eq!(out.matches("<skill>").count(), 12);
    assert!(out.contains("more skills"));
    assert!(!out.contains("<name>disabled</name>"));
    assert!(!out.contains("<name>empty</name>"));
    assert_eq!(
        rank_skills_for_prompt(&skills, true, &BTreeSet::new()).len(),
        40
    );
    assert!(format_skills_for_prompt(&[], true, &BTreeSet::new()).is_empty());
}

#[test]
fn claude_user_skills_and_latest_plugin_dirs_are_roots() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let agent = tempfile::tempdir().unwrap();

    let claude_skills = home.path().join(".claude").join("skills");
    fs::create_dir_all(&claude_skills).unwrap();

    let install = home.path().join("cache/mkt/plug/2.0.0");
    let stale = home.path().join("cache/mkt/plug/1.0.0");
    fs::create_dir_all(install.join("skills")).unwrap();
    fs::create_dir_all(stale.join("skills")).unwrap();
    let plugins = home.path().join(".claude").join("plugins");
    fs::create_dir_all(&plugins).unwrap();
    fs::write(
        plugins.join("installed_plugins.json"),
        serde_json::json!({
            "version": 2,
            "plugins": {
                "plug@mkt": [
                    {"installPath": stale.display().to_string(), "version": "1.0.0", "lastUpdated": "2026-01-01T00:00:00Z"},
                    {"installPath": install.display().to_string(), "version": "2.0.0", "lastUpdated": "2026-02-01T00:00:00Z"}
                ]
            }
        })
        .to_string(),
    )
    .unwrap();

    let roots = skill_search_roots_under(cwd.path(), agent.path(), Some(home.path()));
    assert!(roots.contains(&(claude_skills, "user")), "{roots:?}");
    assert!(
        roots.contains(&(install.join("skills"), "plugin")),
        "{roots:?}"
    );
    assert!(
        !roots.iter().any(|(p, _)| p.starts_with(&stale)),
        "stale plugin versions must not be scanned: {roots:?}"
    );
}

#[test]
fn disabled_claude_plugins_are_skipped() {
    let home = tempfile::tempdir().unwrap();
    let install = home.path().join("cache/mkt/plug/1.0.0");
    fs::create_dir_all(install.join("skills")).unwrap();
    let claude = home.path().join(".claude");
    fs::create_dir_all(claude.join("plugins")).unwrap();
    fs::write(
        claude.join("plugins").join("installed_plugins.json"),
        serde_json::json!({
            "plugins": {
                "off@mkt": [{"installPath": install.display().to_string(), "version": "1.0.0"}]
            }
        })
        .to_string(),
    )
    .unwrap();

    assert_eq!(
        claude_plugin_skill_dirs(home.path()),
        vec![install.join("skills")],
        "installed plugins are enabled unless opted out"
    );

    fs::write(
        claude.join("settings.json"),
        r#"{"enabledPlugins":{"off@mkt":false}}"#,
    )
    .unwrap();
    assert!(claude_plugin_skill_dirs(home.path()).is_empty());
}

#[test]
fn project_claude_skills_root_is_discovered_when_trusted() {
    crate::project_trust::assume_trusted_for_test();
    let proj = tempfile::tempdir().unwrap();
    let nested = proj.path().join("sub");
    let claude_skills = proj.path().join(".claude").join("skills");
    fs::create_dir_all(&claude_skills).unwrap();
    fs::create_dir_all(&nested).unwrap();
    let agent = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let roots = skill_search_roots_under(&nested, agent.path(), Some(home.path()));
    assert!(roots.contains(&(claude_skills, "project")), "{roots:?}");
}
