use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::sync::OnceCell;

use crate::{InputSubmit, Manifest, Plugin};

pub const ECOSYSTEM: &str = "claude-code";

#[derive(Debug, Clone)]
pub struct ClaudeManifest {
    pub name: String,
    pub version: String,
    pub description: Option<String>,
    pub hooks: Vec<String>,
}

#[derive(Debug, Clone)]
struct HookCmd {
    command: String,
    timeout_secs: u64,
}

pub fn is_claude_plugin(dir: &Path) -> bool {
    dir.join(".claude-plugin").join("plugin.json").is_file()
}

pub fn read_manifest(dir: &Path) -> Option<ClaudeManifest> {
    let path = dir.join(".claude-plugin").join("plugin.json");
    let raw = std::fs::read_to_string(&path).ok()?;
    let v: Value = serde_json::from_str(&raw).ok()?;
    let name = v.get("name").and_then(Value::as_str)?.trim().to_string();
    if name.is_empty() {
        return None;
    }
    let version = v
        .get("version")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("0.0.0")
        .to_string();
    let description = v
        .get("description")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let hooks = match v.get("hooks") {
        Some(Value::String(s)) => vec![s.clone()],
        Some(Value::Array(arr)) => arr
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        _ => vec!["./hooks/hooks.json".to_string()],
    };
    Some(ClaudeManifest {
        name,
        version,
        description,
        hooks,
    })
}

fn confined_rel(dir: &Path, rel: &str) -> Option<PathBuf> {
    let p = Path::new(rel);
    if p.is_absolute() {
        return None;
    }
    let mut clean = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::Normal(seg) => clean.push(seg),
            _ => return None,
        }
    }
    Some(dir.join(clean))
}

fn parse_hook_commands(dir: &Path, files: &[String], event: &str) -> Vec<HookCmd> {
    let mut out = Vec::new();
    for rel in files {
        let Some(path) = confined_rel(dir, rel) else {
            continue;
        };
        let Ok(raw) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<Value>(&raw) else {
            continue;
        };
        let Some(groups) = v
            .pointer(&format!("/hooks/{event}"))
            .and_then(Value::as_array)
        else {
            continue;
        };
        for group in groups {
            let Some(hooks) = group.get("hooks").and_then(Value::as_array) else {
                continue;
            };
            for hook in hooks {
                if hook.get("type").and_then(Value::as_str) != Some("command") {
                    continue;
                }
                let Some(command) = hook
                    .get("command")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                else {
                    continue;
                };
                let timeout_secs = hook
                    .get("timeout")
                    .and_then(Value::as_u64)
                    .unwrap_or(10)
                    .clamp(1, 15);
                out.push(HookCmd {
                    command: command.to_string(),
                    timeout_secs,
                });
            }
        }
    }
    out
}

fn extract_hook_context(stdout: &str) -> Option<String> {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
        for ptr in [
            "/hookSpecificOutput/additionalContext",
            "/additionalContext",
            "/systemMessage",
        ] {
            if let Some(s) = v
                .pointer(ptr)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                return Some(s.to_string());
            }
        }
        return None;
    }
    Some(trimmed.to_string())
}

async fn run_hook_cmd(
    hook: &HookCmd,
    plugin_dir: &Path,
    cwd: Option<&str>,
    stdin_payload: &str,
) -> Option<String> {
    let mut cmd = if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.args(["/C", &hook.command]);
        c
    } else {
        let mut c = Command::new("sh");
        c.args(["-c", &hook.command]);
        c
    };
    cmd.env("CLAUDE_PLUGIN_ROOT", plugin_dir)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    if let Some(dir) = cwd.filter(|d| !d.is_empty()) {
        cmd.current_dir(dir);
    }
    let mut child = cmd.spawn().ok()?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(stdin_payload.as_bytes()).await;
        let _ = stdin.shutdown().await;
    }
    let output = tokio::time::timeout(
        Duration::from_secs(hook.timeout_secs),
        child.wait_with_output(),
    )
    .await
    .ok()?
    .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    extract_hook_context(&stdout)
}

pub struct ClaudeCodePlugin {
    dir: PathBuf,
    manifest: Manifest,
    session_start_cmds: Vec<HookCmd>,
    user_prompt_cmds: Vec<HookCmd>,
    session_context: OnceCell<Option<String>>,
    global: AtomicBool,
}

impl ClaudeCodePlugin {
    pub fn load(name: &str, dir: &Path) -> anyhow::Result<Self> {
        let cm = read_manifest(dir).ok_or_else(|| {
            anyhow::anyhow!("missing .claude-plugin/plugin.json in {}", dir.display())
        })?;
        let session_start_cmds = parse_hook_commands(dir, &cm.hooks, "SessionStart");
        let user_prompt_cmds = parse_hook_commands(dir, &cm.hooks, "UserPromptSubmit");
        let mut hooks = Vec::new();
        if !session_start_cmds.is_empty() {
            hooks.push("session_start".to_string());
        }
        if !user_prompt_cmds.is_empty() {
            hooks.push("input_submit".to_string());
        }
        let manifest = Manifest {
            name: name.to_string(),
            version: cm.version,
            hooks,
            ..Manifest::default()
        };
        Ok(Self {
            dir: dir.to_path_buf(),
            manifest,
            session_start_cmds,
            user_prompt_cmds,
            session_context: OnceCell::new(),
            global: AtomicBool::new(false),
        })
    }
}

#[async_trait]
impl Plugin for ClaudeCodePlugin {
    fn manifest(&self) -> Manifest {
        self.manifest.clone()
    }

    async fn prompt_context(&self, cwd: &str) -> Option<String> {
        if self.session_start_cmds.is_empty() {
            return None;
        }
        self.session_context
            .get_or_init(|| async {
                let payload = serde_json::json!({
                    "source": "startup",
                    "cwd": cwd,
                })
                .to_string();
                let mut parts = Vec::new();
                for hook in &self.session_start_cmds {
                    if let Some(ctx) = run_hook_cmd(hook, &self.dir, Some(cwd), &payload).await {
                        parts.push(ctx);
                    }
                }
                if parts.is_empty() {
                    None
                } else {
                    Some(parts.join("\n\n"))
                }
            })
            .await
            .clone()
    }

    async fn input_submit(&self, text: &str) -> InputSubmit {
        if self.user_prompt_cmds.is_empty() {
            return InputSubmit::Pass;
        }
        let payload = serde_json::json!({ "prompt": text }).to_string();
        for hook in &self.user_prompt_cmds {
            let _ = run_hook_cmd(hook, &self.dir, None, &payload).await;
        }
        InputSubmit::Pass
    }

    fn is_global(&self) -> bool {
        self.global.load(Ordering::Relaxed)
    }

    fn set_global(&self, global: bool) {
        self.global.store(global, Ordering::Relaxed);
    }
}
