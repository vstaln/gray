"""Task 5 runner: driver oracle + tmux SGR captures per scenario.

Fresh mktemp GRAY_HOME per run, GRAY_NO_UPDATE_CHECK=1, never touches real
~/.gray (readonly_turn's seed copies config+auth into the isolated run home;
values never printed). Binary: target/debug/gray (fresh; no rebuild).

Usage: python3 task5_run.py [scenario ...]  (default: all 8)
Writes tmux-*.ansi.txt + meta.json into each driver's runs/<s>-<ts>/ dir.
"""
import json
import os
import re
import subprocess
import sys
import tempfile
import time

HERE = os.path.dirname(os.path.abspath(__file__))
GRAY_BIN = os.path.join(HERE, "..", "target", "debug", "gray")
ALL = ["modal_help", "modal_model", "modal_thinking", "modal_provider",
       "modal_context", "modal_permissions", "modal_skills", "readonly_turn"]
# modal open command per scenario for the tmux capture pass
MODAL_CMD = {"modal_help": "/help", "modal_model": "/model",
             "modal_thinking": "/thinking", "modal_provider": "/connect",
             "modal_context": "/context", "modal_permissions": "/permissions",
             "modal_skills": "/skills"}


def sh(*args, **kw):
    return subprocess.run(args, capture_output=True, text=True, timeout=kw.get("timeout", 30))


def tmux(*args, timeout=30):
    return sh("tmux", *args, timeout=timeout)


def pane_text(sess):
    r = tmux("capture-pane", "-p", "-t", sess)
    return r.stdout


def pane_ansi(sess):
    r = tmux("capture-pane", "-e", "-p", "-t", sess)
    return r.stdout


def wait_text(sess, substr, timeout=25):
    end = time.time() + timeout
    while time.time() < end:
        if substr in pane_text(sess):
            return True
        time.sleep(0.5)
    return False


def seed_home(gray_home, workdir, scenario):
    """Mirror scenarios' seeds without importing them (plus thinking_effort=max
    for modal_thinking per brief: reproduce user state)."""
    if scenario == "modal_thinking":
        with open(os.path.join(gray_home, "config.json"), "w") as f:
            json.dump({"model": "muse-spark-1.3-contributor",
                       "thinking_effort": "max"}, f)
    if scenario == "modal_skills":
        bundle = os.path.join(gray_home, "skills", "dogfood-nav")
        os.makedirs(bundle, exist_ok=True)
        with open(os.path.join(bundle, "SKILL.md"), "w") as f:
            f.write("---\nname: dogfood-nav\ndescription: nav fixture skill\n---\nDo the nav thing.\n")


def run_driver(scenario, timeout=420):
    """One driver.py oracle run with isolated mktemp home. Returns dict."""
    home = tempfile.mkdtemp(prefix=f"t5-{scenario}-home-")
    work = tempfile.mkdtemp(prefix=f"t5-{scenario}-work-")
    env = dict(os.environ, DOG_GRAY_HOME=os.path.join(home, "gh"),
               DOG_WORKDIR=os.path.join(work, "wk"),
               GRAY_NO_UPDATE_CHECK="1")
    os.makedirs(os.path.join(home, "gh"), exist_ok=True)
    os.makedirs(os.path.join(work, "wk"), exist_ok=True)
    if scenario == "modal_thinking":
        env["GRAY_THINKING_EFFORT"] = "max"  # user state; config.rs:71-72
    t0 = time.time()
    try:
        p = subprocess.run([sys.executable, "driver.py", scenario], cwd=HERE,
                           env=env, capture_output=True, text=True, timeout=timeout)
        out = p.stdout + p.stderr
    except subprocess.TimeoutExpired:
        out = "TIMEOUT"
    wall = time.time() - t0
    m = re.search(r"run: (\S+)", out)
    rundir = m.group(1) if m else None
    ok = out.startswith("PASS")
    first = out.strip().splitlines()[0] if out.strip() else "no output"
    return {"ok": ok, "wall": wall, "rundir": rundir, "note": first,
            "gray_home": os.path.join(home, "gh"), "out": out}


def parse_metrics(gray_home):
    """tokens in/out/cached + 429/400 counts from the run's GRAY_HOME logs."""
    met = {"in": 0, "out": 0, "cached": 0, "n429": 0, "n400": 0, "lines": 0}
    log = os.path.join(gray_home, "logs", "gray.log")
    if not os.path.exists(log):
        return met
    with open(log, errors="replace") as f:
        body = f.read()
    met["lines"] = body.count("\n")
    for m in re.finditer(r"usage in=(\d+) out=(\d+) cached=(\d+)", body):
        met["in"] += int(m.group(1))
        met["out"] += int(m.group(2))
        met["cached"] += int(m.group(3))
    met["n429"] = len(re.findall(r"429", body))
    met["n400"] = len(re.findall(r"400 Bad Request|status 400", body))
    return met


