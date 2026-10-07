# Customization: reaching pi parity

Goal: anything a pi user can reshape about pi (look, keys, prompts,
commands, tools, model routing, session behavior, UI) can be reshaped in
gray, and pi extensions themselves run in gray. Core gray stays one Rust
binary; code-level extension stays out of process.

Reference: pi 1.0.4 (`reference/pi-mono`, `packages/coding-agent`):
`docs/extensions.md`, `src/core/extensions/types.ts`, and the example
extensions in `examples/extensions/` (~80 of them).

## Where we stand

| Area | pi | gray |
|---|---|---|
| Extension model | TS modules in process (jiti), full API | Sidecar processes, NDJSON over stdio, any language |
| Lifecycle hooks | ~32 events, most can mutate | `tool/before`, `prompt/context`, `turn/end` (claimed in manifest `hooks`), `event/notify` (observe only) |
| Tools | register/override, per-tool prompt snippets, renderers, `setActiveTools` | plugin tools, `tool.override`, live tool sets (1.3) |
| UI | notify/select/confirm/input/editor, status, widgets, footer/header/title, working indicator, editor text, custom editor, autocomplete, overlays | `host/ask`, `host/say`, one-shot above-editor widget snapshot |
| Files, no code | themes, `keybindings.json`, prompt templates, `SYSTEM.md`/`APPEND_SYSTEM.md`, project settings | **themes, prompt templates, keybindings, status line (Level 0, below)**; `~/.gray/AGENTS.md` + project `AGENTS.md`/`CLAUDE.md` |
| Providers/models | `registerProvider`, virtual models | provider plugins; no routing |
| Session state | `appendEntry`, `sendMessage`, custom message types | plugin-private state |
| Distribution | pi packages (npm/git) bundling extensions, skills, prompts, themes | plugin index, skills installer |

## Level 0: files, no code

Shipped:

