//! Shell execution, mini-swe-agent shaped: the model sends `command` and an
//! optional `timeout`, nothing else. A call blocks until the command exits or
//! the timeout passes; past the timeout the command is NOT killed, it moves
//! to a session-owned background job and the call returns its log, pgid and
//! stop command. Time passes only inside commands (`sleep`) or between turns
//! (a finished job wakes the session), so there is no wait primitive to misuse.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use gray_core::agent::{Tool, ToolContext, ToolOutput};
use gray_core::message::ToolDef;
use serde_json::{Value, json};
use tokio::task::JoinHandle;

use crate::shell::contract::{
    DEFAULT_TIMEOUT_SECS, INLINE_BUDGET_BYTES, MAX_TIMEOUT_SECS, MEM_HEAD_BYTES, MEM_TAIL_BYTES,
    PUMP_DRAIN_TIMEOUT, PumpSummary, SLEEP_SLACK_SECS, VIEW_BUDGET_LINES, View,
};
use crate::shell::exit::exit_report;
use crate::shell::fence::fence;
use crate::shell::kill::term_then_kill;
use crate::shell::pump::{Pump, now_grindmill};
use crate::shell::spawn::spawn;
use crate::shell::view::{
    format_elapsed, header, middle_out, resume_hint, squeeze as squeeze_view,
};
use crate::view::Attached;
use gray_core::spill::{self, MeterEvent};
use gray_core::squeeze::squeeze;

/// Bytes served by one `Read more` recovery command. A page has to fit the
/// head sample verbatim, or paging buys nothing: the page is truncated again
/// on its way back in and its middle is elided with a fresh hint. A round trip
/// is the expensive part here, not the bytes.
const READ_CHUNK: u64 = 4 * 1024;
use crate::{fail, finish, get_opt_u64, get_str, resolve_path};

mod jobs;
mod read_dedup;

impl BashTool {
    /// Completion-wake for the agent loop's turn end: block until any
    /// unfinished background job settles (bounded), so a headless run that
    /// backgrounded work holds the turn instead of exiting and killing the
    /// job. Returns false when the timeout elapsed / cancel fired / no jobs.
    pub async fn wait_any_job(&self, ctx: &ToolContext, timeout: Duration) -> bool {
        self.jobs.wait_any(ctx, timeout).await
    }

    /// The same wait as an owned future, for the executor's turn-end hook
    /// (which cannot borrow the tool past the call).
    pub fn wait_any_job_fut(
        &self,
        ctx: &ToolContext,
        timeout: Duration,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send>> {
        let jobs = self.jobs.clone();
        let ctx = ctx.clone();
        Box::pin(async move { jobs.wait_any(&ctx, timeout).await })
    }

    /// Whether any unfinished background job belongs to this session.
    pub fn has_unfinished_jobs(&self, ctx: &ToolContext) -> bool {
        self.jobs.has_unfinished(ctx)
    }

    /// This session's jobs that are running in the background, oldest first.
    pub fn running_jobs(&self, ctx: &ToolContext) -> Vec<gray_core::agent::BackgroundJob> {
        self.jobs.running(ctx)
    }

    /// One line per still-running job of this session (id, elapsed, log,
    /// stop command), for the loop to show the model each turn.
    pub fn running_jobs_note(&self, ctx: &ToolContext) -> Option<String> {
        self.jobs.footer(ctx, &self.jobs.live_ids(ctx))
    }

    /// Request cancellation of one of this session's running jobs.
    pub fn cancel_job(&self, ctx: &ToolContext, id: &str) -> bool {
        self.jobs.cancel_running(ctx, id)
    }
}

/// One registry-owned job collection. Dropping the tool cancels its jobs.
#[derive(Default)]
pub struct BashTool {
    /// Shared so a completion-wake future can be `'static`: the executor
    /// holds `Arc<dyn Tool>` and the turn-end wait outlives the borrow.
    jobs: std::sync::Arc<jobs::Jobs>,
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
    /// Session read ledger, shared with `read`/`write`/`edit` when the host
    /// builds them together (see [`BashTool::with_ledger`]). `None` on the
    /// `Default` a lone tool gets: no ledger, no dedup.
    ledger: Option<Arc<crate::ledger::FileLedger>>,
}

impl BashTool {
    /// Shares the session's read ledger, so a file read through `read` and
    /// one read through `cat` dedup against each other, and `/new` +
    /// compaction's ledger lifecycle covers this tool's entries too.
    pub fn with_ledger(mut self, ledger: Arc<crate::ledger::FileLedger>) -> Self {
        self.ledger = Some(ledger);
        self
    }

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
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn def(&self) -> ToolDef {
        tool_def(jobs_enabled())
    }

