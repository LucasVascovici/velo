#!/usr/bin/env python3
"""Independent checker for a cold-start run (stdlib only).

Shells out to an installed `velo` and asserts, for a repository directory:

  1. `velo fsck` exits zero (the repository is clean),
  2. `velo history --oneline` shows at least one snapshot,
  3. `velo blame <file>` names the expected actor on at least one line.

The checker never imports velo and never trusts what the agent printed; it
only reads what the repository says about itself.

Usage:
  python check.py REPO --actor NAME --file PATH [--velo PATH_TO_VELO]
"""
import argparse
import os
import re
import shutil
import subprocess
import sys

# A `--oneline` history row: optional graph glyphs, then a hex snapshot id.
SNAPSHOT_ROW = re.compile(r"^[*|/\\ ]*[0-9a-f]{8,}\b")


def run(velo, repo, *args):
    """Run `velo <args>` inside `repo`; return (exit code, combined output)."""
    try:
        p = subprocess.run(
            [velo, *args],
            cwd=repo,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=120,
        )
    except (OSError, subprocess.TimeoutExpired) as e:
        return 127, str(e)
    return p.returncode, p.stdout


def main(argv=None):
    ap = argparse.ArgumentParser(
        description="Check a cold-start repository: fsck clean, history "
        "non-empty, blame names the expected actor."
    )
    ap.add_argument("repo", help="repository directory the agent worked in")
    ap.add_argument("--actor", required=True, help="expected actor name in blame")
    ap.add_argument(
        "--file", required=True, help="a file the task produced, relative to the repo root"
    )
    ap.add_argument("--velo", default=None, help="path to the velo binary (default: first on PATH)")
    args = ap.parse_args(argv)

    velo = args.velo or shutil.which("velo")
    if not velo:
        print("FAIL  velo not found on PATH (use --velo)")
        return 2
    if not os.path.isdir(os.path.join(args.repo, ".velo")):
        print("FAIL  %s has no .velo directory" % args.repo)
        return 2

    results = []

    code, _ = run(velo, args.repo, "fsck")
    results.append(("fsck clean", code == 0, "exit %d" % code if code else ""))

    code, out = run(velo, args.repo, "history", "--oneline")
    snaps = [l for l in out.splitlines() if SNAPSHOT_ROW.match(l)]
    results.append(
        ("history non-empty", code == 0 and len(snaps) > 0, "%d snapshot(s)" % len(snaps))
    )

    code, out = run(velo, args.repo, "blame", args.file)
    named = code == 0 and any(args.actor in l for l in out.splitlines())
    detail = "" if named else ("exit %d" % code if code else "actor not in blame output")
    results.append(("blame names %r on %s" % (args.actor, args.file), named, detail))

    for name, ok, detail in results:
        print("%s  %s%s" % ("PASS" if ok else "FAIL", name, "  (%s)" % detail if detail else ""))
    ok_all = all(ok for _, ok, _ in results)
    print("RESULT: %s" % ("PASS" if ok_all else "FAIL"))
    return 0 if ok_all else 1


if __name__ == "__main__":
    sys.exit(main())
