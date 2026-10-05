# systemone — System One decision models for gray

A sidecar plugin that puts fast typed-decision models (TypeSafe Jev, Laya,
Ollaya-served open models, …) behind two tools. Every backend speaks the
same wire — `POST <base>/v1/systemone` — so one client with a configurable
base URL covers all of them.

- `judge` — ask typed questions about a state; get probabilities, not prose.
- `semantic_find` — the decision model scores file chunks against a
  plain-language query when keyword grep is not enough.
- `/s1` — status and a one-shot yes/no check from chat.

## Configuration (env only)

| Variable | Default | Purpose |
|---|---|---|
| `SYSTEMONE_BASE_URL` | `http://localhost:11435` | Backend base URL (`/v1` suffix ok). Falls back to `TYPESAFE_BASE_URL`. |
| `SYSTEMONE_MODEL` | `laya` on localhost, else `jev-latest` | Model id sent in each request. |
| `SYSTEMONE_API_KEY` | unset | Bearer token. Falls back to `TYPESAFE_API_KEY`. |

## Backends

| Backend | Setup |
|---|---|
| Ollaya (local, default) | `curl -fsSL https://ollaya.dev/install.sh \| sh`, then `ollaya pull laya` and `ollaya serve` — daemon listens on `:11435`. |
| TypeSafe | `SYSTEMONE_BASE_URL=https://api.typesafe.ai`, `SYSTEMONE_MODEL=jev-latest`, `SYSTEMONE_API_KEY=…` |
| Any `/v1/systemone`-compatible server | Point `SYSTEMONE_BASE_URL` at it; set `SYSTEMONE_API_KEY` if it requires auth. |

## Build & install

```sh
cargo build --release --manifest-path plugins/systemone/Cargo.toml
GRAY_PLUGIN_PATH="$PWD/plugins/systemone/target/release/gray-systemone" \
  gray plugin install systemone
```

## Tools

### `judge`

`{state: string|object|array, questions: {id → {type, instructions, criteria?}}}`

Types: `choice` (pick from a `criteria` object of options), `score` (rate on
a 2–10 level `criteria` array), `noul` (yes/no probability). Up to 64
questions per call — batch independent checks together. The model cannot
generate text.

### `semantic_find`

`{query, paths, glob?, keywords?, window?, top_k?, threshold?}`

Directories are expanded with `rg --files` (respects `.gitignore`); files
are chunked into `window`-line blocks (default 40) and judged in batches.
Skips oversized/binary/secret-named files. Returns the top chunks with
`p ≥ threshold` (default 0.5).

### `/s1`

- `/s1` or `/s1 status` — base, model, key set/unset, model list.
- `/s1 noul <question> -- <state text>` — prints `p(yes)=0.87`.

## Privacy

With a remote backend, everything sent to `judge` and `semantic_find` —
including file contents — leaves the machine. Secret-named files
(`.env*`, `*.pem`, `*.key`, `id_rsa*`, `auth.json`, `gateway.yaml`, `*.p12`)
are always skipped.

See [ECOSYSTEM.md](./ECOSYSTEM.md) for the surrounding model ecosystem.
