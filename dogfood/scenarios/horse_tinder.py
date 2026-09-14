"""Layer 2, real build: Horse Tinder. The dogfood special.

Gray gets a blank workdir and one prompt: build a single-file Horse
Tinder app. Permissions run auto (mirrors -p default; guard still on),
workdir is a throwaway so nothing real is at risk. A human (the
operator's agent) judges the result: screenshots + index.html.
"""
import os

from driver import seed_home

PROMPT = (
    "Build Horse Tinder: a single index.html in the current directory. "
    "Three horse profiles (name, emoji photo, one-line bio), Yes/No buttons "
    "that advance through the deck, and a matches counter. "
    "No external requests, no other files. Just build it."
)


def extra_env():
    return {"GRAY_PERMISSION": "auto"}


def seed(gray_home, workdir):
    seed_home(gray_home)


def run(sess, ctx):
    if not sess.expect_text("❯", timeout=20):
        return False, "horse-tinder: prompt never appeared (seeding broken?)"
    sess.shot("horse-ready")
    sess.send(PROMPT)
    sess.press("enter")
    if not sess.wait_idle(quiet_secs=8, timeout=600):
        return False, "horse-tinder: turn never went idle (hang?)"
    sess.shot("horse-done")
    page = os.path.join(ctx["workdir"], "index.html")
    if not os.path.exists(page):
        return False, "horse-tinder: index.html was never created"
    with open(page) as f:
        html = f.read()
    missing = [w for w in ("Yes", "No", "match") if w.lower() not in html.lower()]
    extras = [f for f in os.listdir(ctx["workdir"]) if f != "index.html"]
    if missing:
        return False, f"horse-tinder: index.html missing {missing}"
    if extras:
        return False, f"horse-tinder: stray files {extras} (single file asked)"
    with open(os.path.join(ctx["run_dir"], "index.html"), "w") as f:
        f.write(html)
    return True, f"horse-tinder: index.html built ({len(html)} bytes), deck + counter present"
