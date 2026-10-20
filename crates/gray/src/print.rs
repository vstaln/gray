use std::collections::HashMap;
use std::io::{ErrorKind, Write};
use std::path::Path;
use std::time::{Duration, Instant};

use crate::session_store::{JsonlSessionStore, SessionId, SessionMeta};
use gray_core::agent::ToolContext;
use gray_core::event::AgentEvent;
use gray_core::input::{InputEnvelope, InputError};
use gray_core::message::Message;
use gray_core::redaction::{redact_for_disclosure, redact_message};

use crate::build_agent;
use crate::config::Config;

pub(crate) use gray_core::spill::now_millis;

#[derive(Debug, Clone, Default)]
pub struct ActiveToolCall {
    pub name: String,
    pub args: Option<serde_json::Value>,

    pub label: Option<String>,

    pub preview: Option<String>,
}

/// Wrap `text` in an SGR style only when stdout is a terminal and NO_COLOR is
/// unset — piped `gray -p` output must stay plain text.
fn sgr(open: &str, text: impl std::fmt::Display) -> String {
    static ANSI: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let on = *ANSI.get_or_init(|| {
        use std::io::IsTerminal as _;
        std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none()
    });
    if on {
        format!("{open}{text}\x1b[0m")
    } else {
        text.to_string()
    }
}

/// Status chrome (token footer, retries, compaction): dimmed inline on a
/// terminal; on stderr when stdout is piped so `$(gray -p …)` is the answer only.
fn meta<W: Write>(w: &mut W, text: String) -> std::io::Result<()> {
    use std::io::IsTerminal as _;
    if std::io::stdout().is_terminal() {
        writeln!(w, "\n{}", sgr("\x1b[2m", text))
    } else {
        eprintln!("{text}");
        Ok(())
    }
}

pub fn render_event_with_context<W: Write>(
    w: &mut W,
    event: &AgentEvent,
    cwd: Option<&Path>,
    in_flight: &mut HashMap<String, ActiveToolCall>,
    tool_labels: &HashMap<String, String>,
    tool_previews: &HashMap<String, String>,
) -> std::io::Result<()> {
    match event {
        AgentEvent::Start => Ok(()),
        AgentEvent::TextDelta { delta } => {
            write!(w, "{delta}")?;
            w.flush()
        }
        AgentEvent::ThinkingDelta { delta } => {
            write!(w, "{}", sgr("\x1b[2m\x1b[3m", delta))?;
            w.flush()
        }
        AgentEvent::ToolCallStart { id, name } => {
            in_flight.insert(
                id.clone(),
                ActiveToolCall {
                    label: tool_labels.get(name).cloned(),
                    preview: tool_previews.get(name).cloned(),
                    name: name.clone(),
                    args: None,
                },
            );
            Ok(())
        }
        AgentEvent::ToolCallProgress { id, name, .. } => {
            let label = tool_labels.get(name).cloned();
            let preview = tool_previews.get(name).cloned();
            in_flight.entry(id.clone()).or_insert(ActiveToolCall {
                name: name.clone(),
                args: None,
                label,
                preview,
            });
            Ok(())
        }
        AgentEvent::ToolCallEnd { id, args } => {
            let entry = in_flight.entry(id.clone()).or_insert_with(|| {
                let label = args
                    .get("label")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|t| !t.is_empty())
                    .map(str::to_string);
                ActiveToolCall {
                    name: "tool".to_string(),
                    args: None,
                    label,
                    preview: None,
                }
            });
            entry.args = Some(args.clone());
            if entry.name.is_empty() {
                entry.name = "tool".to_string();
            }
            let name = entry.name.as_str();
            let owned;
            let args = match entry.label.as_deref() {
                Some(label) => {
                    owned = crate::tool_fmt::with_tool_label(args, Some(label));
                    &owned
                }
                None => args,
            };

            let preview_path = entry
                .preview
                .as_deref()
                .or_else(|| tool_previews.get(name).map(String::as_str));
            writeln!(
                w,
                "\n{}",
                crate::tool_fmt::format_tool_call_lines_plain(name, args, cwd, preview_path)
            )?;
            w.flush()
        }
        AgentEvent::ToolResult {
            id,
            output,
            is_error,
            ..
        } => {
            let tool = in_flight.remove(id).unwrap_or_default();
            let name = if tool.name.is_empty() {
                "tool"
            } else {
                tool.name.as_str()
            };
            let owned;
            let args = match tool.label.as_deref() {
                Some(label) => match tool.args.as_ref() {
                    Some(a) => {
                        owned = crate::tool_fmt::with_tool_label(a, Some(label));
                        Some(&owned)
                    }
                    None => None,
                },
                None => tool.args.as_ref(),
            };
            let res = crate::tool_fmt::format_tool_result_plain_with_context(
                name, args, output, *is_error, cwd,
            );
            if !res.is_empty() {
                write!(w, "{res}")?;
            }
            w.flush()
        }
        AgentEvent::StepUsage { .. } => Ok(()),

        AgentEvent::Compacted {
            tokens_before,
            tokens_after,
            messages_before,
            messages_after,
            reason,
        } => {
            meta(
                w,
                format!(
                    "\u{21bb} compacted {} \u{2192} {} tok ({} \u{2192} {} messages){}",
                    crate::repl::fmt_usage(*tokens_before),
                    crate::repl::fmt_usage(*tokens_after),
                    messages_before,
                    messages_after,
                    crate::compact::compact_reason_note(*reason),
                ),
            )?;
            w.flush()
        }

        AgentEvent::StreamError { message, details } => {
            let message = scrub_error_text(message);
            let details = scrub_error_text(details);
            if details.is_empty() {
                meta(w, format!("⚠ {message}"))?;
            } else {
                meta(w, format!("⚠ {message}\n  {details}"))?;
            }
            w.flush()
        }

        AgentEvent::MessageBoundary => {
            write!(w, "\n\n")?;
            w.flush()
        }
        AgentEvent::Steered { .. } | AgentEvent::Retract => Ok(()),
        AgentEvent::TurnEnd { usage, .. } => {
            if usage.total() > 0 {
                meta(
                    w,
                    format!(
                        "{} {} tok",
                        crate::tui::glyph("\u{2b22}", "#"),
                        crate::repl::fmt_usage(usage.total())
                    ),
                )?;
            }
            w.flush()
        }
    }
}

