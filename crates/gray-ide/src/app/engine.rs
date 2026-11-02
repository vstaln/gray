//! The `gray --stdio` engine behind a thread: lazy spawn + bootstrap
//! (resume → set_model → list_commands → status), request helpers, and the
//! protocol-row → timeline translation driven from `poll`.
//!
//! One `Eng` per IDE thread (`engines[tid]`); turns themselves live in
//! `self.turns` like the external-CLI providers, just without a per-turn
//! channel — every row arrives on the engine's `rx`.

use super::*;
use crate::engine::{Engine, EngineMsg};

#[derive(Clone, Debug)]
pub(super) struct GrayCmd {
    pub(super) name: String,
    pub(super) description: String,
    pub(super) aliases: Vec<String>,
    #[allow(dead_code)]
    pub(super) surface: String,
    pub(super) source: String,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum ReqKind {
    Prompt,
    Command,
    Interrupt,
    Approve,
    Status,
    Commands,
    #[allow(dead_code)]
    Models,
    Sessions,
    Resume,
    SetModel,
    #[allow(dead_code)]
    Shutdown,
}

/// One question inside an `ask` event (the approval card's payload).
#[derive(Clone, Debug)]
pub(super) struct AskQuestion {
    pub(super) id: String,
    pub(super) header: String,
    pub(super) question: String,
    pub(super) detail: String,
    pub(super) options: Vec<(String, String)>, // (label, description)
}

/// Pending `ask`: which thread/engine it belongs to plus the user's picks so
/// far (question id → picked option label).
pub(super) struct PendingAsk {
    pub(super) tid: String,
    pub(super) ask_id: u64,
    pub(super) questions: Vec<AskQuestion>,
    pub(super) picks: HashMap<String, String>,
}

/// Live state of one thread's `gray --stdio` process.
pub(super) struct Eng {
    pub(super) engine: Engine,
    #[allow(dead_code)]
    pub(super) hello: Option<serde_json::Value>,
    pub(super) session_id: Option<String>,
    pub(super) status: Option<serde_json::Value>,
    pub(super) last_usage: Option<serde_json::Value>,
    pub(super) commands: Vec<GrayCmd>,
    pub(super) pending: HashMap<u64, ReqKind>,
    pub(super) turn_active: bool,
    pub(super) status_at: Instant,
}

impl Eng {
    fn send(&mut self, v: serde_json::Value, kind: ReqKind) -> Result<u64, String> {
        let id = self.engine.request(&v)?;
        self.pending.insert(id, kind);
        Ok(id)
    }
}

/// Gray's builtin slash names (from repl::commands::REGISTRY, incl. aliases).
/// Used to route `/x` to the engine before its `list_commands` reply lands —
/// gray wins collisions with IDE-local commands.
pub(super) const GRAY_BUILTINS: &[&str] = &[
    "models", "model", "connect", "provider", "context", "sessions", "resume", "new", "clear",
    "reset", "compact", "compress", "undo", "retry", "update", "upgrade", "restart", "reboot",
    "jobs", "reload", "agentsmd", "sys", "skills", "skill", "plugins", "hehe", "help", "quit",
    "exit",
];

fn s(v: &serde_json::Value, key: &str) -> String {
    v.get(key)
        .and_then(|x| x.as_str())
        .unwrap_or_default()
        .to_string()
}

impl GrayApp {
    /// Lazily spawn (or respawn) the thread's engine and run the bootstrap
    /// sequence. False when the thread can't have one (legacy provider, no
    /// root) or the spawn failed.
    pub(super) fn ensure_engine(&mut self, tid: &str) -> bool {
        if let Some(e) = self.engines.get(tid) {
            if !e.engine.is_dead() {
                return true;
            }
            self.engines.remove(tid);
        }
        let Some(thread) = self.store.state.thread(tid).cloned() else {
            return false;
        };
        if !provider::is_gray_provider(&thread.provider) {
            return false;
        }
        let Some(root) = self.thread_root(tid) else {
            return false;
        };
        match Engine::spawn(&root, &thread.provider, &thread.model, &thread.effort) {
            Ok(engine) => {
                let mut eng = Eng {
                    engine,
                    hello: None,
                    session_id: thread.session.as_ref().map(|(_, sid)| sid.clone()),
                    status: None,
                    last_usage: None,
                    commands: Vec::new(),
                    pending: HashMap::new(),
                    turn_active: false,
                    status_at: Instant::now(),
                };
                // Resume the thread's saved gray session first so history and
                // model adoption come back, then re-assert the thread's
                // configured provider/model/effort on top.
                if let Some((_, sid)) = thread
                    .session
                    .as_ref()
                    .filter(|(p, _)| provider::is_gray_provider(p))
                {
                    let _ = eng.send(
                        serde_json::json!({"type": "resume", "session": sid}),
                        ReqKind::Resume,
                    );
                }
                let mut sm = serde_json::Map::new();
                sm.insert("type".into(), serde_json::json!("set_model"));
                if thread.provider != "gray" {
                    sm.insert("provider".into(), serde_json::json!(thread.provider));
                }
                if !thread.model.is_empty() && thread.model != "default" {
                    sm.insert("model".into(), serde_json::json!(thread.model));
                }
                if provider::EFFORT_LEVELS.contains(&thread.effort.as_str()) {
                    sm.insert("effort".into(), serde_json::json!(thread.effort));
                }
                if sm.len() > 1 {
                    let _ = eng.send(serde_json::Value::Object(sm), ReqKind::SetModel);
                }
                let _ = eng.send(
                    serde_json::json!({"type": "list_commands"}),
                    ReqKind::Commands,
                );
                let _ = eng.send(serde_json::json!({"type": "status"}), ReqKind::Status);
                self.engines.insert(tid.to_string(), eng);
                true
            }
            Err(e) => {
                self.toast(format!("Couldn't start gray: {e}"), true);
                false
            }
        }
    }

