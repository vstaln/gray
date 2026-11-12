//! Handlers for the non-turn request verbs. Turn verbs (`prompt`,
//! `command`, `steer`) live in mod.rs; everything here either answers from
//! state (status/list_*) or drives session machinery (set_model/resume).

use std::sync::atomic::Ordering;

use serde_json::{Value, json};

use crate::config::Config;
use crate::session_store::{JsonlSessionStore, default_root};

use super::{
    CURRENT_REQ, LAST_USAGE, Session, emit_result, emit_say, is_turn_active, lock,
    mark_turn_interrupted, sync_session,
};

/// `interrupt` — cancel the running turn, same token SIGINT cancels in the
/// REPL. Answers immediately (it's serviced from the reader thread too).
pub(crate) fn interrupt(id: &Value) {
    let taken = lock(&crate::repl::TURN_STATE).take();
    if let Some(token) = taken {
        mark_turn_interrupted();
        token.cancel();
        emit_result(id, true, json!({"interrupted": true}));
        return;
    }
    if !is_turn_active() {
        emit_result(id, true, json!({"interrupted": false}));
        return;
    }
    // A turn is running but hasn't registered its cancel token yet (agent
    // build / session init come first in run_prompt_turn). Watch for the
    // token on a plain thread — same Ctrl+C semantics: cancel as soon as
    // it exists, while the turn is still live.
    let id = id.clone();
    std::thread::spawn(move || {
        for _ in 0..200 {
            std::thread::sleep(std::time::Duration::from_millis(25));
            let taken = lock(&crate::repl::TURN_STATE).take();
            if let Some(token) = taken {
                mark_turn_interrupted();
                token.cancel();
                emit_result(&id, true, json!({"interrupted": true}));
                return;
            }
            if !is_turn_active() {
                break;
            }
        }
        emit_result(&id, true, json!({"interrupted": false}));
    });
}

/// `approve` — answer a pending `ask` event. `answers` maps each question
/// id to the picked option labels (the shape `ask` events carry).
pub(crate) fn approve(v: &Value, id: &Value) {
    let Some(ask_id) = v.get("ask_id").and_then(|a| a.as_u64()) else {
        emit_result(id, false, json!({"error": "approve needs ask_id"}));
        return;
    };
    let Some(answers) = v.get("answers").filter(|a| a.is_object()) else {
        emit_result(
            id,
            false,
            json!({"error": "approve needs answers: {<question_id>: [<label>, ...]}"}),
        );
        return;
    };
    let line = json!({"ask_id": ask_id, "answers": answers}).to_string();
    if crate::ask::deliver_json_answer(ask_id, &line) {
        emit_result(id, true, Value::Object(Default::default()));
    } else {
        emit_result(
            id,
            false,
            json!({"error": "answer not delivered: no open ask with that ask_id, or the answers do not match its questions"}),
        );
    }
}

/// `status` — session/model/context/usage snapshot.
pub(crate) fn status(sess: &mut Session, config: &Config, cwd: &std::path::Path, id: &Value) {
    let last = *lock(&LAST_USAGE);
    let window = config
        .model
        .as_deref()
        .map(crate::setup::resolve_model_context_length)
        .unwrap_or(0);
    let t = &sess.totals;
    let cache_pct = (t.input > 0).then(|| t.cache_read as f64 * 100.0 / t.input as f64);
    emit_result(
        id,
        true,
        json!({
            "status": {
                "session_id": sess.sid(),
                "cwd": cwd.display().to_string(),
                "model": config.model,
                "model_label": crate::setup::composite_label_for(config)
                    .or_else(|| crate::setup::selected_model_label(config)),
                "effort": config.thinking_effort,
                "fast_mode": config.fast_mode,
                "show_reasoning": config.show_reasoning,
                "provider_id": config.provider_id,
                "base_url": config.base_url,
                "credential_source": config.credential_source,
                "bare": config.bare,
                "unconfigured": sess.unconfigured,
                "turn_in_flight": crate::repl::TURN_IN_FLIGHT.load(Ordering::Relaxed),
                "queued_inputs": lock(&super::STEER_QUEUE).len(),
                "context_window": window,
                "compact_ceiling": crate::compact::effective_compact_ceiling(config),
                "context_used": last.as_ref().map(|u| u.input_tokens),
                "last_turn_usage": last,
                "totals": {
                    "turns": t.turns,
                    "input_tokens": t.input,
                    "output_tokens": t.output,
                    "cache_read_tokens": t.cache_read,
                    "cache_write_tokens": t.cache_write,
                    "cost_usd": t.cost,
                    "saved_usd": t.saved,
                    "cache_hit_pct": cache_pct,
                },
            }
        }),
    );
}

