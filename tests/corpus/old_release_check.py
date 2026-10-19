#!/usr/bin/env python3
"""Two promises that make 6.0 a major release, checked against an old build.

    old_release_check.py --old <velo 5.x> --new <velo 6.x>

1. An old binary refuses a format v4 repository and says why.
2. The way back works: a v4 repository bundled by the new build and applied
   by the old build into a fresh repository has identical history ids and
   identical file contents at every branch tip.

Standard library only; runs the same on Linux, macOS and Windows.
"""

import argparse
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
FIXTURE = HERE / "repos" / "format-v4"

ENV = dict(os.environ)
ENV.update(
    VELO_AUTHOR_NAME="Corpus",
    VELO_AUTHOR_EMAIL="corpus@example.invalid",
    NO_COLOR="1",
)


def run(exe, args, cwd, check=True):
    p = subprocess.run(
        [str(exe), *args],
        cwd=str(cwd),
        env=ENV,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    if check and p.returncode != 0:
        raise SystemExit(
            f"FAIL: `velo {' '.join(args)}` exited {p.returncode} in {cwd}:\n{p.stdout}"
        )
    return p


def check_refusal(old, tmp):
    work = tmp / "v4-copy"
    shutil.copytree(FIXTURE, work)
    p = run(old, ["status"], work, check=False)
    out = p.stdout
    print(f"old binary on the v4 fixture: exit {p.returncode}\n{out.strip()}")
    if p.returncode == 0:
        raise SystemExit("FAIL: the old binary opened a format v4 repository")
    low = out.lower()
    if "v4" not in low and "version 4" not in low and "format 4" not in low:
        raise SystemExit("FAIL: the refusal does not name format version 4")
    if "newer" not in low and "upgrade" not in low and "too new" not in low:
        raise SystemExit("FAIL: the refusal does not say the repository is newer")
    if "upgrade" not in low:
        print(
            "FINDING: the old binary's refusal names the versions but gives no "
            "upgrade advice; it cannot be changed after release."
        )
    print("ok: refusal asserted")


def commit_ids(exe, repo):
    """Every snapshot id in the repository, one per line of `history --all`."""
    p = run(exe, ["history", "--all", "--oneline", "-n", "100000"], repo)
    ids = []
    for line in p.stdout.splitlines():
        # An optional graph prefix, then the short id as the first hex word.
        for word in line.replace("*", " ").replace("|", " ").split():
            if len(word) >= 7 and all(c in "0123456789abcdef" for c in word):
                ids.append(word)
            break
    return ids


def read_tree(root):
    out = {}
    for f in sorted(root.rglob("*")):
        if f.is_file() and ".velo" not in f.relative_to(root).parts:
            out[f.relative_to(root).as_posix()] = f.read_bytes()
    return out


def branch_names(exe, repo):
    p = run(exe, ["branches"], repo)
    names = []
    for line in p.stdout.splitlines():
        if not line.startswith(" "):
            continue  # the "Branches:" heading
        words = line.replace("*", " ").split()
        if words:
            names.append(words[0])
    return names


def write(path, text):
    # Bytes, so Windows does not turn the line endings into CRLF.
    path.write_bytes(text.encode())


def build_v4_repo(new, repo):
    repo.mkdir()
    run(new, ["init"], repo)
    # Enough saves on main for delta chains between keyframes.
    for i in range(12):
        write(repo / "main.txt", "".join(f"line {n}\n" for n in range(i + 20)))
        write(repo / "big.txt", "".join(f"row {n} v{i // 4}\n" for n in range(300)))
        run(new, ["save", f"main {i}"], repo)
    run(new, ["tag", "v1"], repo)
    run(new, ["switch", "-c", "feature"], repo)
    for i in range(4):
        write(repo / "feature.txt", f"feature {i}\n")
        run(new, ["save", f"feature {i}"], repo)
    run(new, ["switch", "main"], repo)
    write(repo / "main.txt", "main only\n" * 5)
    run(new, ["save", "main diverges"], repo)
    run(new, ["merge", "feature"], repo)
    run(new, ["tag", "v2"], repo)
    # A signature, if the CLI can make one.
    key = repo.parent / "signing.key"
    k = run(new, ["keygen", "--out", str(key)], repo, check=False)
    if k.returncode == 0:
        run(new, ["sign", "--key", str(key)], repo)
        print("signed the tip snapshot")
    else:
        print(f"note: no signature (keygen failed: {k.stdout.strip()})")


def check_way_back(old, new, tmp):
    src = tmp / "src"
    build_v4_repo(new, src)
    bundle = tmp / "all.bundle"
    run(new, ["bundle", "create", str(bundle)], src)
    dst = tmp / "dst"
    dst.mkdir()
    run(old, ["init"], dst)
    run(old, ["bundle", "apply", str(bundle)], dst)

    a, b = commit_ids(new, src), commit_ids(old, dst)
    if not a:
        raise SystemExit("FAIL: could not read any snapshot id from the new log")
    if sorted(a) != sorted(b):
        raise SystemExit(f"FAIL: history ids differ\nnew: {a}\nold: {b}")
    print(f"ok: {len(a)} snapshot ids identical")

    names = branch_names(new, src)
    if sorted(names) != sorted(branch_names(old, dst)):
        raise SystemExit("FAIL: branch lists differ")
    for name in names:
        trees = []
        for exe, repo in ((new, src), (old, dst)):
            run(exe, ["switch", name, "--force"], repo)
            trees.append(read_tree(repo))
        if trees[0] != trees[1]:
            raise SystemExit(f"FAIL: tip contents of branch {name} differ")
        print(f"ok: branch {name} tip contents identical ({len(trees[0])} files)")
    for tag in ("v1", "v2"):
        trees = []
        for exe, repo in ((new, src), (old, dst)):
            run(exe, ["restore", tag, "--force"], repo)
            trees.append(read_tree(repo))
        if trees[0] != trees[1]:
            raise SystemExit(f"FAIL: contents at tag {tag} differ")
        print(f"ok: tag {tag} contents identical")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--old", required=True, type=Path)
    ap.add_argument("--new", required=True, type=Path)
    args = ap.parse_args()
    old, new = args.old.resolve(), args.new.resolve()
    tmp = Path(tempfile.mkdtemp(prefix="velo-old-release-"))
    try:
        check_refusal(old, tmp)
        check_way_back(old, new, tmp)
    finally:
        shutil.rmtree(tmp, ignore_errors=True)
    print("old_release_check: all good")


if __name__ == "__main__":
    sys.exit(main())
