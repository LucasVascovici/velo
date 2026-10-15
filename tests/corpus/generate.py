#!/usr/bin/env python3
"""Generate one golden repository, bundle and expectations file.

Run this with the binaries of a TAGGED RELEASE build, never a newer one: the
whole point of the corpus is bytes written by the released code.  See
README.md in this directory.

    python tests/corpus/generate.py --velo <velo> [--mcp <velo-mcp>] \
        --tag v5.1.2 --out tests/corpus [--synth-v2]

Standard library only.
"""
import argparse
import json
import os
import shutil
import sqlite3
import struct
import subprocess
import sys
import tempfile

ENV_AUTHOR = {"VELO_AUTHOR_NAME": "Corpus Author", "VELO_AUTHOR_EMAIL": "corpus@example.com"}


class Runner:
    def __init__(self, velo, cwd):
        self.velo = velo
        self.cwd = cwd
        self.env = dict(os.environ, **ENV_AUTHOR)

    def run(self, *args, check=True):
        p = subprocess.run([self.velo, *args], cwd=self.cwd, env=self.env,
                           capture_output=True, text=True)
        if check and p.returncode != 0:
            sys.exit("velo %s failed (%d):\n%s%s" % (" ".join(args), p.returncode, p.stdout, p.stderr))
        return p

    def supports(self, cmd):
        return self.run(cmd, "--help", check=False).returncode == 0


def write(root, rel, data):
    path = os.path.join(root, *rel.split("/"))
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "wb") as f:
        f.write(data if isinstance(data, bytes) else data.encode())


def mcp_save(mcp, repo, message, meta):
    """Drive velo-mcp over stdio JSON-RPC (newline-delimited) for one save."""
    reqs = [
        {"jsonrpc": "2.0", "id": 1, "method": "initialize",
         "params": {"protocolVersion": "2025-03-26", "capabilities": {},
                    "clientInfo": {"name": "corpus", "version": "0"}}},
        {"jsonrpc": "2.0", "method": "notifications/initialized"},
        {"jsonrpc": "2.0", "id": 2, "method": "tools/call",
         "params": {"name": "velo_save",
                    "arguments": {"message": message, "tool_call_id": "tc1", "meta": meta}}},
    ]
    stdin = "".join(json.dumps(r) + "\n" for r in reqs)
    p = subprocess.run([mcp, "--repo", repo, "--run", "corpus-run"], input=stdin,
                       capture_output=True, text=True, env=dict(os.environ, **ENV_AUTHOR))
    replies = [json.loads(l) for l in p.stdout.splitlines() if l.strip()]
    last = [r for r in replies if r.get("id") == 2]
    if not last or last[0]["result"].get("isError"):
        sys.exit("velo-mcp save failed: %s %s" % (p.stdout, p.stderr))


def build_history(r, work, mcp):
    """The deterministic script.  Returns a list of what this tag lacked."""
    lacks = []
    r.run("init")
    write(work, "README.txt", "golden corpus\nline two\n")
    write(work, "crlf.txt", b"windows\r\nline\r\nendings\r\n")
    write(work, "bin/blob.bin", bytes(range(256)))
    write(work, "src/a.txt", "alpha\n")
    write(work, "src/deep/b.txt", "bravo\n")
    r.run("save", "initial")
    write(work, "src/a.txt", "alpha\nalpha two\n")
    r.run("save", "second")

    r.run("switch", "feature")
    write(work, "src/feature.txt", "feature work\n")
    write(work, "src/deep/b.txt", "bravo\nfeature line\n")
    r.run("save", "feature work")
    r.run("switch", "main")
    write(work, "README.txt", "golden corpus\nline two\nmain line\n")
    r.run("save", "main work")
    r.run("merge", "feature")
    # A clean merge may already have recorded its snapshot; a save is only
    # needed if the tool left it pending.
    r.run("save", "merge feature", check=False)

    if r.supports("mv"):
        r.run("mv", "src/a.txt", "src/renamed.txt")
        r.run("save", "rename a")
    else:
        lacks.append("mv (no rename edges)")

    if mcp:
        write(work, "src/meta.txt", "metadata\n")
        mcp_save(mcp, work, "save with metadata", {"corpus": {"step": "meta"}})
    else:
        lacks.append("velo-mcp (no app-namespace metadata; only the reserved author namespace)")

    r.run("tag", "v1")

    write(work, "src/wip.txt", "uncommitted\n")
    if r.supports("stash"):
        r.run("stash", "push", "wip")
    else:
        lacks.append("stash")

    write(work, "src/undone.txt", "to be undone\n")
    r.run("save", "to be undone")
    r.run("undo")
    return lacks


def rows(conn, sql):
    return [list(x) for x in conn.execute(sql)]


def has(conn, table):
    return conn.execute("SELECT 1 FROM sqlite_master WHERE type='table' AND name=?",
                        (table,)).fetchone() is not None


