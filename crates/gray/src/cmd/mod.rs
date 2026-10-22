mod registry;
mod store;
mod ui;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::CmdVerb;
use crate::config::Config;
use crate::repl::commands::{CmdSurface, resolve};
use crate::repl::usage_panel;
use crate::repl::{SessionTotals, handlers};
use crate::session_store::{JsonlSessionStore, SessionId};
use crate::setup::variants::{Parts, RowShape};

#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum CommandResult {
    Text {
        text: String,
        close: bool,
    },
    Ui {
        token: String,
        document: ui::UiDoc,
        fallback_text: String,
    },
    Prompt {
        prompt: String,
        fallback_text: String,
    },
}

impl CommandResult {
    fn text(text: impl Into<String>) -> Self {
        Self::Text {
            text: text.into(),
            close: true,
        }
    }
}

#[derive(Debug)]
struct CmdError(&'static str, String);

impl CmdError {
    fn unknown(name: &str) -> Self {
        Self("unknown_command", format!("unknown command '{name}'"))
    }
    fn local(name: &str) -> Self {
        Self(
            "local_only",
            format!("'/{name}' needs the interactive TUI — run it in a gray session"),
        )
    }
    fn args(msg: impl Into<String>) -> Self {
        Self("invalid_args", msg.into())
    }
    fn unavailable(msg: impl Into<String>) -> Self {
        Self("unavailable", msg.into())
    }
    fn expired() -> Self {
        Self("expired", "interaction expired — re-run the command".into())
    }
    fn invalid_action(msg: impl Into<String>) -> Self {
        Self("invalid_action", msg.into())
    }
    fn not_owner() -> Self {
        Self("not_owner", "interaction belongs to another user".into())
    }
}

fn ok(result: Value) -> Value {
    json!({"ok": true, "protocol": 1, "result": result})
}

fn fail(e: CmdError) -> Value {
    json!({"ok": false, "error": {"code": e.0, "message": e.1, "retryable": false}})
}

pub async fn run_cli(verb: CmdVerb, config: &mut Config) -> anyhow::Result<()> {
    let out = match verb {
        CmdVerb::List => ok(registry::list_json()),
        CmdVerb::Run { name, args } => match gray_home() {
            Ok(home) => match run_command(config, &home, &name, &args).await {
                Ok(res) => ok(serde_json::to_value(res)?),
                Err(e) => fail(e),
            },
            Err(e) => fail(CmdError::unavailable(format!("{e:#}"))),
        },
        CmdVerb::Interact { token, action } => match gray_home() {
            Ok(home) => match interact(config, &home, &token, action.as_deref()) {
                Ok(res) => ok(serde_json::to_value(res)?),
                Err(e) => fail(e),
            },
            Err(e) => fail(CmdError::unavailable(format!("{e:#}"))),
        },
    };
    println!("{out}");
    Ok(())
}

fn gray_home() -> anyhow::Result<PathBuf> {
    crate::setup::gray_home()
}

async fn run_command(
    config: &mut Config,
    home: &Path,
    name: &str,
    args: &[String],
) -> Result<CommandResult, CmdError> {
    let rest = args.join(" ");
    if let Some(def) = resolve(name) {
        if def.surface == CmdSurface::Local {
            return Err(CmdError::local(def.name));
        }
        return match def.name {
            "help" => Ok(help_result()),
            "usage" => usage_result(config, home).await,
            "models" => model_result(config, home, &rest),
            "new" => new_result(home).await,
            "sessions" => sessions_result(home, &rest).await,
            "skills" => skills_result(&rest),
            "context" => context_result(config, home, &rest).await,
            "compact" => compact_result(config, home, &rest).await,
            "undo" => undo_result(home, false).await,
            "retry" => undo_result(home, true).await,
            "jobs" => jobs_result(home, &rest),
            "plugin" => plugin_result(&rest).await,
            "plugins" => Ok(CommandResult::text(crate::plugin_status::table().await)),
            "update" => update_result(&rest).await,
            "agentsmd" => agentsmd_result(&rest),
            "reload" => Ok(CommandResult::text(
                "config reloads on the next turn in this conversation",
            )),
            "gateway" => gateway_result(&rest),
            other => Err(CmdError::unavailable(format!(
                "'/{other}' is not implemented over cmd yet"
            ))),
        };
    }
    if let Some(result) = plugin_command_result(home, name, args).await? {
        return Ok(result);
    }
    if let Some(result) = skill_command_result(name, &rest) {
        return result;
    }
    Err(CmdError::unknown(name))
}

fn help_result() -> CommandResult {
    let mut out = String::new();
    for d in crate::repl::REGISTRY {
        out.push_str(&crate::repl::commands::format_help_line(d));
        out.push('\n');
    }
    let mut seen: std::collections::HashSet<String> = crate::repl::REGISTRY
        .iter()
        .map(|d| d.name.to_string())
        .collect();
    for (name, description) in crate::plugin_cli::completions("") {
        if seen.insert(name.clone()) {
            out.push_str(&format!("  /{name:<10} {description}\n"));
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        let templates = crate::prompt_templates::discover(&cwd);
        if !templates.is_empty() {
            out.push_str("prompt templates:\n");
            for t in templates {
                if seen.insert(t.name.clone()) {
                    out.push_str(&format!("  /{:<10} {}\n", t.name, t.description));
                }
            }
        }
    }
    CommandResult::text(out.trim_end())
}

async fn usage_result(config: &Config, home: &Path) -> Result<CommandResult, CmdError> {
    let store = JsonlSessionStore::new(home.join("sessions"));
    let summaries = store.list().await;
    let pointed = pointed_session(home).filter(|sid| summaries.iter().any(|s| s.id == *sid));
    let id =
        pointed.or_else(|| crate::resume::latest_summary(&summaries, None).map(|s| s.id.clone()));
    let Some(id) = id else {
        return Ok(CommandResult::text(
            "no sessions yet — usage appears after the first turn",
        ));
    };
    let (meta, entries) = store
        .load(&id)
        .await
        .map_err(|e| CmdError::unavailable(format!("cannot load session {id}: {e:#}")))?;
    let model = if meta.model.is_empty() {
        config.model.clone().unwrap_or_default()
    } else {
        meta.model
    };
    let totals = SessionTotals::from_entries(&entries, &model);
    if totals.turns == 0 {
        return Ok(CommandResult::text(
            "no turns yet this session — usage appears after the first turn",
        ));
    }
    let input = usage_panel::PanelInput {
        totals: &totals,
        model: &model,
        rate: crate::setup::get_model_rate(&model),
        warmth: usage_panel::Warmth::Unknown,
    };
    let mut text = format!("✓ Session usage — {}", usage_panel::usage_header(&input));
    for line in usage_panel::usage_plain(&input) {
        text.push('\n');
        text.push_str(&line);
    }
    Ok(CommandResult::text(text))
}

fn pointed_session(home: &Path) -> Option<SessionId> {
    std::fs::read(home.join("session.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|v| {
            v.get("session_id")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .filter(|s| crate::session_store::valid_session_id(s))
        .map(SessionId::new)
}

fn model_result(config: &mut Config, home: &Path, rest: &str) -> Result<CommandResult, CmdError> {
    let raw = rest.trim();
    if !raw.is_empty() {
        return model_direct(config, home, raw);
    }
    let models = model_list_for(config);
    if models.is_empty() {
        return Ok(CommandResult::text(
            "no provider configured — run /models to set up provider & key",
        ));
    }
    let mut draft = store::ModelDraft {
        model: config
            .model
            .clone()
            .or_else(|| models.first().map(|(id, _)| id.clone())),
        effort: config
            .thinking_effort
            .clone()
            .or_else(|| Some("high".to_string())),
        fast: config.fast_mode.unwrap_or(false),
        parts: Default::default(),
    };
    let page = draft
        .model
        .as_deref()
        .and_then(|m| models.iter().position(|(id, _)| id == m))
        .map(|i| i / MAX_SELECT_OPTIONS)
        .unwrap_or(0);

    let mut shapes = BTreeMap::new();
    for (id, _) in &models {
        if let Some(shape) = crate::setup::variants::row_shape(id).filter(|s| s.is_composite()) {
            shapes.insert(id.clone(), (*shape).clone());
        }
    }

    if let Some(shape) = draft.model.as_deref().and_then(|id| shapes.get(id)) {
        draft.parts = if draft.model.as_deref() == config.model.as_deref() {
            crate::setup::selection_parts(shape, &config.model_parts)
        } else {
            shape.settle_parts(None)
        };
    }
    let pending = store::PendingDoc {
        kind: "model_picker".to_string(),
        created_at: crate::util::now_secs(),
        user: None,
        draft: draft.clone(),
        models: models.clone(),
        shapes,
        page,
    };
    let token = store::put(home, &pending)
        .map_err(|e| CmdError::unavailable(format!("cannot store pending doc: {e}")))?;
    Ok(CommandResult::Ui {
        token,
        document: model_picker_doc(&draft, &models, &pending.shapes, page),
        fallback_text: model_picker_fallback(&draft, &models, &pending.shapes),
    })
}

fn model_direct(config: &mut Config, home: &Path, raw: &str) -> Result<CommandResult, CmdError> {
    let path = home.join("config.json");
    let mut tokens = raw.split_whitespace();
    let id = tokens.next().unwrap_or_default();
    let mut picks = Parts::new();
    for t in tokens {
        match t.split_once('=') {
            Some((k, v)) if !k.is_empty() && !v.is_empty() => {
                picks.insert(k.to_string(), v.to_string());
            }
            _ => {
                return Err(CmdError::args(format!(
                    "usage: /models <id> [slot=option ...] — bad token '{t}'"
                )));
            }
        }
    }
    let prev = config.model.clone();
    if picks.is_empty() {
        let applied = handlers::apply_model_selection_at(config, id, None, None, Some(&path))
            .map_err(CmdError::args)?;

        let shape = crate::setup::variants::row_shape(&applied.model).filter(|s| s.is_composite());
        let parts = shape
            .as_ref()
            .map(|s| opening_parts(prev.as_deref(), config, home, &applied.model, s))
            .unwrap_or_default();
        commit_parts(config, home, &parts);
        let name =
            crate::setup::composite_label_for(config).unwrap_or_else(|| applied.model.clone());
        return Ok(CommandResult::text(model_set_text(&applied, &name)));
    }

    let (id, shape) = match crate::setup::variants::row_shape(id).filter(|s| s.is_composite()) {
        Some(shape) => (id.to_string(), shape),
        None => {
            let models = model_list_for(config);
            let canonical =
                crate::setup::validate_direct_model_id(id, &models).map_err(CmdError::args)?;
            match crate::setup::variants::row_shape(&canonical).filter(|s| s.is_composite()) {
                Some(shape) => (canonical, shape),
                None => {
                    return Err(CmdError::args(format!("'{canonical}' has no slots to set")));
                }
            }
        }
    };
    for (k, v) in &picks {
        let Some(slot) = shape.slots.iter().find(|s| &s.key == k) else {
            let keys = shape
                .slots
                .iter()
                .map(|s| s.key.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            return Err(CmdError::args(format!(
                "unknown slot '{k}' — {id} slots: {keys}"
            )));
        };
        if !slot.options.iter().any(|o| &o.id == v) {
            let opts = slot
                .options
                .iter()
                .map(|o| o.id.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            return Err(CmdError::args(format!(
                "unknown {k} option '{v}' — options: {opts}"
            )));
        }
    }

    let baseline = opening_parts(prev.as_deref(), config, home, &id, &shape);
    let satisfies =
        |v: &gray_plugin::ModelVariant| picks.iter().all(|(k, val)| v.parts.get(k) == Some(val));
    let drift = |v: &gray_plugin::ModelVariant| {
        shape
            .slots
            .iter()
            .filter(|s| !picks.contains_key(&s.key) && v.parts.get(&s.key) != baseline.get(&s.key))
            .count()
    };
    let Some(variant) = shape
        .variants
        .iter()
        .filter(|v| satisfies(v))
        .min_by_key(|v| drift(v))
    else {
        let combo = picks
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join(" ");
        return Err(CmdError::args(format!(
            "no {id} variant serves {combo} — check the slot options"
        )));
    };
    let parts = variant.parts.clone();
    let applied = handlers::apply_model_selection_at(config, &id, None, None, Some(&path))
        .map_err(CmdError::args)?;
    commit_parts(config, home, &parts);
    let name = crate::setup::composite_label_for(config).unwrap_or_else(|| applied.model.clone());
    Ok(CommandResult::text(model_set_text(&applied, &name)))
}

fn opening_parts(
    current: Option<&str>,
    config: &Config,
    home: &Path,
    row: &str,
    shape: &RowShape,
) -> Parts {
    let want = if current == Some(row) {
        Some(config.model_parts.clone())
    } else {
        let saved = crate::setup::load_saved_config_at(&home.join("config.json"));
        saved
            .parts_memory
            .get(&crate::setup::effort_memory_key(
                &config.provider_id,
                &config.base_url,
                row,
            ))
            .cloned()
    };
    shape.settle_parts(want.as_ref())
}

fn commit_parts(config: &mut Config, home: &Path, parts: &Parts) {
    config.model_parts = parts.clone();
    let path = home.join("config.json");
    let _cfg_lock = crate::setup::lock_saved_config_at(&path).ok();
    let mut saved = crate::setup::load_saved_config_at(&path);
    let key = crate::setup::effort_memory_key(
        &config.provider_id,
        &config.base_url,
        config.model.as_deref().unwrap_or_default(),
    );
    saved.remember_parts(&key, parts);
    let _ = crate::setup::save_saved_config_at(&path, &saved);
}

fn composite_name(
    row: &str,
    models: &[(String, String)],
    shape: &RowShape,
    parts: &Parts,
    effort: Option<&str>,
) -> Option<String> {
    let name = models
        .iter()
        .find(|(id, _)| id == row)
        .map(|(_, n)| n.as_str())
        .filter(|n| !n.is_empty())
        .unwrap_or(row);
    crate::setup::variants::composite_label_parts(shape, name, parts, effort)
        .map(|segs| segs.into_iter().map(|(t, _)| t).collect())
}

fn model_list_for(config: &Config) -> Vec<(String, String)> {
    let (_, list_key, _) = crate::setup::picker_scope(config);
    let cached = crate::setup::saved_models_for(&list_key, &config.base_url);
    if !cached.is_empty() {
        return cached;
    }
    let (_, rows) = crate::setup::provider_models_for_config(config);
    if !rows.is_empty() {
        return rows;
    }
    config
        .model
        .clone()
        .map(|m| vec![(m, String::new())])
        .unwrap_or_default()
}

const MAX_SELECT_OPTIONS: usize = 25;

fn cap_options(models: &mut Vec<(String, String)>, keep: Option<&str>) {
    if models.len() <= MAX_SELECT_OPTIONS {
        return;
    }
    if let Some(sel) = keep
        && let Some(pos) = models.iter().position(|(id, _)| id == sel)
    {
        let row = models.remove(pos);
        models.insert(0, row);
    }
    models.truncate(MAX_SELECT_OPTIONS);
}

async fn new_result(home: &Path) -> Result<CommandResult, CmdError> {
    let state_path = home.join("session.json");
    if let Ok(bytes) = std::fs::read(&state_path)
        && let Ok(mut state) = serde_json::from_slice::<Value>(&bytes)
        && state.is_object()
    {
        state["session_id"] = Value::Null;
        state["generation"] = Value::String(uuid::Uuid::new_v4().simple().to_string());
        state["last_activity"] = json!(crate::util::now_secs());
        let _ = crate::util::atomic_write_json(&state_path, &state);
    }

    let store = JsonlSessionStore::new(home.join("sessions"));
    for cwd in [std::env::current_dir().ok(), Some(home.join("work"))]
        .into_iter()
        .flatten()
    {
        store.forget(&cwd).await;
    }
    Ok(CommandResult::text(
        "new conversation — the next message opens a fresh session",
    ))
}

async fn sessions_result(home: &Path, rest: &str) -> Result<CommandResult, CmdError> {
    if !rest.trim().is_empty() {
        return Err(CmdError::args(
            "usage: /sessions — resuming is the adapter's `gray -p --session <id>`",
        ));
    }
    let store = JsonlSessionStore::new(home.join("sessions"));
    let summaries = crate::resume::recent_summaries(&store, true).await;
    if summaries.is_empty() {
        return Ok(CommandResult::text("no saved sessions"));
    }
    let mut out = String::new();
    for s in &summaries {
        out.push_str(&crate::resume::format_summary_row(s));
        out.push('\n');
    }
    Ok(CommandResult::text(out.trim_end()))
}

fn unknown_skill_text(name: &str) -> String {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let discovered = crate::skills::discover_skills(&cwd);
    let needle = name.to_lowercase();
    let mut ranked: Vec<(&str, usize)> = discovered
        .skills
        .iter()
        .map(|s| {
            let n = s.name.to_lowercase();
            let score = if n.contains(&needle) || needle.contains(&n) {
                0
            } else if n.starts_with(&needle[..needle.len().min(3)]) {
                1
            } else {
                usize::MAX
            };
            (s.name.as_str(), score)
        })
        .collect();
    ranked.retain(|(_, score)| *score != usize::MAX);
    ranked.sort_by_key(|(n, score)| (*score, n.to_string()));
    let close: Vec<&str> = ranked.iter().take(5).map(|(n, _)| *n).collect();
    if close.is_empty() {
        format!("no skill '{name}' — `gray cmd run skills` lists the discovered names")
    } else {
        format!("no skill '{name}' — closest matches: {}", close.join(", "))
    }
}

fn skills_result(rest: &str) -> Result<CommandResult, CmdError> {
    let rest = rest.trim();
    if !rest.is_empty() {
        let (name, args) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
        return skill_command_result(name.trim(), args)
            .unwrap_or_else(|| Err(CmdError::args(unknown_skill_text(name))));
    }
    let cwd = std::env::current_dir()
        .map_err(|e| CmdError::unavailable(format!("cannot resolve cwd: {e}")))?;
    let discovered = crate::skills::discover_skills(&cwd);
    if discovered.skills.is_empty() {
        return Ok(CommandResult::text("no skills discovered"));
    }
    let auto = crate::setup::skills_auto_enabled();
    let disabled = crate::setup::disabled_skill_names();
    let mut out = String::new();
    if !auto {
        out.push_str("skills auto-load is off (/skills on to re-enable)\n");
    }
    for s in &discovered.skills {
        let mut row = crate::skills::format_discovered_skill_row_full(s);
        if !auto {
            row.push_str(" [auto-off]");
        } else if disabled.contains(&s.name) {
            row.push_str(" [disabled]");
        }
        out.push_str(&row);
        out.push('\n');
    }
    Ok(CommandResult::text(out.trim_end()))
}

async fn session_messages(home: &Path) -> Option<(SessionId, Vec<gray_core::message::Message>)> {
    let id = pointed_session(home)?;
    let store = JsonlSessionStore::new(home.join("sessions"));
    let (_meta, entries) = store.load(&id).await.ok()?;
    Some((id, entries.into_iter().map(|e| e.message).collect()))
}

async fn context_result(
    config: &mut Config,
    home: &Path,
    rest: &str,
) -> Result<CommandResult, CmdError> {
    let val = rest.trim();
    if val.is_empty() || val == "status" || val == "show" {
        let cwd = std::env::current_dir()
            .map_err(|e| CmdError::unavailable(format!("cannot resolve cwd: {e}")))?;
        let messages = session_messages(home)
            .await
            .map(|(_, m)| m)
            .unwrap_or_default();
        let parts = crate::repl::status::collect_context_parts(&cwd, &messages, None);
        return Ok(CommandResult::text(
            crate::repl::status::context_breakdown_text(config, &parts),
        ));
    }
    let (msg, _ok) = crate::repl::status::apply_context_arg(config, val);
    Ok(CommandResult::text(msg))
}

async fn compact_result(
    config: &Config,
    home: &Path,
    rest: &str,
) -> Result<CommandResult, CmdError> {
    let Some((id, messages)) = session_messages(home).await else {
        return Ok(CommandResult::text(
            "no active session in this conversation — nothing to compact",
        ));
    };
    if messages.is_empty() {
        return Ok(CommandResult::text(
            "nothing to compact (conversation is empty)",
        ));
    }
    let cwd = std::env::current_dir()
        .map_err(|e| CmdError::unavailable(format!("cannot resolve cwd: {e}")))?;
    let mut agent = crate::build_model_agent(config, &cwd, Some(id.as_str()))
        .await
        .map_err(|e| CmdError::unavailable(format!("{e:#}")))?
        .with_messages(messages);
    let replaced = agent.messages().len();
    let summary = crate::compact::compact_with_keep(
        &mut agent,
        (!rest.trim().is_empty()).then(|| rest.trim()),
        crate::setup::user_keep_recent_tokens(),
    )
    .await
    .map_err(|e| CmdError::unavailable(format!("compaction failed: {e:#}")))?;
    let Some(summary) = summary else {
        return Ok(CommandResult::text(
            "nothing to compact (conversation already fits in context)",
        ));
    };
    let store = JsonlSessionStore::new(home.join("sessions"));
    store
        .append_compaction_replacement(&id, agent.messages())
        .await
        .map_err(|e| CmdError::unavailable(format!("cannot record compaction: {e:#}")))?;
    Ok(CommandResult::text(format!(
        "Context compacted ({replaced} messages → summary)\n{summary}"
    )))
}

async fn undo_result(home: &Path, retry: bool) -> Result<CommandResult, CmdError> {
    let Some((id, messages)) = session_messages(home).await else {
        return Ok(CommandResult::text(
            "no active session in this conversation — nothing to undo",
        ));
    };
    let Some(cut) = handlers::undo_cut(&messages) else {
        return Ok(CommandResult::text(
            "nothing to undo (no turn of your own to drop)",
        ));
    };
    let resent = messages[cut].text_content();
    let store = JsonlSessionStore::new(home.join("sessions"));
    store
        .rewind(&id, cut)
        .await
        .map_err(|e| CmdError::unavailable(format!("undo failed: {e}")))?;
    let dropped = messages.len() - cut;
    let note = format!("undid the last turn · {dropped} messages out of context");
    if !retry {
        return Ok(CommandResult::text(note));
    }
    if resent.trim().is_empty() {
        return Ok(CommandResult::text(format!(
            "{note} — the dropped turn had no text to retry"
        )));
    }
    Ok(CommandResult::Prompt {
        prompt: resent,
        fallback_text: format!("{note} — retrying it now"),
    })
}

fn jobs_result(home: &Path, rest: &str) -> Result<CommandResult, CmdError> {
    let sid = pointed_session(home).map(|s| s.as_str().to_string());
    let mut toks = rest.split_whitespace();
    match toks.next() {
        None | Some("list") => {
            let wakes = crate::repl::jobs::load_wakes(home, sid.as_deref());
            Ok(CommandResult::text(crate::repl::jobs::dashboard(
                &[],
                &wakes,
                crate::cron::now_secs(),
            )))
        }
        Some("stop") | Some("rm") | Some("remove") => {
            let target = toks.next().unwrap_or("").trim();
            let bare = target
                .trim_start_matches("wake:")
                .trim_start_matches("job:");
            if bare.is_empty() {
                return Err(CmdError::args("usage: /jobs stop <wake id>"));
            }
            let store = crate::cron::CronStore::open(home.join("cron"))
                .map_err(|e| CmdError::unavailable(format!("{e:#}")))?;
            match store.remove(bare) {
                Ok(true) => Ok(CommandResult::text(format!("stopped {bare}"))),
                Ok(false) => Ok(CommandResult::text(format!(
                    "unknown job or wake {bare:?} (in-flight jobs live in the running turn)"
                ))),
                Err(e) => Err(CmdError::unavailable(format!("stop failed: {e:#}"))),
            }
        }
        Some(_) => Err(CmdError::args("usage: /jobs [stop <wake id>]")),
    }
}

async fn plugin_result(rest: &str) -> Result<CommandResult, CmdError> {
    use crate::repl::plugin_cmds::PluginAction;
    let action = crate::repl::plugin_cmds::parse_plugin_args(&format!("plugin {rest}"))
        .map_err(|e| CmdError::args(format!("{e:#}")))?;
    match action {
        PluginAction::List => match crate::plugin_cli::list_rows() {
            Ok(rows) if rows.is_empty() => Ok(CommandResult::text("no plugins installed")),
            Ok(rows) => {
                let out = rows
                    .iter()
                    .map(|r| {
                        let state = if r.on { "" } else { " [disabled]" };
                        let kind = if r.cli { " [command]" } else { "" };
                        format!("{} {} ({}){state}{kind}", r.name, r.version, r.scope)
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                Ok(CommandResult::text(out))
            }
            Err(e) => Err(CmdError::unavailable(format!("plugin list failed: {e:#}"))),
        },
        PluginAction::Update(target) => {
            let mut check = false;
            let mut parts = Vec::new();
            for tok in target.split_whitespace() {
                if tok == "--check" {
                    check = true;
                } else {
                    parts.push(tok);
                }
            }
            let target = if parts.is_empty() {
                "all".to_string()
            } else {
                parts.join(" ")
            };
            let res = if check {
                crate::plugin_cli::check_managed(&target).await
            } else {
                crate::plugin_cli::update_managed(&target).await
            };
            match res {
                Ok(reports) => {
                    let mut lines = Vec::new();
                    for r in &reports {
                        if matches!(r.action, gray_pkg::ops::UpdateAction::Current) {
                            continue;
                        }
                        lines.push(format!("{}: {}", r.name, r.detail()));
                    }
                    Ok(CommandResult::text(if lines.is_empty() {
                        "up to date".to_string()
                    } else {
                        lines.join("\n")
                    }))
                }
                Err(e) => Err(CmdError::unavailable(format!("update failed: {e:#}"))),
            }
        }
        PluginAction::Enable(name) => match crate::plugin_cli::set_managed_enabled(&name, true) {
            Ok(()) => Ok(CommandResult::text(format!("enabled {name}"))),
            Err(e) => Err(CmdError::unavailable(format!("enable failed: {e:#}"))),
        },
        PluginAction::Disable(name) => match crate::plugin_cli::set_managed_enabled(&name, false) {
            Ok(()) => Ok(CommandResult::text(format!("disabled {name}"))),
            Err(e) => Err(CmdError::unavailable(format!("disable failed: {e:#}"))),
        },
        PluginAction::Check(dir) => {
            let exe = crate::exe_watch::on_disk_exe()
                .map_err(|e| CmdError::unavailable(format!("cannot self-spawn: {e}")))?;
            let out = tokio::time::timeout(
                std::time::Duration::from_secs(60),
                tokio::process::Command::new(exe)
                    .args(["plugin", "check", &dir])
                    .stdin(std::process::Stdio::null())
                    .output(),
            )
            .await
            .map_err(|_| CmdError::unavailable("plugin check timed out".to_string()))?
            .map_err(|e| CmdError::unavailable(format!("plugin check failed to start: {e}")))?;
            let text = String::from_utf8_lossy(&out.stdout).trim_end().to_string();
            if out.status.success() {
                Ok(CommandResult::text(text))
            } else {
                Err(CmdError::unavailable(format!(
                    "check failed:\n{}",
                    if text.is_empty() {
                        String::from_utf8_lossy(&out.stderr).trim_end().to_string()
                    } else {
                        text
                    }
                )))
            }
        }
        PluginAction::Install(_) | PluginAction::Remove(_) => Ok(CommandResult::text(
            "installing or removing plugins needs a shell — run `gray plugin <install|remove> …`",
        )),
    }
}

async fn update_result(rest: &str) -> Result<CommandResult, CmdError> {
    let t = rest.trim();
    if t == "auto" || t.starts_with("auto ") {
        let word = t.trim_start_matches("auto").trim();
        return match word {
            "on" | "off" => match crate::auto_update::set_enabled(word == "on") {
                Ok(()) if word == "on" => Ok(CommandResult::text("auto-update on")),
                Ok(()) => Ok(CommandResult::text(
                    "auto-update off — `gray update` still installs manually",
                )),
                Err(e) => Err(CmdError::unavailable(format!(
                    "auto-update toggle failed: {e:#}"
                ))),
            },
            "status" | "" => Ok(CommandResult::text(crate::auto_update::status_text())),
            other => Err(CmdError::args(format!(
                "unknown `/update auto {other}` (on|off|status)"
            ))),
        };
    }
    match t {
        "" | "status" | "check" => {
            let current = env!("CARGO_PKG_VERSION");
            let latest = tokio::time::timeout(
                std::time::Duration::from_secs(15),
                crate::update::latest_version(),
            )
            .await
            .map_err(|_| CmdError::unavailable("update check timed out".to_string()))?
            .map_err(|e| CmdError::unavailable(format!("update check failed: {e:#}")))?;
            if crate::update::update_available(
                crate::update::CHANNEL,
                &latest,
                current,
                env!("GRAY_BUILD_ID"),
            ) {
                Ok(CommandResult::text(format!(
                    "gray {latest} available (you have {current}) — /update install to take it"
                )))
            } else {
                Ok(CommandResult::text(format!("gray {current} is current")))
            }
        }
        "install" | "now" | "run" => {
            let exe = crate::exe_watch::on_disk_exe()
                .map_err(|e| CmdError::unavailable(format!("cannot self-spawn: {e}")))?;
            let out = tokio::time::timeout(
                std::time::Duration::from_secs(120),
                tokio::process::Command::new(exe)
                    .arg("update")
                    .stdin(std::process::Stdio::null())
                    .output(),
            )
            .await
            .map_err(|_| CmdError::unavailable("update timed out".to_string()))?
            .map_err(|e| CmdError::unavailable(format!("update failed to start: {e}")))?;
            let text = String::from_utf8_lossy(&out.stdout).trim_end().to_string();
            if out.status.success() {
                Ok(CommandResult::text(if text.is_empty() {
                    "updated — restart gray (and this bridge) to run it".to_string()
                } else {
                    text
                }))
            } else {
                Err(CmdError::unavailable(format!(
                    "update failed:\n{}",
                    if text.is_empty() {
                        String::from_utf8_lossy(&out.stderr).trim_end().to_string()
                    } else {
                        text
                    }
                )))
            }
        }
        other => Err(CmdError::args(format!(
            "unknown `/update {other}` (status|install|auto on|off|status)"
        ))),
    }
}

fn agentsmd_result(rest: &str) -> Result<CommandResult, CmdError> {
    match rest.trim() {
        "" | "show" | "status" => {
            let path =
                crate::sys_prompt_path().map_err(|e| CmdError::unavailable(format!("{e:#}")))?;
            match crate::load_or_create_system_prompt_at(&path) {
                Ok(body) => Ok(CommandResult::text(format!(
                    "system prompt: {}\n---\n{body}\n---",
                    path.display()
                ))),
                Err(e) => Ok(CommandResult::text(format!(
                    "failed to read {}: {e}",
                    path.display()
                ))),
            }
        }
        other => Err(CmdError::args(format!(
            "unknown `/agentsmd {other}` over cmd — `show` works; edit/reset need the TUI"
        ))),
    }
}

fn gateway_result(rest: &str) -> Result<CommandResult, CmdError> {
    match rest.trim() {
        "" | "status" | "list" => Ok(CommandResult::text(
            crate::plugin_cli::list_rows()
                .unwrap_or_default()
                .iter()
                .map(|r| {
                    crate::setup::format_plugin_row_parts(
                        &r.name, &r.version, &r.scope, &r.ecosystem, r.on,
                    )
                })
                .collect::<Vec<_>>()
                .join("\n"),
        )),
        other => Err(CmdError::args(format!(
            "unknown `/gateway {other}` over cmd — status only here"
        ))),
    }
}

async fn plugin_command_result(
    home: &Path,
    name: &str,
    args: &[String],
) -> Result<Option<CommandResult>, CmdError> {
    let Some((owner, argv)) = find_command_owner(home, name) else {
        return Ok(None);
    };
    use gray_plugin::Plugin as _;
    let sidecar = gray_plugin::sidecar::SidecarPlugin::spawn(argv)
        .await
        .map_err(|e| CmdError::unavailable(format!("{owner} failed to spawn: {e:#}")))?;
    let outcome = sidecar
        .run_command(&format!("/{name}"), args.to_vec())
        .await;
    sidecar.shutdown(std::time::Duration::from_secs(2)).await;
    match outcome {
        Some(gray_plugin::CommandOutcome::Say(text)) => Ok(Some(CommandResult::text(text))),
        Some(gray_plugin::CommandOutcome::Prompt(prompt)) => Ok(Some(CommandResult::Prompt {
            prompt,
            fallback_text: format!("/{name} asked for a turn"),
        })),
        Some(gray_plugin::CommandOutcome::Panel(spec)) => Ok(Some(CommandResult::text(
            crate::repl::pager::spec_plain(&spec),
        ))),
        Some(gray_plugin::CommandOutcome::Picker(spec)) => Ok(Some(CommandResult::text(
            crate::repl::picker::spec_plain(&spec),
        ))),
        Some(gray_plugin::CommandOutcome::ModelPicker(_))
        | Some(gray_plugin::CommandOutcome::Terminal { text: None, .. }) => {
            Err(CmdError::local(name))
        }
        Some(gray_plugin::CommandOutcome::Terminal {
            text: Some(text), ..
        }) => Ok(Some(CommandResult::text(text))),
        None => Err(CmdError::local(name)),
    }
}

fn find_command_owner(home: &Path, name: &str) -> Option<(String, Vec<String>)> {
    let mut homes = vec![(home.to_path_buf(), false)];
    if let Some(main) =
        gray_core::paths::gray_user_home().filter(|u| crate::providers::registry::differs(home, u))
    {
        homes.push((main, true));
    }
    let want = format!("/{name}");
    for (h, global_only) in homes {
        let Ok(lock) = gray_plugin::lock::LockFile::load(&gray_plugin::lock::lock_path(&h)) else {
            continue;
        };
        for (plugin, entry) in &lock.plugins {
            if global_only && !entry.global {
                continue;
            }
            if !crate::plugin_cli::enabled(&h, plugin, entry) {
                continue;
            }
            let Ok(raw) = std::fs::read(h.join("plugins").join(format!("{plugin}-manifest.json")))
            else {
                continue;
            };
            let Ok(m) = serde_json::from_slice::<Value>(&raw) else {
                continue;
            };
            let claims = |key: &str| {
                m[key]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .any(|c| c == want)
            };
            if !claims("commands") && !claims("subcommands") {
                continue;
            }
            let argv = if !entry.argv.is_empty() {
                entry.argv.clone()
            } else {
                match gray_plugin::builder::resolve_argv(&h.join("plugins").join(plugin)) {
                    Ok(a) => a,
                    Err(_) => continue,
                }
            };
            return Some((plugin.clone(), argv));
        }
    }
    None
}

fn skill_command_result(name: &str, args_raw: &str) -> Option<Result<CommandResult, CmdError>> {
    let cwd = std::env::current_dir().ok()?;
    let discovered = crate::skills::discover_skills(&cwd);
    let skill = discovered.skills.iter().find(|s| s.name == name)?;
    let args = (!args_raw.trim().is_empty()).then(|| args_raw.trim().to_string());
    Some((|| {
        crate::skills::validate_skill_args(skill, args.as_deref()).map_err(CmdError::args)?;
        let content = std::fs::read_to_string(&skill.file_path).map_err(|e| {
            CmdError::unavailable(format!("failed to read {}: {e}", skill.file_path.display()))
        })?;
        let body = crate::skills_tool::strip_frontmatter(&content);
        let prompt = handlers::format_skill_paste(body, &skill.name, args.as_deref());
        Ok(CommandResult::Prompt {
            prompt,
            fallback_text: format!("Running the \"{}\" skill.", skill.name),
        })
    })())
}

#[derive(Debug, Default, serde::Deserialize)]
struct Action {
    #[serde(default)]
    user: Option<String>,
    #[serde(default)]
    values: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    submit: Option<String>,
}

fn interact(
    config: &mut Config,
    home: &Path,
    token: &str,
    action_json: Option<&str>,
) -> Result<CommandResult, CmdError> {
    let mut rec = store::get(home, token, crate::util::now_secs())?;
    if rec.kind != "model_picker" {
        return Err(CmdError::invalid_action(format!(
            "unknown pending doc kind '{}'",
            rec.kind
        )));
    }
    let action: Option<Action> = match action_json {
        None => None,
        Some(raw) => Some(
            serde_json::from_str(raw)
                .map_err(|e| CmdError::invalid_action(format!("bad action json: {e}")))?,
        ),
    };
    if let Some(a) = &action {
        match (&rec.user, &a.user) {
            (Some(owner), Some(u)) if u != owner => return Err(CmdError::not_owner()),
            (Some(_), None) => return Err(CmdError::not_owner()),
            (None, Some(u)) => rec.user = Some(u.clone()),
            _ => {}
        }
        for (k, vals) in &a.values {
            let v = vals.last().map(String::as_str).unwrap_or("");
            match k.as_str() {
                "model" => {
                    if !rec.models.iter().any(|(id, _)| id == v) {
                        return Err(CmdError::invalid_action(format!(
                            "unknown model option '{v}'"
                        )));
                    }
                    rec.draft.model = Some(v.to_string());

                    rec.draft.parts = rec
                        .shapes
                        .get(v)
                        .map(|s| s.settle_parts(None))
                        .unwrap_or_default();
                }
                "effort" => {
                    if !["low", "medium", "high", "max"].contains(&v) {
                        return Err(CmdError::invalid_action(format!("unknown effort '{v}'")));
                    }
                    rec.draft.effort = Some(v.to_string());
                }
                _ if k.starts_with("slot:") => {
                    let key = &k["slot:".len()..];
                    let Some(shape) = rec
                        .draft
                        .model
                        .as_deref()
                        .and_then(|m| rec.shapes.get(m))
                        .filter(|s| s.is_composite())
                    else {
                        return Err(CmdError::invalid_action(format!("unknown control '{k}'")));
                    };

                    let cur = shape.settle_parts(Some(&rec.draft.parts));
                    match shape.option_parts(&cur, key, v) {
                        Some(parts) => rec.draft.parts = parts,
                        None => {
                            return Err(CmdError::invalid_action(format!(
                                "no variant serves {key}={v} here"
                            )));
                        }
                    }
                }
                other => {
                    return Err(CmdError::invalid_action(format!(
                        "unknown control '{other}'"
                    )));
                }
            }
        }
    }
    match action.and_then(|a| a.submit) {
        None => {
            store::save(home, token, &rec).map_err(save_err)?;
            Ok(revised_doc(token, &rec))
        }
        Some(submit) => match submit.as_str() {
            "fast" => {
                rec.draft.fast = !rec.draft.fast;
                store::save(home, token, &rec).map_err(save_err)?;
                Ok(revised_doc(token, &rec))
            }
            "apply" => {
                let Some(model) = rec.draft.model.clone() else {
                    return Err(CmdError::invalid_action("no model selected"));
                };

                let shape = rec.shapes.get(&model).filter(|s| s.is_composite());
                let parts = match shape {
                    Some(s) => {
                        if crate::setup::variants::resolve_variant(
                            &s.variants,
                            rec.draft.effort.as_deref(),
                            rec.draft.fast,
                            &rec.draft.parts,
                        )
                        .is_none()
                        {
                            let combo = rec
                                .draft
                                .parts
                                .iter()
                                .map(|(k, v)| format!("{k}={v}"))
                                .collect::<Vec<_>>()
                                .join(" ");
                            return Ok(CommandResult::Text {
                                text: format!(
                                    "no {model} variant serves {combo} — adjust the slot selections"
                                ),
                                close: false,
                            });
                        }
                        rec.draft.parts.clone()
                    }
                    None => Parts::new(),
                };
                match handlers::apply_model_selection_at(
                    config,
                    &model,
                    rec.draft.effort.as_deref(),
                    Some(rec.draft.fast),
                    Some(&home.join("config.json")),
                ) {
                    Ok(applied) => {
                        commit_parts(config, home, &parts);
                        store::remove(home, token);
                        let name = shape
                            .and_then(|s| {
                                composite_name(
                                    &model,
                                    &rec.models,
                                    s,
                                    &parts,
                                    config.thinking_effort.as_deref(),
                                )
                            })
                            .unwrap_or_else(|| applied.model.clone());
                        Ok(CommandResult::text(model_set_text(&applied, &name)))
                    }

                    Err(msg) => Ok(CommandResult::Text {
                        text: msg,
                        close: false,
                    }),
                }
            }
            "page:prev" | "page:next" | "page:info" => {
                let total = rec.models.len().div_ceil(MAX_SELECT_OPTIONS).max(1);
                match submit.as_str() {
                    "page:prev" => rec.page = rec.page.saturating_sub(1),
                    "page:next" => rec.page = (rec.page + 1).min(total - 1),
                    _ => {}
                }
                store::save(home, token, &rec).map_err(save_err)?;
                Ok(revised_doc(token, &rec))
            }
            "cancel" => {
                store::remove(home, token);
                Ok(CommandResult::text("Cancelled."))
            }
            other => Err(CmdError::invalid_action(format!(
                "unknown submit '{other}'"
            ))),
        },
    }
}

fn save_err(e: std::io::Error) -> CmdError {
    CmdError::unavailable(format!("cannot save pending doc: {e}"))
}

fn revised_doc(token: &str, rec: &store::PendingDoc) -> CommandResult {
    CommandResult::Ui {
        token: token.to_string(),
        document: model_picker_doc(&rec.draft, &rec.models, &rec.shapes, rec.page),
        fallback_text: model_picker_fallback(&rec.draft, &rec.models, &rec.shapes),
    }
}

fn model_set_text(applied: &handlers::ModelApply, name: &str) -> String {
    let mut text = format!("✓ Model set to {name}");
    if let Some((old, new)) = &applied.clamped {
        text.push_str(&format!(
            "\nThinking effort clamped from {old} to {new} (not supported by this model)"
        ));
    }
    text
}

fn clip(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

fn picker_header(draft: &store::ModelDraft) -> String {
    format!(
        "**Model** — `{}` · effort `{}` · fast `{}`",
        draft.model.as_deref().unwrap_or("-"),
        draft.effort.as_deref().unwrap_or("-"),
        if draft.fast { "on" } else { "off" },
    )
}

fn model_picker_doc(
    draft: &store::ModelDraft,
    models: &[(String, String)],
    shapes: &BTreeMap<String, RowShape>,
    page: usize,
) -> ui::UiDoc {
    let total_pages = models.len().div_ceil(MAX_SELECT_OPTIONS).max(1);
    let page = page.min(total_pages - 1);
    let start = page * MAX_SELECT_OPTIONS;
    let mut window: Vec<(String, String)> =
        models[start..(start + MAX_SELECT_OPTIONS).min(models.len())].to_vec();
    if let Some(sel) = draft.model.as_deref()
        && !window.iter().any(|(id, _)| id == sel)
        && let Some(row) = models.iter().find(|(id, _)| id == sel)
    {
        if window.len() >= MAX_SELECT_OPTIONS {
            window.pop();
        }
        window.insert(0, row.clone());
    }
    let model_options = window
        .iter()
        .map(|(id, name)| ui::SelectOption {
            label: clip(if name.is_empty() { id } else { name }, 100),
            value: id.clone(),
            description: (!name.is_empty() && name != id).then(|| clip(id, 100)),
            default: draft.model.as_deref() == Some(id.as_str()),
        })
        .collect();
    let effort_options = ["low", "medium", "high", "max"]
        .iter()
        .map(|e| ui::SelectOption {
            label: e.to_string(),
            value: e.to_string(),
            description: None,
            default: draft.effort.as_deref() == Some(*e),
        })
        .collect();
    let mut components = vec![
        ui::MessageNode::Text {
            content: picker_header(draft),
        },
        ui::MessageNode::ActionRow {
            children: vec![ui::MessageNode::StringSelect {
                logical_id: "model".to_string(),
                placeholder: Some("model".to_string()),
                options: model_options,
                min_values: None,
                max_values: None,
            }],
        },
    ];

    if let Some(shape) = draft_shape(draft, shapes) {
        for slot in &shape.slots {
            components.push(slot_select_row(
                slot,
                draft.parts.get(&slot.key).map(String::as_str),
            ));
        }
    }
    components.push(ui::MessageNode::ActionRow {
        children: vec![ui::MessageNode::StringSelect {
            logical_id: "effort".to_string(),
            placeholder: Some("effort".to_string()),
            options: effort_options,
            min_values: None,
            max_values: None,
        }],
    });
    if total_pages > 1 {
        components.push(ui::MessageNode::ActionRow {
            children: vec![
                ui::MessageNode::Button {
                    logical_id: "page:prev".to_string(),
                    label: "◀ Prev".to_string(),
                    style: ui::ButtonStyle::Secondary,
                    url: None,
                    disabled: page == 0,
                },
                ui::MessageNode::Button {
                    logical_id: "page:info".to_string(),
                    label: format!("Page {}/{}", page + 1, total_pages),
                    style: ui::ButtonStyle::Secondary,
                    url: None,
                    disabled: true,
                },
                ui::MessageNode::Button {
                    logical_id: "page:next".to_string(),
                    label: "Next ▶".to_string(),
                    style: ui::ButtonStyle::Secondary,
                    url: None,
                    disabled: page + 1 >= total_pages,
                },
            ],
        });
    }
    components.push(ui::MessageNode::ActionRow {
        children: vec![
            ui::MessageNode::Button {
                logical_id: "apply".to_string(),
                label: "Apply".to_string(),
                style: ui::ButtonStyle::Primary,
                url: None,
                disabled: false,
            },
            ui::MessageNode::Button {
                logical_id: "fast".to_string(),
                label: format!("⚡ fast: {}", if draft.fast { "on" } else { "off" }),
                style: ui::ButtonStyle::Secondary,
                url: None,
                disabled: false,
            },
            ui::MessageNode::Button {
                logical_id: "cancel".to_string(),
                label: "Cancel".to_string(),
                style: ui::ButtonStyle::Secondary,
                url: None,
                disabled: false,
            },
        ],
    });
    ui::UiDoc::message(
        "model-picker",
        ui::Visibility::Ephemeral,
        ui::Lifecycle::Editable,
        components,
    )
}

fn draft_shape<'a>(
    draft: &store::ModelDraft,
    shapes: &'a BTreeMap<String, RowShape>,
) -> Option<&'a RowShape> {
    draft
        .model
        .as_deref()
        .and_then(|id| shapes.get(id))
        .filter(|s| s.is_composite())
}

fn slot_select_row(slot: &gray_plugin::ModelSlot, current: Option<&str>) -> ui::MessageNode {
    let mut options: Vec<(String, String)> = slot
        .options
        .iter()
        .map(|o| (o.id.clone(), o.name.clone()))
        .collect();
    let current = current.filter(|id| slot.options.iter().any(|o| o.id == *id));
    let keep = current.or_else(|| slot.options.first().map(|o| o.id.as_str()));
    cap_options(&mut options, keep);
    ui::MessageNode::ActionRow {
        children: vec![ui::MessageNode::StringSelect {
            logical_id: format!("slot:{}", slot.key),
            placeholder: Some(slot.label.clone()),
            options: options
                .into_iter()
                .map(|(id, name)| ui::SelectOption {
                    label: clip(if name.is_empty() { &id } else { &name }, 100),
                    value: id.clone(),
                    description: (!name.is_empty() && name != id).then(|| clip(&id, 100)),
                    default: keep == Some(id.as_str()),
                })
                .collect(),
            min_values: None,
            max_values: None,
        }],
    }
}

fn model_picker_fallback(
    draft: &store::ModelDraft,
    models: &[(String, String)],
    shapes: &BTreeMap<String, RowShape>,
) -> String {
    let mut out = picker_header(draft);
    for (i, (id, name)) in models.iter().enumerate() {
        let label = if name.is_empty() {
            id.clone()
        } else {
            format!("{name} ({id})")
        };
        out.push_str(&format!("\n{}. {}", i + 1, label));
    }

    if let Some(shape) = draft_shape(draft, shapes) {
        for slot in &shape.slots {
            out.push_str(&format!("\n{}:", slot.label.to_lowercase()));
            for (i, o) in slot.options.iter().enumerate() {
                let name = if o.name.is_empty() { &o.id } else { &o.name };
                out.push_str(&format!("\n  {}. {name} ({})", i + 1, o.id));
            }
        }
    }
    out.push_str("\neffort: low | medium | high | max");
    let picks = draft_shape(draft, shapes)
        .map(|shape| {
            shape
                .slots
                .iter()
                .map(|s| format!("[{}=<opt>]", s.key))
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default();
    out.push_str(&format!(
        "\nreply: model <id> {picks}[effort <level>] [fast on|off]"
    ));
    out
}

#[path = "cmd_tests.rs"]
#[cfg(test)]
mod tests;
