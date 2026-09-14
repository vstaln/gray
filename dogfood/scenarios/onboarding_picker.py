"""Layer 1: fresh-home onboarding picker smoke. No model, no key, offline."""


def run(sess, _ctx):
    if not sess.expect_text("Connect a provider", timeout=15):
        return False, "boot: provider picker never appeared"
    sess.shot("picker-boot")

    # arrows move the highlight without crashing (highlight is
    # reverse-video: compare attribute signatures, not just text)
    _, before = sess.state_sig()
    sess.press("down", "down")
    sess.shot("picker-moved")
    _, after = sess.state_sig()
    if after == before:
        return False, "picker: down-arrow did not move highlight"
    if not sess.child.isalive():
        return False, "picker: gray died after arrow keys"

    # search filter narrows the list
    sess.send("deep")
    sess.shot("picker-filtered")
    if not sess.expect_text("DeepSeek", timeout=5):
        return False, "picker: typing 'deep' did not surface DeepSeek"

    # esc backs out / process survives stray keys at top level
    sess.press("esc")
    sess.ctrl("c")
    if not sess.child.isalive():
        return False, "picker: gray died after esc/ctrl-c"
    sess.shot("picker-survived")
    return True, "picker renders, arrows move, filter narrows, no crash"
