//! Tool approval gate: codex parity (`AskForApproval` + approval presets).
//!
//! Three modes (see `reference/openai/codex/codex-rs/utils/approval-presets`):
//!
//! | gray mode   | codex preset | approval policy                    |
//! |-------------|--------------|--------------------------------------|
//! | `read-only` | Read Only    | everything mutating is denied      |
//! | `auto`      | Ask for approval (Default) | workspace writes free, the rest asks |
//! | `full`      | Full Access  | never asks                          |
//!
//! Per-call decisions mirror codex's `CommandExecutionApprovalDecision` /
//! `FileChangeApprovalDecision` subset that survives without a sandbox or an
//! execpolicy engine: **Accept** (run once), **AcceptForSession** (remember
//! this call this session), **Decline** (skip, the turn continues), **Cancel**
//! (skip and stop listening — Esc always cancels, like codex).
//! "Always" remembers the FULL canonical command for the rest of the session —
//! never a command prefix (a prefix allow would bless untested siblings) and
//! never a global mode flip.
//!
//! The gate lives in gray-core so both the interactive REPL and headless
//! surfaces (gateway daemon, print mode) enforce the same policy. Session
//! memory is a per-gate [`ApprovalCache`] (codex's session-scoped approval
//! cache); the process default is `auto`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::error::CoreError;
use crate::questions::{QuestionBridge, UserOption, UserQuestion};

/// Codex approval preset ids (`:read-only`, `:workspace`,
/// `:danger-full-access`), reused as gray's on-disk mode strings.
pub const MODE_READ_ONLY: &str = "read-only";
pub const MODE_AUTO: &str = "auto";
pub const MODE_FULL: &str = "full";

/// Per-call verdict the gate can return without asking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Allow,
    Deny(&'static str),
    Ask,
}

/// User decision on an asked call (codex decision subset).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Accept,
    AcceptForSession,
    AcceptAlways,
    Decline,
    Cancel,
}

/// Session-scoped memory of accepted calls (codex `with_cached_approval`).
#[derive(Debug, Default)]
pub struct ApprovalCache {
    paths: Mutex<HashSet<PathBuf>>,
    commands: Mutex<HashSet<String>>,
    prefixes: Mutex<HashSet<String>>,
}

impl ApprovalCache {
    pub fn remember_path(&self, path: PathBuf) {
        self.paths.lock().map(|mut g| g.insert(path)).ok();
    }

    pub fn remembered_path(&self, path: &Path) -> bool {
        self.paths.lock().map(|g| g.contains(path)).unwrap_or(false)
    }

    pub fn remember_command(&self, command: String) {
        self.commands
            .lock()
            .map(|mut g| g.insert(canonicalize_command(&command)))
            .ok();
    }

    pub fn remembered_command(&self, command: &str) -> bool {
        self.commands
            .lock()
            .map(|g| g.contains(&canonicalize_command(command)))
            .unwrap_or(false)
    }

    pub fn remember_prefix(&self, prefix: String) {
        self.prefixes
            .lock()
            .map(|mut g| g.insert(canonicalize_command(&prefix)))
            .ok();
    }

    pub fn matched_prefix(&self, command: &str) -> bool {
        self.prefixes
            .lock()
            .map(|g| g.contains(&command_prefix(&canonicalize_command(command))))
            .unwrap_or(false)
    }
}

/// Canonical command identity for approval-cache keys (NOT for execution —
/// the raw string always runs). Trim → split top-level chains → collapse
/// whitespace → strip one `sh -c`/`bash -lc` wrapper layer per segment →
/// rejoin operators in canonical spaced form (`;` becomes `\n`).
///
/// Fail-safe direction: any uncertainty returns a *more specific* key
/// (usually the whole input), which degrades to a cache miss = re-ask.
/// A canonicalizer can never widen an allow.
pub fn canonicalize_command(cmd: &str) -> String {
    let t = cmd.trim();
    if t.is_empty() {
        return String::from("(empty)");
    }
    let (segs, seps) = split_top_level(t);
    let mut out = String::new();
    for (i, seg) in segs.iter().enumerate() {
        if i > 0 {
            out.push_str(seps[i - 1]);
        }
        out.push_str(&unwrap_shell_wrapper(&collapse_ws(seg)));
    }
    if out.trim().is_empty() {
        t.to_string()
    } else {
        out
    }
}

