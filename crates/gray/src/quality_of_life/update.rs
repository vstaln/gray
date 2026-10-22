use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::repl::TuiOpt;

const CHECK_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(1500);
const RESUME_OPTION: &str = "Continue sessions after restart";

struct ProviderCli {
    bin: &'static str,
    npm: &'static str,
    brew: &'static [&'static str],
}

const PROVIDERS: &[ProviderCli] = &[
    ProviderCli {
        bin: "claude",
        npm: "@anthropic-ai/claude-code",
        brew: &["claude-code"],
    },
    ProviderCli {
        bin: "codex",
        npm: "@openai/codex",
        brew: &["codex"],
    },
    ProviderCli {
        bin: "opencode",
        npm: "opencode-ai",
        brew: &["opencode"],
    },
];

enum Step {
    Provider { label: String, cmd: Vec<String> },
    Plugins,
    Gray { latest: String },
}

struct Plan {
    lines: Vec<String>,
    steps: Vec<Step>,
}

fn say(tui: &TuiOpt, msg: &str) {
    crate::repl::say(tui.as_ref().map(|(s, _)| s), msg);
}

fn log_path() -> PathBuf {
    crate::setup::gray_home()
        .map(|h| h.join("logs").join("update-everything.log"))
        .unwrap_or_else(|_| std::env::temp_dir().join("gray-update-everything.log"))
}

fn npm_outdated() -> Option<BTreeMap<String, (String, String)>> {
    let out = std::process::Command::new("npm")
        .args(["outdated", "-g", "--json"])
        .output()
        .ok()?;
    if out.stdout.is_empty() {
        return Some(BTreeMap::new());
    }
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
    let mut m = BTreeMap::new();
    for (name, info) in v.as_object()? {
        let cur = info
            .get("current")
            .and_then(|x| x.as_str())
            .unwrap_or("?")
            .to_string();
        let latest = info
            .get("latest")
            .or_else(|| info.get("wanted"))
            .and_then(|x| x.as_str())
            .unwrap_or("?")
            .to_string();
        m.insert(name.clone(), (cur, latest));
    }
    Some(m)
}

fn brew_outdated() -> Option<BTreeMap<String, (String, String)>> {
    let out = std::process::Command::new("brew")
        .args(["outdated", "--json"])
        .output()
        .ok()?;
    if out.stdout.is_empty() {
        return Some(BTreeMap::new());
    }
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
    let mut m = BTreeMap::new();
    if let Some(rows) = v.get("formulae").and_then(|x| x.as_array()) {
        for e in rows {
            let Some(name) = e.get("name").and_then(|x| x.as_str()) else {
                continue;
            };
            let cur = e
                .get("installed_versions")
                .and_then(|x| x.as_array())
                .and_then(|a| a.first())
                .and_then(|x| x.as_str())
                .unwrap_or("?");
            let latest = e
                .get("current_version")
                .and_then(|x| x.as_str())
                .unwrap_or("?");
            m.insert(name.to_string(), (cur.to_string(), latest.to_string()));
        }
    }
    if let Some(rows) = v.get("casks").and_then(|x| x.as_array()) {
        for e in rows {
            let Some(name) = e.get("name").and_then(|x| x.as_str()) else {
                continue;
            };
            let cur = e
                .get("installed_versions")
                .and_then(|x| x.as_str())
                .unwrap_or("?");
            let latest = e
                .get("current_version")
                .and_then(|x| x.as_str())
                .unwrap_or("?");
            m.insert(name.to_string(), (cur.to_string(), latest.to_string()));
        }
    }
    Some(m)
}

#[cfg(unix)]
fn pid_alive(pid: u32) -> bool {
    (unsafe { libc::kill(pid as i32, 0) } == 0)
        || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(not(unix))]
fn pid_alive(_pid: u32) -> bool {
    false
}

#[cfg(unix)]
fn provider_running(bin: &str) -> bool {
    let Ok(rd) = std::fs::read_dir("/proc") else {
        return false;
    };
    let me = std::process::id().to_string();
    for e in rd.flatten() {
        let name = e.file_name();
        let Some(pid) = name.to_str() else {
            continue;
        };
        if !pid.bytes().all(|b| b.is_ascii_digit()) || pid == me {
            continue;
        }
        if std::fs::read_to_string(e.path().join("comm"))
            .map(|c| c.trim() == bin)
            .unwrap_or(false)
        {
            return true;
        }
        if let Ok(cmdline) = std::fs::read(e.path().join("cmdline")) {
            for arg in cmdline.split(|b| *b == 0) {
                let arg = std::str::from_utf8(arg).unwrap_or("");
                if Path::new(arg).file_name().and_then(|n| n.to_str()) == Some(bin) {
                    return true;
                }
            }
        }
    }
    false
}

