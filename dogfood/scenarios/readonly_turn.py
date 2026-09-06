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
    if "PONG" not in sess.text():
        return False, "chat: exact reply 'PONG' not on screen"
    return True, "chat turn completed, exact reply rendered"
