# gray plugins — authoring guide

Sidecars are child processes speaking newline-delimited JSON over stdio.
Frozen wire spec: [`protocol-v1.md`](protocol-v1.md) (v1.1).
Machine schemas: [`schema/manifest.v1.json`](schema/manifest.v1.json),
[`schema/protocol.v1.json`](schema/protocol.v1.json).
Reference implementation: [`plugins/echo/echo.sh`](../plugins/echo/echo.sh)
(copy it as your starting point).

## Manifest (`plugin/manifest` → result)

```json
{"name":"echo","version":"0.1.0","protocol":"1.1",
 "tools":[{"name":"echo","description":"Echo text back",
   "parameters":{"type":"object","properties":{"text":{"type":"string"}},"required":["text"]},
   "snippet":"echo <text>"}],
 "commands":["/echo"],
 "hooks":["turn/end"],
 "capabilities":[],
 "subcommands":[]}
```

- `name` (non-empty, else boot bails), `version`, `tools` are required.
  Pre-v1 `"tools":["name"]` bare strings still parse.
- `commands` are literal `/`-prefixed names routed from the REPL.
- `hooks`: `prompt/context`, `tool/before` gate requests; `turn/end` is
  informational (events arrive as `event/notify` regardless).
- `protocol: "1.1"` opts into `plugin/shutdown` + `session` params.
  Absent = pre-v1: unknown lines ignored, never sent shutdown.
- `capabilities`: advisory sandbox declaration (`exec`, `http`,
  `session`, `ui`). Parsed, surfaced in `--dump-manifest`, schemad —
  **not enforced yet**.
- `subcommands` (e.g. `/cron`): host-owned namespaces the plugin extends.
  Argv forwards over the same `command/run` wire as `commands`.

## Methods and TTLs

| Method | Direction | Kind | TTL | Params → result |
|---|---|---|---|---|
| `plugin/manifest` | host→sidecar | request | 30 s | — → manifest |
| `tool/call` | host→sidecar | request | 30 s | `{name,args,session}` → `{content,is_error?}` |
| `prompt/context` | host→sidecar | request, gated on `hooks` | 30 s | `{cwd,session}` → `{text}` |
| `tool/before` | host→sidecar | request, gated on `hooks` | 30 s | `{name,args,session}` → `{decision: allow/deny/modify,…}` |
| `command/run` | host→sidecar | request, gated on `commands`/`subcommands` | 30 s | `{name:"/x",argv,session}` → `{text}` **or** `{prompt}` |
| `event/notify` | host→sidecar | notification (no `id`, no reply) | 5 s write | `{type: pre_step/pre_tool/post_tool/turn_end,…}` |
| `plugin/shutdown` | host→sidecar | notification, v1.1 only | 5 s write | `{reason: session_end}` — exit promptly |
| `host/run` | sidecar→host | request (**string** `id`) | 30 s | `{session,prompt}` → `{text}` or `{error}` |
| `host/say` | sidecar→host | request (**string** `id`) | 30 s | `{text}` → `{ok:true}` or `{error}` |

Rules: host ids are numbers, sidecar ids are strings — the namespaces
never collide. Gated methods are only sent to sidecars claiming them, so
pre-v1 plugins (ignore unknown lines) keep working. `command/run`:
`{"prompt"}` wins over `{"text"}` — prompt makes the host run a turn,
text just prints. Without a host handler, `host/*` replies `{"error":…}`
(loud, never a hang). Both hosts install a real runner
(`gray::host::default_handler`, gateway `cron_host_handler`): `host/say`
queues for display (REPL-loop drain) or logs + saves under
`cron/output` (gateway); `host/run` replays the prompt through a fresh
`gray -p` child of the running binary and returns its stdout as
`{"text"}` (shared core: `gray_plugin::host::run_prompt_child`).
Ceiling: the 30 s per-request TTL still applies — a longer turn reports
a loud timeout (its side effects already happened).

Every v1.1 request/notification carries
`"session": {"id": <id or "">, "cwd": <cwd>}`.

## Check your plugin

```sh
gray plugin check ./my-plugin   # spawn + manifest + tool/call + notify + shutdown
python3 docs/schema/validate.py # reference-plugin vs schema (also in CI)
```

`check` resolves the argv from a directory (the dir itself when
executable, else `plugin.sh`, else the single executable inside) and
fails nonzero with per-check PASS/FAIL lines. Test the hang/crash/
reorder/empty-name modes against the fixtures in
`crates/gray-plugin/testdata/`.

## `/plugin` reference (REPL + CLI parity)

`/plugin` in the REPL (`/plugins` alias, case-insensitive) mirrors the
`gray plugin` CLI one-to-one; bare `/plugin` lists.

| Subcommand | REPL | CLI | What it does |
|---|---|---|---|
| `list` | `/plugin list` | `gray plugin list` | list installed plugins (`[disabled]` marks boot-skipped) |
| `search <q>` | `/plugin search <q>` | `gray plugin search <q>` | fans out over the Gray Index + Pi Gallery (preview) (total miss: `not in index: …`; pi-side failure prints the advisory line, exit 0) |
| `install <name\|url>` | `/plugin install <…>` | `gray plugin install <…>` | install by Gray Index name, https tarball URL, `npm:<pkg>[@<version>]`, or `git:<url>[@<ref>]` (pi installs are skills-only — see below) |
| `remove <name>` | `/plugin remove <name>` | `gray plugin remove <name>` | remove an installed plugin |
| `update [name\|all]` | `/plugin update` | `gray plugin update [all]` | update one plugin or everything (bare = `all`) |
| `enable <name>` | `/plugin enable <name>` | `gray plugin enable <name>` | re-enable a disabled plugin |
| `disable <name>` | `/plugin disable <name>` | `gray plugin disable <name>` | skip at boot without uninstalling |
| `check <dir>` | `/plugin check <dir>` | `gray plugin check <dir>` | conformance checks on a plugin dir (see above) |

