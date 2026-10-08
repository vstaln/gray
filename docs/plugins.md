# Sidecar plugins: questions + permissions (`host/ask`)

Thin Rust sidecars own schema/policy; the gray host owns user I/O via
`host/ask`. Two first-party examples:

- `questions` (`vstaln/gray-questions`): the AI asking you clarifying
  questions (`request_user_input`, 1–3 questions, options + free-form notes).
- `permissions` (`vstaln/gray-permissions`): the guard on tool calls
  ("hey, can I run this command?") via `tool/before` plus
  `/permissions` (`/perms`, `/access`).

## Install

```sh
gray plugin install <index-name>            # verified (index hash)
gray plugin install <release-tarball-url>   # unverified, warned
gray plugin install /path/to/executable     # register a local binary
```

`plugin` and `plugins` are the same command (`visible_alias`). A local
executable install probes `manifest` before registering anything; a
`gray-<name>` already on PATH registers with `gray plugin install <name>`
too. `GRAY_PLUGIN_PATH=/path/to/bin gray plugin install <name>` overrides
the binary a name resolves to. Skill specs (`clawhub:<owner/slug>`,
`github:<owner/repo[/path]>`, `url:<https-url>`) install a `SKILL.md`
bundle into `~/.gray/skills` instead of a plugin.

Index entries are `gray-native`/`tarball` with `sha256:<hex>`:

```json
{"ecosystem": "gray-native", "version": "0.1.0",
 "source": {"type": "tarball", "url": "https://…/questions-<target>"},
 "hash": "sha256:<hex>", "scope": ""}
```

## Where plugins live

