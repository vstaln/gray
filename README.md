# gray

**A minimal coding agent that runs tools, edits code, and works with any model provider.**

`Rust` · `OpenAI-compatible` · `JSONL sessions` · `single static binary`

```
  ⠀⠀⠀⣴⡶⣖⡒⠒⠒⠒⣒⡶⣶⡄⠀⠀⠀⠀⠀⠀⠀⢀⣀⣤⣄⣀
  ⠀⠀⡼⠙⡿⣄⡩⠽⠲⠭⣁⡼⣽⠙⣄⠀⠀⠀⠀⢀⣴⣿⠟⠛⠛⢿⣷⡄⠀⠀⠀⠀⢀⠀⠀⠀⣀⣀
  ⢀⣾⣴⣒⣏⣙⣆⣀⣀⣀⣞⣉⣟⣲⣼⣆⠀⠀⠀⣼⣿⠃⠀⠀⣀⣀⣁⣀⠀⣿⣿⡾⠿⠀⢴⡿⠟⠿⣿⣆⠀⢿⣿⡀⠀⣸⣿⠃
  ⠀⠻⡢⣄⢹⠀⠈⢦⢀⠞⠀⢰⢇⡤⡺⠃⠀⠀⠀⢻⣿⡄⠀⠀⠛⠛⣻⣿⠀⣿⣿⠀⠀⠀⣠⣴⣶⡶⣿⣿⠀⠈⣿⣧⢰⣿⠏
  ⠀⠀⠹⡌⢹⣦⡀⢨⢯⠀⣠⢾⠉⡰⠁⠀⠀⠀⠀⠈⠻⣿⣦⣤⣤⣾⡿⠋⠀⣿⣿⠀⠀⠐⣿⣧⣀⣴⣿⣿⠀⠀⠘⣿⣿⡟
  ⠀⠀⠀⠙⣎⡇⣨⢻⣴⢻⡁⡟⡼⠁⠀⠀⠀⠀⠀⠀⠀⠀⠉⠉⠉⠉⠀⠀⠀⠉⠉⠀⠀⠀⠈⠉⠉⠁⠉⠉⠀⣀⣀⣿⡿⠁
  ⠀⠀⠀⠀⠘⣷⣟⣁⣀⣙⣷⡟⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠛⠟⠛⠁
```

## Install

```bash
curl -fsSL https://gray.alignment.id/install.sh | sh              # stable
curl -fsSL https://gray.alignment.id/install.sh | sh -s -- beta   # bleeding edge, rebuilt on every main push
```

or build from source:

```bash
cargo build --release -p gray
```

## Quick start

```bash
gray                                # interactive REPL — nothing forced at boot
echo "hi" | gray -p "one-line summary of this repo"   # print mode for scripts
gray -c                             # resume your last session
```

First run drops you straight at the prompt. Configure whenever you feel like it:

| | |
|---|---|
| `/provider` | pick a provider — free tier, API key, OAuth (xAI / Codex), or local |
| `/key openrouter` | paste an API key right in the CLI (input hidden), stored per-provider in `~/.gray/auth.json` |
| `/model` | searchable model picker from the bundled models.dev catalog |

Any OpenAI-compatible endpoint works out of the box: **OpenRouter, DeepSeek, Groq, OpenAI, ollama, vLLM, LM Studio** — plus OAuth sign-in flows for xAI/Grok and Codex/ChatGPT accounts.

## Commands

| | |
|---|---|
| `/new` | fresh conversation |
| `/model [id]` | switch or browse models |
| `/provider` | provider menu: free tier · API key · OAuth · local |
| `/key [provider]` | add or rotate a key without leaving the chat |
| `/compact [instructions]` | summarize context (auto-compacts when near limit) |
| `/usage` | session tokens & cost |
| `/context [tokens\|auto]` | inspect or set window — e.g. `128k`, `1m`, `auto` to clear |
| `/agentsmd` | edit the system prompt in `$EDITOR` (`show`, `reset` too) |
| `/help`, `/quit` | you know these |

Slash commands autocomplete: <kbd>Enter</kbd> completes and fires, <kbd>Tab</kbd> inserts for editing. Suffixes too — e.g. `/context r` suggests `reserve`.

## Safety

`gray` executes shell commands from the model. The destructive-command guard
(`crates/gray-tools/src/bash.rs`) blocks obvious foot-guns (`rm -rf /`, `mkfs`,
fork bombs, `git reset --hard`) after an allow-prompt — it is prefix-based and
**not a sandbox**: pipes, `&&` chains, `$(...)`, `eval`, `xargs rm`,
`find -delete`, `python -c 'shutil.rmtree(...)'` and `curl … | sh` pass through.
`GRAY_GUARD_BYPASS=1` disables it entirely. There is no container or VM isolation:
run gray in a container/VM for untrusted work.

## CLI subcommands

| subcommand | what it does |
|---|---|
| `gray resume [--last] [--all] [SESSION_ID]` | resume a previous conversation (picker, most-recent, or by id/prefix) |
| `gray cron list\|create\|add\|remove\|show\|run` | manage cron jobs — e.g. `gray cron create --schedule "every 30m" --prompt "check inbox"`, or shorthand `gray cron add "check inbox every 30m"`; `cron run` starts the scheduler daemon |
| `gray proxy start\|status\|providers` | share Codex/Grok/OpenRouter auth via `http://127.0.0.1:8645/v1` (any bearer forwarded) |
| `gray gateway run\|status\|install\|uninstall\|invite\|pairing` | messaging gateway daemon — `run` (foreground), `status`, `install`/`uninstall` (systemd user service, Linux-only), `invite` (OAuth2 invite URL), `pairing approve\|list\|revoke` (bind the owner without editing `gateway.yaml`) |
| `gray update` | update gray to the latest release |

