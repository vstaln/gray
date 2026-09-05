//! The `bash` tool: runs a shell command via `sh -c` with timeout + cancel.

use std::time::Duration;

use async_trait::async_trait;
use gray_core::agent::{ToolContext, ToolOutput};
use gray_core::message::ToolDef;
use serde_json::Value;
use serde_json::json;
use tokio::io::AsyncReadExt;
use tokio::process::Command;

use crate::{Tool, fail, finish, get_opt_u64, get_str};

/// Sanitize binary output: filter 0x00-0x1f except 0x09,0x0A,0x0D, trim to last valid UTF-8
fn sanitize_binary_output(bytes: &[u8]) -> String {
    let filtered: Vec<u8> = bytes
        .iter()
        .filter(|&&b| b == 0x09 || b == 0x0A || b == 0x0D || b >= 0x20)
        .copied()
        .collect();
    // Trim to last valid UTF-8 boundary and convert lossily
    String::from_utf8_lossy(&filtered).into_owned()
}

/// Tail truncation: keep last MAX_LINES / MAX_BYTES, return (truncated_content, Option<temp_path>)
fn truncate_bash_tail(text: &str) -> (String, Option<String>) {
    use crate::{MAX_BYTES, MAX_LINES};
    let total_lines = text.lines().count();
    let needs_truncate = total_lines > MAX_LINES || text.len() > MAX_BYTES;
    if !needs_truncate {
        return (text.to_string(), None);
    }
    // Keep tail: last MAX_LINES and last MAX_BYTES
    let lines: Vec<&str> = text.lines().collect();
    let tail_start_line = total_lines.saturating_sub(MAX_LINES);
    let mut tail_text = lines[tail_start_line..].join("\n");
    if text.ends_with('\n') {
        tail_text.push('\n');
    }
    // Byte cap on tail — keep last MAX_BYTES bytes at char boundary
    let mut truncated = tail_text;
    if truncated.len() > MAX_BYTES {
        let half = MAX_BYTES;
        let raw_start = truncated.len().saturating_sub(half);
        // Find char boundary for start
        let mut start = raw_start;
        while start < truncated.len() && !truncated.is_char_boundary(start) {
            start += 1;
        }
        if start < truncated.len() {
            truncated = truncated[start..].to_string();
        } else {
            truncated = String::new();
        }
        // Handle partial first line: if we cut mid-line, drop it
        if !truncated.starts_with('\n')
            && tail_start_line > 0
            && let Some(nl) = truncated.find('\n')
        {
            truncated = truncated[nl + 1..].to_string();
            truncated = format!(
                "[truncated ... showing last {} lines / {} bytes]\n{}",
                MAX_LINES, MAX_BYTES, truncated
            );
        }
    } else if total_lines > MAX_LINES {
        truncated = format!(
            "[truncated {} lines, showing last {} lines]\n{}",
            total_lines - MAX_LINES,
            MAX_LINES,
            truncated
        );
    }
    // Create temp file for full output (tail truncation keeps only last lines, full is in temp file)
    let temp_path = {
        let mut path = std::env::temp_dir().join(format!(
            "bash-{}-{}.log",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        // Ensure unique if collision
        let mut counter = 0;
        while path.exists() {
            counter += 1;
            path = std::env::temp_dir().join(format!(
                "bash-{}-{}-{}.log",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0),
                counter
            ));
        }
        if std::fs::write(&path, text.as_bytes()).is_ok() {
            Some(path.to_string_lossy().to_string())
        } else {
            None
        }
    };
    let mut out = truncated;
    if let Some(ref p) = temp_path {
        out.push_str(&format!("\n[full output: {}]", p));
    }
    (out, temp_path)
}

/// Default command timeout in seconds.
const DEFAULT_TIMEOUT_SECS: u64 = 60;
/// Hard upper bound for the timeout argument.
const MAX_TIMEOUT_SECS: u64 = 300;

pub const BASH_SNIPPET: &str = "Execute bash commands (ls, grep, find, etc.)";
pub const BASH_GUIDELINES: &[&str] = &[];

/// Runs a command through the shell (`sh -c`).
pub struct BashTool;

#[async_trait]
impl Tool for BashTool {
    fn def(&self) -> ToolDef {
        ToolDef::new(
            "bash",
            "Run a shell command via `sh -c` and capture stdout/stderr. \
             Times out after `timeout` seconds (default 60, max 300).",
            json!({
                "type": "object",
                "properties": {
                    "command": {"type": "string", "description": "Shell command to run"},
                    "timeout": {
                        "type": "integer",
                        "description": "Timeout in seconds (default 60, capped at 300)"
                    }
                },
                "required": ["command"]
            }),
        )
    }

    fn prompt_snippet(&self) -> Option<&'static str> {
        Some(BASH_SNIPPET)
    }

    fn prompt_guidelines(&self) -> Option<&'static [&'static str]> {
        Some(BASH_GUIDELINES)
    }

    async fn execute(&self, ctx: &ToolContext, args: Value) -> ToolOutput {
        let command = match get_str(&args, "command") {
            Ok(c) => c,
            Err(e) => return e,
        };
        let requested = match get_opt_u64(&args, "timeout") {
            Ok(t) => t,
            Err(e) => return e,
        };
        let secs = requested
            .unwrap_or(DEFAULT_TIMEOUT_SECS)
            .clamp(1, MAX_TIMEOUT_SECS);

        match classify(&command) {
            Decision::Allow => {}
            Decision::Deny(msg) => return fail(msg),
            Decision::Prompt { rule, why, alt } => {
                if !prompt_allowance(rule) {
                    return fail(format!(
                        "Blocked by destructive-command guard ({rule}): already asked twice this session — have the user run it manually. {why}"
                    ));
                }
                if !ask_allow_once(ctx, &command, rule, &why, &alt).await {
                    return fail(format!(
                        "Blocked by destructive-command guard ({rule}): user did not approve. {why} Safe alternative: {alt}."
                    ));
                }
            }
        }

        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg(&command)
            .current_dir(&ctx.cwd)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .env("DEBIAN_FRONTEND", "noninteractive")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("SUDO_ASKPASS", "/bin/false")
            .kill_on_drop(true);
        #[cfg(unix)]
        {
            unsafe {
                cmd.pre_exec(|| {
                    // Detach from controlling terminal (setsid) so child processes cannot
                    // open /dev/tty to block on password prompts or leak onto the TUI.
                    libc::setsid();
                    Ok(())
                });
            }
        }

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => return fail(format!("failed to spawn `sh -c`: {e}")),
        };

        // Drain pipes on separate tasks so a chatty child cannot deadlock us.
        let stdout_task = spawn_drain(child.stdout.take());
        let stderr_task = spawn_drain(child.stderr.take());

        let status = tokio::select! {
            status = child.wait() => match status {
                Ok(status) => status,
                Err(e) => return fail(format!("failed to wait for command: {e}")),
            },
            _ = tokio::time::sleep(Duration::from_secs(secs)) => {
                let _ = child.start_kill();
                let _ = child.wait().await;
                return fail(format!(
                    "command timed out after {secs}s and was killed: {command}"
                ));
            }
            _ = ctx.cancel.cancelled() => {
                let _ = child.start_kill();
                let _ = child.wait().await;
                return fail(format!("command cancelled and killed: {command}"));
            }
        };

        // Child exited: pipes are closed, drain tasks finish promptly.
        let stdout_bytes = join_drain_bytes(stdout_task).await;
        let stderr_bytes = join_drain_bytes(stderr_task).await;
        let stdout = sanitize_binary_output(&stdout_bytes);
        let stderr = sanitize_binary_output(&stderr_bytes);

        let mut combined = String::new();
        for stream in [&stdout, &stderr] {
            if !stream.is_empty() {
                if !combined.is_empty() {
                    combined.push('\n');
                }
                combined.push_str(stream);
            }
        }

        // Tail truncation: keep last lines/bytes, write full to temp file
        let (truncated, _temp_path) = truncate_bash_tail(&combined);
        match status.code() {
            Some(0) => finish(truncated),
            Some(code) => fail(format!("command exited with code {code}\n{truncated}")),
            None => fail(format!("command terminated by signal\n{truncated}")),
        }
    }
}

