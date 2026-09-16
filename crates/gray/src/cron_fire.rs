//! Cron fire helpers: pure prompt assembly, wake-gate/silence parsing,
//! transcript rendering. No I/O here except via `run_pre_script`'s caller.

use std::path::PathBuf;

/// Cap for injected pre-script stdout (hermes `_MAX_CONTEXT_CHARS`).
pub const SCRIPT_OUTPUT_CAP: usize = 8000;

/// Wake gate: false only when the last non-empty stdout line is JSON
/// `{"wakeAgent": false}`; anything else (empty, non-JSON, missing flag,
/// `true`) wakes normally.
pub fn parse_wake_gate(output: &str) -> bool {
    let last = output.lines().rev().find(|l| !l.trim().is_empty());
    let Some(line) = last else { return true };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(line.trim()) else {
        return true;
    };
    !matches!(v.get("wakeAgent"), Some(serde_json::Value::Bool(false)))
}

/// `[SILENT]` (case-insensitive, trimmed) as the whole body, first line, or
/// last line suppresses delivery; the fire still records `ok`. Forgiving of
/// model output (hermes parity); a marker buried mid-body is ignored.
pub fn is_silent_response(text: &str) -> bool {
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    let Some(first) = lines.next() else {
        return false;
    };
    if first.eq_ignore_ascii_case("[silent]") {
        return true;
    }
    text.lines()
        .map(str::trim)
        .rfind(|l| !l.is_empty())
        .is_some_and(|l| l.eq_ignore_ascii_case("[silent]"))
}

/// Final prompt: optional `## skills` block (exact `<location>` paths, one
/// per line) + optional fenced `## script-output` block + base prompt.
/// Empty sections are omitted (byte-stable for the common prompt-only job).
pub fn assemble_fire_prompt(
    base: &str,
    skill_paths: &[PathBuf],
    script_output: Option<&str>,
) -> String {
    let mut out = String::new();
    if !skill_paths.is_empty() {
        out.push_str("## skills\nThe following skills apply to this task; read the SKILL.md at each <location> with bash before starting:\n\n");
        for p in skill_paths {
            out.push_str(&format!("- <location>{}</location>\n", p.display()));
        }
        out.push('\n');
    }
    if let Some(body) = script_output {
        let capped: String = body.chars().take(SCRIPT_OUTPUT_CAP).collect();
        out.push_str("## script-output\nPre-run data collection (stdout, capped):\n\n```\n");
        out.push_str(&capped);
        out.push_str("\n```\n\n");
    }
    out.push_str(base);
    out
}

/// Render collected `AgentEvent`s into a storable transcript: text deltas
/// concatenated verbatim; tool calls noted by name so silent-but-active
/// runs still leave a trace.
pub fn transcript_text(events: &[gray_core::event::AgentEvent]) -> String {
    use gray_core::event::AgentEvent;
    let mut text = String::new();
    for ev in events {
        match ev {
            AgentEvent::TextDelta { delta } => text.push_str(delta),
            AgentEvent::ToolCallStart { name, .. } => {
                text.push_str(&format!("\n[tool:{name}]\n"));
            }
            AgentEvent::ToolResult {
                output, is_error, ..
            } => {
                let tag = if *is_error { "result-err" } else { "result" };
                let s = output.chars().take(2000).collect::<String>();
                text.push_str(&format!("[{tag}:{s}]\n"));
            }
            _ => {}
        }
    }
    text
}

/// Pre-script wall clock: a pre-step must stay inside one tick window.
pub const SCRIPT_TIMEOUT_SECS: u64 = 300;

pub struct ScriptOutcome {
    pub ok: bool,
    pub stdout: String,
    pub stderr_tail: String,
}

/// Run the job's pre-script with cwd=`workdir`, piped stdio. Missing file,
/// spawn failure, nonzero exit, or timeout → `ok: false` (caller records
/// `error` without running the agent). Blocking `Command` runs inside
/// `spawn_blocking` so the ticker stays responsive.
pub async fn run_pre_script(script: &std::path::Path, workdir: &std::path::Path) -> ScriptOutcome {
    let script = script.to_path_buf();
    let workdir = workdir.to_path_buf();
    let join = tokio::task::spawn_blocking(move || {
        std::process::Command::new(&script)
            .current_dir(&workdir)
            .output()
    })
    .await;
    // The inner closure has no deadline; the timeout below fires on the
    // *join*, abandoning a hung script thread (one blocked thread leaks
    // until process exit — accepted, same as a hung tool call; the fire
    // still records `error` on time).
    let res = tokio::time::timeout(
        std::time::Duration::from_secs(SCRIPT_TIMEOUT_SECS),
        async move { join },
    )
    .await;
    match res {
        Err(_) => ScriptOutcome {
            ok: false,
            stdout: String::new(),
            stderr_tail: "pre-script timed out".to_string(),
        },
        Ok(Err(e)) => ScriptOutcome {
            ok: false,
            stdout: String::new(),
            stderr_tail: format!("pre-script spawn failed: {e:#}"),
        },
        Ok(Ok(Err(e))) => ScriptOutcome {
            ok: false,
            stdout: String::new(),
            stderr_tail: format!("pre-script failed: {e:#}"),
        },
        Ok(Ok(Ok(out))) => {
            let tail: String = String::from_utf8_lossy(&out.stderr)
                .chars()
                .rev()
                .take(2000)
                .collect::<String>()
                .chars()
                .rev()
                .collect();
            ScriptOutcome {
                ok: out.status.success(),
                stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
                stderr_tail: tail,
            }
        }
    }
}

/// Write the transcript to `$HOME/cron/output/<job-id>/<unix-ts>.md`
/// (0600, atomic tmp+rename). Header carries name/id/fire-time/schedule.
/// Raw-bytes twin of the store's JSON writer (kept here so `gray-cron`
/// keeps no second copy): unique private tmp, mode at creation, dir sync.
pub fn write_local_output(
    home: &std::path::Path,
    job: &gray_cron::CronJob,
    now: i64,
    body: &str,
) -> anyhow::Result<std::path::PathBuf> {
    use std::io::Write as _;
    let dir = home.join("cron").join("output").join(&job.id);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{now}.md"));
    let header = format!(
        "# cron: {} ({})\n- fired: {}\n- schedule: {:?}\n\n",
        job.name, job.id, now, job.schedule
    );
    let full = format!("{header}{body}\n");
    let tmp = dir.join(format!(".gray-{}.tmp", uuid::Uuid::new_v4()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| -> anyhow::Result<()> {
        let mut file = options.open(&tmp)?;
        file.write_all(full.as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&tmp, &path)?;
        #[cfg(unix)]
        std::fs::File::open(&dir)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result?;
    Ok(path)
}

#[path = "cron_fire_tests.rs"]
#[cfg(test)]
mod tests;