    fn drain_notifications(&self, ctx: &ToolContext) -> Vec<String> {
        self.jobs.notifications(ctx)
    }

    async fn execute(&self, ctx: &ToolContext, args: Value) -> ToolOutput {
        if !args.is_object() {
            return fail("bash arguments must be an object".into());
        }
        if let Some(out) = removed_arg(&args) {
            return out;
        }
        let command = match get_str(&args, "command") {
            Ok(c) if !c.trim().is_empty() => c,
            Ok(_) => return fail("command must be non-empty".into()),
            Err(e) => return e,
        };
        if let Some(out) = self.noop_steer(ctx, &command) {
            return out;
        }
        // The advertised stop for one of this session's jobs is carried
        // out here, on the right machine; anything else runs as written.
        if let Some(out) = self.jobs.intercept_stop(ctx, &command).await {
            return out;
        }
        let explicit = match get_opt_u64(&args, "timeout") {
            Ok(v) => v.map(|s| s.clamp(1, MAX_TIMEOUT_SECS)),
            Err(e) => return e,
        };
        if ctx.cancel.is_cancelled() {
            return fail("command not started: cancelled".into());
        }
        let cwd = self.session_cwd(ctx);
        if let Some(out) = image_command(&command, &cwd) {
            return out;
        }
        if let Some(out) = search_command(&command, &cwd, &ctx.cancel).await {
            return out;
        }
        // With the jobs lane, every call is bounded: an omitted `timeout` is
        // the default (stretched past a leading `sleep N`), and reaching it
        // hands the command to a job instead of killing it. Bare mode
        // (`GRAY_NO_JOBS`) is plain mini-swe-agent: explicit timeouts kill,
        // none means unbounded.
        let detach = jobs_enabled();
        let secs = if detach {
            Some(explicit.unwrap_or_else(|| block_bound(&command)))
        } else {
            explicit
        };
        // Jobs already running before this call get a footer line on its
        // result; the job this call may create is described by its own notice.
        let running_before = self.jobs.live_ids(ctx);
        // Read dedup: a `cat`/`sed`/`head`/`tail` of a file already shown
        // whole and unchanged since answers with a stub instead of the bytes
        // (once — see `read_dedup`).
        let dedup = self
            .ledger
            .as_deref()
            .and_then(|ledger| read_dedup::plain_read(&command, &cwd).map(|r| (ledger, r)));
        let dedup_on = read_dedup::enabled();
        if let Some((ledger, read)) = &dedup
            && let Some(hit) = read_dedup::check(ledger, read, dedup_on)
        {
            return hit;
        }
        let log_path = log_path(ctx);
        let logged = log_path.clone();
        let start = Instant::now();
        // A media `cat` the claim refused (compound, flags, glob) runs as a
        // plain shell command with nothing attached:
        // decide the note here, before `command` moves, append it after the
        // run. The predicate itself declines when nothing was ever at stake.
        let unattached = unattached_media_note(&command);
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
        let mut out = run_command(
            command,
            log_path,
            secs,
            start,
            ctx.clone(),
            spawned,
            #[cfg(not(windows))]
            guard,
            detach.then_some(&*self.jobs),
        )
        .await;
        // Whether it succeeded, failed or was killed: a `cd` that ran is a `cd`
        // that ran. A killed command never writes the report, so its cwd stands.
        self.adopt_reported_cwd(ctx, &cwd_report);
        // Arm the next dedup from what this run actually showed. A settled,
        // complete result is the only thing worth citing: a timeout, a
        // cancellation, a background hand-off or a truncated pump drain all
        // prefix the body with a note instead of a header, and their log is
        // part of an unfinished read.
        if let Some((ledger, read)) = &dedup
            && !out.is_error
            && out.content.starts_with("exit ")
            && let Ok(meta) = std::fs::metadata(&logged)
        {
            read_dedup::record(ledger, read, meta.len());
        }
        if let Some(note) = unattached {
            out.content.push('\n');
            out.content.push_str(&note);
        }
        if let Some(footer) = self.jobs.footer(ctx, &running_before) {
            out.content.push('\n');
            out.content.push_str(&footer);
        }
        out
    }
}

/// The bash schema: `command` + `timeout` either way; the description says
/// what reaching the timeout means (a background job, or a kill in bare mode).
fn tool_def(jobs: bool) -> ToolDef {
    // Only what the model can't infer: shell basics are free.
    const VIEW: &str =
        "`cat` bare image, video, PDF or audio paths (as the whole command) to view them.";
    if !jobs {
        return ToolDef::new(
            "bash",
            format!("Run a shell command; no default timeout. {VIEW}"),
            json!({
                "type": "object",
                "properties": {
                    "command": {"type": "string"},
                    "timeout": {"type": "integer", "description": "Seconds; omitted = no limit (max 3600)"}
                }
            }),
        );
    }
    // A `--reminder` cron job delivers into the session that added it
    // (`GRAY_SESSION_ID`). Cron does not fire on Windows.
    const CRON: &str = if cfg!(windows) {
        ""
    } else {
        " To come back at a time instead, `gray cron add \"in 30m\" \"<note to self>\" --reminder` \
         wakes this session (or posts back to this chat); don't run `cron serve`/`tick` to wait for it."
    };
    let default = default_timeout();
    ToolDef::new(
        "bash",
        format!(
            "Run a shell command. It blocks until the command exits or `timeout` passes; then it is \
             NOT killed but keeps running as a background job, and you get its log and a stop command. \
             To wait for it, end your turn: a finished job wakes you, no polling needed. Peek with \
             `tail <log>`. If a command sleeps, pass a `timeout` longer than the sleep. For a hard \
             limit, write `timeout N cmd` yourself. {VIEW}{CRON}"
        ),
        json!({
            "type": "object",
            "properties": {
                "command": {"type": "string"},
                "timeout": {"type": "integer", "description": format!("Seconds to block before it moves to the background (default {default}, max {MAX_TIMEOUT_SECS}); never kills")}
            }
        }),
    )
}

/// Arguments the bash tool used to take (job actions, explicit
/// backgrounding, yield/wait windows) and aliases other harnesses use for
/// them. Each fails loudly with what to do instead; silently ignoring one
/// would drop the caller's intent. Values that carry no intent
/// (`background:false`, `action:"run"`, null) are let through, and so is a
/// stray `job_id` on a plain run: it never meant anything without an action
/// (which does fail), and rejecting it looped real sessions (2026-09-17).
///
/// Wait/yield windows are different: schema-filling models (the gpt-6
/// family fills every property it has ever seen on a tool) echo
/// `wait_ms`/`yield_ms` onto a plain `command` run, where they carry no
/// intent — the call already blocks until exit or timeout. Rejecting the
/// echo made the model resend the identical call until the turn died, so
/// on a run they drop. Bare (no `command`) they still fail loudly.
fn removed_arg(args: &Value) -> Option<ToolOutput> {
    const REMOVED: &[&str] = &[
        "action",
        "background",
        "wait",
        "task_id",
        "from_offset",
        "notify_on",
        // Other harnesses' spellings. The registry only renames an alias onto
        // a name the schema still has, so these must be listed themselves or
        // they would pass through and be silently ignored.
        "run_in_background",
        "is_background",
        "detach",
        "bg",
    ];
    const WAIT_ECHO: &[&str] = &["wait_ms", "yield_ms", "yield_time_ms"];
    let is_run = args
        .get("command")
        .and_then(Value::as_str)
        .is_some_and(|c| !c.trim().is_empty());
    let key = REMOVED
        .iter()
        .chain(WAIT_ECHO.iter().filter(|_| !is_run))
        .find(|key| match args.get(**key) {
            None | Some(Value::Null) | Some(Value::Bool(false)) => false,
            Some(Value::String(s)) if **key == "action" && s == "run" => false,
            Some(_) => true,
        })?;
    let how = if jobs_enabled() {
        "bash takes only `command` and `timeout`. A command still running at its timeout keeps \
         running as a background job: end your turn to be woken when it finishes, `tail` its log \
         to peek, and stop it with the `kill -- -<pgid>` its notice gave you"
    } else {
        "bash takes only `command` and `timeout`; to run something in the background, write \
         `nohup cmd > log 2>&1 &` yourself"
    };
    Some(fail(format!(
        "`{key}` is not a bash argument; remove it. {how}."
    )))
}

/// How long a call without `timeout` blocks before handing off: the default,
/// or long enough for a leading `sleep N` (plus slack) so an in-band wait
/// like `sleep 300 && tail log` runs as written instead of becoming a job.
fn block_bound(command: &str) -> u64 {
    let default = default_timeout();
    match leading_sleep(command) {
        Some(n) => default
            .max(n.saturating_add(SLEEP_SLACK_SECS))
            .min(MAX_TIMEOUT_SECS),
        None => default,
    }
}

/// `command` past any leading `cd DIR &&` / `cd DIR;` / `cd DIR` + newline
/// steps (DIR one word, optionally quoted without spaces-in-quotes tricks).
fn skip_leading_cds(mut command: &str) -> &str {
    loop {
        let Some(after) = command.strip_prefix("cd") else {
            return command;
        };
        if !after.starts_with([' ', '\t']) {
            return command;
        }
        let after = after.trim_start_matches([' ', '\t']);
        let end = after
            .find([' ', '\t', ';', '\n', '&', '|'])
            .unwrap_or(after.len());
        if end == 0 || after[..end].contains(['$', '`', '(']) {
            return command;
        }
        let tail = after[end..].trim_start_matches([' ', '\t']);
        let next = if let Some(t) = tail.strip_prefix("&&") {
            t
        } else if let Some(t) = tail.strip_prefix(';').or_else(|| tail.strip_prefix('\n')) {
            t
        } else {
            return command;
        };
        command = next.trim_start();
    }
}

/// The default block window, overridable per process via
/// `GRAY_BASH_TIMEOUT_SECS` (tests use short ones).
fn default_timeout() -> u64 {
    std::env::var("GRAY_BASH_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|&s| s > 0)
        .map(|s| s.min(MAX_TIMEOUT_SECS))
        .unwrap_or(DEFAULT_TIMEOUT_SECS)
}

/// Seconds of a command whose first real step is a foreground `sleep N`
/// (`N`, `Ns`, `Nm`, `Nh`, fractions rounded up) followed by nothing, `;`,
/// `&&`, `||` or a newline. Leading `cd DIR &&`/`cd DIR;` steps are skipped
/// (`cd repo && sleep 300 && tail log` is the same wait). `sleep 5 &`
/// backgrounds the sleep, so it does not count.
fn leading_sleep(command: &str) -> Option<u64> {
    let rest = skip_leading_cds(command.trim_start()).strip_prefix("sleep")?;
    if !rest.starts_with([' ', '\t']) {
        return None;
    }
    let rest = rest.trim_start_matches([' ', '\t']);
    let end = rest
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || matches!(c, 's' | 'm' | 'h')))
        .unwrap_or(rest.len());
    let (num, after) = rest.split_at(end);
    let after = after.trim_start_matches([' ', '\t']);
    let ends_sleep = after.is_empty()
        || after.starts_with(';')
        || after.starts_with('\n')
        || after.starts_with("&&")
        || after.starts_with("||");
    if !ends_sleep {
        return None;
    }
    let (digits, unit) = match num.as_bytes().last()? {
        b's' => (&num[..num.len() - 1], 1.0),
        b'm' => (&num[..num.len() - 1], 60.0),
        b'h' => (&num[..num.len() - 1], 3600.0),
        _ => (num, 1.0),
    };
    let secs: f64 = digits.parse().ok()?;
    (secs.is_finite() && secs >= 0.0).then(|| (secs * unit).ceil().min(u64::MAX as f64) as u64)
}

