#!/usr/bin/env python3
"""Build the v4 spec fixture: a repository written straight from docs/FORMAT.md 11.5.

This script shares no code with velo. It uses the standard library plus two
vendored pure-Python modules beside it (`blake3_ref.py`, `ed25519_ref.py`).
It computes snapshot ids with the section 4.2 recipe itself, hand-builds
Zstd frames from raw blocks, and writes `repo/` and `expected.json` here.

    python generate_v4.py            rewrite repo/ and expected.json
    python generate_v4.py --selftest recompute every v5.1.2 golden id with the
                                     section 4.2 recipe, then rebuild the
                                     fixture in a temp dir and require it to be
                                     byte-identical to the checked-in one

Regenerate the checked-in copy only when FORMAT.md 11.5 changes. Never run
velo on it: open a copy.

The history uses K = 4 (settings.keyframe_interval), so a chain is at most 3
delta links:

    main:    s1 KF -> s2 d1 -> s3 d2 -> s4 d3 -> s5 KF -> m d1 (merge, base s5)
                                                          -> t1 d2 -> t2 d3   (t1, t2 undone: trash)
    feature: s3 -> f1 d3 -> f2 KF   (forked at s3, chain continues from 2)
"""
import json
import os
import shutil
import sqlite3
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
from blake3_ref import blake3_hex  # noqa: E402
import ed25519_ref  # noqa: E402

CORPUS = os.path.dirname(HERE)
K = 4
BASE_MS = 1_800_000_000_000

