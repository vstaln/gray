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
fn no_args_skill_rejects_any_arg() {
    let s = test_skill("deploy", &[]);
    assert!(validate_skill_args(&s, None).is_ok());
    assert!(validate_skill_args(&s, Some("")).is_ok());
    assert!(validate_skill_args(&s, Some("   ")).is_ok());
    let err = validate_skill_args(&s, Some("bogus-args")).unwrap_err();
    assert!(err.contains("deploy"), "names skill: {err}");
    assert!(err.contains("(none)"), "names valid args: {err}");
}

#[test]
fn declared_args_reject_unknown_naming_valid() {
    let s = test_skill("deploy", &["env", "force"]);
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
    let ranked = rank_skills_for_prompt(&skills);
    assert_eq!(ranked.len(), 1);
    assert_eq!(ranked[0].name, "deploy");
    assert_eq!(ranked[0].description, "deploy the app");
}

#[test]
fn prompt_block_routes_trivial_work_away_from_skills() {
    let s = test_skill("anything", &[]);
    let out = format_skills_for_prompt(&[s]);
    assert!(
        out.contains("trivial single-step"),
        "routing hint missing: {out}"
    );
    assert!(out.contains("<available_skills>"));
}

#[test]
fn pi_plugin_dir_is_a_discovery_root() {
    // P2-2: `<agent_dir>/plugins/pi/<pkg>/<skill>/SKILL.md` (where pi
    // installs land) must surface via default discovery.
    let agent = tempfile::tempdir().unwrap();
    let dir = agent
        .path()
        .join("plugins")
        .join("pi")
        .join("demo-pkg")
        .join("pi-probe-zzz-skill");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        "---\ndescription: probe skill\n---\nBody",
    )
    .unwrap();
    let res = load_skills(agent.path(), agent.path());
    assert!(
        res.skills.iter().any(|s| s.name == "pi-probe-zzz-skill"),
        "installed pi skill not discovered: {:?}",
        res.skills.iter().map(|s| &s.name).collect::<Vec<_>>()
    );
}