pub async fn run_print_mode(config: &Config, prompt: &str) -> anyhow::Result<()> {
    run_print_mode_with_session(config, prompt, None, false).await
}

pub async fn run_print_mode_with_session(
    config: &Config,
    prompt: &str,
    session: Option<&str>,
    continue_last: bool,
) -> anyhow::Result<()> {
    run_print_inner(
        config,
        Some(prompt),
        Message::user(prompt),
        session,
        continue_last,
        None,
    )
    .await
}

pub async fn run_print_mode_json(
    config: &Config,
    prompt: &str,
    session: Option<&str>,
    continue_last: bool,
    max_requests: Option<u32>,
    input_price: Option<f64>,
    output_price: Option<f64>,
) -> anyhow::Result<()> {
    run_print_mode_json_message(
        config,
        Message::user(prompt),
        session,
        continue_last,
        max_requests,
        input_price,
        output_price,
    )
    .await
}

pub async fn run_print_mode_json_input(
    config: &Config,
    input: &InputEnvelope,
    session: Option<&str>,
    continue_last: bool,
    max_requests: Option<u32>,
    input_price: Option<f64>,
    output_price: Option<f64>,
) -> anyhow::Result<()> {
    run_print_mode_json_message(
        config,
        Message::structured_input(input.clone()),
        session,
        continue_last,
        max_requests,
        input_price,
        output_price,
    )
    .await
}

async fn run_print_mode_json_message(
    config: &Config,
    user_message: Message,
    session: Option<&str>,
    continue_last: bool,
    max_requests: Option<u32>,
    input_price: Option<f64>,
    output_price: Option<f64>,
) -> anyhow::Result<()> {
    let mut output = JsonOutput {
        turn_id: uuid::Uuid::new_v4().to_string(),
        session_id: None,
        text: String::new(),
        usage: gray_core::event::Usage::default(),
        meter: None,
        tools: HashMap::new(),
        thinking: String::new(),
        show_reasoning: config.show_reasoning.unwrap_or(true),
        stream_text: stream_text_enabled(),
        segment: TextSegment::default(),
        labels: HashMap::new(),
        previews: HashMap::new(),
    };
    let result = match crate::print_meter::Meter::new(
        max_requests.unwrap_or(32),
        input_price,
        output_price,
        config.max_cost_micros,
    ) {
        Ok(meter) => {
            output.meter = Some(meter);
            run_print_inner(
                config,
                None,
                user_message,
                session,
                continue_last,
                Some(&mut output),
            )
            .await
        }
        Err(error) => Err(error),
    };

    for row in output.take_text(true) {
        output.write(row)?;
    }
    let mut row = match &result {
        Ok(()) => serde_json::json!({"type": "result", "text": output.text, "usage": output.usage}),
        Err(error) => {
            let failure = PrintFailure::of(error);
            let mut row = serde_json::json!({"type": "error", "code": failure.code,
                "retryable": failure.retryable, "message": failure.message()});
            if let Some(hint) = failure.hint {
                row["hint"] = hint.into();
            }

            if let Some(crate::session_store::SessionError::Locked { pid, .. }) =
                error.downcast_ref::<crate::session_store::SessionError>()
            {
                row["message"] = error.to_string().into();
                if let Some(pid) = pid {
                    row["lockHolderPid"] = (*pid).into();
                }
            }

            let detail = scrub_error_text(&format!("{error:#}"));
            if !detail.is_empty() && row["message"] != detail {
                row["detail"] = detail.into();
            }
            row
        }
    };
    if let Some(meter) = &output.meter {
        row["accounting"] = serde_json::to_value(meter.snapshot())?;
    }
    output.write(row)?;
    match result {
        Ok(()) => Ok(()),
        Err(error) => Err(PrintFailure::of(&error).into()),
    }
}

pub fn write_structured_input_error(error: &InputError) {
    let row = serde_json::json!({
        "protocol": 1,
        "type": "error",
        "code": "invalid_input",
        "retryable": false,
        "message": error.to_string(),
    });
    println!("{row}");
}

pub const EXIT_TURN_FAILED: i32 = 1;
pub const EXIT_INFRA: i32 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrintFailure {
    pub code: &'static str,
    pub retryable: bool,
    pub exit: i32,
    pub hint: Option<&'static str>,
}

impl PrintFailure {
    pub fn of(error: &anyhow::Error) -> Self {
        if matches!(
            error.downcast_ref::<crate::session_store::SessionError>(),
            Some(crate::session_store::SessionError::Locked { .. })
        ) {
            return Self {
                code: "session_locked",
                retryable: true,
                exit: EXIT_INFRA,
                hint: Self::hint_for("session_locked"),
            };
        }
        let Some(core) = error.downcast_ref::<gray_core::error::CoreError>() else {
            return Self {
                code: "turn_failed",
                retryable: false,
                exit: EXIT_TURN_FAILED,
                hint: None,
            };
        };
        let retryable = core.retryable();
        Self {
            code: core.code(),
            retryable,
            exit: if retryable {
                EXIT_INFRA
            } else {
                EXIT_TURN_FAILED
            },
            hint: Self::hint_for(core.code()),
        }
    }