Plugins are not developed in this repo — gray keeps only the host side
(protocol, loader, installer) plus test fixtures. Each plugin is its own
repository under [github.com/vstaln](https://github.com/vstaln):

- [gray-account](https://github.com/vstaln/gray-account)
- [gray-antigravity-sub](https://github.com/vstaln/gray-antigravity-sub)
- [gray-background](https://github.com/vstaln/gray-background)
- [gray-claude-sub](https://github.com/vstaln/gray-claude-sub)
- [gray-codex-sub](https://github.com/vstaln/gray-codex-sub)
- gray-devin-sub (local only — no GitHub remote yet)
- [gray-discord-plugin](https://github.com/vstaln/gray-discord-plugin)
- [gray-ledger](https://github.com/vstaln/gray-ledger)
- gray-memory (local only — no GitHub remote yet): curated cross-session
  memory, extracted from core. Serves the per-session snapshot through
  `prompt/context` (`session.id` pins the freeze), owns `/memory` over
  `command/run`, and keeps `gray memory …` through `cli_argv` forwarding.
  `/memory off` writes its own marker (`<home>/memory/enabled`) seeded once
  from the legacy `memory_auto` config key.
- [gray-permissions](https://github.com/vstaln/gray-permissions)
- [gray-questions](https://github.com/vstaln/gray-questions)
- [graysearch](https://github.com/vstaln/graysearch)
- [gray-subagents](https://github.com/vstaln/gray-subagents)

Install one by building it so `gray-<name>` is on PATH, then registering:

```sh
cargo install --git https://github.com/vstaln/<repo> --locked
gray plugin install <name>
```

or, for plugins published in the index, `gray plugin install <index-name>`.
The live index is served from
`https://gray.alignment.id/plugins/index.json` (source:
`vstaln/graysite`, `public/plugins/index.json`) — it is not kept here.

User-written plugins may be Python, a shell script, or anything else that
runs: `gray plugin install /path/to/my-plugin` registers any executable,
and a plugin directory containing a `plugin.sh` is spawned as-is.

## Wire (v1)

Host→sidecar is NDJSON over stdio: `plugin/manifest`, `tool/call`,
`tool/before`, `command/run`, `prompt/context`, `event/notify` (no reply),
`plugin/shutdown` (clean exit). Sidecar→host asking is one method:

```json
{"id": "q1", "method": "host/ask",
 "params": {"questions": [{"id": "color", "header": "Color",
   "question": "Which color?",
   "options": [{"label": "Red", "description": "warm"}]}],
  "blocking": true}}
```

The host replies `{"id": "q1", "result": {"answers": …}}` or
`{"id": "q1", "error": …}`. Asking sidecars claim protocol `"1.1"` in
`plugin/manifest`; pre-1.1 sidecars keep the 30s fail-fast.

## TTLs

- Plugin inner TTL: 300s (both sidecars `recv_timeout(300s)` so the plugin
  reports its own timeout, never the host's generic one).
- Host handler TTL (`ASK_HANDLER_TTL`): 300s per `host/ask` task.
- Host outer TTL (`ASK_TTL`): 330s on `tool/call`/`tool/before` for
  protocol-1.1 sidecars; everything else keeps `HOST_TTL` 30s.
- `plugin/tools` (protocol 1.3): `HOST_TTL` 30s per refresh.

## Protocol 1.3: dynamic tools + media

A sidecar whose tool set changes while the session runs (a bridge to an
external tool server, a tool marketplace) claims `"protocol": "1.3"` in
`plugin/manifest`.
1.3 implies 1.1 (asking and its TTLs still apply). The manifest `tools`
array is only a hint and may be `[]`; the live set comes from
`plugin/tools`:

```json
{"id": 7, "method": "plugin/tools", "params": {}}
{"id": 7, "result": {"tools": [{"name": "fs_read", "description": "…",
  "parameters": {"type": "object", "properties": {}}}]}}
```

The host asks once at spawn and again after every
`{"method": "host/tools_changed", "params": {}}` the sidecar emits (a
notification: no `id`, no reply; bursts are debounced 200 ms into one
refresh). Tool entries use the manifest tool shape. The model's tool list
is rebuilt at the start of every turn from the current set; builtin names
(`bash`, `read`, …) can never be shadowed by a live tool.

A 1.3 `tool/call` reply may carry media next to `content`:

```json
{"id": 3, "result": {"content": "here",
  "images": [{"mime": "image/png", "data_base64": "iVBOR…"}],
  "media": [{"mime": "application/pdf", "data_base64": "JVBER…",
             "fallback": "text a model without PDF input sees"}]}}
```

An entry missing `mime` or `data_base64` is dropped with a warning; the
rest of the reply stands. Media is passed through un-re-encoded, so the
sidecar keeps each item under the native media cap (8 MiB).

For a complete 1.3 sidecar, see [gray-mcp](https://github.com/vstaln/gray-mcp).

## Semantics

- `blocking: false` resolves empty immediately in v1 (no follow-up
  message injection).
- Bad tool args are rejected without asking.
- No host handler is a loud `{"error": …}`, never a hang.
- Empty/timeout answers deny: permissions fails closed, `request_user_input`
  reports no user reachable.

## Headless (cron/gateway, piped stdin, `-p`)

No TTY prompt exists there: piped stdin takes one number-or-free-text
line per question (blank/EOF skips); anything else resolves empty
immediately. Permissions denies; questions reports no user reachable.

## App setup

Apps own their setup. An app that declares `setup` in its manifest shows a
`gray <name> setup` hint in `/gateway`; running it executes the app's own
wizard (for Discord: `gray discord setup`, which asks, writes its config
privately, runs its own `doctor`, and installs the daemon under whatever
init the box has). Gray core keeps the picker and the forwarding — the
flow itself is plugin code.

## ChatGPT subscription provider

The ChatGPT/Codex subscription provider lives outside this repo, in
[`vstaln/gray-codex-sub`](https://github.com/vstaln/gray-codex-sub), as the
standalone `gray-codex-sub` protocol-1.2 sidecar. Build it there, then register
the binary:

```sh
gray plugin install /path/to/gray-codex-sub
```

A sidecar that does not answer `<bin> manifest` is probed over the sidecar
wire instead, and the install asks for the `provider.credentials` capability.
Declined consent hides the provider row; grant it later with
`gray plugin capabilities <name> --all`. Removal: `gray plugin remove <name>`.

## Port candidates (hermes + pi)

Consolidated 2026-10-08 from `customization.md` (pi parity plan), the
pi example catalog (`reference/pi-mono/packages/coding-agent/examples/extensions/`,
79 examples) and the hermes plugin dir (`~/.hermes/hermes-agent/plugins/`,
18 plugins).

### The genuine count

- **pi:** 79 example extensions. ~13 are toys/demos/test fixtures
  (snake, tic-tac-toe, space-invaders, doom-overlay, pirate,
  rainbow-editor, hello, overlay-test, overlay-qa-tests, rpc-demo,
  working-message-test, qna, mac-system-theme on this Linux box).
  ~9 already have a gray equivalent (protected-paths + project-trust →
  gray-permissions/trust; question + questionnaire → gray-questions;
  subagent → gray-subagents; custom-footer + working-indicator +
  minimal-mode → Level-0 status_line/lean; reload-runtime → `/reload`).
  **9 are the named protocol-2.0 drivers** below; the remaining ~48 are
  meant to run *unmodified* through gray-pi-compat, not be ported one
  by one.
- **hermes:** 18 plugins. 4 already covered or partially covered
  (memory → gray-memory; platforms → gray-discord-plugin, discord only;
  model-providers → the `gray-*-sub` provider sidecars; cron_providers →
  core `gray cron`). **14 genuine candidates** below.

Genuinely new port units: **23 named** (9 pi drivers + 14 hermes),
plus ~48 pi extensions absorbed wholesale by pi-compat.

### pi — protocol-2.0 drivers (port in this order, per customization.md)

1. `permission-gate` — confirm dangerous bash commands (partly covered
   by gray-permissions; port for the prompt UX)
2. `plan-mode/` — read-only exploration mode with `/plan`
3. `todo` — todo tool + `/todos` with custom rendering + persistence
4. `custom-compaction` — custom summarize-everything compaction
5. `input-transform` — rewrite submitted input
6. `status-line` — live turn progress in the footer
7. `handoff` — `/handoff <goal>` into a fresh focused session
8. `git-checkpoint` — stash checkpoint per turn, restore on fork
9. `notify` — OSC 777 desktop notification when the agent settles

### pi — absorbed by gray-pi-compat (no individual port)

Real value once protocol 2.0 + the compat sidecar exist:
auto-commit-on-exit, bash-spawn-hook, bookmark, border-status-editor,
built-in-tool-renderer, claude-rules, commands, confirm-destructive,
custom-header, custom-provider-anthropic, custom-provider-gitlab-duo,
debug-provider, dirty-repo-guard, dynamic-resources, dynamic-tools,
entry-renderer, event-bus, file-trigger, github-issue-autocomplete,
git-merge-and-resolve, gondolin, hidden-thinking-label, inline-bash,
input-transform-streaming, interactive-shell, jev-router,
message-renderer, modal-editor, model-status, preset,
prompt-customizer, provider-payload, sandbox, send-user-message,
session-name, shutdown-command, ssh, structured-output, summarize,
system-prompt-header, timed-confirm, titlebar-spinner, tool-override,
tools, trigger-compact, truncated-tool, widget-placement, with-deps. (48)

### hermes — 14 candidates

| plugin | what it is | note |
|---|---|---|
| `browser` | browser automation tool | high value, big surface |
| `web` | web search/fetch tools | overlaps graysearch? check first |
| `image_gen` | image generation (openrouter etc.) | straight tool port |
| `video_gen` | video generation | straight tool port |
| `spotify` | 7 playback/queue/search tools, PKCE OAuth | self-contained |
| `kanban` | kanban board tool | self-contained |
| `disk-cleanup` | auto-clean ephemeral session files via hooks | needs lifecycle hooks |
| `security-guidance` | appends warnings to dangerous file writes | maps to `tool/after` |
| `observability` | turn tracing/telemetry | design first |
| `context_engine` | pluggable context engine provider | needs provider iface |
| `dashboard_auth` | auth for a web dashboard | only with a dashboard |
| `google_meet` | join/transcribe/speak in Meet calls | heavy, v1 transcribe-only |
| `teams_pipeline` | Teams transcript-first meeting summaries | heavy, Graph-backed |
| `hermes-achievements` | achievement system | novelty, low |

Not candidates: `memory` (gray-memory), `platforms`
(gray-discord-plugin covers the pattern; other platforms = new sidecars
of the same shape), `model-providers` (gray `*-sub` sidecars cover
subscriptions; generic profiles would be provider-plugin work),
`cron_providers` (core `gray cron` exists).
