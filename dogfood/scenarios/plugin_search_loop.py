"""Loop runner: 10 varied searches across 3 fresh homes, PASS/FAIL per query.

Homes 1-2 use an empty Gray Index over loopback (production index 404s)
so searches fan out to the live pi side. Home 3 uses an invalid
GRAY_PLUGIN_INDEX file:// URL to prove the degrade path stays graceful
(no crash, no hang). Writes RESULTS.md into the run dir. No model.
"""

import os
import time

from driver import Session
from scenarios.plugin_common import boot_fresh, start_index_server

MARKERS = ("[Gray Index]", "[Pi Gallery (preview)]",
           "Pi Gallery (preview): unreachable", "Gray Index: unreachable",
           "not in index:", "search failed:", "usage: /plugin search")

QUERIES_H1 = ["picadillo", "skills", "themes", "extensions"]
QUERIES_H2 = ["context-mode", "a", "zzzqxj-novendor-xyz", ""]
QUERIES_H3 = ["pi", "picadillo"]  # degrade home: invalid file:// index

_state = {}


def seed(gray_home, workdir):
    _state["index_url"] = start_index_server(workdir)


def extra_env():
    return {"GRAY_PLUGIN_INDEX": _state["index_url"]}


def do_search(sess, query, timeout=50):
    """Run one search, classify the outcome. Returns (outcome, detail)."""
    sess.press("esc")
    before = sess.text()
    sess.send("/plugin search" + (f" {query}" if query else ""))
    sess.press("enter")
    end = time.time() + timeout
    outcome, detail = None, ""
    while time.time() < end:
        cur = sess.text()
        if cur != before and any(m in cur for m in MARKERS):
            labels = [m for m in ("[Gray Index]", "[Pi Gallery (preview)]") if m in cur]
            if "usage: /plugin search" in cur and not query:
                outcome, detail = "USAGE", "empty query rejected client-side"
            elif labels:
                outcome, detail = "HITS", "+".join(
                    "gray" if l == "[Gray Index]" else "pi" for l in labels)
                if "Gray Index: unreachable" in cur:
                    # b78cedf degrade-to-advisory: broken gray index still
                    # serves pi hits with a gray-unreachable advisory line.
                    detail += "+gray-advisory"
            elif "Pi Gallery (preview): unreachable" in cur:
                outcome, detail = "ADVISORY", "pi side down, gray side survived"
            elif "not in index:" in cur:
                outcome, detail = "MISS", "total miss line rendered"
            elif "search failed:" in cur:
                outcome, detail = "ERROR", "graceful failure line, no crash"
            break
        time.sleep(0.5)
    if outcome is None:
        return "STALL", "no marker within timeout"
    sess.wait_idle(quiet_secs=2, timeout=60)
    if not sess.child.isalive():
        return "CRASH", detail + " (gray died)"
    return outcome, detail


def run(sess, ctx):
    rows = []
    extra = []

    def attempt(home_tag, s, query):
        t0 = time.time()
        outcome, detail = do_search(s, query)
        ok = outcome in ("HITS", "MISS", "ADVISORY", "ERROR", "USAGE")
        # Empty query must be USAGE; degrade home must stay graceful:
        # hard ERROR, or pi HITS carrying the gray-unreachable advisory.
        if query == "" and outcome != "USAGE":
            ok = False
        rows.append((home_tag, query or "(empty)", outcome, detail,
                     f"{time.time() - t0:.0f}s", "PASS" if ok else "FAIL"))
        s.shot(f"loop-{home_tag}-{query or 'empty'}")
        return ok

    if not boot_fresh(sess, "loop-h1-ready"):
        return False, "search-loop: home1 boot failed"
    for q in QUERIES_H1:
        attempt("home1", sess, q)

    # Home 2: second fresh home, same healthy index.
    try:
        h2 = os.path.join(ctx["run_dir"], "home2")
        w2 = os.path.join(ctx["run_dir"], "work2")
        os.makedirs(h2, exist_ok=True)
        os.makedirs(w2, exist_ok=True)
        s2 = Session(h2, w2, ctx["run_dir"],
                     extra_env={"GRAY_PLUGIN_INDEX": _state["index_url"]})
        extra.append(s2)
        if not boot_fresh(s2, "loop-h2-ready"):
            rows.append(("home2", "-", "BOOT-FAIL", "picker never ready", "-", "FAIL"))
        else:
            for q in QUERIES_H2:
                attempt("home2", s2, q)
    except Exception as e:
        rows.append(("home2", "-", "SPAWN-FAIL", repr(e), "-", "FAIL"))

    # Home 3: degrade path — invalid file:// index URL, networking untouched.
    try:
        h3 = os.path.join(ctx["run_dir"], "home3")
        w3 = os.path.join(ctx["run_dir"], "work3")
        os.makedirs(h3, exist_ok=True)
        os.makedirs(w3, exist_ok=True)
        s3 = Session(h3, w3, ctx["run_dir"],
                     extra_env={"GRAY_PLUGIN_INDEX": "file:///nonexistent/index.json"})
        extra.append(s3)
        if not boot_fresh(s3, "loop-h3-ready"):
            rows.append(("home3", "-", "BOOT-FAIL", "picker never ready", "-", "FAIL"))
        else:
            for q in QUERIES_H3:
                t0 = time.time()
                outcome, detail = do_search(s3, q)
                # Graceful = hard 'search failed' OR pi hits + gray advisory
                # (b78cedf degrade path), never crash/hang.
                ok = outcome == "ERROR" or "gray-advisory" in detail
                rows.append(("home3", q, outcome, detail,
                             f"{time.time() - t0:.0f}s", "PASS" if ok else "FAIL"))
                s3.shot(f"loop-home3-{q}")
    except Exception as e:
        rows.append(("home3", "-", "SPAWN-FAIL", repr(e), "-", "FAIL"))
    finally:
        for s in extra:
            try:
                s.close()
            except Exception:
                pass

    lines = ["# plugin_search_loop RESULTS", "",
             "| # | home | query | outcome | detail | time | verdict |",
             "|---|---|---|---|---|---|---|"]
    for i, (h, q, o, d, t, v) in enumerate(rows, 1):
        lines.append(f"| {i} | {h} | `{q}` | {o} | {d} | {t} | {v} |")
    passed = sum(1 for r in rows if r[5] == "PASS")
    lines += ["", f"{passed}/{len(rows)} queries passed."]
    with open(os.path.join(ctx["run_dir"], "RESULTS.md"), "w") as f:
        f.write("\n".join(lines) + "\n")

    if len(rows) != 10:
        return False, f"search-loop: only {len(rows)}/10 queries ran"
    if passed != len(rows):
        bad = [f"{r[0]}:{r[1]}={r[2]}" for r in rows if r[5] == "FAIL"]
        return False, f"search-loop: {passed}/10 passed; failed: {', '.join(bad)}"
    return True, "search-loop: 10/10 queries graceful across 3 homes, RESULTS.md written"
