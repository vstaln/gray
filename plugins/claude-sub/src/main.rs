//! claude-sub: a protocol-1.2 provider sidecar for Gray.
//!
//! Claude Pro/Max subscription through the official `claude` CLI: the host
//! owns tools, approvals and compaction, the sidecar only answers — one
//! upstream request per chat turn, enforced by the admission relay.
//!
//! Wire (NDJSON over stdio, host ids are opaque):
//! - `plugin/manifest` → manifest + the `claude-subscription` provider decl.
//! - `provider/models` → pinned catalog (no HTTP endpoint exists).
//! - `provider/chat` → one relayed turn (declares the per-turn relay URL +
//!   bearer the host's standard Responses POST must use).
//! - `provider/auth/*` → the user's own `claude auth login` owns
//!   credentials; start/poll report external-login status, refresh/revoke
//!   are unsupported.
//! - `plugin/shutdown` → clean exit. Unknown methods are protocol errors
//!   (provider sidecars must fail loudly, never hang a turn).

use claude_sub::{catalog, manifest, models, relay, setup};

use gray_plugin::{ProviderRefreshRequest, ProviderRevokeRequest, ProviderRpcError};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::Write;
use std::sync::{Arc, Mutex};

#[derive(Deserialize)]
struct Request {
    id: Value,
    method: String,
    #[serde(default)]
    params: Option<Value>,
}

#[derive(Serialize)]
struct Response {
    id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<Value>,
}

/// Active relay intents by per-turn token (see `relay::RelayIntent`):
/// the relay server fills the transcript in when the host POSTs.
type Relays = relay::Intents;

#[tokio::main(flavor = "multi_thread")]
async fn main() -> anyhow::Result<()> {
    let relays: Relays = Arc::new(Mutex::new(HashMap::new()));
    let mut lines =
        tokio::io::AsyncBufReadExt::lines(tokio::io::BufReader::new(tokio::io::stdin()));
    let mut stdout = std::io::stdout();
    // Progress lines (`host/say`) need the transport's request path; the
    // binary speaks raw NDJSON, so progress is folded into the turn reply.
    // (A host.say-emitting variant would need the SidecarPlugin transport;
    // the sse text already carries the full answer.)
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let request: Request = match serde_json::from_str(&line) {
            Ok(request) => request,
            Err(_) => continue,
        };
        let outcome = handle(&relays, &request).await;
        let response = match outcome {
            Ok(result) => Response {
                id: request.id,
                result: Some(result),
                error: None,
            },
            Err(error) => Response {
                id: request.id,
                result: None,
                error: Some(error_value(error)),
            },
        };
        let frame = serde_json::to_string(&response)?;
        // Protocol frames only: never log credential payloads or upstream bodies.
        let _ = writeln!(stdout, "{frame}");
        stdout.flush()?;
        if request.method == "plugin/shutdown" {
            return Ok(());
        }
    }
    Ok(())
}

async fn handle(relays: &Relays, request: &Request) -> Result<Value, ProviderRpcError> {
    let params = request.params.clone().unwrap_or_else(|| json!({}));
    match request.method.as_str() {
        "plugin/manifest" => Ok(serde_json::to_value(manifest::manifest()).unwrap()),
        "provider/auth/start" => {
            ensure_provider(
                params
                    .get("provider")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                params
                    .get("auth_method")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            )?;
            // External login: the CLI owns credentials; report where to fix it.
            match setup::resolve_command() {
                None => Err(ProviderRpcError::Unavailable(setup::INSTALL_HINT.into())),
                Some(_) => Err(ProviderRpcError::Unavailable(
                    "Claude Code login lives in your terminal: run `claude auth login`, then retry.".into(),
                )),
            }
        }
        "provider/auth/poll" => Err(ProviderRpcError::Protocol(
            "external login has no pollable operation".into(),
        )),
        "provider/auth/cancel" => Ok(json!({})),
        "provider/auth/refresh" => {
            let req: ProviderRefreshRequest =
                serde_json::from_value(params).map_err(|_| invalid("provider refresh request"))?;
            ensure_provider(&req.provider, &req.auth_method)?;
            Ok(json!({"status": "unsupported"}))
        }
        "provider/auth/revoke" => {
            let req: ProviderRevokeRequest =
                serde_json::from_value(params).map_err(|_| invalid("provider revoke request"))?;
            ensure_provider(&req.provider, &req.auth_method)?;
            Ok(json!({"status": "unsupported"}))
        }
        "provider/models" => {
            // External login owns no credential material: only the
            // provider/auth-method pair is checked, the envelope (if any)
            // is ignored.
            let provider = params
                .get("provider")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let auth_method = params
                .get("auth_method")
                .and_then(Value::as_str)
                .unwrap_or_default();
            ensure_provider(provider, auth_method)?;
            Ok(serde_json::to_value(models::catalog()).unwrap())
        }
        "provider/chat" => chat_turn(relays, params).await,
        "plugin/shutdown" => Ok(json!({})),
        _ => Err(ProviderRpcError::Protocol("unknown provider method".into())),
    }
}