/// Spawns a task reading a pipe fully into bytes (for binary sanitization).
fn spawn_drain(
    pipe: Option<impl AsyncReadExt + Unpin + Send + 'static>,
) -> tokio::task::JoinHandle<Vec<u8>> {
    tokio::spawn(async move {
        let mut buf = Vec::new();
        if let Some(mut pipe) = pipe {
            let _ = pipe.read_to_end(&mut buf).await;
        }
        buf
    })
}

async fn join_drain_bytes(handle: tokio::task::JoinHandle<Vec<u8>>) -> Vec<u8> {
    handle.await.unwrap_or_default()
}

/// Guard verdict as data: Allow / Prompt / Forbidden.
enum Decision {
    Allow,
    /// Ask the user (first 2 occurrences per process, then auto-deny).
    Prompt {
        rule: &'static str,
        why: String,
        alt: String,
    },
    Deny(String),
}

/// Repeat counts for Prompt rules — graduated response.
/// Per-process memory (resets on restart) — persist when a real incident demands it.
static PROMPT_SEEN: std::sync::Mutex<Vec<(&'static str, usize)>> =
    std::sync::Mutex::new(Vec::new());

fn prompt_allowance(rule: &'static str) -> bool {
    let mut seen = PROMPT_SEEN.lock().unwrap_or_else(|e| e.into_inner());
    let n = seen.iter_mut().find(|(r, _)| *r == rule).map(|(_, n)| n);
    let count = match n {
        Some(c) => {
            *c += 1;
            *c
        }
        None => {
            seen.push((rule, 1));
            1
        }
    };
    count <= 2
}

/// Never-legit destructive commands, evaluated before spawn (dcg core-pack ideas,
/// reimplemented std-only).
/// Bypass: `GRAY_GUARD_BYPASS=1` (dcg `DCG_BYPASS=1` parity, for CI/piped mode).
/// Token/substring matching, no regex/AST; heredoc/`python -c`
/// payloads are unscanned — upgrade when a real incident hits.
fn classify(command: &str) -> Decision {
    if std::env::var("GRAY_GUARD_BYPASS").as_deref() == Ok("1") {
        return Decision::Allow;
    }
    match classify_normalized(&normalize_guard_head(command)) {
        Decision::Allow => embedded_payload(command)
            .map(|p| classify_normalized(&normalize_guard_head(&p)))
            .unwrap_or(Decision::Allow),
        d => d,
    }
}

/// Strips wrapper prefixes agents prepend: repeated `sudo`/`command`/`env K=V`, `\cmd` escapes.
fn normalize_guard_head(command: &str) -> String {
    let mut rest = command.trim_start().to_string();
    loop {
        let t = rest.trim_start();
        if let Some(after) = t.strip_prefix("sudo ") {
            rest = after.to_string();
        } else if let Some(after) = t.strip_prefix("command ") {
            rest = after.to_string();
        } else if let Some(after) = t.strip_prefix("env ") {
            // drop KEY=VAL pairs following env
            let mut parts = after.split_whitespace();
            let mut idx = 0usize;
            let mut cut = after.len();
            for part in parts.by_ref() {
                if part.contains('=') {
                    idx += part.len() + 1;
                } else {
                    cut = idx;
                    break;
                }
            }
            rest = after[cut.min(after.len())..].to_string();
        } else if let Some(after) = t.strip_prefix('\\') {
            rest = after.to_string();
        } else {
            return t.to_string();
        }
    }
}

/// Extracts `sh|bash -c "<payload>"` for recursive scanning (obvious bypass otherwise).
fn embedded_payload(command: &str) -> Option<String> {
    let mut tokens = command.split_whitespace().peekable();
    if !matches!(
        tokens.next(),
        Some("sh") | Some("bash") | Some("dash") | Some("zsh")
    ) {
        return None;
    }
    let mut seen_c = false;
    let mut rest: Vec<&str> = Vec::new();
    for tok in tokens {
        if seen_c {
            rest.push(tok);
        } else if tok == "-c" {
            seen_c = true;
        }
    }
    if rest.is_empty() {
        return None;
    }
    let joined = rest.join(" ");
    Some(joined.trim_matches(|c| c == '"' || c == '\'').to_string())
}

fn classify_normalized(cmd: &str) -> Decision {
    let head = cmd.split_whitespace().next().unwrap_or("");
    let base = head.rsplit('/').next().unwrap_or(head);
    let deny = |rule: &'static str, why: String, alt: &str| {
        Decision::Deny(format!(
            "Blocked by destructive-command guard ({rule}): {why}. Safe alternative: {alt}. \
             If the user explicitly asked for this, have them run it manually."
        ))
    };
    let prompt = |rule: &'static str, why: String, alt: &str| Decision::Prompt {
        rule,
        why,
        alt: alt.to_string(),
    };
    match base {
        "mkfs" | "mkswap" | "wipefs" | "mkfs.ext4" | "mkfs.xfs" | "mkfs.vfat" | "mkfs.btrfs" => {
            return deny(
                "disk-wipe",
                format!("{base} destroys filesystems"),
                "operate on a disposable VM/disk image, snapshot first",
            );
        }
        "shutdown" | "poweroff" | "reboot" | "halt" => {
            return deny(
                "host-power",
                format!("{base} takes the host down"),
                "schedule downtime with the user first",
            );
        }
        "fdisk" | "parted" => {
            if !(cmd.contains("-l") || cmd.contains("print")) {
                return deny(
                    "disk-edit",
                    format!("{base} without list/print edits partition tables"),
                    &format!("{base} -l / {base} print to inspect read-only"),
                );
            }
            return Decision::Allow;
        }
        "systemctl" => {
            if cmd.contains("poweroff") || cmd.contains("reboot") {
                return deny(
                    "host-power",
                    "systemctl poweroff/reboot takes the host down".to_string(),
                    "schedule downtime with the user first",
                );
            }
            return Decision::Allow;
        }
        _ => {}
    }
    // Fork-bomb needs a function definition too — bare ":|:&" in prose (echo) is not one.
    if cmd.contains(":|:&") && cmd.contains("()") {
        return deny(
            "fork-bomb",
            "fork bomb pattern hangs the host".to_string(),
            "don't run fork bombs",
        );
    }
    if base == "dd" && cmd.contains("of=/dev/") {
        return deny(
            "dd-device",
            "dd writing to /dev/ destroys disks".to_string(),
            "write to a regular file, double-check `of=`",
        );
    }
    if base == "rm" {
        if cmd.contains("--no-preserve-root") {
            return deny(
                "rm-rf-root",
                "rm --no-preserve-root disables the last safeguard".to_string(),
                "delete a narrower path, preview with `ls`/`find … | wc -l` first",
            );
        }
        let targets_root = cmd
            .split_whitespace()
            .skip(1)
            .filter(|t| !t.starts_with('-'))
            .any(|t| {
                matches!(
                    t,
                    "/" | "/*" | "~" | "~/*" | "/root" | "/home" | "/etc" | "/boot"
                )
            });
        if targets_root {
            return deny(
                "rm-rf-root",
                "rm targeting a system root is unrecoverable".to_string(),
                "delete a narrower path, preview with `ls`/`find … | wc -l` first",
            );
        }
        return Decision::Allow;
    }
    if base == "git" {
        if cmd.contains("reset") && cmd.contains("--hard") {
            return prompt(
                "git-reset-hard",
                "git reset --hard discards uncommitted work".to_string(),
                "`git stash` first or have the user run it",
            );
        }
        if cmd.contains("clean")
            && cmd
                .split_whitespace()
                .any(|t| t.starts_with('-') && t.contains('f'))
        {
            return prompt(
                "git-clean-force",
                "git clean -f deletes untracked files permanently".to_string(),
                "`git clean -n` to preview, `git stash -u` to keep",
            );
        }
        if cmd.contains("push") && cmd.split_whitespace().any(|t| t == "--force" || t == "-f") {
            return prompt(
                "git-push-force",
                "git push --force rewrites shared history".to_string(),
                "`git push --force-with-lease` after user confirmation",
            );
        }
    }
    Decision::Allow
}