#[cfg(not(unix))]
fn provider_running(_bin: &str) -> bool {
    false
}

fn open_holder_pid(path: &Path) -> Option<u32> {
    let content = std::fs::read_to_string(path).ok()?;
    let content = content.trim();
    serde_json::from_str::<serde_json::Value>(content)
        .ok()
        .and_then(|v| v.get("pid").and_then(|p| p.as_u64()))
        .map(|p| p as u32)
        .or_else(|| content.parse::<u32>().ok())
}

fn open_lock_held(path: &Path) -> bool {
    let Ok(f) = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
    else {
        return false;
    };
    matches!(f.try_lock(), Err(std::fs::TryLockError::WouldBlock))
}

fn live_sessions() -> usize {
    let Ok(home) = crate::setup::gray_home() else {
        return 0;
    };
    let Ok(rd) = std::fs::read_dir(home.join("sessions")) else {
        return 0;
    };
    let me = std::process::id();
    let mut n = 0;
    for e in rd.flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("open") {
            continue;
        }
        match open_holder_pid(&p) {
            Some(pid) if pid != me && pid_alive(pid) => n += 1,
            Some(_) => {}
            None if open_lock_held(&p) => n += 1,
            None => {}
        }
    }
    n
}

async fn build_plan() -> Plan {
    let mut lines = Vec::new();
    let mut steps = Vec::new();

    let current = env!("CARGO_PKG_VERSION");
    match tokio::time::timeout(CHECK_TIMEOUT, crate::update::latest_version()).await {
        Ok(Ok(latest))
            if crate::update::update_available(
                crate::update::CHANNEL,
                &latest,
                current,
                env!("GRAY_BUILD_ID"),
            ) =>
        {
            lines.push(format!("gray {current} → {latest}"));
            steps.push(Step::Gray { latest });
        }
        Ok(Ok(_)) => lines.push(format!("gray {current} already current")),
        Ok(Err(e)) => lines.push(format!("gray update check failed: {e}")),
        Err(_) => lines.push("gray update check timed out".to_string()),
    }

    match crate::plugin_cli::check_managed("all").await {
        Ok(rows) if rows.is_empty() => lines.push("plugins: none installed".to_string()),
        Ok(rows) => {
            let stale: Vec<String> = rows
                .iter()
                .filter(|r| matches!(r.action, gray_pkg::ops::UpdateAction::Stale(_)))
                .map(|r| r.name.clone())
                .collect();
            if stale.is_empty() {
                lines.push(format!("plugins: {} installed, all current", rows.len()));
            } else {
                lines.push(format!("plugins to update: {}", stale.join(", ")));
                steps.push(Step::Plugins);
            }
        }
        Err(e) => lines.push(format!("plugin check failed: {e}")),
    }

    let npm = tokio::task::spawn_blocking(npm_outdated)
        .await
        .ok()
        .flatten()
        .unwrap_or_default();
    let brew = tokio::task::spawn_blocking(brew_outdated)
        .await
        .ok()
        .flatten()
        .unwrap_or_default();
    for p in PROVIDERS {
        let npm_hit = npm.get(p.npm);
        let brew_hit = p.brew.iter().find_map(|n| brew.get(*n));
        if npm_hit.is_none() && brew_hit.is_none() {
            continue;
        }
        let running = provider_running(p.bin);
        if let Some((cur, latest)) = npm_hit {
            if running {
                lines.push(format!(
                    "{} {} → {} (npm) — skipped: {bin} is running",
                    p.npm,
                    cur,
                    latest,
                    bin = p.bin
                ));
            } else {
                lines.push(format!("{} {} → {} (npm)", p.npm, cur, latest));
                steps.push(Step::Provider {
                    label: p.npm.to_string(),
                    cmd: vec!["npm".into(), "install".into(), "-g".into(), p.npm.into()],
                });
            }
            continue;
        }
        if let Some((cur, latest)) = brew_hit {
            let name = p
                .brew
                .iter()
                .find(|n| brew.contains_key(**n))
                .copied()
                .unwrap_or(p.bin);
            if running {
                lines.push(format!(
                    "{name} {cur} → {latest} (brew) — skipped: {bin} is running",
                    bin = p.bin
                ));
            } else {
                lines.push(format!("{name} {cur} → {latest} (brew)"));
                steps.push(Step::Provider {
                    label: name.to_string(),
                    cmd: vec!["brew".into(), "upgrade".into(), name.into()],
                });
            }
        }
    }

    let n = live_sessions();
    if n > 0 {
        lines.push(format!(
            "{n} running session{} will be interrupted",
            if n == 1 { "" } else { "s" }
        ));
    }

    Plan { lines, steps }
}

