"""Shared bits for /plugin dogfood scenarios (tour, adversarial, search loop).

Fresh homes have no Gray Index cache and the production index 404s, so
every search would fail fast with `search failed: ...`. Scenarios that
need real search/install behavior serve an empty index over loopback
http (allowed by the https-only fetch rule) and point GRAY_PLUGIN_INDEX
at it. The pi side (npm registry) still uses live networking.
"""

import functools
import http.server
import threading
import time

INDEX_BODY = b'{"schema":1,"generated":"dogfood","plugins":{}}'


def _quiet_handler(workdir):
    class H(http.server.SimpleHTTPRequestHandler):
        def log_message(self, *args):
            pass

    return functools.partial(H, directory=workdir)


def start_index_server(workdir):
    """Write an empty Gray Index into workdir, serve it on loopback. Returns URL."""
    import os

    with open(os.path.join(workdir, "index.json"), "wb") as f:
        f.write(INDEX_BODY)
    srv = http.server.HTTPServer(("127.0.0.1", 0), _quiet_handler(workdir))
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    return f"http://127.0.0.1:{srv.server_port}/index.json"


def boot_fresh(sess, shot_label="booted"):
    """Dismiss the provider picker on a keyless fresh home; wait for the prompt."""
    if not sess.expect_text("Connect a provider", timeout=20):
        return False
    sess.press("esc")
    if not sess.expect_text("\u276f", timeout=15):
        return False
    sess.shot(shot_label)
    return True


def run_cmd(sess, cmd, wants, timeout=50):
    """Send a slash command, wait until the screen changes AND shows a want.

    Returns the matched want, or None on stall (caller FAILs). Settles with
    a short idle wait so follow-up assertions see final output.
    """
    if isinstance(wants, str):
        wants = [wants]
    sess.press("esc")
    before = sess.text()
    sess.send(cmd)
    sess.press("enter")
    hit = None
    end = time.time() + timeout
    while time.time() < end:
        cur = sess.text()
        if cur != before:
            for w in wants:
                if w in cur:
                    hit = w
                    break
            if hit:
                break
        time.sleep(0.5)
    if hit is None:
        return None
    sess.wait_idle(quiet_secs=2, timeout=60)
    return hit
