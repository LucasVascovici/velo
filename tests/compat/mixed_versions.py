#!/usr/bin/env python3
"""Mixed-version compatibility: a 5.x client with a 6.x peer, and the reverse.

    mixed_versions.py --old <velody 5.x> --new <velody 6.x>

Four cells, each printed as a PASS or FAIL line:

  * HTTP, old client -> new `serve-http`; HTTP, new client -> old `serve-http`.
    The wire protocol is unchanged, so both must work. The client clones,
    saves, pushes and fetches; afterwards the server's history ids equal the
    client's and `fsck` is green on both sides.
  * Path remote, new client -> old (format v3) remote: works, and the remote
    is migrated in place to v4 (the old binary can no longer read it).
  * Path remote, old client -> new (format v4) remote: refused by design. A
    path remote is opened in-process, so the old binary meets the v4 schema
    itself and must refuse with a typed error, leaving the remote untouched.

The refusal of a v4 repository in a working directory and the bundle round
trip are tests/corpus/old_release_check.py. Standard library only.
"""

import argparse
import hashlib
import os
import socket
import subprocess
import sys
import tempfile
import time
import urllib.request
from pathlib import Path

TOKEN = "mixed-versions-token"

ENV = dict(os.environ)
ENV.update(
    VELODY_AUTHOR_NAME="Compat",
    VELODY_AUTHOR_EMAIL="compat@example.invalid",
    NO_COLOR="1",
)
for _k in [k for k in ENV if k.startswith("VELODY_TOKEN") or k == "VELODY_SERVE_TOKEN"]:
    del ENV[_k]


class Fail(Exception):
    pass


def run(exe, args, cwd, check=True, env=None):
    p = subprocess.run(
        [str(exe), *args],
        cwd=str(cwd),
        env=env or ENV,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        encoding="utf-8",
        errors="replace",
        timeout=120,
    )
    if check and p.returncode != 0:
        raise Fail(f"`velody {' '.join(args)}` exited {p.returncode} in {cwd}:\n{p.stdout}")
    return p


def write(path, text):
    path.write_bytes(text.encode())  # bytes: no CRLF translation on Windows


def commit_ids(exe, repo):
    """Every snapshot id in the repository, from `history --all --oneline`."""
    p = run(exe, ["history", "--all", "--oneline", "-n", "100000"], repo)
    ids = []
    for line in p.stdout.splitlines():
        for word in line.replace("*", " ").replace("|", " ").split():
            if len(word) >= 7 and all(c in "0123456789abcdef" for c in word):
                ids.append(word)
            break
    return ids


def make_repo(exe, repo):
    repo.mkdir()
    run(exe, ["init"], repo)
    write(repo / "a.txt", "".join(f"line {n}\n" for n in range(40)))
    run(exe, ["save", "server first"], repo)
    write(repo / "a.txt", "".join(f"line {n}\n" for n in range(45)))
    run(exe, ["save", "server second"], repo)


def tree_digest(root):
    h = hashlib.sha256()
    for f in sorted(Path(root).rglob("*")):
        if f.is_file() and not f.name.endswith(("-wal", "-shm", ".lock")):
            h.update(f.relative_to(root).as_posix().encode())
            h.update(f.read_bytes())
    return h.hexdigest()


