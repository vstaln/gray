//! Shell execution: ordinary blocking calls and session-owned background jobs.
//! Both modes share timeout, cancellation, redaction, bounded output and reaping.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use gray_core::agent::{AttachedImage, Tool, ToolContext, ToolOutput};
use gray_core::message::ToolDef;
use serde_json::{Value, json};
use tokio::task::JoinHandle;

use crate::shell::contract::{
    DEFAULT_TIMEOUT_SECS, INLINE_BUDGET_BYTES, MAX_TIMEOUT_SECS, MAX_YIELD_MS, MEM_HEAD_BYTES,
    MEM_TAIL_BYTES, MIN_YIELD_MS, PUMP_DRAIN_TIMEOUT, PumpSummary, VIEW_BUDGET_LINES, View,
};
use crate::shell::exit::exit_report;
use crate::shell::fence::fence;
use crate::shell::kill::term_then_kill;
use crate::shell::pump::Pump;
use crate::shell::spawn::spawn;
use crate::shell::view::{format_elapsed, header, middle_out, resume_hint};

/// Bytes served by one `Read more` recovery command. The inline budget is
/// 48 KiB, so 16 KiB pages need a third of the round trips a 4 KiB page did.
const READ_CHUNK: u64 = 16 * 1024;
use crate::{fail, get_opt_bool, get_opt_u64, get_str, resolve_path};

mod jobs;

/// One registry-owned job collection. Dropping the tool cancels its jobs.
#[derive(Default)]
pub struct BashTool {
    jobs: jobs::Jobs,
    /// Per-session working directory. Every command is a fresh `sh -c`, so a
    /// `cd` would otherwise be forgotten the moment it ran; the shell reports
    /// its final directory and the next command in the same session starts
    /// there. Keyed by session id (empty for an anonymous run) so a shared
    /// registry cannot cross sessions.
    ///
    /// The base the entry was recorded against is stored with it: a caller that
    /// hands over a different context cwd is being explicit, so the stale
    /// record is dropped rather than silently overriding it.
    cwd: Mutex<BTreeMap<String, (PathBuf, PathBuf)>>,
}

impl BashTool {
    /// Where this session's commands run. Falls back to the context cwd when
    /// the record belongs to a different base, or when the recorded directory
    /// has since been deleted, so neither a moved session nor a vanished
    /// directory can wedge it.
    fn session_cwd(&self, ctx: &ToolContext) -> PathBuf {
        let key = session_key(ctx);
        let cell = self.cwd.lock().unwrap_or_else(|e| e.into_inner());
        cell.get(&key)
            .filter(|(base, _)| *base == ctx.cwd)
            .map(|(_, current)| current)
            .filter(|p| p.is_dir())
            .cloned()
            .unwrap_or_else(|| ctx.cwd.clone())
    }

    /// Adopt the directory the shell reported, if it is still there. A command
    /// that was killed, or one whose trailing comment swallowed the report,
    /// leaves the previous cwd standing rather than resetting it.
    fn adopt_reported_cwd(&self, ctx: &ToolContext, report: &Path) {
        let reported = std::fs::read_to_string(report)
            .ok()
            .map(|text| PathBuf::from(text.trim()));
        if let Some(dir) = reported.filter(|p| p.is_dir()) {
            self.cwd
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(session_key(ctx), (ctx.cwd.clone(), dir));
        }
        let _ = std::fs::remove_file(report);
    }
}