/// Ask the shell to report its final directory to `$GRAY_CWD_REPORT`.
///
/// Appended, never substituted: the command the model wrote is still what runs,
/// and the report goes to a file so stdout (and the durable log) are untouched.
///
/// The original exit status is captured and re-raised, because a bare
/// `; printf ...` would end the command with *the report's* status and turn
/// every failing command into a success -- which is how the benign-exit table
/// lost `grep`'s "no matches" the first time this was tried.
///
/// Each piece of the suffix sits on its OWN line. Joining with `; ` broke any
/// command whose last line is a heredoc terminator: `EOF` became
/// `EOF; __gray_rc=$?`, so the terminator never matched and the suffix landed
/// inside the heredoc body -- a shell file written with a garbage trailer, or a
/// SyntaxError for an interpreter heredoc. 33 of 47 DeepSWE runs in the
/// 2026-09-29 retro reported exactly this; it cost each one a wasted turn at
/// best.
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
        "{command}\n__gray_rc=$?\nprintf '%s' {reported} > \"$GRAY_CWD_REPORT\" 2>/dev/null\nexit $__gray_rc"
    )
}

/// `GRAY_NO_JOBS=1` (bare mode) drops the background lane entirely: an
/// explicit `timeout` kills the process group, as in mini-swe-agent, and no
/// `timeout` means no limit. The schema is the same two fields either way.
fn jobs_enabled() -> bool {
    std::env::var_os("GRAY_NO_JOBS").is_none()
}