/// Minimal top-level splitter for cache keys only: splits on `&&`, `||`,
/// `;`, `|` outside single/double quotes, returning `(segments, separators)`
/// where `separators[i]` is the canonical form of the operator between
/// `segments[i]` and `segments[i + 1]` (`" && "`, `" || "`, `" | "`, or
/// `"\n"` for `;`). A lone `&` (background) is not a separator — it stays
/// verbatim in its segment (safe miss, never false allow). Subshells/
/// heredocs are NOT understood (kept verbatim in their segment — safe miss,
/// never false allow). No escape processing beyond backslash-skip inside
/// double quotes (documented limit; worst case is a safe miss).
fn split_top_level(cmd: &str) -> (Vec<String>, Vec<&'static str>) {
    let mut segs = Vec::new();
    let mut seps = Vec::new();
    let mut cur = String::new();
    let chars: Vec<char> = cmd.chars().collect();
    let mut i = 0;
    let mut quote: Option<char> = None;
    while i < chars.len() {
        let c = chars[i];
        if let Some(q) = quote {
            cur.push(c);
            if q == '"' && c == '\\' && i + 1 < chars.len() {
                cur.push(chars[i + 1]);
                i += 1;
            } else if c == q {
                quote = None;
            }
            i += 1;
            continue;
        }
        match c {
            '\'' | '"' => {
                quote = Some(c);
                cur.push(c);
                i += 1;
            }
            '&' if i + 1 < chars.len() && chars[i + 1] == '&' => {
                segs.push(std::mem::take(&mut cur));
                seps.push(" && ");
                i += 2;
            }
            '|' if i + 1 < chars.len() && chars[i + 1] == '|' => {
                segs.push(std::mem::take(&mut cur));
                seps.push(" || ");
                i += 2;
            }
            ';' => {
                segs.push(std::mem::take(&mut cur));
                seps.push("\n");
                i += 1;
            }
            '|' => {
                segs.push(std::mem::take(&mut cur));
                seps.push(" | ");
                i += 1;
            }
            _ => {
                cur.push(c);
                i += 1;
            }
        }
    }
    segs.push(cur);
    (segs, seps)
}

/// Collapse every whitespace run to one space.
fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Strip one `sh -c` / `bash -lc` / `/bin/sh -c` wrapper layer. Returns the
/// input unchanged unless it is exactly `shell [-lc]+ <single-quoted|double-quoted|bare-tail>`.
fn unwrap_shell_wrapper(seg: &str) -> String {
    // Quote-aware split into (shell, flags, rest); the rest is the verbatim
    // remainder after the flags (covers the bare-tail form `sh -c echo hi`).
    // No escape processing beyond backslash-skip inside double quotes
    // (documented limit; worst case is a safe miss).
    let mut tokens: Vec<&str> = Vec::new();
    let mut rest_start = seg.len();
    let mut tok_start: Option<usize> = None;
    let mut quote: Option<char> = None;
    let mut prev_was_backslash = false;
    for (idx, c) in seg.char_indices() {
        if tokens.len() == 2 {
            rest_start = idx;
            break;
        }
        if let Some(q) = quote {
            if q == '"' && c == '\\' {
                prev_was_backslash = !prev_was_backslash;
            } else {
                if c == q && !prev_was_backslash {
                    quote = None;
                }
                prev_was_backslash = false;
            }
            continue;
        }
        match c {
            '\'' | '"' => {
                quote = Some(c);
                if tok_start.is_none() {
                    tok_start = Some(idx);
                }
            }
            c if c.is_whitespace() => {
                if let Some(s) = tok_start {
                    tokens.push(&seg[s..idx]);
                    tok_start = None;
                }
            }
            _ => {
                if tok_start.is_none() {
                    tok_start = Some(idx);
                }
            }
        }
    }
    if tokens.len() < 2
        && let Some(s) = tok_start
    {
        tokens.push(&seg[s..]);
    }
    if tokens.len() != 2 {
        return seg.to_string();
    }
    let rest = seg[rest_start..].trim_start();
    if rest.is_empty() {
        return seg.to_string();
    }
    let mut shell = tokens[0];
    if let Some(s) = shell.strip_prefix("/bin/") {
        shell = s;
    }
    if shell != "sh" && shell != "bash" {
        return seg.to_string();
    }
    let flags = tokens[1];
    if flags.is_empty()
        || !flags.contains('c')
        || !flags.chars().all(|c| c == 'l' || c == 'c' || c == '-')
    {
        return seg.to_string();
    }
    // Strip ONE layer of surrounding matching quotes; unbalanced quotes
    // return the input unchanged.
    let b = rest.as_bytes();
    if rest.len() >= 2 && (b[0] == b'\'' || b[0] == b'"') && b[0] == b[rest.len() - 1] {
        return rest[1..rest.len() - 1].to_string();
    }
    if rest.starts_with('\'') || rest.starts_with('"') {
        return seg.to_string();
    }
    rest.to_string()
}