def tmux_capture_pass(scenario, rundir):
    """Gray directly in tmux; capture-pane -e per modal into rundir.
    Returns (rgb_set, note)."""
    if scenario == "readonly_turn" or scenario not in MODAL_CMD:
        return set(), "n/a (no modal)"
    sess = "t5m-" + scenario.replace("modal_", "")
    tmux("kill-session", "-t", sess)
    gh = tempfile.mkdtemp(prefix=f"t5-{scenario}-tmuxhome-")
    seed_home(gh, gh, scenario)
    env_prefix = f"GRAY_HOME={gh} GRAY_NO_UPDATE_CHECK=1 TERM=xterm-256color"
    if scenario == "modal_thinking":
        env_prefix += " GRAY_THINKING_EFFORT=max"
    if scenario == "modal_skills":
        env_prefix += f" HOME={gh}"
    tmux("new-session", "-d", "-s", sess, "-x", "120", "-y", "30")
    tmux("send-keys", "-t", sess, f"env {env_prefix} {GRAY_BIN}", "Enter")
    try:
        if wait_text(sess, "Connect a provider", 12):
            tmux("send-keys", "-t", sess, "Escape")
        elif not wait_text(sess, "\u276f", 20):
            # Seeded-model boot skips the provider picker; accept either path.
            with open(os.path.join(rundir, "tmux-boot-fail.ansi.txt"), "w") as f:
                f.write(pane_ansi(sess))
            return set(), "boot picker never appeared"
        if not wait_text(sess, "\u276f", 15):
            return set(), "no prompt after Esc"
        time.sleep(0.5)
        with open(os.path.join(rundir, "tmux-ready.ansi.txt"), "w") as f:
            f.write(pane_ansi(sess))
        tmux("send-keys", "-t", sess, MODAL_CMD[scenario], "Enter")
        time.sleep(3)
        ansi = pane_ansi(sess)
        with open(os.path.join(rundir, "tmux-modal-open.ansi.txt"), "w") as f:
            f.write(ansi)
        with open(os.path.join(rundir, "tmux-modal-open.txt"), "w") as f:
            f.write(pane_text(sess))
        tmux("send-keys", "-t", sess, "Down")
        time.sleep(1)
        with open(os.path.join(rundir, "tmux-modal-moved.ansi.txt"), "w") as f:
            f.write(pane_ansi(sess))
        tmux("send-keys", "-t", sess, "Escape")
        time.sleep(1)
        rgbs = set(re.findall(r"48;2;(\d+);(\d+);(\d+)", ansi))
        return rgbs, "captured"
    finally:
        tmux("kill-session", "-t", sess)


def main():
    scenarios = sys.argv[1:] or ALL
    for s in scenarios:
        if s not in ALL:
            print(f"SKIP unknown {s}")
            continue
        # 429s are environmental: wait 60s, retry x3 (only readonly_turn dials out)
        attempt, res, delay_note = 0, None, ""
        while True:
            res = run_driver(s)
            met = parse_metrics(res["gray_home"])
            res["met"] = met
            transient = ("429" in res["out"] or "overloaded" in res["out"]
                         or met["n429"] > 0)
            if (s == "readonly_turn" and not res["ok"] and transient
                    and attempt < 3):
                attempt += 1
                delay_note += f" [429-retry{attempt}:wait60s]"
                time.sleep(60)
                continue
            break
        res["note"] += delay_note
        if res["rundir"] and os.path.isdir(res["rundir"]):
            try:
                rgbs, cnote = tmux_capture_pass(s, res["rundir"])
                res["rgbs"] = sorted(rgbs)
                res["capnote"] = cnote
                meta = {k: v for k, v in res.items() if k != "out"}
                with open(os.path.join(res["rundir"], "meta.json"), "w") as f:
                    json.dump(meta, f, indent=1)
            except Exception as e:
                print(f"{s}: capture pass raised {e!r}")
        m = res["met"]
        print(f"{'PASS' if res['ok'] else 'FAIL'} {s} wall={res['wall']:.0f}s "
              f"in={m['in']} out={m['out']} cached={m['cached']} "
              f"429={m['n429']} 400={m['n400']} :: {res['note']} :: run={res['rundir']}")


if __name__ == "__main__":
    main()