- **User themes.** `~/.gray/themes/<name>.json`, picked with `/theme <name>`
  (saved as `"theme"` in `config.json`; `GRAY_THEME` overrides per run).
  Gray still ships one palette and no other built-ins; a theme overrides
  only the roles it names (the `UiTheme` roles in `crates/gray/src/theme.rs`).
  Colors: `#rrggbb`, `#rgb`, ANSI names, 0-255 indexes, `default` (the
  terminal's own color), or names from a `vars` map. `/theme new <name>`
  writes the current palette as a starting point; `/theme reload` re-reads
  the active file. Lines already in the transcript keep the colors they
  were drawn with.
- **Prompt templates.** `prompts/*.md` become `/name`. Pi's placeholder
  syntax (`$1`, `$@`, `$ARGUMENTS`, `${1:-default}`, `${@:2}`, `${@:2:1}`);
  a body without placeholders gets the arguments appended. Searched:
  project `.gray/prompts`, `.pi/prompts`, `.claude/commands` (cwd up to
  the git root), then `~/.gray/prompts`, `~/.pi/agent/prompts`,
  `~/.claude/commands`. First name wins; built-ins always win; a template
  shadows a plugin command of the same name.
- **Keybindings.** `~/.gray/keybindings.json` uses pi's file format and
  ids (`tui.editor.cursorWordLeft`, `tui.input.submit`, `app.interrupt`,
  `app.exit`, …), so a pi `keybindings.json` mostly works as is: a value
  (string or list) replaces that action's defaults, `[]` unbinds, pi ids
  gray doesn't implement are skipped quietly. Gray adds slash-command
  bindings: `"/compact": "ctrl+shift+k"`. Ctrl+C always keeps clearing /
  cancelling. Every editor and mid-turn key goes through one table
  (`crates/gray/src/keymap.rs`); `/hotkeys` lists the live bindings,
  `/hotkeys reload` or `/reload` re-reads the file.
- **`/reload`.** Re-reads the theme, keybindings and prompt templates
  without restarting.
- **Status line.** `"status_line"` in `config.json` lays out the footer
  as two segment lists:

  ```json
  "status_line": {
    "left":  ["context", "cache", "timer", "work", "⎇ {branch}"],
    "right": ["status", "command", "model", "effort"],
    "separator": " · ",
    "command": "~/bin/gray-status",
    "interval_ms": 5000
  }
  ```

  Segments: `context`, `cache`, `timer`, `work`, `model`, `effort`,
  `cwd`, `dir`, `branch`, `command`, `status` (all plugin statuses) and
  `status:<key>`. `{name}` inside a string is a template; anything else is
  literal text. Empty segments drop out with their separator. `command`
  runs through `sh -c` every `interval_ms` (floor 500ms, 3s timeout) with
  a JSON snapshot on stdin (`cwd`, `workspace.current_dir`, `model.id`,
  `model.display_name`, `context`, `branch`, …, the Claude Code statusline
  fields); its first stdout line is the `command` segment. Plugin
  `host/ui/status` (protocol 2.0, below) feeds `status`. `/reload` picks
  up edits.
- **System prompt.** Already covered: `~/.gray/AGENTS.md` is the editable
  system prompt (pi's `SYSTEM.md`), the nearest project `AGENTS.md`/`CLAUDE.md`
  is appended per turn (pi's `APPEND_SYSTEM.md`/context files).

Next:

- **Project settings.** `.gray/settings.json` merged over `config.json`
  for an allowlist of keys (model, effort, theme, context limits, lean),
  only for trusted projects.

## Level 1: plugin protocol 2.0

The existing manifest `hooks` claim list stays the subscription mechanism:
a sidecar is only called for what it claims, so unclaimed events cost no
IPC. 2.0 widens what can be claimed and what a sidecar can call back.

### Events (host → sidecar)

Mutating events return a patch or a verdict; observe-only events are
notifications. Each maps to the pi event an ported extension expects.

| gray method | pi event | reply |
|---|---|---|
| `input/submit` | `input` | `{text}` rewrite, `{handled:true}` swallow, or nothing |
| `agent/before_start` | `before_agent_start` | system prompt append/replace, injected messages |
| `context/build` | `context` | replacement message list for this model call |
| `provider/before_request` | `before_provider_request` | headers/body patch |
| `tool/before` (exists) | `tool_call` | allow/deny/patch args |
| `tool/after` | `tool_result` | replacement result content |
| `compact/before` | `session_before_compact` | custom summary, or cancel |
| `session/before_switch`, `/before_fork` | same | cancel |
| `model/select` | `model_select` | observe |
| `bash/user` | `user_bash` | handle `!cmd` instead of the shell |
| `turn/start`, `turn/end` (exists), `agent/settled`, `message/*`, `tool/execution_*`, `session/start`, `session/shutdown` | same | observe (notifications) |

### Calls (sidecar → host)

- `host/ui/notify|select|confirm|input|editor`: `host/ask` generalized.
- `host/ui/status {key, text}`: footer status slots.
- `host/ui/widget {key, lines|spec, placement}`: live, updatable widgets
  above or below the editor (replaces the one-shot snapshot).
- `host/ui/footer|header|title`, `host/ui/working {visible, message}`.
- `host/ui/editor_text {get|set}`.
- `host/session/append {type, data}`: plugin state stored in the session
  JSONL, replayed to the plugin on resume/fork (pi `appendEntry`).
- `host/message/send {role, content, display}` and
  `host/user_message/send`: inject into the conversation (pi
  `sendMessage` / `sendUserMessage`).
- `host/tools/set_active {names}`: narrow or widen the model's tool list,
  built-ins included (pi `setActiveTools`).

### Rendering

Sidecars cannot paint the terminal, so rendering is declarative: a tool
or custom message type registers a render spec (styled spans by theme
role, simple rows/columns, collapsible body) and the host draws it with
ratatui. Covers pi's `renderCall`/`renderResult`/`registerMessageRenderer`
for the common cases; arbitrary components need Level 2.

### Manifest additions

`shortcuts` (key → command), `flags` (CLI flags passed through to the
plugin), `message_types`, `renderers`. Plus `/reload` to restart sidecars
without restarting the session.

### Order

Build 2.0 by porting pi examples one at a time and adding exactly what each
needs: permission-gate, plan-mode, todo, custom-compaction,
input-transform, status-line, then handoff, git-checkpoint, notify.

## pi-compat

A sidecar (`~/grayplugins/gray-pi-compat`, Node or Bun) implementing pi's
`ExtensionAPI` and `ctx.ui` on top of protocol 2.0, loading
`~/.pi/agent/extensions`, `.pi/extensions` and pi packages unchanged.
pi's own example extensions are its test suite. Out of reach without
Level 2: `ctx.ui.custom` components, `setEditorComponent`, raw
`onTerminalInput`. Those calls degrade to a notice instead of crashing.

## Level 2: in-process scripting (only if needed)

What IPC cannot serve: per-keystroke hooks, per-frame rendering, full
overlays. Recommendation: **declarative specs first; if that falls short,
embed QuickJS (`rquickjs`), not Lua or Rhai.** Reasons: pi extensions and
pi-compat are already JS/TS, so users write one language for both tiers;
QuickJS is ~1 MB with no Node; pi itself already embeds QuickJS for
codemode. Lua is faster and smaller but adds a third language; Rhai is the
easiest to sandbox but nobody already knows it. Decide after pi-compat
shows which examples still fail.

## Also

- Virtual/router models: a plugin picks model + effort per request.
- Gray packages: one manifest bundling plugins, skills, prompts, themes,
  keybindings; `gray install github:owner/repo`.
- `gray doctor` lists every loaded customization, its source and any
  shadowing (template vs plugin command, plugin tool vs built-in).