def expectations(db_path, tag, lacks):
    conn = sqlite3.connect("file:%s?mode=ro" % db_path.replace("\\", "/"), uri=True)
    out = {"tag": tag, "user_version": conn.execute("PRAGMA user_version").fetchone()[0],
           "lacks": lacks}
    out["snapshots"] = [dict(zip(
        ["hash", "parent_hash", "merge_parent", "message", "branch", "created_at_ms"], x))
        for x in conn.execute(
            "SELECT hash, parent_hash, merge_parent, message, branch, created_at_ms "
            "FROM snapshots ORDER BY created_at_ms, hash")]
    out["file_map"] = {}
    for h, p, fh, m in conn.execute(
            "SELECT snapshot_hash, path, hash, mode FROM file_map ORDER BY snapshot_hash, path"):
        out["file_map"].setdefault(h, []).append({"path": p, "hash": fh, "mode": m})
    out["tags"] = rows(conn, "SELECT name, snapshot_hash FROM tags ORDER BY name")
    out["snapshot_meta"] = rows(conn, "SELECT snapshot_id, namespace, key, value FROM snapshot_meta "
                                      "ORDER BY snapshot_id, namespace, key") if has(conn, "snapshot_meta") else []
    out["renames"] = rows(conn, "SELECT snapshot_hash, from_path, to_path FROM renames "
                                "ORDER BY snapshot_hash, to_path") if has(conn, "renames") else []
    out["trash_ids"] = [x[0] for x in conn.execute("SELECT hash FROM trash ORDER BY hash")] if has(conn, "trash") else []
    out["stash_names"] = [x[0] for x in conn.execute("SELECT name FROM stash ORDER BY name")] if has(conn, "stash") else []
    out["branch_tips"] = rows(conn, "SELECT name, tip FROM branches ORDER BY name") if has(conn, "branches") else []
    conn.close()
    return out


def checkpoint(db_path):
    conn = sqlite3.connect(db_path)
    conn.execute("PRAGMA wal_checkpoint(TRUNCATE)")
    conn.close()
    for suffix in ("-wal", "-shm"):
        try:
            os.remove(db_path + suffix)
        except FileNotFoundError:
            pass


def make_synth_v2(velo, out):
    """No release ever wrote bundle v2, so derive one from a v3 bundle of a
    history with no renames: set the version to 2, drop the trailing renames
    count (FORMAT.md section 9)."""
    with tempfile.TemporaryDirectory() as tmp:
        r = Runner(velo, tmp)
        r.run("init")
        write(tmp, "a.txt", "one\n")
        r.run("save", "first")
        write(tmp, "a.txt", "one\ntwo\n")
        write(tmp, "dir/b.txt", "bee\n")
        r.run("save", "second")
        r.run("tag", "v1")
        bundle = os.path.join(tmp, "..", "synth-src-%d.bundle" % os.getpid())
        r.run("bundle", "create", bundle)
        with open(bundle, "rb") as f:
            data = bytearray(f.read())
        os.remove(bundle)
    assert data[:8] == b"VELOBND2" and struct.unpack_from("<I", data, 8)[0] == 3
    assert data[-4:] == b"\0\0\0\0", "history must have no renames"
    struct.pack_into("<I", data, 8, 2)
    del data[-4:]
    dest = os.path.join(out, "bundles", "synth.v2.bundle")
    os.makedirs(os.path.dirname(dest), exist_ok=True)
    with open(dest, "wb") as f:
        f.write(data)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--velo", required=True)
    ap.add_argument("--mcp")
    ap.add_argument("--tag", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--synth-v2", action="store_true",
                    help="also write bundles/synth.v2.bundle (do this from one tag only)")
    a = ap.parse_args()
    velo = os.path.abspath(a.velo)
    mcp = os.path.abspath(a.mcp) if a.mcp else None
    out = os.path.abspath(a.out)

    with tempfile.TemporaryDirectory() as tmp:
        work = os.path.join(tmp, "work")
        os.makedirs(work)
        r = Runner(velo, work)
        lacks = build_history(r, work, mcp)
        bundle_tmp = os.path.join(tmp, "out.bundle")
        r.run("bundle", "create", bundle_tmp)
        db = os.path.join(work, ".velo", "velo.db")
        checkpoint(db)

        dest = os.path.join(out, "repos", a.tag)
        shutil.rmtree(dest, ignore_errors=True)
        shutil.copytree(work, dest)
        # Read the expectations only after the copy: opening a WAL database
        # re-creates its -wal/-shm files.
        exp = expectations(db, a.tag, lacks)
        for suffix in ("-wal", "-shm"):
            assert not os.path.exists(os.path.join(dest, ".velo", "velo.db" + suffix))
        os.makedirs(os.path.join(out, "bundles"), exist_ok=True)
        shutil.copyfile(bundle_tmp, os.path.join(out, "bundles", a.tag + ".v3.bundle"))
        os.makedirs(os.path.join(out, "expected"), exist_ok=True)
        with open(os.path.join(out, "expected", a.tag + ".json"), "w", newline="\n") as f:
            json.dump(exp, f, indent=1, sort_keys=True)
            f.write("\n")
    if a.synth_v2:
        make_synth_v2(velo, out)
    print("generated", a.tag, "lacks:", lacks or "nothing")


if __name__ == "__main__":
    main()
