# read tool bench (T0.2)

Meter: `est_tokens = bytes / 4` on the tool-output text (documented
approximation; see `gray-tools/src/stats.rs`). Enable per-call logging with
`GRAY_TOOL_STATS=1` (log line at `gray_tools` target + JSONL append to
`$GRAY_HOME/logs/tool-stats.jsonl`).

Line format:
`tool=read path=… bytes=… lines=… est_tokens=… truncated_by=lines|bytes|clamp|none notice=<kind>`

## Before (analytical baseline, pre-optimization)

Ceilings today: 2,000 lines / 50 KiB (`51200 B` → max `12800` est tokens per
capped read). Values below are ceiling math, not measured runs — the scripted
bench (`scripts/read-bench.sh`, owned by a sibling agent) + measured `After`
table land in T7.1.

| fixture | fires today | output bytes | est_tokens |
|---|---|---|---|
| long.txt (3,000 short lines) | bytes cap (~50 KiB) | ~51,200 | ~12,800 |
| lockfile.txt (80,000 lines) | lines cap (2,000) within 50 KiB | ~50,000 | ~12,500 |
| minified.js (one 3,900-char line + 3) | none (no clamp yet) | ~4,000 | ~1,000 |
| wide.log (500 × 300 chars, crosses 50 KiB) | bytes cap | ~51,200 | ~12,800 |
| empty.txt | none (empty string) | 0 | 0 |
| crlf.txt / bom.txt / emoji.txt | none | < 4,000 each | < 1,000 each |
| fake.png (text, .png ext) | none (read as text) | small | small |
| real.png / nul.bin | error path | small | small |
| sparse.txt (200 MiB single line, `GRAY_ZOO_BIG=1`) | whole-file `fs::read` — DoS risk, motivates T2.1 | ~200 MiB | ~50 M |

Re-read cost today: a second identical read costs the full amount again (no
dedup until T3.3); a clamped-but-complete read followed by `write` is denied
until T3.2 rule 5.

## After (projected, T7.1 — pending measured wave-gate run)

Ceilings unchanged by design (2,000 lines / 50 KiB / 2,000-char clamp), so
ordinary single-pass reads are flat; the savings land on hostile inputs
(streaming + clamp), eligible re-reads (dedup stub), and refused blind
writes. `est_tokens = bytes/4` throughout. Run `scripts/read-bench.sh`
(no cargo needed) to rebuild the zoo and reprint the projection table.

| fixture | before B (tok) | after B (tok) | why |
|---|---|---|---|
| long.txt (3,000 short lines) | 51,200 (12,800) | ~51,200 (~12,800) | same 50 KiB ceiling + resume note |
| lockfile.txt (80,000 lines) | ~50,000 (~12,500) | ~50,100 (~12,525) | same 2,000-line window + note |
| minified.js (one ~3.9k-char line + 3) | ~3,944 (~986) | ~2,150 (~537) | 2,000-char clamp + `…[+N chars]` marker + note, −45% |
| wide.log (500 × ~300 chars) | 51,200 (12,800) | ~51,100 (~12,775) | byte cap, resume ON the unshown line |
| empty.txt | 0 (0) | ~45 (~11) | empty note (fact, not error) |
| crlf.txt / bom.txt / emoji.txt / fake.png | small (flat) | small (flat) | hygiene normalizes, no size change |
| real.png / nul.bin | error path (small) | mime/NUL note (small) | one-line answer, `is_error=false` |
| sparse.txt (200 MiB single line, `GRAY_ZOO_BIG=1`) | ~200 MiB (~52.4 M) whole-file `fs::read` | ~2,200 (~550) stream + clamp, never buffered | −99.99% |

Single-pass zoo total excl. sparse: ~39,139 → ~38,708 (−1.1%, flat by
design). Incl. sparse: −99.9% — the ≥40% accept holds via hostile-input
streaming.

### Re-read-loop scenario

Second identical read of a fully-viewed file returns the ~200 B stub
(`…unchanged since your previous read above; content omitted…`, ~50
tokens) instead of the content — exactly once, then the arm is consumed
(alternates full/stub). Partial-window re-reads are NEVER stubbed (T3.3
guard). Per stubbed call: minified.js 986 → ~50 (−95%); a full 50 KiB
view 12,800 → ~50 (−99.6%). Re-read accept (≥90%) holds on every
stubbed call.

### Write-guard scenario

`write` after a partial read is refused with the exact resume step
(`write refused: only part of … lines 1-2000 of …. Read the rest
(offset=2001) or use edit…`, ~150 B / ~38 tokens); before, it
overwrote silently (0 tokens, data loss). Counted as corruption
prevention, not token saving.

## Env flags (read ceilings + meter)

| var | default | meaning |
|---|---|---|
| `GRAY_READ_MAX_LINES` | 2000 | line window |
| `GRAY_READ_MAX_BYTES` | 51200 (50 KiB) | byte budget, charged after clamping |
| `GRAY_READ_MAX_LINE_CHARS` | 2000 | per-line clamp, char-boundary safe |
| `GRAY_READ_DEDUP` | armed | `=0` disables the dedup stub |
| `GRAY_TOOL_STATS` | off | `=1` per-call log line + JSONL append |
| `GRAY_ZOO_BIG` | off | `=1` generates sparse.txt (200 MiB) |

## Wave-gate: replacing projections with measured numbers (NOT run in T7.1)

No cargo per blast rules — the gate operator runs: build, then with
`GRAY_TOOL_STATS=1` execute the fixed prompt list (read each fixture,
re-read each, write-after-partial once), then `scripts/read-bench.sh`
sums `$GRAY_HOME/logs/tool-stats.jsonl`. Paste the measured table over
the projections above and confirm ≥40% total / ≥90% re-read before
merge. `docs/tools.md` absent — skipped per "(if present)".