/// Session identity for the cwd cell: the session id when there is one, empty
/// for an anonymous run (headless, print) where every call shares one cwd.
fn session_key(ctx: &ToolContext) -> String {
    ctx.session_id.clone().unwrap_or_default()
}

/// `cat PATH...` of media returns images, video, PDFs and audio as model
/// parts instead of the binary garbage a shell would stream: every model
/// already reaches for `cat`. It shares the 2000px cap with `read` and pasted
/// attachments.
///
/// The command is claimed before the shell ever runs, so anything the shell
/// would interpret (flags, pipes, redirects, globs, quotes, `$`) falls through
/// to a normal run — as does a missing file, so the shell's own error is what
/// the model sees. A leading `~` is the one thing expanded here: the shell
/// would have done it and nothing else would, and without it `cat
/// ~/shot.png` fails while the absolute path works. A missing, undecodable or
/// non-media path is skipped with a note and the valid ones still ship; when
/// nothing is usable the claim is dropped and the shell gives the error, which
/// reads better than a note attached to nothing.
// Cap multi-image claims: 8 paths (the per-turn inline-attach cap) and 20 MiB
// of aggregate base64 (4x the 5 MiB per-image cap in `crate::images`). Past
// the path cap the claim is cut to the prefix with a note; past the byte
// budget intake stops with a note. Either way the valid prefix still returns
// vision blocks instead of the whole command falling through to a text-only
// run.
const MAX_IMAGE_CLAIM_PATHS: usize = 8;
const MAX_IMAGE_CLAIM_BYTES: usize = 20 * 1024 * 1024;

