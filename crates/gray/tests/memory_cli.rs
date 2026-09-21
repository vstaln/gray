use std::path::Path;
use std::process::{Command, Output};

fn command(home: &Path, cwd: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_gray"))
        .env("GRAY_HOME", home)
        .env_remove("GRAY_NO_MEMORY")
        .env_remove("GRAY_API_KEY")
        .env_remove("OPENAI_API_KEY")
        .current_dir(cwd)
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn memory_commands_work_without_provider_and_keep_scopes_separate() {
    let tmp = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let home = tmp.path().join("home");
    let a = tmp.path().join("a");
    let b = tmp.path().join("b");
    std::fs::create_dir_all(a.join(".git")).unwrap();
    std::fs::create_dir_all(a.join("src")).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    let run = |cwd: &Path, args: &[&str]| {
        let out = command(&home, cwd, args);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    };
    assert_eq!(run(&a, &["memory", "list"]), "No memories.\n");
    // Gray initializes its logger even for local commands; memory itself stays read-only.
    assert!(!home.join("memory").exists());
    assert_eq!(
        run(&a, &["memory", "set", "language", "Use Rust."]),
        "Memory updated.\n"
    );
    assert_eq!(
        run(&a.join("src"), &["memory", "list"]),
        "- language: Use Rust.\n"
    );
    assert_eq!(run(&b, &["memory", "list"]), "No memories.\n");
    run(
        &a,
        &[
            "memory",
            "--scope",
            "user",
            "set",
            "style",
            "Keep answers concise.",
        ],
    );
    assert_eq!(
        run(&b, &["memory", "--scope", "user", "list"]),
        "- style: Keep answers concise.\n"
    );
    run(&a, &["memory", "set", "language", "Use Go."]);
    assert_eq!(run(&a, &["memory", "list"]), "- language: Use Go.\n");
    assert_eq!(
        run(&a, &["memory", "set", "language", "Use Go."]),
        "Memory unchanged.\n"
    );
    run(&a, &["memory", "remove", "language"]);
    assert_eq!(run(&a, &["memory", "list"]), "No memories.\n");
    assert!(
        !command(&home, &a, &["memory", "remove", "language"])
            .status
            .success()
    );
}

#[test]
fn audit_advises_without_deleting_and_growth_warns_on_the_third_net_add() {
    let tmp = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let home = tmp.path().join("home");
    let run = |args: &[&str]| {
        let out = command(&home, tmp.path(), args);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    };
    run(&["memory", "set", "bare", "A bare decision."]);
    let audit = run(&["memory", "audit"]);
    assert!(audit.contains("bare: no Why recorded"), "{audit}");
    assert!(audit.contains("This audit deletes nothing"), "{audit}");
    // Advisory only: the entry survives the audit untouched.
    assert_eq!(run(&["memory", "list"]), "- bare: A bare decision.\n");
    // The second net add stays quiet; the third trips the ratchet warning,
    // and every net add after it keeps warning until something is removed.
    assert_eq!(run(&["memory", "set", "b", "Two."]), "Memory updated.\n");
    let third = run(&["memory", "set", "c", "Three."]);
    assert!(third.starts_with("Memory updated.\n"), "{third}");
    assert!(third.contains("grown to 3 entries over 3 saves"), "{third}");
    let fourth = run(&["memory", "set", "d", "Four."]);
    assert!(fourth.contains("grown to 4 entries"), "{fourth}");
    assert!(fourth.contains("gray memory audit"), "{fourth}");
    // Removing one entry resets the streak, so the next save is quiet again.
    run(&["memory", "remove", "bare"]);
    assert_eq!(run(&["memory", "set", "e", "Five."]), "Memory updated.\n");
}

#[test]
fn rejects_bad_input_without_echoing_secret() {
    let tmp = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    for (key, value) in [
        ("../escape", "value"),
        ("blank", " "),
        ("line", "a\nb"),
        ("secret", "sk-testfakecredential123456789"),
    ] {
        let out = command(tmp.path(), tmp.path(), &["memory", "set", key, value]);
        assert!(!out.status.success());
        assert!(!String::from_utf8_lossy(&out.stderr).contains("sk-testfakecredential"));
    }
}

#[test]
fn concurrent_processes_preserve_different_entries() {
    let tmp = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let mut children = Vec::new();
    for i in 0..8 {
        children.push(
            Command::new(env!("CARGO_BIN_EXE_gray"))
                .env("GRAY_HOME", tmp.path())
                .env_remove("GRAY_NO_MEMORY")
                .current_dir(tmp.path())
                .args([
                    "memory",
                    "set",
                    &format!("key-{i}"),
                    &format!("Confirmed decision {i}."),
                ])
                .stdout(std::process::Stdio::null())
                .spawn()
                .unwrap(),
        );
    }
    for mut child in children {
        assert!(child.wait().unwrap().success());
    }
    let out = command(tmp.path(), tmp.path(), &["memory", "list"]);
    assert!(out.status.success());
    assert_eq!(String::from_utf8(out.stdout).unwrap().lines().count(), 8);
}

#[test]
fn opt_out_prevents_saves_but_allows_inspection_and_removal() {
    let tmp = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    assert!(
        command(
            tmp.path(),
            tmp.path(),
            &["memory", "set", "test", "A fact."]
        )
        .status
        .success()
    );
    for (args, success) in [
        (vec!["memory", "set", "test", "Changed."], false),
        (vec!["memory", "list"], true),
        (vec!["memory", "remove", "test"], true),
    ] {
        let out = Command::new(env!("CARGO_BIN_EXE_gray"))
            .env("GRAY_HOME", tmp.path())
            .env("GRAY_NO_MEMORY", "1")
            .current_dir(tmp.path())
            .args(args)
            .output()
            .unwrap();
        assert_eq!(out.status.success(), success);
    }
}
