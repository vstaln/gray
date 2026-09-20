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
gray plugin install <release-tarball-url>   # day one (unverified, warned)
gray plugins install questions              # after index publish (verified)
gray plugins install permissions            # after index publish (verified)
```

`plugin` and `plugins` are the same command (`visible_alias`). `git:`
installs do NOT carry sidecars (that arm only extracts pi skills), so a
release tarball URL or an index name is the only day-one path.

Index entries are `gray-native`/`tarball` with `sha256:<hex>`:

```json
{"ecosystem": "gray-native", "version": "0.1.0",
 "source": {"type": "tarball", "url": "https://…/questions-<target>"},
 "hash": "sha256:<hex>", "scope": ""}
```

Local verification points `GRAY_PLUGIN_INDEX` at a loopback index serving
the same shape.

## First-party catalog (`background`, `discord`)

`gray plugin install background|discord` installs from gray's own catalog,
pinned by commit. Neither needs Python:

- `background` downloads a prebuilt, checksum-verified release binary.
- `discord` clones its pinned commit and runs `cargo build --release
  --locked`, so it needs a Rust toolchain and compiles exactly the code that
  was reviewed. The binary is published to `<gray-home>/plugins/discord/` and
  registered as a sidecar in `plugins/lock.json`.

Two guards make the build path trustworthy: the catalog pin must be a full
40-character commit ID (refs and short prefixes are rejected before git
runs), and the built binary must answer `plugin/manifest` with the expected
name before anything is registered. A failed build leaves no partial
install behind.

User-written plugins may still be Python, a shell script, or anything else
that runs: `GRAY_PLUGIN_PATH=/path/to/my-plugin gray plugin install myname`
registers any executable, and a plugin directory containing a `plugin.sh` is
spawned as-is. What is Rust-only is gray's *own* catalog.

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
