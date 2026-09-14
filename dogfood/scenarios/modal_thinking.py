"""Modal matrix: /thinking picker, muse-spark max-absent + chrome.

Seeds model=muse-spark-1.3-contributor (no keys, offline). The provider's
live error (see readonly_turn 20260907-210728 gray.log) says max is NOT
supported for this model — Supported values: [minimal, low, medium, high,
xhigh]. The picker MUST NOT list max. Also exercises navigate/cancel and
the garbage-level error. Zero LLM calls.
"""
import json
import os
import time


def seed(gray_home, _workdir):
    with open(os.path.join(gray_home, "config.json"), "w") as f:
        json.dump({"model": "muse-spark-1.3-contributor"}, f)


def _boot(sess):
    # Seeded model may skip the provider picker; accept either path.
    if sess.expect_text("Connect a provider", timeout=8):
        sess.press("esc")
    if not sess.expect_text("\u276f", timeout=15):
        return False
    sess.shot("thinking-ready")
    return True


def run(sess, _ctx):
    if not _boot(sess):
        return False, "thinking: boot prompt never appeared"

    sess.press("esc")
    before = sess.text()
    sess.send("/thinking")
    sess.press("enter")
    # Wait for the modal title (picker) — could also be a bare toggle line
    # if the command routed elsewhere; title is the oracle.
    found = False
    end = time.time() + 30
    while time.time() < end:
        if "Thinking effort" in sess.text():
            found = True
            break
        time.sleep(0.5)
    if not found:
        return False, "thinking: '/thinking' did not open the effort picker"
    sess.wait_idle(quiet_secs=2, timeout=30)
    sess.shot("thinking-open")

    body = sess.text()
    # Core assertion: max MUST be absent for muse-spark-1.3-contributor.
    # Scope to the picker modal rows only: lines between the "Thinking
    # effort" title and the navigate hint. The footer status line carries
    # the current-effort badge `· max` (correct there) and must not trip
    # the assertion; it also shares the navigate hint line when wrapped,
    # so the slice + cache filter excludes it either way.
    lines = body.splitlines()
    try:
        top = next(i for i, ln in enumerate(lines) if "Thinking effort" in ln)
    except StopIteration:
        top = 0
    try:
        bottom = next(i for i, ln in enumerate(lines) if "navigate" in ln and i > top)
    except StopIteration:
        bottom = len(lines)
    rows = [ln for ln in lines[top:bottom] if "cache" not in ln]
    rowtext = "\n".join(rows)
    # A real picker row pairs the level with its description on one line
    # (e.g. `max ... Maximum reasoning`); the footer badge never does.
    if any("max" in ln.replace("·", " ").split() and "Maximum" in ln for ln in rows):
        return False, "thinking: picker lists 'max' for muse-spark (provider 400s it)"
    # Tokenize on whitespace so 'max' inside other words can't false-pass.
    toks = set(rowtext.replace("·", " ").split())
    if "max" in toks:
        return False, "thinking: picker lists 'max' for muse-spark (provider 400s it)"
    for want in ("low", "medium", "high"):
        if want not in toks:
            return False, f"thinking: picker missing expected level '{want}'"
    if "xhigh" not in toks and "minimal" not in toks:
        return False, "thinking: picker missing both xhigh and minimal"

    # Navigate without selecting.
    _, sig_before = sess.state_sig()
    sess.press("down")
    time.sleep(0.8)
    sess.shot("thinking-moved")
    _, sig_after = sess.state_sig()
    if sig_after == sig_before:
        return False, "thinking: down-arrow did not move highlight"
    if not sess.child.isalive():
        return False, "thinking: gray died after arrow keys"

    # Cancel.
    sess.press("esc")
    if not sess.expect_text("\u276f", timeout=15):
        return False, "thinking: Esc did not close the picker"
    sess.shot("thinking-closed")
    _ = before

    # Garbage level: clean single-line error.
    sess.press("esc")
    b2 = sess.text()
    sess.send("/thinking garbage-level-xyz")
    sess.press("enter")
    hit = False
    end = time.time() + 20
    while time.time() < end:
        cur = sess.text()
        if cur != b2 and "unknown level" in cur:
            hit = True
            break
        time.sleep(0.5)
    if not hit:
        return False, "thinking: garbage level gave no 'unknown level' error"
    sess.shot("thinking-garbage")

    if not sess.child.isalive():
        return False, "thinking: gray died before the end"
    return True, "thinking ok: max absent for muse-spark, nav/cancel/garbage"