#[async_trait]
impl Tool for BashTool {
    fn def(&self) -> ToolDef {
        ToolDef::new(
            "bash",
            "Run a shell command via sh -c. Default: wait for exit. For independent long work, \
             use background:true to return immediately, or yield_ms to return a job ID if still \
             running after that window. Multiple jobs can run concurrently; continue other work \
             instead of polling. Completion notices arrive between model rounds (or on the next \
             user turn when idle). Use action:list/status/output/cancel with job_id to manage jobs; \
             output/status accept wait_ms (bounded blocking wait, clamped 0-30000ms) so one call \
             can await a job instead of polling. Jobs belong to this session and stop when Gray exits. \
             timeout is an optional total runtime limit (no default: commands run until they exit; \
             capped at 3600s), NOT the yield window. \
             Non-zero exits are data, not tool errors. Full output is logged; inline output is bounded. \
             Imaging: to look at an image or video run `gray view <path>...`\
             (several at once, downscaled) as the whole command — bare paths only, so pipes,\
             globs, `$`, quotes and flags fall through to a normal run. Images\
             (png/jpg/jpeg/gif/webp/bmp/heic/heif) come back as themselves; a video\
             (mp4/mov/webm/mkv/avi) comes back as a tiled contact sheet of sampled\
             frames, with `--frames N` (before the paths) to set the tile count.\
             `cat` is for text/source files, not media. Either way the file comes back\
             as an image; bash output is otherwise text only, so never pixel-dump or\
             ASCII-art an image to inspect it.",
            json!({
                "type": "object",
                "properties": {
                    "command": {"type": "string", "description": "Shell command; required for action:run (default)"},
                    "action": {"type": "string", "enum": ["run", "list", "status", "output", "cancel"]},
                    "job_id": {"type": "string", "description": "Job ID returned by bash; required for status/output/cancel"},
                    "background": {"type": "boolean", "description": "Return immediately; run independently in this session"},
                    "timeout": {"type": "integer", "description": "Optional total runtime limit in seconds (omitted = no limit; clamped 1-3600)"},
                    "yield_ms": {"type": "integer", "description": "Wait at most this many milliseconds before returning a running job (clamped 100-10000); omitted means wait for exit"},
                    "wait_ms": {"type": "integer", "description": "Bounded blocking wait on action:output/status only: await the job's exit up to this many ms (clamped 0-30000) instead of polling; omitted means return immediately"}
                }
            }),
        )
    }

    fn drain_notifications(&self, ctx: &ToolContext) -> Vec<String> {
        self.jobs.notifications(ctx)
    }

    async fn execute(&self, ctx: &ToolContext, args: Value) -> ToolOutput {
        if !args.is_object() {
            return fail("bash arguments must be an object".into());
        }
        let action = match args.get("action") {
            None => "run",
            Some(Value::String(s)) => s.as_str(),
            _ => return fail("action must be a string".into()),
        };
        if args.get("wait_ms").is_some_and(|v| !v.is_null()) && action == "run" {
            return fail("wait_ms is only valid for action:output/status".into());
        }
        if action != "run" {
            // `wait` left the run surface (rejected loudly on the run path
            // below); the async wait path takes `wait_ms` on output/status.
            return self.jobs.action(ctx, action, &args).await;
        }
        // job_id on a plain run carries no extra intent (nothing is dropped),
        // and real models echo it back from the schema: ignore it. The
        // removed-API family below would silently lose intent, so it still
        // fails loudly — with removal as the first instruction, never a
        // suggestion that re-triggers the same failure.
        // `wait` left the run surface with the async wait path: it only rides
        // output/status as `wait_ms` now, so a run carrying it fails loudly
        // instead of silently dropping the intent.
        if args.get("wait").is_some_and(|v| !v.is_null()) {
            return fail(
                "`wait` is not a run argument; remove it. To await a job use action:output/status with wait_ms; for background work use background:true or yield_ms".into(),
            );
        }
        for key in ["task_id", "from_offset", "notify_on", "run_in_background"] {
            if args.get(key).is_some_and(|v| !v.is_null()) {
                return fail(format!(
                    "`{key}` is not a run argument; remove it. For background work use background:true or yield_ms; job_id only pairs with action:status/output/cancel"
                ));
            }
        }
        let command = match get_str(&args, "command") {
            Ok(c) if !c.trim().is_empty() => c,
            Ok(_) => return fail("command must be non-empty".into()),
            Err(e) => return e,
        };
        let secs = match get_opt_u64(&args, "timeout") {
            Ok(v) => v.map(|s| s.clamp(1, MAX_TIMEOUT_SECS)),
            Err(e) => return e,
        };
        let background = match get_opt_bool(&args, "background") {
            Ok(v) => v.unwrap_or(false),
            Err(e) => return e,
        };
        let window = match get_opt_u64(&args, "yield_ms") {
            Ok(v) => v.map(|ms| Duration::from_millis(ms.clamp(MIN_YIELD_MS, MAX_YIELD_MS))),
            Err(e) => return e,
        };
        if ctx.cancel.is_cancelled() {
            return fail("command not started: cancelled".into());
        }
        let cwd = self.session_cwd(ctx);
        if let Some(out) = image_command(&command, &cwd) {
            return out;
        }
        if background || window.is_some() {
            return self
                .jobs
                .start(
                    ctx,
                    command,
                    secs,
                    cwd,
                    if background {
                        Duration::ZERO
                    } else {
                        window.unwrap()
                    },
                )
                .await;
        }
        let log_path = log_path(ctx);
        let start = Instant::now();
        // Report the final directory to a scratch file so the next command in
        // this session starts where this one finished.
        let cwd_report =
            std::env::temp_dir().join(format!("gray-cwd-{}.txt", uuid::Uuid::new_v4()));
        let spawned = match spawn(
            &with_cwd_report(&command),
            &cwd,
            ctx.session_id.as_deref(),
            Some(&cwd_report),
        ) {
            Ok(s) => s,
            Err(e) => return fail(format!("failed to spawn `sh -c`: {e}")),
        };
        #[cfg(not(windows))]
        let guard = crate::shell::kill::GroupGuard::new(spawned.pgid);
        let out = run_command(
            command,
            log_path,
            secs,
            start,
            ctx.clone(),
            spawned,
            #[cfg(not(windows))]
            guard,
        )
        .await;
        // Whether it succeeded, failed or was killed: a `cd` that ran is a `cd`
        // that ran. A killed command never writes the report, so its cwd stands.
        self.adopt_reported_cwd(ctx, &cwd_report);
        out
    }
}

