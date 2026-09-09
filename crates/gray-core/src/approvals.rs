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
//! cache); the process default is `full` (yolo locally; opt into
//! `auto`/`read-only` via `/permissions` — remote surfaces stay deny-by-default).

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
}

impl ApprovalCache {
    pub fn remember_path(&self, path: PathBuf) {
        self.paths.lock().map(|mut g| g.insert(path)).ok();
    }

    pub fn remembered_path(&self, path: &Path) -> bool {
        self.paths.lock().map(|g| g.contains(path)).unwrap_or(false)
    }

    /// Raw command bytes are the cache identity: any normalization risks
    /// approving a different shell program (`printf 'a  b'` vs `printf 'a b'`,
    /// `sh -c` wrappers, newline-joined chains). Different bytes re-ask.
    pub fn remember_command(&self, command: String) {
        self.commands.lock().map(|mut g| g.insert(command)).ok();
    }

    pub fn remembered_command(&self, command: &str) -> bool {
        self.commands
            .lock()
            .map(|g| g.contains(command))
            .unwrap_or(false)
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
            mode: Arc::new(Mutex::new(MODE_FULL.to_string())),
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
            "read" | "ls" | "find" | "glob" | "grep" | "skill" | "request_user_input" => {
                Verdict::Allow
            }
            _ => Verdict::Deny("read-only mode: mutating tools are disabled"),
        },
        _ => match tool {
            "read" | "ls" | "find" | "glob" | "grep" | "skill" | "request_user_input" => {
                Verdict::Allow
            }
            "write" | "edit" => match tool_path(args) {
                Some(p) if path_in_cwd(cwd, &p) => Verdict::Allow,
                Some(_) => Verdict::Ask,
                None => Verdict::Ask,
            },
            "bash" => Verdict::Ask,
            // Fail closed: unknown/sidecar tools prompt, never silent Allow.
            _ => Verdict::Ask,
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
                s.contains("always") || s.contains("YOLO") || s.contains("don't ask again")
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
        // Dead `schedule_task` arm removed: no implementing tool, so it falls
        // through to the fail-closed unknown-tool path (Ask, never Allow).
        assert_eq!(
            verdict("auto", "schedule_task", &json!({}), &cwd()),
            Verdict::Ask
        );
    }

    // UNRUN (cargo test banned under X): run in TTY/CI.
    #[test]
    fn unknown_tools_fail_closed_in_auto() {
        for tool in ["whatever", "sidecar_plugin_tool", "schedule_task", ""] {
            assert_eq!(
                verdict("auto", tool, &json!({}), &cwd()),
                Verdict::Ask,
                "{tool} must prompt, never silent Allow"
            );
        }
    }

    // UNRUN (cargo test banned under X): run in TTY/CI.
    #[tokio::test]
    async fn unknown_tool_denies_without_bridge() {
        let gate = ApprovalGate::new("auto");
        let err = gate
            .check("sidecar_plugin_tool", &json!({}), &cwd(), "x", None)
            .await
            .expect_err("fail-closed without a user");
        assert!(err.contains("declined"), "{err}");
    }

    // UNRUN (cargo test banned under X): run in TTY/CI.
    #[test]
    fn glob_parity_with_find() {
        for mode in ["read-only", "auto", "full"] {
            assert_eq!(
                verdict(mode, "glob", &json!({}), &cwd()),
                Verdict::Allow,
                "glob in {mode}"
            );
        }
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
    fn command_identity_is_raw_bytes() {
        // Whitespace, quoting, and shell wrappers change the program that
        // runs, so they must change the cache identity: only byte-identical
        // commands hit.
        let gate = ApprovalGate::new("auto");
        gate.cache.remember_command("echo hi".to_string());
        assert!(gate.cache.remembered_command("echo hi"));
        assert!(!gate.cache.remembered_command("  echo hi  "));
        assert!(!gate.cache.remembered_command("echo  hi"));
        assert!(!gate.cache.remembered_command("bash -lc 'echo hi'"));
        assert!(!gate.cache.remembered_command("echo hi && echo bye"));
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
    async fn accept_always_records_exact_command_not_prefix() {
        use crate::questions::QuestionBridge;
        // Scripted with the legacy "always" label so the run exercises the
        // AcceptAlways path.
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
        // Same command, different spelling → must re-ask (Err without a
        // user). Raw bytes are the identity now.
        let out2 = gate
            .check(
                "bash",
                &json!({"command": "/bin/bash -lc 'cargo test --lib'"}),
                &cwd(),
                "x",
                None,
            )
            .await;
        assert!(out2.is_err(), "different bytes must re-ask");
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
}
