#!/usr/bin/env python3
"""Independent checker for a cold-start run (stdlib only).

Shells out to an installed `velody` and asserts, for a repository directory:

  1. `velody fsck` exits zero (the repository is clean),
  2. `velody history --all --oneline` shows at least one snapshot,
  3. `velody blame <file>` (at `--at BRANCH` when given) names the expected
     actor on at least one line,
  4. with `--run ID`, `velody history --all --where mcp:run=ID` finds at least
     one snapshot: the MCP server records the run id as metadata, not as the
     author, so blame alone cannot show it.

History is listed across all branches because a repository written through
the library has no checked-out position, and the task may publish on a branch
of its own (the config registry does).

The checker never imports velody and never trusts what the agent printed; it
only reads what the repository says about itself.

Usage:
  python check.py REPO --actor NAME --file PATH [--at BRANCH] [--run ID] [--velody PATH_TO_VELO]
"""
import argparse
import os
import re
import shutil
import subprocess
import sys

# A `--oneline` history row: optional graph glyphs, then a hex snapshot id.
SNAPSHOT_ROW = re.compile(r"^[*|/\\ ]*[0-9a-f]{8,}\b")


def run(velody, repo, *args):
    """Run `velody <args>` inside `repo`; return (exit code, combined output)."""
    try:
        p = subprocess.run(
            [velody, *args],
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
    ap.add_argument("--at", default=None, help="branch or snapshot to blame at (default: the checked-out branch)")
    ap.add_argument("--run", default=None, help="MCP run id that must be recorded on at least one snapshot")
    ap.add_argument("--velody", default=None, help="path to the velody binary (default: first on PATH)")
    args = ap.parse_args(argv)

    velody = args.velody or shutil.which("velody")
    if not velody:
        print("FAIL  velody not found on PATH (use --velody)")
        return 2
    if not os.path.isdir(os.path.join(args.repo, ".velo")):
        print("FAIL  %s has no .velo directory" % args.repo)
        return 2

    results = []

    code, _ = run(velody, args.repo, "fsck")
    results.append(("fsck clean", code == 0, "exit %d" % code if code else ""))

    code, out = run(velody, args.repo, "history", "--all", "--oneline")
    snaps = [l for l in out.splitlines() if SNAPSHOT_ROW.match(l)]
    results.append(
        ("history non-empty", code == 0 and len(snaps) > 0, "%d snapshot(s)" % len(snaps))
    )

    blame_args = ["blame", args.file] + (["--at", args.at] if args.at else [])
    code, out = run(velody, args.repo, *blame_args)
    named = code == 0 and any(args.actor in l for l in out.splitlines())
    detail = "" if named else ("exit %d" % code if code else "actor not in blame output")
    results.append(("blame names %r on %s" % (args.actor, args.file), named, detail))

    if args.run:
        code, out = run(velody, args.repo, "history", "--all", "--oneline", "--where", "mcp:run=" + args.run)
        snaps = [l for l in out.splitlines() if SNAPSHOT_ROW.match(l)]
        results.append(
            ("run %r recorded" % args.run, code == 0 and len(snaps) > 0, "%d snapshot(s)" % len(snaps))
        )

    for name, ok, detail in results:
        print("%s  %s%s" % ("PASS" if ok else "FAIL", name, "  (%s)" % detail if detail else ""))
    ok_all = all(ok for _, ok, _ in results)
    print("RESULT: %s" % ("PASS" if ok_all else "FAIL"))
    return 0 if ok_all else 1


if __name__ == "__main__":
    sys.exit(main())
