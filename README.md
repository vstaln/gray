<!-- ────────────────────────────────────────────────────────────────────────
     LOGO PLACEHOLDER
     The <img> below points at assets/logo-dark.svg — right now that's a
     stand-in pulled from gray.alignment.id. Drop the real mark in at that
     path (SVG, white on transparent, ~360px wide) and delete this comment.
     ───────────────────────────────────────────────────────────────────────── -->
<div align="center">
  <img alt="Gray" src="assets/logo-dark.svg" width="108" />
  <h1>gray</h1>
  <p><strong>A minimal, modular AI agent harness.</strong><br/>Start small. Extend anything.</p>
  <p>
    <a href="https://gray.alignment.id">Website</a> ·
    <a href="docs/customize.md">Docs</a> ·
    <a href="CHANGELOG.md">Changelog</a> ·
    <a href="https://github.com/vstaln/gray/releases">Releases</a>
  </p>
  <p>
    <a href="LICENSE"><img alt="License: MIT" src="https://img.shields.io/badge/license-MIT-1c1c20?style=flat-square&labelColor=0a0a0b" /></a>
    <a href="https://www.rust-lang.org"><img alt="Built with Rust" src="https://img.shields.io/badge/built%20with-rust-1c1c20?style=flat-square&labelColor=0a0a0b&logo=rust&logoColor=d4a373" /></a>
    <a href="#platform-support"><img alt="Platform: linux and macOS" src="https://img.shields.io/badge/platform-linux%20%C2%B7%20macos-1c1c20?style=flat-square&labelColor=0a0a0b" /></a>
    <a href="https://github.com/vstaln/gray/releases"><img alt="Latest release" src="https://img.shields.io/github/v/release/vstaln/gray?style=flat-square&labelColor=0a0a0b&color=131316" /></a>
  </p>
</div>

<br/>

<div align="center">
  <img alt="Dithered Carina Nebula — cosmic cliffs" src="assets/space/carina-dither.png" width="100%" />
</div>

