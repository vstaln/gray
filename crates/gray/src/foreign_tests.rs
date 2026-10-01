use super::*;
use gray_core::agent::{CommandOutcome, PluginHooks};

fn fixture_package() -> tempfile::TempDir {
    let dir = tempfile::TempDir::new().expect("temp package");
    std::fs::write(dir.path().join("AGENTS.md"), "# Pony rules\n\nBe lazy.\n").unwrap();
    std::fs::create_dir_all(dir.path().join("commands")).unwrap();
    std::fs::write(
        dir.path().join("commands/review.md"),
        "---\ndescription: Review the diff\n---\n\nReview $ARGUMENTS\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("commands/mode.md"),
        "---\ndescription: Switch mode\n---\n\nSwitch to $ARGUMENTS\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("gray.json"),
        r#"{"state_file": ".mode", "state_prefix": "Level: ", "state_commands": ["mode"]}"#,
    )
    .unwrap();
    dir
}

fn load_fixture(dir: &tempfile::TempDir) -> ForeignPlugin {
    ForeignPlugin::load("pony", dir.path()).expect("fixture offers inject + commands")
}

#[test]
fn empty_dir_is_inert() {
    let dir = tempfile::TempDir::new().expect("temp package");
    assert!(ForeignPlugin::load("pony", dir.path()).is_none());
}

#[test]
fn session_scan_picks_up_offering_packages_only() {
    use gray_core::agent::PluginHooks;
    let home = tempfile::TempDir::new().expect("temp home");
    let pi = home.path().join("plugins").join("pi");
    std::fs::create_dir_all(pi.join("pony").join("commands")).unwrap();
    std::fs::write(pi.join("pony").join("AGENTS.md"), "Be lazy.\n").unwrap();
    std::fs::write(pi.join("pony").join("commands/go.md"), "Go $ARGUMENTS\n").unwrap();
    std::fs::create_dir_all(pi.join("empty")).unwrap();
    let hooks = super::foreign_hooks_in(&pi);
    assert_eq!(hooks.len(), 1, "the empty package offers nothing");
    let names: Vec<String> = hooks[0].commands().into_iter().map(|c| c.name).collect();
    assert_eq!(names, vec!["/go".to_string()]);
}

#[tokio::test]
async fn inject_serves_agents_md() {
    let dir = fixture_package();
    let p = load_fixture(&dir);
    let ctx = p.prompt_context().await.expect("AGENTS.md injects");
    assert!(ctx.contains("Be lazy"), "{ctx}");
    assert!(ctx.contains("pony"), "names the package: {ctx}");
    assert!(!ctx.contains("Level:"), "no state yet: {ctx}");
}

#[tokio::test]
async fn missing_agents_md_injects_nothing() {
    let dir = tempfile::TempDir::new().expect("temp package");
    std::fs::create_dir_all(dir.path().join("commands")).unwrap();
    std::fs::write(dir.path().join("commands/x.md"), "Do x\n").unwrap();
    let p = ForeignPlugin::load("pony", dir.path()).expect("commands alone load");
    assert!(p.prompt_context().await.is_none());
}

#[test]
fn commands_advertise_names_and_descriptions() {
    let dir = fixture_package();
    let p = load_fixture(&dir);
    let mut cmds: Vec<(String, String)> = p
        .commands()
        .into_iter()
        .map(|c| (c.name, c.description))
        .collect();
    cmds.sort();
    assert_eq!(
        cmds,
        vec![
            ("/mode".to_string(), "Switch mode".to_string()),
            ("/review".to_string(), "Review the diff".to_string()),
        ]
    );
}

#[tokio::test]
async fn prompt_command_submits_substituted_body() {
    let dir = fixture_package();
    let p = load_fixture(&dir);
    match p.run_command("/review", vec!["the diff".to_string()]).await {
        Some(CommandOutcome::Prompt(text)) => {
            assert_eq!(text, "Review the diff");
            assert!(!text.contains("description"), "{text}");
        }
        other => panic!("expected Prompt, got {other:?}"),
    }
}

#[tokio::test]
async fn state_command_writes_and_appends() {
    let dir = fixture_package();
    let p = load_fixture(&dir);
    match p.run_command("/mode", vec!["ultra".to_string()]).await {
        Some(CommandOutcome::Say(text)) => assert_eq!(text, "mode ultra"),
        other => panic!("expected Say, got {other:?}"),
    }
    assert_eq!(
        std::fs::read_to_string(dir.path().join(".mode")).unwrap(),
        "ultra"
    );
    let ctx = p.prompt_context().await.expect("state appends");
    assert!(ctx.contains("Level: ultra"), "{ctx}");
}

#[tokio::test]
async fn unknown_command_declines() {
    let dir = fixture_package();
    let p = load_fixture(&dir);
    assert!(p.run_command("/nope", vec![]).await.is_none());
}

#[tokio::test]
async fn corrupt_manifest_stays_static() {
    let dir = fixture_package();
    std::fs::write(dir.path().join("gray.json"), b"{oops").unwrap();
    let p = load_fixture(&dir);
    // The manifest is unreadable, so every command is a prompt command and
    // nothing writes state.
    match p.run_command("/mode", vec!["ultra".to_string()]).await {
        Some(CommandOutcome::Prompt(text)) => assert!(text.contains("Switch to ultra"), "{text}"),
        other => panic!("expected Prompt, got {other:?}"),
    }
    assert!(!dir.path().join(".mode").exists());
    let ctx = p.prompt_context().await.expect("rules still inject");
    assert!(!ctx.contains("Level:"), "{ctx}");
}
