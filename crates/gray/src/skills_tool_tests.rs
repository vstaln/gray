use super::*;

// These tests used to mutate HOME while unrelated tests read it. A mutex
// local to this file cannot protect those readers. Reuse the subprocess pattern
// from home_paths: set environment before startup, run one exact test per child.
fn isolated_home(test: &str) -> bool {
    if std::env::var("GRAY_SKILLS_TEST_CHILD").as_deref() == Ok(test) {
        return false;
    }
    let home = tempfile::tempdir().unwrap();
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            &format!("skills_tool::tests::{test}"),
            "--nocapture",
        ])
        .env("GRAY_SKILLS_TEST_CHILD", test)
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .env("GRAY_HOME", home.path().join(".gray"))
        .env("XDG_CONFIG_HOME", home.path().join(".config"))
        .status()
        .unwrap();
    assert!(status.success(), "isolated {test} failed");
    true
}

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
    if isolated_home("skills_context_matches_fresh_discovery_and_rescans_on_change") {
        return;
    }
    use gray_plugin::Plugin;
    let work = tempfile::tempdir().unwrap();
    let cwd = work.path().to_str().unwrap().to_string();
    let fresh_block = || {
        let found = crate::skills::discover_skills(work.path());
        let block = crate::skills::format_skills_for_prompt(
            &found.skills,
            crate::setup::skills_auto_enabled(),
            &crate::setup::disabled_skill_names(),
        );
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
}

#[tokio::test]
async fn skills_plugin_is_context_only_and_serves_block() {
    if isolated_home("skills_plugin_is_context_only_and_serves_block") {
        return;
    }
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
    let empty = tempfile::tempdir().unwrap();
    let none = plugin.prompt_context(empty.path().to_str().unwrap()).await;
    assert_eq!(none, None);
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

#[test]
fn project_context_block_serves_nearest_ancestor() {
    let tmp = tempfile::tempdir().unwrap();
    let sub = tmp.path().join("crate");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::write(tmp.path().join("CLAUDE.md"), "root rules").unwrap();
    std::fs::write(sub.join("AGENTS.md"), "nested rules").unwrap();
    let block = project_context_block(&sub).expect("nearest AGENTS.md must serve");
    assert!(
        block.starts_with("<project_context source=\""),
        "missing opening tag: {block}"
    );
    assert!(
        block.contains("nested rules"),
        "nearest file not served: {block}"
    );
    assert!(!block.contains("root rules"), "ancestor file must not win");
    assert!(
        block.trim_end().ends_with("</project_context>"),
        "unclosed: {block}"
    );
    // Self-describing: a stored prompt that never mentions the block still
    // learns what it is and how to weigh it.
    assert!(
        block.contains("outrank general defaults"),
        "no semantics line: {block}"
    );
    assert!(
        block.contains(&sub.join("AGENTS.md").display().to_string()),
        "no source path"
    );
}

#[test]
fn project_context_block_strips_rationale_comments_for_the_executor() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("AGENTS.md"),
        "# gray rules\n\n- Run the tests.\n# r1: a 3-crate run once let a clippy failure reach CI\n- Never push main.\n# r12: the user said so on 2026-09-21\n",
    )
    .unwrap();
    let block = project_context_block(tmp.path()).expect("rules must still serve");
    assert!(block.contains("- Run the tests."), "{block}");
    assert!(block.contains("- Never push main."), "{block}");
    assert!(
        !block.contains("clippy failure"),
        "rationale leaked to the executor: {block}"
    );
    assert!(
        !block.lines().any(is_rationale_comment),
        "a rationale line leaked into the served block: {block}"
    );
    assert!(
        block.contains("stripped before you see them"),
        "block must explain the channel so an editor preserves it: {block}"
    );
}

#[test]
fn strip_rationale_comments_is_identity_without_comments() {
    let body = "# Title\n\n- Rule one.\n- Rule two.\n";
    assert_eq!(strip_rationale_comments(body), body);
    // A heading is not a comment, and neither is prose.
    assert_eq!(
        strip_rationale_comments("# Requirements:\n- Rule."),
        "# Requirements:\n- Rule."
    );
    assert_eq!(
        strip_rationale_comments("# round two notes"),
        "# round two notes"
    );
}

