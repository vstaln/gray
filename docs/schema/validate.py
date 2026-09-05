#!/usr/bin/env python3
"""CI conformance: boot plugins/echo/echo.sh, validate its manifest.

Stdlib only (no jsonschema dep): structural check against
manifest.v1.json's required keys, tool entry shapes, and enums.
Full draft-07 validation is deferred to a later pass; this catches
drift between the reference plugin, the schema, and Manifest::from_result.
Usage: python3 docs/schema/validate.py
"""

import json
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent.parent
SCHEMA = json.loads((Path(__file__).parent / "manifest.v1.json").read_text())
ECHO = ROOT / "plugins" / "echo" / "echo.sh"

CAPABILITIES = {"exec", "http", "session", "ui"}
HOOKS = {"prompt/context", "tool/before", "turn/end"}


def fail(msg):
    print(f"FAIL {msg}")
    sys.exit(1)


def main():
    proc = subprocess.Popen(
        [str(ECHO)], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True
    )
    try:
        out, _ = proc.communicate('{"id":1,"method":"plugin/manifest"}\n', timeout=30)
    except subprocess.TimeoutExpired:
        proc.kill()
        fail("echo.sh manifest handshake timed out")
    lines = [ln for ln in out.splitlines() if ln.strip()]
    if not lines:
        fail("echo.sh printed nothing on plugin/manifest")
    try:
        reply = json.loads(lines[0])
    except json.JSONDecodeError as e:
        fail(f"first line is not JSON: {e}")
    if reply.get("id") != 1 or "result" not in reply:
        fail(f"bad reply envelope: {lines[0][:200]}")
    m = reply["result"]

    for key in SCHEMA["required"]:
        if key not in m:
            fail(f"manifest missing required key {key!r}")
    if not m["name"] or not m["version"]:
        fail("manifest name/version must be non-empty")
    for t in m["tools"]:
        if isinstance(t, str):
            continue  # pre-v1 bare name still parses
        if not isinstance(t, dict) or not t.get("name"):
            fail(f"bad tool entry: {t!r}")
    for c in m.get("commands", []) + m.get("subcommands", []):
        if not c.startswith("/"):
            fail(f"command {c!r} must start with /")
    for h in m.get("hooks", []):
        if h not in HOOKS:
            fail(f"unknown hook {h!r}")
    for c in m.get("capabilities", []):
        if c not in CAPABILITIES:
            fail(f"unknown capability {c!r}")
    if "protocol" in m and m["protocol"] != "1.1":
        fail(f"unknown protocol {m['protocol']!r}")
    print(f"PASS echo manifest: name={m['name']} tools={len(m['tools'])} protocol={m.get('protocol')}")


if __name__ == "__main__":
    main()
