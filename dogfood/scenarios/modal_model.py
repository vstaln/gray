"""Modal matrix: /model picker offline. Open, navigate, filter, cancel, garbage.

Fresh home, no keys, no network required. Picker renders from catalog
cache (may be empty offline) — assertions are on chrome, not data:
title, highlight moves, filter narrows, Esc closes, garbage errors cleanly.
"""
import time

from scenarios.plugin_common import boot_fresh, run_cmd


def run(sess, _ctx):
    if not boot_fresh(sess, "model-ready"):
        return False, "model: boot prompt never appeared"

    # Open picker via slash command.
    if not run_cmd(sess, "/model", "Select model", timeout=30):
        return False, "model: '/model' did not open the picker"
    sess.shot("model-open")

    # Navigate: highlight moves (reverse-video signature, not just text).
    _, before = sess.state_sig()
    sess.press("down", "down")
    time.sleep(0.8)
    sess.shot("model-moved")
    _, after = sess.state_sig()
    if after == before:
        return False, "model: down-arrow did not move highlight"
    if not sess.child.isalive():
        return False, "model: gray died after arrow keys"
    sess.press("up")
    time.sleep(0.5)

    # Filter: typing narrows (custom-model row echoes the filter).
    sess.send("zzzqxj-novendor-xyz")
    time.sleep(1.0)
    sess.shot("model-filtered")
    body = sess.text()
    if "zzzqxj-novendor-xyz" not in body and "No models" not in body:
        return False, "model: filter text not reflected in picker"

    # Cancel: Esc closes back to the prompt.
    sess.press("esc")
    if not sess.expect_text("\u276f", timeout=15):
        return False, "model: Esc did not close the picker"
    sess.shot("model-closed")

    # Garbage direct arg: clean error, no crash, no stall.
    if not run_cmd(sess, "/model nope-not-real-xyz-123",
                   ["/model", "unknown model", "no match"], timeout=30):
        return False, "model: garbage '/model <bad>' gave no error"
    sess.shot("model-garbage")

    # Follow-up still sane.
    if not run_cmd(sess, "/help", "/quit", timeout=30):
        return False, "model: follow-up '/help' failed after cancel/garbage"
    if not sess.child.isalive():
        return False, "model: gray died before the end"
    return True, "model ok: open/navigate/filter/cancel/garbage"
