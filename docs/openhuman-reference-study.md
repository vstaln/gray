# OpenHuman — reference study

Study only. No gray file was written from this.

- Upstream: <https://github.com/tinyhumansai/openhuman> @ `43b3b71` (2026-10-01),
  shallow clone at `reference/tinyhumansai/openhuman` (114 MB).
- Read under [`docs/reference-study-protocol.md`](./reference-study-protocol.md):
  design parity only, own vocabulary, no verbatim text, no translation.
- Upstream proper nouns are kept to a minimum below. Where a number is theirs,
  it is stated as a number, not as their code.

## License verdict: no, it does not permit porting

| Repo | License |
|---|---|
| `tinyhumansai/openhuman` | GPL-3.0 |
| all 18 `tinyhumansai/*` engine crates it vendors (`tinyagents`, `tinyjuice`, `tinymemory`, `tinybox`, `tinymcp`, …) | GPL-3.0 |
| gray | MIT (`LICENSE`) |

gray is MIT and ships as one binary with its dependencies. Copying or translating
any substantial part of a GPL-3.0 tree into it makes gray a derivative work, and
the whole binary has to go out under GPL-3.0 — there is no smaller carve-out for
"just the compression router" or "just the command classifier", and no exception
file, dual-license offer, or trademark carve-out exists upstream to lean on.

So the license does not permit the port, which means the port question never
arises: this is a design-parity study, same as every other clone under
`reference/`. Read it for the ideas, write the code from this note.

## What it is, in my own words

A personal-assistant product: one Rust core that runs agent turns, keeps a
persistent local memory, and talks to chat platforms, wrapped in a Tauri desktop
app, a React browser UI, a terminal client, and an embeddable library facade. The
interesting part for gray is not the product — it is the discipline underneath:

1. every capability is a Cargo feature gate, and CI refuses to let the
   dependency floor grow;
2. every tool result is squeezed before it enters context, and nothing is thrown
   away — the full original stays retrievable behind a handle;
3. one security domain sits between the agent and the machine, with policy,
   approval, redaction, secret storage, egress control and an audit trail, none
   of it feature-gated;
4. everything is measured on a mock provider and written down with the
   reproduction script next to the number.

## Component map and data flow

- **Core library** (one crate, many flat domain modules): agent, tools, memory,
  channels, scheduling, hooks, skills, MCP, search, media, config, plus a
  subsystem/event-bus layer. Hosts (desktop shell, TUI, CLI) embed it in-process;
  there is no sidecar daemon and no socket between the core and its UI.
- **Engine crates** are pulled in as vendored submodules, each behind a small
  `*-bus` contract crate that owns the interface and its wire types. Capability
  comes from loading one of these engines, not from the core growing.
- **Frontend** talks to the core over loopback JSON-RPC (`fetch` straight from
  the webview), with a Tauri-IPC fallback only for runtimes the webview would
  block as mixed content, plus one event socket for streaming.
- **Skills** are `SKILL.md` packages: metadata and tool descriptors only. The
  old design — one sandboxed JS VM per skill with per-skill bridges and a
  per-skill 5-second cron tick — was deleted. Scheduling moved to one scheduler
  domain, execution to native handlers and managed language runtimes.
- **Memory** is a separate engine (chunking, scoring, retrieval, embeddings);
  the host owns RPC, tools, credentials and the namespace document store.
  Transcripts are per-thread JSONL, and compaction **seals a generation instead
  of deleting one**, so a resumed session reads the latest generation while the
  full history stays recoverable.

## Numbers worth borrowing

| Area | Number |
|---|---|
| Tool-output size gate | 2 KB below which nothing is squeezed |
| Offload-to-store threshold | ~500 tokens of estimated input |
| Preview head | first 500 chars + an extractive outline + a shape line |
| Store bounds | 256 entries / 64 MiB, FIFO eviction, optional TTL and disk tier |
| Extra schema cost of the query tools | ~1.5 KB of tool schema |
| Optional ML squeeze | target ratio 0.5, 200 000 char cap, degrades silently when absent |
| Cold single turn | 102 ms, 47.6 MiB settled |
| Full bootstrap (9 phases) | 476 ms, 51.2 MiB |
| Warmed turn in the same process | 0.5–1.9 MiB |
| Marginal cost per live agent in-process | 1 985 / 1 866 / 1 770 KiB at N = 50 / 100 / 500 |
| One process per agent instead | ~48 MiB per instance, flat → ~25× denser in-process |
| Threads | +0.35 per agent; idle CPU flat (3 ms per 10 s) at every N |
| Binary | 115.9 MiB unstripped with all gates; ~60 MiB stripped for the embed recipe; 51 MiB stripped pure-slim |
| Live-connection liveness | dead after ping interval + ping timeout + 5 s (≈50 s defaults); reconnect backs off 1 s → 30 s |
| Secrets in transit | single-use login tokens, 5-minute TTL |
| Human approval of a parked call | 10-minute TTL |
| File hooks | default 30 s timeout, 5 follow-up injections per session, fail-open unless flagged closed |
| Rule overlays for command output | ~96 builtin rules, three layers (binary / user / repo), later layers win |

## Candidate adoptions, ranked

### 1. Dependency-floor ratchet (do this one first)

They measure three different things and know which is which: total `name+version`
lines = compile units you pay for; unique crate names = how many distinct
projects you trust; crates with a C/C++ build script = whether a build needs
`cc`/`cmake`/`pkg-config`. CI allows the number to go down and never up. The
subtle part worth copying: `cargo tree` marks an already-expanded subtree with a
`(*)` back-reference, and counting those inflates the total — strip and
deduplicate first or the baseline is wrong from day one.

