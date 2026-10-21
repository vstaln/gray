//! Tool consent for interactive sessions.
//!
//! In the interactive TUI the first invocation of each tool pauses for
//! user consent — run once, allow the tool (or the MCP server, or the
//! command) for the session, project or everywhere, or deny, optionally
//! with a reason handed back to the agent. The gate only engages when
//! the ask service was installed with a live TUI; print mode, piped
//! stdin and CI keep the previous auto-allow behavior.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use futures::future::BoxFuture;
use gray_core::agent::{BackgroundJob, ToolContext, ToolExecutor, ToolOutput};
use gray_core::message::ToolDef;

const RUN_ONCE: &str = "Run once";
const DENY: &str = "Deny";
const DENY_WHY: &str = "Deny with a reason";

enum Verdict {
    Once,
    Session,
    Project,
    Global,
    BashCommands(String),
    Deny(Option<String>),
}

pub fn executor_wrap() -> gray_plugin::builder::ExecutorWrap {
    Box::new(|inner| Arc::new(ConsentExecutor::new(inner)))
}

struct ConsentExecutor {
    inner: Arc<dyn ToolExecutor>,
    allowed: &'static tokio::sync::Mutex<HashSet<String>>,
}

fn session_allowed() -> &'static tokio::sync::Mutex<HashSet<String>> {
    static SET: std::sync::OnceLock<tokio::sync::Mutex<HashSet<String>>> =
        std::sync::OnceLock::new();
    SET.get_or_init(Default::default)
}

impl ConsentExecutor {
    fn new(inner: Arc<dyn ToolExecutor>) -> Self {
        Self {
            inner,
            allowed: session_allowed(),
        }
    }
}

/// Consent key and display name for a tool call. MCP tools flatten to
/// `mcp__<server>__<tool>`; consent is keyed on the server so allowing
/// `mcp__treg` covers every tool that server offers.
fn consent_target(name: &str) -> (String, String) {
    if let Some(rest) = name.strip_prefix("mcp__")
        && let Some(server) = rest.split("__").next()
        && !server.is_empty()
    {
        return (format!("mcp__{server}"), format!("{server} tools"));
    }
    (name.to_string(), name.to_string())
}

/// Persisted consent: `~/.gray/tool-consent.json`
/// `{"global": ["mcp__treg", ...], "bash_cmds": ["cargo", ...],
///   "projects": {"/abs/dir": ["bash", ...]}}`.
#[derive(Default)]
struct ToolConsentStore {
    path: PathBuf,
    global: HashSet<String>,
    bash_cmds: HashSet<String>,
    projects: HashMap<String, HashSet<String>>,
}

impl ToolConsentStore {
    fn load() -> Self {
        let mut store = ToolConsentStore {
            path: gray_core::paths::gray_home()
                .map(|h| h.join("tool-consent.json"))
                .unwrap_or_default(),
            ..Default::default()
        };
        let Ok(text) = std::fs::read_to_string(&store.path) else {
            return store;
        };
        let v: serde_json::Value = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(e) => {
                log::warn!(
                    "{}: ignoring corrupt tool-consent file: {e}",
                    store.path.display()
                );
                return store;
            }
        };
        let set = |v: Option<&serde_json::Value>| -> HashSet<String> {
            v.and_then(|a| a.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|s| s.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default()
        };
        store.global = set(v.get("global"));
        store.bash_cmds = set(v.get("bash_cmds"));
        if let Some(projects) = v.get("projects").and_then(|p| p.as_object()) {
            for (dir, keys) in projects {
                store.projects.insert(dir.clone(), set(Some(keys)));
            }
        }
        store
    }

    fn save(&self) {
        if self.path.as_os_str().is_empty() {
            return;
        }
        let body = serde_json::json!({
            "global": self.global,
            "bash_cmds": self.bash_cmds,
            "projects": self.projects,
        });
        if let Some(parent) = self.path.parent()
            && let Err(e) = std::fs::create_dir_all(parent)
        {
            log::warn!("tool-consent: could not create {}: {e}", parent.display());
            return;
        }
        if let Err(e) = std::fs::write(
            &self.path,
            serde_json::to_string_pretty(&body).unwrap_or_default() + "\n",
        ) {
            log::warn!("tool-consent: could not write {}: {e}", self.path.display());
        }
    }