    /// The working directory an engine for `tid` runs in (worktree, else the
    /// thread's project root) — unlike `work_root`, independent of which
    /// thread is focused.
    pub(super) fn thread_root(&self, tid: &str) -> Option<PathBuf> {
        let t = self.store.state.thread(tid)?;
        if let Some(w) = &t.worktree {
            return Some(PathBuf::from(w));
        }
        self.store
            .state
            .project(&t.project_id)
            .map(|p| PathBuf::from(&p.path))
    }

    /// Send `v` to the thread's engine (spawning it when needed).
    pub(super) fn engine_send(&mut self, tid: &str, v: serde_json::Value, kind: ReqKind) {
        if !self.ensure_engine(tid) {
            return;
        }
        if let Some(eng) = self.engines.get_mut(tid)
            && let Err(e) = eng.send(v, kind)
        {
            self.toast(e, true);
        }
    }

    pub(super) fn engine_command(&mut self, tid: &str, line: &str) {
        let line = if line.starts_with('/') {
            line.to_string()
        } else {
            format!("/{line}")
        };
        self.engine_send(
            tid,
            serde_json::json!({"type": "command", "line": line}),
            ReqKind::Command,
        );
    }

    pub(super) fn engine_prompt(&mut self, tid: &str, text: &str, attachments: &[PathBuf]) {
        let atts: Vec<serde_json::Value> = attachments
            .iter()
            .map(|p| serde_json::json!(p.display().to_string()))
            .collect();
        self.engine_send(
            tid,
            serde_json::json!({"type": "prompt", "text": text, "attachments": atts}),
            ReqKind::Prompt,
        );
    }

    pub(super) fn engine_interrupt(&mut self, tid: &str) {
        self.engine_send(
            tid,
            serde_json::json!({"type": "interrupt"}),
            ReqKind::Interrupt,
        );
    }

    pub(super) fn engine_approve(&mut self, tid: &str, ask_id: u64, answers: serde_json::Value) {
        self.engine_send(
            tid,
            serde_json::json!({"type": "approve", "ask_id": ask_id, "answers": answers}),
            ReqKind::Approve,
        );
    }

    /// Apply provider/model/effort to the live engine (any may be `None`).
    pub(super) fn engine_set_model(
        &mut self,
        tid: &str,
        provider: Option<&str>,
        model: Option<&str>,
        effort: Option<&str>,
    ) {
        let mut v = serde_json::Map::new();
        v.insert("type".into(), serde_json::json!("set_model"));
        if let Some(p) = provider.filter(|p| !p.is_empty() && *p != "gray") {
            v.insert("provider".into(), serde_json::json!(p));
        }
        if let Some(m) = model.filter(|m| !m.is_empty() && *m != "default") {
            v.insert("model".into(), serde_json::json!(m));
        }
        if let Some(e) = effort.filter(|e| !e.is_empty()) {
            v.insert("effort".into(), serde_json::json!(e));
        }
        if v.len() > 1 {
            self.engine_send(tid, serde_json::Value::Object(v), ReqKind::SetModel);
        }
    }

