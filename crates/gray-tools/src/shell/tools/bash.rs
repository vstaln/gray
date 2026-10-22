use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use gray_core::agent::{Tool, ToolContext, ToolOutput};
use gray_core::message::ToolDef;
use serde_json::{Value, json};
use tokio::task::JoinHandle;

#[cfg(unix)]
use crate::shell::contract::EXIT_STDIO_GRACE_MS;
use crate::shell::contract::{
    DEFAULT_TIMEOUT_SECS, DETACHED_DRAIN_TIMEOUT, INLINE_BUDGET_BYTES, MAX_TIMEOUT_SECS,
    MEM_HEAD_BYTES, MEM_TAIL_BYTES, PUMP_DRAIN_TIMEOUT, PumpSummary, SLEEP_SLACK_SECS,
    VIEW_BUDGET_LINES, View,
};
use crate::shell::exit::exit_report;
use crate::shell::fence::fence;
use crate::shell::kill::term_then_kill;
use crate::shell::pump::{Pump, now_grindmill};
use crate::shell::spawn::{PluginEnv, spawn};
use crate::shell::view::{
    format_elapsed, header, middle_out, resume_hint, squeeze as squeeze_view,
};
use crate::view::Attached;

const READ_CHUNK: u64 = 4 * 1024;
use crate::{fail, get_opt_u64, get_str, resolve_path};

mod jobs;

impl BashTool {

    pub fn wait_any_job_fut(
        &self,
        ctx: &ToolContext,
        timeout: Duration,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send>> {
        let jobs = self.jobs.clone();
        let ctx = ctx.clone();
        Box::pin(async move { jobs.wait_any(&ctx, timeout).await })
    }

    pub fn has_unfinished_jobs(&self, ctx: &ToolContext) -> bool {
        self.jobs.has_unfinished(ctx)
    }

    pub fn running_jobs(&self, ctx: &ToolContext) -> Vec<gray_core::agent::BackgroundJob> {
        self.jobs.running(ctx)
    }

    pub fn running_jobs_note(&self, ctx: &ToolContext) -> Option<String> {
        self.jobs.footer(ctx, &self.jobs.live_ids(ctx))
    }

    pub fn cancel_job(&self, ctx: &ToolContext, id: &str) -> bool {
        self.jobs.cancel_running(ctx, id)
    }
}

#[derive(Default)]
pub struct BashTool {
    jobs: std::sync::Arc<jobs::Jobs>,

    cwd: Mutex<BTreeMap<String, (PathBuf, PathBuf)>>,

    plugin_env: PluginEnv,
}

impl BashTool {
    /// Environment contributed by enabled plugins (`shell_env` manifests);
    /// merged once when the registry is built.
    pub fn with_plugin_env(mut self, env: PluginEnv) -> Self {
        self.plugin_env = env;
        self
    }

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
        let command = match get_str(&args, "command") {
            Ok(c) if !c.trim().is_empty() => c,
            Ok(_) => return fail("command must be non-empty".into()),
            Err(e) => return e,
        };
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
        if let Some(msg) = media_dump_refusal(&command, &cwd) {
            return fail(msg);
        }

        let detach = jobs_enabled();
        let secs = if detach {
            Some(explicit.unwrap_or_else(|| block_bound(&command)))
        } else {
            explicit
        };

        let running_before = self.jobs.live_ids(ctx);

        let log_path = log_path(ctx);
        let start = Instant::now();

        let cwd_report =
            std::env::temp_dir().join(format!("gray-cwd-{}.txt", uuid::Uuid::new_v4()));
        let capture_ps =
            crate::shell::exit::masks_pipeline(&command) && crate::shell::spawn::pipestatus_shell();
        let ps_report = capture_ps
            .then(|| std::env::temp_dir().join(format!("gray-ps-{}", uuid::Uuid::new_v4())));
        let spawned = match spawn(
            &with_cwd_report(&command, capture_ps),
            &cwd,
            ctx.session_id.as_deref(),
            Some(&cwd_report),
            ps_report.as_deref(),
            Some(&self.plugin_env),
        ) {
            Ok(s) => s,
            Err(e) => return fail(format!("failed to spawn shell: {e}")),
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
            cwd_report.clone(),
            ps_report,
        )
        .await;