/// `gray find` / `gray grep` claimed before the shell runs, for the same
/// reason `cat` of media is: the model should not have to know that `gray`
/// happens to be on its PATH. Text in, text out, so this is one thin parse
/// over [`crate::search_cmd`] — the index, or the tool's own lane, decided
/// there.
async fn search_command(
    command: &str,
    cwd: &Path,
    cancel: &tokio_util::sync::CancellationToken,
) -> Option<ToolOutput> {
    let words = search_words(command)?;
    let mut parts = words.into_iter();
    if parts.next()? != "gray" {
        return None;
    }
    let sub = parts.next()?;
    if sub != "find" && sub != "grep" {
        return None;
    }
    let rest: Vec<String> = parts.collect();
    let mut positional: Vec<String> = Vec::new();
    let (mut limit, mut glob, mut context) = (None, None, None);
    let (mut ignore_case, mut literal) = (false, false);
    let mut i = 0;
    while i < rest.len() {
        let arg = rest[i].as_str();
        // `--name=value` carries its own value; bare `--name` consumes the
        // next word. A flag whose value is missing, or an unknown flag, falls
        // through to the shell: it owns the usage error, and it names the
        // real argv.
        let mut take = |name: &str| -> Option<String> {
            if let Some(v) = arg.strip_prefix(name).and_then(|r| r.strip_prefix('=')) {
                return Some(v.to_string());
            }
            i += 1;
            rest.get(i).cloned()
        };
        match arg {
            "--ignore-case" | "-i" => ignore_case = true,
            "--literal" | "-F" => literal = true,
            a if a == "--limit" || a.starts_with("--limit=") => {
                limit = Some(take("--limit")?.parse().ok()?)
            }
            a if a == "--glob" || a.starts_with("--glob=") => glob = Some(take("--glob")?),
            a if a == "--context" || a.starts_with("--context=") => {
                context = Some(take("--context")?.parse().ok()?)
            }
            a if a.starts_with('-') => return None,
            _ => positional.push(arg.to_string()),
        }
        i += 1;
    }
    let (Some(pattern), path) = (positional.first().cloned(), positional.get(1).cloned()) else {
        return None;
    };
    if positional.len() > 2 {
        return None;
    }
    let args = crate::search_cmd::SearchArgs {
        pattern,
        path: path.map(|p| cwd.join(p)),
        limit,
        glob,
        ignore_case,
        literal,
        context,
    };
    let ctx = ToolContext {
        cwd: cwd.to_path_buf(),
        cancel: cancel.clone(),
        session_id: None,
    };
    let text = if sub == "find" {
        crate::search_cmd::find(&args, &ctx).await
    } else {
        crate::search_cmd::grep(&args, &ctx).await
    };
    Some(finish(text))
}

/// Splits a `gray find` / `gray grep` command line into words the way the
/// shell would for the simple cases: whitespace separates, `'…'` is literal,
/// `"…"` and `\` escape. Bare `*` / `?` stay in the word, since the claimed
/// search reads them as its own glob. None when the shell would do more than
/// split — pipes, redirects, separators, substitution, an unclosed quote — so
/// the command falls through to a real shell instead of being half-claimed.
fn search_words(command: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut chars = command.chars();
    while let Some(c) = chars.next() {
        match c {
            c if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                }
            }
            '\'' => {
                in_word = true;
                loop {
                    match chars.next()? {
                        '\'' => break,
                        c => word.push(c),
                    }
                }
            }
            '"' => {
                in_word = true;
                loop {
                    match chars.next()? {
                        '"' => break,
                        '$' | '`' => return None,
                        '\\' => match chars.next()? {
                            c @ ('"' | '\\' | '$' | '`') => word.push(c),
                            '\n' => {}
                            c => {
                                word.push('\\');
                                word.push(c);
                            }
                        },
                        c => word.push(c),
                    }
                }
            }
            '\\' => {
                in_word = true;
                match chars.next()? {
                    '\n' => {}
                    c => word.push(c),
                }
            }
            '|' | '&' | ';' | '<' | '>' | '$' | '`' | '(' | ')' => return None,
            c => {
                in_word = true;
                word.push(c);
            }
        }
    }
    if in_word {
        words.push(word);
    }
    Some(words)
}