    pub(super) fn engine_resume(&mut self, tid: &str, session: &str) {
        self.engine_send(
            tid,
            serde_json::json!({"type": "resume", "session": session}),
            ReqKind::Resume,
        );
    }

    /// Whether `name` is a gray command: builtin registry name/alias, or a
    /// name/alias reported by `list_commands` once the engine answered.
    pub(super) fn is_gray_command(&self, tid: Option<&str>, name: &str) -> bool {
        if GRAY_BUILTINS.contains(&name) {
            return true;
        }
        tid.and_then(|t| self.engines.get(t))
            .map(|e| {
                e.commands
                    .iter()
                    .any(|c| c.name == name || c.aliases.iter().any(|a| a == name))
            })
            .unwrap_or(false)
    }

    /// Interactive-only surfaces that stay in the IDE (the protocol degrades
    /// them to text or refuses): models/plugins pickers, sessions browser,
    /// connect/login flows.
    fn interactive_route(
        &mut self,
        name: &str,
        arg: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        match name {
            "models" | "model" | "connect" | "provider" => {
                if arg.is_empty() {
                    self.open_model_picker(window, cx);
                } else if let Some(tid) = self.active_thread.clone() {
                    self.engine_set_model(&tid, None, Some(arg), None);
                }
                true
            }
            "plugins" => {
                if arg.is_empty() {
                    self.run(Cmd::TogglePlugins, window, cx);
                } else if let Some(tid) = self.active_thread.clone() {
                    self.engine_command(&tid, &format!("/{name} {arg}"));
                }
                true
            }
            "sessions" | "resume" => {
                if arg.is_empty() {
                    self.run(Cmd::ToggleSessions, window, cx);
                } else if let Some(tid) = self.active_thread.clone() {
                    self.engine_resume(&tid, arg);
                }
                true
            }
            "login" | "logout" | "whoami" => {
                // The IDE's connect surface is the model picker / providers.
                self.open_model_picker(window, cx);
                true
            }
            _ => false,
        }
    }

    /// Route a `/line` from the composer for a gray-engine thread: interactive
    /// commands open IDE UI, gray commands go through the real dispatcher,
    /// IDE-local commands run locally only when gray doesn't own the name,
    /// anything else is forwarded so gray can answer (like the REPL does).
    pub(super) fn engine_slash(&mut self, cmd: &str, window: &mut Window, cx: &mut Context<Self>) {
        let (name, arg) = cmd.split_once(' ').unwrap_or((cmd, ""));
        let arg = arg.trim();
        let tid = self.active_thread.clone();
        if self.interactive_route(name, arg, window, cx) {
            return;
        }
        if self.is_gray_command(tid.as_deref(), name) {
            self.engine_slash_command(cmd, cx);
            return;
        }
        if let Some(c) = self.local_slash(name, arg) {
            self.run(c, window, cx);
            return;
        }
        // Unknown on a gray thread: gray answers with its own error/help,
        // exactly like the REPL.
        self.engine_slash_command(cmd, cx);
    }

    /// Send a `/line` to the engine, creating the thread first when the
    /// composer fired on the welcome screen.
    fn engine_slash_command(&mut self, cmd: &str, cx: &mut Context<Self>) {
        let tid = match self.active_thread.clone() {
            Some(t) => t,
            None => match self.create_thread(cx) {
                Some(t) => t,
                None => return,
            },
        };
        self.engine_command(&tid, cmd);
    }