/// Ask the shell to report its final directory to `$GRAY_CWD_REPORT`.
///
/// Appended, never substituted: the command the model wrote is still what runs,
/// and the report goes to a file so stdout (and the durable log) are untouched.
///
/// The original exit status is captured and re-raised, because a bare
/// `; printf ...` would end the command with *the report's* status and turn
/// every failing command into a success -- which is how the benign-exit table
/// lost `grep`'s "no matches" the first time this was tried. A command ending
/// in a `#` comment swallows the whole suffix, reports nothing, and leaves the
/// cwd where it was.
fn with_cwd_report(command: &str) -> String {
    // Git Bash's plain `pwd` is an MSYS path (/c/Users/...), which Rust's
    // `is_dir` rejects on Windows, so the report would be read and thrown away
    // on exactly the platform that cannot be checked locally. `pwd -W` is
    // MSYS's Windows-path form (C:/Users/...), which Rust resolves. A shell
    // without `-W` writes nothing and the cwd simply stays put.
    #[cfg(windows)]
    let reported = "$(pwd -W)";
    #[cfg(not(windows))]
    let reported = "\"$PWD\"";
    format!(
        "{command}; __gray_rc=$?; printf '%s' {reported} > \"$GRAY_CWD_REPORT\" 2>/dev/null; exit $__gray_rc"
    )
}

/// Session identity for the cwd cell: the session id when there is one, empty
/// for an anonymous run (headless, print) where every call shares one cwd.
fn session_key(ctx: &ToolContext) -> String {
    ctx.session_id.clone().unwrap_or_default()
}

/// `cat <one image file>` returns the image as a vision block at full
/// resolution — decode, EXIF orientation, re-encode, no downscale — instead
/// of the binary garbage a shell would stream. Claims exactly
/// `cat` plus a single bare path: flags, pipes, redirects, globs, quotes,
/// and multi-file cats all fall through to a normal run. A missing or
/// non-image file falls through too, so the shell's own error message or
/// text output is what the model sees.
/// `cat <one image>` and `gray view <one or more images>` both hand images
/// back instead of streaming binary garbage through the shell: bash's vision
/// path, so no separate tool is needed for it. `cat` is the full-resolution
/// exception a pasted attachment needs; `view` is the everyday path and
/// downscales to the 2000px cap, the same cap pasted attachments and `read`
/// use.
///
/// The command is claimed before the shell ever runs, so anything the shell
/// would interpret (flags, pipes, redirects, globs, quotes, `$`) falls through
/// to a normal run — as does a missing file, so the shell's own error is what
/// the model sees. A leading `~` is the one thing expanded here: the shell
/// would have done it and nothing else would, and without it `cat ~/shot.png`
/// streams binary garbage while `cat /home/me/shot.png` shows the image. A
/// missing, undecodable or non-image path is skipped with a note and the valid
/// ones still ship; when nothing is usable the claim is dropped and the shell
/// gives the error, which reads better than a note attached to nothing.
// Cap multi-image claims: 8 paths (the per-turn inline-attach cap) and 20 MiB
// of aggregate base64 (4x the 5 MiB per-image cap in `crate::images`). Past
// the path cap the claim is cut to the prefix with a note; past the byte
// budget intake stops with a note. Either way the valid prefix still returns
// vision blocks instead of the whole command falling through to a text-only
// run.
const MAX_IMAGE_CLAIM_PATHS: usize = 8;
const MAX_IMAGE_CLAIM_BYTES: usize = 20 * 1024 * 1024;

