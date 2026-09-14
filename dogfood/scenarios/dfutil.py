"""Shared helpers for TUI dogfood scenarios.

Fresh-home rule: scenarios NEVER copy the operator's real ~/.gray.
Auth + model come from the environment (OPENCODE_API_KEY/GRAY_API_KEY
are inherited, never printed). GRAY_HOME is the driver's per-run
fresh dir (gitignored runs/).
"""

import glob
import json
import os
import re

ZEN_BASE = "https://opencode.ai/zen/go/v1"
MODEL = "muse-spark-1.3-contributor"

REDACT_RE = re.compile(
    r"(token|secret|key|password)([^ ]{0,3}[:=] ?)[^ ]+",
    re.IGNORECASE,
)
TURN_RE = re.compile(
    r"agent run end: stop=(\S+), usage in=(\d+) out=(\d+) "
    r"cached=(\d+) hit=(\d+)%, (\d+) messages"
)
TOOL_RE = re.compile(r"tool start: (\S+)")


def redact(s):
    return REDACT_RE.sub(r"\1\2<redacted>", str(s))


def auth_env():
    """Env for keyed TUI scenarios. Values flow to the child only."""
    env = {
        "GRAY_BASE_URL": ZEN_BASE,
        "GRAY_MODEL": MODEL,
        # "full" = approval gate never asks (codex :danger-full-access).
        # The destructive-command guard is a separate layer: Deny verdicts
        # fail closed in every mode (see gray-tools/src/shell/guard.rs),
        # so `rm -rf /` etc. stay blocked. "auto" would stall every bash
        # call on an interactive Allow-card the harness can't click through.
        "GRAY_PERMISSION": "full",
    }
    key = os.environ.get("OPENCODE_API_KEY") or os.environ.get("GRAY_API_KEY")
    if key:
        env["GRAY_API_KEY"] = key
    eff = os.environ.get("DOG_EFFORT")
    if eff:
        env["GRAY_THINKING_EFFORT"] = eff
    return env


def parse_gray_log(gray_home):
    """Turn rows + error/tool counters from GRAY_HOME/logs/gray.log."""
    out = {
        "turns": [],  # (stop, in, out, cached, hit, messages)
        "n_429": 0,
        "n_400": 0,
        "tools": {},
        "provider_errors": [],
    }
    path = os.path.join(gray_home, "logs", "gray.log")
    if not os.path.exists(path):
        return out
    with open(path, errors="replace") as f:
        for line in f:
            m = TURN_RE.search(line)
            if m:
                out["turns"].append(
                    (m.group(1), int(m.group(2)), int(m.group(3)),
                     int(m.group(4)), int(m.group(5)), int(m.group(6)))
                )
            if "429" in line or "rate limited" in line.lower():
                out["n_429"] += 1
            if "status 400" in line:
                out["n_400"] += 1
            tm = TOOL_RE.search(line)
            if tm:
                out["tools"][tm.group(1)] = out["tools"].get(tm.group(1), 0) + 1
            if "ERROR" in line and len(out["provider_errors"]) < 5:
                out["provider_errors"].append(redact(line.strip()[:220]))
    return out


def session_usages(gray_home):
    """Per-turn usage dicts from session JSONL entries (file order)."""
    rows = []
    for path in sorted(glob.glob(os.path.join(gray_home, "sessions", "*.jsonl"))):
        with open(path, errors="replace") as f:
            for line in f:
                try:
                    o = json.loads(line)
                except ValueError:
                    continue
                u = o.get("usage")
                if u:
                    rows.append({
                        "file": os.path.basename(path),
                        "in": u.get("input_tokens", 0),
                        "out": u.get("output_tokens", 0),
                        "reasoning": u.get("reasoning_tokens", 0),
                        "cached": u.get("cached_tokens", 0),
                        "read": u.get("cache_read_input_tokens", 0),
                        "write": u.get("cache_write_input_tokens", 0),
                    })
    return rows


def session_ids(gray_home):
    return sorted(
        os.path.splitext(os.path.basename(p))[0]
        for p in glob.glob(os.path.join(gray_home, "sessions", "*.jsonl"))
    )
