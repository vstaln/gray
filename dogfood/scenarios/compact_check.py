"""Compaction flow, cheap: /compact command + post-compact recall.

Three bio files hold distinctive facts. Turn 1 loads them (summaries
on screen), /compact compresses, turn 2 asks for one pre-compact fact
(Ada's number 714). PASS requires the recall — the summary preserved it.
NOTE: threshold auto-compact at scale is NOT covered here (too costly);
this tests the explicit /compact command path only.
"""

import os

from scenarios.dfutil import auth_env

BIOS = {
    "ada.txt": ("Ada Lovelace", "714",
                "Ada Lovelace, analytical engines. Favorite number 714. "
                "Keeps a brass sextant on her desk.\n"),
    "grace.txt": ("Grace Hopper", "HOPPER-9",
                  "Grace Hopper, compilers. Her ship is named HOPPER-9. "
                  "Drinks black coffee, no sugar.\n"),
    "alan.txt": ("Alan Turing", "ORLEANS-REINETTE",
                 "Alan Turing, computability. His apple variety is "
                 "ORLEANS-REINETTE. Runs 10 miles every morning.\n"),
}


def extra_env():
    return auth_env()


def seed(gray_home, workdir):
    for name, (_, _, body) in BIOS.items():
        with open(os.path.join(workdir, name), "w") as f:
            f.write(body)


def run(sess, ctx):
    if not sess.expect_text("\u276f", timeout=25):
        return False, "compact: prompt never appeared"
    sess.send("Read ada.txt, grace.txt and alan.txt, then give a one-line "
              "summary of each person.")
    sess.press("enter")
    if not sess.wait_idle(quiet_secs=6, timeout=420):
        return False, "compact: load turn never went idle"
    sess.shot("compact-loaded")
    if "714" not in sess.text():
        return False, "compact: load turn missed Ada's fact"

    import time
    sess.send("/compact")
    sess.press("enter")
    # Confirmation wording varies across builds (inline "compressed
    # context" vs checkpoint-note flow) and can scroll out of the 30-row
    # viewport under a long summary: accept any marker on a CHANGED screen.
    markers = ("compressed context", "Critical Context", "checkpoint",
               "summary", "Summary")
    before = sess.text()
    hit, end = None, time.time() + 300
    while time.time() < end:
        cur = sess.text()
        if cur != before:
            for m in markers:
                if m in cur:
                    hit = m
                    break
            if hit:
                break
        time.sleep(1.0)
    if hit is None:
        return False, "compact: /compact showed no summary marker"
    sess.shot("compact-done")

    sess.send("What was Ada's favorite number? Reply with just the number.")
    sess.press("enter")
    if not sess.wait_idle(quiet_secs=6, timeout=420):
        return False, "compact: recall turn never went idle"
    sess.shot("compact-recalled")
    if "714" not in sess.text():
        return False, "compact: post-compact recall lost the fact"
    return True, "compact: /compact ok, pre-compact fact recalled after"