# The pre-existing tables, copied from crates/velo-core/src/db.rs SCHEMA.
SCHEMA_V3 = """
    CREATE TABLE IF NOT EXISTS snapshots (
        hash          TEXT PRIMARY KEY,
        message       TEXT NOT NULL,
        branch        TEXT NOT NULL,
        parent_hash   TEXT NOT NULL DEFAULT '',
        merge_parent  TEXT NOT NULL DEFAULT '',
        created_at_ms INTEGER NOT NULL DEFAULT 0
    );
    CREATE TABLE IF NOT EXISTS file_map (
        snapshot_hash TEXT NOT NULL,
        path          TEXT NOT NULL,
        hash          TEXT NOT NULL,
        mode          INTEGER NOT NULL DEFAULT 0
    );
    -- App-namespaced snapshot metadata. Part of snapshot identity, so rows are
    -- immutable: changing one would change the id it belongs to.
    CREATE TABLE IF NOT EXISTS snapshot_meta (
        snapshot_id TEXT NOT NULL,
        namespace   TEXT NOT NULL,
        key         TEXT NOT NULL,
        value       TEXT NOT NULL,
        PRIMARY KEY (snapshot_id, namespace, key)
    );
    CREATE TABLE IF NOT EXISTS tags (
        name          TEXT PRIMARY KEY,
        snapshot_hash TEXT NOT NULL
    );
    CREATE TABLE IF NOT EXISTS trash (
        hash          TEXT PRIMARY KEY,
        message       TEXT NOT NULL,
        branch        TEXT NOT NULL,
        parent_hash   TEXT NOT NULL DEFAULT '',
        merge_parent  TEXT NOT NULL DEFAULT '',
        created_at_ms INTEGER NOT NULL DEFAULT 0,
        deleted_at_ms INTEGER NOT NULL DEFAULT 0
    );
    -- Tags removed by `undo`, held so `redo` can restore them.
    CREATE TABLE IF NOT EXISTS trash_tags (
        name          TEXT PRIMARY KEY,
        snapshot_hash TEXT NOT NULL
    );
    CREATE TABLE IF NOT EXISTS index_cache (
        path     TEXT PRIMARY KEY,
        mtime_ns INTEGER NOT NULL,
        size     INTEGER NOT NULL,
        hash     TEXT    NOT NULL
    );
    -- Named stash shelves.
    -- Each shelf stores the dirty state of the working tree at stash time.
    -- The snapshot_hash references a regular snapshot row (branch = '_stash').
    CREATE TABLE IF NOT EXISTS stash (
        id            INTEGER PRIMARY KEY AUTOINCREMENT,
        name          TEXT NOT NULL UNIQUE,
        snapshot_hash TEXT NOT NULL,
        branch        TEXT NOT NULL,
        parent_hash   TEXT NOT NULL DEFAULT '',
        created_at_ms INTEGER NOT NULL DEFAULT 0
    );
    -- Files with active merge conflicts.
    -- The three object hashes are all we need to recompute hunks on demand.
    CREATE TABLE IF NOT EXISTS conflict_files (
        path          TEXT PRIMARY KEY,
        ancestor_hash TEXT NOT NULL,
        our_hash      TEXT NOT NULL,
        their_hash    TEXT NOT NULL
    );

    -- Per-hunk resolution decisions.
    -- decision: 'ours' | 'theirs' | 'both_ours' | 'both_theirs' | 'manual'
    -- manual_content: newline-delimited lines (only set when decision='manual')
    CREATE TABLE IF NOT EXISTS hunk_decisions (
        file_path      TEXT    NOT NULL,
        hunk_id        INTEGER NOT NULL,
        decision       TEXT    NOT NULL,
        manual_content TEXT,
        PRIMARY KEY (file_path, hunk_id)
    );

    -- Branches as first-class refs, so a branch created but not yet committed to
    -- still exists. `tip = ''` means the branch exists but is unborn.
    CREATE TABLE IF NOT EXISTS branches (
        name TEXT PRIMARY KEY,
        tip  TEXT NOT NULL DEFAULT ''
    );

    CREATE TABLE IF NOT EXISTS remotes (
        name TEXT PRIMARY KEY,
        url  TEXT NOT NULL
    );
    CREATE TABLE IF NOT EXISTS remote_refs (
        remote TEXT NOT NULL,
        branch TEXT NOT NULL,
        hash   TEXT NOT NULL,
        PRIMARY KEY (remote, branch)
    );

    -- Paths a snapshot moved.  A rename cannot be recovered from `file_map`,
    -- which holds whole trees: there a move is a delete and an add, and telling
    -- those apart afterwards is guesswork.  So it is recorded when it happens,
    -- by the caller who knows it happened.
    --
    -- Not part of a snapshot's identity, deliberately.  Identity answers what
    -- a tree is, and two identical trees are the same snapshot however each was
    -- arrived at.  `fsck` checks the edges structurally instead.
    CREATE TABLE IF NOT EXISTS renames (
        snapshot_hash TEXT NOT NULL,
        from_path     TEXT NOT NULL,
        to_path       TEXT NOT NULL,
        PRIMARY KEY (snapshot_hash, to_path)
    );

    -- Moves made in the working tree, waiting for the save that will record
    -- them.  Velo has no staging area because the disk is the snapshot; a
    -- rename is the one fact the disk cannot hold, since after the move the
    -- tree is identical to a delete plus an add.  Keyed by destination: two
    -- files cannot arrive at one path.
    CREATE TABLE IF NOT EXISTS pending_renames (
        from_path TEXT NOT NULL,
        to_path   TEXT PRIMARY KEY
    );

    -- Ids that compaction removed or re-minted, mapped to what replaced them
    -- (docs/FORMAT.md 11.3).  Local only: not part of identity, not in bundles,
    -- and never collected by `gc`, since the whole point is to outlive the rows
    -- it describes.
    CREATE TABLE IF NOT EXISTS compactions (
        old_hash        TEXT PRIMARY KEY,
        new_hash        TEXT NOT NULL,
        compacted_at_ms INTEGER NOT NULL
    );

    -- Signatures over snapshot ids (docs/FORMAT.md 11.1).  Outside identity: a
    -- signature never changes a snapshot's id, and a countersignature is just
    -- another row.  Keys and signatures are lowercase hex.
    CREATE TABLE IF NOT EXISTS signatures (
        snapshot_hash TEXT NOT NULL,
        algorithm     TEXT NOT NULL,
        public_key    TEXT NOT NULL,
        signature     TEXT NOT NULL,
        PRIMARY KEY (snapshot_hash, public_key)
    );

    -- Repository settings. `objects` = 'database' puts object content in the
    -- `objects` table below; no row means files under .velo/objects.
    CREATE TABLE IF NOT EXISTS settings (
        key   TEXT PRIMARY KEY,
        value TEXT NOT NULL
    );

    -- Object content when the repository's object location is the database:
    -- one zstd frame of the full content per object (docs/FORMAT.md 7.2).
    CREATE TABLE IF NOT EXISTS objects (
        hash TEXT PRIMARY KEY,
        data BLOB NOT NULL
    );

    CREATE INDEX IF NOT EXISTS idx_filemap_snap  ON file_map (snapshot_hash);
    CREATE INDEX IF NOT EXISTS idx_filemap_path  ON file_map (path);
    CREATE INDEX IF NOT EXISTS idx_snap_branch   ON snapshots (branch, created_at_ms);
    CREATE INDEX IF NOT EXISTS idx_trash_branch  ON trash (branch, deleted_at_ms);
    CREATE INDEX IF NOT EXISTS idx_stash_name    ON stash (name);
    CREATE INDEX IF NOT EXISTS idx_meta_snap     ON snapshot_meta (snapshot_id);
    CREATE INDEX IF NOT EXISTS idx_meta_lookup   ON snapshot_meta (namespace, key, value);
    -- The backward walk asks 'which snapshot renamed something to this path',
    -- so `to_path` leads.  The primary key already covers the per-snapshot
    -- lookup that follows.
    CREATE INDEX IF NOT EXISTS idx_renames_to     ON renames (to_path);
"""