fn image_command(command: &str, cwd: &Path) -> Option<ToolOutput> {
    use base64::Engine as _;
    let mut parts = command.split_whitespace();
    let (cmd, sub) = (parts.next()?, parts.next()?);
    let rest: Vec<&str> = parts.collect();
    let (paths, full_res) = match (cmd, rest.is_empty()) {
        ("cat", true) => (vec![sub], true),
        ("gray", false) if sub == "view" => {
            // `--frames N` is a flag, not a path: let the shell run the CLI so
            // clap parses it, rather than claiming it as a missing file.
            if rest.iter().any(|a| a.starts_with('-')) {
                return None;
            }
            (rest, false)
        }
        _ => return None,
    };
    if paths.iter().any(|p| shell_meta(p)) {
        return None;
    }
    // A path that is not there is skipped like a decode failure, not fatal:
    // the common case is one typo among several paths, and sinking the valid
    // ones with it is the whole complaint this claim shape exists to avoid.
    // A lone bad path still yields nothing usable, and the `images.is_empty()`
    // bail below hands it to the shell, which reports the path itself.
    let mut files: Vec<PathBuf> = Vec::with_capacity(paths.len());
    let mut failed: Vec<String> = Vec::new();
    for raw in paths {
        match resolve_bare_path(cwd, raw) {
            Some(full) => files.push(full),
            None => failed.push(format!("{raw}: no such file")),
        }
    }
    let total = files.len();
    let capped = total > MAX_IMAGE_CLAIM_PATHS;
    files.truncate(MAX_IMAGE_CLAIM_PATHS);
    let mut shown = Vec::with_capacity(files.len());
    let mut images = Vec::with_capacity(files.len());
    let mut bytes: usize = 0;
    for full in files {
        // `cat <one image>` is the full-resolution exception and stays that
        // way; `cat <one video>` is not a vision part, so it is refused here
        // and the claim drops, which leaves the shell to report the path.
        // The agent-facing way to see a video is `gray view`.
        let (mime, data, sheet) = if full_res {
            match std::fs::read(&full)
                .ok()
                .and_then(|raw| crate::images::encode_image_full(&raw).ok())
            {
                Some(pair) => (
                    pair.0,
                    base64::engine::general_purpose::STANDARD.encode(&pair.1),
                    false,
                ),
                None => {
                    failed.push(format!("{}: unreadable or undecodable", full.display()));
                    continue;
                }
            }
        } else {
            match crate::view::load(&full) {
                Ok(part) => (part.media_type, part.data, part.derived_from_video),
                Err(e) => {
                    failed.push(e.to_string());
                    continue;
                }
            }
        };
        if !images.is_empty() && bytes + data.len() > MAX_IMAGE_CLAIM_BYTES {
            failed.push(format!(
                "{}: skipped past the multi-image byte budget",
                full.display()
            ));
            break;
        }
        bytes += data.len();
        shown.push(if sheet {
            format!("{} (contact sheet)", full.display())
        } else {
            full.display().to_string()
        });
        images.push(AttachedImage {
            media_type: mime,
            data,
        });
    }
    if images.is_empty() {
        // Every path failed: drop the claim so the shell (or the CLI it runs)
        // reports the errors directly, which beats a note attached to no image.
        return None;
    }
    let mut content = format!("Image shown: {}", shown.join(", "));
    if capped {
        content.push_str(&format!(
            " (showing first {MAX_IMAGE_CLAIM_PATHS} of {total} paths)"
        ));
    }
    for f in &failed {
        content.push_str(&format!("; skipped: {f}"));
    }
    Some(ToolOutput {
        content,
        is_error: false,
        images,
    })
}

/// True when the shell would read more than a bare path into this argument:
/// flags, pipes, redirects, substitution, globs, quotes. Those fall through,
/// where the shell is there to interpret them.
fn shell_meta(arg: &str) -> bool {
    arg.starts_with('-')
        || arg.chars().any(|c| {
            matches!(
                c,
                '|' | '&' | ';' | '<' | '>' | '$' | '`' | '"' | '\'' | '*' | '?' | '(' | ')'
            )
        })
}