    fn is_allowed(&self, key: &str, cwd: &std::path::Path, bash_word: Option<&str>) -> bool {
        self.global.contains(key)
            || bash_word.is_some_and(|w| self.bash_cmds.contains(w))
            || self
                .projects
                .get(&cwd.to_string_lossy().to_string())
                .is_some_and(|s| s.contains(key))
    }
}

/// First whitespace-separated word of a bash command, when it looks like
/// a plain program name (`cargo`, `./deploy.sh`, `git` — not `FOO=1` or
/// a shell builtin line).
fn bash_word(args: &serde_json::Value) -> Option<String> {
    let cmd = args.get("command")?.as_str()?.trim();
    let word = cmd.split_whitespace().next()?;
    let simple = !word.is_empty()
        && word.len() <= 64
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/' | '+'));
    simple.then(|| word.to_string())
}

fn describe(name: &str, args: &serde_json::Value) -> String {
    let detail = match name {
        "bash" | "shell" => args.get("command"),
        "read" | "write" | "edit" | "ls" => args.get("path").or_else(|| args.get("file_path")),
        "grep" | "find" | "search" => args.get("pattern").or_else(|| args.get("path")),
        "web_search" => args.get("query"),
        "web_fetch" => args.get("url"),
        _ => args
            .get("path")
            .or_else(|| args.get("command"))
            .or_else(|| args.get("query"))
            .or_else(|| args.get("url")),
    }
    .and_then(|v| v.as_str())
    .map(str::trim)
    .filter(|s| !s.is_empty())
    .map(str::to_string)
    .unwrap_or_else(|| serde_json::to_string(args).unwrap_or_default());
    crate::repl::format::truncate_chars(detail.lines().next().unwrap_or(&detail), 400).to_string()
}

async fn ask_consent(name: &str, args: &serde_json::Value, cwd: &std::path::Path) -> Verdict {
    let (_key, disp) = consent_target(name);
    let detail = if name.starts_with("mcp__") {
        format!("{name} {}", describe(name, args))
    } else {
        describe(name, args)
    };
    let word = if matches!(name, "bash" | "shell") {
        bash_word(args)
    } else {
        None
    };
    let session_label = format!("Allow {disp} for this session");
    let project_label = format!("Always allow {disp} in this project");
    let global_label = format!("Always allow {disp}");
    let cmd_label = word
        .as_ref()
        .map(|w| format!("Always allow `{w}` commands"));

    let mut options =
        vec![serde_json::json!({"label": RUN_ONCE, "description": "run this call only"})];
    if let Some(l) = &cmd_label {
        options.push(serde_json::json!({"label": l, "description": "remember for all projects"}));
    }
    options.extend([
        serde_json::json!({"label": session_label, "description": format!("allow {disp} until this session ends")}),
        serde_json::json!({"label": project_label, "description": format!("remember for {}", cwd.display())}),
        serde_json::json!({"label": global_label, "description": "remember for every project"}),
        serde_json::json!({"label": DENY, "description": "refuse this call"}),
        serde_json::json!({"label": DENY_WHY, "description": "refuse and tell the agent why", "freeform": true}),
    ]);

    let question = format!("Allow {disp}?");
    let result = crate::ask::handle_ask(serde_json::json!({
        "questions": [{
            "id": "consent",
            "header": "consent",
            "question": question,
            "detail": detail,
            "options": options,
            "no_other": true,
        }],
        "blocking": true,
    }))
    .await;

    let answers = result
        .pointer("/answers/consent/answers")
        .and_then(|v| v.as_array());
    let picked = answers.and_then(|answers| {
        answers
            .iter()
            .filter_map(|a| a.as_str())
            .find(|a| !a.starts_with("user_note: "))
            .map(str::to_string)
    });
    let note = answers.and_then(|answers| {
        answers
            .iter()
            .filter_map(|a| a.as_str())
            .find_map(|a| a.strip_prefix("user_note: ").map(str::to_string))
    });
    match picked.as_deref() {
        Some(RUN_ONCE) => Verdict::Once,
        Some(l) if Some(l) == cmd_label.as_deref() => {
            Verdict::BashCommands(word.unwrap_or_default())
        }
        Some(l) if l == session_label => Verdict::Session,
        Some(l) if l == project_label => Verdict::Project,
        Some(l) if l == global_label => Verdict::Global,
        Some(DENY_WHY) => Verdict::Deny(note),
        _ => Verdict::Deny(None),
    }
}

