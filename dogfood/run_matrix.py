"""TUI dogfood matrix: run scenarios, retry env-429s, write RESULTS.md.

Usage:
    python3 run_matrix.py [--only multiturn_bugfix,resume_continue]

Each scenario runs via driver.py (PTY + screenshot harness — the repo's
TUI automation path). Environmental 429s: wait 60s, retry up to 3x.
RESULTS.md lands in the final runs/<scenario>-<ts>/ dir with full token
accounting from GRAY_HOME (gray.log turns + session-JSONL usage rows).
Secrets are never written: only token COUNTS, never values.
"""

import glob
import os
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
from scenarios.dfutil import parse_gray_log, redact, session_usages  # noqa: E402

MATRIX = [
    ("multiturn_bugfix", {}),
    ("resume_continue", {}),
    ("error_recovery", {}),
    ("effort_fix", {"DOG_EFFORT": "low", "tag": "effort_low"}),
    ("effort_fix", {"DOG_EFFORT": "high", "tag": "effort_high"}),
    ("compact_check", {}),
]

ATTEMPT_TIMEOUT = 1200


def newest_run(name):
    cands = sorted(glob.glob(os.path.join(HERE, "runs", f"{name}-*")),
                   key=os.path.getmtime)
    return cands[-1] if cands else None


def run_once(name, extra):
    env = dict(os.environ)
    env.update(extra)
    env.pop("DOG_GRAY_HOME", None)
    env.pop("DOG_WORKDIR", None)
    t0 = time.time()
    try:
        p = subprocess.run([sys.executable, "driver.py", name],
                           cwd=HERE, capture_output=True, text=True,
                           timeout=ATTEMPT_TIMEOUT, env=env)
        dt = time.time() - t0
        return p.returncode, redact(p.stdout + p.stderr)[-1500:], dt
    except subprocess.TimeoutExpired as e:
        dt = time.time() - t0
        out = redact((e.stdout or b"").decode(errors="replace") +
                     (e.stderr or b"").decode(errors="replace"))[-1500:]
        return 124, f"TIMEOUT after {ATTEMPT_TIMEOUT}s :: {out}", dt


def pick_run(name, before, t0):
    """Newest runs/<name>-* dir from this attempt holding result.txt.

    The checkout is shared: foreign agents may create same-named dirs
    concurrently. Only dirs born inside this attempt window count, and a
    dir without result.txt is stillborn (killed/collided), never a verdict.
    """
    cands = []
    for d in glob.glob(os.path.join(HERE, "runs", f"{name}-*")):
        if d in before:
            continue
        try:
            if os.path.getmtime(d) < t0 - 5:
                continue
        except OSError:
            continue
        cands.append(d)
    with_result = [d for d in cands
                   if os.path.exists(os.path.join(d, "result.txt"))]
    pool = with_result or cands
    return sorted(pool, key=os.path.getmtime)[-1] if pool else None


def environmental(run_dir):
    """True if the failure looks like a transient 429/limit, worth retry."""
    if not run_dir:
        return False
    g = parse_gray_log(os.path.join(run_dir, "gray-home"))
    if g["n_429"]:
        return True
    for src in ("result.txt",):
        p = os.path.join(run_dir, src)
        if os.path.exists(p):
            t = open(p, errors="replace").read().lower()
            if "rate limit" in t or "429" in t or "overloaded" in t:
                return True
    return False


def fmt_table(rows):
    lines = ["| turn | stop | in | out | reasoning | cached | cache_read | cache_write |",
             "| --- | --- | --- | --- | --- | --- | --- | --- |"]
    for i, r in enumerate(rows, 1):
        lines.append(f"| {i} | {r['stop']} | {r['in']} | {r['out']} | "
                     f"{r['reasoning']} | {r['cached']} | {r['read']} | {r['write']} |")
    return "\n".join(lines)