def assert_same_history(client_exe, client, server_exe, server):
    a, b = commit_ids(client_exe, client), commit_ids(server_exe, server)
    if not a:
        raise Fail("no snapshot ids read from the client log")
    if sorted(a) != sorted(b):
        raise Fail(f"history ids differ: client {a} vs server {b}")
    for exe, repo in ((client_exe, client), (server_exe, server)):
        run(exe, ["fsck"], repo)


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def cell_http(client_exe, server_exe, tmp):
    server = tmp / "server"
    make_repo(server_exe, server)
    port = free_port()
    env = dict(ENV, VELODY_SERVE_TOKEN=TOKEN)
    proc = subprocess.Popen(
        [str(server_exe), "serve-http", str(server), "--listen", f"127.0.0.1:{port}"],
        cwd=str(tmp),
        env=env,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    try:
        deadline = time.time() + 30
        while True:
            if proc.poll() is not None:
                raise Fail(f"serve-http exited early with {proc.returncode}")
            try:
                socket.create_connection(("127.0.0.1", port), timeout=1).close()
                break
            except OSError:
                if time.time() > deadline:
                    raise Fail("serve-http did not become ready")
                time.sleep(0.1)
        cenv = dict(ENV, VELODY_TOKEN=TOKEN)
        url = f"http://127.0.0.1:{port}/"
        client = tmp / "client"
        run(client_exe, ["clone", url, str(client)], tmp, env=cenv)
        write(client / "b.txt", "client work\n")
        run(client_exe, ["save", "client save"], client, env=cenv)
        run(client_exe, ["push"], client, env=cenv)
        # A second clone fetches what the first pushed.
        other = tmp / "other"
        run(client_exe, ["clone", url, str(other)], tmp, env=cenv)
        write(other / "c.txt", "other work\n")
        run(client_exe, ["save", "other save"], other, env=cenv)
        run(client_exe, ["push"], other, env=cenv)
        run(client_exe, ["pull"], client, env=cenv)
        run(client_exe, ["fetch"], client, env=cenv)
        assert_same_history(client_exe, client, server_exe, server)
    finally:
        proc.kill()
        proc.wait()


def cell_path_new_to_old(old, new, tmp):
    """A new client against a v3 path remote made by the old binary."""
    remote = tmp / "remote"
    make_repo(old, remote)
    client = tmp / "client"
    run(new, ["clone", str(remote), str(client)], tmp)
    write(client / "b.txt", "client work\n")
    run(new, ["save", "client save"], client)
    run(new, ["push"], client)
    run(new, ["fetch"], client)
    # The new binary opened the remote in-process; check it with the new one
    # (it may have migrated the remote in place) and report which.
    assert_same_history(new, client, new, remote)
    # Opening a v3 path remote migrates it in place to v4, so the old binary
    # no longer reads it. That is the intended behaviour, recorded in the
    # README, and asserted so a silent change is noticed.
    if run(old, ["fsck"], remote, check=False).returncode == 0:
        raise Fail("the v3 remote was not migrated in place by the new client")


def cell_path_old_to_new(old, new, tmp):
    """An old client against a v4 path remote: refused, remote untouched."""
    remote = tmp / "remote"
    make_repo(new, remote)
    before = tree_digest(remote)
    client = tmp / "client"
    p = run(old, ["clone", str(remote), str(client)], tmp, check=False)
    out = p.stdout.lower()
    if p.returncode == 0:
        raise Fail("the old client opened a v4 path remote")
    if not any(w in out for w in ("v4", "version 4", "format 4", "schema", "newer")):
        raise Fail(f"the refusal does not name the schema/version:\n{p.stdout}")
    if tree_digest(remote) != before:
        raise Fail("the refused clone changed the remote")
    run(new, ["fsck"], remote)
    if tree_digest(remote) != before:
        raise Fail("fsck by the new binary changed the remote")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--old", required=True)
    ap.add_argument("--new", required=True)
    a = ap.parse_args()
    old, new = Path(a.old).resolve(), Path(a.new).resolve()

    cells = [
        ("HTTP    old client -> new server", lambda t: cell_http(old, new, t)),
        ("HTTP    new client -> old server", lambda t: cell_http(new, old, t)),
        ("path    old client -> new (v4) remote: refused", lambda t: cell_path_old_to_new(old, new, t)),
        ("path    new client -> old (v3) remote", lambda t: cell_path_new_to_old(old, new, t)),
    ]
    failed = 0
    for name, fn in cells:
        with tempfile.TemporaryDirectory(prefix="velody-mixed-") as d:
            try:
                fn(Path(d))
                print(f"PASS {name}")
            except Fail as e:
                failed += 1
                print(f"FAIL {name}\n{e}")
            except Exception as e:  # a crash is a failure, never a silent pass
                failed += 1
                print(f"FAIL {name}\n{type(e).__name__}: {e}")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