Copy rule: the gray source is always labeled `Gray Index`; the pi
source is always labeled exactly `Pi Gallery (preview)` — same strings
as the `/plugin search` output rows. Official plugins below ship from
the Gray Index.

## Pi Gallery (preview)

A second plugin source drawn from the pi package universe. The
`(preview)` label is display-time copy only (the lock stores
`ecosystem: "pi-gallery"`, never the label), and "preview" means
exactly this: search is advisory and installs are **skills-only** —
anything beyond skills waits for the P3 runtime bridge.

Install specs:

| Spec | Example | What happens |
|---|---|---|
| `npm:<pkg>[@<version>]` | `npm:pi-foo`, `npm:@scope/bar@1.2.3` (split on the last `@`, so scoped names keep their leading `@`; unpinned resolves `dist-tags.latest`) | metadata via the npm registry; tarball downloaded and verified against `dist.integrity` (`sha512-<base64>`, `dist.shasum` fallback) |
| `git:<url>[@<ref>]` | `git:https://host/o/r.git@main`; raw `https://….git`, `ssh://`, `git://`, `git@host:…` forms parse too (ref splits on the last `@` after the authority) | shallow `git clone --depth 1` (+ `--branch <ref>` when pinned) via the `git` CLI — never reimplemented |

Locked behavior:

- Skills-only honesty: only `.md` skill files are copied, into
  `<plugins_dir>/pi/<name>/` (`@scope/name` becomes `scope-name`);
  package code is never executed. The installer reports
  `skills taken: …` and, when present,
  `skipped N extension files (P3)` / `skipped N theme files (P3)` —
  extensions/themes are left behind for P3. A package with no skills
  bails honestly (`ships no skills (nothing to install;
  extensions/themes need P3)`) and writes nothing.
- Installed pi skills light up in discovery with no extra step
  (`<agent_dir>/plugins/pi/<pkg>/` is a skills root).
- `git:` installs print `warning: unverified install …` (no index hash
  to check against; the lock records the post-clone commit sha).
  `npm:` installs are hash-verified and record the registry integrity
  string. Failures remove the staging dir / dest and write nothing (no
  half-state); reinstalls preserve the existing `enabled` flag.

Search scope: `search` fans out over the Gray Index (substring over the
cached index — hits first, name-sorted) plus the pi side (npm
`/-/v1/search`, up to 20 hits). Gray wins name collisions (the pi
duplicate is suppressed). Each hit renders
`name version [source] - desc`, with the ` - desc` suffix omitted when
empty — always the case for Gray Index hits (the index carries no
descriptions). The pi side never fails the search: on error it prints
exactly `Pi Gallery (preview): unreachable` and still exits 0. Miss
shape is locked: no hits with a reachable pi side renders the bare line
`not in index: <q> (try /plugin install <https-url>)` (REPL prints it;
the CLI exits nonzero with the same string); no gray hits with an
unreachable pi side prints the advisory line only.

Trust: a project-scoped plugin installs only after the project is
trusted — never auto-install from an untrusted checkout.

## Publish

Ship a directory with an executable (see `plugins/echo/`); users enable
it via `gray.yml` sidecar entries. Keep the manifest honest (only claim
hooks/commands you answer) and exit 0 on `plugin/shutdown`.

## Links

- Official plugins (the Gray Index seed,
  [`plugins/official.json`](../plugins/official.json)): `gateway` (source
  `plugins/gateway`). (`echo` stays a protocol reference only — see the top
  of this file — not an official plugin.)
- Gateway sidecar ([`plugins/gateway/gateway.sh`](../plugins/gateway/gateway.sh)):
  answers `/gateway` over `command/run` by delegating argv to the
  `gray gateway …` CLI (`status|install|uninstall|pairing|invite`),
  manifest `commands:["/gateway"]` + `capabilities:["exec"]`.
- Cron (in-process scheduler, not a sidecar): `gray-cron` holds the job
  store (`$GRAY_HOME/cron/jobs.json`) + schedule math; the gateway daemon
  fires due jobs on its 60 s claim-guarded ticker (`claim_due` is atomic,
  so concurrent tickers never double-run) and delivers back to chat
  wrapped (`Cronjob: …` + manage hint). Manage it with no daemon running:
  `gray cron list|add|remove|show` (`add "every 1h" "prompt"
  [--deliver telegram[:chat]] [--name x] [--in /work/dir]`), one-shot
  sends via `gray send <platform[:chat[:thread]]> <text>`. Schedule kinds:
  `every 1h` / bare `30m` / `in 10m` / RFC3339 one-shots / 5-field cron
  (all ≥60 s). Agent self-scheduling unlocks in phase 3; until then
  scheduling is human-driven (CLI) only.
- Skills (prompt-time context, not sidecars): `crates/gray/src/skills/`.
- Gateway (chat delivery, shares the agent builder): `crates/gray-gateway/`.
- Pi Gallery (preview): the pi skill source (see `Pi Gallery (preview)`
  above); official plugins above ship from the Gray Index.