/// Session prefix rule identity: first two whitespace tokens (the binary +
/// subcommand, e.g. `cargo test`). Single-token commands are their own prefix.
pub fn command_prefix(canonical: &str) -> String {
    let mut it = canonical.split_whitespace();
    match (it.next(), it.next()) {
        (Some(a), Some(b)) => format!("{a} {b}"),
        (Some(a), None) => a.to_string(),
        _ => canonical.to_string(),
    }
}

/// Approval mode + session memory. `Clone` shares both (the mode cell and
/// the cache) across turn contexts.
#[derive(Debug, Clone)]
pub struct ApprovalGate {
    mode: Arc<Mutex<String>>,
    cache: Arc<ApprovalCache>,
}

impl Default for ApprovalGate {
    fn default() -> Self {
        Self {
            mode: Arc::new(Mutex::new(MODE_AUTO.to_string())),
            cache: Arc::new(ApprovalCache::default()),
        }
    }
}

impl ApprovalGate {
    pub fn new(mode: &str) -> Self {
        let gate = Self::default();
        gate.set_mode(mode);
        gate
    }

    pub fn mode(&self) -> String {
        self.mode
            .lock()
            .map(|g| g.clone())
            .unwrap_or_else(|e| e.into_inner().clone())
    }

    /// Sets the mode; unknown values fall back to `auto` (fail safe).
    pub fn set_mode(&self, mode: &str) {
        let clean = normalize_mode(mode).unwrap_or(MODE_AUTO);
        if let Ok(mut g) = self.mode.lock() {
            *g = clean.to_string();
        }
    }
}

/// Parses a mode string (`read-only`, `auto`, `full`); `None` when unknown.
pub fn normalize_mode(s: &str) -> Option<&'static str> {
    match s.trim().to_ascii_lowercase().replace(' ', "-").as_str() {
        "read-only" | "readonly" | "read_only" | "ro" => Some(MODE_READ_ONLY),
        "auto" | "ask" | "default" | "ask-for-approval" | "workspace" => Some(MODE_AUTO),
        "full" | "full-access" | "full_access" | "never" | "yolo" => Some(MODE_FULL),
        _ => None,
    }
}

/// One row of the `/permissions` picker: (mode id, label, description),
/// mirroring codex's preset label + description copy.
pub fn permission_modes() -> Vec<(&'static str, &'static str, &'static str)> {
    vec![
        (
            MODE_AUTO,
            "Ask for approval",
            "Read and edit files in the workspace, run commands. Approval is required to access the internet or edit other files.",
        ),
        (
            MODE_FULL,
            "Full Access",
            "Edit files outside the workspace and access the internet without asking for approval. Exercise caution.",
        ),
        (
            MODE_READ_ONLY,
            "Read Only",
            "Read files in the workspace. Approval is required to edit files or access the internet.",
        ),
    ]
}