# The v4 tables, FORMAT.md 11.5 "Storage schema (v4)".
SCHEMA_V4 = """
CREATE TABLE IF NOT EXISTS delta_base (
    snapshot_hash TEXT PRIMARY KEY,
    base_hash     TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS file_delta (
    snapshot_hash TEXT NOT NULL,
    path          TEXT NOT NULL,
    hash          TEXT NOT NULL,
    mode          INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (snapshot_hash, path)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS idx_delta_base_base ON delta_base (base_hash);
CREATE INDEX IF NOT EXISTS idx_file_delta_path ON file_delta (path);
"""


# ---------------------------------------------------------------- identity

def snapshot_id(tree, parent, merge, message, time_ms, meta):
    """FORMAT.md 4.2. `tree` is {path: (hash, mode)}, `meta` a list of
    (namespace, key, value)."""
    out = b"velo-snapshot-v2\n"
    for path in sorted(tree, key=lambda p: p.encode("utf-8")):
        h, mode = tree[path]
        out += path.encode() + b"\0" + h.encode() + b"\0" + str(mode).encode() + b"\n"
    out += b"parent\0" + parent.encode()
    out += b"\nmerge\0" + merge.encode()
    out += b"\nmessage\0" + message.encode()
    out += b"\ntime\0" + str(time_ms).encode()
    out += b"\nmeta\0"
    for ns, key, value in sorted(meta, key=lambda t: (t[0].encode(), t[1].encode())):
        out += ns.encode() + b"\0" + key.encode() + b"\0" + value.encode() + b"\n"
    return blake3_hex(out)


# ------------------------------------------------------------------ objects

def normalise(content: bytes) -> bytes:
    """FORMAT.md 2.1."""
    return content if b"\0" in content else content.replace(b"\r", b"")


def zstd_raw_frame(content: bytes) -> bytes:
    """A valid Zstd frame made only of raw (uncompressed) blocks."""
    n = len(content)
    out = bytearray(b"\x28\xb5\x2f\xfd")
    if n < 256:
        flag, fcs = 0, n.to_bytes(1, "little")
    elif n < 65536 + 256:
        flag, fcs = 1, (n - 256).to_bytes(2, "little")
    elif n < 2 ** 32:
        flag, fcs = 2, n.to_bytes(4, "little")
    else:
        flag, fcs = 3, n.to_bytes(8, "little")
    out.append((flag << 6) | (1 << 5))  # single segment, no checksum, no dictionary
    out += fcs
    pos = 0
    while True:
        size = min(len(content) - pos, 128 * 1024)
        last = pos + size >= len(content)
        header = (1 if last else 0) | (0 << 1) | (size << 3)  # block type 0 = raw
        out += header.to_bytes(3, "little")
        out += content[pos:pos + size]
        pos += size
        if last:
            break
    return bytes(out)


# ------------------------------------------------------------------ history

def files_s1():
    return {
        "README.txt": (b"# demo\n", 0),
        "bin/blob.bin": (bytes([0, 1, 2, 3, 255, 254, 0, 7]), 0),
        "crlf.txt": (b"line1\r\nline2\r\n", 0),
        "docs/notes.txt": (b"notes\n", 0),
        "run.sh": (b"#!/bin/sh\necho hi\n", 1),
        "src/a.txt": (b"alpha\n", 0),
        "src/b.txt": (b"bravo\n", 0),
    }