fn run_logged(cmd: &[String], log: &Path) -> bool {
    use std::io::Write as _;
    let Some((prog, args)) = cmd.split_first() else {
        return false;
    };
    let out = std::process::Command::new(prog).args(args).output();
    if let Some(parent) = log.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log)
    {
        let _ = writeln!(f, "$ {}", cmd.join(" "));
        if let Ok(o) = &out {
            let _ = f.write_all(&o.stdout);
            let _ = f.write_all(&o.stderr);
        }
    }
    out.map(|o| o.status.success()).unwrap_or(false)
}

async fn apply(tui: &TuiOpt, steps: &[Step], report: &mut dyn FnMut(&str)) -> bool {
    let log = log_path();
    let mut gray_updated = false;
    for step in steps {
        match step {
            Step::Provider { label, cmd } => {
                let cmd = cmd.clone();
                let lg = log.clone();
                let ok = tokio::task::spawn_blocking(move || run_logged(&cmd, &lg))
                    .await
                    .unwrap_or(false);
                report(&format!(
                    "{label}: {}{}",
                    if ok { "updated" } else { "failed" },
                    if ok {
                        String::new()
                    } else {
                        format!(" (log: {})", log.display())
                    }
                ));
            }
            Step::Plugins => match crate::plugin_cli::update_managed("all").await {
                Ok(rows) => {
                    if rows.is_empty() {
                        report("plugins: nothing installed");
                    }
                    for r in rows {
                        report(&format!("plugin {}: {}", r.name, r.detail()));
                    }
                }
                Err(e) => report(&format!("plugins: failed: {e}")),
            },
            Step::Gray { latest } => {
                let shared = tui.as_ref().map(|(s, _)| s);
                let res = crate::repl::maintenance::with_terminal(shared, crate::update::install);
                match res {
                    Ok(()) => {
                        crate::update::warn_on_shadow();
                        gray_updated = true;
                        report(&format!("gray: updated to {latest}"));
                    }
                    Err(e) => report(&format!("gray: failed: {e}")),
                }
            }
        }
    }
    gray_updated
}

pub(crate) async fn handle(tui: &TuiOpt) {
    if tui.is_none() {
        run_cli().await;
        return;
    }
    say(tui, "checking updates for gray, plugins and provider CLIs…");
    let plan = build_plan().await;
    say(tui, &plan.lines.join("\n"));
    if plan.steps.is_empty() {
        say(tui, "nothing to update — already current");
        return;
    }
    let shared = tui.as_ref().map(|(s, _)| s).expect("checked above");
    let Some(answers) = crate::ask::ask_questions_modal(
        shared,
        vec![crate::ask::AskQuestion {
            id: "update-everything".into(),
            header: "update everything".into(),
            question: "apply the updates above? (Enter runs, Esc cancels)".into(),
            detail: String::new(),
            options: vec![crate::ask::AskOption {
                label: RESUME_OPTION.into(),
                description: "restart re-enters this conversation".into(),
                opt_in: false,
                freeform: false,
            }],
            multi: true,
            default_all: true,
            no_other: false,
        }],
    ) else {
        say(tui, "cancelled — nothing changed");
        return;
    };
    let resume = answers
        .iter()
        .any(|a| a.answers.iter().any(|x| x == RESUME_OPTION));
    let mut lines = Vec::new();
    let gray_updated = apply(tui, &plan.steps, &mut |l: &str| {
        lines.push(l.to_string());
    })
    .await;
    lines.push(format!("log: {}", log_path().display()));
    say(tui, &lines.join("\n"));
    if gray_updated {
        crate::repl::maintenance::restart_with(tui, resume).await;
    }
}

pub async fn cli() -> anyhow::Result<()> {
    run_cli().await;
    Ok(())
}

async fn run_cli() {
    let plan = build_plan().await;
    for l in &plan.lines {
        println!("{l}");
    }
    if plan.steps.is_empty() {
        println!("nothing to update — already current");
        return;
    }
    print!("apply these updates? [y/N] ");
    use std::io::Write as _;
    let _ = std::io::stdout().flush();
    let mut buf = String::new();
    if std::io::stdin().read_line(&mut buf).unwrap_or(0) == 0
        || !matches!(buf.trim(), "y" | "Y" | "yes" | "YES")
    {
        println!("cancelled — nothing changed");
        return;
    }
    let tui: TuiOpt = None;
    let mut lines = Vec::new();
    let gray_updated = apply(&tui, &plan.steps, &mut |l: &str| {
        lines.push(l.to_string());
    })
    .await;
    for l in lines {
        println!("{l}");
    }
    println!("log: {}", log_path().display());
    if gray_updated {
        println!("restart gray to run the new build");
    }
}
