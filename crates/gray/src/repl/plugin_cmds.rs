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

/// Bare `/plugin` or `/plugins` (no args, case-insensitive): a single
/// whitespace-delimited token naming the command. Trailing spaces still
/// count as bare; anything else (explicit subcommand) does not.
pub(crate) fn is_bare_plugin_cmd(raw: &str) -> bool {
    let toks: Vec<&str> = raw.split_whitespace().collect();
    if toks.len() != 1 {
        return false;
    }
    let Some(cmd) = toks[0].strip_prefix('/') else {
        return false;
    };
    matches!(cmd.to_ascii_lowercase().as_str(), "plugin" | "plugins")
}

/// Bare `/marketplace` (no args, case-insensitive): same bare-token shape
/// as [`is_bare_plugin_cmd`]; anything else is usage, not the store.
pub(crate) fn is_bare_marketplace_cmd(raw: &str) -> bool {
    let toks: Vec<&str> = raw.split_whitespace().collect();
    if toks.len() != 1 {
        return false;
    }
    let Some(cmd) = toks[0].strip_prefix('/') else {
        return false;
    };
    cmd.eq_ignore_ascii_case("marketplace")
}

/// Bare `/marketplace` opens the store modal (TTY only); headless prints
/// a TTY note, non-bare prints usage (the store has no subcommands).
pub(crate) async fn handle_marketplace_command(
    raw: &str,
    tui: Option<&crate::composer::SharedTui>,
) {
    if is_bare_marketplace_cmd(raw) && tui.is_some() {
        let bg = tui.map(|s| s.lock().expect("tui lock").snapshot());
        let result = with_modal_sync(tui, || crate::setup::run_marketplace_modal(bg.as_ref()));
        match result {
            Ok(true) => {
                if let Some(shared) = tui {
                    let mut t = shared.lock().expect("tui lock");
                    t.push_action("Marketplace updated", None);
                    let _ = t.draw();
                }
            }
            Ok(false) => {
                if let Some(shared) = tui {
                    let mut t = shared.lock().expect("tui lock");
                    t.textarea.set_text("");
                    t.matches.clear();
                    t.sel = 0;
                    t.history_idx = None;
                    t.draft.clear();
                    t.attachments.clear();
                    t.pending_pastes.clear();
                    let _ = t.draw();
                }
            }
            Err(e) => {
                if let Some(shared) = tui {
                    shared
                        .lock()
                        .expect("tui lock")
                        .push_dim(format!("└ error: {e}"));
                }
            }
        }
        return;
    }
    if is_bare_marketplace_cmd(raw) {
        say(tui, "marketplace needs an interactive terminal");
        return;
    }
    say(tui, "usage: /marketplace");
}

pub(crate) async fn handle_plugin_command(raw: &str, tui: Option<&crate::composer::SharedTui>) {
    // Bare `/plugin` or `/plugins` with a TTY opens the interactive picker;
    // explicit subcommands and headless runs keep the text output below.
    if is_bare_plugin_cmd(raw) && tui.is_some() {
        let bg = tui.map(|s| s.lock().expect("tui lock").snapshot());
        let result = with_modal_sync(tui, || crate::setup::run_plugins_modal(bg.as_ref()));
        match result {
            Ok(true) => {
                if let Some(shared) = tui {
                    let mut t = shared.lock().expect("tui lock");
                    t.push_action("Plugins updated", None);
                    t.ensure_gap(1);
                    let _ = t.draw();
                }
            }
            Ok(false) => {
                if let Some(shared) = tui {
                    let mut t = shared.lock().expect("tui lock");
                    t.textarea.set_text("");
                    t.matches.clear();
                    t.sel = 0;
                    t.history_idx = None;
                    t.draft.clear();
                    t.attachments.clear();
                    t.pending_pastes.clear();
                    // Dismissed picker leaves the slash card with no feedback:
                    // restore the trailing gap so it doesn't jam the input box.
                    t.ensure_gap(1);
                    let _ = t.draw();
                }
            }
            Err(e) => {
                if let Some(shared) = tui {
                    shared
                        .lock()
                        .expect("tui lock")
                        .push_dim(format!("└ error: {e}"));
                }
            }
        }
        return;
    }
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
        PluginAction::Search(query) => match ops::search_all(&query).await {
            Ok(out) => {
                if out.hits.is_empty()
                    && !out.pi_unreachable
                    && !out.gray_unreachable
                    && !out.clawhub_unreachable
                    && !out.claude_unreachable
                {
                    say(
                        tui,
                        &format!("not in index: {query} (try /plugin install <https-url>)"),
                    );
                } else {
                    for hit in &out.hits {
                        say(tui, &ops::format_search_hit(hit));
                    }
                    if out.gray_unreachable {
                        say(tui, ops::GRAY_UNREACHABLE_LINE);
                    }
                    if out.pi_unreachable {
                        say(tui, ops::PI_UNREACHABLE_LINE);
                    }
                    if out.clawhub_unreachable {
                        say(tui, ops::CLAWHUB_UNREACHABLE_LINE);
                    }
                    if out.claude_unreachable {
                        say(tui, ops::CLAUDE_UNREACHABLE_LINE);
                    }
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