        self.adopt_reported_cwd(ctx, &cwd_report);

        if let Some(footer) = self.jobs.footer(ctx, &running_before) {
            out.content.push('\n');
            out.content.push_str(&footer);
        }
        out
    }
}

fn tool_def(jobs: bool) -> ToolDef {
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

    let default = default_timeout();
    ToolDef::new(
        "bash",
        format!(
            "Run a shell command. Independent commands can go as separate bash calls in the same turn \
             (read-only ones run concurrently). It blocks until the command exits or `timeout` passes; then it is \
             NOT killed but keeps running as a background job, and you get its log and a stop command. \
             To wait for it, end your turn: a finished job wakes you, no polling needed. Peek with \
             `tail <log>`. A command that exits while leaving its process group alive (`nohup ... &`, \
             `cmd &`) is registered as a background job the same way. If a command sleeps, pass a `timeout` longer than the sleep. For a hard \
             limit, write `timeout N cmd` yourself. {VIEW}"
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

fn block_bound(command: &str) -> u64 {
    let default = default_timeout();
    match leading_sleep(command) {
        Some(n) => default
            .max(n.saturating_add(SLEEP_SLACK_SECS))
            .min(MAX_TIMEOUT_SECS),
        None => default,
    }
}

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

fn default_timeout() -> u64 {
    std::env::var("GRAY_BASH_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|&s| s > 0)
        .map(|s| s.min(MAX_TIMEOUT_SECS))
        .unwrap_or(DEFAULT_TIMEOUT_SECS)
}

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

fn with_cwd_report(command: &str, pipestatus: bool) -> String {
    #[cfg(windows)]
    let reported = "$(pwd -W)";
    #[cfg(not(windows))]
    let reported = "\"$PWD\"";
    let (capture, write) = if pipestatus {
        (
            " __gray_ps=\"${PIPESTATUS[*]-}\"",
            "printf '%s' \"$__gray_ps\" > \"$GRAY_PS_REPORT\" 2>/dev/null\n",
        )
    } else {
        ("", "")
    };
    format!(
        "{command}\n__gray_rc=$?{capture}\n{write}printf '%s' {reported} > \"$GRAY_CWD_REPORT\" 2>/dev/null\nexit $__gray_rc"
    )
}

fn read_pipestatus(path: Option<&std::path::Path>) -> Option<Vec<i32>> {
    let text = std::fs::read_to_string(path?).ok()?;
    let _ = std::fs::remove_file(path?);
    let stages: Vec<i32> = text
        .split_whitespace()
        .filter_map(|t| t.parse().ok())
        .collect();
    (!stages.is_empty()).then_some(stages)
}

fn jobs_enabled() -> bool {
    std::env::var_os("GRAY_NO_JOBS").is_none()
}

fn session_key(ctx: &ToolContext) -> String {
    ctx.session_id.clone().unwrap_or_default()
}

const MAX_IMAGE_CLAIM_PATHS: usize = 8;
const MAX_IMAGE_CLAIM_BYTES: usize = 20 * 1024 * 1024;

fn is_duration(w: &str) -> bool {
    let num = w.trim_end_matches(['s', 'm', 'h', 'd']);
    !num.is_empty() && num.parse::<f64>().is_ok()
}

fn is_assignment(w: &str) -> bool {
    let Some(eq) = w.find('=') else {
        return false;
    };
    let mut name = &w[..eq];
    name = name.strip_suffix('+').unwrap_or(name);
    !name.is_empty()
        && name
            .chars()
            .enumerate()
            .all(|(i, c)| c == '_' || c.is_ascii_alphanumeric() && (i > 0 || !c.is_ascii_digit()))
}

const BYTE_DUMP_HEADS: &[&str] = &["cat", "head", "tail", "base64", "xxd", "od", "hexdump"];

/// Splits a command line at `;`, `|`, `&` and newlines so a dump head's
/// arguments never bleed into the next command (`tail x.log; ls *.mp4`).
fn command_segments(command: &str) -> Vec<Vec<&str>> {
    let b = command.as_bytes();
    let mut out = Vec::new();
    let mut start = 0;
    for i in 0..b.len() {
        let sep = match b[i] {
            b';' | b'\n' | b'|' => true,
            // `&>` and `>&` are redirects, not separators.
            b'&' => b.get(i + 1) != Some(&b'>') && (i == 0 || b[i - 1] != b'>'),
            _ => false,
        };
        if sep {
            out.push(&command[start..i]);
            start = i + 1;
        }
    }
    out.push(&command[start..]);
    out.into_iter()
        .map(|seg| seg.split_whitespace().collect::<Vec<_>>())
        .filter(|ws| !ws.is_empty())
        .collect()
}

const HEAD_WRAPPERS: &[&str] = &[
    "sudo", "doas", "nice", "env", "timeout", "time", "command", "builtin", "exec", "xargs",
    "watch", "stdbuf", "do", "then", "!",
];

fn is_command_position(ws: &[&str], i: usize) -> bool {
    ws[..i].iter().all(|p| {
        p.starts_with('-') || is_assignment(p) || is_duration(p) || HEAD_WRAPPERS.contains(p)
    })
}

fn media_dump_refusal(command: &str, cwd: &Path) -> Option<String> {
    let whole_bare = command.split_whitespace().count() == 2;
    command_segments(command)
        .iter()
        .find_map(|ws| segment_dump_refusal(ws, whole_bare, cwd))
}

fn segment_dump_refusal(ws: &[&str], whole_bare: bool, cwd: &Path) -> Option<String> {
    if ws.iter().any(|t| {
        matches!(*t, ">" | ">>" | "&>" | "&>>")
            || t.starts_with('>')
            || t.starts_with("1>")
            || t.starts_with("&>")
    }) {
        return None;
    }
    for (i, t) in ws.iter().enumerate() {
        if !BYTE_DUMP_HEADS.contains(t) || !is_command_position(ws, i) {
            continue;
        }
        for arg in &ws[i + 1..] {
            if arg.starts_with('-') {
                continue;
            }
            if !crate::images::is_viewable_extension(Path::new(arg)) {
                continue;
            }
            let bare_missing = *t == "cat"
                && i == 0
                && whole_bare
                && ws.len() == 2
                && !shell_meta(arg)
                && resolve_bare_path(cwd, arg).is_none();
            if !bare_missing {
                return Some(format!(
                    "refused: `{t} {arg}` would dump raw media bytes into context. \
                     Attach the file instead — a bare `cat {arg}` shows it to the model."
                ));
            }
        }
    }
    None
}

fn image_command(command: &str, cwd: &Path) -> Option<ToolOutput> {
    let mut parts = command.split_whitespace();
    if parts.next()? != "cat" {
        return None;
    }

    let paths: Vec<&str> = parts.collect();
    if paths.is_empty()
        || paths
            .iter()
            .any(|p| shell_meta(p) || !crate::images::is_viewable_extension(Path::new(p)))
    {
        return None;
    }

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
        usage: None,
    })
}

fn shell_meta(arg: &str) -> bool {
    arg.starts_with('-')
        || arg.chars().any(|c| {
            matches!(
                c,
                '|' | '&' | ';' | '<' | '>' | '$' | '`' | '"' | '\'' | '*' | '?' | '(' | ')'
            )
        })
}

fn resolve_bare_path(cwd: &Path, raw: &str) -> Option<PathBuf> {
    let expanded = expand_tilde(raw);
    let full = resolve_path(cwd, expanded.as_deref().unwrap_or(raw));
    full.is_file().then_some(full)
}

fn expand_tilde(raw: &str) -> Option<String> {
    let home = std::env::var("HOME").ok().filter(|h| !h.is_empty())?;
    if raw == "~" {
        return Some(home);
    }
    let rest = raw.strip_prefix("~/")?;
    Some(format!("{home}/{rest}"))
}

fn log_path(ctx: &ToolContext) -> PathBuf {
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

struct Handoff {
    spawned: crate::shell::contract::Spawned,
    #[cfg(not(windows))]
    guard: crate::shell::kill::GroupGuard,
    pump: tokio::task::JoinHandle<crate::shell::contract::PumpSummary>,
    #[cfg(unix)]
    last: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

enum Settled {
    Done(ToolOutput),

    Stalled(Box<Handoff>),
}

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
    track_group: bool,
    ps_report: Option<&std::path::Path>,
    #[cfg(unix)] last: std::sync::Arc<std::sync::atomic::AtomicU64>,
) -> Settled {
    #[cfg(windows)]
    let _ = track_group;
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

            match child.try_wait() {
                Ok(Some(st)) => { exited = Some(st); Cause::Exit }
                _ => Cause::Timeout,
            }
        }
        _ = ctx.cancel.cancelled() => Cause::Cancel,
    };
    if handoff && matches!(cause, Cause::Timeout) {
        return Settled::Stalled(Box::new(Handoff {
            spawned,
            #[cfg(not(windows))]
            guard,
            pump,
            #[cfg(unix)]
            last,
        }));
    }
    #[cfg(unix)]
    let mut detached_group = false;
    #[cfg(unix)]
    let mut pipe_open = false;
    #[cfg(unix)]
    if matches!(cause, Cause::Exit) && (track_group || crate::shell::kill::group_alive(target)) {
        if handoff {
            tokio::time::sleep(Duration::from_millis(150)).await;
            if crate::shell::kill::group_alive(target) {
                return Settled::Stalled(Box::new(Handoff {
                    spawned,
                    #[cfg(not(windows))]
                    guard,
                    pump,
                    #[cfg(unix)]
                    last,
                }));
            }
        } else if track_group {
            let exited_ms = now_grindmill();
            loop {
                let quiet_ms = now_grindmill().saturating_sub(
                    last.load(std::sync::atomic::Ordering::Relaxed)
                        .max(exited_ms),
                );
                if pump.is_finished() || quiet_ms >= EXIT_STDIO_GRACE_MS {
                    break;
                }
                let cancelled = tokio::select! {
                    _ = tokio::time::sleep(Duration::from_millis(20)) => false,
                    _ = ctx.cancel.cancelled() => true,
                };
                if cancelled {
                    let _ = term_then_kill(target, Duration::from_secs(2)).await;
                    break;
                }
            }
            detached_group = crate::shell::kill::group_alive(target);
            pipe_open = !pump.is_finished();
        } else {
            detached_group = true;
        }
    }
    #[cfg(not(unix))]
    let pipe_open = false;
    #[cfg(not(unix))]
    let detached_group = false;

    let first_line: Option<String> = match cause {
        Cause::Exit => None,

        Cause::Timeout => {
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

    #[cfg(windows)]
    drop(spawned.job);
    #[cfg(unix)]
    crate::shell::kill::untrack_group(spawned.pgid);
    #[cfg(not(windows))]
    guard.disarm();
    let status = exited.expect("every cause resolves a status");
    let pipe_stages = read_pipestatus(ps_report);
    let drain_timeout = if detached_group || pipe_open {
        DETACHED_DRAIN_TIMEOUT
    } else {
        PUMP_DRAIN_TIMEOUT
    };
    let (summary, drain_truncated) = match drain_pump(pump, log_path, drain_timeout).await {
        Ok(v) => v,
        Err(e) => return Settled::Done(fail(format!("output pump failed: {e}"))),
    };
    let mut first = first_line;
    #[cfg(unix)]
    if detached_group {
        let note = format!(
            "process group {0} still running after the command exited; it is not tracked \u{b7} stop: `kill -- -{0}`",
            spawned.pgid
        );
        first = Some(match first {
            Some(f) => format!("{f} ({note})"),
            None => note,
        });
    }
    if drain_truncated {
        let note = format!(
            "output truncated: pump drain timed out after {}s",
            drain_timeout.as_secs()
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
        command,
        log_path,
        status,
        &summary,
        start,
        first,
        pipe_stages,
    ))
}

#[allow(clippy::too_many_arguments)]
async fn run_command(
    command: String,
    log_path: PathBuf,
    secs: Option<u64>,
    start: Instant,
    ctx: ToolContext,
    spawned: crate::shell::contract::Spawned,
    #[cfg(not(windows))] guard: crate::shell::kill::GroupGuard,

    adopt: Option<&jobs::Jobs>,

    cwd_report: PathBuf,
    ps_report: Option<PathBuf>,
) -> ToolOutput {
    let mut spawned = spawned;
    let last = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(now_grindmill()));
    let output_sink = gray_core::agent::current_tool_output();
    let pump = {
        let child = &mut spawned.child;
        Pump::start(
            child.stdout.take(),
            child.stderr.take(),
            log_path.clone(),
            last.clone(),
            output_sink,
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
        false,
        ps_report.as_deref(),
        #[cfg(unix)]
        last,
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
                #[cfg(unix)]
                last,
            } = *hoff;
            let jobs = adopt.expect("a handoff is only armed on the jobs lane");

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
                    true,
                    ps_report.as_deref(),
                    #[cfg(unix)]
                    last,
                )
                .await
                {
                    Settled::Done(out) => out,
                    Settled::Stalled(_) => {
                        std::unreachable!("handoff=false never returns Stalled")
                    }
                };
                let _ = std::fs::remove_file(&cwd_report);
                let _ = tx.send(Some(out));
            });
            notice
        }
    }
}

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