/// `list_commands` — builtin registry + forwardable plugin CLI commands +
/// in-process plugin hook commands (the same union /help prints).
pub(crate) fn list_commands(sess: &Session, id: &Value) {
    let mut out: Vec<Value> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for d in crate::repl::commands::REGISTRY.iter() {
        seen.insert(d.name.to_string());
        out.push(json!({
            "name": d.name,
            "description": d.desc,
            "aliases": d.aliases,
            "scope": d.scope,
            "surface": d.surface,
            "source": "builtin",
        }));
    }
    for (cmd, desc) in crate::plugin_cli::completions("") {
        let name = cmd.trim_start_matches('/').to_string();
        if seen.insert(name.clone()) {
            out.push(json!({
                "name": name,
                "description": desc,
                "source": "plugin",
            }));
        }
    }
    for h in sess
        .agent
        .as_ref()
        .map(|a| a.hooks().to_vec())
        .unwrap_or_default()
    {
        for c in h.commands() {
            let name = c.name.trim_start_matches('/').to_string();
            if seen.insert(name.clone()) {
                out.push(json!({
                    "name": name,
                    "description": "",
                    "turn_local_verbs": c.turn_local_verbs,
                    "source": "plugin",
                }));
            }
        }
    }
    emit_result(id, true, json!({"commands": out}));
}

/// `list_models` — the `/models` rail verbatim, via the same
/// `dump_model_catalog_json` that backs `gray --dump-models`.
pub(crate) async fn list_models(config: &Config, v: &Value, id: &Value) {
    let live = v.get("live").and_then(|a| a.as_bool()).unwrap_or(false);
    let mode = if live {
        crate::setup::DumpModels::Live
    } else {
        crate::setup::DumpModels::Cached
    };
    let config = config.clone();
    match tokio::task::spawn_blocking(move || crate::setup::dump_model_catalog_json(&config, mode))
        .await
    {
        Ok(catalog) => emit_result(id, true, json!({"catalog": catalog})),
        Err(e) => emit_result(id, false, json!({"error": format!("{e:#}")})),
    }
}

/// `set_model {provider?, model?, effort?}` — provider adoption goes through
/// `select_cli_provider` (the `--provider` path: scoped config + credentials,
/// in memory), model/effort through `apply_model_selection` (the `/models
/// <id>` path), then the agent is rebuilt like `handle_model` does.
pub(crate) async fn set_model(
    sess: &mut Session,
    config: &mut Config,
    cwd: &std::path::Path,
    tui: &crate::repl::TuiOpt,
    v: &Value,
    id: &Value,
) {
    let provider = v.get("provider").and_then(|p| p.as_str());
    let model = v.get("model").and_then(|m| m.as_str());
    let effort = v.get("effort").and_then(|e| e.as_str());
    if provider.is_none() && model.is_none() {
        emit_result(
            id,
            false,
            json!({"error": "set_model needs provider and/or model"}),
        );
        return;
    }
    if let Some(p) = provider
        && let Err(e) = crate::setup::select_cli_provider(config, p, model, false)
    {
        emit_result(id, false, json!({"error": format!("{e:#}")}));
        return;
    }
    // Effort alone applies onto the current model.
    let target = model
        .map(str::to_string)
        .or_else(|| effort.and_then(|_| config.model.clone()));
    if let Some(m) = target {
        match crate::repl::handlers::apply_model_selection(config, &m, effort, None) {
            Ok(applied) => {
                emit_say("info", &format!("✓ Model set to {}", applied.model));
                if let Some((old, new)) = &applied.clamped {
                    emit_say(
                        "warn",
                        &format!(
                            "Thinking effort clamped from {old} to {new} (not supported by this model)"
                        ),
                    );
                }
            }
            Err(msg) => {
                emit_result(id, false, json!({"error": msg}));
                return;
            }
        }
    }
    crate::config::export_provider_env(config);
    let sid = sess.sid();
    crate::repl::handlers::switch_agent_model(
        &mut sess.agent,
        config,
        cwd,
        sid.as_deref(),
        tui.as_ref().map(|(s, _)| s),
    )
    .await;
    sess.unconfigured = config.model.is_none();
    emit_result(
        id,
        true,
        json!({
            "model": config.model,
            "effort": config.thinking_effort,
            "provider_id": config.provider_id,
            "base_url": config.base_url,
            "credential_source": config.credential_source,
        }),
    );
}

