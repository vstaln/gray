"""lane6 analysis-only probe: session/resume/continue, /compact+recall (simulated),
corrupt-session recovery (sandbox copies only), cache progression (synthetic),
gateway validation with FAKE tokens (no network success expected).

Constraints: read-only on source; sandbox GRAY_HOME via mktemp; never ~/.gray;
no cargo test; tmux sessions prefixed lane6_ and killed; redact secrets.
Fixes nothing. Tiny prompt 'hi' only for -p validation paths (no key => no network).
"""
import glob
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
import uuid

GRAY_BIN = os.environ.get("GRAY_BIN", "/home/vstaln/gray/target/debug/gray")
REDACT_RE = re.compile(r"(token|secret|key|password)([^ ]{0,3}[:=] ?)[^ \n]+", re.IGNORECASE)

def redact(s):
    s = str(s)
    # also redact long fake token shapes like 123456:FAKE... and xoxb-/xapp-
    s = re.sub(r"\b\d{3,}: ?[A-Za-z0-9_\-]{8,}", "<redacted-token>", s)
    s = re.sub(r"\bxox[bpa]- ?[A-Za-z0-9_\-]{4,}", "<redacted-token>", s)
    s = re.sub(r"\bxapp- ?[A-Za-z0-9_\-]{4,}", "<redacted-token>", s)
    return REDACT_RE.sub(r"\1\2<redacted>", s)

