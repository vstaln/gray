use super::*;

#[test]
fn strip_frontmatter_removes_yaml_header() {
    let content = "---\nname: test\ndescription: A test skill\n---\n\nBody here.\n";
    assert_eq!(strip_frontmatter(content), "Body here.\n");
}

#[test]
fn strip_frontmatter_keeps_body_without_frontmatter() {
    assert_eq!(strip_frontmatter("Just content."), "Just content.");
}

#[test]
fn substitutions_expand_arguments_and_skill_dir() {
    let body = "Deploy $ARGUMENTS from ${SKILL_DIR}/bin.";
    assert_eq!(
        apply_substitutions(body, Some("staging"), "/skills/deploy"),
        "Deploy staging from /skills/deploy/bin."
    );
    assert_eq!(
        apply_substitutions("No args here.", None, "/d"),
        "No args here."
    );
}

#[test]
fn resolve_skill_name_finds_project_skill() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join(".gray/skills/commit");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        "---\nname: commit\ndescription: commit changes\n---\nBody",
    )
    .unwrap();
    let resolved = resolve_skill_name(tmp.path(), "commit").unwrap();
    assert_eq!(resolved, dir.join("SKILL.md"));
    assert!(resolve_skill_name(tmp.path(), "missing").is_none());
}

#[tokio::test]
async fn skills_context_matches_fresh_discovery_and_rescans_on_change() {
    use gray_plugin::Plugin;
    // Isolate from the user's real skills (see the test above for why
    // this save/set/restore dance exists).
    let prev_home = std::env::var("HOME").ok();
    let prev_gray = std::env::var("GRAY_HOME").ok();
    let prev_xdg = std::env::var("XDG_CONFIG_HOME").ok();
    let iso = tempfile::tempdir().unwrap();
    unsafe {
        std::env::set_var("HOME", iso.path());
        std::env::set_var("GRAY_HOME", iso.path().join(".gray"));
        std::env::set_var("XDG_CONFIG_HOME", iso.path().join(".config"));
    }
    let restore = || unsafe {
        match &prev_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
        match &prev_gray {
            Some(v) => std::env::set_var("GRAY_HOME", v),
            None => std::env::remove_var("GRAY_HOME"),
        }
        match &prev_xdg {
            Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }
    };

    let work = tempfile::tempdir().unwrap();
    let cwd = work.path().to_str().unwrap().to_string();
    let fresh_block = || {
        let found = crate::skills::discover_skills(work.path());
        let block = crate::skills::format_skills_for_prompt(&found.skills);
        if block.trim().is_empty() {
            None
        } else {
            Some(block)
        }
    };
    let write_skill = |name: &str, description: &str| {
        let dir = work.path().join(".gray/skills").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            format!("---\ndescription: {description}\n---\nBody"),
        )
        .unwrap();
    };

    let plugin = SkillsPlugin::default();
    // Fingerprint is stable with no changes…
    let fp1 = crate::skills::discovery_fingerprint(work.path());
    assert_eq!(fp1, crate::skills::discovery_fingerprint(work.path()));

    write_skill("demo-a", "first demo skill");
    // …moves when a skill appears…
    let fp2 = crate::skills::discovery_fingerprint(work.path());
    assert_ne!(fp1, fp2, "fingerprint must move on new skill");
    // …and the served block matches a fresh discovery exactly (no
    // behavior change vs per-turn rediscovery)…
    let a = plugin.prompt_context(&cwd).await;
    assert_eq!(a, fresh_block());
    assert!(a.unwrap().contains("first demo skill"));
    // …is stable across turns…
    assert_eq!(plugin.prompt_context(&cwd).await, fresh_block());

    // …moves on in-place edits (dir mtime alone would miss these)…
    write_skill("demo-a", "edited demo description");
    let fp3 = crate::skills::discovery_fingerprint(work.path());
    assert_ne!(fp2, fp3, "fingerprint must move on content edit");
    let b = plugin.prompt_context(&cwd).await;
    assert_eq!(b, fresh_block());
    assert!(b.unwrap().contains("edited demo description"));

    // …and on removal.
    std::fs::remove_dir_all(work.path().join(".gray/skills/demo-a")).unwrap();
    let fp4 = crate::skills::discovery_fingerprint(work.path());
    assert_ne!(fp3, fp4, "fingerprint must move on removal");
    assert_eq!(plugin.prompt_context(&cwd).await, fresh_block());

    restore();
}

#[tokio::test]
async fn skills_plugin_is_context_only_and_serves_block() {
    use gray_plugin::Plugin;
    let plugin = SkillsPlugin::default();
    // Bash-only: no tools ride this plugin.
    assert!(
        plugin.tools().is_empty(),
        "skills plugin must carry no tools"
    );
    assert!(
        plugin.manifest().tools.is_empty(),
        "manifest must advertise no tools"
    );
    // Isolate from global skills for the empty case.
    let prev_home = std::env::var("HOME").ok();
    let prev_gray = std::env::var("GRAY_HOME").ok();
    let prev_xdg = std::env::var("XDG_CONFIG_HOME").ok();
    let iso = tempfile::tempdir().unwrap();
    unsafe {
        std::env::set_var("HOME", iso.path());
        std::env::set_var("GRAY_HOME", iso.path().join(".gray"));
        std::env::set_var("XDG_CONFIG_HOME", iso.path().join(".config"));
    }
    let empty = tempfile::tempdir().unwrap();
    let none = plugin.prompt_context(empty.path().to_str().unwrap()).await;
    assert_eq!(none, None);
    // Restore before the project-skill case (discovery walks cwd only,
    // globals stay isolated only for the empty check above).
    unsafe {
        match &prev_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
        match &prev_gray {
            Some(v) => std::env::set_var("GRAY_HOME", v),
            None => std::env::remove_var("GRAY_HOME"),
        }
        match &prev_xdg {
            Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }
    }
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join(".gray/skills/paste-demo");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        "---\ndescription: demo skill\n---\nDemo body.",
    )
    .unwrap();
    let ctx = plugin
        .prompt_context(tmp.path().to_str().unwrap())
        .await
        .expect("discovered skills must produce hook context");
    assert!(ctx.contains("<available_skills>"), "missing block: {ctx}");
    assert!(ctx.contains("paste-demo"), "missing skill name: {ctx}");
    assert!(ctx.contains("SKILL.md"), "missing exact location: {ctx}");
    assert!(
        ctx.contains("cat <location>") || ctx.contains("cat "),
        "block must tell the model to read via bash: {ctx}"
    );
}