Global flags: `-p/--print` (one-shot prompt mode), `-c/--continue` (reopen latest session), `--session <ID>` (resume by id), `--context-window <TOKENS>` (e.g. `128000`, `128k`), `--context-reserve`, `--context-keep`, `--dump-manifest` (print merged plugin manifest as JSON and exit).

## Shape

```
crates/
├── gray           REPL · onboarding · config · TUI
├── gray-core      agent loop · events · messages
├── gray-cron      cron scheduling · job store · ticker
├── gray-gateway   Telegram/Discord/Slack gateway daemon
├── gray-markdown  streaming markdown renderer for the TUI
├── gray-plugin    plugin trait · manifest · profile loader
├── gray-provider  OpenAI-compatible SSE streaming (+ retries)
├── gray-session   JSONL session store with parent-id branching
└── gray-tools     read · write · edit · bash · find · grep · ls · cron_tool · plugin loader (gray.yml profiles)
```

- **Streaming first** — text deltas, tool calls, and usage arrive as typed events over SSE.
- **Sessions persist** to `~/.gray/sessions/*.jsonl`; `-c` reopens the latest.
- **Ctrl-C means cancel** mid-turn (first press) and exit at the prompt; interrupted turns still persist what reached memory.
- **Logs** go to `~/.gray/logs/gray.log` — set `GRAY_LOG=debug` for the firehose.

## Context window & auto-compact

Context window resolves as: `--context-window` / `GRAY_CONTEXT_WINDOW` > auto-fetched provider value > LiteLLM model table > hardcoded fallback per model. Inspect with `/context`, set with `/context 128k` (or `1m`, `auto` to clear).

When usage nears the limit (`tokens > window − 16k` reserve, pi parity), gray auto-compacts before the next turn by summarizing history into a 2-message summary (same flow as manual `/compact`). On `context_length` / `max_tokens` overflow errors it compacts and retries once. No flag needed — auto is the default; use `/compact` to force a manual summarization.

## Environment

| var | meaning |
|---|---|
| `GRAY_HOME` | config root (default `~/.gray`) |
| `GRAY_API_KEY` / `OPENAI_API_KEY` | API key (env beats stored keys) |
| `GRAY_MODEL`, `GRAY_BASE_URL` | defaults before `~/.gray/config.json` is consulted |
| `GRAY_CONTEXT_WINDOW` / `--context-window` | override window in tokens — `128000`, `128k`, `1m`, or `auto` to clear |
| `GRAY_LOG` | log level: `error`…`trace` (default `info`) |
| `GRAY_NO_UPDATE_CHECK=1` | disable the startup update check |
| `GRAY_AUTO_UPDATE=1` | background self-update, no prompt |
| `GRAY_INSTALL_DIR` | installer destination dir (overrides default `~/.local/bin`; `--system` installs system-wide) |
| `GRAY_GUARD_BYPASS=1` | disable the destructive-command guard entirely (CI/piped mode) |
| `GRAY_PERMISSION` | tool permission: `ask` (prompt before risky commands, default) or `auto` (no prompts; default in `-p` print mode) |

## Platform support

| OS/arch | binary | notes |
|---|---|---|
| Linux x86_64 / aarch64 | musl-static | fully supported |
| macOS arm64 / x86_64 | Rust-static, **not notarized** | curl-installed binaries run fine; browser downloads may hit Gatekeeper quarantine |
| Windows | via WSL only | native Windows unsupported |

`gray gateway install` (systemd user service) is Linux-only. Single static binary —
"zero runtime deps" means no sidecar services; you still need `sh`, `curl`/`wget`,
`tar`, and `sha256sum`/`shasum` for the installer.

## Stability

Stable in 1.x: CLI flags, session JSONL schema, plugin wire v1, ~/.gray layout. Not stable: TUI, internal crate APIs, gray-markdown.

## Gateway

The gateway (`gray gateway`) exposes gray over Telegram, Discord, and Slack —
meant to run as a daemon on a VPS. Foreground: `gray gateway run`;
persistent service (Linux-only): `gray gateway install` (systemd user service).

Config lives in `~/.gray/gateway.yaml`, written `0600` (owner-only). Security
model is deny-by-default: nobody talks to the agent unless allowlisted.

Key config keys (under `platforms.<telegram|discord|slack>` plus top level):

| key | meaning |
|---|---|
| `platforms.<p>.enabled` / `.token` | turn the platform on; bot token |
| `platforms.<p>.allowed_users` | who may talk to the agent (`"*"` = everyone, only meaningful with `dm_policy: open`) |
| `platforms.<p>.dm_policy` | DM admission: pairing (default) or open |
| `group_per_user` | per-user threads in groups (default on) |
| `autostart` | auto-start the in-process gateway when gray launches — default **off** |
| `denied_tools` | extra tools blocked in gateway sessions (merged with the built-in deny set) |
| `streaming` | stream replies (default on) |
| `cron_delivery` | deliver due cron results to chat (default on) |

Pairing flow (no `gateway.yaml` edit needed): the user DMs the bot, gray prints
a code, the operator runs `gray gateway pairing approve <platform> <CODE>`;
`pairing list` shows pending + approved users, `pairing revoke` drops one.

## Acknowledgements

Ideas and designs informed by [pi](https://github.com/badlogic/pi-mono), Codex,
OpenClaw, hermes, and dcg — thanks to those projects and their authors.

A naming note: `cargo install gray` belongs to another crate, so the install
path is the installer script above (or a source build); the binary stays `gray`.

---

<div align="center">

<sub>MIT © 2026 vstaln</sub>

</div>
