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

/// Bound for delivery excerpts (keeps the 4000-char cap the old origin note
/// used; the full transcript is already on disk, the mirror and live box
/// share this excerpt).
pub const DELIVERY_EXCERPT_CHARS: usize = 4000;

/// Hermes `_cron_mirror_message`: the clean (unwrapped, no header/footer, no
/// file path) output appended to the origin session transcript as a labelled
/// `USER` turn so a reply continues in context. `USER`, never assistant —
/// an assistant-role mirror lands assistant→assistant and breaks strict
/// alternation; consecutive user turns merge safely.
pub fn mirror_message(name: &str, body: &str) -> String {
    format!("[Cron delivery: {name}]\n{body}")
}

/// Bounded excerpt of a fire transcript for delivery (mirror + live box).
pub fn delivery_excerpt(text: &str) -> String {
    text.chars().take(DELIVERY_EXCERPT_CHARS).collect()
}

/// Plain-text live-chat rendering for hosts with no native cron renderer.
/// No job id, no dashes, no stop/manage footer, no file path (the path
/// travels in the JSON `path` field, for logs). A reminder is its stored
/// text, byte for byte: it was never a model answer, so it gets no framing.
pub fn format_delivery_plain(name: &str, body: &str, reminder: bool, failed: bool) -> String {
    if reminder && !failed {
        return body.to_string();
    }
    let title = if failed {
        format!("{name} failed")
    } else {
        name.to_string()
    };
    format!("{title}\n\n{}", body.trim())
}

/// Job name for a reminder added without `--name`: a slug of the EXACT text
/// ("clean my roo" -> "clean-my-roo"). Never corrected, never paraphrased —
/// the model that schedules the reminder must not rewrite what the user said.
pub fn reminder_name(text: &str) -> String {
    let mut slug = String::new();
    let mut dash = true; // swallow leading separators
    for c in text.chars() {
        if c.is_alphanumeric() {
            slug.extend(c.to_lowercase());
            dash = false;
        } else if !dash {
            slug.push('-');
            dash = true;
        }
    }
    let capped: String = slug.chars().take(40).collect();
    let capped = capped.trim_end_matches('-').to_string();
    if capped.is_empty() {
        "reminder".to_string()
    } else {
        capped
    }
}

/// Mask credentials before a transcript is written to disk or shown in chat.
/// Exact string values from `<home>/auth.json` first (the operator's real
/// keys, which no shape test can recognise), then `gray_core::redaction` for
/// the token shapes it already owns. Paths survive: the transcript is a tool
/// log, and the redactor only scrubs paths when a secret fired alongside
/// them.
pub fn redact_secrets(text: &str, home: &std::path::Path) -> String {
    let mut out = text.to_string();
    for secret in known_secrets(home) {
        out = out.replace(&secret, gray_core::redaction::REDACTED);
    }
    let redacted = gray_core::redaction::redact_for_disclosure(&out);
    if redacted.has_secret() {
        redacted.into_text()
    } else {
        out
    }
}

/// Every long, whitespace-free string in `<home>/auth.json`: credential
/// values and the ids that reference them, longest first so a value that
/// contains another is replaced whole.
fn known_secrets(home: &std::path::Path) -> Vec<String> {
    let Ok(raw) = std::fs::read_to_string(home.join("auth.json")) else {
        return Vec::new();
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    collect_strings(&v, &mut found);
    found.retain(|s| s.chars().count() >= 16 && !s.chars().any(char::is_whitespace));
    found.sort_by_key(|s| std::cmp::Reverse(s.len()));
    found.dedup();
    found
}

fn collect_strings(v: &serde_json::Value, out: &mut Vec<String>) {
    match v {
        serde_json::Value::String(s) => out.push(s.clone()),
        serde_json::Value::Array(a) => a.iter().for_each(|x| collect_strings(x, out)),
        serde_json::Value::Object(m) => m.values().for_each(|x| collect_strings(x, out)),
        _ => {}
    }
}

/// One step of the final-text fold: streamed text, or a tool call/result
/// boundary. Split from `AgentEvent` so the rule is testable without
/// constructing events.
enum Piece<'a> {
    Text(&'a str),
    Boundary,
}

/// Everything streamed after the last tool boundary, trimmed. Narration
/// before a tool call ("let me look around...") and all tool output are
/// dropped: a cron delivery is the last assistant message, never a transcript.
fn final_text_of<'a>(pieces: impl Iterator<Item = Piece<'a>>) -> String {
    let mut text = String::new();
    for p in pieces {
        match p {
            Piece::Text(t) => text.push_str(t),
            Piece::Boundary => text.clear(),
        }
    }
    text.trim().to_string()
}

