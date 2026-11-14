# Plugins

Plugins are sidecar processes that speak NDJSON over stdio, in any
language. Gray keeps only the host side (protocol, loader, installer);
every plugin is its own repository under
[github.com/vstaln](https://github.com/vstaln), and the index of
published plugins is served from
`https://gray.alignment.id/plugins/index.json` (source: `vstaln/graysite`,
`public/plugins/index.json`).

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
the binary a name resolves to.

Index entries are `gray-native`/`tarball` with `sha256:<hex>`:

```json
{"ecosystem": "gray-native", "version": "0.1.0",
 "source": {"type": "tarball", "url": "https://…/questions-<target>"},
 "hash": "sha256:<hex>", "scope": ""}
```

## Writing your own

A plugin may be Rust, Python, a shell script, or anything else that runs:
`gray plugin install /path/to/my-plugin` registers any executable, and a
plugin directory containing a `plugin.sh` is spawned as-is. To build a
plugin from source so `gray-<name>` is on PATH, then register it:

```sh
cargo install --git https://github.com/vstaln/<repo> --locked
gray plugin install <name>
```

## Wire (v1)

Host→sidecar is NDJSON over stdio: `plugin/manifest`, `tool/call`,
`tool/before`, `command/run`, `prompt/context`, `event/notify` (no reply),
`plugin/shutdown` (clean exit).

### Contract rules every sidecar must follow

- **Requests carry an `id` and must get exactly one reply**
  (`{"id": …, "result": …}` or `{"id": …, "error": …}`) — even unknown
  methods, which answer `{"error": {"code": -32601, …}}`. A request that
  goes unanswered stalls the host for the full TTL.
- **Notifications carry no `id` and produce no output.** `event/notify`
  and `plugin/shutdown` are notifications: replying emits a stray
  `{"id": null, …}` line the host can misroute. `plugin/shutdown` means
  exit promptly (within ~5s; the host escalates to SIGKILL). stdin EOF is
  the same contract — exit, or a host crash strands the sidecar.
- **stdout is wire-only.** Anything that isn't an NDJSON reply or an
  outbound request corrupts the stream; diagnostics go to stderr.
- **`argv[0] manifest` prints the same JSON `plugin/manifest` returns.**
  The installer probes it before registering and `gray plugin check`
  diffs it against the wire answer.

The full sidecar-author contract (message shapes, lifecycle, error
forms) lives in `PLUGIN-CONTRACT.md` at the root of the plugin source
tree (`grayplugins/`).

Protocol 2.0 adds mutating events, each claims-gated by `hooks` like the
v1 methods (unclaimed = no IPC):

- `input/submit` — `{"text","session"}` → `{text}` rewrite,
  `{handled: true}` swallow (no turn, nothing recorded), else pass.
- `agent/before_start` — `{"session"}` → `{text}` appended to history as a
  user message before the turn's first request (skipped under `--lean`).
- `context/build` — `{"messages","session"}` → `{"messages":[...]}`
  replaces the outbound list for that request only; the persisted
  transcript is never touched. Hooks chain in claim order.
- `tool/after` — `{"name","content","is_error","session"}` →
  `{content?, is_error?}` replaces the tool result before observers and
  history see it.

## Host calls

A sidecar can call back into the host. Each call needs the matching
capability, which the user grants at install (`gray plugin capabilities`):

- `host/ask`: a blocking question in the middle of a tool call (below).
- `host/say`: print a line into the conversation.
- `host/run`: run an agent turn as the user, with their model, credentials
  and budget.
- `host/sessions`: list running gray sessions (ids and pids).
- `host/restart`: restart gray on the binary on disk, resuming the
  conversation.

`host/ask` looks like this:

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

- Plugin inner TTL: an asking plugin should give up on its own wait at
  300s, so it reports its own timeout instead of the host's generic one.
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

## Tool display: `label` and `preview`

A tool entry (in the manifest's `tools`, or in a 1.3 `plugin/tools` reply)
may set two display-only fields. Neither reaches the model, and the wire args
of a call are never changed.

- `label` is the transcript headline. Without one, the wire name is
  humanized: `rlm_spawn` shows as `Rlm Spawn`.
- `preview` is the muted text after the headline. It takes one of two forms.
  - A **dot path** with no `{`, such as `"document.title"`, shows that string
    from the args, trimmed.
  - A **template**, any preview containing `{`, renders the args:
    - `{a.b}` reads a dot path. Strings are trimmed, numbers and booleans
      print as text, arrays join their scalar items with `, `, and objects,
      null, and missing paths are empty.
    - `{x|model}` shows a model id the way gray's footer names it, without the
      vendor word: `claude-haiku-5-5` shows as `Haiku 5.5`.
    - `{x|firstline}` keeps the first non-empty line, capped at 80 cells with
      `…`. An unknown filter reads as no filter.
    - Segments split on ` · `. A segment whose placeholders all come out empty
      is dropped along with its literal text; the rest join with ` · `.
    - A line break, a real newline or the two characters `\n`, starts a second
      line. It shows muted under the headline and is omitted when empty.

A template owns the whole suffix. When it renders empty, the headline stands
alone, with no raw `key=value` dump after it.

```json
{"name": "rlm_spawn", "label": "Started agent",
 "preview": "{name} · {model|model} · {effort}\n{prompt|firstline}"}
```

For the args `{"name": "e2e-theme", "model": "claude-haiku-5-5",
"effort": "max", "prompt": "verify the theme fixes\nmore"}` the header reads
`Started agent e2e-theme · Haiku 5.5 · max`, with `verify the theme fixes` on
the line under it.

## Composer actions

A manifest can bind keys in the input box to a plugin command:

```json
"composer_actions": [
  {"keys": ["ctrl+space", "f8"], "argv": ["dictate", "6"], "busy": "listening, then transcribing"}
]
```

Pressing a key runs the plugin's CLI argv followed by `argv`. While it runs,
the input box shows `busy` above it and a blinking prompt glyph, and Esc
kills it. Trimmed stdout is inserted at the caret. A non-zero exit or empty
output shows the last line of stderr as an error. Keys already bound in
`keybindings.json` win over plugin keys. The plugin needs a CLI entry
(`cli_argv` in the lock), and actions load at startup.

## Settings

A manifest can add rows to `/settings`:

```json
"settings": [
  {"key": "auto_copy", "label": "Copy answers automatically", "type": "bool", "default": false},
  {"key": "style", "label": "Answer style", "type": "choice", "choices": ["short", "long"], "default": "short"}
]
```

Rows appear under the plugin's `display_name` (or name). Enter flips a `bool` or
moves to the next `choice`. Gray writes the values to
`$GRAY_HOME/plugin-settings/<plugin>.json` as a flat object, e.g.
`{"auto_copy": true}`; keys you never changed are absent, so fall back to your
default. Read the file when you need a value; there is no change notification.

## `host/ask` semantics

- `blocking: false` resolves empty immediately in v1 (no follow-up
  message injection).
- Bad tool args are rejected without asking.
- No host handler is a loud `{"error": …}`, never a hang.
- An empty or timed-out answer means no user answered. A guard plugin
  (like `gray-permissions`) should treat it as a denial.

## Headless (cron/gateway, piped stdin, `-p`)

No TTY prompt exists there: piped stdin takes one number-or-free-text
line per question (blank/EOF skips); anything else resolves empty
immediately.

## App setup

Apps own their setup. An app that declares `setup` in its manifest shows a
`gray <name> setup` hint in `/gateway`; running it executes the app's own
wizard (for Discord: `gray discord setup`, which asks, writes its config
privately, runs its own `doctor`, and installs the daemon under whatever
init the box has). Gray core keeps the picker and the forwarding — the
flow itself is plugin code.

## Provider plugins

Subscription providers (`gray-codex-sub`, `gray-claude-sub`, `gray-devin-sub`
and the rest) are protocol-1.2 sidecars. Build one, then register the binary:

```sh
gray plugin install /path/to/gray-<name>-sub
```

A sidecar that does not answer `<bin> manifest` is probed over the sidecar
wire instead, and the install asks for the `provider.credentials` capability.
Declined consent hides the provider row; grant it later with
`gray plugin capabilities <name> --all`. Removal: `gray plugin remove <name>`.
