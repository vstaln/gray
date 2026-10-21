use super::*;
pub(crate) use crate::setup::marketplace::activation::ActivationChoice;

#[derive(Default)]
pub(crate) struct PluginCommandOutcome {
    pub installed: Vec<String>,
    pub activate_now: bool,
}

pub(crate) fn should_prompt(installed: &[String], agent_exists: bool) -> bool {
    !installed.is_empty() && agent_exists
}

pub(crate) fn should_reload(
    installed: &[String],
    choice: ActivationChoice,
    agent_exists: bool,
) -> bool {
    should_prompt(installed, agent_exists) && choice == ActivationChoice::Now
}

#[derive(Debug, PartialEq)]
pub(crate) enum PluginAction {
    List,
    Install(String),
    Remove(String),
    Update(String),
    Enable(String),
    Disable(String),
    Check(String),
}

const USAGE: &str = "usage: /plugin <list|install|remove|update|enable|disable|check>";

pub(crate) fn parse_plugin_args(raw: &str) -> anyhow::Result<PluginAction> {
    let mut toks = raw.split_whitespace();
    toks.next();
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

#[cfg(test)]
thread_local! {
    static TEST_PLUGIN_MODAL: std::cell::RefCell<Option<crate::setup::PluginModalOutcome>> =
        const { std::cell::RefCell::new(None) };
}

fn run_plugin_picker(
    bg: Option<&crate::setup::BackgroundSnapshot>,
) -> anyhow::Result<crate::setup::PluginModalOutcome> {
    #[cfg(test)]
    if let Some(outcome) = TEST_PLUGIN_MODAL.with_borrow_mut(Option::take) {
        return Ok(outcome);
    }
    crate::setup::run_plugins_modal(bg)
}

pub(crate) async fn handle_plugin_command(
    raw: &str,
    tui: Option<&crate::composer::SharedTui>,
    agent_exists: bool,
) -> PluginCommandOutcome {
    if is_bare_plugin_cmd(raw) && tui.is_some() {
        let bg = tui.map(|s| s.lock().expect("tui lock").snapshot());
        let result = with_modal_sync(tui, || run_plugin_picker(bg.as_ref()));
        let mut command_outcome = PluginCommandOutcome::default();
        match result {
            Ok(outcome) => {
                command_outcome.installed = outcome.installed;
                if outcome.changed {
                    if let Some(shared) = tui {
                        let mut t = shared.lock().expect("tui lock");
                        t.push_action("Plugins updated", None);
                        t.ensure_gap();
                        let _ = t.draw();
                    }
                } else if let Some(shared) = tui {
                    let mut t = shared.lock().expect("tui lock");
                    t.clear_draft();
                    t.ensure_gap();
                    let _ = t.draw();
                }
                for warning in outcome.warnings {
                    say(tui, &format!("Plugin install warning: {warning}"));
                }
                if should_prompt(&command_outcome.installed, agent_exists) {
                    let bg = tui.map(|s| s.lock().expect("tui lock").snapshot());
                    let choice = with_modal_sync(tui, || {
                        crate::setup::marketplace::activation::prompt(
                            bg.as_ref(),
                            &command_outcome.installed,
                        )
                    });
                    match choice {
                        Ok(ActivationChoice::Now) => command_outcome.activate_now = true,
                        Ok(ActivationChoice::Later) => {
                            say(tui, "Plugins will load on the next start or reload.");
                        }
                        Err(e) => say(
                            tui,
                            &format!(
                                "Activation prompt failed: {e}. Plugins will load on the next start or reload."
                            ),
                        ),
                    }
                }
            }
            Err(e) => {
                if let Some(shared) = tui {
                    super::notice::push_notice(
                        &mut shared.lock().expect("tui lock"),
                        "Plugin error",
                        &e.to_string(),
                        true,
                    );
                }
            }
        }
        return command_outcome;
    }
    let action = match parse_plugin_args(raw) {
        Ok(a) => a,
        Err(e) => {
            say(tui, &format!("{e:#}"));
            return PluginCommandOutcome::default();
        }
    };
    match action {
        PluginAction::List => match crate::plugin_cli::list_rows() {
            Ok(rows) if rows.is_empty() => say(tui, "no plugins installed"),
            Ok(rows) => {
                for r in &rows {
                    let state = if r.on { "" } else { " [disabled]" };
                    let kind = if r.cli { " [command]" } else { "" };
                    say(
                        tui,
                        &format!("{} {} ({}){state}{kind}", r.name, r.version, r.scope),
                    );
                }
            }
            Err(e) => say(tui, &format!("plugin list failed: {e:#}")),
        },
        PluginAction::Install(spec) => match crate::plugin_cli::home() {
            Ok(home) => match crate::plugin_cli::install_spec_lines(&home, &spec, false).await {
                Ok(lines) => {
                    for line in lines {
                        say(tui, &line);
                    }
                }
                Err(e) => say(tui, &format!("install failed: {e:#}")),
            },
            Err(e) => say(tui, &format!("install failed: {e:#}")),
        },
        PluginAction::Remove(name) => match crate::plugin_cli::remove_managed(&name) {
            Ok(()) => say(tui, &format!("removed {name}")),
            Err(e) => say(tui, &format!("remove failed: {e:#}")),
        },
        PluginAction::Update(target) => {
            let mut check = false;
            let mut parts = Vec::new();
            for tok in target.split_whitespace() {
                if tok == "--check" {
                    check = true;
                } else {
                    parts.push(tok);
                }
            }
            let target = if parts.is_empty() {
                "all".to_string()
            } else {
                parts.join(" ")
            };
            let res = if check {
                crate::plugin_cli::check_managed(&target).await
            } else {
                crate::plugin_cli::update_managed(&target).await
            };
            match res {
                Ok(reports) => {
                    let mut said = false;
                    for r in &reports {
                        if matches!(r.action, gray_pkg::ops::UpdateAction::Current) {
                            continue;
                        }
                        said = true;
                        say(tui, &format!("{}: {}", r.name, r.detail()));
                    }
                    if !said {
                        say(tui, "up to date");
                    }
                }
                Err(e) => say(tui, &format!("update failed: {e:#}")),
            }
        }
        PluginAction::Enable(name) => match crate::plugin_cli::set_managed_enabled(&name, true) {
            Ok(()) => say(tui, &format!("enabled {name}")),
            Err(e) => say(tui, &format!("enable failed: {e:#}")),
        },
        PluginAction::Disable(name) => match crate::plugin_cli::set_managed_enabled(&name, false) {
            Ok(()) => say(tui, &format!("disabled {name}")),
            Err(e) => say(tui, &format!("disable failed: {e:#}")),
        },
        PluginAction::Check(dir) => {
            if let Err(e) = crate::plugin_check::check_plugin_dir(&dir).await {
                say(tui, &format!("check failed: {e:#}"));
            }
        }
    }
    PluginCommandOutcome::default()
}

#[path = "plugin_cmds_tests.rs"]
#[cfg(test)]
mod tests;

pub(crate) async fn provider_command_outcome(
    provider_id: &str,
    cmd: &str,
    argv: Vec<String>,
) -> Option<gray_core::agent::CommandOutcome> {
    use gray_plugin::Plugin as _;
    let home = crate::setup::gray_home().ok()?;
    let installed = crate::providers::ProviderRegistry::load_cached(&home)
        .installed()
        .into_iter()
        .find(|p| p.provider_id() == provider_id)?;
    let plugin = gray_plugin::sidecar::SidecarPlugin::spawn(installed.argv.clone())
        .await
        .ok()?;
    let outcome = plugin.run_command(cmd, argv).await;
    plugin.shutdown(std::time::Duration::from_secs(2)).await;
    outcome
}