    pub fn message(&self) -> &'static str {
        match self.code {
            "auth_failed" => "The provider rejected the credentials.",
            "rate_limited" => "The provider is rate limiting this account.",
            "bad_request" => "The provider rejected the request as malformed.",
            "context_overflow" => "The conversation outgrew the context window.",
            "server_error" => "The provider failed on its side.",
            "stream_broken" => "The provider dropped the response stream.",
            "connection_failed" => "Could not reach the provider.",
            "timeout" => "The provider did not answer in time.",
            "loop_detected" => "Agent stopped on a repeated-tool loop.",
            "cancelled" => "Cancelled.",
            "serialization" => "Serialization failure.",
            "session_locked" => "The session is already open in another process.",
            "quota_exhausted" => "The provider's usage allowance is exhausted.",
            _ => {
                "Agent turn failed; actions may already have occurred. Do not automatically retry."
            }
        }
    }

    fn hint_for(code: &str) -> Option<&'static str> {
        Some(match code {
            "auth_failed" => {
                "Check the API key for this provider: `gray setup` (or `/models` in the REPL)."
            }
            "rate_limited" => "Back off and retry, or switch provider/models with `/models`.",
            "bad_request" => {
                "Often a model id the endpoint does not serve; `/models` shows what is configured."
            }
            "context_overflow" => "Start `/new` or run `/compact`.",
            "connection_failed" => {
                "Check the base URL and network; `gray setup` re-points the provider."
            }
            "timeout" => "Retry, or raise the request timeout for slow providers.",
            "server_error" | "stream_broken" => "Retrying the turn usually succeeds.",
            "session_locked" => "Close the other instance, or open a different session.",
            "quota_exhausted" => {
                "Wait for the quota window to reset, or switch models with `/models`."
            }
            _ => return None,
        })
    }
}

impl std::fmt::Display for PrintFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message())?;
        if let Some(hint) = self.hint {
            write!(f, "\n  hint: {hint}")?;
        }
        write!(f, "\n  (exit code {})", self.exit)
    }
}

impl std::error::Error for PrintFailure {}

const DETAIL_CAP: usize = 240;
const OUTPUT_CAP: usize = 1200;
const THINKING_FLUSH: usize = 800;

const TEXT_EVERY: Duration = Duration::from_millis(200);

struct JsonOutput {
    turn_id: String,
    session_id: Option<String>,
    text: String,
    usage: gray_core::event::Usage,
    meter: Option<crate::print_meter::Meter>,

    tools: HashMap<String, String>,

    thinking: String,
    show_reasoning: bool,

    stream_text: bool,
    segment: TextSegment,

    labels: HashMap<String, String>,

    previews: HashMap<String, String>,
}

#[derive(Default)]
struct TextSegment {
    index: u64,

    raw: String,

    sent: usize,

    tail: String,
    last_emit: Option<Instant>,
}

fn scrub(text: &str) -> String {
    let redaction = redact_for_disclosure(text);
    if redaction.has_secret() {
        redaction.into_text()
    } else {
        text.to_string()
    }
}

