<div align="center">

<img src="docs/img/grayai.png" width="140" alt="gray logo"/>

# gray

**A minimal coding agent that runs tools, edits code, and works with any model provider.**

`Rust` · `OpenAI-compatible` · `JSONL sessions` · `zero runtime deps beyond the toolchain`

</div>

<img src="docs/img/inspo-ui.jpg" width="100%" alt=""/>

---

```
  ⠀⠀⠀⣴⡶⣖⡒⠒⠒⠒⣒⡶⣶⡄⠀⠀⠀⠀⠀⠀⠀⢀⣀⣤⣄⣀
  ⠀⠀⡼⠙⡿⣄⡩⠽⠲⠭⣁⡼⣽⠙⣄⠀⠀⠀⠀⢀⣴⣿⠟⠛⠛⢿⣷⡄⠀⠀⠀⠀⢀⠀⠀⠀⣀⣀
  ⢀⣾⣴⣒⣏⣙⣆⣀⣀⣀⣞⣉⣟⣲⣼⣆⠀⠀⠀⣼⣿⠃⠀⠀⣀⣀⣁⣀⠀⣿⣿⡾⠿⠀⢴⡿⠟⠿⣿⣆⠀⢿⣿⡀⠀⣸⣿⠃
  ⠀⠻⡢⣄⢹⠀⠈⢦⢀⠞⠀⢰⢇⡤⡺⠃⠀⠀⠀⢻⣿⡄⠀⠀⠛⠛⣻⣿⠀⣿⣿⠀⠀⠀⣠⣴⣶⡶⣿⣿⠀⠈⣿⣧⢰⣿⠏
  ⠀⠀⠹⡌⢹⣦⡀⢨⢯⠀⣠⢾⠉⡰⠁⠀⠀⠀⠀⠈⠻⣿⣦⣤⣤⣾⡿⠋⠀⣿⣿⠀⠀⠐⣿⣧⣀⣴⣿⣿⠀⠀⠘⣿⣿⡟
  ⠀⠀⠀⠙⣎⡇⣨⢻⣴⢻⡁⡟⡼⠁⠀⠀⠀⠀⠀⠀⠀⠀⠉⠉⠉⠉⠀⠀⠀⠉⠉⠀⠀⠀⠈⠉⠉⠁⠉⠉⠀⣀⣀⣿⡿⠁
  ⠀⠀⠀⠀⠘⣷⣟⣁⣀⣙⣷⡟⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠛⠟⠛⠁
```

## Quick start

```bash
cargo build --release -p gray

# interactive REPL: nothing forced at boot
./target/release/gray

# one-shot print mode for scripts and pipes
echo "hi" | ./target/release/gray -p "summarize this repo in one line"

# resume your last session
./target/release/gray -c
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
| `/sys` | edit the system prompt in `$EDITOR` (`show`, `reset` too) |
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

## Environment

| var | meaning |
|---|---|
| `GRAY_HOME` | config root (default `~/.gray`) |
| `GRAY_API_KEY` / `OPENAI_API_KEY` | API key (env beats stored keys) |
| `GRAY_MODEL`, `GRAY_BASE_URL` | defaults before `~/.gray/config.json` is consulted |
| `GRAY_LOG` | log level: `error`…`trace` (default `info`) |

<img src="docs/img/inspo-terminal.jpg" width="480" alt=""/>

---

<div align="center">

<sub>MIT © 2026 vstaln</sub>

</div>
