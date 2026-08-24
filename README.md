# gray

A minimal, modular agent harness in Rust.

```
GRAY_API_KEY=sk-... gray --model anthropic/claude-sonnet-4
# → web UI at http://127.0.0.1:7654 — open it, talk, done.

gray --print "explain this repo in one line"   # scripting / no browser
```

## Status: v0.0.1 — working harness

82 tests, clippy-clean, one binary, zero runtime deps beyond the Rust toolchain.

## Usage

```bash
cargo build --release -p gray
export GRAY_API_KEY=...          # or OPENAI_API_KEY
./target/release/gray            # web UI on 127.0.0.1:7654 (default)
./target/release/gray --port 8080
./target/release/gray --base-url http://localhost:11434/v1 --model llama3  # ollama
echo "hi" | ... gray --print "summarize ."    # print mode
```

Any OpenAI-compatible endpoint works (OpenRouter, DeepSeek, Groq, ollama, vLLM).
Sessions persist to `~/.gray/sessions/*.jsonl`.

## Shape (planned)

```
crates/
├── gray-core        event-driven ReAct loop; no I/O of its own
├── gray-provider    OpenAI-compatible + Anthropic wire protocols, SSE streaming
├── gray-tools       bash / read / write / edit / grep behind an async Tool trait
├── gray-session     SessionStore trait + append-only JSONL tree
└── gray             axum server on localhost, embedded chat UI, --print mode
web/                 Vite+React UI lifted from gray-app's chat components
```

Design principles:

- **Minimal core, hard walls.** Core knows nothing about HTTP, terminals, or files.
- **Tiny prompt.** One identity line + Muse Code engineering conventions (~700 tokens total).
- **Errors are data.** A failing tool is a message to the model, never a crash.