def write_results(run_dir, name, tag, ok, note, attempts, walls, extra_note=""):
    g = parse_gray_log(os.path.join(run_dir, "gray-home"))
    su = session_usages(os.path.join(run_dir, "gray-home"))
    # Align session usage rows with gray.log turns (both 1:1 per turn).
    rows = []
    for i, t in enumerate(g["turns"]):
        s = su[i] if i < len(su) else {}
        rows.append({"stop": t[0], "in": t[1], "out": t[2],
                     "reasoning": s.get("reasoning", "n/a"),
                     "cached": t[3],
                     "read": s.get("read", "n/a"),
                     "write": s.get("write", "n/a")})
    tot_in = sum(t[1] for t in g["turns"])
    tot_out = sum(t[2] for t in g["turns"])
    tot_re = sum(s.get("reasoning", 0) for s in su if isinstance(s.get("reasoning"), int))
    reads = [s["read"] for s in su if isinstance(s.get("read"), int)]
    prog = ("cache-hit progression: " +
            (" -> ".join(map(str, reads)) if reads else "no session usage rows")) \
        + ("; turn2+ cache hit: " +
           ("YES" if any(r > 0 for r in reads[1:]) else "NO") if len(reads) > 1 else "")
    tools = ", ".join(f"{k}x{v}" for k, v in sorted(g["tools"].items())) or "none"
    rp = os.path.join(run_dir, "result.txt")
    verdict = open(rp, errors="replace").read().strip() if os.path.exists(rp) \
        else "missing result.txt (stillborn run)"
    lines = [
        f"# RESULTS — {tag} ({name})",
        "",
        f"verdict: {'PASS' if ok else 'FAIL'} — {redact(note)}",
        f"driver verdict file: {redact(verdict)}",
        f"attempts: {attempts} (env-429 retry: wait 60s, up to 3x)",
        "wall_time_s: " + ", ".join(f"{w:.0f}" for w in walls),
        f"run_dir: {run_dir}",
        "",
        "## token accounting (per turn, from GRAY_HOME)",
        fmt_table(rows) if rows else "no agent-run-end lines in gray.log",
        "",
        f"totals: in={tot_in} out={tot_out} reasoning={tot_re} "
        f"(reasoning from session JSONL; n/a in print-mode rows)",
        prog,
        "",
        "## transport",
        f"429 lines: {g['n_429']} | 400 lines: {g['n_400']} | "
        f"tool calls: {tools}",
    ]
    if g["provider_errors"]:
        lines += ["", "## provider errors (redacted)"] + \
                 [f"- {e}" for e in g["provider_errors"]]
    if extra_note:
        lines += ["", "## note", extra_note]
    lines += ["",
              "TUI path: dogfood/driver.py (pexpect PTY + pyte screenshots; "
              "the repo's committed TUI harness, equivalent to "
              "tmux send-keys/capture-pane).",
              "Isolation: fresh per-run GRAY_HOME under runs/ (gitignored); "
              "auth via env only, real ~/.gray never read or written.",
              ""]
    with open(os.path.join(run_dir, "RESULTS.md"), "w") as f:
        f.write("\n".join(lines) + "\n")
    return {"tag": tag, "ok": ok, "walls": walls, "in": tot_in,
            "out": tot_out, "re": tot_re, "n429": g["n_429"],
            "n400": g["n_400"], "tools": tools, "dir": run_dir}


def main():
    only = None
    if "--only" in sys.argv:
        only = set(sys.argv[sys.argv.index("--only") + 1].split(","))
    summaries = []
    for name, cfg in MATRIX:
        tag = cfg.get("tag", name)
        if only and tag not in only and name not in only:
            continue
        extra = {k: v for k, v in cfg.items() if k != "tag"}
        attempts, walls, last_dir, note, rc = 0, [], None, "", 1
        while True:
            attempts += 1
            before = set(glob.glob(os.path.join(HERE, "runs", f"{name}-*")))
            t_start = time.time()
            rc, out, dt = run_once(name, extra)
            walls.append(dt)
            last_dir = pick_run(name, before, t_start) or newest_run(name)
            if last_dir is None:
                note = f"no run dir appeared :: {out[-300:]}"
            else:
                rp = os.path.join(last_dir, "result.txt")
                note = open(rp, errors="replace").read().strip() \
                    if os.path.exists(rp) else f"no result.txt :: {out[-300:]}"
                if rc == 0 and not os.path.exists(rp):
                    rc = 1
            print(f"[{tag}] attempt {attempts}: rc={rc} {dt:.0f}s :: {note[:160]}",
                  flush=True)
            if rc == 0 or attempts > 3 or not environmental(last_dir):
                break
            print(f"[{tag}] environmental 429 — sleeping 60s before retry", flush=True)
            time.sleep(60)
        if last_dir is None:
            summaries.append({"tag": tag, "ok": False, "walls": walls,
                              "in": 0, "out": 0, "re": 0, "n429": 0,
                              "n400": 0, "tools": "none", "dir": "(no run dir)"})
        else:
            summaries.append(write_results(last_dir, name, tag, rc == 0, note,
                                           attempts, walls))
    print("\n==== SUMMARY ====")
    for s in summaries:
        print(f"{s['tag']}: {'PASS' if s['ok'] else 'FAIL'} "
              f"wall={[f'{w:.0f}s' for w in s['walls']]} "
              f"in={s['in']} out={s['out']} re={s['re']} "
              f"429={s['n429']} 400={s['n400']} tools={s['tools']}")
        print(f"  {s['dir']}/RESULTS.md")
    eff = [s for s in summaries if s["tag"].startswith("effort_")]
    if len(eff) == 2:
        lo, hi = eff
        cmp_lines = [
            "",
            "## effort comparison (identical task, low vs high)",
            "|  | low | high |",
            "| --- | --- | --- |",
            f"| verdict | {'PASS' if lo['ok'] else 'FAIL'} | {'PASS' if hi['ok'] else 'FAIL'} |",
            f"| wall s | {', '.join(f'{w:.0f}' for w in lo['walls'])} | "
            f"{', '.join(f'{w:.0f}' for w in hi['walls'])} |",
            f"| in / out / reasoning | {lo['in']} / {lo['out']} / {lo['re']} | "
            f"{hi['in']} / {hi['out']} / {hi['re']} |",
            f"| 429 / 400 | {lo['n429']} / {lo['n400']} | {hi['n429']} / {hi['n400']} |",
            f"| tools | {lo['tools']} | {hi['tools']} |",
            "",
        ]
        print("\n".join(cmp_lines))
        for s in eff:
            if os.path.isdir(s["dir"]):
                with open(os.path.join(s["dir"], "RESULTS.md"), "a") as f:
                    f.write("\n".join(cmp_lines) + "\n")
    return 0 if all(s["ok"] for s in summaries) else 1


if __name__ == "__main__":
    sys.exit(main())