/// One bare path, expanded and resolved, when the shell would have found it:
/// a leading `~`/`~/` — the shell's job, now ours since it never runs — over a
/// plain relative or absolute path. None when the file is not there, so the
/// shell reports the missing path instead of a tool error.
fn resolve_bare_path(cwd: &Path, raw: &str) -> Option<PathBuf> {
    let expanded = expand_tilde(raw);
    let full = resolve_path(cwd, expanded.as_deref().unwrap_or(raw));
    full.is_file().then_some(full)
}

/// `~/shot.png` -> `$HOME/shot.png`, `~` -> `$HOME`. The one expansion the
/// shell would have done, since the fast path claims the command first.
fn expand_tilde(raw: &str) -> Option<String> {
    let home = std::env::var("HOME").ok().filter(|h| !h.is_empty())?;
    if raw == "~" {
        return Some(home);
    }
    let rest = raw.strip_prefix("~/")?;
    Some(format!("{home}/{rest}"))
}

fn log_path(ctx: &ToolContext) -> PathBuf {
    // Session IDs are host-supplied, not path components trusted from a tool call.
    let session = ctx.session_id.as_deref().unwrap_or("nosession");
    let safe: String = session
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    shell_dir()
        .join(safe)
        .join(format!("bash-{}.log", uuid::Uuid::new_v4().as_simple()))
}

/// Sleep until the explicit timeout, or never when none was requested:
/// `pending()` is the "no timeout" arm of the `select!` in `run_command`.
async fn wait_or_pend(secs: Option<u64>, start: Instant) {
    match secs {
        Some(s) => {
            tokio::time::sleep_until(tokio::time::Instant::from_std(
                start + Duration::from_secs(s),
            ))
            .await
        }
        None => std::future::pending::<()>().await,
    }
}

async fn run_command(
    command: String,
    log_path: PathBuf,
    secs: Option<u64>,
    start: Instant,
    ctx: ToolContext,
    spawned: crate::shell::contract::Spawned,
    #[cfg(not(windows))] mut guard: crate::shell::kill::GroupGuard,
) -> ToolOutput {
    #[cfg(not(windows))]
    let target = spawned.pgid;
    #[cfg(windows)]
    let target = &spawned.job;
    let mut child = spawned.child;
    let pump = Pump::start(child.stdout.take(), child.stderr.take(), log_path.clone());
    enum Cause {
        Exit,
        Timeout,
        Cancel,
    }
    let mut exited: Option<std::process::ExitStatus> = None;
    let cause = tokio::select! {
        s = child.wait() => match s {
            Ok(st) => { exited = Some(st); Cause::Exit }
            Err(e) => {
                let _ = term_then_kill(target, Duration::from_secs(2)).await;
                let _ = child.start_kill();
                abort_pump(pump).await;
                return fail(format!("failed to wait for command: {e}"));
            }
        },
        _ = wait_or_pend(secs, start) => {
            // Raced exit between the timer and now: report it, don't kill.
            match child.try_wait() {
                Ok(Some(st)) => { exited = Some(st); Cause::Exit }
                _ => Cause::Timeout,
            }
        }
        _ = ctx.cancel.cancelled() => Cause::Cancel,
    };
    // Unix escalates SIGTERM → SIGKILL; Windows terminates the owned job.
    // Failed termination is a harness error, never a successful timeout.
    let first_line: Option<String> = match cause {
        Cause::Exit => None,
        Cause::Timeout => {
            // Reap while Unix escalation polls: macOS can return EPERM
            // when only an unreaped zombie remains in the process group.
            // try_join also stops waiting if termination fails; never hide
            // that failure or block forever waiting for an unkillable child.
            match tokio::try_join!(term_then_kill(target, Duration::from_secs(2)), async {
                child
                    .wait()
                    .await
                    .map_err(|e| format!("failed to wait for command: {e}"))
            }) {
                Ok(((), st)) => exited = Some(st),
                Err(e) => {
                    let _ = child.start_kill();
                    abort_pump(pump).await;
                    return fail(e);
                }
            }
            Some(format!(
                "timed out after {}s (process group killed) · rerun without `timeout` to let it finish, or with a larger one (max {MAX_TIMEOUT_SECS}s)",
                secs.unwrap_or_default()
            ))
        }
        Cause::Cancel => {
            // Reap while Unix escalation polls: macOS can return EPERM
            // when only an unreaped zombie remains in the process group.
            // try_join also stops waiting if termination fails; never hide
            // that failure or block forever waiting for an unkillable child.
            match tokio::try_join!(term_then_kill(target, Duration::from_secs(2)), async {
                child
                    .wait()
                    .await
                    .map_err(|e| format!("failed to wait for command: {e}"))
            }) {
                Ok(((), st)) => exited = Some(st),
                Err(e) => {
                    let _ = child.start_kill();
                    abort_pump(pump).await;
                    return fail(e);
                }
            }
            Some(format!(
                "cancelled by user after {}",
                format_elapsed(start.elapsed())
            ))
        }
    };
    // A Windows shell may exit before background descendants release the
    // pipes. End this call's job before draining, not after a 30s pipe wait.
    // Native Windows shell background jobs therefore never outlive a call.
    #[cfg(windows)]
    drop(spawned.job);
    #[cfg(not(windows))]
    guard.disarm();
    let status = exited.expect("every cause resolves a status");
    let (summary, drain_truncated) = match drain_pump(pump, &log_path).await {
        Ok(v) => v,
        Err(e) => return fail(format!("output pump failed: {e}")),
    };
    let mut first = first_line;
    if drain_truncated {
        let note = format!(
            "output truncated: pump drain timed out after {}s",
            PUMP_DRAIN_TIMEOUT.as_secs()
        );
        first = Some(match first {
            Some(f) => format!("{f} ({note})"),
            None => note,
        });
    }
    if summary.log_write_failed {
        let note = "log write failed; view is memory-only";
        first = Some(match first {
            Some(f) => format!("{f} ({note})"),
            None => note.to_string(),
        });
    }
    finish_inline(&command, &log_path, status, &summary, start, first)
}
fn gray_home() -> PathBuf {
    gray_core::paths::gray_home().unwrap_or_else(|| std::env::temp_dir().join(".gray"))
}