/// Extracts the target path from tool args (same alias set the tools use).
pub fn tool_path(args: &serde_json::Value) -> Option<String> {
    args.get("path")
        .or_else(|| args.get("file_path"))
        .or_else(|| args.get("filePath"))
        .or_else(|| args.get("file"))
        .or_else(|| args.get("filename"))
        .or_else(|| args.get("target"))
        .or_else(|| args.get("destination"))
        .or_else(|| args.get("target_file"))
        .or_else(|| args.get("targetFile"))
        .or_else(|| args.get("TargetFile"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

/// True when `path` resolves inside `cwd` (lexically; symlinks are resolved
/// below when the file exists).
pub fn path_in_cwd(cwd: &Path, path: &str) -> bool {
    let joined = if Path::new(path).is_absolute() {
        PathBuf::from(path)
    } else {
        cwd.join(path)
    };
    let canon = |p: PathBuf| {
        std::fs::canonicalize(&p).unwrap_or_else(|_| {
            let mut out = PathBuf::new();
            for comp in p.components() {
                match comp {
                    std::path::Component::ParentDir => {
                        out.pop();
                    }
                    std::path::Component::CurDir => {}
                    c => out.push(c.as_os_str()),
                }
            }
            out
        })
    };
    canon(joined).starts_with(canon(cwd.to_path_buf()))
}

/// Static verdict for a tool call: no asking, no session memory yet.
pub fn verdict(mode: &str, tool: &str, args: &serde_json::Value, cwd: &Path) -> Verdict {
    match normalize_mode(mode).unwrap_or(MODE_AUTO) {
        MODE_FULL => Verdict::Allow,
        MODE_READ_ONLY => match tool {
            "read" | "ls" | "find" | "grep" | "skill" | "request_user_input" => Verdict::Allow,
            _ => Verdict::Deny("read-only mode: mutating tools are disabled"),
        },
        _ => match tool {
            "read" | "ls" | "find" | "grep" | "skill" | "request_user_input" | "schedule_task" => {
                Verdict::Allow
            }
            "write" | "edit" => match tool_path(args) {
                Some(p) if path_in_cwd(cwd, &p) => Verdict::Allow,
                Some(_) => Verdict::Ask,
                None => Verdict::Ask,
            },
            "bash" => Verdict::Ask,
            _ => Verdict::Allow,
        },
    }
}

impl ApprovalGate {
    /// Full gate check: static verdict, then session memory, then ask.
    /// `label` is the human summary shown in the prompt (command or path).
    pub async fn check(
        &self,
        tool: &str,
        args: &serde_json::Value,
        cwd: &Path,
        label: &str,
        questions: Option<&QuestionBridge>,
    ) -> Result<(), String> {
        match verdict(&self.mode(), tool, args, cwd) {
            Verdict::Allow => Ok(()),
            Verdict::Deny(msg) => Err(msg.to_string()),
            Verdict::Ask => {
                if (tool == "write" || tool == "edit")
                    && let Some(p) = tool_path(args)
                {
                    let full = if Path::new(&p).is_absolute() {
                        PathBuf::from(&p)
                    } else {
                        cwd.join(&p)
                    };
                    if self.cache.remembered_path(&full) {
                        return Ok(());
                    }
                }
                if tool == "bash"
                    && let Some(cmd) = args.get("command").and_then(|v| v.as_str())
                    && self.cache.remembered_command(cmd)
                {
                    return Ok(());
                }
                if tool == "bash"
                    && let Some(cmd) = args.get("command").and_then(|v| v.as_str())
                    && self.cache.matched_prefix(cmd)
                {
                    return Ok(());
                }
                match ask_user(tool, label, questions).await {
                    Decision::Accept => Ok(()),
                    Decision::AcceptForSession => {
                        self.remember(tool, args, cwd);
                        Ok(())
                    }
                    Decision::AcceptAlways => {
                        // Bash "always" remembers the FULL canonical command
                        // for this session. Never the 2-token prefix: a prefix
                        // allow lets `cargo test --lib` bless `cargo rm -rf`
                        // siblings. Non-bash AcceptAlways keeps the existing
                        // session-scoped `remember()` path (paths); neither
                        // flips global mode.
                        if tool == "bash"
                            && let Some(cmd) = args.get("command").and_then(|v| v.as_str())
                        {
                            self.cache.remember_command(cmd.to_string());
                        } else {
                            self.remember(tool, args, cwd);
                        }
                        Ok(())
                    }
                    Decision::Decline => Err(format!(
                        "{tool} declined by user — do NOT re-attempt via write/edit/bash workarounds; ask the user instead"
                    )),
                    Decision::Cancel => Err(format!("{tool} cancelled by user")),
                }
            }
        }
    }

    fn remember(&self, tool: &str, args: &serde_json::Value, cwd: &Path) {
        if (tool == "write" || tool == "edit")
            && let Some(p) = tool_path(args)
        {
            let full = if Path::new(&p).is_absolute() {
                PathBuf::from(&p)
            } else {
                cwd.join(&p)
            };
            self.cache.remember_path(full);
        }
        if tool == "bash"
            && let Some(cmd) = args.get("command").and_then(|v| v.as_str())
        {
            self.cache.remember_command(cmd.to_string());
        }
    }
}

/// Asks the user once via the question bridge. Fail-closed: no bridge,
/// cancel, error, or anything but an explicit accept denies (codex: Esc
/// always cancels).
pub async fn ask_user(tool: &str, label: &str, questions: Option<&QuestionBridge>) -> Decision {
    let Some(bridge) = questions else {
        return Decision::Decline;
    };
    let preview: String = label.chars().take(160).collect();
    let q = UserQuestion {
        id: "tool-approval".to_string(),
        header: "Allow?".to_string(),
        question: format!("Allow {tool}?\n{preview}"),
        options: vec![
            UserOption {
                label: "Yes (Recommended)".to_string(),
                description: "Run this once.".to_string(),
            },
            UserOption {
                label: "Yes, for session".to_string(),
                description: "Run this and remember for the rest of the session.".to_string(),
            },
            UserOption {
                label: "Yes, for this prefix".to_string(),
                description: "Remember this command prefix for the rest of the session."
                    .to_string(),
            },
            UserOption {
                label: "No".to_string(),
                description: "Skip it; the turn continues.".to_string(),
            },
        ],
        is_other: false,
    };
    match bridge.0.ask(vec![q], true).await {
        Ok(answers) => {
            let picked: Vec<&str> = answers
                .iter()
                .flat_map(|a| a.answers.iter().map(String::as_str))
                .collect();
            if picked.iter().any(|s| {
                s.contains("for this prefix")
                    || s.contains("always")
                    || s.contains("YOLO")
                    || s.contains("don't ask again")
            }) {
                Decision::AcceptAlways
            } else if picked.contains(&"Yes, for session") {
                Decision::AcceptForSession
            } else if picked.iter().any(|s| s.starts_with("Yes")) {
                Decision::Accept
            } else if picked.contains(&"No") {
                Decision::Decline
            } else {
                Decision::Cancel
            }
        }
        Err(CoreError::Cancelled) => Decision::Cancel,
        Err(_) => Decision::Decline,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn cwd() -> PathBuf {
        PathBuf::from("/work/proj")
    }

    #[test]
    fn normalize_accepts_aliases_and_rejects_unknown() {
        assert_eq!(normalize_mode("read-only"), Some(MODE_READ_ONLY));
        assert_eq!(normalize_mode("RO"), Some(MODE_READ_ONLY));
        assert_eq!(normalize_mode("auto"), Some(MODE_AUTO));
        assert_eq!(normalize_mode("Full Access"), Some(MODE_FULL));
        assert_eq!(normalize_mode("yolo"), Some(MODE_FULL));
        assert_eq!(normalize_mode("bogus"), None);
    }

    #[test]
    fn full_allows_everything() {
        for tool in ["read", "write", "edit", "bash", "schedule_task", "whatever"] {
            assert_eq!(
                verdict("full", tool, &json!({}), &cwd()),
                Verdict::Allow,
                "{tool}"
            );
        }
    }

    #[test]
    fn read_only_denies_mutations_but_allows_reads() {
        for tool in ["read", "ls", "find", "grep", "skill", "request_user_input"] {
            assert_eq!(
                verdict("read-only", tool, &json!({}), &cwd()),
                Verdict::Allow,
                "{tool}"
            );
        }
        for tool in ["write", "edit", "bash", "schedule_task"] {
            assert!(
                matches!(
                    verdict("read-only", tool, &json!({}), &cwd()),
                    Verdict::Deny(_)
                ),
                "{tool}"
            );
        }
    }

    #[test]
    fn auto_verdict_matrix() {
        assert_eq!(verdict("auto", "read", &json!({}), &cwd()), Verdict::Allow);
        assert_eq!(
            verdict("auto", "write", &json!({"path": "src/a.rs"}), &cwd()),
            Verdict::Allow
        );
        assert_eq!(
            verdict("auto", "edit", &json!({"path": "/work/proj/b.rs"}), &cwd()),
            Verdict::Allow
        );
        assert_eq!(
            verdict("auto", "write", &json!({"path": "/etc/passwd"}), &cwd()),
            Verdict::Ask
        );
        assert_eq!(
            verdict("auto", "write", &json!({"path": "../outside.txt"}), &cwd()),
            Verdict::Ask
        );
        assert_eq!(verdict("auto", "write", &json!({}), &cwd()), Verdict::Ask);
        assert_eq!(
            verdict("auto", "bash", &json!({"command": "ls"}), &cwd()),
            Verdict::Ask
        );
        assert_eq!(verdict("auto", "skill", &json!({}), &cwd()), Verdict::Allow);
        assert_eq!(
            verdict("auto", "schedule_task", &json!({}), &cwd()),
            Verdict::Allow
        );
    }

    #[test]
    fn path_aliases_resolve_inside_cwd() {
        assert_eq!(
            verdict("auto", "edit", &json!({"file_path": "x.rs"}), &cwd()),
            Verdict::Allow
        );
        assert_eq!(
            verdict("auto", "write", &json!({"target": "/etc/x"}), &cwd()),
            Verdict::Ask
        );
    }

    #[test]
    fn gate_caches_session_accepts() {
        let gate = ApprovalGate::new("auto");
        gate.cache.remember_path(PathBuf::from("/etc/x"));
        gate.cache.remember_command("rm -rf /tmp/y".to_string());
        assert!(gate.cache.remembered_path(Path::new("/etc/x")));
        assert!(gate.cache.remembered_command("rm -rf /tmp/y"));
        assert!(!gate.cache.remembered_command("other"));
    }

    #[test]
    fn unknown_mode_falls_back_to_auto() {
        let gate = ApprovalGate::new("bogus");
        assert_eq!(gate.mode(), MODE_AUTO);
        assert_eq!(
            verdict(&gate.mode(), "read", &json!({}), &cwd()),
            Verdict::Allow
        );
    }

    #[test]
    fn canonical_spellings_collide() {
        assert_eq!(canonicalize_command("/bin/bash -lc 'echo hi'"), "echo hi");
        assert_eq!(canonicalize_command("bash  -lc  \"echo hi\""), "echo hi");
        assert_eq!(canonicalize_command("  echo hi  "), "echo hi");
        assert_eq!(
            canonicalize_command("sh -c 'echo a  &&  echo b'"),
            "echo a && echo b"
        );
        assert_eq!(canonicalize_command("echo a && echo b"), "echo a && echo b");
    }
    #[test]
    fn canonical_never_empties_and_never_panics() {
        for cmd in [
            "",
            "   ",
            "sh -c",
            "bash -lc '",
            "echo 'unbalanced",
            "sudo\nrm -rf /",
        ] {
            let c = canonicalize_command(cmd);
            assert!(!c.is_empty(), "{cmd:?} must degrade to *something* askable");
        }
    }
    #[test]
    fn prefix_takes_two_tokens() {
        assert_eq!(command_prefix("cargo test --lib"), "cargo test");
        assert_eq!(command_prefix("ls"), "ls");
        assert_eq!(command_prefix("git commit -m x"), "git commit");
    }
    #[test]
    fn session_cache_hits_across_spellings() {
        let gate = ApprovalGate::new("auto");
        gate.cache
            .remember_command("bash -lc 'echo hi'".to_string());
        assert!(gate.cache.remembered_command("/bin/bash -lc \"echo hi\""));
        assert!(!gate.cache.remembered_command("echo bye"));
    }
    #[tokio::test]
    async fn no_bridge_denies_without_asking() {
        let gate = ApprovalGate::new("auto");
        let err = gate
            .check("bash", &json!({"command": "ls"}), &cwd(), "ls", None)
            .await
            .expect_err("fail-closed without a user");
        assert!(err.contains("declined"), "{err}");
        let err = gate
            .check("write", &json!({"path": "/etc/x"}), &cwd(), "/etc/x", None)
            .await
            .expect_err("fail-closed without a user");
        assert!(err.contains("declined"), "{err}");
    }

    #[tokio::test]
    async fn decline_message_forbids_workarounds() {
        let gate = ApprovalGate::new("auto");
        let err = gate
            .check("bash", &json!({"command": "ls"}), &cwd(), "ls", None)
            .await
            .expect_err("fail-closed without a user");
        assert!(err.contains("declined by user"), "{err}");
        assert!(err.contains("do NOT re-attempt"), "{err}");
        assert!(err.contains("write/edit/bash"), "{err}");
        assert!(err.contains("ask the user instead"), "{err}");
    }

    #[tokio::test]
    async fn accept_always_records_full_command_not_prefix() {
        use crate::questions::QuestionBridge;
        // Scripted with the legacy "always" label so the run exercises the
        // AcceptAlways path (which used to cache a 2-token prefix).
        let bridge = QuestionBridge::scripted(vec!["Yes, always (don't ask again)".to_string()]);
        let gate = ApprovalGate::new("auto");
        let out = gate
            .check(
                "bash",
                &json!({"command": "cargo test --lib"}),
                &cwd(),
                "cargo test --lib",
                Some(&bridge),
            )
            .await;
        assert!(out.is_ok());
        assert_eq!(
            gate.mode(),
            "auto",
            "AcceptAlways must NOT flip global mode anymore"
        );
        // Same command, different spelling, no bridge → Ok via full-command rule:
        let out2 = gate
            .check(
                "bash",
                &json!({"command": "/bin/bash -lc 'cargo test --lib'"}),
                &cwd(),
                "x",
                None,
            )
            .await;
        assert!(out2.is_ok(), "same full command, canonicalized");
        // Same 2-token prefix but DIFFERENT args → must re-ask (Err without
        // a user). A prefix allow would let `cargo test --lib` bless
        // `cargo publish --dry-run`-style siblings.
        let out3 = gate
            .check(
                "bash",
                &json!({"command": "cargo test --doc"}),
                &cwd(),
                "x",
                None,
            )
            .await;
        assert!(out3.is_err(), "different args must not ride a prefix allow");
        // Unrelated command → still asks (Err without a user):
        let out4 = gate
            .check(
                "bash",
                &json!({"command": "rm -rf /tmp/x"}),
                &cwd(),
                "x",
                None,
            )
            .await;
        assert!(out4.is_err());
    }

    #[tokio::test]
    async fn ask_user_maps_prefix_label_to_accept_always() {
        use crate::questions::QuestionBridge;
        let bridge = QuestionBridge::scripted(vec!["Yes, for this prefix".to_string()]);
        assert_eq!(
            ask_user("bash", "cargo test", Some(&bridge)).await,
            Decision::AcceptAlways
        );
    }
}
