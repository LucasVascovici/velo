#!/usr/bin/env python3
"""Independent check for a cold-start run (stdlib only).

Given the repository an agent produced, run the installed `velo` binary's
`fsck`, `history` and `blame` and print one JSON verdict. The agent's own
account of what it did is never consulted: only the repository is.

    python check.py --repo DIR --velo velo --actor agent-1 [--file PATH ...]

Checks: fsck exits 0; `history --all` lists at least one snapshot; for every
checked file `velo blame` succeeds and the expected actor is named on at least
one of its lines. Without --file, every tracked file except `.veloignore` is
checked. `velo blame` prints no author column, so the actor comes from the
blame data in `velo report --json` (the same attribution, with names).

Exit 0 when every check passes, 1 otherwise.
"""
import argparse
import json
import re
import subprocess
import sys


def run(velo, repo, *args):
    p = subprocess.run([velo, *args], cwd=repo, capture_output=True,
                       text=True, encoding="utf-8", errors="replace")
    return p.returncode, p.stdout, p.stderr


def check(repo, velo, actor, files=None):
    verdict = {"fsck_ok": False, "history_count": 0, "history_ok": False,
               "actor": actor, "files": {}, "blame_actor_ok": False,
               "errors": [], "ok": False}

    code, out, err = run(velo, repo, "fsck")
    verdict["fsck_ok"] = code == 0
    if code != 0:
        verdict["errors"].append("fsck exited %d: %s" % (code, (out + err).strip()[-300:]))

    code, out, err = run(velo, repo, "history", "--all", "--oneline")
    if code == 0:
        verdict["history_count"] = len(re.findall(r"^\*", out, re.M))
    else:
        verdict["errors"].append("history exited %d: %s" % (code, err.strip()[-300:]))
    verdict["history_ok"] = verdict["history_count"] >= 1

    code, out, err = run(velo, repo, "report", "--json", "--no-fsck")
    authors = {}
    if code == 0:
        try:
            for f in json.loads(out).get("files", []):
                authors[f["path"]] = {(l.get("author") or {}).get("name")
                                      for l in f.get("lines", [])}
        except (ValueError, KeyError, TypeError) as e:
            verdict["errors"].append("report json unreadable: %s" % e)
    else:
        verdict["errors"].append("report exited %d: %s" % (code, err.strip()[-300:]))

    targets = files or sorted(p for p in authors if p != ".veloignore")
    if not targets:
        verdict["errors"].append("no files to check")
    for path in targets:
        bcode, _, berr = run(velo, repo, "blame", path)
        named = actor in authors.get(path, set())
        verdict["files"][path] = {"blame_ok": bcode == 0, "actor_named": named}
        if bcode != 0:
            verdict["errors"].append("blame %s exited %d: %s" % (path, bcode, berr.strip()[-200:]))
    verdict["blame_actor_ok"] = bool(targets) and all(
        v["blame_ok"] and v["actor_named"] for v in verdict["files"].values())

    verdict["ok"] = verdict["fsck_ok"] and verdict["history_ok"] and verdict["blame_actor_ok"]
    return verdict


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--repo", required=True)
    ap.add_argument("--velo", required=True)
    ap.add_argument("--actor", required=True)
    ap.add_argument("--file", action="append", help="file to check; repeatable")
    a = ap.parse_args(argv)
    try:
        v = check(a.repo, a.velo, a.actor, a.file)
    except OSError as e:
        v = {"ok": False, "errors": [str(e)]}
    print(json.dumps(v, indent=2))
    return 0 if v["ok"] else 1


if __name__ == "__main__":
    sys.exit(main())