Gray is a tiny agent core — streaming tool calls over SSE, JSONL sessions, self-managing context — that you extend only when you need to: skills, stdio plugins, cron, a messaging gateway. Any OpenAI-compatible provider works out of the box. No plugin marketplace, no roadmap promises — the release binary carries everything below; from-source builds are feature-gated (see [Install](#install)).

| | |
|---|---|
| **One binary, no runtime** | musl-static on Linux, Rust-static on macOS. `curl \| sh` lands you in a REPL; `gray update` self-updates. |
| **Any provider, your keys** | OpenRouter, DeepSeek, Groq, OpenAI, ollama, vLLM, LM Studio — anything OpenAI-compatible — plus OAuth sign-in for xAI/Grok and Codex/ChatGPT. Searchable model picker over the bundled models.dev catalog. |
| **Sessions that survive** | JSONL transcripts in `~/.gray/sessions` with parent-id branching. `-c` reopens the latest, `/resume` picks any of them. Interrupted turns keep what reached memory. |
| **Context that manages itself** | The window auto-resolves from your provider, gray auto-compacts before the limit and retries once on overflow. `/compact` forces it by hand. |
| **Batteries in, guard on** | read · write · edit · bash · find · grep · ls · glob · cron. A destructive-command guard asks before foot-guns; Ctrl-C cancels a runaway turn. |
| **Lives where you do** | Telegram / Discord / Slack gateway daemon — deny-by-default, pairing flow, heartbeats — plus cron jobs the agent can self-schedule. Release binary; from source add `--features all-platforms`. |
| **Extend the harness** | Skills from `SKILL.md`, sidecar plugins over stdio (frozen wire v1), or `/acp` to *become* claude, codex, cursor, opencode… |

## Install

```bash
curl -fsSL https://gray.alignment.id/install.sh | sh              # stable
curl -fsSL https://gray.alignment.id/install.sh | sh -s -- beta   # bleeding edge, rebuilt on every main push
```

or from source:

```bash
cargo build --release -p gray                             # harness core
cargo build --release -p gray --features all-platforms    # + Telegram/Discord/Slack adapters
cargo build --release -p gray --features clipboard        # + image paste in the TUI
```

| build | adds |
|---|---|
| default | harness core: CLI, TUI, provider, sessions, tools, cron |
| `--features all-platforms` | Telegram + Discord + Slack gateway adapters (what the release binary ships) |
| `--features clipboard` | image/paste attachments (arboard + image) |

Windows runs via WSL; macOS binaries are Rust-static but **not notarized** — curl-installed binaries run fine, browser downloads may hit Gatekeeper quarantine.

## Quick start

```bash
gray
```

First run drops you straight at the prompt. Configure whenever you feel like it:

| command | what it does |
|---|---|
| `/provider` | pick a provider — free tier, API key, OAuth (xAI / Codex), or local |
| `/key openrouter` | paste an API key right in the CLI (input hidden), stored per-provider in `~/.gray/auth.json` |
| `/model` | searchable picker over the bundled models.dev catalog |

## Watch it go

<div align="center">
  <img alt="gray building a complete single-file app in the terminal" src="assets/gray-demo.gif" width="100%" />
</div>

## Commands

Slash commands autocomplete: Enter completes and fires, Tab inserts for editing — suffixes too, so `/context r` suggests `reserve`.

| | |
|---|---|
| `/new` · `/resume [id\|--last\|--all]` | fresh conversation, or reopen a previous one |
| `/model [id]` · `/provider` · `/key [provider]` | models, providers, keys — without leaving the chat |
| `/compact [instructions]` | summarize context (auto-compacts near the limit) |
| `/context [tokens\|auto]` | inspect or set the window — `128k`, `1m`, `auto` to clear |
| `/permissions [mode]` | read-only · auto · full — Shift+Tab cycles |
| `/thinking` · `/effort [level]` | toggle reasoning, pick the effort |
| `/usage` | session tokens & cost |
| `/skills` · `/skills:<name> [args]` | list skills, run one |
| `/plugin <subcommand>` | list · search · install · remove · update · enable · disable · check |
| `/agentsmd` | edit the system prompt in `$EDITOR` (`show`, `reset` too) |
| `/acp [agent] [prompt]` | run as an external ACP agent (claude, codex, cursor, opencode…) |
| `/feedback <text>` | save feedback locally + open a prefilled GitHub issue |
| `/help` · `/quit` | you know these |

### CLI surface

`gray` itself plus four subcommands — everything else is a slash command away:

| subcommand | what it does |
|---|---|
| `gray resume [--last\|--all] [SESSION_ID]` | resume a conversation — picker, most-recent, or by id/prefix |
| `gray gateway run\|status\|install\|uninstall\|invite\|pairing` | messaging gateway daemon (systemd user service, Linux-only) |
| `gray plugin <list\|search\|install\|remove\|update\|enable\|disable\|check>` | manage plugins |
| `gray update` | update gray to the latest release |

Global flags: `-p/--print` (one-shot), `-c/--continue` (reopen latest), `--session <ID>`, `--acp <AGENT>`, `--context-window <TOKENS>`, `--context-reserve`, `--context-keep`, `--dump-manifest`.

## Extend

Make gray yours: [docs/customize.md](docs/customize.md) (skills, plugins, providers, config) · [docs/plugins.md](docs/plugins.md) (plugin authoring) · [docs/protocol-v1.md](docs/protocol-v1.md) (frozen wire spec).

**Skills** — `SKILL.md` bodies discovered across opencode / claude / agent directories. `/skills` lists them, `/skills:<name> [args]` runs one.

**Plugins** — sidecar child processes speaking newline-delimited JSON over stdio, with timeout and crash degradation. `gray.yml` profiles order built-ins and sidecars; [`plugins/echo/`](plugins/echo) is a copy-paste reference implementation.

**ACP agents** — `/acp` turns gray into any external coding agent over the [Agent Client Protocol](https://agentclientprotocol.com): bare `/acp` opens a picker, `/acp <agent> <prompt>` delegates one-shot, `/acp off` returns to native — and `gray -p '…' --acp opencode` works in print mode. Probed via `which`: `codex`, `claude`, `opencode`, `cursor`, `gemini`, `copilot`, `grok`, `goose` / `kimi` / `kiro`; customs go in `~/.gray/acp.json`. Permission requests are **denied by default** — `--yolo` (or `GRAY_ACP_AUTO_APPROVE=1`) auto-approves, and the external agent's own permission model applies: gray's bash guard does not run in ACP mode. Design doc: [docs/ACP_PLAN.md](docs/ACP_PLAN.md).

## Gateway

`gray gateway` exposes gray over Telegram, Discord, and Slack — meant to run as a daemon on a VPS. Config lives in `~/.gray/gateway.yaml`, written `0600` (owner-only). The security model is deny-by-default: nobody talks to the agent unless allowlisted — or paired: the user DMs the bot, gray prints a code, you run `gray gateway pairing approve <platform> <CODE>` (`pairing list` / `revoke` manage the rest).

Always-on: `gray gateway install` (systemd user service, `Restart=always`, survives reboot with linger) or `gray gateway run` under your own supervisor. `gray gateway status --probe` reports heartbeat health; heartbeats live in `~/.gray/state/gateway.heartbeat`, lifecycle in `state/gateway.lifecycle.json`, logs rotate at 10 MB × 3.

<div align="center">
  <img alt="Dithered Blue Marble" src="assets/space/bluemarble-dither.png" width="31%" />
  <img alt="Dithered Jupiter storm" src="assets/space/jupiter-dither.png" width="31%" />
  <img alt="Dithered Saturn" src="assets/space/saturn-dither.png" width="31%" />
</div>

## Safety

`gray` executes shell commands from the model. The destructive-command guard (`crates/gray-tools/src/shell/guard.rs`) blocks obvious foot-guns (`rm -rf /`, `mkfs`, fork bombs, `git reset --hard`) after an allow-prompt — it is prefix-based and **not a sandbox**: pipes, `&&` chains, `$(...)`, `eval`, `xargs rm`, `find -delete`, `python -c 'shutil.rmtree(...)'` and `curl … | sh` all pass through. `GRAY_GUARD_BYPASS=1` disables it entirely. There is no container or VM isolation: run gray in a container/VM for untrusted work. Security reports: [SECURITY.md](SECURITY.md).

Persistence note: gateway and REPL sessions keep raw transcripts at `0600` under `~/.gray/sessions` for exact resume — including any secret that crossed a tool call. `gray -p` print mode scrubs secrets before persisting; set `persist_redacted: true` in `gateway.yaml` to scrub gateway transcripts too. Plan backups, snapshots, and disk access accordingly.

## Context window & auto-compact

The window resolves as: `--context-window` / `GRAY_CONTEXT_WINDOW` → auto-fetched provider value → LiteLLM model table → hardcoded fallback. Inspect with `/context`, set with `/context 128k` (or `1m`; `auto` clears).

When usage nears the limit (`tokens > window − 16k` reserve), gray summarizes history into a 2-message summary before the next turn — the same flow as manual `/compact` — and on `context_length` / `max_tokens` overflow errors it compacts and retries once. Auto is the default; no flag needed.

## Layout

| crate | role |
|---|---|
| `gray` | REPL · onboarding · config · TUI |
| `gray-core` | agent loop · events · messages |
| `gray-provider` | OpenAI-compatible SSE streaming, retries, prompt caching |
| `gray-session` | JSONL session store with parent-id branching |
| `gray-tools` | read · write · edit · bash · find · grep · ls · glob · cron_tool · plugin loader |
| `gray-plugin` | plugin trait · manifest · `gray.yml` profile loader |
| `gray-pkg` | plugin package management |
| `gray-acp` | Agent Client Protocol client (external agents) |
| `gray-cron` | cron scheduling · job store · ticker |
| `gray-gateway` | Telegram / Discord / Slack gateway daemon |
| `gray-supervise` | supervision core — restart contract, heartbeat, lifecycle, probe, rotation |
| `gray-extras` | outside the default build: proxy, OAuth sign-in, cron CLI |
| `gray-markdown` | streaming markdown renderer for the TUI |

Design notes: streaming first — text deltas, tool calls, and usage arrive as typed events over SSE. Logs go to `~/.gray/logs/gray.log` (`GRAY_LOG=debug` for the firehose). Benchmarks: [docs/read-tool-bench.md](docs/read-tool-bench.md). Shell-tool notes: [docs/harness/shell-tool.md](docs/harness/shell-tool.md).

## Environment

The essentials — everything else is one `--help` or doc page away.

| var | meaning |
|---|---|
| `GRAY_HOME` | config root (default `~/.gray`) |
| `GRAY_API_KEY` / `OPENAI_API_KEY` | API key — env beats stored keys |
| `GRAY_MODEL` · `GRAY_BASE_URL` | defaults before `~/.gray/config.json` is consulted |
| `GRAY_CONTEXT_WINDOW` | override the window in tokens — `128000`, `128k`, `1m`, or `auto` |
| `GRAY_PERMISSION` | `read-only` · `auto` (default — commands and outside-workspace edits ask) · `full` (no prompts) |
| `GRAY_GUARD_BYPASS=1` | disable the destructive-command guard entirely (CI / piped mode) |
| `GRAY_NO_UPDATE_CHECK=1` · `GRAY_AUTO_UPDATE=1` | silence the startup update check, or background self-update |
| `GRAY_LOG` | `error`…`trace` (default `info`) |
| `GRAY_ACP_AUTO_APPROVE=1` | auto-approve ACP permission requests (same as `--yolo`) |
| `GRAY_PARALLEL_READS` | `0` runs every tool sequentially (default: read-only tools concurrent, input order preserved) |

## Platform support

| OS / arch | binary | notes |
|---|---|---|
| Linux x86_64 / aarch64 | musl-static | fully supported — `gray gateway install` (systemd user service) is Linux-only |
| macOS arm64 / x86_64 | Rust-static, **not notarized** | curl-installed binaries run fine; browser downloads may hit Gatekeeper quarantine |
| Windows | via WSL only | native Windows unsupported |

"Zero runtime deps" means no sidecar services — you still need `sh`, `curl` / `wget`, `tar`, and `sha256sum` / `shasum` for the installer.

## Stability

The 1.x stability contract (CLI flags, session JSONL schema, plugin wire v1, `~/.gray` layout) takes effect at 1.0 — on 0.x these are best-effort. Not stable: the TUI, internal crate APIs, `gray-markdown`. Per-release changes: [CHANGELOG.md](CHANGELOG.md). Rollback is publisher-side today (manifest re-point); user-side `gray update --to <version>` is planned.

---

Ideas and designs informed by the projects listed in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) — thanks to those projects and their authors.

A naming note: `cargo install gray` belongs to another crate, so the install path is the installer script above (or a source build). The binary stays `gray`.

MIT © 2026 vstaln
