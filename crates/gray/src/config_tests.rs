//! Config resolution: the exec prefix (where the model's shell commands run).

use super::*;
use crate::Cli;
use clap::Parser;

/// Flags > env > saved file, like every other resolved setting. A saved
/// `exec_prefix` names the box, so it must survive a restart that has no
/// `GRAY_EXEC_PREFIX` in the environment.
#[test]
fn exec_prefix_prefers_the_environment_over_the_saved_file() {
    let cli = Cli::parse_from(["gray"]);
    let config = Config::resolve_with(&cli, |k| {
        (k == "GRAY_EXEC_PREFIX").then(|| "sh -s".to_string())
    })
    .expect("config resolves");
    assert_eq!(config.exec_prefix.as_deref(), Some("sh -s"));
}

/// Empty is not a prefix: a blank setting must leave commands running locally
/// rather than spawning a program named "".
#[test]
fn a_blank_exec_prefix_is_no_exec_prefix() {
    let cli = Cli::parse_from(["gray"]);
    let config = Config::resolve_with(&cli, |k| {
        (k == "GRAY_EXEC_PREFIX").then(|| "   ".to_string())
    })
    .expect("config resolves");
    assert_eq!(config.exec_prefix, None);
}

/// Absent everywhere = local commands, which is the default every existing
/// install depends on.
#[test]
fn no_exec_prefix_means_local_commands() {
    let cli = Cli::parse_from(["gray"]);
    let config = Config::resolve_with(&cli, |_| None).expect("config resolves");
    assert_eq!(config.exec_prefix, None);
}