impl BashTool {
    /// Whole-command no-ops (`true`, `:`) are how a model stalls for a
    /// background job it does not know how to await. Jobs report on their own
    /// between turns, so say that instead of spawning a shell that does
    /// nothing and prints nothing.
    fn noop_steer(&self, ctx: &ToolContext, command: &str) -> Option<ToolOutput> {
        let trimmed = command.trim();
        if !matches!(trimmed, "true" | ":") {
            return None;
        }
        let live = self.jobs.live_ids(ctx);
        let mut note = format!("`{trimmed}` is a no-op: nothing ran and nothing was waited on.");
        match self.jobs.footer(ctx, &live) {
            None => {
                note.push_str(" No background jobs are running; if you are done, end your turn.")
            }
            Some(footer) => {
                note.push_str(
                    " To wait for a background job, end your turn: you are woken when it finishes.\n",
                );
                note.push_str(&footer);
            }
        }
        Some(ToolOutput::ok(note))
    }
}

/// What to say when a `cat <media>` ran as an ordinary shell command instead
/// of being claimed: the shell streams binary (or nothing) while nothing is
/// attached to the turn, which costs the model a pile of re-runs. Only shape
/// refusals get this: a bare claim that missed on a missing file is already
/// reported by the shell, and a lecture on top of its own error helps nobody.
fn unattached_media_note(command: &str) -> Option<String> {
    let ws: Vec<&str> = command.split_whitespace().collect();
    let cat_at = ws.iter().position(|t| *t == "cat")?;
    let arg = *ws.get(cat_at + 1)?;
    if !crate::images::is_viewable_extension(Path::new(arg)) {
        return None;
    }
    let bare = cat_at == 0 && ws.len() == 2 && !shell_meta(arg);
    (!bare).then(|| {
        let rerun = if shell_meta(arg) {
            "cat /path/to.jpg".to_string()
        } else {
            format!("cat {arg}")
        };
        format!(
            "note: `cat` on a media file ran as a plain shell command here, so the \
             media was NOT attached to this turn. Use the whole command instead: {rerun}"
        )
    })
}

