# gray

**A minimal coding agent that runs tools, edits code, and works with any model provider.**

`Rust` · `OpenAI-compatible` · `JSONL sessions` · `zero runtime deps beyond the toolchain`

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
| `/context-window [tokens\|auto]` | inspect or set window — e.g. `128k`, `1m`, `auto` to clear |
| `/agentsmd` | edit the system prompt in `$EDITOR` (`show`, `reset` too) |
| `/help`, `/quit` | you know these |

Slash commands autocomplete: <kbd>Enter</kbd> completes and fires, <kbd>Tab</kbd> inserts for editing.

## Shape

```
crates/
├── gray           REPL · onboarding · config · TUI
├── gray-core      agent loop · events · messages
├── gray-provider  OpenAI-compatible SSE streaming (+ retries)
├── gray-session   JSONL session store with parent-id branching
└── gray-tools     read · write · edit · bash tool registry
```

- **Streaming first** — text deltas, tool calls, and usage arrive as typed events over SSE.
- **Sessions persist** to `~/.gray/sessions/*.jsonl`; `-c` reopens the latest.
- **Ctrl-C means cancel** mid-turn (first press) and exit at the prompt; interrupted turns still persist what reached memory.
- **Logs** go to `~/.gray/logs/gray.log` — set `GRAY_LOG=debug` for the firehose.

## Context window & auto-compact

Context window resolves as: `--context-window` / `GRAY_CONTEXT_WINDOW` > auto-fetched provider value > hardcoded fallback per model. Inspect with `/context-window`, set with `/context-window 128k` (or `1m`, `auto` to clear).

When usage nears the limit (`tokens > window − 16k` reserve, pi parity), gray auto-compacts before the next turn by summarizing history into a 2-message summary (same flow as manual `/compact`). On `context_length` / `max_tokens` overflow errors it compacts and retries once. No flag needed — auto is the default; use `/compact` to force a manual summarization.

## Environment

| var | meaning |
|---|---|
| `GRAY_HOME` | config root (default `~/.gray`) |
| `GRAY_API_KEY` / `OPENAI_API_KEY` | API key (env beats stored keys) |
| `GRAY_MODEL`, `GRAY_BASE_URL` | defaults before `~/.gray/config.json` is consulted |
| `GRAY_CONTEXT_WINDOW` / `--context-window` | override window in tokens — `128000`, `128k`, `1m`, or `auto` to clear |
| `GRAY_LOG` | log level: `error`…`trace` (default `info`) |

<img src="docs/img/hokusai-kajikazawa.jpg" width="520" alt=""/>

---

<div align="center">

<sub>MIT © 2026 vstaln</sub>

</div>