fn stream_text_enabled() -> bool {
    std::env::var("GRAY_STREAM_TEXT").is_ok_and(|v| {
        matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

/// `GRAY_PRINT_WAIT_BACKGROUND=0` restores exit-at-turn-end for `-p`.
fn background_wait_enabled() -> bool {
    !matches!(
        std::env::var("GRAY_PRINT_WAIT_BACKGROUND").as_deref(),
        Ok("0" | "false" | "off")
    )
}

fn steer_enabled() -> bool {
    std::env::var("GRAY_STDIN_STEER").is_ok_and(|v| {
        matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

impl JsonOutput {
    fn display_name(&self, name: &str) -> String {
        self.labels
            .get(name)
            .cloned()
            .unwrap_or_else(|| crate::tool_fmt::humanize_tool_name(name))
    }

    fn write(&self, mut row: serde_json::Value) -> std::io::Result<()> {
        row["protocol"] = 1.into();
        row["turn_id"] = self.turn_id.clone().into();
        row["session_id"] = self.session_id.clone().into();
        let mut out = std::io::stdout().lock();
        writeln!(out, "{row}")?;
        out.flush()
    }

    fn event(&mut self, event: &AgentEvent) -> std::io::Result<()> {
        for row in self.rows(event) {
            self.write(row)?;
        }
        Ok(())
    }

    fn rows(&mut self, event: &AgentEvent) -> Vec<serde_json::Value> {
        if let AgentEvent::ThinkingDelta { delta } = event {
            if self.show_reasoning {
                self.thinking.push_str(delta);
                if self.thinking.chars().count() >= THINKING_FLUSH {
                    return self.take_thinking();
                }
            }
            return Vec::new();
        }
        if let AgentEvent::TextDelta { delta } = event {
            if !self.stream_text {
                return Vec::new();
            }
            self.segment.raw.push_str(delta);
            let due = self
                .segment
                .last_emit
                .is_none_or(|at| at.elapsed() >= TEXT_EVERY);
            return if due {
                self.take_text(false)
            } else {
                Vec::new()
            };
        }

        if let AgentEvent::Retract = event {
            let seg = self.segment.index;
            self.segment = TextSegment {
                index: seg + 1,
                ..TextSegment::default()
            };
            return vec![serde_json::json!({"type": "retract", "segment": seg})];
        }

        let mut rows = match event {
            AgentEvent::ToolCallStart { .. }
            | AgentEvent::MessageBoundary
            | AgentEvent::StepUsage { .. }
            | AgentEvent::TurnEnd { .. } => self.take_text(true),
            _ => Vec::new(),
        };
        if matches!(event, AgentEvent::StepUsage { .. }) {
            return rows;
        }
        let mut row = serde_json::json!({"type": "progress"});
        let phase = match event {
            AgentEvent::Start => "generating",
            AgentEvent::ToolCallStart { id, name } => {
                self.tools.insert(id.clone(), name.clone());
                row["tool"] = name.as_str().into();
                row["label"] = self.display_name(name).into();
                row["call_id"] = disclose(id, DETAIL_CAP).into();
                "tool_started"
            }
            AgentEvent::ToolCallEnd { id, args } => {
                if let Some(name) = self.tools.get(id) {
                    row["tool"] = name.as_str().into();
                    row["label"] = self.display_name(name).into();
                    row["call_id"] = disclose(id, DETAIL_CAP).into();

                    let preview = self
                        .previews
                        .get(name)
                        .map(|x| x.trim())
                        .filter(|x| !x.is_empty())
                        .and_then(|path| crate::tool_fmt::preview_at(args, path));

                    if let Some(detail) = tool_detail(name, args, preview.as_deref()) {
                        row["detail"] = detail.into();
                    }
                }
                "tool_ran"
            }
            AgentEvent::ToolResult {
                id,
                output,
                is_error,
            } => {
                if let Some(name) = self.tools.remove(id) {
                    row["label"] = self.display_name(&name).into();
                    row["tool"] = name.into();
                }
                if !id.is_empty() {
                    row["call_id"] = disclose(id, DETAIL_CAP).into();
                }
                if *is_error {
                    row["error"] = true.into();
                }
                let output = disclose_output(output, OUTPUT_CAP);
                if !output.is_empty() {
                    row["output"] = output.into();
                }
                "tool_finished"
            }
            AgentEvent::StreamError { message, .. } => {
                row["detail"] = disclose(message, DETAIL_CAP).into();
                "provider_retry"
            }
            AgentEvent::Steered { text } => {
                row["detail"] = disclose(text, DETAIL_CAP).into();
                "steered"
            }
            AgentEvent::Compacted { .. } => "compacted",
            AgentEvent::TurnEnd { usage, .. } => {
                self.usage.accumulate(usage);
                rows.append(&mut self.take_thinking());
                "persisting"
            }
            _ => return rows,
        };
        row["phase"] = phase.into();
        rows.push(row);
        rows
    }

    fn take_text(&mut self, done: bool) -> Vec<serde_json::Value> {
        let segment = &mut self.segment;
        if segment.raw.is_empty() {
            return Vec::new();
        }
        let stable = if done {
            segment.raw.len()
        } else {
            segment.raw.rfind('\n').map_or(segment.sent, |i| i + 1)
        };
        let delta = scrub(&segment.raw[segment.sent..stable]);
        let tail = if done {
            String::new()
        } else {
            let open = &segment.raw[stable..];

            let shown = open.rfind(char::is_whitespace).map_or("", |i| &open[..i]);
            scrub(shown)
        };
        if !done && delta.is_empty() && tail == segment.tail {
            return Vec::new();
        }
        let row = serde_json::json!({
            "type": "progress", "phase": "text", "segment": segment.index,
            "delta": delta, "tail": tail, "done": done,
        });
        if done {
            *segment = TextSegment {
                index: segment.index + 1,
                ..TextSegment::default()
            };
        } else {
            segment.sent = stable;
            segment.tail = tail;
            segment.last_emit = Some(Instant::now());
        }
        vec![row]
    }

    fn take_thinking(&mut self) -> Vec<serde_json::Value> {
        let text = std::mem::take(&mut self.thinking);
        if text.trim().is_empty() {
            return Vec::new();
        }
        vec![serde_json::json!({
            "type": "progress", "phase": "thinking", "detail": disclose(&text, DETAIL_CAP)
        })]
    }
}

fn disclose(text: &str, cap: usize) -> String {
    let flat: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let capped: String = flat.chars().take(cap).collect();
    let capped = if flat.chars().count() > cap {
        format!("{capped}…")
    } else {
        capped
    };
    let redaction = redact_for_disclosure(&capped);
    if redaction.has_secret() {
        redaction.into_text()
    } else {
        capped
    }
}

fn disclose_output(text: &str, cap: usize) -> String {
    let look_ahead = cap.saturating_add(256);
    let mut chars = text.chars();
    let prefix = chars.by_ref().take(look_ahead).collect::<String>();
    let source_truncated = chars.next().is_some();
    let redaction = redact_for_disclosure(&prefix);

    let base = if redaction.has_secret() {
        redaction.into_text()
    } else {
        prefix
    };
    if !source_truncated && base.chars().count() <= cap {
        return base;
    }
    format!("{}…", base.chars().take(cap).collect::<String>())
}

fn tool_detail(name: &str, args: &serde_json::Value, preview: Option<&str>) -> Option<String> {
    let arg = |k: &str| args.get(k).and_then(serde_json::Value::as_str);

    if let Some(text) = preview {
        return Some(disclose(text, DETAIL_CAP));
    }
    match name {
        "bash" | "shell" => arg("command").map(|c| disclose(c, DETAIL_CAP)),
        "read" | "view" | "cat" => {
            let path = arg("path")?;

            let (Some(start), Some(limit)) = (
                args.get("offset").and_then(|v| v.as_u64()),
                args.get("limit").and_then(|v| v.as_u64()),
            ) else {
                return Some(disclose(path, DETAIL_CAP));
            };
            if start == 0 || limit == 0 {
                return Some(disclose(path, DETAIL_CAP));
            }
            Some(disclose(
                &format!("{path} L{start}-{}", start + limit - 1),
                DETAIL_CAP,
            ))
        }
        "write" | "edit" | "apply_patch" | "create" | "str_replace" => {
            arg("path").map(|p| disclose(p, DETAIL_CAP))
        }
        "skill" | "use_skill" => arg("skill")
            .or_else(|| arg("name"))
            .map(|n| disclose(n, DETAIL_CAP)),
        "web_search" => arg("query").map(|q| disclose(q, DETAIL_CAP)),
        "web_fetch" => arg("url").map(|u| disclose(u, DETAIL_CAP)),
        _ => None,
    }
}

struct RunningGuard(Option<std::path::PathBuf>);

#[cfg(unix)]
fn parent_id() -> u32 {
    std::os::unix::process::parent_id()
}

#[cfg(not(unix))]
fn parent_id() -> u32 {
    0
}

impl RunningGuard {
    fn register(session_id: &str, task: &str, model: &str, compact_ceiling: usize) -> Self {
        let Ok(home) = crate::setup::gray_home() else {
            return Self(None);
        };
        let dir = home.join("running");
        if std::fs::create_dir_all(&dir).is_err() {
            return Self(None);
        }
        let pid = std::process::id();
        let path = dir.join(format!("{pid}.json"));
        let task: String = task.split_whitespace().collect::<Vec<_>>().join(" ");
        let task: String = task.chars().take(160).collect();
        let row = serde_json::json!({
            "pid": pid,
            "ppid": parent_id(),
            "session": session_id,
            "owner": std::env::var("GRAY_SESSION_ID").unwrap_or_default(),
            "name": std::env::var("GRAY_AGENT_NAME").unwrap_or_default(),
            "origin": std::env::var("GRAY_SESSION_ORIGIN").unwrap_or_default(),
            "model": model,
            "compact_ceiling": compact_ceiling,
            "cwd": std::env::current_dir().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(),
            "task": task,
            "started": std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs_f64())
                .unwrap_or(0.0),
        });
        if std::fs::write(&path, row.to_string()).is_err() {
            return Self(None);
        }
        Self(Some(path))
    }
}

impl Drop for RunningGuard {
    fn drop(&mut self) {
        if let Some(path) = &self.0 {
            let _ = std::fs::remove_file(path);
        }
    }
}

const STEER_LINE_CAP: usize = 2 * 1024 * 1024;

#[derive(Debug, PartialEq)]
enum BoundedLine {
    Eof,
    Line,
    TooLong,
}

fn read_bounded_line<R: std::io::BufRead>(
    r: &mut R,
    buf: &mut Vec<u8>,
    cap: usize,
) -> std::io::Result<BoundedLine> {
    use std::io::{BufRead, Read};
    buf.clear();
    if r.by_ref().take(cap as u64 + 1).read_until(b'\n', buf)? == 0 {
        return Ok(BoundedLine::Eof);
    }
    if buf.len() <= cap || buf.ends_with(b"\n") {
        return Ok(BoundedLine::Line);
    }
    buf.clear();
    loop {
        let chunk = match r.fill_buf() {
            Ok(chunk) => chunk,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        if chunk.is_empty() {
            break;
        }
        match chunk.iter().position(|&b| b == b'\n') {
            Some(i) => {
                r.consume(i + 1);
                break;
            }
            None => {
                let len = chunk.len();
                r.consume(len);
            }
        }
    }
    Ok(BoundedLine::TooLong)
}

async fn run_print_inner(
    config: &Config,
    prompt: Option<&str>,
    user_message: Message,
    session: Option<&str>,
    continue_last: bool,
    mut json: Option<&mut JsonOutput>,
) -> anyhow::Result<()> {
    let mut config_owned = config.clone();
    let _ = crate::repl::normalize_adopted_model(&mut config_owned);
    let config = &config_owned;
    crate::setup::set_user_context_window(config.context_window);
    crate::setup::set_user_reserve_tokens(config.context_reserve);
    crate::setup::set_user_keep_recent_tokens(config.context_keep);

    // Flag-based connects (--base-url/--model) never passed /models, so probe
    // the endpoint once (async, cheap) — it fills the context cache so the
    // endpoint's real context_length drives compaction instead of the 256k
    // fallback. Cached on disk for later runs.
    let custom_base = crate::setup::normalize_custom_base_url(&config.base_url)
        != crate::setup::normalize_custom_base_url(crate::config::DEFAULT_BASE_URL);
    if !crate::config::offline()
        && custom_base
        && config.context_window.is_none()
        && !config.uses_plugin_credentials()
        && !config.bare
        && {
            crate::setup::context::ensure_disk_loaded();
            config
                .model
                .as_deref()
                .is_some_and(|m| crate::setup::get_cached_model_context(m).is_none())
        }
    {
        let base = config.base_url.clone();
        let key = config.api_key.clone();
        tokio::task::spawn_blocking(move || {
            crate::setup::fetch_live_provider_models(&base, key.as_deref());
        });
    }

    let steering = json.is_some() && steer_enabled();
    match json.as_deref() {
        Some(output) if crate::ask::json_enabled() || steering => {
            crate::ask::install_json(output.turn_id.clone())
        }
        _ => crate::ask::install(None, false),
    }
    let steer_rx = if steering {
        crate::ask::set_demux();
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        tokio::task::spawn_blocking(move || {
            let stdin = std::io::stdin();
            let mut buf = Vec::new();
            loop {
                match read_bounded_line(&mut stdin.lock(), &mut buf, STEER_LINE_CAP) {
                    Ok(BoundedLine::Eof) | Err(_) => break,
                    Ok(BoundedLine::TooLong) => continue,
                    Ok(BoundedLine::Line) => {}
                }
                let Ok(line) = std::str::from_utf8(&buf) else {
                    continue;
                };
                let Ok(v) = serde_json::from_str::<serde_json::Value>(line.trim()) else {
                    continue;
                };
                if v.get("type").and_then(|t| t.as_str()) == Some("steer") {
                    if let Some(text) = v
                        .get("text")
                        .and_then(|t| t.as_str())
                        .filter(|t| !t.trim().is_empty())
                    {
                        let _ = tx.send(text.to_string());
                    }
                } else if let Some(ask_id) = v.get("ask_id").and_then(|a| a.as_u64()) {
                    crate::ask::deliver_json_answer(ask_id, line);
                }
            }
        });
        Some(std::sync::Arc::new(std::sync::Mutex::new(rx)))
    } else {
        None
    };
    let cwd = std::env::current_dir()?;
    if !config.bare {
        let _ = crate::plugin_status::ensure_boot(None);
    }
    let store = JsonlSessionStore::default();
    let prompt_text = crate::failed_turn::prompt_text(prompt, &user_message);
    let prompt_hash = crate::failed_turn::prompt_hash(&prompt_text);

    let (resume_target, needs_maintain): (Option<SessionId>, bool) = match session {
        Some(raw)
            if json.is_some()
                && crate::session_store::valid_session_id(raw)
                && store.exists_on_disk(&SessionId::new(raw)).await =>
        {
            (Some(SessionId::new(raw)), true)
        }
        Some(raw) => (
            Some(crate::resume::resolve_session_strict(&store, raw, false).await?),
            false,
        ),
        None if continue_last => (crate::resume::latest_session_anywhere(&store).await, false),
        None => (
            crate::failed_turn::resume_session(&store, &cwd, &prompt_hash).await,
            false,
        ),
    };

    let mut _open_guard = None;
    if let Some(sid) = resume_target.as_ref() {
        _open_guard = Some(store.acquire_open(sid).await?);
        if needs_maintain {
            store.maintain(sid).await?;
        }
    }
    log::debug!(target: "gray_timing", "spawn session_resolved elapsed_ms={}", crate::turn_caps::process_start().elapsed().as_millis());
    let history: Vec<Message> = match &resume_target {
        Some(sid) => {
            let (_, entries) = store
                .load(sid)
                .await
                .map_err(|e| anyhow::anyhow!("could not resume session {}: {e}", sid.as_str()))?;
            entries.into_iter().map(|e| e.message).collect()
        }
        None => Vec::new(),
    };
    log::debug!(target: "gray_timing", "spawn history_loaded elapsed_ms={}", crate::turn_caps::process_start().elapsed().as_millis());

    let session_id = match resume_target.clone() {
        Some(id) => id,
        None => store.fresh_id().await,
    };
    if let Some(output) = json.as_deref_mut() {
        output.session_id = Some(session_id.as_str().to_owned());
    }
    let initial_count = history.len();
    let cancel = tokio_util::sync::CancellationToken::new();
    let ctx = ToolContext {
        cwd: cwd.clone(),
        cancel: cancel.clone(),
        session_id: Some(session_id.as_str().to_owned()),
    };
    if std::env::var_os("GRAY_SESSION_ID").is_none() {
        unsafe { std::env::set_var("GRAY_SESSION_ID", session_id.as_str()) };
    }
    let task_text = user_message
        .content
        .iter()
        .find_map(|b| match b {
            gray_core::message::ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .unwrap_or("");
    let _running = RunningGuard::register(
        session_id.as_str(),
        task_text,
        config.model.as_deref().unwrap_or(""),
        crate::compact::effective_compact_ceiling(config),
    );

    if let Some(max) = config.max_wall_secs
        && crate::turn_caps::process_start().elapsed().as_secs() >= max
    {
        anyhow::bail!("max wall time {max}s reached — stopping (--max-wall-secs SECS)");
    }
    let mut agent = build_agent(config, &cwd, Some(session_id.as_str())).await?;
    agent.set_must_act(config.bare);
    if std::env::var_os("GRAY_BENCH_NUDGES").is_some() {
        agent.set_bench_nudges(true);
        if let Ok(v) = std::env::var("GRAY_THRASH_NUDGE_AT") {
            let mut marks: Vec<u32> = v.split(',').filter_map(|s| s.trim().parse().ok()).collect();
            marks.sort_unstable();
            marks.dedup();
            agent.set_bench_thrash_marks(marks);
        }
    }
    log::debug!(target: "gray_timing", "spawn agent_built elapsed_ms={}", crate::turn_caps::process_start().elapsed().as_millis());

    if let Some(out) = json.as_deref_mut() {
        let mut row = serde_json::json!({
            "type": "progress",
            "phase": "compact_ceiling",
            "compact_ceiling": crate::compact::effective_compact_ceiling(config),
        });
        if let Some(cliff) = agent.price_cliff() {
            row["price_cliff"] = cliff.into();
        }
        out.write(row)?;
    }
    let tool_labels: std::collections::HashMap<String, String> = agent
        .tool_defs()
        .iter()
        .filter_map(|t| {
            t.label
                .as_ref()
                .map(|l| l.trim())
                .filter(|l| !l.is_empty())
                .map(|l| (t.name.clone(), l.to_string()))
        })
        .collect();
    let tool_previews: std::collections::HashMap<String, String> = agent
        .tool_defs()
        .iter()
        .filter_map(|t| {
            t.preview
                .as_ref()
                .map(|x| x.trim())
                .filter(|x| !x.is_empty())
                .map(|x| (t.name.clone(), x.to_string()))
        })
        .collect();
    if let Some(output) = json.as_deref_mut() {
        output.labels = tool_labels.clone();
        output.previews = tool_previews.clone();
    }
    if resume_target.is_none() {
        store
            .create(
                SessionMeta::new(
                    session_id.clone(),
                    now_millis(),
                    cwd.clone(),
                    config.model.as_deref().unwrap_or("unset"),
                )
                .with_origin(crate::session_store::print_session_origin()),
            )
            .await?;

        match store.acquire_open(&session_id).await {
            Ok(guard) => _open_guard = Some(guard),
            Err(e) => log::warn!("session open-lock failed: {e}"),
        }
    }
    if let Some(meter) = json.as_ref().and_then(|output| output.meter.as_ref()) {
        agent = agent.map_provider(|provider| meter.wrap(provider));
    }
    if !history.is_empty() {
        agent = agent.with_messages(history);
    }
    for w in crate::take_profile_warnings() {
        eprintln!("warning: {w}");
    }

    let history_revision = agent.history_revision();

    let checkpointed = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(initial_count));
    let checkpoint_failed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    agent.set_checkpoint(Some(session_checkpoint(
        JsonlSessionStore::new(store.root_dir()),
        session_id.clone(),
        checkpointed.clone(),
        checkpoint_failed.clone(),
        history_revision,
    )));
    if let Some(rx) = &steer_rx {
        let rx = rx.clone();
        agent.set_steer(std::sync::Arc::new(move || {
            rx.lock().ok().and_then(|mut r| r.try_recv().ok())
        }));
    }

    let user_msg = if let Some(prompt) = prompt {
        let inline = crate::repl::attachments::extract_inline_image_paths(prompt, &cwd);
        crate::repl::build_user_message_with_attachments(prompt, &inline)
    } else {
        user_message
    };
    log::debug!(target: "gray_timing", "spawn_to_ready_ms={}", crate::turn_caps::process_start().elapsed().as_millis());

    let sigint_cancel = cancel.clone();
    let sigint_task = tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            sigint_cancel.cancel();
        }
    });

    let stdout = std::io::stdout();
    let mut in_flight: HashMap<String, ActiveToolCall> = HashMap::new();
    let mut render_err: Option<std::io::Error> = None;
    let mut turn_usage: Option<gray_core::event::Usage> = None;
    let turn_start = std::time::Instant::now();
    let render_cancel = cancel.clone();
    let mut on_event = |ev: &AgentEvent| {
        if let AgentEvent::TurnEnd { usage, .. } = ev {
            match turn_usage.as_mut() {
                Some(u) => u.accumulate(usage),
                None => turn_usage = Some(*usage),
            }
        }
        if render_err.is_some() {
            return;
        }
        let rendered = match json.as_deref_mut() {
            Some(output) => output.event(ev),
            None => render_event_with_context(
                &mut stdout.lock(),
                ev,
                Some(&cwd),
                &mut in_flight,
                &tool_labels,
                &tool_previews,
            ),
        };
        if let Err(e) = rendered {
            render_cancel.cancel();
            render_err = Some(e);
        }
    };
    let map_err = |e: gray_core::error::CoreError| {
        let msg = crate::repl::format_core_error(&e, &config.base_url);
        anyhow::Error::new(e).context(scrub_error_text(&msg))
    };
    let mut run_result = agent
        .run_streaming(user_msg, ctx.clone(), &mut on_event)
        .await
        .map_err(map_err);

    // A turn that ends with background jobs still running (a bash call that
    // yielded at its timeout) is not done: wait for each completion notice
    // and wake the agent on it, the way the REPL's idle wake does. Without
    // this a `gray -p` child — every subagent — exits mid-task, orphaning
    // the job and reporting "I'll resume when it wakes me" as its answer.
    let wait_background = background_wait_enabled();
    let exec = agent.executor_handle();
    while run_result.is_ok() && !cancel.is_cancelled() {
        if let Some(rx) = &steer_rx
            && let Some(text) = rx.lock().ok().and_then(|mut r| r.try_recv().ok())
        {
            on_event(&AgentEvent::Steered { text: text.clone() });
            run_result = agent
                .run_streaming(
                    gray_core::message::Message::user(text),
                    ctx.clone(),
                    &mut on_event,
                )
                .await
                .map_err(map_err);
            continue;
        }
        if !wait_background {
            break;
        }
        let notices = agent.drain_background_notifications(&ctx);
        if notices.is_empty() {
            if !exec.has_pending_background(&ctx) {
                break;
            }
            // Short slices keep steers responsive while a job runs.
            tokio::select! {
                _ = exec.wait_for_notification(&ctx, std::time::Duration::from_secs(1)) => {}
                _ = cancel.cancelled() => break,
            }
            continue;
        }
        let text = notices
            .iter()
            .map(|n| format!("{}\n{n}", crate::composer::transcript::BG_NOTICE_MARKER))
            .collect::<Vec<_>>()
            .join("\n\n");
        run_result = agent
            .run_streaming(
                gray_core::message::Message::user_injected(text),
                ctx.clone(),
                &mut on_event,
            )
            .await
            .map_err(map_err);
    }
    sigint_task.abort();
    agent.set_checkpoint(None);
    let turn_duration_ms = turn_start.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
    if run_result.is_ok()
        && let Some(output) = json.as_deref_mut()
        && let Some(message) = agent
            .messages()
            .last()
            .filter(|m| m.role == gray_core::message::Role::Assistant)
    {
        output.text = redact_message(message).text_content();
    }

    let checkpoint_failed = checkpoint_failed.load(std::sync::atomic::Ordering::Relaxed);
    let rewritten = agent.history_revision() != history_revision || checkpoint_failed;
    let persist_from = if checkpoint_failed {
        0
    } else {
        initial_count.max(checkpointed.load(std::sync::atomic::Ordering::Relaxed))
    };
    let persist_result = append_new_messages(
        &store,
        &session_id,
        persist_from,
        agent.messages(),
        rewritten,
        turn_usage,
        Some(turn_duration_ms),
    )
    .await;

    let render_broken = render_err
        .as_ref()
        .is_some_and(|e| e.kind() == ErrorKind::BrokenPipe);

    if render_broken || json.is_some() {
        let _ = crate::host::take_host_say();
    } else {
        let msgs = crate::host::take_host_say();
        if !msgs.is_empty() {
            let mut out = stdout.lock();
            'say: for msg in &msgs {
                for line in msg {
                    if writeln!(out, "{line}").is_err() {
                        break 'say;
                    }
                }
            }
            let _ = out.flush();
        }
    }

    let result = if render_broken {
        match (&run_result, &persist_result) {
            (_, Err(p)) => Err(anyhow::anyhow!("{p:#}")),
            (Ok(_), Ok(_)) => Ok(()),
            (Err(r), Ok(_)) if r.to_string().to_lowercase().contains("cancel") => Ok(()),
            (Err(_), Ok(_)) => run_result.map(|_| ()),
        }
    } else if let Some(e) = render_err {
        match (run_result, persist_result) {
            (Err(r), Err(p)) => Err(anyhow::anyhow!(
                "{r:#}; stdout write failed: {e}; also failed to persist session: {p:#}"
            )),
            (Err(r), _) => Err(anyhow::anyhow!("{r:#}; stdout write failed: {e}")),
            (_, Err(p)) => Err(anyhow::anyhow!(
                "stdout write failed: {e}; also failed to persist session: {p:#}"
            )),
            _ => Err(anyhow::anyhow!("stdout write failed: {e}")),
        }
    } else {
        crate::ask::shutdown();
        match (run_result, persist_result) {
            (Err(r), Err(p)) => Err(anyhow::anyhow!(
                "{r:#}; also failed to persist session: {p:#}"
            )),
            (Err(r), _) => Err(r),
            (_, Err(p)) => Err(p),
            (Ok(_), Ok(_)) => Ok(()),
        }
    };
    match &result {
        Ok(()) => crate::failed_turn::clear_checkpoint(&session_id, &cwd, &prompt_hash),
        Err(e) => {
            let failure = PrintFailure::of(e);
            crate::failed_turn::record_failure(
                &session_id,
                &cwd,
                &prompt_text,
                &prompt_hash,
                failure.code,
                failure.exit,
            );
        }
    }
    result
}

pub(crate) fn scrub_error_text(s: &str) -> String {
    redact_for_disclosure(s).into_text()
}

fn session_checkpoint(
    store: JsonlSessionStore,
    sid: SessionId,
    done: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    failed: std::sync::Arc<std::sync::atomic::AtomicBool>,
    history_revision: u64,
) -> gray_core::agent::CheckpointHook {
    let store = std::sync::Arc::new(store);
    std::sync::Arc::new(move |msgs: &[Message], rev: u64| {
        use std::sync::atomic::Ordering::Relaxed;
        let from = done.load(Relaxed);
        let fresh = if rev == history_revision && !failed.load(Relaxed) && from <= msgs.len() {
            msgs[from..].to_vec()
        } else {
            Vec::new()
        };
        let (store, sid, done, failed) = (store.clone(), sid.clone(), done.clone(), failed.clone());
        Box::pin(async move {
            for m in &fresh {
                if store.append(&sid, &redact_message(m)).await.is_err() {
                    failed.store(true, Relaxed);
                    return;
                }
                done.fetch_add(1, Relaxed);
            }
        })
    })
}

pub async fn append_new_messages(
    store: &JsonlSessionStore,
    sid: &SessionId,
    prior_count: usize,
    messages: &[Message],
    history_rewritten: bool,
    usage: Option<gray_core::event::Usage>,
    duration_ms: Option<u64>,
) -> anyhow::Result<()> {
    if history_rewritten || messages.len() < prior_count {
        let redacted: Vec<Message> = messages.iter().map(redact_message).collect();
        return store
            .append_compaction_replacement(sid, &redacted)
            .await
            .map_err(|e| anyhow::anyhow!("failed to persist compacted session: {e}"));
    }
    let new = &messages[prior_count.min(messages.len())..];
    for (i, msg) in new.iter().enumerate() {
        let is_last = i + 1 == new.len();

        let redacted = redact_message(msg);
        store
            .append_with_usage_and_duration(
                sid,
                &redacted,
                if is_last { usage } else { None },
                if is_last { duration_ms } else { None },
            )
            .await
            .map_err(|e| anyhow::anyhow!("failed to append message to session: {e}"))?;
    }
    Ok(())
}

pub async fn save_session(
    store: &JsonlSessionStore,
    model: &str,
    cwd: &Path,
    messages: &[Message],
) -> anyhow::Result<SessionId> {
    let session_id = store.fresh_id().await;
    let timestamp = now_millis();

    let meta = SessionMeta::new(
        session_id.clone(),
        timestamp,
        cwd.to_path_buf(),
        model.to_string(),
    )
    .with_origin(crate::session_store::print_session_origin());
    store.create(meta).await?;

    for msg in messages {
        let redacted = redact_message(msg);
        store
            .append(&session_id, &redacted)
            .await
            .map_err(|e| anyhow::anyhow!("failed to append message to session: {e}"))?;
    }

    Ok(session_id)
}

#[path = "print_tests.rs"]
#[cfg(test)]
mod tests;
