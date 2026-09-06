"""Marketplace backend dogfood: degraded search + failed install records.

Hermetic (no live network): Gray Index + Pi registry point at a closed
loopback port (unreachable advisories), Claude marketplaces at a local dir
with no catalog (unreachable), ClawHub at a loopback fixture server (hits).
Asserts `/plugin search` prints the advisory lines AND still returns other
sources' hits (no hard error), then a failed unknown-spec install gains an
`errors::list` row (lockfile-side check on errors.json). No model, no keys.
"""

import http.server
import json
import os
import threading

from scenarios.plugin_common import boot_fresh, run_cmd

HIT_SLUG = "gifgrep"
HIT_NAME = "arein/gifgrep"
BAD_SPEC = "nope-not-real-xyz"

_state = {}


class _ClawHubFixture(http.server.BaseHTTPRequestHandler):
    """GET /search -> one fixture hit; everything else 404 (the verdicts
    batch POST then degrades to its best-effort empty map)."""

    def do_GET(self):
        if self.path.startswith("/search"):
            body = json.dumps(
                {
                    "results": [
                        {
                            "slug": HIT_SLUG,
                            "displayName": "GifGrep",
                            "summary": "grep gifs fixture",
                            "version": "1.2.3",
                            "ownerHandle": "arein",
                            "official": False,
                        }
                    ]
                }
            ).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
        else:
            self.send_response(404)
            self.end_headers()

    def log_message(self, *args):
        pass


def seed(gray_home, workdir):
    # Claude "empty": a real local dir with no catalog -> fast unreachable
    # (a literally-empty env value would fall back to the live defaults).
    empty = os.path.join(workdir, "empty-market")
    os.makedirs(empty, exist_ok=True)
    _state["claude"] = empty
    srv = http.server.HTTPServer(("127.0.0.1", 0), _ClawHubFixture)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    _state["clawhub"] = f"http://127.0.0.1:{srv.server_port}"


def extra_env():
    return {
        "GRAY_PLUGIN_INDEX": "http://127.0.0.1:9/",
        "GRAY_NPM_REGISTRY": "http://127.0.0.1:9",
        "GRAY_CLAWHUB_BASE": _state["clawhub"],
        "GRAY_CLAUDE_MARKETPLACES": _state["claude"],
    }


def _errors_path(gray_home):
    return os.path.join(gray_home, "errors.json")


def run(sess, ctx):
    if not boot_fresh(sess, "mkt-ready"):
        return False, "backend: boot prompt never appeared"
    if os.path.exists(_errors_path(ctx["gray_home"])):
        return False, "backend: errors.json exists before any failure"

    if not run_cmd(sess, "/plugin search gifgrep", "Gray Index: unreachable"):
        return False, "backend: search did not print the gray advisory"
    text = sess.text()
    for want in (
        "Pi Gallery (preview): unreachable",
        "Claude: unreachable",
        HIT_NAME,
        "[ClawHub]",
    ):
        if want not in text:
            return False, f"backend: degraded search missing {want!r}"
    if "search failed:" in text:
        return False, "backend: search hard-errored instead of degrading"
    sess.shot("mkt-search-degraded")

    if not run_cmd(sess, f"/plugin install {BAD_SPEC}", "install failed:"):
        return False, "backend: unknown-spec install did not fail cleanly"
    sess.shot("mkt-install-failed")

    try:
        with open(_errors_path(ctx["gray_home"])) as f:
            entries = json.load(f)
    except Exception as e:
        return False, f"backend: errors.json unreadable after failed install: {e!r}"
    rows = [e for e in entries if e.get("item") == BAD_SPEC]
    if not rows:
        return False, f"backend: no errors row for {BAD_SPEC!r} ({len(entries)} rows)"
    if not sess.child.isalive():
        return False, "backend: gray died before the end"
    return True, (
        "backend ok: advisory + clawhub-hits degraded search; "
        f"failed install recorded ({rows[0].get('source')}/{rows[0].get('item')})"
    )