fn shell_dir() -> PathBuf {
    gray_home().join("shell")
}

async fn abort_pump(pump: JoinHandle<PumpSummary>) {
    pump.abort();
    let _ = pump.await;
}

/// Bounded pump drain after the child exits: a grandchild inheriting the
/// pipes keeps the pump alive forever, so the result must never wait past
/// [`PUMP_DRAIN_TIMEOUT`]. On timeout the handle is aborted and
/// `truncated=true` so the caller reports truncated cleanup. Happy path
/// (drain completes in time) is byte-identical to a bare `pump.await`.
async fn drain_pump(
    mut pump: JoinHandle<PumpSummary>,
    log_path: &std::path::Path,
) -> Result<(PumpSummary, bool), tokio::task::JoinError> {
    match tokio::time::timeout(PUMP_DRAIN_TIMEOUT, &mut pump).await {
        Ok(res) => res.map(|s| (s, false)),
        Err(_) => {
            pump.abort();
            let _ = (&mut pump).await;
            log::warn!("shell: pump drain timed out; aborted, reporting truncated");
            Ok((truncated_summary_from_disk(log_path), true))
        }
    }
}

/// Fallback summary when the drain times out: rebuilt from the log file with
/// bounded reads (head + tail budgets only, never the whole file) so the
/// rendered view stays middle-out with an `omitted` marker and the header
/// size stays honest.
fn truncated_summary_from_disk(log_path: &std::path::Path) -> PumpSummary {
    use std::io::{Read, Seek, SeekFrom};
    let total_bytes = std::fs::metadata(log_path).map(|m| m.len()).unwrap_or(0);
    let mut head = Vec::new();
    let mut tail = Vec::new();
    if let Ok(mut f) = std::fs::File::open(log_path) {
        let _ = f
            .by_ref()
            .take(MEM_HEAD_BYTES as u64)
            .read_to_end(&mut head);
        let tail_len = (MEM_TAIL_BYTES as u64).min(total_bytes);
        if total_bytes > head.len() as u64
            && f.seek(SeekFrom::Start(total_bytes.saturating_sub(tail_len)))
                .is_ok()
        {
            let _ = f.by_ref().take(tail_len).read_to_end(&mut tail);
        }
    }
    let total_lines = head
        .iter()
        .chain(tail.iter())
        .filter(|&&b| b == b'\n')
        .count();
    // Best effort: only the sampled head+tail are in hand (never read the
    // whole file here); the live pump path above tracks this exactly.
    let has_cr = head.iter().chain(tail.iter()).any(|&b| b == b'\r');
    PumpSummary {
        total_bytes,
        total_lines,
        head,
        tail,
        has_cr,
        log_write_failed: false,
    }
}

