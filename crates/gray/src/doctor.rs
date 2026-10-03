//! `gray doctor` — is this setup usable?
//!
//! Diagnose only. Nothing here writes a config, installs a plugin, or starts
//! the gateway: a tool that "fixes" things on the way past is a tool that can
//! break a working setup, and the fix is always one command the user can see.
//! A failing check prints what it found and exits non-zero; a check that could
//! not run (an offline provider call, a platform-only probe) is `skip`, never
//! a failure.

use crate::config::Config;

/// One line of the report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub name: &'static str,
    pub status: Status,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Pass,
    /// Something the user probably wants to know, that is not broken.
    Warn,
    Fail,
    /// Not checked here (needs the network, or does not apply to this host).
    Skip,
}

impl Status {
    fn label(self) -> &'static str {
        match self {
            Status::Pass => "ok  ",
            Status::Warn => "warn",
            Status::Fail => "FAIL",
            Status::Skip => "skip",
        }
    }
}

impl Check {
    fn new(name: &'static str, status: Status, detail: impl Into<String>) -> Self {
        Self {
            name,
            status,
            detail: detail.into(),
        }
    }
}

/// The report as printed, one check per line.
pub fn render(checks: &[Check]) -> String {
    let width = checks.iter().map(|c| c.name.len()).max().unwrap_or(0);
    let mut out = String::new();
    for check in checks {
        out.push_str(&format!(
            "{:width$}  {}  {}\n",
            check.name,
            check.status.label(),
            check.detail,
            width = width
        ));
    }
    let failed = checks.iter().filter(|c| c.status == Status::Fail).count();
    let warned = checks.iter().filter(|c| c.status == Status::Warn).count();
    out.push_str(&if failed == 0 {
        format!("{warned} warning(s), no failures\n")
    } else {
        format!("{failed} failure(s), {warned} warning(s)\n")
    });
    out
}

/// 0 when nothing failed. Warnings (a stopped gateway, a missing model) are
/// states, not breakage, and must not fail a health check a script runs.
pub fn exit_code(checks: &[Check]) -> i32 {
    i32::from(checks.iter().any(|c| c.status == Status::Fail))
}

/// Runs the checks against this machine. `online` adds the one check that
/// spends a request: a single-token call to the configured model. It is opt-in
/// because a health check nobody asked to spend money on is a bill.
pub fn run(config: &Config, online: bool) -> i32 {
    let checks = collect(config, online);
    print!("{}", render(&checks));
    exit_code(&checks)
}

fn collect(config: &Config, online: bool) -> Vec<Check> {
    vec![
        home(),
        credentials(config),
        model(config),
        context_window(config),
        shell(),
        exec_prefix(config),
        gateway(),
        plugins(),
        provider_call(config, online),
    ]
}

/// The home dir must exist and accept a write: every session, log, memory entry
/// and cron job lands there, and a read-only home fails at the worst moment.
fn home() -> Check {
    let name = "gray home";
    match crate::setup::gray_home() {
        Err(e) => Check::new(name, Status::Fail, format!("unresolvable: {e:#}")),
        Ok(home) => {
            let probe = home.join(format!(".doctor-{}", std::process::id()));
            match std::fs::write(&probe, b"") {
                Ok(()) => {
                    let _ = std::fs::remove_file(&probe);
                    Check::new(name, Status::Pass, home.display().to_string())
                }
                Err(e) => Check::new(
                    name,
                    Status::Fail,
                    format!("{} is not writable: {e}", home.display()),
                ),
            }
        }
    }
}

/// Whether *this* provider has a credential. The value is never printed — a
/// health report lands in bug reports and terminals.
fn credentials(config: &Config) -> Check {
    let name = "provider key";
    if config.uses_plugin_credentials() {
        return Check::new(
            name,
            Status::Pass,
            format!("plugin credential ({})", config.provider_id),
        );
    }
    match config.api_key.as_deref() {
        Some(key) if !key.trim().is_empty() => Check::new(
            name,
            Status::Pass,
            format!("{} ({} chars)", config.base_url, key.len()),
        ),
        _ => Check::new(
            name,
            Status::Fail,
            format!(
                "no key for {} — run /connect",
                crate::config::DEFAULT_BASE_URL
            ),
        ),
    }
}

fn model(config: &Config) -> Check {
    let name = "model";
    match config.model.as_deref().filter(|m| !m.is_empty()) {
        Some(model) => Check::new(name, Status::Pass, model.to_string()),
        None => Check::new(
            name,
            Status::Warn,
            "not set yet — /model picks one (a fresh session asks on first use)".to_string(),
        ),
    }
}

/// The window decides when auto-compact fires, so *where* it came from is the
/// interesting part: a fallback table entry silently truncates long sessions.
fn context_window(config: &Config) -> Check {
    let name = "context window";
    let Some(model) = config.model.as_deref().filter(|m| !m.is_empty()) else {
        return Check::new(name, Status::Skip, "no model selected yet".to_string());
    };
    let resolved = crate::setup::context::resolve_model_context_length(model);
    // `guess` means the built-in table, which silently truncates a long
    // session at a window the model does not actually have: worth naming.
    let source = match crate::setup::context::context_source(model) {
        "guess" => "built-in table (refresh with /model)",
        other => other,
    };
    Check::new(name, Status::Pass, format!("{resolved} tokens ({source})"))
}

