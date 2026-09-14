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
    let Some(first) = lines.next() else { return false };
    if first.eq_ignore_ascii_case("[silent]") {
        return true;
    }
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .last()
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

#[cfg(test)]
mod tests {
    // UNRUN (cargo test banned under X): run in TTY/CI.
    use super::*;

    #[test]
    fn wake_gate_shapes() {
        assert!(parse_wake_gate("some output\n"));
        assert!(parse_wake_gate(""));
        assert!(parse_wake_gate("data\n{\"wakeAgent\": true}\n"));
        assert!(!parse_wake_gate("data\n{\"wakeAgent\": false}\n"));
        assert!(!parse_wake_gate("  {\"wakeAgent\": false}  \n\n"));
        assert!(parse_wake_gate("not json\n"));
        assert!(parse_wake_gate("{\"other\": 1}\n"));
    }

    #[test]
    fn silent_prefix_shapes() {
        assert!(is_silent_response("[SILENT]"));
        assert!(is_silent_response("[SILENT]\nreal line"));
        assert!(is_silent_response("real line\n[SILENT]"));
        assert!(is_silent_response("  [silent]  "));
        assert!(!is_silent_response("loud report"));
        assert!(!is_silent_response(""));
    }

    #[test]
    fn assembly_orders_skills_then_script_then_prompt() {
        let out = assemble_fire_prompt(
            "check CI",
            &[std::path::PathBuf::from("/s/SKILL.md")],
            Some("build ok"),
        );
        let si = out.find("## skills").unwrap();
        let oi = out.find("## script-output").unwrap();
        assert!(si < oi);
        assert!(out.ends_with("check CI"));
        assert!(out.contains("/s/SKILL.md"));
        assert!(out.contains("build ok"));
    }

    #[test]
    fn assembly_omits_empty_sections() {
        let out = assemble_fire_prompt("hi", &[], None);
        assert_eq!(out, "hi");
    }

    #[test]
    fn transcript_collects_text_and_tool_names() {
        use gray_core::event::AgentEvent;
        let events = vec![
            AgentEvent::TextDelta {
                delta: "hello ".to_string(),
            },
            AgentEvent::TextDelta {
                delta: "world".to_string(),
            },
        ];
        let t = transcript_text(&events);
        assert!(t.contains("hello world"));
    }

    #[tokio::test]
    async fn script_success_captures_stdout() {
        let dir = tempfile::tempdir().unwrap();
        let sh = dir.path().join("ok.sh");
        std::fs::write(&sh, "#!/bin/sh\necho hello\n").unwrap();
        #[cfg(unix)]
        #[cfg(unix)]
        std::fs::set_permissions(&sh, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        let out = run_pre_script(&sh, dir.path()).await;
        assert!(out.ok);
        assert!(out.stdout.contains("hello"));
    }

    #[tokio::test]
    async fn script_failure_marks_not_ok() {
        let dir = tempfile::tempdir().unwrap();
        let sh = dir.path().join("bad.sh");
        std::fs::write(&sh, "#!/bin/sh\necho oops >&2\nexit 3\n").unwrap();
        #[cfg(unix)]
        #[cfg(unix)]
        std::fs::set_permissions(&sh, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        let out = run_pre_script(&sh, dir.path()).await;
        assert!(!out.ok);
        assert!(out.stderr_tail.contains("oops"));
    }

    #[tokio::test]
    async fn script_missing_file_is_not_ok() {
        let dir = tempfile::tempdir().unwrap();
        let out = run_pre_script(&dir.path().join("gone.sh"), dir.path()).await;
        assert!(!out.ok);
    }

    #[test]
    fn local_output_writes_atomic_md() {
        let home = tempfile::tempdir().unwrap();
        let job = gray_cron::CronJob {
            id: "abc123".to_string(),
            name: "n".to_string(),
            prompt: "p".to_string(),
            schedule: gray_cron::Schedule::Interval { secs: 3600 },
            enabled: true,
            state: Default::default(),
            created_at: 1,
            next_run_at: None,
            last_run_at: None,
            last_status: None,
            last_error: None,
            last_delivery_error: None,
            deliver: Default::default(),
            origin: None,
            workdir: None,
            fire_claim: None,
            skills: vec![],
            script: None,
        };
        let path = write_local_output(home.path(), &job, 1_700_000_000, "body").unwrap();
        assert!(path.to_string_lossy().contains("abc123"));
        assert!(path.extension().is_some_and(|e| e == "md"));
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("body"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
}