/// Render a reaped command: header + fenced body. `first_line` prefixes the
/// header (timeout/cancel/truncation arms) or is absent (plain exit).
fn finish_inline(
    command: &str,
    log_path: &std::path::Path,
    status: std::process::ExitStatus,
    summary: &PumpSummary,
    start: Instant,
    first_line: Option<String>,
) -> ToolOutput {
    let elapsed = start.elapsed();
    let report = exit_report(status, command);
    let view = build_view(log_path, summary);
    let head = header(&report, Some(&view), elapsed, log_path);
    let mut out = match first_line {
        Some(first) => format!("{first}\n{head}"),
        None => head,
    };
    if !view.body.is_empty() {
        out.push('\n');
        out.push_str(&fence(&view.body));
    }
    if let Some((start, end)) = view.omitted_range {
        // Absolute, shell-quoted path: never expand the display-only ~/
        // shorthand. Git Bash consumes the command string, so the path must
        // use forward slashes; backslashes are shell escapes there.
        let path = log_path
            .to_string_lossy()
            .replace('\\', "/")
            .replace('\'', "'\"'\"'");
        let command = if small_log_on_disk(log_path, summary) {
            // middle_out offsets are sanitized bytes. Recover by line instead,
            // including the last shown line in case it was cut mid-line.
            let first = view.shown_lines.0.max(1);
            let last = first.saturating_add(199);
            format!("sed -n '{first},{last}p;{last}q' '{path}' | head -c {READ_CHUNK}")
        } else {
            // Large-log sampling tracks raw offsets, not sanitized ones.
            let count = end.saturating_sub(start).min(READ_CHUNK);
            format!("dd if='{path}' bs=1 skip={start} count={count} 2>/dev/null")
        };
        // The marker above already names the window (`resume_hint` prints the
        // byte range), so say only what it does not: where the next page
        // starts. Elision without that made models re-read whole logs to find
        // what was missing, and once hid a compiler error in the middle.
        let next = start + READ_CHUNK;
        if next < end {
            out.push_str(&format!(
                "\nThen skip={next} for the next {READ_CHUNK} bytes."
            ));
        }
        out.push_str(&format!("\nRead more: {command}"));
    }
    if let Some(hint) = missing_command_hint(&out) {
        out.push('\n');
        out.push_str(&hint);
    }
    ToolOutput::ok(out)
}

/// One-line nudge when a command was missing entirely. The benchmark retro
/// showed agents burning turns re-trying tools the image does not ship
/// (`rg`, `xxd`, `goyacc`, linters) or trying to install them offline; the
/// shell's own "not found" line never says what to do instead.
fn missing_command_hint(text: &str) -> Option<String> {
    let line = text.lines().find(|l| {
        let low = l.to_ascii_lowercase();
        (low.contains("command not found") || low.contains(": not found"))
            && (low.starts_with("sh:")
                || low.starts_with("bash:")
                || low.starts_with("dash:")
                || low.starts_with("zsh:")
                || low.contains(": command not found"))
    })?;
    let name = not_found_subject(line).unwrap_or_else(|| "that command".to_string());
    let equivalent = equivalent_for(&name).unwrap_or("`grep`, `sed`, `awk`, `python3`");
    Some(format!(
        "`{name}` is not installed here · use {equivalent} or confirm with `command -v {name}`"
    ))
}

/// Honest per-binary substitutes, from the DeepSWE campaign's not-found
/// telemetry (84 `rg`, 30 `xxd`, 9 `file`, 4 `time` misses). Only list a
/// replacement that exists on a bare POSIX image; everything else keeps the
/// generic list. New binaries are one line each.
const EQUIVALENTS: &[(&str, &str)] = &[
    ("rg", "`grep -r` / `grep -rn`"),
    ("xxd", "`od -c` (bytes) or `od -An -tx1` (hex)"),
    ("hexdump", "`od -c` / `od -An -tx1`"),
    ("jq", "`python3 -m json.tool` or `python3 -c`"),
    ("realpath", "`readlink -f`"),
    ("time", "`date +%s.%N` before/after the command"),
];

fn equivalent_for(name: &str) -> Option<&'static str> {
    EQUIVALENTS
        .iter()
        .find(|(bin, _)| *bin == name)
        .map(|(_, equivalent)| *equivalent)
}