def build_history():
    """Return the snapshots in creation order.

    Every snapshot is a dict: name, parent (name or None), merge (name or None),
    message, branch, meta, renames, trash (bool) and `edit`, which turns the
    parent's {path: (bytes, mode)} into this snapshot's.
    """
    snaps = []

    def add(name, parent, message, branch, edit, merge=None, meta=(), renames=(), trash=False):
        snaps.append(dict(name=name, parent=parent, merge=merge, message=message,
                          branch=branch, edit=edit, meta=list(meta),
                          renames=list(renames), trash=trash))

    def put(path, content, mode=0):
        def f(t):
            t[path] = (content, mode)
        return f

    def rm(path):
        def f(t):
            del t[path]
        return f

    def mv(src, dst):
        def f(t):
            t[dst] = t.pop(src)
        return f

    add("s1", None, "initial", "main", lambda t: t.update(files_s1()))
    add("s2", "s1", "edit readme", "main", put("README.txt", b"# demo\nsecond line\n"),
        meta=[("velo", "author.name", "Spec Author"),
              ("velo", "author.email", "spec@example.com")])
    add("s3", "s2", "rename b", "main", mv("src/b.txt", "src/renamed.txt"),
        renames=[("src/b.txt", "src/renamed.txt")])
    add("f1", "s3", "feature start", "feature", put("src/feature.txt", b"feature v1\n"))
    add("s4", "s3", "drop notes", "main", rm("docs/notes.txt"))
    add("f2", "f1", "feature done", "feature", put("src/feature.txt", b"feature v2\n"))
    add("s5", "s4", "add epsilon", "main", put("src/e.txt", b"epsilon\n"))
    add("m", "s5", "merge feature", "main", put("src/feature.txt", b"feature v2\n"), merge="f2")
    add("t1", "m", "oops one", "main", put("src/a.txt", b"alpha oops\n"), trash=True)
    add("t2", "t1", "oops two", "main", put("src/t2.txt", b"two\n"), trash=True)
    return snaps


