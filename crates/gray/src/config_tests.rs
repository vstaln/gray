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

/// `--bare` and `GRAY_BARE=1` both select a bare run; neither = normal run.
#[test]
fn bare_comes_from_the_flag_or_env() {
    let plain = Cli::parse_from(["gray"]);
    assert!(
        !Config::resolve_with(&plain, |_| None)
            .expect("config resolves")
            .bare
    );
    let flag = Cli::parse_from(["gray", "--bare"]);
    assert!(
        Config::resolve_with(&flag, |_| None)
            .expect("config resolves")
            .bare
    );
    let env = Config::resolve_with(&plain, |k| (k == "GRAY_BARE").then(|| "1".to_string()))
        .expect("config resolves");
    assert!(env.bare);
}

/// Lean is the default: `--lean` / `GRAY_LEAN=1` force on, `GRAY_LEAN=0`
/// opts out, the flag wins over an opting-out env, and nothing set means
/// on (a saved `lean: false` would still opt out — uninjectable here).
#[test]
fn lean_defaults_on_with_flag_and_env_overrides() {
    let plain = Cli::parse_from(["gray"]);
    let flag = Cli::parse_from(["gray", "--lean"]);
    let env0 = |k: &str| (k == "GRAY_LEAN").then(|| "0".to_string());
    let env1 = |k: &str| (k == "GRAY_LEAN").then(|| "1".to_string());
    assert!(
        !Config::resolve_with(&plain, env0)
            .expect("config resolves")
            .lean
    );
    assert!(
        Config::resolve_with(&flag, env0)
            .expect("config resolves")
            .lean
    );
    assert!(
        Config::resolve_with(&plain, env1)
            .expect("config resolves")
            .lean
    );
    assert!(
        Config::resolve_with(&plain, |_| None)
            .expect("config resolves")
            .lean
    );
}

// ── api key precedence ──
// UNRUN (cargo test banned under X — verified via check + clippy only).

fn key_with(env_key: &str, env_val: &str) -> impl FnMut(&str) -> Option<String> {
    let (k, v) = (env_key.to_string(), env_val.to_string());
    move |name| (name == k).then(|| v.clone())
}

/// (a) Fresh install: `OPENAI_API_KEY` must not ride onto the default
/// OpenRouter base — there it is just a bad Bearer.
#[test]
fn openai_env_key_is_ignored_on_the_default_openrouter_base() {
    let cli = Cli::parse_from(["gray"]);
    let key = resolve_api_key(
        &cli,
        DEFAULT_BASE_URL,
        &mut key_with("OPENAI_API_KEY", "sk-oai"),
        None,
    );
    assert_eq!(key, None);
    // Slashes/spacing on the resolved base don't dodge the rule.
    let key = resolve_api_key(
        &cli,
        " https://openrouter.ai/api/v1/ ",
        &mut key_with("OPENAI_API_KEY", "sk-oai"),
        None,
    );
    assert_eq!(key, None);
}

/// (b) A stored key wins over `OPENAI_API_KEY` — the env var stopped being
/// the override the moment a provider was connected.
#[test]
fn the_saved_key_beats_openai_env_key() {
    let cli = Cli::parse_from(["gray"]);
    let key = resolve_api_key(
        &cli,
        DEFAULT_BASE_URL,
        &mut key_with("OPENAI_API_KEY", "sk-oai"),
        Some("sk-stored".to_string()),
    );
    assert_eq!(key.as_deref(), Some("sk-stored"));
}

/// (c) `OPENAI_API_KEY` still fills a non-default base with no stored key.
#[test]
fn openai_env_key_applies_to_the_openai_base() {
    let cli = Cli::parse_from(["gray"]);
    let key = resolve_api_key(
        &cli,
        "https://api.openai.com/v1",
        &mut key_with("OPENAI_API_KEY", "sk-oai"),
        None,
    );
    assert_eq!(key.as_deref(), Some("sk-oai"));
}

/// (d) `GRAY_API_KEY` is the explicit override — it beats the stored key on
/// any base, including the default one.
#[test]
fn gray_api_key_beats_the_saved_key() {
    let cli = Cli::parse_from(["gray"]);
    let key = resolve_api_key(
        &cli,
        DEFAULT_BASE_URL,
        &mut key_with("GRAY_API_KEY", "sk-gray"),
        Some("sk-stored".to_string()),
    );
    assert_eq!(key.as_deref(), Some("sk-gray"));
}