/// Pulls the tool name out of the shell's not-found wording:
/// `bash: line 1: rg: command not found`, `sh: 1: rg: not found`,
/// `zsh: command not found: rg`.
fn not_found_subject(line: &str) -> Option<String> {
    let low = line.to_ascii_lowercase();
    let idx = low.find("not found")?;
    // `zsh: command not found: rg` names the tool after the phrase.
    if let Some(tok) = line[idx + "not found".len()..]
        .trim_start_matches([':', ' '])
        .split_whitespace()
        .next()
        .filter(|t| !t.is_empty())
    {
        return Some(tok.to_string());
    }
    // `bash: line 1: rg: command not found` names it before, behind the
    // literal word "command".
    line[..idx]
        .rsplit(|c: char| c == ':' || c.is_whitespace())
        .find(|t| !t.is_empty() && *t != "command")
        .map(str::to_string)
}

fn small_log_on_disk(log_path: &std::path::Path, summary: &PumpSummary) -> bool {
    let file_len = std::fs::metadata(log_path).map(|m| m.len()).unwrap_or(0);
    summary.total_bytes <= INLINE_BUDGET_BYTES as u64 && file_len <= INLINE_BUDGET_BYTES as u64
}

/// Bounded inline view (≤ ~12 KiB): the whole log read back from disk when
/// it fits the inline budget, else head ++ tail from the bounded memory
/// sample with the elided shape imposed explicitly.
/// `{{MARKER}}` is replaced only when bytes were actually omitted, so user
/// text can never collide with the slot.
fn build_view(log_path: &std::path::Path, summary: &PumpSummary) -> View {
    // Byte-bounded: the small path reads the log back from disk — never read
    // an unbounded log on a stale/small summary. Check the real length first
    // and fall back to the bounded in-memory head ++ tail.
    let use_disk = small_log_on_disk(log_path, summary);
    // On read error fall through to the memory sample below.
    if use_disk && let Ok(bytes) = std::fs::read(log_path) {
        let mut view = middle_out(&bytes, INLINE_BUDGET_BYTES, VIEW_BUDGET_LINES, 0);
        if view.omitted_range.is_some() {
            let hint = resume_hint(&view, log_path);
            view.body = view.body.replace("{{MARKER}}", &hint);
        }
        return view;
    }
    // Large logs: the ≤12 KiB memory sample (first MEM_HEAD_BYTES verbatim ++
    // ring of last MEM_TAIL_BYTES) always fits the inline budget, so a plain
    // middle_out pass would report nothing omitted. Sanitize head and tail
    // separately instead — a whole-path middle_out returns its sanitized
    // input verbatim with exact line counts — and impose the elided shape
    // with honest totals from the summary. The full log stays on disk.
    let head_part = middle_out(&summary.head, usize::MAX, usize::MAX, 0);
    let tail_part = middle_out(&summary.tail, usize::MAX, usize::MAX, 0);
    let shown = (head_part.total_lines, tail_part.total_lines);
    let omitted_lines = summary.total_lines.saturating_sub(shown.0 + shown.1);
    let omitted_bytes = usize::try_from(
        summary
            .total_bytes
            .saturating_sub((summary.head.len() + summary.tail.len()) as u64),
    )
    .unwrap_or(usize::MAX);
    if omitted_lines == 0 && omitted_bytes == 0 {
        // Degenerate (summary disagrees with the log length, e.g. the drain
        // fallback undercounted): render the sample whole, no marker.
        let mut cat = summary.head.clone();
        cat.extend_from_slice(&summary.tail);
        return middle_out(&cat, INLINE_BUDGET_BYTES, VIEW_BUDGET_LINES, 0);
    }
    // Omitted middle in raw-log space: the head is the first bytes verbatim
    // and the tail ring the last.
    let omitted_range = Some((
        summary.head.len() as u64,
        summary
            .total_bytes
            .saturating_sub(summary.tail.len() as u64),
    ));
    let mut view = View {
        body: String::new(),
        shown_lines: shown,
        omitted_lines,
        omitted_bytes,
        omitted_range,
        total_lines: summary.total_lines,
        total_bytes: summary.total_bytes,
        has_cr: summary.has_cr,
    };
    let hint = resume_hint(&view, log_path);
    // trim_end on the head is count-preserving (a trailing newline never
    // starts a new line), so shown counts match the displayed lines exactly.
    view.body = format!(
        "{}\n{}\n{}",
        head_part.body.trim_end_matches('\n'),
        hint,
        tail_part.body
    );
    view
}

#[path = "bash_tests.rs"]
#[cfg(test)]
mod tests;
