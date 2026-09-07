use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1::{
    AuthenticateRequest, CancelNotification, InitializeRequest, LoadSessionRequest,
    RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    SelectedPermissionOutcome, SessionId, SessionNotification,
};
use agent_client_protocol::util::MatchDispatch;
use agent_client_protocol::{
    AcpAgent, AcpAgentConfig, ActiveSession, Builder, Client, ConnectionTo, NullRun, Responder,
    SessionMessage,
};

use crate::error::AcpError;
use crate::events::EventMapper;
use crate::registry::AgentSpec;

pub const SESSION_START_TIMEOUT: Duration = Duration::from_secs(90);
pub const PROMPT_TIMEOUT: Duration = Duration::from_secs(600);

pub trait PermissionPrompt: Send + Sync {
    fn ask(&self, tool_call_title: &str, options: Vec<String>) -> Option<String>;
}

pub struct DenyAllPrompt;

impl PermissionPrompt for DenyAllPrompt {
    fn ask(&self, _tool_call_title: &str, _options: Vec<String>) -> Option<String> {
        None
    }
}

#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub agent_key: String,
    pub agent_display: String,
    pub session_id: String,
    pub mode: Option<String>,
}

pub struct AcpSessionOptions {
    pub spec: AgentSpec,
    pub cwd: PathBuf,
    pub resume_session_id: Option<String>,
    pub auto_approve: bool,
    pub permission_prompt: Arc<dyn PermissionPrompt>,
    pub display: String,
}

fn agent_transport(spec: &AgentSpec) -> AcpAgent {
    let mut config = AcpAgentConfig::new(spec.command.clone()).args(spec.args.clone());
    for (k, v) in &spec.env {
        config = config.env(k, v);
    }
    AcpAgent::new(config)
}

fn answer_permission(
    req: RequestPermissionRequest,
    auto_approve: bool,
    prompt: &Arc<dyn PermissionPrompt>,
    responder: Responder<RequestPermissionResponse>,
) -> Result<(), agent_client_protocol::Error> {
    let title = req.tool_call.tool_call_id.0.to_string();
    let option_ids: Vec<String> = req
        .options
        .iter()
        .map(|o| o.option_id.0.to_string())
        .collect();
    let chosen = if auto_approve {
        option_ids
            .iter()
            .find(|id| *id == "allow-always")
            .or_else(|| option_ids.iter().find(|id| *id == "allow-once"))
            .or_else(|| option_ids.first())
            .cloned()
    } else {
        prompt.as_ref().ask(&title, option_ids)
    };
    match chosen {
        Some(id) => responder.respond(RequestPermissionResponse::new(
            RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(
                agent_client_protocol::schema::v1::PermissionOptionId::new(id),
            )),
        )),
        None => responder.respond(RequestPermissionResponse::new(
            RequestPermissionOutcome::Cancelled,
        )),
    }
}

fn gray_client_capabilities() -> agent_client_protocol::schema::v1::ClientCapabilities {
    use agent_client_protocol::schema::v1::{ClientCapabilities, FileSystemCapabilities};
    ClientCapabilities::new()
        .fs(FileSystemCapabilities::new()
            .read_text_file(true)
            .write_text_file(true))
        .terminal(false)
}

fn guard_path(cwd: &std::path::Path, path: &std::path::Path) -> Result<PathBuf, String> {
    if std::env::var("GRAY_ACP_ALLOW_ANY_PATH").as_deref() == Ok("1") {
        return Ok(path.to_path_buf());
    }
    let canon_cwd = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    let canon_path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        canon_cwd.join(path)
    };
    let normalized = canon_path;
    if normalized.starts_with(&canon_cwd) {
        Ok(normalized)
    } else {
        Err(format!("path outside workspace: {}", path.display()))
    }
}

fn client_builder(
    auto_approve: bool,
    prompt_impl: Arc<dyn PermissionPrompt>,
    updates: tokio::sync::mpsc::UnboundedSender<gray_core::event::AgentEvent>,
    cwd: PathBuf,
) -> Builder<
    Client,
    impl agent_client_protocol::HandleDispatchFrom<agent_client_protocol::Agent>,
    NullRun,