/// Asks the connected user whether a Prompt-verdict command may run once.
/// Fail-closed: no bridge, cancel, error, or anything but an explicit
/// "Run once" denies (codex: Esc always cancels).
async fn ask_allow_once(
    ctx: &ToolContext,
    command: &str,
    rule: &str,
    why: &str,
    alt: &str,
) -> bool {
    use gray_core::questions::{UserOption, UserQuestion};
    let Some(bridge) = &ctx.questions else {
        return false;
    };
    let preview: String = command.chars().take(120).collect();
    let q = UserQuestion {
        id: "guard-approval".to_string(),
        header: "Allow?".to_string(),
        question: format!("[{rule}] Run this once? {why} Alternative: {alt}"),
        options: vec![
            UserOption {
                label: "Deny (Recommended)".to_string(),
                description: "Do not run it.".to_string(),
            },
            UserOption {
                label: "Run once".to_string(),
                description: format!("Run this once: {preview}"),
            },
        ],
        is_other: false,
    };
    match bridge.0.ask(vec![q], true).await {
        Ok(answers) => answers
            .iter()
            .flat_map(|a| &a.answers)
            .any(|s| s == "Run once"),
        Err(_) => false,
    }
}

#[cfg(test)]
mod guard_tests {
    use super::*;

    fn is_deny(cmd: &str) -> bool {
        // Bypass env must not leak between tests; classify honors it.
        assert_ne!(
            std::env::var("GRAY_GUARD_BYPASS").as_deref(),
            Ok("1"),
            "bypass set during test"
        );
        matches!(classify(cmd), Decision::Deny(_))
    }

