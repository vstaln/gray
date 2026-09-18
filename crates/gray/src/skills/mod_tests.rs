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
fn prompt_block_gives_proactive_skill_guidance() {
    let s = test_skill("anything", &[]);
    let out = format_skills_for_prompt(&[s]);
    assert!(
        out.contains("before acting even for simple tasks"),
        "proactive read-first hint missing: {out}"
    );
    assert!(
        out.contains("do not wait"),
        "no-wait-for-/skills hint missing: {out}"
    );
    assert!(out.contains("read tool"), "read-tool hint missing: {out}");
    assert!(
        out.contains("fallback only"),
        "bash-fallback-only hint missing: {out}"
    );
    assert!(
        out.contains("name the skill"),
        "name-the-skill hint missing: {out}"
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

#[test]
fn prompt_block_caps_skill_list_and_says_so() {
    let skills: Vec<Skill> = (0..60)
        .map(|i| test_skill(&format!("skill-{i:02}"), &[]))
        .collect();
    let out = format_skills_for_prompt(&skills);
    assert_eq!(
        out.matches("<skill>").count(),
        40,
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
    let out = format_skills_for_prompt(&skills);
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
    let out = format_skills_for_prompt(&skills);
    assert_eq!(out.matches("<skill>").count(), 40);
    assert!(!out.contains("more skills"));
    assert!(!out.contains("<name>disabled</name>"));
    assert!(!out.contains("<name>empty</name>"));
    assert_eq!(rank_skills_for_prompt(&skills).len(), 40);
    assert!(format_skills_for_prompt(&[]).is_empty());
}