> {
    use agent_client_protocol::schema::v1::{
        ReadTextFileRequest, ReadTextFileResponse, WriteTextFileRequest, WriteTextFileResponse,
    };
    let cwd_read = cwd.clone();
    let cwd_write = cwd.clone();
    Client
        .builder()
        .name("gray")
        .on_receive_notification(
            move |n: SessionNotification, _cx| {
                let updates = updates.clone();
                async move {
                    let mut mapper = EventMapper::new();
                    for ev in mapper.map_update(&n.update) {
                        let _ = updates.send(ev);
                    }
                    Ok(())
                }
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .on_receive_request(
            move |req: RequestPermissionRequest, responder, _cx| {
                let prompt_impl = prompt_impl.clone();
                async move { answer_permission(req, auto_approve, &prompt_impl, responder) }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            move |req: ReadTextFileRequest, responder: Responder<ReadTextFileResponse>, _cx| {
                let cwd_read = cwd_read.clone();
                async move {
                    match guard_path(&cwd_read, &req.path) {
                        Err(e) => responder.respond_with_error(
                            agent_client_protocol::Error::invalid_request()
                                .data(serde_json::json!(e)),
                        ),
                        Ok(path) => {
                            let start = req.line.unwrap_or(1).max(1) as usize;
                            let limit = req.limit.unwrap_or(2000).min(5000) as usize;
                            match std::fs::read_to_string(&path) {
                                Err(e) => responder.respond_with_error(
                                    agent_client_protocol::Error::internal_error().data(
                                        serde_json::json!(format!("read {}: {e}", path.display())),
                                    ),
                                ),
                                Ok(text) => {
                                    let lines: Vec<&str> = text.lines().collect();
                                    let from = (start - 1).min(lines.len());
                                    let to = (from + limit).min(lines.len());
                                    responder.respond(ReadTextFileResponse::new(
                                        lines[from..to].join("\n"),
                                    ))
                                }
                            }
                        }
                    }
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            move |req: WriteTextFileRequest, responder: Responder<WriteTextFileResponse>, _cx| {
                let cwd_write = cwd_write.clone();
                async move {
                    match guard_path(&cwd_write, &req.path) {
                        Err(e) => responder.respond_with_error(
                            agent_client_protocol::Error::invalid_request()
                                .data(serde_json::json!(e)),
                        ),
                        Ok(path) => match std::fs::write(&path, &req.content) {
                            Err(e) => responder.respond_with_error(
                                agent_client_protocol::Error::internal_error().data(
                                    serde_json::json!(format!("write {}: {e}", path.display())),
                                ),
                            ),
                            Ok(()) => responder.respond(WriteTextFileResponse::new()),
                        },
                    }
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
}

async fn open_session(
    conn: &ConnectionTo<agent_client_protocol::Agent>,
    cwd: PathBuf,
    resume: Option<String>,
    auth_hint: Option<String>,
) -> Result<(ActiveSession<'static, agent_client_protocol::Agent>, String), AcpError> {
    use agent_client_protocol::schema::v1::AuthMethodId;

    let init_req =
        InitializeRequest::new(ProtocolVersion::V1).client_capabilities(gray_client_capabilities());
    let init = conn
        .send_request(init_req)
        .block_task()
        .await
        .map_err(|e| AcpError::Request {
            method: "initialize",
            message: e.to_string(),
        })?;
    if init.protocol_version != ProtocolVersion::V1 {
        return Err(AcpError::VersionMismatch(format!(
            "{:?}",
            init.protocol_version
        )));
    }
    if let Some(resume_id) = resume.as_deref()
        && init.agent_capabilities.load_session
    {
        let req = LoadSessionRequest::new(SessionId::new(resume_id), cwd.clone());
        if conn.send_request(req).block_task().await.is_ok() {
            let session = conn
                .build_session(cwd)
                .block_task()
                .start_session()
                .await
                .map_err(|e| AcpError::Request {
                    method: "session/load",
                    message: e.to_string(),
                })?;
            let id = session.session_id().0.to_string();
            return Ok((session, id));
        }
    }
    match conn
        .build_session(cwd.clone())
        .block_task()
        .start_session()
        .await
    {
        Ok(session) => {
            let id = session.session_id().0.to_string();
            Ok((session, id))
        }
        Err(e) => {
            let msg = e.to_string();
            if !init.auth_methods.is_empty() && msg.to_lowercase().contains("auth") {
                let methods: Vec<String> = init
                    .auth_methods
                    .iter()
                    .map(|m| m.id().0.to_string())
                    .collect();
                let method_id = auth_hint
                    .or_else(|| methods.first().cloned())
                    .unwrap_or_default();
                if method_id.is_empty() {
                    return Err(AcpError::AuthRequired(methods.join(", ")));
                }
                conn.send_request(AuthenticateRequest::new(AuthMethodId::new(method_id)))
                    .block_task()
                    .await
                    .map_err(|e| AcpError::Request {
                        method: "authenticate",
                        message: e.to_string(),
                    })?;
                let session = conn
                    .build_session(cwd)
                    .block_task()
                    .start_session()
                    .await
                    .map_err(|e| AcpError::Request {
                        method: "session/new",
                        message: e.to_string(),
                    })?;
                let id = session.session_id().0.to_string();
                Ok((session, id))
            } else {
                Err(AcpError::Request {
                    method: "session/new",
                    message: msg,
                })
            }
        }
    }
}

pub struct AcpSession {
    spec: AgentSpec,
    display: String,
    session_id: String,
    mode: Option<String>,
    usage_text: Option<String>,
    auto_approve: bool,
    permission_prompt: Arc<dyn PermissionPrompt>,
    cwd: PathBuf,
    cancel_flag: Arc<std::sync::atomic::AtomicBool>,
}

/// Display-only label for the permission posture (`on`/`off`).
/// No approval-logic effect; used by `/acp status` rendering.
pub fn auto_approve_label(auto_approve: bool) -> &'static str {
    if auto_approve { "on" } else { "off" }
}

impl AcpSession {
    pub async fn start(opts: AcpSessionOptions) -> Result<Self, AcpError> {
        let display = if opts.display.is_empty() {
            opts.spec.display.to_string()
        } else {
            opts.display
        };
        if !crate::registry::installed(&opts.spec) {
            return Err(AcpError::NotInstalled(
                opts.spec.key.to_string(),
                opts.spec.install_hint.to_string(),
            ));
        }
        // The pinned codex adapter predates the current ~/.codex/config.toml
        // schema and crashes parsing it: run it under an isolated CODEX_HOME
        // (defaults + copied auth) instead of the user's real one.
        let mut spec = opts.spec.clone();
        if spec.key == "codex" && !spec.env.iter().any(|(k, _)| k == "CODEX_HOME") {
            let home = crate::registry::ensure_codex_home();
            spec.env
                .push(("CODEX_HOME".to_string(), home.display().to_string()));
        }
        let spec = spec;
        let cwd = opts.cwd.clone();
        let resume = opts.resume_session_id.clone();
        let prompt_impl = opts.permission_prompt.clone();
        let auto_approve = opts.auto_approve;
        let (updates_tx, _updates_rx) =
            tokio::sync::mpsc::unbounded_channel::<gray_core::event::AgentEvent>();

        let spec_for_connect = spec.clone();
        let cwd_for_connect = cwd.clone();
        let cwd_for_builder = cwd.clone();
        let run = client_builder(auto_approve, prompt_impl, updates_tx, cwd_for_builder)
            .connect_with(
                agent_transport(&spec_for_connect),
                |conn: ConnectionTo<agent_client_protocol::Agent>| async move {
                    let (_session, session_id) = open_session(&conn, cwd_for_connect, resume, None)
                        .await
                        .map_err(|e| {
                            agent_client_protocol::Error::internal_error()
                                .data(serde_json::json!(e.to_string()))
                        })?;
                    Ok(session_id)
                },
            );

        let session_id = tokio::time::timeout(SESSION_START_TIMEOUT, run)
            .await
            .map_err(|_| AcpError::Timeout(SESSION_START_TIMEOUT))?
            .map_err(|e| AcpError::Other(anyhow::anyhow!(e.to_string())))?;

        Ok(Self {
            spec: opts.spec,
            display,
            session_id,
            mode: None,
            usage_text: None,
            auto_approve: opts.auto_approve,
            permission_prompt: opts.permission_prompt,
            cwd: opts.cwd,
            cancel_flag: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        })
    }

    pub fn info(&self) -> SessionInfo {
        SessionInfo {
            agent_key: self.spec.key.to_string(),
            agent_display: self.display.clone(),
            session_id: self.session_id.clone(),
            mode: self.mode.clone(),
        }
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn agent_key(&self) -> &str {
        self.spec.key
    }

    pub fn agent_display(&self) -> &str {
        &self.display
    }

    pub fn usage_text(&self) -> Option<&str> {
        self.usage_text.as_deref()
    }

    /// Whether this session auto-approves permission requests
    /// (`--yolo` / `GRAY_ACP_AUTO_APPROVE=1`). Display-only accessor;
    /// approval logic itself is unchanged.
    pub fn auto_approve(&self) -> bool {
        self.auto_approve
    }

    pub async fn prompt(
        &mut self,
        text: &str,
        on_event: &mut dyn FnMut(&gray_core::event::AgentEvent),
    ) -> Result<gray_core::event::StopReason, AcpError> {
        if !crate::registry::installed(&self.spec) {
            return Err(AcpError::NotInstalled(
                self.spec.key.to_string(),
                self.spec.install_hint.to_string(),
            ));
        }
        self.cancel_flag
            .store(false, std::sync::atomic::Ordering::SeqCst);
        let cancel_flag = self.cancel_flag.clone();
        // The connect closure below moves its own clone; keep one here so the
        // result mapping can read cancellation without borrowing `self`.
        let cancel_flag_for_result = cancel_flag.clone();
        let spec = self.spec.clone();
        let cwd = self.cwd.clone();
        let resume = Some(self.session_id.clone());
        let text = text.to_string();
        let auto_approve = self.auto_approve;
        let prompt_impl = self.permission_prompt.clone();
        let (updates_tx, mut updates_rx) =
            tokio::sync::mpsc::unbounded_channel::<gray_core::event::AgentEvent>();

        let cwd_for_builder = cwd.clone();
        let driver = client_builder(auto_approve, prompt_impl, updates_tx, cwd_for_builder)
            .connect_with(
                agent_transport(&spec),
                |conn: ConnectionTo<agent_client_protocol::Agent>| async move {
                    let err = |method: &'static str, message: String| {
                        agent_client_protocol::Error::internal_error()
                            .data(serde_json::json!(format!("{method}: {message}")))
                    };
                    let (mut session, opened_id) = open_session(&conn, cwd, resume, None)
                        .await
                        .map_err(|e| err("session/new", e.to_string()))?;
                    let cancel_conn = conn.clone();
                    let cancel_flag_spawn = cancel_flag.clone();
                    let sid = session.session_id().clone();
                    tokio::spawn(async move {
                        loop {
                            tokio::time::sleep(Duration::from_millis(100)).await;
                            if cancel_flag_spawn.load(std::sync::atomic::Ordering::SeqCst) {
                                let _ = cancel_conn
                                    .send_notification(CancelNotification::new(sid.clone()));
                                break;
                            }
                        }
                    });
                    session
                        .send_prompt(text)
                        .map_err(|e| err("session/prompt", e.to_string()))?;
                    let mapper = EventMapper::new();
                    loop {
                        let msg = session
                            .read_update()
                            .await
                            .map_err(|e| err("session/prompt", e.to_string()))?;
                        match msg {
                            SessionMessage::SessionMessage(dispatch) => {
                                MatchDispatch::new(dispatch)
                                    .if_notification(async |n: SessionNotification| {
                                        let mut mapper = EventMapper::new();
                                        for ev in mapper.map_update(&n.update) {
                                            let _ = ev;
                                        }
                                        Ok(())
                                    })
                                    .await
                                    .otherwise_ignore()
                                    .map_err(|e| err("session/update", e.to_string()))?;
                            }
                            SessionMessage::StopReason(stop) => {
                                if cancel_flag.load(std::sync::atomic::Ordering::SeqCst) {
                                    return Err(agent_client_protocol::Error::internal_error()
                                        .data(serde_json::json!("cancelled")));
                                }
                                return Ok((mapper.map_stop(&stop), opened_id));
                            }
                            _ => {}
                        }
                    }
                },
            );

        let pump = async {
            for ev in EventMapper::new().begin() {
                on_event(&ev);
            }
            while let Some(ev) = updates_rx.recv().await {
                on_event(&ev);
            }
        };
        let run = async {
            tokio::time::timeout(PROMPT_TIMEOUT, driver)
                .await
                .map_err(|_| AcpError::Timeout(PROMPT_TIMEOUT))?
                .map_err(|e| {
                    let msg = e.to_string();
                    if cancel_flag_for_result.load(std::sync::atomic::Ordering::SeqCst)
                        || msg.contains("cancelled")
                    {
                        AcpError::Cancelled
                    } else {
                        AcpError::Other(anyhow::anyhow!(msg))
                    }
                })
                .map(|(stop, new_id): (gray_core::event::StopReason, String)| {
                    // The agent may have rotated the id (load fallback,
                    // fresh session after /new): track the effective one.
                    self.session_id = new_id;
                    stop
                })
        };
        let (reason, _) = tokio::join!(run, pump);
        let reason = reason?;
        on_event(&gray_core::event::AgentEvent::TurnEnd {
            stop_reason: reason,
            usage: gray_core::event::Usage::default(),
        });
        Ok(reason)
    }

    pub async fn cancel(&self) {
        self.cancel_flag
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    pub async fn new_session(&mut self) -> Result<(), AcpError> {
        self.session_id = String::new();
        Ok(())
    }

    pub async fn shutdown(self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_approve_label_renders_on_off() {
        assert_eq!(auto_approve_label(true), "on");
        assert_eq!(auto_approve_label(false), "off");
    }

    #[test]
    fn guard_path_keeps_paths_inside_workspace() {
        use std::path::PathBuf;
        let cwd = std::env::temp_dir()
            .canonicalize()
            .unwrap_or_else(|_| std::env::temp_dir());
        let inside = cwd.join("sub").join("file.txt");
        assert_eq!(guard_path(&cwd, &inside), Ok(inside));
        if let Some(parent) = cwd.parent() {
            let outside: PathBuf = parent.join("definitely-outside-gray-workspace-xyz");
            assert!(
                guard_path(&cwd, &outside).is_err(),
                "sibling of cwd must be rejected"
            );
        }
    }
}