impl ToolExecutor for ConsentExecutor {
    fn drain_notifications(&self, ctx: &ToolContext) -> Vec<String> {
        self.inner.drain_notifications(ctx)
    }

    fn live_defs(&self) -> Option<Vec<ToolDef>> {
        self.inner.live_defs()
    }

    fn wait_for_notification(
        &self,
        ctx: &ToolContext,
        timeout: std::time::Duration,
    ) -> BoxFuture<'static, Option<()>> {
        self.inner.wait_for_notification(ctx, timeout)
    }

    fn has_pending_background(&self, ctx: &ToolContext) -> bool {
        self.inner.has_pending_background(ctx)
    }

    fn repin_session(&self, id: &str) {
        self.inner.repin_session(id);
    }

    fn background_jobs(&self, ctx: &ToolContext) -> Vec<BackgroundJob> {
        self.inner.background_jobs(ctx)
    }

    fn running_jobs_note(&self, ctx: &ToolContext) -> Option<String> {
        self.inner.running_jobs_note(ctx)
    }

    fn cancel_background(&self, ctx: &ToolContext, id: &str) -> bool {
        self.inner.cancel_background(ctx, id)
    }

    fn execute(
        &self,
        ctx: &ToolContext,
        name: &str,
        args: serde_json::Value,
    ) -> BoxFuture<'static, ToolOutput> {
        let inner = self.inner.clone();
        let allowed = self.allowed;
        let ctx = ctx.clone();
        let name = name.to_string();
        Box::pin(async move {
            // MCP tools run without a prompt: the server itself was already
            // consented to when it started (user config, or the project
            // `.mcp.json` server prompt).
            if crate::ask::interactive_tui() && !name.starts_with("mcp__") {
                let (key, _disp) = consent_target(&name);
                // The lock is held across the prompt so parallel tool calls
                // queue on it instead of stacking modals.
                let mut allowed = allowed.lock().await;
                let persisted = ToolConsentStore::load();
                let word = bash_word(&args);
                if !allowed.contains(&key) && !persisted.is_allowed(&key, &ctx.cwd, word.as_deref())
                {
                    match ask_consent(&name, &args, &ctx.cwd).await {
                        Verdict::Session => {
                            allowed.insert(key);
                        }
                        Verdict::Project => {
                            let mut store = ToolConsentStore::load();
                            store
                                .projects
                                .entry(ctx.cwd.to_string_lossy().to_string())
                                .or_default()
                                .insert(key);
                            store.save();
                        }
                        Verdict::Global => {
                            let mut store = ToolConsentStore::load();
                            store.global.insert(key);
                            store.save();
                        }
                        Verdict::BashCommands(w) => {
                            if !w.is_empty() {
                                let mut store = ToolConsentStore::load();
                                store.bash_cmds.insert(w);
                                store.save();
                            }
                        }
                        Verdict::Once => {}
                        Verdict::Deny(reason) => {
                            return ToolOutput::error(match reason {
                                Some(r) if !r.is_empty() => {
                                    format!("the user declined {name}: {r}")
                                }
                                _ => format!("the user declined the {name} tool call"),
                            });
                        }
                    }
                }
            }
            inner.execute(&ctx, &name, args).await
        })
    }
}
