# `gray find` / `gray grep` — the resident search index

Gray keeps a watcher-backed `fff-search` index per search root and answers
`gray find` / `gray grep` from it. The `find` and `grep` **tools** are
untouched: they are `fd`/`rg` and stay that way, on every tree, forever.

## Why a command and not a tool

The index used to sit *behind* the `find` and `grep` tools, which meant two
tools silently changed what they returned depending on the directory, the
pattern, and a decade of glob semantics. Every such case needs a decline rule,
and a missed rule is a wrong answer rather than a slow one — the first version
shipped `find "crates/gray-tools/src/*.rs"` returning 69 files where `fd`
returns 23, because fff compiles globs with globset's default
`literal_separator = false` and paginates before a caller could filter.

Two things settled the shape:

- Gray's default tool surface is bash-only, so the model does not have `find`
  or `grep` tools anyway — it has `fd`, `rg` and `grep`. A command sits beside
  the tools it accelerates instead of behind them.
- The index is a *faster way to run the same question*, so it has to be
  reachable on purpose. If it is invisible, the model cannot choose between
  "cheap and always right" and "fast when it pays".

## The contract

**Whatever `fd`/`rg` would have answered, `gray find`/`gray grep` answers.**
The index is a fast path in front of the existing lanes, never a different
answer. When it is not usable, the call runs the very same `FindTool` /
`GrepTool` the model could have run itself, so the bytes are identical either
way and the only observable difference is time.

"Not usable" is about **cost** before it is about capability. The index lives
in process memory, so a process holding none would pay a full scan to answer a
question `fd` answers in ~20ms — measured at **1.4s against 20ms** on a
20k-file repo. So:

```
first search in a process  ->  the tool lane (fd/rg), and the index starts
                               building in the background
every search after that   ->  the index, when it can be exact
```

The index therefore has to earn each call, and the cases where it cannot are
declined rather than approximated:

- **No index in this process yet.** Cost, not capability — see above.
- **Non-git directories.** fff's walker skips hidden files outside git
  worktrees and only honors `.gitignore` inside them; `fd --hidden` does
  neither.
- **Depth-anchored globs.** `a/*.rs` would also match every file below `a/`.
  fff paginates *before* a caller can filter, so `find` serves only
  slash-free patterns (a slash-free glob is a basename glob under
  `literal_separator(true)`, which is exactly what `fd` compares) and `grep`
  declines a `glob` containing a `/`.
- **`ignoreCase`.** fff 0.10.x only offers `smart_case`, which cannot express
  "always insensitive"; `(?i)` would push literals off its fast path.
- **Negated globs** (`glob="!*.rs"`). fff needs `Constraint::Not(Glob(..))`; a
  bare `!` glob compiles as a literal filename char.
- **Invalid patterns.** fff matches nothing and reports a fallback internally;
  `fd`/`rg` produce the canonical error text, which is what the model should
  read.
- **Empty constrained searches.** When a glob-scoped query finds nothing, fff
  retries the raw pattern with the glob dropped; `rg`'s contract is zero hits.
- **File targets, init failures, scan timeouts, a cancelled call.**

`find` also appends matching directories (`sub/`) the way `fd` does — fff's
directory search is fuzzy-text only and ignores glob constraints, so the lane
matches `picker.get_dirs()` against the same glob itself.

## What it costs

- At most `MAX_RESIDENT_ROOTS` (4) indexes are resident; the oldest is evicted,
  which drops its watcher. A tree beyond the cap uses `fd`/`rg`.
- Each content cache is capped at 4096 files / 64 MB rather than fff's
  auto-sized 512 MB. A full index of this repo costs ~11 MB of RSS.
- Frecency databases (LMDB: paths, counts, timestamps — no file contents) live
  under `<gray_home>/fff-frecency/<hash-of-dir>`, so ranking survives restarts.
- First search in a process: the tool lane, while the index builds behind it.

## Measured

`cargo test -p gray-tools --test index_bench -- --nocapture`, 500 files × 200
lines, alternating lanes, medians:

| | index (warm) | tool lane | |
|---|---|---|---|
| `find` | 10.6 ms | 8.5 ms | **0.8×** — `fd` wins on a tree this small |
| `grep` | 5.3 ms | 15.2 ms | **2.9×** |

Fresh-process cost on a 20k-file repo: `gray find` 21 ms (tool lane), `fd`
9 ms — the index is never worth a cold scan, which is the whole point of the
policy above. The gain is grep, on repeat searches, in one process.

## Dependency

`fff-search = "=0.10.6"`, exact-pinned: nothing changes under us until someone
bumps it deliberately. It brings git2 (finding the worktree, git status in the
watcher), LMDB (frecency) and blake3 (content hashing) — three statically
linked C libraries, which is most of the +4.6 MB in the release tarball. The
optional `zlob` (needs Zig) and `ffi` features stay off: consumers must not
need a Zig toolchain to build. Revisit the pin when 0.11 has aged — it adds
`case_mode`, which would let the index serve `ignoreCase` too.

Audit status: MIT, crates.io only (no git or path sources anywhere in the
lock), no network-capable crate among the 89 it pulls, no process spawn at
runtime, and its `init_tracing` — which would seize the global `tracing`
subscriber and install a SIGSEGV handler — has no callers in the SDK.
`rustsec/audit-check` passes on the workspace lock.

## Testing

`crates/gray-tools/src/search_index_tests.rs` (pool behavior, glob
semantics, decline rules, the warm/cold policy, watcher pickup) and
`crates/gray/src/search_tests.rs` (the command answers exactly what the tool
answers, cold and warm).

```bash
cargo test -p gray-tools search_index
cargo test -p gray search
```
