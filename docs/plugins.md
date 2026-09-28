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

## Foreign packages (any plugin, no per-plugin code)

A `git:` install takes three things from any package, all markdown-only —
package code (hooks, MCP servers, lifecycle scripts) is never executed:

- `skills/*/SKILL.md` → the skill loader (unchanged).
- `commands/*.md` and `.opencode/command/*.md` → slash commands. The file
  body runs as a prompt (`command/run` → `Prompt`) with `$ARGUMENTS`
  substituted; the frontmatter `description:` names it in `/help`.
  `commands/` wins a stem both layouts declare.
- `<pkg>/AGENTS.md` → appended to every turn's system prompt
  (`prompt/context`), rationale-stripped and capped like project rules.

An optional `<pkg>/gray.json` adds a generic mode switch without any
plugin-specific code in gray:

```json
{"state_file": ".mode", "state_prefix": "Level: ",
 "state_commands": ["ponytail"]}
```

A listed command writes its argv to the state file (empty argv clears it)
and confirms briefly; the file's content is appended to the inject block.
Corrupt manifests, traversal-y state paths, and missing files all read as
"package stays static" — never an error, never a half-state.

Local verification points `GRAY_PLUGIN_INDEX` at a loopback index serving
the same shape.

## First-party catalog (`background`, `discord`)

`gray install plugin background|discord` installs from gray's own catalog,
pinned by commit. Neither needs Python:

- `background` downloads a prebuilt, checksum-verified release binary.
- `discord` clones its pinned commit and runs `cargo build --release
  --locked`, so it needs a Rust toolchain and compiles exactly the code that
  was reviewed. The binary is published to `<gray-home>/plugins/discord/` and
  registered as a sidecar in `plugins/lock.json`.

Two guards make the build path trustworthy: the catalog pin must be a full
40-character commit ID (refs and short prefixes are rejected before git
runs), and the built binary must answer `plugin/manifest` with the expected
name before anything is registered. A failed build, or a binary that fails
that check, leaves nothing behind -- the artifact is still inside the build
tempdir when it is verified, so nothing is published until it passes.

Note the two spellings: `gray install plugin <name>` is this catalog path,
while `gray plugin install <spec>` is the package manager, which resolves
names through the gray-pkg index and its own tarball URLs.

User-written plugins may still be Python, a shell script, or anything else
that runs: `GRAY_PLUGIN_PATH=/path/to/my-plugin gray install plugin myname`
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

## App setup (`/gateway`)

Apps that gray talks to (Discord today; slack, telegram, … tomorrow) each
declare what setup needs, and `/gateway` drives it — no separate wizard,
no TTY requirement, no systemd assumption:

```text
/gateway            # connections picker; a needs-setup row opens the flow
gray gateway setup discord --field token=… --field owner_id=… --field channel_id=…
```

The flow asks for exactly what the app declares (masked for secrets, with
the portal URL beside each field), writes the app's config privately
(dir `0700`, file `0600`, atomic merge that keeps unknown keys), runs the
app's own `doctor` as the referee, registers the app's outgoing tool, and
starts the daemon under whatever init the box has — runit, systemd user,
or gray itself (detached, pidfile next to the config). A failed doctor is
reported verbatim; nothing prints success until it is true. Secrets never
appear in logs, errors, or anything the model can see.

A `channel`-kind field (Discord's home channel) offers a picker instead of
paste-an-ID: the bot's servers, then that server's channels newest-first
(snowflake IDs are time-ordered), with the DM between the bot and its
owner on top. Paste-an-ID always remains, and is the headless path.

Declarations live in gray's catalog for first-party apps
(`plugin_cli::setup_decl`); second-party apps self-declare through the
sidecar `plugin/manifest` wire. Budgets are not part of setup — an app's
own accounting command (`gray discord budget set`) turns that on if you
want it.

## codex-auth (ChatGPT subscription provider)

`codex-auth` is a protocol-1.2 provider plugin. It owns the ChatGPT
subscription OAuth flow and returns credential references, never tokens, to
gray. After it is installed, `/connect` shows a **Codex — ChatGPT
subscription** row.

Install from a source checkout:

```sh
cargo build -p codex-auth
GRAY_PLUGIN_PATH="$PWD/target/debug/codex-auth" gray plugin install codex-auth
```

The install probes the plugin over the sidecar wire and asks for the
`provider.credentials` capability. Declined consent means the provider row is
hidden; grant it later with:

```sh
gray plugin capabilities codex-auth --all
```

`/connect` opens the ChatGPT login page and waits for the loopback callback on
`127.0.0.1:1455` or `127.0.0.1:1457`. The plugin validates PKCE and exact
state, redirects are disabled, and the account id travels only as a non-secret
metadata header.

Upgrade = rebuild the plugin and re-run the install command. Removal:
`gray plugin uninstall codex-auth`, which removes the plugin lock entry and
provider cache row; `/connect` removes the namespaced credential from
`~/.gray/auth.json` on request.