/// One relayed turn: park the intent, open the loopback relay, and hand
/// the host the relay URL + per-turn bearer its standard Responses POST
/// uses. The admitted POST translates, spawns, folds and streams — the
/// sidecar answers from the relay, never from this method.
async fn chat_turn(relays: &Relays, params: Value) -> Result<Value, ProviderRpcError> {
    let provider = params
        .get("provider")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if provider != manifest::PROVIDER_ID {
        return Err(ProviderRpcError::Protocol("unknown provider".into()));
    }
    let model = params
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("sonnet")
        .to_string();
    let effort: Option<String> = params
        .get("effort")
        .and_then(Value::as_str)
        .map(str::to_string);
    // Fail fast before opening anything: no binary, no login, no relay.
    if crate::setup::resolve_command().is_none() {
        return Err(ProviderRpcError::Unavailable(setup::INSTALL_HINT.into()));
    }
    if let Some(key) = setup::conflicting_env() {
        return Err(ProviderRpcError::Unavailable(format!(
            "subscription provider refuses conflicting {key}: unset it so native uses your Claude login"
        )));
    }
    let bearer = format!("claude-sub-{}", hex_id());
    // The relay server is per-turn: bind now so the host gets a live port.
    // The admitted POST carries the Responses body; the handler translates,
    // spawns native, folds SSE and streams it back.
    let intent = relay::RelayIntent {
        model: model.clone(),
        effort: effort.clone(),
    };
    relays
        .lock()
        .map(|mut r| {
            r.insert(bearer.clone(), intent);
        })
        .ok();
    let relays2 = relays.clone();
    let bearer2 = bearer.clone();
    let (port, _handle) =
        relay::start_turn_server(relays2, bearer2).map_err(ProviderRpcError::Unavailable)?;
    // The handle is intentionally detached: the turn ends when the admitted
    // POST completes (or the sidecar exits); the OS reaps the listener.
    std::mem::forget(_handle);
    Ok(json!({
        "relay_url": format!("http://127.0.0.1:{port}/relay/{bearer}/responses"),
        "relay_token": bearer,
        "native_model": catalog::native_model(&model).map_err(ProviderRpcError::Unavailable)?,
    }))
}

fn hex_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    format!("{:08x}{:08x}", t, std::process::id())
}

fn ensure_provider(provider: &str, auth_method: &str) -> Result<(), ProviderRpcError> {
    if provider == manifest::PROVIDER_ID && auth_method == manifest::AUTH_METHOD_ID {
        Ok(())
    } else {
        Err(ProviderRpcError::Protocol(
            "unknown provider or auth method".into(),
        ))
    }
}

fn invalid(message: &'static str) -> ProviderRpcError {
    ProviderRpcError::Protocol(message.into())
}

fn error_value(error: ProviderRpcError) -> Value {
    match error {
        ProviderRpcError::Rpc(failure) => json!({
            "code": failure.code,
            "message": failure.message,
            "retryable": failure.retryable,
            "terminal": failure.terminal,
        }),
        ProviderRpcError::Protocol(message) => {
            json!({"code": "protocol", "message": message})
        }
        ProviderRpcError::Unavailable(message) => {
            json!({"code": "unavailable", "message": message})
        }
        ProviderRpcError::CapabilityMissing(message) => {
            json!({"code": "capability_missing", "message": message})
        }
    }
}