For gray: `scripts/` has no CI checks at all today, so this is one bash script,
one baseline file, one job in `.github/workflows/ci.yml`. Highest value per line
of anything in this note.

### 2. Truncate by handle instead of by amputation

gray's `crates/gray-core/src/tool_out.rs` caps every tool result at 2000 lines /
50 KiB and keeps head + tail. Past that line the agent is reading a **lie**: the
middle is gone and the annotation only says how much. Every long `rg`, test run
or `git log` becomes a wrong answer.

Their fix: keep the cheap preview in context, put the full original somewhere the
model cannot reach directly, hand back an opaque handle, and give the model
read-only tools that query the stored original (search by line / pattern / rank,
head+tail or relevance slice, structural summary). None of those tools calls a
model, so a slow summariser cannot stall a turn; because the preview is built
without a model call it also replaces the "summarise this" step entirely.

For gray this is ~100 lines: one process-global store keyed by digest with the
256-entry / 64 MiB bound, a footer carrying the handle, and one read-only tool
that pulls it back. The shape already exists here for binaries — `gray view`
renders a blob the model cannot see as text — so this is the text twin of a tool
gray ships. The compression *per content kind* (JSON tables, diff squeezing, code
folding via tree-sitter, HTML stripping, an optional local salience model) is a
research project — do not adopt it.

Also worth stealing from the same doc: **a compressor may decline, and its output
must never be bigger than its input.** Fall back, then pass through.

### 3. Classify a command once, take the max class across the pipeline

Every side-effecting tool call is sorted into a small fixed set of risk classes
(read / write / network / install / destructive), and the answer for a compound
shell line is the **highest** class in it — `ls | curl …` is network, not read.
Classification is fail-closed: anything not provably read-only is at least write.
Across a piped command it has to be quote-aware and heredoc-aware, or `echo "rm -rf /"` becomes a false positive.

For gray: gray has no approval gate, and that is a legitimate product call for a
terminal agent whose user is the operator. But the classifier is worth having as
a reusable helper anyway — plugin `tool/before` hooks (`crates/gray-plugin/src/lib.rs`)
would otherwise re-implement it per plugin, badly.

### 4. One egress chokepoint

All text that leaves the machine — provider wire, gateway post, feedback upload,
telemetry — passes a single function that classifies the payload (what kind of
data, how identifying) and enforces local-only rules at one place.

gray already scrubs absolute and repo-relative paths before anything is logged or
sent (`crates/gray-core/src/redaction.rs`). What is missing is the *chokepoint*
and the data-kind vocabulary: "what can leave this box" should be answerable by
reading one file, not by auditing every call site.

### 5. Injection scan at ingest of untrusted text

They scan every remote tool definition and every inbound channel message before
it reaches a model, with local heuristics.

gray ingests the same class of untrusted text — issue bodies, web pages, gateway
messages. What gray has today is the *model* half of the defence: tool output is
tagged at the trust boundary (`crates/gray/src/tool_fmt/mod.rs:679`) and the
system prompt tells the model to treat it as untrusted. There is no local scan,
so the check rides entirely on model compliance. Worth adding at ingest, not only
at tool-registration time. Keep it a cheap lexical pass with a hard "flag, never
rewrite" stance; auto-rewriting untrusted text is how you corrupt a diff.

### 6. Density benchmark (deferred)

Marginal cost per live agent, and in-process vs one-process-per-agent, measured
against a mock provider with a fixed latency so idle time behaves like a network
wait. Only worth building if gray actually goes multi-session-per-process; today
it is one session per process and the number is academic.

### Considered and not adopting

- **File-based hooks in another host's format.** gray's sidecar plugins already
  cover the three moments that matter, and adopting a third party's wire format
  for parity buys compatibility we do not need.
- **Per-skill sandboxes, per-skill schedulers, in-process JS VMs.** Upstream deleted
  all three for having no callers. That is a useful negative result: it is the same
  call gray made with sidecars instead of embedded VMs.
- **Hand-rolled socket framing, Tauri desktop, wallet, web3, cloud deploy.** Out
  of gray's lane.

## Failure modes they documented (worth reading for the reasoning alone)

- A hook that denies only when it manages to run is not a security control —
  hence one "crashed, missing, timed-out, unparseable output counts as denial"
  switch covering every way a script can fail to answer.
- Adding a hook must never loosen a policy another hook set: layers concatenate
  and the strictest verdict wins.
- An escalation to "ask" with no approval channel available denies rather than
  quietly allowing.
- A non-loopback bind is refused without a core token.
- When the optional ML compressor or its runtime is missing, the pipeline degrades
  to the deterministic path and never fails the turn.
- If a stored original is evicted or unknown, retrieval is a clear error, never a
  silent empty result.

## Open questions for gray

1. Do we want an approval gate at all, or is "the operator is the user, hooks are
   the escape hatch" the deliberate line? If yes, where does the tier live —
   config, or a session toggle?
2. Should the handle store be per-process (dies with the session, nothing on
   disk) or survive in the session store? Per-process is the smaller diff and the
   smaller blast radius; the cost is that a resumed session cannot zoom back.
3. Is the egress chokepoint a function, a trait, or a `gray-core` type the gateway
   and providers both call? A function is the smallest thing that works.
4. Do we keep the existing head+tail caps as the floor under the handle, or does
   the handle replace them entirely?
5. Which CI baseline number do we pin first — package count or unique crate names?