    /// The IDE-local command map, extracted from `slash` unchanged.
    fn local_slash(&self, name: &str, arg: &str) -> Option<Cmd> {
        let arg = arg.to_string();
        Some(match name {
            "new" => Cmd::NewThread,
            "theme" => Cmd::ToggleTheme,
            "pin" => Cmd::Pin,
            "plugins" => Cmd::TogglePlugins,
            "sessions" => Cmd::ToggleSessions,
            "skills" => Cmd::ToggleSkills,
            "resume" => Cmd::ResumeSession(arg),
            "model" => Cmd::Model(arg),
            "provider" => Cmd::Provider(arg),
            "rename" => Cmd::Rename(arg),
            "mode" => Cmd::Mode(arg),
            "branch" => Cmd::NewBranch(arg),
            "checkout" => Cmd::Branch(arg),
            "diff" => Cmd::ToggleDiff,
            "terminal" => Cmd::ToggleTerminal,
            "settings" => {
                if arg.is_empty() {
                    Cmd::Settings
                } else {
                    Cmd::SettingsSection(arg.to_lowercase())
                }
            }
            "archive" => Cmd::Archive,
            "checkpoint" => Cmd::Checkpoint,
            "restore" => Cmd::Restore,
            "discard" => Cmd::DiscardChanges,
            "stop" => Cmd::Stop,
            "settle" => Cmd::Settle,
            "install" => Cmd::PluginInstall(arg),
            "uninstall" => Cmd::PluginRemove(arg),
            "enable" => Cmd::PluginEnable(arg, true),
            "disable" => Cmd::PluginEnable(arg, false),
            "snooze" => Cmd::Snooze(arg.parse().unwrap_or(60)),
            "worktree" => Cmd::Worktree(arg),
            "effort" => Cmd::Effort(arg),
            "background" | "bg" => Cmd::SetBackground(arg),
            "view" | "preview" => Cmd::PreviewFile(arg),
            "gray" => Cmd::Gray(arg),
            "approve" => Cmd::Approve(true),
            "reject" => Cmd::Approve(false),
            "layout" => Cmd::SetLayout(arg),
            "attach" | "image" => Cmd::AttachImage(arg),
            "autoaccept" => Cmd::SetAutoAccept(matches!(arg.as_str(), "on" | "true" | "1")),
            "turndiff" | "changes" => Cmd::TurnDiff,
            "headdiff" => Cmd::HeadDiff,
            _ => return None,
        })
    }

    /// Drain every engine's row channel and translate. Called from `poll`.
    pub(super) fn drain_engines(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let mut msgs: Vec<(String, EngineMsg)> = Vec::new();
        for (tid, eng) in self.engines.iter_mut() {
            while let Ok(m) = eng.engine.rx.try_recv() {
                msgs.push((tid.clone(), m));
            }
            // Keep the status line fresh: every few seconds while idle, every
            // ~1.5s while a turn streams (usage updates with each step).
            let every = if eng.turn_active { 1_500 } else { 4_000 };
            if eng.status_at.elapsed() > Duration::from_millis(every) {
                let _ = eng.send(serde_json::json!({"type": "status"}), ReqKind::Status);
                eng.status_at = Instant::now();
            }
        }
        let had = !msgs.is_empty();
        for (tid, m) in msgs {
            match m {
                EngineMsg::Row(v) => self.engine_row(&tid, v, window, cx),
                EngineMsg::Died(tail) => self.engine_died(&tid, &tail, window, cx),
            }
        }
        had
    }

    /// Append a timeline activity item to `tid`.
    fn activity(&mut self, tid: &str, kind: &str, label: &str, detail: &str) {
        if detail.trim().is_empty() && matches!(kind, "output" | "say") {
            return;
        }
        self.persist(Event::ActivityAdded {
            thread_id: tid.to_string(),
            id: new_id(),
            kind: kind.to_string(),
            label: label.to_string(),
            detail: detail.trim_end().to_string(),
            at: now(),
        });
    }

    /// Thinking deltas accumulate on the turn and fold into one activity at
    /// the next non-thinking event (or turn end).
    fn flush_thinking(&mut self, tid: &str) {
        let Some(a) = self.turns.get_mut(tid) else {
            return;
        };
        let text = std::mem::take(&mut a.thinking);
        if !text.trim().is_empty() {
            self.persist(Event::ActivityAdded {
                thread_id: tid.to_string(),
                id: new_id(),
                kind: "thinking".into(),
                label: "Thinking".into(),
                detail: text.trim_end().to_string(),
                at: now(),
            });
        }
    }

