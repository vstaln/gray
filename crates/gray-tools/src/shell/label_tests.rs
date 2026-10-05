use super::*;

#[test]
fn setup_is_peeled_and_the_directory_kept() {
    let core = core_command(
        "cd ~/wt/provider-chat && CARGO_BUILD_BUILD_DIR=target nice -n 19 ionice -c3 flock /tmp/gray.lock cargo check -p gray-provider --tests 2>&1 | head -40",
    );
    assert_eq!(
        core.command,
        "cargo check -p gray-provider --tests 2>&1 | head -40"
    );
    assert_eq!(core.cwd, Some("~/wt/provider-chat"));
}

#[test]
fn plain_commands_are_untouched() {
    let core = core_command("ls -la");
    assert_eq!(core.command, "ls -la");
    assert_eq!(core.cwd, None);
}

#[test]
fn export_set_env_timeout_and_friends_peel() {
    assert_eq!(
        core_command("set -euo pipefail && export RUST_LOG=debug A=1 && timeout -k 5 300 env FOO=bar cargo test").command,
        "cargo test"
    );
    assert_eq!(
        core_command("time nohup stdbuf -oL make build").command,
        "make build"
    );
    assert_eq!(
        core_command("nice -19 python3 -m pytest").command,
        "python3 -m pytest"
    );
}

#[test]
fn anything_that_could_do_work_stops_the_peel() {
    // A segment that is not pure setup keeps the whole chain visible.
    let risky = "rm -rf build && cargo check";
    assert_eq!(core_command(risky).command, risky);
    // Expansion in the cd target could run something: not peeled.
    let sub = "cd $(mktemp -d) && cargo check";
    assert_eq!(core_command(sub).command, sub);
    // A quoted assignment stops at the assignment.
    assert_eq!(
        core_command("FOO='a b' cargo check").command,
        "FOO='a b' cargo check"
    );
    // `flock -c` runs a string; keep it whole.
    assert_eq!(
        core_command("flock -c 'make' /tmp/l").command,
        "flock -c 'make' /tmp/l"
    );
    // A wrapper with nothing after it is the command.
    assert_eq!(core_command("nice").command, "nice");
}

#[test]
fn job_names_read_like_the_command() {
    let cases = [
        (
            "cd ~/wt/provider-chat && nice -n 19 cargo check -p gray-provider",
            "cargo-check",
        ),
        ("npm test -- --watch", "npm-test"),
        ("npm run build:prod", "npm-run"),
        ("pytest tests/ -x", "pytest"),
        ("python3 -m pytest -q", "pytest"),
        ("./scripts/deploy.sh --prod", "deploy"),
        ("git --no-pager log --oneline", "git-log"),
        ("sleep 30", "sleep"),
        ("$(which cargo) build", "job"),
        ("", "job"),
    ];
    for (cmd, want) in cases {
        assert_eq!(job_name(cmd), want, "{cmd}");
    }
}

#[test]
fn names_are_short_and_safe() {
    let name = job_name("Some_Very-Long.Program.Name.That.Goes.On.And.On --flag");
    assert!(name.len() <= 24, "{name}");
    assert!(
        name.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
        "{name}"
    );
    assert!(!name.ends_with('-'), "{name}");
}

#[test]
fn a_taken_name_gets_a_counter() {
    let taken = ["cargo-check", "cargo-check-2"];
    assert_eq!(unique_name("npm-test", |n| taken.contains(&n)), "npm-test");
    assert_eq!(
        unique_name("cargo-check", |n| taken.contains(&n)),
        "cargo-check-3"
    );
}
