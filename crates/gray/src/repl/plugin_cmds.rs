//! Plugin manager slash-command: `/plugin <list|search|install|…>`.
//!
//! Thin REPL shell over `gray-pkg::ops`, mirroring the `gray plugin` CLI
//! messages through [`say`](super::say).

use super::*;
use gray_pkg::ops::{self, InstallOpts};

#[derive(Debug, PartialEq)]
pub(crate) enum PluginAction {
    List,
    Search(String),
    Install(String),
    Remove(String),
    Update(String),
    Enable(String),
    Disable(String),
    Check(String),
}

const USAGE: &str = "usage: /plugin <list|search|install|remove|update|enable|disable|check>";

/// Strips the leading `/plugin` (or `/plugins`), splits the first token as
/// the subcommand (default `list` when bare), rest as a single arg.
pub(crate) fn parse_plugin_args(raw: &str) -> anyhow::Result<PluginAction> {
    let mut toks = raw.split_whitespace();
    toks.next(); // strip /plugin or /plugins
    let sub = toks.next().map(|t| t.to_ascii_lowercase());
    let arg = toks.collect::<Vec<_>>().join(" ");
    let need = |what: &str| {
        if arg.is_empty() {
            anyhow::bail!("usage: /plugin {what} <…>");
        }
        Ok(arg.clone())
    };
    match sub.as_deref() {
        None => Ok(PluginAction::List),
        Some("list") if arg.is_empty() => Ok(PluginAction::List),
        Some("list") => anyhow::bail!("usage: /plugin list"),
        Some("search") => Ok(PluginAction::Search(need("search")?)),
        Some("install") => Ok(PluginAction::Install(need("install")?)),
        Some("remove") => Ok(PluginAction::Remove(need("remove")?)),
        Some("update") if arg.is_empty() => Ok(PluginAction::Update("all".to_string())),
        Some("update") => Ok(PluginAction::Update(arg)),
        Some("enable") => Ok(PluginAction::Enable(need("enable")?)),
        Some("disable") => Ok(PluginAction::Disable(need("disable")?)),
        Some("check") => Ok(PluginAction::Check(need("check")?)),
        Some(other) => anyhow::bail!("unknown plugin subcommand '{other}' — {USAGE}"),
    }
}

/// Substring search over the Gray Index, mirroring `gray plugin search`.
async fn search_index(query: &str) -> anyhow::Result<Vec<String>> {
    let client = gray_pkg::fetch::client()?;
    let index = gray_pkg::index::fetch_index(&client).await?;
    let mut hits: Vec<(&String, &gray_pkg::index::Entry)> = index
        .plugins
        .iter()
        .filter(|(n, _)| n.contains(query))
        .collect();
    hits.sort_by(|a, b| a.0.cmp(b.0));
    if hits.is_empty() {
        anyhow::bail!("not in index: {query} (try /plugin install <https-url>)");
    }
    Ok(hits
        .iter()
        .map(|(n, e)| format!("{n} {}", e.version))
        .collect())
}

pub(crate) async fn handle_plugin_command(raw: &str, tui: Option<&crate::composer::SharedTui>) {
    let action = match parse_plugin_args(raw) {
        Ok(a) => a,
        Err(e) => {
            say(tui, &format!("{e:#}"));
            return;
        }
    };
    match action {
        PluginAction::List => match ops::list() {
            Ok(plugins) if plugins.is_empty() => say(tui, "no plugins installed"),
            Ok(plugins) => {
                for (name, e) in &plugins {
                    let state = if e.enabled { "" } else { " [disabled]" };
                    say(tui, &format!("{name} {} ({}){state}", e.version, e.scope));
                }
            }
            Err(e) => say(tui, &format!("plugin list failed: {e:#}")),
        },
        PluginAction::Search(query) => match search_index(&query).await {
            Ok(lines) => {
                for line in lines {
                    say(tui, &line);
                }
            }
            Err(e) => say(tui, &format!("search failed: {e:#}")),
        },
        PluginAction::Install(spec) => {
            match ops::install(ops::parse_spec(&spec), InstallOpts::default()).await {
                Ok(r) => say(
                    tui,
                    &format!("installed {} {} at {}", r.name, r.version, r.path.display()),
                ),
                Err(e) => say(tui, &format!("install failed: {e:#}")),
            }
        }
        PluginAction::Remove(name) => match ops::remove(&name) {
            Ok(()) => say(tui, &format!("removed {name}")),
            Err(e) => say(tui, &format!("remove failed: {e:#}")),
        },
        PluginAction::Update(target) => match ops::update(&target).await {
            Ok(reports) if reports.is_empty() => say(tui, "up to date"),
            Ok(reports) => {
                for r in reports {
                    say(tui, &format!("updated {} {}", r.name, r.version));
                }
            }
            Err(e) => say(tui, &format!("update failed: {e:#}")),
        },
        PluginAction::Enable(name) => match ops::set_enabled(&name, true) {
            Ok(()) => say(tui, &format!("enabled {name}")),
            Err(e) => say(tui, &format!("enable failed: {e:#}")),
        },
        PluginAction::Disable(name) => match ops::set_enabled(&name, false) {
            Ok(()) => say(tui, &format!("disabled {name}")),
            Err(e) => say(tui, &format!("disable failed: {e:#}")),
        },
        PluginAction::Check(dir) => {
            if let Err(e) = crate::plugin_check::check_plugin_dir(&dir).await {
                say(tui, &format!("check failed: {e:#}"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::dispatch::{Flow, dispatch_command};
    use super::{PluginAction, parse_plugin_args};

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
        let mut config =
            crate::config::Config::resolve_with(&cli, |_| None).expect("config resolves");
        let cwd = tempfile::tempdir().expect("tempdir").keep();
        let tui: super::TuiOpt = None;
        let mut agent: Option<super::Agent> = None;
        let mut acp: Option<gray_acp::AcpSession> = None;
        let mut session_state: Option<super::SessionState> = None;
        let mut totals = super::SessionTotals::default();
        let mut pending: Option<super::ReplCommand> = None;
        let mut history = Vec::new();
        let mut unconfigured = false;
        let mut hide_thinking = false;
        let gate = gray_core::approvals::ApprovalGate::new("auto");
        let flow = dispatch_command(
            super::ReplCommand::Plugin("/plugin list".to_string()),
            &mut agent,
            &mut acp,
            &mut config,
            &cwd,
            &tui,
            &mut session_state,
            &mut totals,
            &mut pending,
            &mut history,
            &mut unconfigured,
            &mut hide_thinking,
            &gate,
        )
        .await
        .expect("dispatch ok");
        assert_eq!(flow, Flow::Continue);
        assert!(pending.is_none());
    }
}
