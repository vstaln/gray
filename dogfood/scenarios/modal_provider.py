"""Modal matrix: /provider (alias /connect) picker offline.

Fresh home. Boot picker already proves render; this reopens via /provider
after dismissal, then navigate/filter/cancel. Zero LLM calls.
"""
import time

from scenarios.plugin_common import boot_fresh, run_cmd


def run(sess, _ctx):
    if not boot_fresh(sess, "provider-ready"):
        return False, "provider: boot prompt never appeared"

    # Canonical first (single Enter submits). The /provider alias needs
    # double-Enter (first completes to /connect) — asserted below.
    if not run_cmd(sess, "/connect", "Connect a provider", timeout=30):
        return False, "provider: '/connect' did not reopen the picker"
    sess.shot("provider-open")

    _, before = sess.state_sig()
    sess.press("down", "down")
    time.sleep(0.8)
    sess.shot("provider-moved")
    _, after = sess.state_sig()
    if after == before:
        return False, "provider: down-arrow did not move highlight"
    if not sess.child.isalive():
        return False, "provider: gray died after arrow keys"

    sess.send("deep")
    time.sleep(1.0)
    sess.shot("provider-filtered")
    if "DeepSeek" not in sess.text():
        return False, "provider: typing 'deep' did not surface DeepSeek"

    sess.press("esc")
    if not sess.expect_text("\u276f", timeout=15):
        return False, "provider: Esc did not close the picker"
    sess.shot("provider-closed")

    # Alias parity: /provider completes to /connect on first Enter
    # (composer/input/mod.rs popup-fill), second Enter submits.
    sess.press("esc")
    b0 = sess.text()
    sess.send("/provider")
    sess.press("enter")
    time.sleep(1.2)
    if "/connect" not in sess.text():
        return False, "provider: '/provider' did not complete to /connect"
    sess.shot("provider-alias-completed")
    sess.press("enter")
    if not sess.expect_text("Connect a provider", timeout=30):
        # Picker may already be open from the completed text; accept it.
        if "Connect a provider" not in sess.text():
            return False, "provider: alias double-Enter did not open picker"
    sess.shot("provider-alias-open")
    sess.press("esc")
    if not sess.expect_text("\u276f", timeout=15):
        return False, "provider: Esc did not close the alias picker"

    if not sess.child.isalive():
        return False, "provider: gray died before the end"
    _ = b0
    return True, "provider ok: reopen/navigate/filter/cancel + alias double-Enter"