def build(out_repo, out_expected):
    """Write the fixture repository to `out_repo` and expectations to `out_expected`."""
    if os.path.exists(out_repo):
        shutil.rmtree(out_repo)
    velo = os.path.join(out_repo, ".velo")
    objdir = os.path.join(velo, "objects")
    os.makedirs(objdir)

    # Compute trees and ids.
    snaps = build_history()
    by_name = {}
    contents = {}  # hash -> normalised bytes
    for i, s in enumerate(snaps):
        parent = by_name[s["parent"]] if s["parent"] else None
        t = dict(parent["raw"]) if parent else {}
        s["edit"](t)
        s["raw"] = t
        tree = {}
        for path, (data, mode) in t.items():
            data = normalise(data)
            h = blake3_hex(data)
            contents[h] = data
            tree[path] = (h, mode)
        s["tree"] = tree
        s["created_at_ms"] = BASE_MS + 1000 * i
        s["id"] = snapshot_id(
            tree, parent["id"] if parent else "",
            by_name[s["merge"]]["id"] if s["merge"] else "",
            s["message"], s["created_at_ms"], s["meta"])
        by_name[s["name"]] = s

    # Storage: keyframe when the parent's chain + 1 would reach K (rule 4).
    for s in snaps:
        if s["parent"] is None:
            s["storage"], s["base"], s["chain"] = "keyframe", "", 0
        else:
            p = by_name[s["parent"]]
            if p["chain"] + 1 >= K:
                s["storage"], s["base"], s["chain"] = "keyframe", "", 0
            else:
                s["storage"], s["base"], s["chain"] = "delta", p["id"], p["chain"] + 1

    # Objects.
    for h, data in sorted(contents.items()):
        with open(os.path.join(objdir, h), "wb") as f:
            f.write(zstd_raw_frame(data))

    # Working tree: the tree of PARENT = main's tip, the last non-trashed main snapshot.
    tip = [s for s in snaps if s["branch"] == "main" and not s["trash"]][-1]
    feature_tip = [s for s in snaps if s["branch"] == "feature"][-1]
    for path, (h, mode) in tip["tree"].items():
        dest = os.path.join(out_repo, *path.split("/"))
        os.makedirs(os.path.dirname(dest), exist_ok=True)
        with open(dest, "wb") as f:
            f.write(contents[h])
    with open(os.path.join(velo, "HEAD"), "wb") as f:
        f.write(b"main")
    with open(os.path.join(velo, "PARENT"), "wb") as f:
        f.write(tip["id"].encode())

    # Signature over the merge, with the test seed.
    with open(os.path.join(CORPUS, "keys", "test.seed"), "r") as f:
        seed = bytes.fromhex(f.read().strip())
    pub = ed25519_ref.public_key(seed).hex()
    merge_snap = by_name["m"]
    sig = ed25519_ref.sign(seed, b"velo-signature-v1\n" + merge_snap["id"].encode()).hex()

    # Database.
    db_path = os.path.join(velo, "velo.db")
    conn = sqlite3.connect(db_path, isolation_level=None)
    conn.execute("PRAGMA auto_vacuum = INCREMENTAL")
    conn.execute("PRAGMA journal_mode = WAL")
    conn.executescript(SCHEMA_V3)
    conn.executescript(SCHEMA_V4)
    conn.execute("BEGIN")
    for s in snaps:
        parent = by_name[s["parent"]]["id"] if s["parent"] else ""
        merge = by_name[s["merge"]]["id"] if s["merge"] else ""
        if s["trash"]:
            # t2 was undone first, then t1: a chain through a trashed snapshot.
            conn.execute(
                "INSERT INTO trash (hash, message, branch, parent_hash, merge_parent,"
                " created_at_ms, deleted_at_ms) VALUES (?,?,?,?,?,?,?)",
                (s["id"], s["message"], s["branch"], parent, merge, s["created_at_ms"],
                 BASE_MS + 100_000 + (1000 if s["name"] == "t1" else 0)))
        else:
            conn.execute(
                "INSERT INTO snapshots (hash, message, branch, parent_hash, merge_parent,"
                " created_at_ms) VALUES (?,?,?,?,?,?)",
                (s["id"], s["message"], s["branch"], parent, merge, s["created_at_ms"]))
        if s["storage"] == "keyframe":
            for path in sorted(s["tree"]):
                h, mode = s["tree"][path]
                conn.execute("INSERT INTO file_map (snapshot_hash, path, hash, mode)"
                             " VALUES (?,?,?,?)", (s["id"], path, h, mode))
        else:
            base = by_name[s["parent"]]["tree"]
            conn.execute("INSERT INTO delta_base (snapshot_hash, base_hash) VALUES (?,?)",
                         (s["id"], s["base"]))
            rows = []
            for path in sorted(set(base) | set(s["tree"])):
                if path not in s["tree"]:
                    rows.append((path, "", 0))
                elif base.get(path) != s["tree"][path]:
                    rows.append((path,) + s["tree"][path])
            for path, h, mode in rows:
                conn.execute("INSERT INTO file_delta (snapshot_hash, path, hash, mode)"
                             " VALUES (?,?,?,?)", (s["id"], path, h, mode))
        for ns, key, value in s["meta"]:
            conn.execute("INSERT INTO snapshot_meta (snapshot_id, namespace, key, value)"
                         " VALUES (?,?,?,?)", (s["id"], ns, key, value))
        for src, dst in s["renames"]:
            conn.execute("INSERT INTO renames (snapshot_hash, from_path, to_path)"
                         " VALUES (?,?,?)", (s["id"], src, dst))
    tag_target = by_name["s2"]["id"]
    conn.execute("INSERT INTO tags (name, snapshot_hash) VALUES ('v1', ?)", (tag_target,))
    conn.execute("INSERT INTO signatures (snapshot_hash, algorithm, public_key, signature)"
                 " VALUES (?, 'ed25519', ?, ?)", (merge_snap["id"], pub, sig))
    conn.execute("INSERT INTO branches (name, tip) VALUES ('main', ?)", (tip["id"],))
    conn.execute("INSERT INTO branches (name, tip) VALUES ('feature', ?)",
                 (feature_tip["id"],))
    conn.execute("INSERT INTO settings (key, value) VALUES ('keyframe_interval', ?)",
                 (str(K),))
    conn.execute("COMMIT")
    conn.execute("PRAGMA user_version = 4")
    conn.execute("VACUUM")
    # Leave the file the way a cleanly closed velo repository is left: still in
    # WAL mode (header bytes 18-19 are 2, as every velo.db has them, so opening
    # it never rewrites the header) but fully checkpointed, so closing removes
    # the -wal and -shm files and none is needed.
    conn.execute("PRAGMA wal_checkpoint(TRUNCATE)")
    conn.close()
    # Offset 96 holds the SQLite library version that last wrote the file. It is
    # informational (readers ignore it); pin it so the bytes do not depend on
    # the Python build that ran this script.
    with open(db_path, "r+b") as f:
        f.seek(96)
        f.write((3_045_000).to_bytes(4, "big"))
    for leftover in ("-wal", "-shm", "-journal"):
        assert not os.path.exists(db_path + leftover)

    expected = {
        "user_version": 4,
        "keyframe_interval": K,
        "head": "main",
        "parent": tip["id"],
        "branch_tips": {"main": tip["id"], "feature": feature_tip["id"]},
        "tags": {"v1": tag_target},
        "signatures": [{"snapshot": merge_snap["id"], "algorithm": "ed25519",
                        "public_key": pub, "signature": sig}],
        "snapshots": [],
    }
    for s in snaps:
        expected["snapshots"].append({
            "name": s["name"],
            "id": s["id"],
            "parent": by_name[s["parent"]]["id"] if s["parent"] else "",
            "merge_parent": by_name[s["merge"]]["id"] if s["merge"] else "",
            "message": s["message"],
            "branch": s["branch"],
            "created_at_ms": s["created_at_ms"],
            "storage": s["storage"],
            "base": s["base"],
            "chain": s["chain"],
            "in_trash": s["trash"],
            "meta": [list(m) for m in s["meta"]],
            "tree": [[p, s["tree"][p][0], s["tree"][p][1]] for p in sorted(s["tree"])],
        })
    with open(out_expected, "w", newline="\n") as f:
        json.dump(expected, f, indent=1, sort_keys=True)
        f.write("\n")
    return expected