/// The final assistant message of a fire (see [`final_text_of`]).
pub fn final_assistant_text(events: &[gray_core::event::AgentEvent]) -> String {
    use gray_core::event::AgentEvent;
    final_text_of(events.iter().filter_map(|ev| match ev {
        AgentEvent::TextDelta { delta } => Some(Piece::Text(delta)),
        AgentEvent::ToolCallStart { .. } | AgentEvent::ToolResult { .. } => Some(Piece::Boundary),
        _ => None,
    }))
}

/// Close any `<untrusted-output>` block a length cap cut open, so the
/// transcript always carries balanced tags.
fn close_untrusted(mut s: String) -> String {
    for (open, close) in [
        ("<untrusted-output>", "</untrusted-output>"),
        ("<untrusted_output>", "</untrusted_output>"),
    ] {
        let opens = s.matches(open).count();
        let closes = s.matches(close).count();
        for _ in closes..opens {
            s.push('\n');
            s.push_str(close);
        }
    }
    s
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
                let s = close_untrusted(output.chars().take(2000).collect::<String>());
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
/// `error` without running the agent). Async process I/O keeps the ticker
/// responsive; dropping a timed-out child kills it.
pub async fn run_pre_script(script: &std::path::Path, workdir: &std::path::Path) -> ScriptOutcome {
    run_pre_script_with_timeout(
        script,
        workdir,
        std::time::Duration::from_secs(SCRIPT_TIMEOUT_SECS),
    )
    .await
}

/// Turn a finished pre-script process into its outcome, keeping the tail
/// of stderr bounded.
fn completed(out: std::process::Output) -> ScriptOutcome {
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

/// Name the script in a spawn failure: without it the operator is left
/// guessing which job's pre-script is broken.
fn spawn_failure(script: &std::path::Path, e: &std::io::Error) -> ScriptOutcome {
    ScriptOutcome {
        ok: false,
        stdout: String::new(),
        stderr_tail: format!("pre-script spawn failed for {}: {e:#}", script.display()),
    }
}

async fn run_pre_script_with_timeout(
    script: &std::path::Path,
    workdir: &std::path::Path,
    timeout: std::time::Duration,
) -> ScriptOutcome {
    // Shebang scripts are not executables on Windows: go through the same
    // POSIX shell the bash tool uses. A missing shell is a spawn failure,
    // not a panic path, so it maps into the same ScriptOutcome.
    #[cfg(windows)]
    let mut command = match gray_tools::shell::shell_path() {
        Ok(shell) => {
            // The script path is data, not shell code: `sh -c <path>` eats
            // backslashes as escapes (CI: `C:UsersRUNNER~1...sh: command
            // not found`). A quoted positional keeps the path verbatim, and
            // Git Bash accepts forward slashes.
            let script_fs = script.to_string_lossy().replace('\\', "/");
            let mut cmd = tokio::process::Command::new(shell);
            cmd.arg("-c").arg("exec \"$1\"").arg("sh").arg(script_fs);
            cmd
        }
        Err(e) => {
            return ScriptOutcome {
                ok: false,
                stdout: String::new(),
                stderr_tail: format!("pre-script spawn failed: {e}"),
            };
        }
    };
    #[cfg(not(windows))]
    let mut command = tokio::process::Command::new(script);
    command.current_dir(workdir).kill_on_drop(true);
    let res = tokio::time::timeout(timeout, command.output()).await;
    match res {
        Err(_) => ScriptOutcome {
            ok: false,
            stdout: String::new(),
            stderr_tail: "pre-script timed out".to_string(),
        },
        Ok(Err(e)) => {
            // ETXTBSY means the script was open for writing somewhere at
            // the instant we exec'd it — a pre-script written moments
            // earlier by another process is enough. It is transient by
            // definition, so one short retry turns a spurious job failure
            // into a non-event; anything else is reported as-is.
            if e.raw_os_error() == Some(26) {
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                match tokio::time::timeout(timeout, command.output()).await {
                    Ok(Ok(out)) => completed(out),
                    Ok(Err(e)) => spawn_failure(script, &e),
                    Err(_) => ScriptOutcome {
                        ok: false,
                        stdout: String::new(),
                        stderr_tail: "pre-script timed out".to_string(),
                    },
                }
            } else {
                spawn_failure(script, &e)
            }
        }
        Ok(Ok(out)) => completed(out),
    }
}

/// Write the transcript to `$HOME/cron/output/<job-id>/<unix-ts>.md`
/// (0600, atomic tmp+rename). Header carries name/id/fire-time/schedule.
/// Raw-bytes twin of the store's JSON writer (kept here so `gray-cron`
/// keeps no second copy): unique private tmp, mode at creation, dir sync.
pub fn write_local_output(
    home: &std::path::Path,
    job: &crate::cron::CronJob,
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

#[path = "cron_fire_render_tests.rs"]
#[cfg(test)]
mod render_tests;