    fn engine_died(&mut self, tid: &str, tail: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.engines.remove(tid);
        if self.pending_ask.as_ref().is_some_and(|a| a.tid == tid) {
            self.pending_ask = None;
        }
        if self.turns.contains_key(tid) {
            let detail = if tail.is_empty() {
                "turn ended: gray engine exited".to_string()
            } else {
                format!("turn ended: gray engine exited: {tail}")
            };
            self.finish_turn(tid, Err(detail), false, window, cx);
        } else if !tail.is_empty() {
            self.activity(tid, "error", "Engine stopped", tail);
        }
    }

    /// The row → timeline translation. Protocol rows carry `session_id`,
    /// `req` and `turn_id`; `turn_id` isn't needed for attribution (one turn
    /// at a time per engine).
    fn engine_row(
        &mut self,
        tid: &str,
        v: serde_json::Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(sid) = v.get("session_id").and_then(|x| x.as_str()) {
            let changed = self
                .engines
                .get(tid)
                .and_then(|e| e.session_id.clone())
                .as_deref()
                != Some(sid);
            if changed {
                if let Some(e) = self.engines.get_mut(tid) {
                    e.session_id = Some(sid.to_string());
                }
                if let Some(th) = self.store.state.thread(tid) {
                    let provider = th.provider.clone();
                    let known = th
                        .session
                        .as_ref()
                        .map(|(p, s)| p == &provider && s == sid)
                        .unwrap_or(false);
                    if !known {
                        self.persist(Event::ThreadSession {
                            id: tid.to_string(),
                            provider,
                            session: sid.to_string(),
                        });
                    }
                }
            }
        }
        match s(&v, "type").as_str() {
            "hello" => {
                if let Some(e) = self.engines.get_mut(tid) {
                    e.hello = Some(v);
                }
            }
            "turn_start" => {
                if let Some(e) = self.engines.get_mut(tid) {
                    e.turn_active = true;
                }
                self.turns.insert(
                    tid.to_string(),
                    ActiveTurn {
                        turn: None,
                        text: String::new(),
                        thinking: String::new(),
                        started: Instant::now(),
                    },
                );
            }
            "text" => {
                let d = s(&v, "delta");
                if let Some(a) = self.turns.get_mut(tid) {
                    a.text.push_str(&d);
                }
            }
            "thinking" => {
                let d = s(&v, "delta");
                if let Some(a) = self.turns.get_mut(tid) {
                    a.thinking.push_str(&d);
                }
            }
            "retract" => {
                if let Some(a) = self.turns.get_mut(tid) {
                    a.text.clear();
                }
            }
            "tool_start" | "tool_progress" => {}
            "tool_call" => {
                self.flush_thinking(tid);
                let name = s(&v, "name");
                let args = v
                    .get("args")
                    .map(|a| {
                        let t = a.to_string();
                        t.chars().take(400).collect::<String>()
                    })
                    .unwrap_or_default();
                self.activity(tid, "tool", &name, &args);
            }
            "tool_result" => {
                self.flush_thinking(tid);
                let name = s(&v, "name");
                let err = v.get("is_error").and_then(|x| x.as_bool()).unwrap_or(false);
                self.activity(
                    tid,
                    if err { "error" } else { "output" },
                    &format!("{name} output"),
                    &s(&v, "output"),
                );
            }
            "usage" | "turn_usage" => {
                if let Some(e) = self.engines.get_mut(tid) {
                    e.last_usage = v.get("usage").cloned();
                }
            }
            "steered" => {
                self.flush_thinking(tid);
                self.activity(tid, "steered", "Steered into turn", &s(&v, "text"));
            }
            "compacted" => {
                self.flush_thinking(tid);
                let detail = format!(
                    "{} → {} tokens · {} → {} messages · {}",
                    s(&v, "tokens_before"),
                    s(&v, "tokens_after"),
                    s(&v, "messages_before"),
                    s(&v, "messages_after"),
                    s(&v, "reason"),
                );
                self.activity(tid, "compacted", "Context compacted", &detail);
            }
            "notice" => {
                self.activity(tid, "notice", "Notice", &s(&v, "cause"));
            }
            "say" => {
                self.flush_thinking(tid);
                let level = s(&v, "level");
                let kind = s(&v, "kind");
                let (k, label) = match (level.as_str(), kind.as_str()) {
                    ("error", _) => ("error", "Error"),
                    ("warn", "provider_retry") => ("warn", "Provider retry"),
                    ("warn", _) => ("warn", "Warning"),
                    _ => ("say", "Output"),
                };
                self.activity(tid, k, label, &s(&v, "text"));
            }
            "ask" => self.engine_ask(tid, &v, window, cx),
            "turn_end" => {
                let ok = v.get("ok").and_then(|x| x.as_bool()).unwrap_or(false);
                let interrupted = v
                    .get("interrupted")
                    .and_then(|x| x.as_bool())
                    .unwrap_or(false);
                if let Some(e) = self.engines.get_mut(tid) {
                    e.turn_active = false;
                    e.status_at = Instant::now() - Duration::from_secs(4);
                }
                self.flush_thinking(tid);
                self.finish_turn(
                    tid,
                    if ok {
                        Ok(())
                    } else {
                        Err("turn failed".into())
                    },
                    interrupted,
                    window,
                    cx,
                );
            }
            "result" => self.engine_result(tid, &v, window, cx),
            "error" => self.activity(tid, "error", "Error", &s(&v, "message")),
            _ => {}
        }
        cx.notify();
    }