    #[test]
    fn blocks_rm_root_variants() {
        for cmd in [
            "rm -rf /",
            "rm -rf /*",
            "rm -rf ~",
            "sudo rm -rf /",
            "\\rm -rf /",
            "rm --no-preserve-root -rf /tmp/x",
        ] {
            assert!(is_deny(cmd), "{cmd}");
        }
    }

    #[test]
    fn blocks_disk_power_forkbomb() {
        for cmd in [
            "mkfs.ext4 /dev/sda1",
            "dd if=x of=/dev/sda",
            "shutdown now",
            "sudo reboot",
            ":(){ :|:& };:",
        ] {
            assert!(is_deny(cmd), "{cmd}");
        }
    }

    #[test]
    fn fork_bomb_signature_needs_function_definition() {
        // Bare prose mentioning the pattern is not a bomb.
        assert!(matches!(classify("echo \":|:&\""), Decision::Allow));
    }

    #[test]
    fn git_destructive_prompts_not_denies() {
        for cmd in [
            "git reset --hard",
            "git clean -fd",
            "git push --force origin main",
        ] {
            assert!(matches!(classify(cmd), Decision::Prompt { .. }), "{cmd}");
        }
    }

    #[test]
    fn blocks_embedded_sh_c_payload() {
        assert!(is_deny("bash -c \"rm -rf /\""));
        assert!(matches!(
            classify("bash -c \"git reset --hard\""),
            Decision::Prompt { .. }
        ));
    }

    #[test]
    fn allows_ordinary_commands() {
        for cmd in [
            "rm -rf ./build",
            "ls /",
            "echo hi",
            "git status",
            "git push --force-with-lease origin main",
            "fdisk -l",
            "git clean -n",
        ] {
            assert!(matches!(classify(cmd), Decision::Allow), "{cmd}");
        }
    }
}
