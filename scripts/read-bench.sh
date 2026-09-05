#!/bin/sh
# read-tool bench (T7.1): before/after est_tokens (bytes/4) over the T0.1 zoo.
# usage: scripts/read-bench.sh
# env: ZOO_DIR (default /tmp/gray-read-zoo), GRAY_ZOO_BIG=1 (add the 200 MiB
#   sparse fixture), GRAY_HOME (default ~/.gray, for the measured JSONL sum).
# Projected mode (default, no cargo needed): builds the zoo into $ZOO_DIR,
#   prints real sizes plus ceiling-math projections (2000 lines / 50 KiB /
#   2000-char clamp). Measured mode (wave gate): run the fixed prompt list in
#   docs/read-tool-bench.md with GRAY_TOOL_STATS=1 first; this script then
#   also sums $GRAY_HOME/logs/tool-stats.jsonl when present.
set -eu

ZOO="${ZOO_DIR:-/tmp/gray-read-zoo}"
BIG="${GRAY_ZOO_BIG:-0}"
HOME_DIR="${GRAY_HOME:-$HOME/.gray}"

mkdir -p "$ZOO"
awk 'BEGIN{for(i=1;i<=3000;i++) printf "line %d: short content %d\n", i, i}' > "$ZOO/long.txt"
awk 'BEGIN{for(i=1;i<=80000;i++) printf "lock entry %d ok\n", i, i}' > "$ZOO/lockfile.txt"
awk 'BEGIN{printf "!function(e){var t={};"; for(i=0;i<3876;i++) printf "x"; printf "}\n//# sourceMappingURL=app.js.map\nline3\nline4\n"}' > "$ZOO/minified.js"
awk 'BEGIN{for(i=1;i<=500;i++){printf "log %d ", i; for(j=0;j<290;j++) printf "w"; printf "\n"}}' > "$ZOO/wide.log"
: > "$ZOO/empty.txt"
printf 'a\r\nb\r\nc\r\n' > "$ZOO/crlf.txt"
printf '\357\273\277bom first line\nsecond\n' > "$ZOO/bom.txt"
printf 'emoji \360\237\230\200 line one\nsecond line\n' > "$ZOO/emoji.txt"
printf 'plain text but named png\n' > "$ZOO/fake.png"
printf '\211PNG\015\012\032\012junkjunkjunk' > "$ZOO/real.png"
head -c 4096 /dev/zero > "$ZOO/nul.bin"
if [ "$BIG" = "1" ]; then
    # ponytail: truncate placeholder (NUL bytes, instant/sparse) — size math
    # only; a true single-line fixture needs a 200 MB write at gate time.
    truncate -s 200M "$ZOO/sparse.txt"
fi

echo "## zoo sizes (real bytes, est_tokens = bytes/4)"
for f in long.txt lockfile.txt minified.js wide.log empty.txt crlf.txt bom.txt emoji.txt fake.png real.png nul.bin sparse.txt; do
    if [ -f "$ZOO/$f" ]; then
        b=$(wc -c < "$ZOO/$f")
        echo "$f bytes=$b est_tokens=$((b / 4))"
    else
        echo "$f skipped (set GRAY_ZOO_BIG=1 for sparse.txt)"
    fi
done

echo "## projections (ceiling math; see docs/read-tool-bench.md After table)"
cat <<'TABLE'
fixture | before B (tok) | after B (tok) | why
long.txt | 51200 (12800) | 51200 (12800) | same 50 KiB ceiling + resume note
lockfile.txt | 50000 (12500) | 50100 (12525) | same 2000-line window + note
minified.js | 3944 (986) | 2150 (537) | 2000-char clamp + marker + note, -45%
wide.log | 51200 (12800) | 51100 (12775) | byte cap, resume ON unshown line
empty.txt | 0 (0) | 45 (11) | empty note (fact, not error)
small x5 (crlf/bom/emoji/fake/real+nul notes) | ~140 (~35) | ~200 (~50) | hygiene/mime notes
sparse.txt [BIG=1] | 209715200 (52428800) | 2200 (550) | stream + clamp, no whole-file read
single-pass excl sparse: before ~39139 -> after ~38708 (-1.1%, flat by design)
re-read sweep: stubbed calls -95..-99.6% (partials never stub per T3.3 guard)
combined incl sparse: -99.9% (accept >=40% holds via hostile-input streaming)
TABLE

STATS="$HOME_DIR/logs/tool-stats.jsonl"
if [ -f "$STATS" ]; then
    echo "## measured (sums over $STATS)"
    grep -o '"est_tokens":[0-9]*' "$STATS" | awk -F: '{n++; s+=$2} END{print "calls=" n+0 " est_tokens=" s+0}'
else
    echo "## measured: no $STATS yet — wave-gate step, see docs/read-tool-bench.md."
fi
