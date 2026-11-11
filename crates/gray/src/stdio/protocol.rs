#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Request {
    Prompt,
    Command,
    Steer,
    Interrupt,
    Approve,
    ListCommands,
    ListModels,
    SetModel,
    Status,
    ListSessions,
    Resume,
    Shutdown,
}

impl Request {
    const ALL: [Request; 12] = [
        Request::Prompt,
        Request::Command,
        Request::Steer,
        Request::Interrupt,
        Request::Approve,
        Request::ListCommands,
        Request::ListModels,
        Request::SetModel,
        Request::Status,
        Request::ListSessions,
        Request::Resume,
        Request::Shutdown,
    ];

    pub(crate) fn parse(name: &str) -> Option<Request> {
        if name == "quit" {
            return Some(Request::Shutdown);
        }
        Self::ALL.into_iter().find(|r| r.row().0 == name)
    }

    fn row(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Request::Prompt => (
                "prompt",
                "`text`, `attachments?`",
                "a turn, like typing in the REPL (`@mentions` resolve the same way)",
            ),
            Request::Command => (
                "command",
                "`line`",
                "any slash command through the real dispatcher; `/` added if missing; output as `say`",
            ),
            Request::Steer => (
                "steer",
                "`text`, `attachments?`",
                "queue input for the running turn",
            ),
            Request::Interrupt => ("interrupt", "—", "cancel the running turn, as SIGINT does"),
            Request::Approve => (
                "approve",
                "`ask_id`, `answers`",
                "answer an `ask`; `answers` = `{question_id: [label, …]}`; fails if no such ask is open",
            ),
            Request::ListCommands => ("list_commands", "—", "builtin registry + plugin commands"),
            Request::ListModels => (
                "list_models",
                "`live?`",
                "the catalog schema of `gray --dump-models`, with this session's effort levels",
            ),
            Request::SetModel => (
                "set_model",
                "`provider?`, `model?`, `effort?`",
                "`/models` semantics, incl. variant suffixes and clamping",
            ),
            Request::Status => (
                "status",
                "—",
                "session/model/context/usage snapshot, `turn_in_flight`, `queued_inputs`",
            ),
            Request::ListSessions => ("list_sessions", "`all?`", "saved sessions for this cwd"),
            Request::Resume => (
                "resume",
                "`session`, `last?`, `all?`",
                "`/resume` through the real handler",
            ),
            Request::Shutdown => (
                "shutdown",
                "—",
                "graceful exit: hooks then shell tasks; `quit` is an alias",
            ),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Event {
    Hello,
    TurnStart,
    Text,
    Thinking,
    ToolStart,
    ToolProgress,
    ToolCall,
    ToolResult,
    Usage,
    TurnUsage,
    Steered,
    Compacted,
    Notice,
    Say,
    Ask,
    TurnEnd,
    Result,
    Error,
}

impl Event {
    const ALL: [Event; 18] = [
        Event::Hello,
        Event::TurnStart,
        Event::Text,
        Event::Thinking,
        Event::ToolStart,
        Event::ToolProgress,
        Event::ToolCall,
        Event::ToolResult,
        Event::Usage,
        Event::TurnUsage,
        Event::Steered,
        Event::Compacted,
        Event::Notice,
        Event::Say,
        Event::Ask,
        Event::TurnEnd,
        Event::Result,
        Event::Error,
    ];

    pub(crate) fn name(self) -> &'static str {
        self.row().0
    }

    fn row(self) -> (&'static str, &'static str) {
        match self {
            Event::Hello => (
                "hello",
                "`gray_version`, `cwd`, `model`, `effort`, `provider_id`, `base_url`, `unconfigured` (always the first row)",
            ),
            Event::TurnStart => ("turn_start", "`prompt`"),
            Event::Text => ("text", "`delta`"),
            Event::Thinking => ("thinking", "`delta`"),
            Event::ToolStart => ("tool_start", "`id`, `name`"),
            Event::ToolProgress => ("tool_progress", "`id`, `name`, `args_so_far`"),
            Event::ToolCall => ("tool_call", "`id`, `name`, `args`"),
            Event::ToolResult => (
                "tool_result",
                "`id`, `name`, `output`, `is_error` (output ~8K, secrets redacted)",
            ),
            Event::Usage => ("usage", "`step: true`, `usage`"),
            Event::TurnUsage => ("turn_usage", "`usage`, `stop_reason`"),
            Event::Steered => ("steered", "`text`"),
            Event::Compacted => (
                "compacted",
                "`tokens_before/after`, `messages_before/after`, `reason`",
            ),
            Event::Notice => ("notice", "`kind`, `cause`"),
            Event::Say => (
                "say",
                "`level`, `text`, `kind?` — anything the REPL would print",
            ),
            Event::Ask => ("ask", "`ask_id`, `questions` — answer with `approve`"),
            Event::TurnEnd => (
                "turn_end",
                "`ok`, `interrupted`, `usage`, `error?` (errors arrive as `say` before it)",
            ),
            Event::Result => (
                "result",
                "`id`, `ok`, `error?` and the payload keys inline; `req` equals `id`, `turn_id` is set only when the result closes that turn",
            ),
            Event::Error => ("error", "`code`, `message`"),
        }
    }

    pub(crate) fn listed(self) -> bool {
        Self::ALL.contains(&self)
    }
}

const HEAD: &str = r#"# `gray --stdio`

Generated by `gray --dump-protocol`; edit `crates/gray/src/stdio/protocol.rs`.

One long-lived `gray` driving the same engine the REPL uses —
`run_prompt_turn` for turns, `dispatch_command` for slash commands — over
newline-delimited JSON on stdin/stdout. Built for gray-gui: one process per
IDE thread.

```
gray --stdio [--model M] [--provider P] [--base-url U] [--api-key K] \
             [--session ID | -r [ID] | -c] [--bare]
```

stdout carries protocol rows only; diagnostics go to stderr. Interactive-only
surfaces (`/models` with no arg, `/plugins`, login flows, pagers) degrade to
`say` text or an error result.

## Envelope

```json
{"protocol": 2, "seq": 17, "session_id": "abc123",
 "req": 5, "turn_id": "turn-…", "type": "…", "...": "…"}
```

`seq` is monotonic; `session_id` is null before a session exists; `req` is the
request id that produced the row (null = ambient); `turn_id` is null outside a
turn. Clients should refuse a `protocol` they do not know.

Requests are `{"type": "…", "id": <any JSON>}`. Every request gets exactly one
`result`; a line that is not JSON gets an `error` row with code `bad_json`.
Requests *queued* mid-turn first get `{"queued": true}`, then a second
`result` when their work completes: `{"turn_id": …}` if they ran as their own
turn, `{"steered": true}` if the running turn consumed them.

## Requests

| type | fields | behavior |
|------|--------|----------|
"#;

const MID: &str = r#"
## Events

Turn-local events carry `turn_id` + `req`; ambient ones carry `req: null`.

| type | fields |
|------|--------|
"#;

const TAIL: &str = r#"
## Concurrency

One turn at a time per process, like the REPL. `interrupt`, `approve` and
`steer` are answered immediately even mid-turn. `prompt`/`command`/`steer`
sent mid-turn are queued: the running turn may consume them as a steer,
leftovers run in order after it, and the deferred `turn_start` carries their
original `req`. Background work (jobs, cron, mailbox) emits `say` and may run
auto-turns between requests — poll `status` to tell idle from busy.

## Client notes

Spawn one process per thread and correlate on `result`; `req` on events saves
tracking turn ownership. Don't drive no-arg `/models`, `/plugins` or
`/sessions` — use `list_models` / `set_model` / `resume`.
"#;

pub fn render() -> String {
    let mut out = String::from(HEAD);
    for r in Request::ALL {
        let (name, fields, behavior) = r.row();
        out.push_str(&format!("| `{name}` | {fields} | {behavior} |\n"));
    }
    out.push_str(MID);
    for e in Event::ALL {
        let (name, fields) = e.row();
        out.push_str(&format!("| `{name}` | {fields} |\n"));
    }
    out.push_str(TAIL);
    out
}
