//! Shell execution: ordinary blocking calls and session-owned background jobs.
//! Both modes share timeout, cancellation, redaction, bounded output and reaping.

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
    DEFAULT_TIMEOUT_SECS, INLINE_BUDGET_BYTES, MAX_TIMEOUT_SECS, MAX_YIELD_MS, MEM_HEAD_BYTES,
    MEM_TAIL_BYTES, MIN_YIELD_MS, PUMP_DRAIN_TIMEOUT, PumpSummary, VIEW_BUDGET_LINES, View,
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
use crate::{fail, finish, get_opt_bool, get_opt_u64, get_str, resolve_path};

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
        // Only what the model can't infer: shell basics it already knows.
        const VIEW: &str =
            "`cat` bare image, video, PDF or audio paths (as the whole command) to view them.";
        if !jobs_enabled() {
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
        ToolDef::new(
            "bash",
            format!(
                "Run a shell command; no default timeout. background:true or yield_ms \
                 returns a job id; then action status/output/cancel/list with job_id. {VIEW}"
            ),
            json!({
                "type": "object",
                "properties": {
                    "command": {"type": "string", "description": "Shell command to run (run action only — omit for job actions)"},
                    "action": {"type": "string", "enum": ["run", "list", "status", "output", "cancel"]},
                    "job_id": {"type": "string"},
                    "background": {"type": "boolean"},
                    "timeout": {"type": "integer", "description": "Seconds; omitted = no limit (max 3600)"},
                    "yield_ms": {"type": "integer", "description": "Return a job id if still running after this (100-10000)"},
                    "wait_ms": {"type": "integer", "description": "output/status: wait up to this for exit (max 600000)"}
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
            if !jobs_enabled() {
                return fail(
                    "managed jobs are disabled here (GRAY_NO_JOBS=1): drop `action`, `job_id` and \
                     `background` and just run the command."
                        .into(),
                );
            }
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
        if let Some(out) = self.noop_steer(ctx, &command) {
            return out;
        }
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
        if let Some(out) = search_command(&command, &cwd, &ctx.cancel).await {
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
        // Read dedup: a `cat`/`sed`/`head`/`tail` of a file already shown
        // whole and unchanged since answers with a stub instead of the bytes
        // (once — see `read_dedup`). Inline lane only: a backgrounded read
        // neither stubs nor records, so job semantics stay as they were.
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
            Some(&self.jobs),
            stall_bound(),
            // Auto-yield only when the jobs lane exists to receive the handoff
            // and the caller set no explicit timeout: an explicit timeout is
            // the caller saying "block me until this is done or N seconds".
            if jobs_enabled() && secs.is_none() {
                auto_yield_window()
            } else {
                Duration::ZERO
            },
            None,
        )
        .await;
        // Whether it succeeded, failed or was killed: a `cd` that ran is a `cd`
        // that ran. A killed command never writes the report, so its cwd stands.
        self.adopt_reported_cwd(ctx, &cwd_report);
        // Arm the next dedup from what this run actually showed. A settled,
        // complete result is the only thing worth citing: a timeout, a
        // cancellation, a silent hand-off or a truncated pump drain all
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

/// `GRAY_NO_JOBS=1` drops the managed-job surface: no `action`, `job_id`,
/// `background`, `yield_ms` or `wait_ms` in the schema, and those actions are
/// refused. The DeepSWE runs touched jobs in 153 of 6,915 bash calls while
/// paying for five extra schema properties (and the prose about them) on
/// every one. `timeout` stays: it is the anti-hang knob, not a jobs feature.
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
    /// background job it does not know how to await — jobs report on their
    /// own between turns, so steer the stall instead of spawning a shell
    /// that does nothing and prints nothing.
    fn noop_steer(&self, ctx: &ToolContext, command: &str) -> Option<ToolOutput> {
        let trimmed = command.trim();
        if !matches!(trimmed, "true" | ":") {
            return None;
        }
        let live = self.jobs.live_ids(ctx);
        let mut note =
            format!("`{trimmed}` is a no-op — nothing ran and nothing was waited on.");
        if live.is_empty() {
            note.push_str(
                " To end the turn just end it; to collect a finished job use \
                 action:output with job_id.",
            );
        } else {
            note.push_str(&format!(
                " Live jobs: {}. Collect one with action:output + job_id \
                 (wait_ms blocks until it settles), or end the turn — \
                 completions are reported automatically.",
                live.join(", ")
            ));
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

/// Auto-yield arm: resolves once the command has been running for `window`
/// since `start`. Disabled (`enabled` false / zero window) is `pending()`
/// forever, so the `select!` arm never fires. Wall-clock based, unlike the
/// silence arm: output resets nothing on purpose — a build that streams
/// progress for an hour still yields at the window so the model can do
/// other work while it runs.
async fn yield_arm(enabled: bool, start: Instant, window: Duration) {
    if !enabled || window.is_zero() {
        std::future::pending::<()>().await;
    }
    tokio::time::sleep_until(tokio::time::Instant::from_std(start + window)).await
}

/// Auto-yield window for the blocking lane (ms), overridable per-process via
/// `GRAY_BASH_YIELD_MS`; `0` disables. Default [`DEFAULT_AUTO_YIELD_MS`].
fn auto_yield_window() -> Duration {
    std::env::var("GRAY_BASH_YIELD_MS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map(Duration::from_millis)
        .unwrap_or(Duration::from_millis(
            crate::shell::contract::DEFAULT_AUTO_YIELD_MS,
        ))
}

/// A still-running command handed back by [`settle_command`] on a silent
/// stall or an auto-yield: the live child, its armed group guard, and its
/// already-draining pump, so [`run_command`] can register it as a background
/// job. Never a kill. `silenced` distinguishes the two (stuck vs merely slow)
/// for the notice the agent reads.
struct Handoff {
    spawned: crate::shell::contract::Spawned,
    #[cfg(not(windows))]
    guard: crate::shell::kill::GroupGuard,
    pump: tokio::task::JoinHandle<crate::shell::contract::PumpSummary>,
    silenced: bool,
}

enum Settled {
    /// The command exited, timed out, was cancelled, or failed to wait.
    Done(ToolOutput),
    /// A silent blocking command was handed off; it keeps running. Boxed:
    /// the handoff carries the live child, guard, pump and clock (~300 B)
    /// and would otherwise blow up every `Settled` on the stack.
    Stalled(Box<Handoff>),
}

/// The shared wait-and-render for one spawned command (exit, explicit timeout,
/// or cancel). When `handoff` is set and no explicit `timeout` was requested, a
/// command silent longer than `bound` stops the wait early and returns the
/// running child, its armed group guard and its already-draining pump as
/// [`Settled::Stalled`] — it is never killed here.
///
/// A distinct function from [`run_command`] on purpose: the background
/// continuation calls it with `handoff=false`, so it is not a recursive
/// `async fn` and its future stays `Send` for `tokio::spawn`.
// one lane carries a live child + its group guard + pump + liveness clock
// + silence bound + registry handle; bundling them into structs is speculative
// and would thrash this Send-sensitive call path, so the count stands.
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
    last: std::sync::Arc<std::sync::atomic::AtomicU64>,
    bound: Duration,
    // Auto-yield window for the blocking lane (0 disables). Still-running
    // past this (without an explicit `timeout`) hands off like the silence
    // path — independent of output, so a chatty build yields too.
    auto_yield: Duration,
    handoff: bool,
) -> Settled {
    #[cfg(not(windows))]
    let target = spawned.pgid;
    #[cfg(windows)]
    let target = &spawned.job;
    let child = &mut spawned.child;
    let stall_on = handoff && secs.is_none() && !bound.is_zero();
    // Duration-based auto-yield: a command still running after `auto_yield`
    // hands off to the background lane even while producing output. Silence
    // (`bound`) covers the hung case; this covers the merely slow one. Both
    // share the same `Settled::Stalled` handoff — the child keeps running and
    // the agent keeps working. Armed only on the blocking lane (handoff),
    // without an explicit `timeout`, when the window is non-zero.
    let auto_yield_on = handoff && secs.is_none() && !auto_yield.is_zero();
    enum Cause {
        Exit,
        Timeout,
        Cancel,
        Stall,
        Yield,
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
        _ = stall_arm(stall_on, last.clone(), bound) => Cause::Stall,
        _ = yield_arm(auto_yield_on, start, auto_yield) => Cause::Yield,
    };
    if matches!(cause, Cause::Stall | Cause::Yield) {
        // Hand the running child back to run_command, which registers it.
        return Settled::Stalled(Box::new(Handoff {
            spawned,
            #[cfg(not(windows))]
            guard,
            pump,
            silenced: matches!(cause, Cause::Stall),
        }));
    }
    // Unix escalates SIGTERM -> SIGKILL; Windows terminates the owned job.
    // Failed termination is a harness error, never a successful timeout.
    let first_line: Option<String> = match cause {
        Cause::Exit => None,
        // Stall and yield return above; they never reach this render path.
        Cause::Stall | Cause::Yield => {
            std::unreachable!("stall/yield return before the render path")
        }
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

/// Blocking + managed-background entry point for one command. Starts the
/// output pump (or reuses a handed-off command's), runs [`settle_command`]
/// with `handoff` gated on the blocking lane, and on a silent stall hands the
/// running child to the background lane: registers the job and spawns the
/// continuation (a fresh [`settle_command`] with `handoff=false`, so it never
/// re-hands-off), returning a "still running" notice. The child keeps running
/// and is never killed.
// one lane carries a live child + its group guard + pump + liveness clock
// + silence bound + registry handle; bundling them into structs is speculative
// and would thrash this Send-sensitive call path, so the count stands.
#[allow(clippy::too_many_arguments)]
async fn run_command(
    command: String,
    log_path: PathBuf,
    secs: Option<u64>,
    start: Instant,
    ctx: ToolContext,
    spawned: crate::shell::contract::Spawned,
    #[cfg(not(windows))] guard: crate::shell::kill::GroupGuard,
    // Blocking lane only. When `Some`, a command silent longer than `bound`
    // (with no explicit `timeout`) is registered here as a background job and
    // the wait continues in a detached worker — never killed. `None` for the
    // managed background/job lane, whose own call never blocks.
    adopt: Option<&jobs::Jobs>,
    // Silent threshold before a stuck blocking command is handed to the
    // background lane (0 disables). Only armed for `adopt` + no `timeout`.
    bound: Duration,
    // Auto-yield window: a blocking command still running after this long is
    // handed to the background lane even while producing output (0 disables).
    // Only armed for `adopt` + no `timeout` + jobs enabled.
    auto_yield: Duration,
    // Continuation of a handed-off command: reuse the running pump instead of
    // starting a new one (stdout/stderr are already taken).
    reuse_pump: Option<tokio::task::JoinHandle<crate::shell::contract::PumpSummary>>,
) -> ToolOutput {
    let mut spawned = spawned;
    let last = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(now_grindmill()));
    let pump = match reuse_pump {
        // Continuation: the original pump already owns the child's pipes, so
        // reuse it and do not take stdout/stderr again.
        Some(p) => p,
        None => {
            let child = &mut spawned.child;
            Pump::start(
                child.stdout.take(),
                child.stderr.take(),
                log_path.clone(),
                last.clone(),
            )
        }
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
        last,
        bound,
        auto_yield,
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
                silenced: bound_elapsed,
            } = *hoff;
            let jobs = adopt.expect("a stall is only armed for the blocking lane");
            let (id, tx, worker_ctx) = jobs.register(&ctx, &command, log_path.clone(), start);
            // With GRAY_NO_JOBS=1 the follow-up actions are not in the schema
            // and are refused, so the notice must not advertise them.
            let await_hint = if jobs_enabled() {
                format!(
                    ", or await it here with bash action: output/status job_id:{id} and wait_ms \
                     (e.g. 30000), or cancel it"
                )
            } else {
                String::new()
            };
            // Silence-handoff (bound elapsed, no output) and auto-yield
            // (window elapsed, output irrelevant) read differently: the first
            // hints the command may be stuck, the second is routine duration
            // backgrounding — never imply stuckness for a command that may
            // simply be a long build.
            let notice = if !bound_elapsed {
                ToolOutput::ok(format!(
                    "still running \u{b7} job {id} \u{b7} yielded after {} (duration limit, not a stall) \u{b7} log {}\nMoved to a background job; it keeps running. Continue other work now (its finish is reported between model rounds or the next turn){await_hint}.",
                    auto_yield.as_secs(),
                    log_path.display()
                ))
            } else {
                ToolOutput::ok(format!(
                    "still running \u{b7} job {id} \u{b7} silent: no new output for {}s \u{b7} log {}\nNot killed \u{2014} moved to a background job so it keeps running. Continue other work (its finish is reported between model rounds or the next turn){await_hint}. Inspect the log to see why it went silent.",
                    bound.as_secs(),
                    log_path.display()
                ))
            };
            // A fresh liveness clock feeds the job's own (inert) stall arm.
            let last = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(now_grindmill()));
            tokio::spawn(async move {
                let out = match settle_command(
                    &command,
                    &log_path,
                    secs,
                    start,
                    &worker_ctx,
                    spawned,
                    #[cfg(not(windows))]
                    guard,
                    pump,
                    last,
                    Duration::ZERO,
                    Duration::ZERO,
                    false,
                )
                .await
                {
                    Settled::Done(out) => out,
                    // handoff=false keeps the job lane's stall arm inert.
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
/// Poll granularity for the stall arm: fine enough to hand off promptly,
/// coarse enough that a 600s bound costs a few hundred cheap wakeups at most.
const STALL_CHECK_MS: u64 = 500;

/// The blocking lane's stall arm: resolves once `last` has not advanced for
/// `bound` (a silent command still stamps `last` at spawn, so the gap is
/// measured from the last output chunk). When `enabled` is false it is
/// `pending()` forever, so the `select!` arm never fires \u{2014} this is how the
/// background job's own `run_command` (which must never re-hand-off) keeps the
/// arm inert.
async fn stall_arm(
    enabled: bool,
    last: std::sync::Arc<std::sync::atomic::AtomicU64>,
    bound: Duration,
) {
    if !enabled || bound.is_zero() {
        std::future::pending::<()>().await;
    }
    let bound_ms = u64::try_from(bound.as_millis()).unwrap_or(u64::MAX);
    loop {
        let gap = now_grindmill().saturating_sub(last.load(std::sync::atomic::Ordering::Relaxed));
        if gap >= bound_ms {
            return;
        }
        // Wake to re-check at the earlier of the remaining gap or a coarse
        // interval, so a command that resumes output is noticed cheaply.
        let remaining = bound_ms.saturating_sub(gap);
        tokio::time::sleep(Duration::from_millis(remaining.min(STALL_CHECK_MS))).await;
    }
}

/// Silent threshold for the blocking lane (seconds), overridable per-process in
/// tests via `GRAY_SHELL_STALL_SECS`. Never a kill: past this a stuck command
/// is handed to the background lane and the agent decides.
fn stall_bound() -> Duration {
    std::env::var("GRAY_SHELL_STALL_SECS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|&s| s > 0)
        .map(Duration::from_secs)
        .unwrap_or(Duration::from_secs(
            crate::shell::contract::MAX_BLOCKING_SILENCE_SECS,
        ))
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