# ----------------------------------------------------------------- selftest

def read_tree(root):
    out = {}
    for dirpath, _, names in os.walk(root):
        for n in names:
            p = os.path.join(dirpath, n)
            with open(p, "rb") as f:
                out[os.path.relpath(p, root).replace("\\", "/")] = f.read()
    return out


def selftest():
    with open(os.path.join(CORPUS, "expected", "v5.1.2.json")) as f:
        golden = json.load(f)
    meta = {}
    for sid, ns, key, value in golden["snapshot_meta"]:
        meta.setdefault(sid, []).append((ns, key, value))
    checked = 0
    for s in golden["snapshots"]:
        tree = {e["path"]: (e["hash"], e["mode"]) for e in golden["file_map"][s["hash"]]}
        got = snapshot_id(tree, s["parent_hash"], s["merge_parent"], s["message"],
                          s["created_at_ms"], meta.get(s["hash"], []))
        assert got == s["hash"], "id mismatch for %s: %s" % (s["message"], got)
        checked += 1
    assert checked >= 8, checked
    print("recomputed %d v5.1.2 snapshot ids" % checked)

    tmp = tempfile.mkdtemp(prefix="velo-v4-selftest-")
    try:
        repo = os.path.join(tmp, "repo")
        exp = os.path.join(tmp, "expected.json")
        build(repo, exp)
        a, b = read_tree(repo), read_tree(os.path.join(HERE, "repo"))
        assert sorted(a) == sorted(b), "file lists differ: %s" % (set(a) ^ set(b))
        for name in a:
            assert a[name] == b[name], "bytes differ: " + name
        with open(exp, "rb") as f1, open(os.path.join(HERE, "expected.json"), "rb") as f2:
            assert f1.read() == f2.read(), "expected.json differs"
    finally:
        shutil.rmtree(tmp, ignore_errors=True)
    print("fixture rebuilt byte-identically (%d files)" % len(a))


if __name__ == "__main__":
    if "--selftest" in sys.argv[1:]:
        selftest()
    else:
        build(os.path.join(HERE, "repo"), os.path.join(HERE, "expected.json"))
        print("wrote", os.path.join(HERE, "repo"))