/// `list_sessions {all?}` — recent saved sessions (this cwd unless `all`).
pub(crate) async fn list_sessions(v: &Value, id: &Value) -> anyhow::Result<()> {
    let all = v.get("all").and_then(|a| a.as_bool()).unwrap_or(false);
    let Some(root) = default_root() else {
        emit_result(id, false, json!({"error": "session store unavailable"}));
        return Ok(());
    };
    let store = JsonlSessionStore::new(root);
    let summaries = crate::resume::recent_summaries(&store, all).await;
    emit_result(id, true, json!({"sessions": summaries}));
    Ok(())
}

/// `resume {session|target, all?, last?}` — dispatches `ReplCommand::Resume`
/// through the real handler, so locking/history/model adoption match `/sessions`.
pub(crate) async fn resume(
    sess: &mut Session,
    config: &mut Config,
    cwd: &std::path::Path,
    tui: &crate::repl::TuiOpt,
    v: &Value,
    id: &Value,
) {
    let target = v
        .get("session")
        .or_else(|| v.get("session_id"))
        .or_else(|| v.get("target"))
        .and_then(|x| x.as_str())
        .map(str::to_string);
    let last = v.get("last").and_then(|a| a.as_bool()).unwrap_or(false);
    let all = v.get("all").and_then(|a| a.as_bool()).unwrap_or(false);
    if target.is_none() && !last {
        // Bare resume in the headless REPL prints the recent-session list;
        // here the client asks for it explicitly instead.
        emit_result(
            id,
            false,
            json!({"error": "resume needs session (or last:true)"}),
        );
        return;
    }
    let expected = if let (Some(t), Some(root)) = (target.as_deref(), default_root()) {
        let store = JsonlSessionStore::new(root);
        crate::resume::resolve_prefix(&store, t, all)
            .await
            .map(|s| s.as_str().to_string())
            .or_else(|| Some(t.to_string()))
    } else {
        None
    };
    *lock(&CURRENT_REQ) = Some(id.clone());
    let r = crate::repl::dispatch::dispatch_command(
        crate::repl::ReplCommand::Resume(crate::repl::ResumeArgs {
            target: target.clone(),
            last,
            all,
            dismiss: false,
        }),
        &mut sess.agent,
        config,
        cwd,
        tui,
        &mut sess.state,
        &mut sess.totals,
        &mut sess.pending_command,
        &mut sess.pending_history,
        &mut sess.unconfigured,
        &mut sess.hide_thinking,
    )
    .await;
    *lock(&CURRENT_REQ) = None;
    sync_session(sess);
    match r {
        Err(e) => emit_result(id, false, json!({"error": format!("{e:#}")})),
        Ok(_) => {
            let ok = expected
                .as_deref()
                .map(|e| sess.sid().as_deref() == Some(e))
                .unwrap_or_else(|| sess.state.is_some());
            emit_result(
                id,
                ok,
                json!({"session_id": sess.sid(), "model": config.model}),
            );
        }
    }
}
