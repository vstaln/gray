use super::*;

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

pub(crate) async fn handle_plugin_command(raw: &str, tui: Option<&crate::composer::SharedTui>) {
    let action = match parse_plugin_args(raw) {
        Ok(a) => a,
        Err(e) => {
            say(tui, &format!("{e:#}"));
            return;
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
            // The conformance probe lives in the plugin-dev plugin now —
            // `gray plugin check` forwards to it.
            let exe = crate::exe_watch::on_disk_exe();
            match exe {
                Err(e) => say(tui, &format!("cannot self-spawn: {e:#}")),
                Ok(exe) => {
                    let out = tokio::time::timeout(
                        std::time::Duration::from_secs(60),
                        tokio::process::Command::new(exe)
                            .args(["plugin", "check", &dir])
                            .stdin(std::process::Stdio::null())
                            .output(),
                    )
                    .await;
                    match out {
                        Err(_) => say(tui, "plugin check timed out"),
                        Ok(Err(e)) => say(tui, &format!("plugin check failed to start: {e}")),
                        Ok(Ok(o)) => {
                            let text = String::from_utf8_lossy(&o.stdout).trim_end().to_string();
                            if text.is_empty() {
                                say(tui, "check produced no output");
                            } else {
                                say(tui, &text);
                            }
                        }
                    }
                }
            }
        }
    }
}

#[path = "plugin_cmds_tests.rs"]
#[cfg(test)]
mod tests;

pub(crate) async fn provider_command_outcome(
    provider_id: &str,
    cmd: &str,
    argv: Vec<String>,
    raw: Option<String>,
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
    let outcome = plugin.run_command(cmd, argv, raw).await;
    plugin.shutdown(std::time::Duration::from_secs(2)).await;
    outcome
}