#[test]
fn project_context_block_none_when_only_blank_lines_survive_the_strip() {
    let tmp = tempfile::tempdir().unwrap();
    // Comments separated by blank lines: the strip leaves only the blanks.
    std::fs::write(
        tmp.path().join("AGENTS.md"),
        "# r1: only a rationale\n\n# r2: and another\n\n",
    )
    .unwrap();
    assert_eq!(project_context_block(tmp.path()), None);
}

#[test]
fn project_context_block_none_when_only_rationale_comments() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("AGENTS.md"),
        "# r1: only a rationale, no rules left to follow\n",
    )
    .unwrap();
    assert_eq!(project_context_block(tmp.path()), None);
}

#[test]
fn project_context_block_prefers_agents_over_claude_on_same_level() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("CLAUDE.md"), "claude rules").unwrap();
    std::fs::write(tmp.path().join("AGENTS.md"), "agents rules").unwrap();
    let block = project_context_block(tmp.path()).unwrap();
    assert!(
        block.contains("agents rules"),
        "AGENTS.md must win: {block}"
    );
    assert!(
        !block.contains("claude rules"),
        "CLAUDE.md must lose: {block}"
    );
}

#[test]
fn project_context_block_none_when_absent_or_empty() {
    let tmp = tempfile::tempdir().unwrap();
    assert_eq!(project_context_block(tmp.path()), None);
    std::fs::write(tmp.path().join("AGENTS.md"), "  \n\n").unwrap();
    assert_eq!(project_context_block(tmp.path()), None);
}

#[test]
fn project_context_block_truncates_huge_file() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("AGENTS.md"), "x".repeat(40_000)).unwrap();
    let block = project_context_block(tmp.path()).unwrap();
    assert!(block.contains("truncated"), "no truncation marker: {block}");
    assert!(
        block.len() < 40_000,
        "block must be bounded, got {} bytes",
        block.len()
    );
}

#[tokio::test]
async fn project_context_plugin_is_context_only_and_serves_block() {
    use gray_plugin::Plugin;
    let plugin = ProjectContextPlugin;
    assert!(plugin.tools().is_empty(), "must carry no tools");
    assert!(
        plugin.manifest().tools.is_empty(),
        "manifest must advertise none"
    );
    let empty = tempfile::tempdir().unwrap();
    assert_eq!(
        plugin.prompt_context(empty.path().to_str().unwrap()).await,
        None
    );
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("AGENTS.md"), "build with make").unwrap();
    let ctx = plugin
        .prompt_context(tmp.path().to_str().unwrap())
        .await
        .expect("a project AGENTS.md must produce hook context");
    assert!(ctx.contains("<project_context"), "missing block: {ctx}");
    assert!(ctx.contains("build with make"), "missing body: {ctx}");
}

#[test]
fn project_context_block_skips_gray_home_level() {
    if isolated_home("project_context_block_skips_gray_home_level") {
        return;
    }
    // isolated_home points GRAY_HOME at a scratch dir: its AGENTS.md is the
    // stored system prompt, and serving it would duplicate the whole prompt
    // every turn. The level is skipped, but a rule file ABOVE gray home
    // still counts — the walk continues upward.
    let gray_home = crate::setup::gray_home().unwrap();
    std::fs::create_dir_all(&gray_home).unwrap();
    std::fs::write(gray_home.join("AGENTS.md"), "the system prompt itself").unwrap();
    assert_eq!(
        project_context_block(&gray_home),
        None,
        "gray-home AGENTS.md must never serve as project context"
    );
    let above = gray_home.parent().unwrap().to_path_buf();
    std::fs::write(above.join("AGENTS.md"), "ancestor project rules").unwrap();
    let block = project_context_block(&gray_home).expect("ancestor above gray home must serve");
    assert!(
        block.contains("ancestor project rules"),
        "walk must continue: {block}"
    );
    assert!(
        !block.contains("system prompt itself"),
        "gray home leaked: {block}"
    );
}