    /// `result` rows: correlate to the request via `pending`. A `queued:true`
    /// ack keeps the pending entry for the deferred second result.
    fn engine_result(
        &mut self,
        tid: &str,
        v: &serde_json::Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ok = v.get("ok").and_then(|x| x.as_bool()).unwrap_or(false);
        if v.get("queued").and_then(|x| x.as_bool()) == Some(true) {
            // The request landed mid-turn: gray will steer it in or run it
            // next, exactly like typed input in the REPL.
            let is_prompt = v.get("id").and_then(|i| i.as_u64()).is_some_and(|i| {
                self.engines
                    .get(tid)
                    .is_some_and(|e| e.pending.get(&i) == Some(&ReqKind::Prompt))
            });
            if is_prompt {
                self.activity(
                    tid,
                    "queued",
                    "Queued",
                    "sent while a turn was running — gray steers it in or runs it next",
                );
            }
            return;
        }
        let id = v.get("id").and_then(|x| x.as_u64());
        let kind = id.and_then(|i| self.engines.get_mut(tid).and_then(|e| e.pending.remove(&i)));
        match kind {
            Some(ReqKind::Status) => {
                if let Some(e) = self.engines.get_mut(tid) {
                    e.status = v.get("status").cloned();
                }
            }
            Some(ReqKind::Commands) => {
                let cmds = v
                    .get("commands")
                    .and_then(|c| c.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|c| {
                                Some(GrayCmd {
                                    name: c.get("name")?.as_str()?.to_string(),
                                    description: s(c, "description"),
                                    aliases: c
                                        .get("aliases")
                                        .and_then(|a| a.as_array())
                                        .map(|a| {
                                            a.iter()
                                                .filter_map(|x| x.as_str().map(str::to_string))
                                                .collect()
                                        })
                                        .unwrap_or_default(),
                                    surface: s(c, "surface"),
                                    source: s(c, "source"),
                                })
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                if let Some(e) = self.engines.get_mut(tid) {
                    e.commands = cmds;
                }
            }
            Some(ReqKind::Models) => {
                if let Some(cat) = v.get("catalog")
                    && let Some(list) = provider::parse_model_catalog(cat)
                {
                    self.model_catalog = Some(list);
                }
            }
            Some(ReqKind::Sessions) => {
                self.gray_sessions = v
                    .get("sessions")
                    .and_then(|x| x.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|x| {
                                let id = x.get("id")?.as_str()?.to_string();
                                let (preview, subagent) =
                                    sessions::clean_preview(&s(x, "first_user_text"));
                                Some(GraySession {
                                    live: sessions::is_live(&id),
                                    id,
                                    timestamp: x
                                        .get("last_message_at")
                                        .and_then(|t| t.as_i64())
                                        .unwrap_or_else(|| {
                                            x.get("started_at")
                                                .and_then(|t| t.as_i64())
                                                .unwrap_or(0)
                                        }),
                                    cwd: s(x, "cwd"),
                                    model: String::new(),
                                    preview,
                                    subagent,
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();
            }
            Some(ReqKind::Resume) => {
                if ok {
                    self.toast("Session resumed", false);
                } else {
                    self.toast(format!("Resume failed: {}", s(v, "error")), true);
                }
            }
            Some(ReqKind::SetModel) => {
                if ok {
                    let model = s(v, "model");
                    let effort = s(v, "effort");
                    let provider_id = s(v, "provider_id");
                    if let Some(e) = self.engines.get_mut(tid) {
                        e.status_at = Instant::now() - Duration::from_secs(4);
                    }
                    // Store tracks the applied truth: chip and spawn args read
                    // it when no engine status exists yet.
                    if !provider_id.is_empty() && !model.is_empty() {
                        let mode = self
                            .store
                            .state
                            .thread(tid)
                            .map(|t| t.mode.clone())
                            .unwrap_or_else(|| "full".into());
                        self.persist(Event::ThreadConfigured {
                            id: tid.to_string(),
                            provider: provider_id,
                            model,
                            mode,
                        });
                    }
                    if provider::EFFORT_LEVELS.contains(&effort.as_str()) {
                        self.persist(Event::ThreadEffort {
                            id: tid.to_string(),
                            effort,
                        });
                    }
                } else {
                    self.toast(format!("Model not set: {}", s(v, "error")), true);
                }
            }
            Some(ReqKind::Approve) | Some(ReqKind::Interrupt) | Some(ReqKind::Shutdown) => {}
            Some(ReqKind::Prompt) | Some(ReqKind::Command) | None => {
                if !ok {
                    let err = s(v, "error");
                    if !err.is_empty() {
                        self.activity(tid, "error", "Error", &err);
                    }
                }
            }
        }
        let _ = window;
        let _ = cx;
    }

    /// `ask` event → the approval card. `answers` maps each question id to the
    /// picked option labels; single-question asks answer on click.
    fn engine_ask(
        &mut self,
        tid: &str,
        v: &serde_json::Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(ask_id) = v.get("ask_id").and_then(|a| a.as_u64()) else {
            return;
        };
        let questions: Vec<AskQuestion> = v
            .get("questions")
            .and_then(|q| q.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|q| {
                        Some(AskQuestion {
                            id: q.get("id")?.as_str()?.to_string(),
                            header: s(q, "header"),
                            question: s(q, "question"),
                            detail: s(q, "detail"),
                            options: q
                                .get("options")
                                .and_then(|o| o.as_array())
                                .map(|os| {
                                    os.iter()
                                        .filter_map(|o| {
                                            Some((
                                                o.get("label")?.as_str()?.to_string(),
                                                s(o, "description"),
                                            ))
                                        })
                                        .collect()
                                })
                                .unwrap_or_default(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        if questions.is_empty() {
            return;
        }
        let summary = questions
            .iter()
            .map(|q| {
                if q.question.is_empty() {
                    q.header.clone()
                } else {
                    q.question.clone()
                }
            })
            .collect::<Vec<_>>()
            .join(" · ");
        self.notify_turn(
            tid,
            format!("Approve: {}", preview(&summary, 80)),
            NotificationType::Warning,
            window,
            cx,
        );
        self.pending_ask = Some(PendingAsk {
            tid: tid.to_string(),
            ask_id,
            questions,
            picks: HashMap::new(),
        });
    }

    /// Answer a pending ask: the picked label for one question (`qid`) or the
    /// submit-all path (`qid` empty fills unanswered with first options).
    pub(super) fn answer_ask(&mut self, qid: Option<(&str, &str)>) {
        let Some(mut ask) = self.pending_ask.take() else {
            return;
        };
        if let Some((q, label)) = qid {
            ask.picks.insert(q.to_string(), label.to_string());
        }
        if ask.questions.len() == 1 || qid.is_none() {
            self.send_ask(ask);
        } else {
            self.pending_ask = Some(ask);
        }
    }

    /// `/reject` on an ask: pick the negative-sounding option per question
    /// (falling back to the last) — gray waits on a real answer either way.
    pub(super) fn reject_ask(&mut self) {
        let Some(mut ask) = self.pending_ask.take() else {
            return;
        };
        for q in &ask.questions {
            if ask.picks.contains_key(&q.id) {
                continue;
            }
            let pick = q
                .options
                .iter()
                .find(|(l, _)| {
                    let l = l.to_ascii_lowercase();
                    l.starts_with("no")
                        || l.contains("deny")
                        || l.contains("reject")
                        || l.contains("cancel")
                })
                .or_else(|| q.options.last())
                .map(|o| o.0.clone())
                .unwrap_or_default();
            ask.picks.insert(q.id.clone(), pick);
        }
        self.send_ask(ask);
    }

    /// Fill unanswered questions with their first option, send `approve`, and
    /// record the answer on the timeline.
    fn send_ask(&mut self, mut ask: PendingAsk) {
        {
            for q in &ask.questions {
                ask.picks
                    .entry(q.id.clone())
                    .or_insert_with(|| q.options.first().map(|o| o.0.clone()).unwrap_or_default());
            }
            let answers = serde_json::json!(
                ask.picks
                    .iter()
                    .map(|(k, v)| (k.clone(), vec![v.clone()]))
                    .collect::<std::collections::HashMap<_, _>>()
            );
            let detail = ask
                .questions
                .iter()
                .map(|q| {
                    let pick = ask.picks.get(&q.id).cloned().unwrap_or_default();
                    format!(
                        "{} → {pick}",
                        if q.question.is_empty() {
                            &q.header
                        } else {
                            &q.question
                        }
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            let tid = ask.tid.clone();
            self.engine_approve(&tid, ask.ask_id, answers);
            self.persist(Event::ActivityAdded {
                thread_id: tid,
                id: new_id(),
                kind: "approval".into(),
                label: "Answered".into(),
                detail,
                at: now(),
            });
        }
    }

    /// Push the active thread's stored provider/model/effort to its live
    /// engine (no-op when the thread has no engine — spawn args carry it).
    pub(super) fn push_model_to_engine(&mut self) {
        let Some(tid) = self.active_thread.clone() else {
            return;
        };
        if !self.engines.contains_key(&tid) {
            return;
        }
        let Some(t) = self.store.state.thread(&tid) else {
            return;
        };
        let (p, m, e) = (t.provider.clone(), t.model.clone(), t.effort.clone());
        self.engine_set_model(&tid, Some(&p), Some(&m), Some(&e));
    }

    /// Compact status text under the composer for the active thread's engine.
    pub(super) fn engine_status_line(&self) -> Option<String> {
        let tid = self.active_thread.as_ref()?;
        let eng = self.engines.get(tid)?;
        let st = eng.status.as_ref()?;
        let model = st
            .get("model_label")
            .and_then(|x| x.as_str())
            .filter(|x| !x.is_empty())
            .or_else(|| st.get("model").and_then(|x| x.as_str()))
            .unwrap_or("no model");
        let effort = st.get("effort").and_then(|x| x.as_str()).unwrap_or("");
        let used = st.get("context_used").and_then(|x| x.as_u64()).or_else(|| {
            eng.last_usage
                .as_ref()
                .and_then(|u| u.get("input_tokens").and_then(|x| x.as_u64()))
        });
        let window = st
            .get("context_window")
            .and_then(|x| x.as_u64())
            .unwrap_or(0);
        let totals = st.get("totals").cloned().unwrap_or_default();
        let cost = totals
            .get("cost_usd")
            .and_then(|x| x.as_f64())
            .unwrap_or(0.0);
        let cache = totals.get("cache_hit_pct").and_then(|x| x.as_f64());
        let queued = st
            .get("queued_inputs")
            .and_then(|x| x.as_u64())
            .unwrap_or(0);
        let mut parts = vec![model.to_string()];
        if !effort.is_empty() {
            parts.push(effort.to_string());
        }
        if let Some(u) = used {
            parts.push(format!("ctx {}/{}", fmt_tokens(u), fmt_tokens(window)));
        } else if window > 0 {
            parts.push(format!("ctx –/{}", fmt_tokens(window)));
        }
        if cost > 0.0 {
            parts.push(format!("${cost:.3}"));
        }
        if let Some(p) = cache {
            parts.push(format!("cache {p:.0}%"));
        }
        if queued > 0 {
            parts.push(format!("{queued} queued"));
        }
        Some(parts.join(" · "))
    }
}

fn fmt_tokens(n: u64) -> String {
    if n >= 1000 {
        format!("{:.1}k", n as f64 / 1000.0)
    } else {
        n.to_string()
    }
}
