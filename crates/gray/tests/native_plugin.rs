//! Native plugin registration/command routing against the real CLI, no provider.
#![cfg(unix)]
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    os::unix::fs::PermissionsExt,
    path::Path,
    process::{Command, Stdio},
};

fn fixture(root: &Path, name: &str, widget: bool) -> std::path::PathBuf {
    let path = root.join(format!("{name}.sh"));
    let manifest = json!({"name":name,"version":"1.0.0","widget":widget,"commands":[format!("/{name}"),"/alias"],"completion":["settings","run"],"tools":[]});
    fs::write(&path,format!("#!/bin/sh\nif [ \"$1\" = manifest ]; then\n  printf '%s\\n' '{}'\nelse\n  printf '<%s>\\n' \"$@\"\nfi\n",manifest)).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}
fn cli(home: &Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_gray"));
    c.env("GRAY_HOME", home)
        .env_remove("GRAY_MODEL")
        .env_remove("GRAY_PLUGIN_PATH");
    c
}
fn install(home: &Path, path: &Path, name: &str) -> std::process::Output {
    cli(home)
        .args(["install", "plugin", name])
        .env("GRAY_PLUGIN_PATH", path)
        .output()
        .unwrap()
}
fn repl(home: &Path, input: &str) -> String {
    let mut p = cli(home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    p.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
    let out = p.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}
#[test]
fn fresh_home_help_and_quoted_slash_work_before_agent_construction() {
    let temp = tempfile::tempdir().unwrap();
    let bin = fixture(temp.path(), "sample", false);
    assert!(install(temp.path(), &bin, "sample").status.success());
    let output = repl(
        temp.path(),
        "/help\n/alias run \"two word task\" '' '$(not-executed)'\n/quit\n",
    );
    assert!(output.contains("/sample"), "{output}");
    assert!(output.contains("/alias"), "{output}");
    assert!(output.contains("<two word task>"), "{output}");
    assert!(output.contains("<>"), "{output}");
    assert!(output.contains("<$(not-executed)>"), "{output}");
    let output = repl(temp.path(), "/alias 'unterminated\n/quit\n");
    assert!(output.contains("check quotes"), "{output}");
}
#[test]
fn direct_cli_forwards_argument_boundaries_and_nonzero_exit() {
    let temp = tempfile::tempdir().unwrap();
    let bin = fixture(temp.path(), "sample", false);
    assert!(install(temp.path(), &bin, "sample").status.success());
    let out = cli(temp.path())
        .args(["sample", "run", "two words", ""])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        "<run>\n<two words>\n<>\n"
    );
    fs::write(bin, "#!/bin/sh\nexit 7\n").unwrap();
    let out = cli(temp.path()).args(["sample", "run"]).output().unwrap();
    assert_eq!(out.status.code(), Some(7));
}
#[test]
fn conflicting_widget_registration_does_not_publish_partial_command() {
    let temp = tempfile::tempdir().unwrap();
    let one = fixture(temp.path(), "one", true);
    let two = fixture(temp.path(), "two", true);
    assert!(install(temp.path(), &one, "one").status.success());
    assert!(!install(temp.path(), &two, "two").status.success());
    let registry: Value =
        serde_json::from_slice(&fs::read(temp.path().join("plugins/commands.json")).unwrap())
            .unwrap();
    assert!(registry["plugins"].get("two").is_none());
    assert!(!temp.path().join("plugins/two-manifest.json").exists());
}
#[test]
fn disabled_plugin_is_unavailable_in_help_commands_and_widget() {
    let temp = tempfile::tempdir().unwrap();
    let bin = fixture(temp.path(), "sample", true);
    assert!(install(temp.path(), &bin, "sample").status.success());
    let path = temp.path().join("plugins/lock.json");
    let mut value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    value["plugins"]["sample"]["enabled"] = json!(false);
    fs::write(path, value.to_string()).unwrap();
    assert!(
        !cli(temp.path())
            .args(["sample", "run"])
            .output()
            .unwrap()
            .status
            .success()
    );
    let output = repl(temp.path(), "/help\n/quit\n");
    assert!(!output.contains("/sample"), "{output}");
}
