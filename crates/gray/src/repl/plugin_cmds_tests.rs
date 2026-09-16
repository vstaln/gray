use super::dispatch::{Flow, dispatch_command};
use super::{PluginAction, is_bare_marketplace_cmd, is_bare_plugin_cmd, parse_plugin_args};

#[test]
fn bare_detection_covers_case_and_trailing_space() {
    assert!(is_bare_plugin_cmd("/plugin"));
    assert!(is_bare_plugin_cmd("/PLUGINS"));
    assert!(is_bare_plugin_cmd("/plugin  "));
    assert!(!is_bare_plugin_cmd("/plugin list"));
    assert!(!is_bare_plugin_cmd("/plugin list foo"));
    assert!(!is_bare_plugin_cmd("/plugin install foo"));
    assert!(!is_bare_plugin_cmd("/plugin enable foo"));
}

#[test]
fn marketplace_bare_detection_covers_case_and_trailing_space() {
    assert!(is_bare_marketplace_cmd("/marketplace"));
    assert!(is_bare_marketplace_cmd("/MARKETPLACE"));
    assert!(is_bare_marketplace_cmd("/marketplace  "));
    assert!(!is_bare_marketplace_cmd("/marketplace foo"));
    assert!(!is_bare_marketplace_cmd("/plugin"));
    assert!(!is_bare_marketplace_cmd("/marketplaces"));
}

#[test]
fn parses_install_url() {
    assert!(matches!(
        parse_plugin_args("/plugin install https://h/x.tar.gz"),
        Ok(PluginAction::Install(_))
    ));
    assert!(matches!(
        parse_plugin_args("/plugins"),
        Ok(PluginAction::List)
    ));
}

#[test]
fn parse_covers_every_subcommand() {
    assert!(matches!(
        parse_plugin_args("/plugin"),
        Ok(PluginAction::List)
    ));
    assert!(matches!(
        parse_plugin_args("/plugin LIST"),
        Ok(PluginAction::List)
    ));
    assert!(parse_plugin_args("/plugin list foo").is_err());
    assert!(parse_plugin_args("/plugin list foo bar").is_err());
    assert!(matches!(
        parse_plugin_args("/plugin search foo"),
        Ok(PluginAction::Search(_))
    ));
    assert!(matches!(
        parse_plugin_args("/plugin install foo"),
        Ok(PluginAction::Install(_))
    ));
    assert!(matches!(
        parse_plugin_args("/plugin remove foo"),
        Ok(PluginAction::Remove(_))
    ));
    // Bare update targets everything, like `gray plugin update` (default `all`).
    assert!(matches!(
        parse_plugin_args("/plugin update"),
        Ok(PluginAction::Update(t)) if t == "all"
    ));
    assert!(matches!(
        parse_plugin_args("/plugin update foo"),
        Ok(PluginAction::Update(_))
    ));
    assert!(matches!(
        parse_plugin_args("/plugin enable foo"),
        Ok(PluginAction::Enable(_))
    ));
    assert!(matches!(
        parse_plugin_args("/plugin disable foo"),
        Ok(PluginAction::Disable(_))
    ));
    assert!(matches!(
        parse_plugin_args("/plugin check ./x"),
        Ok(PluginAction::Check(_))
    ));
    assert!(parse_plugin_args("/plugin install").is_err());
    assert!(parse_plugin_args("/plugin search").is_err());
    assert!(parse_plugin_args("/plugin frobnicate x").is_err());
}

/// `/plugin list` routes through the Plugin arm (read-only lockfile
/// read): it returns Continue with no queued follow-up instead of
/// falling into the unknown-command path.
#[tokio::test]
async fn plugin_list_dispatches_without_unknown_command() {
    use clap::Parser;
    let cli = crate::Cli::try_parse_from(["gray"]).expect("cli parses");
    let mut config = crate::config::Config::resolve_with(&cli, |_| None).expect("config resolves");
    let cwd = tempfile::tempdir().expect("tempdir").keep();
    let tui: super::TuiOpt = None;
    let mut agent: Option<super::Agent> = None;
    let mut session_state: Option<super::SessionState> = None;
    let mut totals = super::SessionTotals::default();
    let mut pending: Option<super::ReplCommand> = None;
    let mut history = Vec::new();
    let mut unconfigured = false;
    let mut hide_thinking = false;
    let flow = dispatch_command(
        super::ReplCommand::Plugin("/plugin list".to_string()),
        &mut agent,
        &mut config,
        &cwd,
        &tui,
        &mut session_state,
        &mut totals,
        &mut pending,
        &mut history,
        &mut unconfigured,
        &mut hide_thinking,
    )
    .await
    .expect("dispatch ok");
    assert_eq!(flow, Flow::Continue);
    assert!(pending.is_none());
}