/// The shell the model's `bash` actually uses. On Windows this is Git Bash,
/// whose absence is the single most common "it worked yesterday" failure.
fn shell() -> Check {
    let name = "shell";
    #[cfg(windows)]
    {
        match gray_tools::shell::shell_path() {
            Ok(path) => Check::new(name, Status::Pass, path.display().to_string()),
            Err(e) => Check::new(name, Status::Fail, format!("{e}")),
        }
    }
    #[cfg(not(windows))]
    {
        match std::process::Command::new("sh")
            .arg("-c")
            .arg("exit 0")
            .status()
        {
            Ok(status) if status.success() => Check::new(name, Status::Pass, "sh -c".to_string()),
            Ok(status) => Check::new(name, Status::Fail, format!("sh -c exited {status}")),
            Err(e) => Check::new(name, Status::Fail, format!("no `sh` on PATH: {e}")),
        }
    }
}

/// Reported, never run: a prefix points at a container or a remote host, and a
/// health check has no business executing a command over the network.
fn exec_prefix(config: &Config) -> Check {
    let name = "exec prefix";
    match config
        .exec_prefix
        .as_deref()
        .filter(|p| !p.trim().is_empty())
    {
        Some(prefix) => Check::new(
            name,
            Status::Pass,
            format!("{prefix} (not exercised: it runs commands elsewhere)"),
        ),
        None => Check::new(
            name,
            Status::Pass,
            "none (commands run locally)".to_string(),
        ),
    }
}

/// A stopped gateway is a state, not a failure: plenty of sessions never use
/// one. Stale state (a pid file with no process) is worth a warning because it
/// is what `gray gateway start` trips over.
fn gateway() -> Check {
    let name = "gateway";
    let Ok(home) = crate::setup::gray_home() else {
        return Check::new(name, Status::Skip, "home unresolvable".to_string());
    };
    let recorded = crate::gateway::pid::read(&home);
    match (recorded.as_ref(), crate::gateway::pid::running(&home)) {
        (_, Some(record)) => {
            Check::new(name, Status::Pass, format!("running (pid {})", record.pid))
        }
        (Some(_), None) => Check::new(
            name,
            Status::Warn,
            "state file present but no process — `gray gateway start`".to_string(),
        ),
        (None, _) => {
            if crate::setup::gw_auto_enabled() {
                Check::new(
                    name,
                    Status::Pass,
                    "not running (nothing to route)".to_string(),
                )
            } else {
                Check::new(
                    name,
                    Status::Skip,
                    "off (`/gateway on` to enable)".to_string(),
                )
            }
        }
    }
}

/// Installed plugins, and whether their binary is actually on disk. A plugin
/// whose entry survives but whose executable does not fails at command time
/// with a bare ENOENT.
fn plugins() -> Check {
    let name = "plugins";
    match crate::plugin_cli::list_rows() {
        Err(e) => Check::new(name, Status::Warn, format!("cannot list: {e:#}")),
        Ok(rows) if rows.is_empty() => Check::new(name, Status::Pass, "none installed".to_string()),
        Ok(rows) => {
            let enabled: Vec<&str> = rows
                .iter()
                .filter(|r| r.on)
                .map(|r| r.name.as_str())
                .collect();
            if enabled.is_empty() {
                Check::new(
                    name,
                    Status::Warn,
                    format!("{} installed, all disabled", rows.len()),
                )
            } else {
                Check::new(
                    name,
                    Status::Pass,
                    format!("{} enabled: {}", enabled.len(), enabled.join(", ")),
                )
            }
        }
    }
}

/// Reach the provider: a `GET /models` against the configured base URL with
/// the configured key. It costs no tokens (unlike a completion), it is the
/// same call every connect flow makes, and an empty list is the provider
/// saying no -- wrong key, wrong base URL, or a network that is not there.
///
/// Opt-in, because it leaves the machine.
fn provider_call(config: &Config, online: bool) -> Check {
    let name = "provider reachability";
    if !online {
        return Check::new(
            name,
            Status::Skip,
            "not run — pass --online to make one request".to_string(),
        );
    }
    if !config.uses_plugin_credentials()
        && config.api_key.as_deref().unwrap_or("").trim().is_empty()
    {
        return Check::new(name, Status::Skip, "no credential".to_string());
    }
    if config.uses_plugin_credentials() {
        return Check::new(
            name,
            Status::Skip,
            "plugin-backed provider — its own doctor checks it".to_string(),
        );
    }
    // Subscription routes drive a local CLI, not HTTP: probe the binary.
    if let Some(model) = config.model.as_deref()
        && let Some(_native) = model.strip_prefix(gray_provider::claude_subscription::MODEL_PREFIX)
    {
        return match gray_provider::claude_subscription::resolve_command() {
            Some(bin) => Check::new(name, Status::Pass, format!("claude CLI at {bin}")),
            None => Check::new(
                name,
                Status::Fail,
                "`claude` not found on PATH (npm install -g @anthropic-ai/claude-code, then `claude auth login`)".to_string(),
            ),
        };
    }
    let models = crate::setup::context::fetch_live_provider_models(
        &config.base_url,
        config.api_key.as_deref(),
    );
    if models.is_empty() {
        Check::new(
            name,
            Status::Fail,
            format!(
                "{} returned no models (key, base URL, or network)",
                config.base_url
            ),
        )
    } else {
        Check::new(
            name,
            Status::Pass,
            format!("{} · {} models", config.base_url, models.len()),
        )
    }
}

#[path = "doctor_tests.rs"]
#[cfg(test)]
mod tests;
