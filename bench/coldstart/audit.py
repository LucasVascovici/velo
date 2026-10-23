#!/usr/bin/env python3
"""Audit a cold-start transcript for reads outside the sandbox.

    audit.py TRANSCRIPT --sandbox DIR

The transcript is plain text or JSONL (every string value is scanned). Exit
status: 1 when the run is INVALID, 0 otherwise (warnings are printed).

Isolation is the whole test and a software sandbox is imperfect, so this audit
is a heuristic over what the transcript shows; it cannot see what the agent
remembers from training. State that limit when reporting results.

The heuristics live in the two lists below, in one place:

* INVALID_NAMES: local planning files an agent must never read. Any mention
  of one of these names makes the run invalid.
* INVALID_PATTERNS: regular expressions for the source tree or tests of a velo
  checkout. A match makes the run invalid.
* Any absolute path under a directory that holds crates/velo-core (a checkout
  root) is invalid, checked against the filesystem.
* Any other absolute path (or ~/ path) outside the sandbox is only a warning:
  agents legitimately touch system paths such as /usr/bin or a temp directory.
"""
import argparse
import json
import os
import re
import sys

INVALID_NAMES = ["ARCHITECTURE.md", "DEVELOPING.md", "CLAUDE.md"]
INVALID_PATTERNS = [
    r"crates[/\\]velo-",
    r"bindings[/\\][^/\\\s\"']+[/\\]src",
    r"bindings[/\\][^/\\\s\"']+[/\\]tests[/\\]",
    r"velo[\w.-]*[/\\]tests[/\\]",
]
PATH_RE = re.compile(r"(?:[A-Za-z]:[\\/]|~/|/)[^\s\"'`<>|*?,;()\[\]{}]+")


def _strings(value):
    if isinstance(value, str):
        yield value
    elif isinstance(value, dict):
        for v in value.values():
            yield from _strings(v)
    elif isinstance(value, list):
        for v in value:
            yield from _strings(v)


def transcript_text(raw):
    """Flatten JSONL to the strings inside it; plain lines pass through."""
    out = []
    for line in raw.splitlines():
        s = line.strip()
        if s.startswith("{") or s.startswith("["):
            try:
                out.extend(_strings(json.loads(s)))
                continue
            except ValueError:
                pass
        out.append(line)
    return "\n".join(out)


def _norm(p):
    return os.path.normcase(os.path.abspath(p)).replace("\\", "/").rstrip("/")


def _in_checkout(path):
    """True when an ancestor of path holds crates/velo-core."""
    cur = os.path.dirname(os.path.abspath(path))
    while True:
        if os.path.isdir(os.path.join(cur, "crates", "velo-core")):
            return True
        parent = os.path.dirname(cur)
        if parent == cur:
            return False
        cur = parent


def audit(text, sandbox):
    """Return (invalid_reasons, warnings), each a list of strings."""
    invalid, warnings = [], []
    sb = _norm(sandbox)
    for name in INVALID_NAMES:
        if re.search(r"(?<![\w-])" + re.escape(name), text):
            invalid.append("mentions " + name)
    for pat in INVALID_PATTERNS:
        for m in sorted({m.group(0) for m in re.finditer(pat, text)}):
            invalid.append("touches %r (pattern %s)" % (m, pat))
    for p in sorted(set(PATH_RE.findall(text))):
        p = p.rstrip(".:")
        if p.startswith("~/"):
            p = os.path.expanduser(p)
        n = _norm(p)
        if n == sb or n.startswith(sb + "/"):
            continue
        if _in_checkout(p):
            invalid.append("path under a velo checkout: " + p)
        else:
            warnings.append("outside sandbox: " + p)
    return invalid, warnings


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("transcript")
    ap.add_argument("--sandbox", required=True)
    a = ap.parse_args(argv)
    with open(a.transcript, encoding="utf-8", errors="replace") as f:
        text = transcript_text(f.read())
    invalid, warnings = audit(text, a.sandbox)
    for w in warnings:
        print("WARNING " + w)
    for r in invalid:
        print("INVALID " + r)
    print("RESULT: " + ("INVALID" if invalid else "valid (%d warnings)" % len(warnings)))
    return 1 if invalid else 0


if __name__ == "__main__":
    sys.exit(main())
