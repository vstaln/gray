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
