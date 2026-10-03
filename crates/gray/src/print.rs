//! Print mode: one-shot execution of a user prompt, streaming events to stdout
//! and recording the conversation to a JSONL session.

use std::collections::HashMap;
use std::io::{ErrorKind, Write};
use std::path::Path;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::session_store::{JsonlSessionStore, SessionId, SessionMeta};
use gray_core::agent::ToolContext;
use gray_core::event::AgentEvent;
use gray_core::input::{InputEnvelope, InputError};
use gray_core::message::Message;
use gray_core::redaction::{redact_for_disclosure, redact_message};

use crate::build_agent;
use crate::config::Config;

/// Wall-clock milliseconds since the Unix epoch (0 on a pre-epoch clock).
pub(crate) fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Tracking state for active tool call during streaming.
#[derive(Debug, Clone, Default)]
pub struct ActiveToolCall {
    pub name: String,
    pub args: Option<serde_json::Value>,
    /// Display-only headline (plugin `label`): renderer-injected into a
    /// copy of args, so the wire args the executor saw stay untouched.
    pub label: Option<String>,
}

/// Renders a single AgentEvent with active tool tracking and CWD context.
///
/// In-flight calls are keyed by call id so concurrent starts/ends/results
/// can't clobber each other (single-slot tracking swapped names/args on
/// parallel calls).
pub fn render_event_with_context<W: Write>(
    w: &mut W,
    event: &AgentEvent,
    cwd: Option<&Path>,
    in_flight: &mut HashMap<String, ActiveToolCall>,
    tool_labels: &HashMap<String, String>,
) -> std::io::Result<()> {
    match event {
        AgentEvent::Start => Ok(()),
        AgentEvent::TextDelta { delta } => {
            write!(w, "{delta}")?;
            w.flush()
        }
        AgentEvent::ThinkingDelta { delta } => {
            // Same dim+italic treatment as the REPL (pi's thinking style).
            write!(w, "\x1b[2m\x1b[3m{delta}\x1b[0m")?;
            w.flush()
        }
        AgentEvent::ToolCallStart { id, name } => {
            in_flight.insert(
                id.clone(),
                ActiveToolCall {
                    label: tool_labels.get(name).cloned(),
                    name: name.clone(),
                    args: None,
                },
            );
            Ok(())
        }
        AgentEvent::ToolCallProgress { id, name, .. } => {
            let label = tool_labels.get(name).cloned();
            in_flight.entry(id.clone()).or_insert(ActiveToolCall {
                name: name.clone(),
                args: None,
                label,
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
            writeln!(
                w,
                "\n{}",
                crate::tool_fmt::format_tool_call_header_plain(name, args, cwd)
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
        // Compaction is observable, not silent: one dim accounting line per
        // history rewrite (arXiv:2512.22087 / 2601.16746).
        AgentEvent::Compacted {
            tokens_before,
            tokens_after,
            messages_before,
            messages_after,
        } => {
            writeln!(
                w,
                "\n\x1b[2m\u{21bb} compacted {} \u{2192} {} tok ({} \u{2192} {} messages)\x1b[0m",
                crate::repl::fmt_usage(*tokens_before),
                crate::repl::fmt_usage(*tokens_after),
                messages_before,
                messages_after,
            )?;
            w.flush()
        }
        // Codex steal: retry notices go to the same stream, dim, never fatal.
        AgentEvent::StreamError { message, details } => {
            // Provider retry notices can echo request details: scrub first.
            let message = scrub_error_text(message);
            let details = scrub_error_text(details);
            if details.is_empty() {
                writeln!(w, "\n\x1b[2m⚠ {message}\x1b[0m")?;
            } else {
                writeln!(w, "\n\x1b[2m⚠ {message}\n└ {details}\x1b[0m")?;
            }
            w.flush()
        }
        AgentEvent::TurnEnd { usage, .. } => {
            if usage.total() > 0 {
                writeln!(
                    w,
                    "\n\x1b[2m\u{2b22} {} tok\x1b[0m",
                    crate::repl::fmt_usage(usage.total())
                )?;
            }
            w.flush()
        }
    }
}

/// Executes a prompt in one-shot print mode, printing events to stdout and persisting the session.
pub async fn run_print_mode(config: &Config, prompt: &str) -> anyhow::Result<()> {
    run_print_mode_with_session(config, prompt, None, false).await
}

/// Print mode with `--session <id>` / `-c` support: a resolved session is
/// continued in place (new messages appended to its JSONL); otherwise a fresh
/// session is created. Bogus ids fail with the same `no session matching`
/// error as `gray resume <id>` (exit 1), before any model work starts.
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

/// Machine-readable print mode. Never forwards reasoning, tool arguments/results,
/// provider error bodies, or plugin host/say output to the transport.
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

/// Machine-readable one-shot mode for a versioned structured input event.
/// The event remains a typed user block in the session instead of being
/// converted into prompt prose.
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
    // A failed or cancelled turn never reaches `TurnEnd`: close any prose
    // still open so the consumer is not left holding a half segment.
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

/// Emit a bounded protocol-1 error row before a structured input can enter
/// the agent. The input parser never includes payload contents in this row.
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

/// Process exit codes for `--json` print mode. Harnesses branch on these (and
/// the JSON `code`) instead of parsing prose: infra failures are the turn the
/// provider dropped, safe to re-run; `turn_failed` means the agent itself
/// stopped and retrying repeats the same outcome.
pub const EXIT_TURN_FAILED: i32 = 1;
pub const EXIT_INFRA: i32 = 3;

/// Machine-readable failure class carried from `--json` print mode to the
/// process exit, with the operator hint that names the next command to run.
/// `retryable` marks provider/network deaths — the bench-class failure that
/// decides whether a harness re-runs the turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrintFailure {
    pub code: &'static str,
    pub retryable: bool,
    pub exit: i32,
    pub hint: Option<&'static str>,
}

impl PrintFailure {
    /// Classify an anyhow error by downcasting to [`gray_core::error::CoreError`];
    /// anything unrecognized is a plain turn failure.
    pub fn of(error: &anyhow::Error) -> Self {
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

    /// One-line operator message; never carries provider response bodies.
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
            _ => {
                "Agent turn failed; actions may already have occurred. Do not automatically retry."
            }
        }
    }

    /// The mmx lesson: every failure class names the command that fixes it.
    fn hint_for(code: &str) -> Option<&'static str> {
        Some(match code {
            "auth_failed" => {
                "Check the API key for this provider: `gray setup` (or `/connect` in the REPL)."
            }
            "rate_limited" => "Back off and retry, or switch provider/model with `/model`.",
            "bad_request" => {
                "Often a model id the endpoint does not serve; `/model` shows what is configured."
            }
            "context_overflow" => "Start `/new` or run `/compact`.",
            "connection_failed" => {
                "Check the base URL and network; `gray setup` re-points the provider."
            }
            "timeout" => "Retry, or raise the request timeout for slow providers.",
            "server_error" | "stream_broken" => "Retrying the turn usually succeeds.",
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

/// Caps on disclosed tool data (chars) and the reasoning buffer flush
/// threshold. Bounds what a chatty surface (Discord, Telegram, a log tail)
/// receives per row; the full text stays in the session log.
const DETAIL_CAP: usize = 240;
const OUTPUT_CAP: usize = 1200;
const THINKING_FLUSH: usize = 800;
/// Minimum gap between two streamed `text` rows for one segment. The first
/// row of a segment goes out at once; after that a chat surface edits at
/// about one frame a second, so a row per token is pure overhead.
const TEXT_EVERY: Duration = Duration::from_millis(200);

struct JsonOutput {
    turn_id: String,
    session_id: Option<String>,
    text: String,
    usage: gray_core::event::Usage,
    meter: Option<crate::print_meter::Meter>,
    /// Tool call id -> name, for rows that carry no name of their own
    /// (`ToolCallEnd` has args only). Cleared per call by `ToolResult`.
    tools: HashMap<String, String>,
    /// Reasoning buffer: flushed as one `thinking` row per THINKING_FLUSH
    /// chars, never one row per token.
    thinking: String,
    show_reasoning: bool,
    /// `GRAY_STREAM_TEXT=1`: stream assistant prose as `text` rows so a chat
    /// surface can edit its reply in place instead of posting it at the end.
    /// Opt-in: a consumer that never asked keeps the old row set.
    stream_text: bool,
    segment: TextSegment,
    /// Display names for plugin tools that declare a manifest `label`.
    /// Every other tool is named by humanizing its wire name.
    labels: HashMap<String, String>,
}

/// The assistant prose streamed since the last tool boundary. A new segment
/// starts after every tool call, so a surface can give each run of prose its
/// own message (Hermes' segment break).
#[derive(Default)]
struct TextSegment {
    /// Zero-based segment number within the turn.
    index: u64,
    /// Unredacted prose received for this segment.
    raw: String,
    /// Byte offset into `raw` up to which complete lines were sent as
    /// `delta`. Redaction works line by line, so a sent line never changes.
    sent: usize,
    /// The provisional tail last sent, to skip rows that say nothing new.
    tail: String,
    last_emit: Option<Instant>,
}

/// Redact a streamed fragment exactly as the final answer is: scrub only
/// when a secret fired, so prose and paths stay verbatim.
fn scrub(text: &str) -> String {
    let redaction = redact_for_disclosure(text);
    if redaction.has_secret() {
        redaction.into_text()
    } else {
        text.to_string()
    }
}

/// `GRAY_STREAM_TEXT` truthy check (same spelling rules as
/// `GRAY_SHOW_REASONING`).
fn stream_text_enabled() -> bool {
    std::env::var("GRAY_STREAM_TEXT").is_ok_and(|v| {
        matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

impl JsonOutput {
    /// What a surface should call `name`: the plugin's own label, else the
    /// wire name humanized (`discord_send` → `Discord Send`; a single token
    /// such as `bash` stays as it is), the same rule the TUI headers use.
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

    /// Progress rows for one event (thinking flushes first at turn end).
    /// Pure builder so the narration wire is unit-testable off stdout.
    fn rows(&mut self, event: &AgentEvent) -> Vec<serde_json::Value> {
        // Reasoning arrives per token; batch it into capped `thinking` rows.
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
        // A tool call, the end of a provider step, or the end of the turn
        // closes the prose before it: the closing row carries everything.
        let mut rows = match event {
            AgentEvent::ToolCallStart { .. }
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
                    if let Some(detail) = tool_detail(name, args) {
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
            AgentEvent::Compacted { .. } => "compacted",
            AgentEvent::TurnEnd { usage, .. } => {
                self.usage = *usage;
                rows.append(&mut self.take_thinking());
                "persisting"
            }
            _ => return rows,
        };
        row["phase"] = phase.into();
        rows.push(row);
        rows
    }

    /// One `text` row for the live segment. Complete lines go out once as
    /// `delta` (append-only: a consumer concatenates them); the unfinished
    /// line goes out as a provisional `tail` that the next row replaces,
    /// with its last word held back so a secret is never shown half typed.
    /// `done` sends everything, marks the segment closed, and starts the
    /// next one.
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
            // Hold the word still being typed: its redaction is only known
            // once it is complete.
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

    /// One `thinking` row for whatever reasoning has accumulated. Redacted
    /// like every disclosed detail: a model that reads a key file can
    /// quote it back.
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

/// Collapse to one line, cap, and redact secrets. Every `detail` on the wire
/// goes through here: tool args and model output are untrusted for secrets.
///
/// Paths stay verbatim when no secret is present — the same gate as
/// [`gray_core::redaction::redact_message`]: this wire feeds owner-local
/// surfaces (Discord narration), where `cat /tmp/shot.png` redacted to
/// `cat <path>` is noise, not safety. A secret in the same string
/// still scrubs the whole unit (paths included).
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

/// Preserve line breaks for a bounded terminal transcript while applying the
/// same disclosure rules as one-line details. The full result remains in the
/// session log; chat surfaces only receive this redacted prefix.
fn disclose_output(text: &str, cap: usize) -> String {
    // Redaction is linear in the input. Keep a small look-ahead past the
    // disclosure cap so a credential split at the boundary is still treated
    // as a credential, without scanning an unbounded tool result.
    let look_ahead = cap.saturating_add(256);
    let mut chars = text.chars();
    let prefix = chars.by_ref().take(look_ahead).collect::<String>();
    let source_truncated = chars.next().is_some();
    let redaction = redact_for_disclosure(&prefix);
    // ponytail: secret-free output stays byte-faithful (paths included);
    // scrub only when a secret fired.
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

/// The one-line "what did it just do" for known tools. Returns `None` for
/// anything else rather than dumping raw args: a value we do not
/// understand is exactly where a token hides.
fn tool_detail(name: &str, args: &serde_json::Value) -> Option<String> {
    let arg = |k: &str| args.get(k).and_then(serde_json::Value::as_str);
    match name {
        "bash" | "shell" => arg("command").map(|c| disclose(c, DETAIL_CAP)),
        "read" | "view" | "cat" => {
            let path = arg("path")?;
            // Line range when the reader has one: `config.yaml L110-139`.
            // `offset` is 1-based and negative means tail; either way the
            // path alone is the honest summary.
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
        "discord_send" => arg("content").map(|c| disclose(c, DETAIL_CAP)),
        "discord_send_ui" | "discord_open_modal" | "discord_ui_schema" => {
            Some("(discord ui)".to_string())
        }
        "discord_file" => arg("path")
            .or_else(|| arg("file_id"))
            .or_else(|| arg("action"))
            .map(|v| disclose(v, DETAIL_CAP)),
        _ => None,
    }
}

async fn run_print_inner(
    config: &Config,
    prompt: Option<&str>,
    user_message: Message,
    session: Option<&str>,
    continue_last: bool,
    mut json: Option<&mut JsonOutput>,
) -> anyhow::Result<()> {
    crate::setup::set_user_context_window(config.context_window);
    crate::setup::set_user_reserve_tokens(config.context_reserve);
    crate::setup::set_user_keep_recent_tokens(config.context_keep);
    // Sidecar `host/ask` without a TUI: the `--json` wire when the caller
    // asked for it (a chat bridge answers on stdin), else piped-stdin/empty.
    match json.as_deref() {
        Some(output) if crate::ask::json_enabled() => {
            crate::ask::install_json(output.turn_id.clone())
        }
        _ => crate::ask::install(None, false),
    }
    let cwd = std::env::current_dir()?;
    let store = JsonlSessionStore::default();
    // Explicit `--session` wins over `-c` (same precedence as the REPL).
    let resume_target: Option<SessionId> = match session {
        Some(raw) if json.is_some() && uuid::Uuid::parse_str(raw).is_ok() => {
            let id = SessionId::new(raw);
            store.maintain(&id).await?;
            Some(id)
        }
        Some(raw) => Some(crate::resume::resolve_session_strict(&store, raw, false).await?),
        None if continue_last => crate::resume::latest_session_anywhere(&store).await,
        None => None,
    };
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
    // Pin memory to the same durable identity for plain and JSON print mode.
    let session_id = resume_target.clone().unwrap_or_else(SessionId::generate);
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

    // One-shot run = one turn: only max_turns=0 (nonsensical but
    // explicit) and an already-blown wall clock can stop it pre-run.
    // max_turns=0 is rejected at resolve, so this is dead-simple.
    if let Some(max) = config.max_wall_secs
        && crate::turn_caps::process_start().elapsed().as_secs() >= max
    {
        anyhow::bail!("max wall time {max}s reached — stopping (--max-wall-secs SECS)");
    }
    let mut agent = build_agent(config, &cwd, Some(session_id.as_str())).await?;
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
    if let Some(output) = json.as_deref_mut() {
        output.labels = tool_labels.clone();
    }
    if resume_target.is_none() {
        store
            .create(SessionMeta::new(
                session_id.clone(),
                now_millis(),
                cwd.clone(),
                config.model.as_deref().unwrap_or("unset"),
            ))
            .await?;
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
    // Headless `-p` has no paste-attach: inline file links still carry vision.
    // Structured input already owns its typed block and never scans arbitrary
    // text for attachment paths.
    let user_msg = if let Some(prompt) = prompt {
        let inline = crate::repl::attachments::extract_inline_image_paths(prompt, &cwd);
        crate::repl::build_user_message_with_attachments(prompt, &inline)
    } else {
        user_message
    };
    // SIGINT only signals the shared token — never wrap the run in a
    // select! that would drop it. Aborted once the run returns.
    let sigint_cancel = cancel.clone();
    let sigint_task = tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            sigint_cancel.cancel();
        }
    });
    // Stream events live so piped output isn't all-or-nothing. The first
    // stdout error cancels paid model/tool work and stops further writes
    // (recorded once, never spammed per event).
    let stdout = std::io::stdout();
    let mut in_flight: HashMap<String, ActiveToolCall> = HashMap::new();
    let mut render_err: Option<std::io::Error> = None;
    let run_result = {
        let render_cancel = cancel.clone();
        let mut on_event = |ev: &AgentEvent| {
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
                ),
            };
            if let Err(e) = rendered {
                render_cancel.cancel();
                render_err = Some(e);
            }
        };
        agent
            .run_streaming(user_msg, ctx, &mut on_event)
            .await
            .map_err(|e| {
                let msg = crate::repl::format_core_error(&e, &config.base_url);
                anyhow::anyhow!(scrub_error_text(&msg))
            })
    };
    sigint_task.abort();
    if run_result.is_ok()
        && let Some(output) = json.as_deref_mut()
    {
        // Read the live final assistant message, not redacted JSONL and not
        // an exact-text search that could select a previous identical turn.
        if let Some(message) = agent
            .messages()
            .last()
            .filter(|m| m.role == gray_core::message::Role::Assistant)
        {
            output.text = redact_message(message).text_content();
        }
    }

    // F14: no `?` between run completion and finalization — always attempt
    // session persistence AND shell teardown, then propagate the original
    // error combined with any persistence error.
    let persist_result = append_new_messages(
        &store,
        &session_id,
        initial_count,
        agent.messages(),
        agent.history_revision() != history_revision,
    )
    .await;

    let render_broken = render_err
        .as_ref()
        .is_some_and(|e| e.kind() == ErrorKind::BrokenPipe);
    // Cron (or other plugin-initiated) `host/say` lines queued mid-turn.
    // Fallible writeln on a locked handle: stdout may already be closed.
    if render_broken || json.is_some() {
        let _ = crate::host::take_host_say();
    } else {
        let lines = crate::host::take_host_say();
        if !lines.is_empty() {
            let mut out = stdout.lock();
            for line in &lines {
                if writeln!(out, "{line}").is_err() {
                    break;
                }
            }
            let _ = out.flush();
        }
    }

    if render_broken {
        // Graceful BrokenPipe: the consumer went away. Paid work is already
        // cancelled, history persisted, shell torn down. Swallow a
        // cancellation (it is our own stop), keep any other error.
        match (&run_result, &persist_result) {
            (_, Err(p)) => return Err(anyhow::anyhow!("{p:#}")),
            (Ok(_), Ok(_)) => return Ok(()),
            (Err(r), Ok(_)) if r.to_string().to_lowercase().contains("cancel") => {
                return Ok(());
            }
            (Err(_), Ok(_)) => return run_result.map(|_| ()),
        }
    }
    if let Some(e) = render_err {
        // Non-pipe stdout failure: surface it, combined with run/persist.
        return match (run_result, persist_result) {
            (Err(r), Err(p)) => Err(anyhow::anyhow!(
                "{r:#}; stdout write failed: {e}; also failed to persist session: {p:#}"
            )),
            (Err(r), _) => Err(anyhow::anyhow!("{r:#}; stdout write failed: {e}")),
            (_, Err(p)) => Err(anyhow::anyhow!(
                "stdout write failed: {e}; also failed to persist session: {p:#}"
            )),
            _ => Err(anyhow::anyhow!("stdout write failed: {e}")),
        };
    }
    crate::ask::shutdown();
    match (run_result, persist_result) {
        (Err(r), Err(p)) => Err(anyhow::anyhow!(
            "{r:#}; also failed to persist session: {p:#}"
        )),
        (Err(r), _) => Err(r),
        (_, Err(p)) => Err(p),
        (Ok(_), Ok(_)) => Ok(()),
    }
}

/// Scrub provider/error text before it reaches stderr or a receipt.
pub(crate) fn scrub_error_text(s: &str) -> String {
    redact_for_disclosure(s).into_text()
}

/// Appends only messages at index `prior_count..` to an existing session
/// (print-mode `--session`/`-c` continuation in place — never a new file).
/// When in-loop compaction rewrote history (even if it grew past the cursor),
/// persists the whole active transcript behind a replacement boundary.
pub async fn append_new_messages(
    store: &JsonlSessionStore,
    sid: &SessionId,
    prior_count: usize,
    messages: &[Message],
    history_rewritten: bool,
) -> anyhow::Result<()> {
    if history_rewritten || messages.len() < prior_count {
        // Same redaction as the normal path: secrets never land in the file.
        let redacted: Vec<Message> = messages.iter().map(redact_message).collect();
        return store
            .append_compaction_replacement(sid, &redacted)
            .await
            .map_err(|e| anyhow::anyhow!("failed to persist compacted session: {e}"));
    }
    for msg in &messages[prior_count.min(messages.len())..] {
        // Durable JSONL: persist the redacted copy (secrets/paths never land
        // in the session file); the live turn keeps the raw text.
        let redacted = redact_message(msg);
        store
            .append(sid, &redacted)
            .await
            .map_err(|e| anyhow::anyhow!("failed to append message to session: {e}"))?;
    }
    Ok(())
}

/// Saves the accumulated messages to the session store.
pub async fn save_session(
    store: &JsonlSessionStore,
    model: &str,
    cwd: &Path,
    messages: &[Message],
) -> anyhow::Result<SessionId> {
    let session_id = SessionId::generate();
    let timestamp = now_millis();

    let meta = SessionMeta::new(
        session_id.clone(),
        timestamp,
        cwd.to_path_buf(),
        model.to_string(),
    );
    store.create(meta).await?;

    for msg in messages {
        // Durable JSONL: persist the redacted copy (see append_new_messages).
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