async fn drain_pump(
    mut pump: JoinHandle<PumpSummary>,
    log_path: &std::path::Path,
    timeout: Duration,
) -> Result<(PumpSummary, bool), tokio::task::JoinError> {
    match tokio::time::timeout(timeout, &mut pump).await {
        Ok(res) => res.map(|s| (s, false)),
        Err(_) => {
            pump.abort();
            let _ = (&mut pump).await;
            log::warn!("shell: pump drain timed out; aborted, reporting truncated");
            Ok((truncated_summary_from_disk(log_path), true))
        }
    }
}

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

#[allow(clippy::too_many_arguments)]
fn finish_inline(
    command: &str,
    log_path: &std::path::Path,
    status: std::process::ExitStatus,
    summary: &PumpSummary,
    start: Instant,
    first_line: Option<String>,
    pipe_stages: Option<Vec<i32>>,
) -> ToolOutput {
    let elapsed = start.elapsed();
    let report = exit_report(status, command, pipe_stages.as_deref());
    let mut view = build_view(log_path, summary);

    view.body = squeeze_view(&view.body);
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
        let path = log_path
            .to_string_lossy()
            .replace('\\', "/")
            .replace('\'', "'\"'\"'");
        let command = if small_log_on_disk(log_path, summary) {
            let first = view.shown_lines.0.max(1);
            let last = first.saturating_add(199);
            format!("sed -n '{first},{last}p;{last}q' '{path}' | head -c {READ_CHUNK}")
        } else {
            let count = end.saturating_sub(start).min(READ_CHUNK);
            format!("dd if='{path}' bs=1 skip={start} count={count} 2>/dev/null")
        };

        let next = start + READ_CHUNK;
        if next < end {
            out.push_str(&format!(
                "\nThen skip={next} for the next {READ_CHUNK} bytes."
            ));
        }
        out.push_str(&format!("\nRead more: {command}"));
    }
    ToolOutput::ok(out)
}

fn small_log_on_disk(log_path: &std::path::Path, summary: &PumpSummary) -> bool {
    let file_len = std::fs::metadata(log_path).map(|m| m.len()).unwrap_or(0);
    summary.total_bytes <= INLINE_BUDGET_BYTES as u64 && file_len <= INLINE_BUDGET_BYTES as u64
}

fn build_view(log_path: &std::path::Path, summary: &PumpSummary) -> View {
    let use_disk = small_log_on_disk(log_path, summary);

    if use_disk && let Ok(bytes) = std::fs::read(log_path) {
        let mut view = middle_out(&bytes, INLINE_BUDGET_BYTES, VIEW_BUDGET_LINES, 0);
        if view.omitted_range.is_some() {
            let hint = resume_hint(&view, log_path);
            view.body = view.body.replace("{{MARKER}}", &hint);
        }
        return view;
    }

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
        let mut cat = summary.head.clone();
        cat.extend_from_slice(&summary.tail);
        return middle_out(&cat, INLINE_BUDGET_BYTES, VIEW_BUDGET_LINES, 0);
    }

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
