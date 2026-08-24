//! Web mode: Axum HTTP server serving the embedded single-page chat UI
//! and the `/api` endpoints for SSE streaming and session management.

use std::path::Path;
use std::sync::Arc;

use axum::extract::{Path as AxumPath, State};
use axum::http::header::{HeaderName, HeaderValue, CONTENT_TYPE};
use axum::http::{StatusCode, Uri};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures::stream;
use gray_core::agent::{Agent, ToolContext};
use gray_core::message::Message;
use gray_session::{JsonlSessionStore, SessionEntry, SessionError, SessionId, SessionMeta, SessionStore, SessionSummary};
use rust_embed::RustEmbed;
use serde::{Deserialize, Serialize};

use crate::config::Config;

/// Embedded assets for the web UI.
#[derive(RustEmbed)]
#[folder = "../../web/"]
pub struct Assets;

/// Factory type for creating [`Agent`] instances given a working directory.
pub type AgentFactory = Arc<dyn Fn(&Path) -> anyhow::Result<Agent> + Send + Sync>;

/// Shared state for Axum web handlers.
#[derive(Clone)]
pub struct AppState {
    pub config: Config,
    pub store: Arc<dyn SessionStore>,
    pub agent_factory: AgentFactory,
}

/// Request body for `POST /api/chat`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatRequestPayload {
    pub session_id: Option<String>,
    pub message: String,
}

/// Creates the Axum [`Router`] configured with all web and API routes.
pub fn create_router(state: AppState) -> Router {
    Router::new()
        .route("/api/chat", post(chat_handler))
        .route("/api/sessions", get(list_sessions_handler))
        .route(
            "/api/sessions/:id",
            get(get_session_handler).delete(delete_session_handler),
        )
        .route("/api/config", get(get_config_handler))
        .fallback(static_handler)
        .with_state(state)
}

/// Runs the web server mode, binding to the configured address and serving requests.
pub async fn run_web_mode(config: &Config) -> anyhow::Result<()> {
    let store = Arc::new(JsonlSessionStore::default());
    let cfg = config.clone();
    let agent_factory: AgentFactory = Arc::new(move |cwd| crate::build_agent(&cfg, cwd));

    let state = AppState {
        config: config.clone(),
        store,
        agent_factory,
    };

    let router = create_router(state);
    let addr = format!("127.0.0.1:{}", config.port);
    let listener = tokio::net::TcpListener::bind(&addr).await
        .map_err(|e| anyhow::anyhow!("failed to bind to {addr}: {e}"))?;

    println!("Listening on http://{addr}");
    axum::serve(listener, router).await
        .map_err(|e| anyhow::anyhow!("server error: {e}"))?;

    Ok(())
}

async fn chat_handler(
    State(state): State<AppState>,
    Json(payload): Json<ChatRequestPayload>,
) -> Result<Response, (StatusCode, String)> {
    let cwd = std::env::current_dir()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to get current dir: {e}")))?;

    let (session_id, initial_messages) = match payload.session_id.as_deref() {
        Some(id_str) if !id_str.trim().is_empty() => {
            let id = SessionId::new(id_str);
            let entries = match state.store.load(&id).await {
                Ok((_meta, entries)) => entries,
                Err(e) => {
                    return Err((
                        StatusCode::NOT_FOUND,
                        format!("Session not found: {e}"),
                    ));
                }
            };
            let msgs = entries.into_iter().map(|e| e.message).collect::<Vec<_>>();
            (id, msgs)
        }
        _ => {
            let id = SessionId::generate();
            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            let meta = SessionMeta::new(id.clone(), timestamp, cwd.clone(), state.config.model.clone());
            state.store.create(meta).await;
            (id, Vec::new())
        }
    };

    let initial_count = initial_messages.len();
    let user_msg = Message::user(&payload.message);

    state
        .store
        .append(&session_id, &user_msg)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to append message: {e}")))?;

    let mut agent = (state.agent_factory)(&cwd)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to build agent: {e}")))?
        .with_messages(initial_messages);

    let cancel = tokio_util::sync::CancellationToken::new();
    let ctx = ToolContext {
        cwd,
        cancel,
    };

    let events = agent
        .run(user_msg, ctx)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Agent execution failed: {e}")))?;

    // Append new assistant / tool messages produced during this turn
    if agent.messages().len() > initial_count + 1 {
        for msg in &agent.messages()[initial_count + 1..] {
            let _ = state.store.append(&session_id, msg).await;
        }
    }

    let stream = stream::iter(events.into_iter().map(|ev| {
        Event::default().json_data(&ev)
    }));

    let mut response = Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response();

    if let Ok(val) = HeaderValue::from_str(session_id.as_str()) {
        response.headers_mut().insert(HeaderName::from_static("x-session-id"), val);
    }

    Ok(response)
}

async fn list_sessions_handler(
    State(state): State<AppState>,
) -> Json<Vec<SessionSummary>> {
    let summaries = state.store.list().await;
    Json(summaries)
}

async fn get_session_handler(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<Vec<SessionEntry>>, (StatusCode, String)> {
    let session_id = SessionId::new(id);
    match state.store.load(&session_id).await {
        Ok((_meta, entries)) => Ok(Json(entries)),
        Err(SessionError::NotFound(_)) => {
            Err((StatusCode::NOT_FOUND, "Session not found".to_string()))
        }
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, e.to_string())),
    }
}

async fn delete_session_handler(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    let session_id = SessionId::new(id);
    state
        .store
        .delete(&session_id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(StatusCode::OK)
}

async fn get_config_handler(
    State(state): State<AppState>,
) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "model": state.config.model,
        "base_url": state.config.base_url,
    }))
}

async fn static_handler(uri: Uri) -> impl IntoResponse {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };

    match Assets::get(path) {
        Some(content) => {
            let mime = mime_type(path);
            ([(CONTENT_TYPE, mime)], content.data).into_response()
        }
        None => match Assets::get("index.html") {
            Some(index) => {
                ([(CONTENT_TYPE, "text/html; charset=utf-8")], index.data).into_response()
            }
            None => (StatusCode::NOT_FOUND, "404 Not Found").into_response(),
        },
    }
}

fn mime_type(path: &str) -> &'static str {
    if path.ends_with(".html") {
        "text/html; charset=utf-8"
    } else if path.ends_with(".js") {
        "application/javascript; charset=utf-8"
    } else if path.ends_with(".css") {
        "text/css; charset=utf-8"
    } else if path.ends_with(".json") {
        "application/json"
    } else if path.ends_with(".svg") {
        "image/svg+xml"
    } else if path.ends_with(".png") {
        "image/png"
    } else {
        "application/octet-stream"
    }
}