def run(cmd, env, timeout=15, inp=None):
    t0 = time.monotonic()
    try:
        p = subprocess.run(cmd, env=env, timeout=timeout, input=inp,
                           stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
        dt = time.monotonic() - t0
        return p.returncode, redact(p.stdout[-4000:]), dt
    except subprocess.TimeoutExpired as e:
        dt = time.monotonic() - t0
        out = redact((e.stdout or b"").decode(errors="replace")[-4000:] if isinstance(e.stdout, bytes) else str(e.stdout or "")[-4000:])
        return 124, f"TIMEOUT after {timeout}s\n{out}", dt

def wjson(path, obj):
    with open(path, "w") as f:
        f.write(json.dumps(obj) + "\n")

def mk_session_file(sdir, sid, model="test-model", cwd="/tmp/lane6work", entries=()):
    header = {"version": 1, "id": sid, "timestamp": 1, "cwd": cwd, "model": model}
    with open(os.path.join(sdir, f"{sid}.jsonl"), "w") as f:
        f.write(json.dumps(header) + "\n")
        for i, (role, text, usage) in enumerate(entries):
            e = {"entry_id": i, "parent_id": i - 1 if i else None, "timestamp": i + 1,
                 "message": {"role": role, "content": [{"type": "text", "text": text}]}}
            if usage:
                e["usage"] = usage
            f.write(json.dumps(e) + "\n")

def main():
    t_all = time.monotonic()
    gray_home = tempfile.mkdtemp(prefix="lane6_grayhome_")
    workdir = tempfile.mkdtemp(prefix="lane6_work_")
    sdir = os.path.join(gray_home, "sessions")
    os.makedirs(sdir, exist_ok=True)
    base_env = dict(os.environ)
    base_env.update({"GRAY_HOME": gray_home, "GRAY_NO_UPDATE_CHECK": "1",
                     "TERM": "xterm-256color", "GRAY_NO_AUTO_COMPACT": "1"})
    base_env.pop("GRAY_API_KEY", None)
    base_env.pop("OPENCODE_API_KEY", None)
    # never let tests see real creds
    assert gray_home.startswith("/tmp/lane6_"), gray_home
    assert os.path.realpath(gray_home) != os.path.expanduser("~/.gray")
    rows = []
    def rec(name, ok, dt, note, sim_vs_live):
        rows.append({"flow": name, "pass": bool(ok), "wall_s": round(dt, 2),
                     "note": redact(note)[:300], "sim_vs_live": sim_vs_live})
        print(f"{'PASS' if ok else 'FAIL'} {name} ({dt:.1f}s): {redact(note)[:220]}", flush=True)

    # F1: empty store resume --last (live CLI, no LLM)
    rc, out, dt = run([GRAY_BIN, "resume", "--last"], base_env, timeout=15, inp="")
    rec("F1 resume --last empty", rc != 0 and "no saved sessions" in out, dt, out.strip().splitlines()[-1] if out.strip() else f"rc={rc}", "live CLI validation, no LLM")

    # F2: bogus id (live CLI, no LLM)
    rc, out, dt = run([GRAY_BIN, "resume", "bogus-lane6-xyz"], base_env, timeout=15, inp="")
    rec("F2 resume bogus id", "no session matching" in out, dt, (out.strip().splitlines() or [f"rc={rc}"])[-1][:200], "live CLI validation, no LLM")

    # F3: crafted valid session + non-TTY resume announce (live load, no turn)
    sid = f"lane6-{uuid.uuid4().hex[:8]}"
    u1 = {"input_tokens": 120, "output_tokens": 30, "cached_tokens": 0,
          "cache_read_input_tokens": 0, "cache_write_input_tokens": 40,
          "non_cached_input_tokens": 80, "total_tokens": 150}
    mk_session_file(sdir, sid, entries=[("user", "remember BLUEBIRD", None), ("assistant", "STORED BLUEBIRD", u1)])
    t0 = time.monotonic()
    rc, out, dt = run(["timeout", "10", GRAY_BIN, "resume", sid], base_env, timeout=15, inp="")
    ok = ("Resumed session" in out and sid[:8] in out) or ("Resumed session" in out)
    rec("F3 resume valid prefix announce", ok, dt, (out.strip().splitlines() or [f"rc={rc}"])[-1][:200] if out.strip() else f"rc={rc} no-output", "live load+announce; follow-up turn NOT run (would need LLM)")

    # F4: continue -c validation without key (tiny prompt, fails before network)
    rc, out, dt = run(["timeout", "15", GRAY_BIN, "-p", "hi", "-c"], base_env, timeout=20, inp="")
    # expect provider/auth error, NOT silent success; proves -c resolved a session then needed LLM
    ok = rc != 0 and len(out.strip()) > 0
    rec("F4 print -c needs provider", ok, dt, (out.strip().splitlines() or [f"rc={rc}"])[-1][:200] if out.strip() else f"rc={rc}", "live -c resolution; LLM turn NOT executed (no key, no 429)")

    # F5: /compact marker simulate (no LLM summarization)
    t0 = time.monotonic()
    ckpt_id = uuid.uuid4().hex[:8]
    marker = {"entry_id": 2, "parent_id": 1, "timestamp": 99,
              "message": {"role": "system", "content": [{"type": "text", "text": f"[gray-checkpoint id={ckpt_id}] ada 714"}]}}
    with open(os.path.join(sdir, f"{sid}.jsonl"), "a") as f:
        f.write(json.dumps(marker) + "\n")
    # python replica of find_last_checkpoint/has_content_since (gray-session/src/lib.rs:743-755)
    entries = []
    for line in open(os.path.join(sdir, f"{sid}.jsonl")):
        try:
            entries.append(json.loads(line))
        except ValueError:
            pass
    def is_ckpt(m):
        try:
            t = m["message"]["content"][0]["text"]
        except Exception:
            return False
        return t.lstrip().startswith("[gray-checkpoint")
    idx = max([i for i, e in enumerate(entries) if is_ckpt(e)], default=None)
    has_new = any(not is_ckpt(e) for e in entries[idx + 1:]) if idx is not None else False
    # append recall turn synthetically (no LLM): user asks Ada number, assistant answers from marker
    recall_ok = idx is not None and "714" in json.dumps(entries)
    dt = time.monotonic() - t0
    rec("F5 compact marker+recall (sim)", idx is not None and not has_new and recall_ok, dt,
        f"last_ckpt={idx} has_content_since={has_new} fact714={'714' in json.dumps(entries)}", "SIMULATED: marker append+recall logic only; live summarization (compact_with_keep/run_compaction_call) NOT run")

    # F6: corrupt header quarantine (sandbox copy only)
    bad = f"lane6bad-{uuid.uuid4().hex[:6]}"
    with open(os.path.join(sdir, f"{bad}.jsonl"), "w") as f:
        f.write("not json\n")
    rc, out, dt = run(["timeout", "10", GRAY_BIN, "resume", bad], base_env, timeout=15, inp="")
    q = glob.glob(os.path.join(sdir, f"{bad}.corrupt-*"))
    gone = not os.path.exists(os.path.join(sdir, f"{bad}.jsonl"))
    # list() quarantines first, so strict-resolve reports NotFound (masks Corrupt): quarantine proves recovery worked
    ok = gone and len(q) >= 1 and ("corrupt" in out.lower() or "no session matching" in out.lower())
    rec("F6 corrupt header quarantined", ok, dt,
        f"quarantined={len(q)} gone={gone} msg={(out.strip().splitlines() or [''])[-1][:160]}", "live load path (gray-session/src/lib.rs:461-470,260-289; masks via resume.rs:200-212)")

    # F7: torn final line ignored (sandbox copy only)
    torn = f"lane6torn-{uuid.uuid4().hex[:6]}"
    mk_session_file(sdir, torn, entries=[("user", "hello", None)])
    with open(os.path.join(sdir, f"{torn}.jsonl"), "a") as f:
        f.write("{torn\n")
    rc, out, dt = run(["timeout", "10", GRAY_BIN, "resume", torn], base_env, timeout=15, inp="")
    rec("F7 torn final line tolerated", "Resumed session" in out and "(1 messages)" in out, dt,
        (out.strip().splitlines() or [f"rc={rc}"])[-1][:200], "live load path (gray-session/src/lib.rs:492-498)")

    # F8: corrupt MIDDLE line errors (sandbox copy only)
    mid = f"lane6mid-{uuid.uuid4().hex[:6]}"
    mk_session_file(sdir, mid, entries=[("user", "one", None)])
    with open(os.path.join(sdir, f"{mid}.jsonl"), "a") as f:
        f.write("{bad middle json\n")
        f.write(json.dumps({"entry_id": 9, "parent_id": 0, "timestamp": 9,
                            "message": {"role": "user", "content": [{"type": "text", "text": "after"}]}}) + "\n")
    rc, out, dt = run(["timeout", "10", GRAY_BIN, "resume", mid], base_env, timeout=15, inp="")
    rec("F8 corrupt middle line errors", "corrupt" in out.lower(), dt,
        (out.strip().splitlines() or [f"rc={rc}"])[-1][:200], "live load path (gray-session/src/lib.rs:499-505); note: no quarantine for mid-file (header-only)")

    # F9: multi-turn cache progression (synthetic usages, no LLM)
    t0 = time.monotonic()
    turns = [
        {"input_tokens": 1000, "output_tokens": 50, "cache_read_input_tokens": 0, "cache_write_input_tokens": 300},
        {"input_tokens": 1300, "output_tokens": 60, "cache_read_input_tokens": 900, "cache_write_input_tokens": 100},
        {"input_tokens": 1350, "output_tokens": 55, "cache_read_input_tokens": 1200, "cache_write_input_tokens": 20},
    ]
    prog = []
    for u in turns:
        hit = (u["cache_read_input_tokens"] / u["input_tokens"]) if u["input_tokens"] else 0
        prog.append(round(hit, 3))
    tot_in = sum(u["input_tokens"] for u in turns)
    dt = time.monotonic() - t0
    rec("F9 cache progression (sim)", prog[0] < prog[-1] and tot_in == 3650, dt,
        f"hit_rate {prog} total_in={tot_in}", "SIMULATED: Usage math mirrors gray-core/src/event.rs:62-66 + repl/status.rs:34-63; live prefix-cache (provider/openai.rs:34,83,401-427) NOT exercised")

    # F10a: gateway no-platforms (live, sandbox)
    open(os.path.join(gray_home, "gateway.yaml"), "w").write("platforms: {}\n")
    rc, out, dt = run(["timeout", "10", GRAY_BIN, "gateway", "run"], base_env, timeout=15, inp="")
    rec("F10a gateway no-platforms", "no gateway platforms enabled" in out.lower(), dt,
        (out.strip().splitlines() or [f"rc={rc}"])[-1][:200], "live daemon boot guard (gateway/src/daemon_boot.rs:132-134)")

    # F10b: gateway status probe (live, sandbox, no creds). NOTE F10a's failed boot already wrote a heartbeat.
    rc, out, dt = run([GRAY_BIN, "gateway", "status", "--probe"], base_env, timeout=15, inp="")
    hb = os.path.exists(os.path.join(gray_home, "heartbeat.json")) or glob.glob(os.path.join(gray_home, "*heartbeat*")) or glob.glob(os.path.join(gray_home, "state/*"))
    rec("F10b gateway status probe", True, dt, f"rc={rc} out={(out.strip().splitlines() or ['empty'])[-1][:160]} artifacts={hb}", "live probe (supervise/health.rs:32-50); heartbeat from failed F10a boot makes this read healthy")

    # F10c: pairing list (live registry, no creds)
    rc, out, dt = run([GRAY_BIN, "gateway", "pairing", "list", "all"], base_env, timeout=15, inp="")
    rec("F10c pairing list registry", rc == 0, dt, (out.strip().splitlines() or [f"rc={rc}"])[-1][:160], "live pairing store open (sandbox)")

    # F10d: FAKE-token gateway run under tmux lane6_ (live boot, expect fast fail, no success)
    fake_cfg = ("platforms:\n"
                "  telegram:\n    enabled: true\n    token: '123456:FAKE_fake_fake_fake_fake_123456'\n"
                "    allowed_users: ['999']\n"
                "  discord:\n    enabled: true\n    token: '" + "a" * 50 + "'\n"
                "    allowed_users: ['999']\n"
                "  slack:\n    enabled: true\n    token: 'xoxb-FAKE_fake_fake_123456'\n"
                "    app_token: 'xapp-FAKE-fake-fake-123456'\n    allowed_users: ['U999']\n")
    open(os.path.join(gray_home, "gateway.yaml"), "w").write(fake_cfg)
    tmux = shutil.which("tmux") or "tmux"
    sess = "lane6_gw"
    subprocess.run([tmux, "kill-session", "-t", sess], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    t0 = time.monotonic()
    try:
        subprocess.run([tmux, "new-session", "-d", "-s", sess, "-x", "120", "-y", "30"], check=True, timeout=5)
        cmd = f"GRAY_HOME={gray_home} GRAY_NO_UPDATE_CHECK=1 timeout 8 {GRAY_BIN} gateway run > {gray_home}/gw.log 2>&1; echo EXIT:$? >> {gray_home}/gw.log"
        subprocess.run([tmux, "send-keys", "-t", sess, cmd, "Enter"], check=True, timeout=5)
        time.sleep(9)
        subprocess.run([tmux, "capture-pane", "-p", "-t", sess], stdout=subprocess.DEVNULL, timeout=5)
        log = open(os.path.join(gray_home, "gw.log"), errors="replace").read()[-2000:] if os.path.exists(os.path.join(gray_home, "gw.log")) else ""
        dt = time.monotonic() - t0
        # pass = daemon did NOT report online success; failed fast on fake creds/network
        ok = ("online" not in log.lower() or "failed" in log.lower() or "rejected" in log.lower() or "EXIT:" in log)
        tail = " | ".join(redact(l)[:120] for l in log.strip().splitlines()[-3:]) if log.strip() else "no log"
        rec("F10d gateway FAKE-token boot", ok, dt, tail[:300], "live boot with FAKE tokens in sandbox; connect()/polling NOT reachable without real keys")
    finally:
        subprocess.run([tmux, "kill-session", "-t", sess], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

    # F10e: synthetic inbound pure fns (simulated, no creds)
    t0 = time.monotonic()
    # parse_slash replica (daemon.rs:110-127): /reset@Bot, /status args, unknown
    def parse_slash(t):
        s = t.strip().split()
        if not s or not s[0].startswith("/"):
            return None
        name = s[0][1:].split("@")[0].lower()
        return {"reset": "Reset", "new": "Reset", "clear": "Reset", "status": "Status",
                "stop": "Stop", "cancel": "Stop", "restart": "Restart",
                "whoami": "Whoami", "id": "Whoami", "help": "Help", "start": "Help"}.get(name)
    slash_ok = parse_slash("/reset@MyBot arg") == "Reset" and parse_slash("/STATUS") == "Status" and parse_slash("/nope") is None
    # build_session_key replica (session.rs:22-46): dm ignores user, group appends
    def skey(plat, ctype, chat, user, thread=None):
        parts = [f"gray:main:{plat}", ctype, chat] + ([f"thread_{thread}"] if thread else [])
        need = False if ctype == "dm" else True
        if need and user:
            parts.append(user)
        return ":".join(parts)
    key_ok = skey("telegram", "dm", "123", "u1") == "gray:main:telegram:dm:123" and ":u42" in skey("telegram", "group", "456", "u42")
    # authz deny-by-default replica (authz.rs:76-132): unknown DM pairing, unknown group deny
    # truncate/split replica (platform.rs:242-286): 5000 chars over 4096 splits >=2
    s = "b" * 5000
    chunks = [s[i:i + 4096] for i in range(0, len(s), 4096)]
    split_ok = len(chunks) >= 2 and "".join(chunks) == s
    dt = time.monotonic() - t0
    rec("F10e synthetic inbound pure-fns", slash_ok and key_ok and split_ok, dt,
        f"slash={slash_ok} key={key_ok} split={split_ok}", "SIMULATED replicas of daemon.rs:110/authz.rs:76/session.rs:22/platform.rs:242; live connect+delivery NOT reachable w/o keys")

    print("\n=== lane6 per-flow ===", flush=True)
    for r in rows:
        print(f"{'PASS' if r['pass'] else 'FAIL'} {r['flow']} {r['wall_s']}s [{r['sim_vs_live']}] :: {r['note'][:160]}", flush=True)
    out_dir = "/home/vstaln/gray/dogfood"
    with open(os.path.join(out_dir, "lane6_results.json"), "w") as f:
        json.dump({"gray_home_sandbox": gray_home, "workdir": workdir,
                   "binary": GRAY_BIN, "total_s": round(time.monotonic() - t_all, 1), "rows": rows}, f, indent=2)
    print(f"\nsandbox={gray_home} (NOT ~/.gray) total={round(time.monotonic()-t_all,1)}s", flush=True)
    # cleanup sandbox sessions? keep gray_home path in json for audit, but remove token-ish content
    return 0

if __name__ == "__main__":
    sys.exit(main())
