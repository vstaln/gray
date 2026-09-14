"""Layer 2: seeded-home chat turn against the real model. Read-only.

seed() copies the operator's real config.json + auth.json into the
temp GRAY_HOME (values never printed, never logged) and drops a
scratch file in the workdir. run() then asks for a fixed reply with
NO tool calls: side-effect free, fast, cheap.
"""
import os

from driver import seed_home

PROMPT = "Reply with exactly: PONG. Do not call any tools."


def seed(gray_home, workdir):
    seed_home(gray_home)
    with open(os.path.join(workdir, "notes.txt"), "w") as f:
        f.write("scratch content for dogfood runs\n")


def run(sess, _ctx):
    if not sess.expect_text("❯", timeout=20):
        return False, "chat: input prompt never appeared (seeding broken?)"
    sess.shot("chat-ready")
    sess.send(PROMPT)
    sess.press("enter")
    if not sess.wait_idle(quiet_secs=5, timeout=180):
        return False, "chat: turn never went idle (hang?)"
    sess.shot("chat-answered")
    # The user card echoes the prompt (which contains PONG), so a bare
    # substring check passes even on provider errors — fail on those first.
    # Any `✗` error card (e.g. `✗ Rate limited`) fails, even alongside `✻ Worked`.
    body = sess.text()
    if "Bad request" in body or "MissingSessionID" in body:
        return False, "chat: provider error on screen (see chat-answered shot)"
    if "✗" in body or "Rate limited" in body:
        return False, "chat: error card on screen (see chat-answered shot)"
    bare = [ln.strip() for ln in body.splitlines()]
    if "PONG" not in bare:
        return False, "chat: exact bare-PONG reply not on screen"
    if "✻ Worked" not in body:
        return False, "chat: no worked receipt on screen"
    return True, "chat turn completed, exact reply rendered"
