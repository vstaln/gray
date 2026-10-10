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

fn look(cmd: &str) -> Option<(Look, Vec<String>, Option<String>)> {
    read_only(cmd).map(|r| (r.look, r.targets, r.detail))
}

fn strs(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

#[test]
fn bare_cat_of_media_is_a_view() {
    assert_eq!(
        look("cat /home/u/assets/charts/g1_products.jpg"),
        Some((
            Look::Viewed,
            strs(&["/home/u/assets/charts/g1_products.jpg"]),
            None
        ))
    );
    assert_eq!(look("cat a.png b.pdf").map(|l| l.0), Some(Look::Viewed));
}

#[test]
fn media_cat_that_the_tool_does_not_attach_is_a_plain_read() {
    // Only the whole-command shape attaches; these dump bytes instead.
    assert_eq!(look("cd ~/x && cat a.png").map(|l| l.0), Some(Look::Read));
    assert_eq!(
        look("cat a.png | head -c 10").map(|l| l.0),
        Some(Look::Read)
    );
    assert_eq!(look("cat a.png notes.txt").map(|l| l.0), Some(Look::Read));
}

#[test]
fn file_reads_name_the_files_and_the_range() {
    assert_eq!(
        look("cat src/main.rs"),
        Some((Look::Read, strs(&["src/main.rs"]), None))
    );
    assert_eq!(
        look("sed -n '120,180p' crates/gray/src/lib.rs"),
        Some((
            Look::Read,
            strs(&["crates/gray/src/lib.rs"]),
            Some("lines 120\u{2013}180".into())
        ))
    );
    assert_eq!(
        look("sed -n '40,$p' a.rs").and_then(|l| l.2),
        Some("from line 40".into())
    );
    assert_eq!(
        look("head -n 50 README.md"),
        Some((
            Look::Read,
            strs(&["README.md"]),
            Some("first 50 lines".into())
        ))
    );
    assert_eq!(
        look("tail -20 app.log 2>/dev/null"),
        Some((Look::Read, strs(&["app.log"]), Some("last 20 lines".into())))
    );
    assert_eq!(
        look("tail -f app.log").and_then(|l| l.2),
        Some("following".into())
    );
    assert_eq!(
        look("cat Cargo.toml | grep -n version | head -3"),
        Some((Look::Read, strs(&["Cargo.toml"]), None))
    );
}

#[test]
fn listings_and_searches() {
    assert_eq!(look("ls"), Some((Look::Listed, strs(&["."]), None)));
    assert_eq!(
        look("cd ~/gray && ls -la crates"),
        Some((Look::Listed, strs(&["crates"]), None))
    );
    assert_eq!(
        look("grep -rn \"fn main\" crates/gray/src"),
        Some((
            Look::Searched,
            strs(&["fn main"]),
            Some("crates/gray/src".into())
        ))
    );
    assert_eq!(
        look("rg -t rust -e announce"),
        Some((Look::Searched, strs(&["announce"]), None))
    );
    assert_eq!(
        look("find crates -name '*.rs'"),
        Some((Look::Searched, strs(&["*.rs"]), Some("crates".into())))
    );
    assert_eq!(look("find src -type d").map(|l| l.0), Some(Look::Listed));
    assert_eq!(
        look("fd label crates"),
        Some((Look::Searched, strs(&["label"]), Some("crates".into())))
    );
}

#[test]
fn anything_that_could_write_or_run_more_stays_ran() {
    for cmd in [
        "cat a > b",
        "cat a >> b",
        "cat a | tee b",
        "cat a; rm b",
        "cat a && rm b",
        "cat a || true",
        "cat $(ls)",
        "cat `ls`",
        "cat \"$HOME/x\"",
        "sed -i 's/a/b/' f",
        "sed -n 's/a/b/p' f",
        "sed -n '1w out' f",
        "sed -n 1,5p",
        "find . -name x -delete",
        "find . -exec rm {} +",
        "fd x -x rm",
        "rg --pre ./run.sh x",
        "grep -f pats.txt src",
        "cat a | sort -o b",
        "cat a | uniq - b",
        "cat a | xargs rm",
        "cat a | awk '{print > \"b\"}'",
        "tree -o out.txt",
        "cat 'unterminated",
        "cat a\nrm b",
        "cat",
        "# cat a",
        "cargo test",
        "echo hi",
    ] {
        assert_eq!(read_only(cmd), None, "{cmd:?} must stay Ran");
    }
}