fn image_command(command: &str, cwd: &Path) -> Option<ToolOutput> {
    let mut parts = command.split_whitespace();
    if parts.next()? != "cat" {
        return None;
    }
    // Any argument that is not a media path (text, a flag, `|`, `>`) makes
    // it `cat`'s own job again, so the shell runs it.
    let paths: Vec<&str> = parts.collect();
    if paths.is_empty()
        || paths
            .iter()
            .any(|p| shell_meta(p) || !crate::images::is_viewable_extension(Path::new(p)))
    {
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
    let mut media = Vec::new();
    let mut texts: Vec<String> = Vec::new();
    let mut bytes: usize = 0;
    for full in files {
        let part = match crate::view::attach(&full) {
            Ok(part) => part,
            Err(e) => {
                failed.push(e);
                continue;
            }
        };
        let size = match &part {
            Attached::Image(img, _) => img.data.len(),
            Attached::Media(m, _) => m.data.len(),
            Attached::Text(_, _) => 0,
        };
        if (!images.is_empty() || !media.is_empty()) && bytes + size > MAX_IMAGE_CLAIM_BYTES {
            failed.push(format!(
                "{}: skipped past the multi-image byte budget",
                full.display()
            ));
            break;
        }
        bytes += size;
        match part {
            Attached::Image(img, label) => {
                shown.push(label);
                images.push(img);
            }
            Attached::Media(m, label) => {
                shown.push(label);
                media.push(m);
            }
            Attached::Text(t, label) => {
                shown.push(label);
                texts.push(t);
            }
        }
    }
    if images.is_empty() && media.is_empty() && texts.is_empty() {
        // Every path failed: drop the claim so the shell reports the errors
        // directly, which beats a note attached to no media.
        return None;
    }
    let mut content = format!("Shown: {}", shown.join(", "));
    if capped {
        content.push_str(&format!(
            " (showing first {MAX_IMAGE_CLAIM_PATHS} of {total} paths)"
        ));
    }
    for f in &failed {
        content.push_str(&format!("; skipped: {f}"));
    }
    for t in texts {
        content.push_str("\n\n");
        content.push_str(&t);
    }
    Some(ToolOutput {
        content,
        is_error: false,
        images,
        media,
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

/// A still-running command handed back by [`settle_command`] when its
/// timeout passed on the jobs lane: the live child, its armed group guard,
/// and its already-draining pump, so [`run_command`] can register it as a
/// background job. Never a kill.
struct Handoff {
    spawned: crate::shell::contract::Spawned,
    #[cfg(not(windows))]
    guard: crate::shell::kill::GroupGuard,
    pump: tokio::task::JoinHandle<crate::shell::contract::PumpSummary>,
}

enum Settled {
    /// The command exited, timed out, was cancelled, or failed to wait.
    Done(ToolOutput),
    /// A command that reached its timeout was handed off; it keeps running. Boxed:
    /// the handoff carries the live child, guard, pump and clock (~300 B)
    /// and would otherwise blow up every `Settled` on the stack.
    Stalled(Box<Handoff>),
}

/// The shared wait-and-render for one spawned command: exit, timeout or
/// cancel. With `handoff` set (the jobs lane), reaching the timeout stops the
/// wait and returns the running child, its armed group guard and its
/// already-draining pump as [`Settled::Stalled`]; it is never killed here.
/// Without it (bare mode), the timeout kills the process group.
///
/// A distinct function from [`run_command`] on purpose: the background
/// continuation calls it with `handoff=false` and no timeout, so it is not a
/// recursive `async fn` and its future stays `Send` for `tokio::spawn`.
// one lane carries a live child + its group guard + pump + clock; bundling
// them into structs is speculative and would thrash this Send-sensitive call
// path, so the count stands.
#[allow(clippy::too_many_arguments)]
async fn settle_command(
    command: &str,
    log_path: &std::path::Path,
    secs: Option<u64>,
    start: Instant,
    ctx: &ToolContext,
    mut spawned: crate::shell::contract::Spawned,
    #[cfg(not(windows))] mut guard: crate::shell::kill::GroupGuard,
    pump: tokio::task::JoinHandle<crate::shell::contract::PumpSummary>,
    handoff: bool,
) -> Settled {
    #[cfg(not(windows))]
    let target = spawned.pgid;
    #[cfg(windows)]
    let target = &spawned.job;
    let child = &mut spawned.child;
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
                return Settled::Done(fail(format!("failed to wait for command: {e}")));
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
    if handoff && matches!(cause, Cause::Timeout) {
        // Hand the running child back to run_command, which registers it.
        return Settled::Stalled(Box::new(Handoff {
            spawned,
            #[cfg(not(windows))]
            guard,
            pump,
        }));
    }
    // Unix escalates SIGTERM -> SIGKILL; Windows terminates the owned job.
    // Failed termination is a harness error, never a successful timeout.
    let first_line: Option<String> = match cause {
        Cause::Exit => None,
        // Bare mode only: the jobs lane handed the timeout off above.
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
                    return Settled::Done(fail(e));
                }
            }
            Some(format!(
                "timed out after {}s (process group killed) \u{b7} rerun without `timeout` to let it finish, or with a larger one (max {MAX_TIMEOUT_SECS}s)",
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
                    return Settled::Done(fail(e));
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
    let (summary, drain_truncated) = match drain_pump(pump, log_path).await {
        Ok(v) => v,
        Err(e) => return Settled::Done(fail(format!("output pump failed: {e}"))),
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
    Settled::Done(finish_inline(
        command, log_path, status, &summary, start, first,
    ))
}

/// Entry point for one spawned command. Starts the output pump, runs
/// [`settle_command`], and when the command reaches its timeout on the jobs
/// lane (`adopt`), registers it as a background job and spawns the
/// continuation (a fresh [`settle_command`] with `handoff=false` and no
/// timeout, so it runs until exit or cancel), returning a "still running"
/// notice with its log, pgid and stop command. Never kills on timeout there.
// one lane carries a live child + its group guard + pump + registry handle;
// bundling them into structs is speculative and would thrash this
// Send-sensitive call path, so the count stands.
#[allow(clippy::too_many_arguments)]
async fn run_command(
    command: String,
    log_path: PathBuf,
    secs: Option<u64>,
    start: Instant,
    ctx: ToolContext,
    spawned: crate::shell::contract::Spawned,
    #[cfg(not(windows))] guard: crate::shell::kill::GroupGuard,
    // Jobs lane: where a command that reaches its timeout is registered.
    // `None` (bare mode) makes the timeout a kill.
    adopt: Option<&jobs::Jobs>,
) -> ToolOutput {
    let mut spawned = spawned;
    let last = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(now_grindmill()));
    let pump = {
        let child = &mut spawned.child;
        Pump::start(
            child.stdout.take(),
            child.stderr.take(),
            log_path.clone(),
            last,
        )
    };
    match settle_command(
        &command,
        &log_path,
        secs,
        start,
        &ctx,
        spawned,
        #[cfg(not(windows))]
        guard,
        pump,
        adopt.is_some(),
    )
    .await
    {
        Settled::Done(out) => out,
        Settled::Stalled(hoff) => {
            let Handoff {
                spawned,
                #[cfg(not(windows))]
                guard,
                pump,
            } = *hoff;
            let jobs = adopt.expect("a handoff is only armed on the jobs lane");
            // The stop command has to take the whole tree: a bare `kill
            // <pgid>` would hit only the `sh` wrapper and leave cargo running.
            #[cfg(not(windows))]
            let (handle, stop) = (
                format!("pgid {}", spawned.pgid),
                format!("kill -- -{}", spawned.pgid),
            );
            #[cfg(windows)]
            let (handle, stop) = (
                format!("pid {}", spawned.pid),
                format!("taskkill /T /F /PID {}", spawned.pid),
            );
            let (id, tx, worker_ctx) =
                jobs.register(&ctx, &command, log_path.clone(), start, stop.clone());
            let notice = handoff_notice(&id, secs.unwrap_or_default(), &handle, &stop, &log_path);
            tokio::spawn(async move {
                let out = match settle_command(
                    &command,
                    &log_path,
                    None,
                    start,
                    &worker_ctx,
                    spawned,
                    #[cfg(not(windows))]
                    guard,
                    pump,
                    false,
                )
                .await
                {
                    Settled::Done(out) => out,
                    Settled::Stalled(_) => {
                        std::unreachable!("handoff=false never returns Stalled")
                    }
                };
                let _ = tx.send(Some(out));
            });
            notice
        }
    }
}

/// The whole UX of a command that outlived its timeout: what it is, that it
/// was not killed, how to wait (end the turn), peek (`tail`) and stop it, and
/// what it printed so far (mini-swe-agent shows partial output on timeout;
/// here the command also keeps going). The first line keeps the
/// `still running · job ` shape the loop guard and transcript renderer key on.
fn handoff_notice(id: &str, secs: u64, handle: &str, stop: &str, log: &Path) -> ToolOutput {
    let quoted = shell_quote_path(log);
    let mut text = format!(
        "still running \u{b7} job {id} \u{b7} yielded after {secs}s \u{b7} {handle} \u{b7} log {}\n\
         Not killed: it keeps running in the background. To wait, end your turn; you are woken when it \
         finishes (no polling needed). Peek: `tail {quoted}`. Stop: `{stop}`.",
        log.display()
    );
    let summary = truncated_summary_from_disk(log);
    if summary.total_bytes > 0 {
        let view = build_view(log, &summary);
        text.push_str("\nPartial output (snapshot):\n");
        text.push_str(&fence(&squeeze_view(&view.body)));
    }
    ToolOutput::ok(text)
}

/// Single-quote a path for a shell command the model will copy. Forward
/// slashes for Git Bash, which treats backslashes as escapes.
fn shell_quote_path(path: &Path) -> String {
    let raw = path
        .to_string_lossy()
        .replace('\\', "/")
        .replace('\'', "'\"'\"'");
    format!("'{raw}'")
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
    let mut view = build_view(log_path, summary);
    // Kind-aware compression, after the window is chosen: `view`'s line
    // numbers (`shown_lines`, `omitted_lines`) describe the window in the
    // on-disk log, and every paging hint below is derived from them, so they
    // have to stay anchored to the log rather than to a squeezed copy of it.
    // Squeezing the body only changes how much of that window is displayed.
    // Color and repeat runs go first (the log on disk keeps every byte, so
    // `dd`/`sed` paging still lands on the text this body was cut from), so
    // the kind rules match plain lines.
    view.body = squeeze_view(&view.body);
    let squeezed = squeeze(&view.body, command);
    let squeeze_note = squeezed.squeezed().then(|| {
        spill::record(MeterEvent {
            ts: spill::now_millis(),
            rule: squeezed.rule.to_string(),
            raw: squeezed.raw_bytes as u64,
            sent: squeezed.sent_bytes as u64,
        });
        format!(
            "squeezed by {} · {} → {}",
            squeezed.rule,
            spill::fmt_bytes(squeezed.raw_bytes),
            spill::fmt_bytes(squeezed.sent_bytes)
        )
    });
    if squeeze_note.is_some() {
        view.body = squeezed.text.clone();
    }
    let head = header(&report, Some(&view), elapsed, log_path);
    let mut out = match first_line {
        Some(first) => format!("{first}\n{head}"),
        None => head,
    };
    // Disclosure before the fence: a body that is shorter than the log is a
    // statement about the bytes, and the model has to be able to see it.
    if let Some(note) = &squeeze_note {
        out.push('\n');
        out.push_str(note);
    }
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
