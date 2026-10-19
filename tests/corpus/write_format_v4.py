#!/usr/bin/env python3
"""Write the spec-written v4 fixture: tests/corpus/repos/format-v4/ and
tests/corpus/expected/format-v4.json.

This script is written from docs/FORMAT.md alone (section 2 objects, 4.2 identity,
7.2 schema, 11.1 signatures, 11.6 storage v4). It shares no code with velo and
uses only the Python standard library: a repository written by the code under
test would be a tautology, and this is where an ambiguous spec is found.

It is deterministic: fixed timestamps, no randomness. Running it twice gives
byte-identical output (given the same SQLite library for velo.db).

    python tests/corpus/write_format_v4.py --out tests/corpus

The checked-in output is pinned under the golden-change rule (see README.md).
Never regenerate it with a velo build.
"""
import argparse
import base64
import hashlib
import json
import os
import shutil
import sqlite3
import struct
import sys

K = 4  # keyframe interval for this fixture (the real writer constant is 128)

# --------------------------------------------------------------------------
# BLAKE3, hash mode only, after the public reference implementation.
# --------------------------------------------------------------------------
M32 = 0xFFFFFFFF
IV = [0x6A09E667, 0xBB67AE85, 0x3C6EF372, 0xA54FF53A,
      0x510E527F, 0x9B05688C, 0x1F83D9AB, 0x5BE0CD19]
PERM = [2, 6, 3, 10, 7, 0, 4, 13, 1, 11, 12, 5, 9, 14, 15, 8]
CHUNK_START, CHUNK_END, PARENT, ROOT = 1, 2, 4, 8


def _rotr(x, n):
    return ((x >> n) | (x << (32 - n))) & M32


def _g(s, a, b, c, d, mx, my):
    s[a] = (s[a] + s[b] + mx) & M32
    s[d] = _rotr(s[d] ^ s[a], 16)
    s[c] = (s[c] + s[d]) & M32
    s[b] = _rotr(s[b] ^ s[c], 12)
    s[a] = (s[a] + s[b] + my) & M32
    s[d] = _rotr(s[d] ^ s[a], 8)
    s[c] = (s[c] + s[d]) & M32
    s[b] = _rotr(s[b] ^ s[c], 7)


def _round(s, m):
    _g(s, 0, 4, 8, 12, m[0], m[1])
    _g(s, 1, 5, 9, 13, m[2], m[3])
    _g(s, 2, 6, 10, 14, m[4], m[5])
    _g(s, 3, 7, 11, 15, m[6], m[7])
    _g(s, 0, 5, 10, 15, m[8], m[9])
    _g(s, 1, 6, 11, 12, m[10], m[11])
    _g(s, 2, 7, 8, 13, m[12], m[13])
    _g(s, 3, 4, 9, 14, m[14], m[15])


def _compress(cv, block_words, counter, block_len, flags):
    s = list(cv) + IV[:4] + [counter & M32, (counter >> 32) & M32, block_len, flags]
    m = list(block_words)
    for r in range(7):
        _round(s, m)
        if r < 6:
            m = [m[i] for i in PERM]
    return [s[i] ^ s[i + 8] for i in range(8)] + [s[i + 8] ^ cv[i] for i in range(8)]


def _words(block):
    block = block + b"\0" * (64 - len(block))
    return list(struct.unpack("<16I", block))


def _chunk_output(chunk, counter):
    """Return (cv, block_words, counter, block_len, flags) of the chunk's last block."""
    cv = list(IV)
    blocks = [chunk[i:i + 64] for i in range(0, len(chunk), 64)] or [b""]
    for i, blk in enumerate(blocks):
        flags = CHUNK_START if i == 0 else 0
        if i == len(blocks) - 1:
            return (cv, _words(blk), counter, len(blk), flags | CHUNK_END)
        cv = _compress(cv, _words(blk), counter, 64, flags)[:8]


def _chunk_cv(chunk, counter):
    cv, w, c, bl, fl = _chunk_output(chunk, counter)
    return _compress(cv, w, c, bl, fl)[:8]


def _parent_cv(left, right):
    return _compress(IV, left + right, 0, 64, PARENT)[:8]


def blake3_hex(data: bytes) -> str:
    chunks = [data[i:i + 1024] for i in range(0, len(data), 1024)] or [b""]
    if len(chunks) == 1:
        cv, w, c, bl, fl = _chunk_output(chunks[0], 0)
        out = _compress(cv, w, c, bl, fl | ROOT)[:8]
        return struct.pack("<8I", *out).hex()
    # Chunk chaining values merge as a left-full binary tree: the left subtree
    # holds the largest power of two of chunks strictly less than the total.
    cvs = [_chunk_cv(ch, i) for i, ch in enumerate(chunks)]

    def subtree(lo, hi, root):
        n = hi - lo
        if n == 1:
            return cvs[lo]
        split = 1
        while split * 2 < n:
            split *= 2
        left = subtree(lo, lo + split, False)
        right = subtree(lo + split, hi, False)
        if root:
            return _compress(IV, left + right, 0, 64, PARENT | ROOT)[:8]
        return _parent_cv(left, right)

    return struct.pack("<8I", *subtree(0, len(cvs), True)).hex()


def blake3_selftest():
    """Official vectors: input byte i is i % 251."""
    def vec(n):
        return bytes(i % 251 for i in range(n))
    cases = {
        0: "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262",
        1: "2d3adedff11b61f14c886e35afa036736dcd87a74d27b5c1510225d0f592e213",
        1024: "42214739f095a406f3fc83deb889744ac00df831c10daa55189b5d121c855af7",
        1025: "d00278ae47eb27b34faecf67b4fe263f82d5412916c1ffd97c8cb7fb814b8444",
    }
    for n, want in cases.items():
        got = blake3_hex(vec(n))
        if got != want:
            sys.exit(f"BLAKE3 self-test failed for length {n}: {got} != {want}")


# --------------------------------------------------------------------------
# Ed25519, after the RFC 8032 section 6 reference code.
# --------------------------------------------------------------------------
P = 2 ** 255 - 19
Q = 2 ** 252 + 27742317777372353535851937790883648493


def _sha512(s):
    return hashlib.sha512(s).digest()


def _inv(x):
    return pow(x, P - 2, P)


D = -121665 * _inv(121666) % P
SQRT_M1 = pow(2, (P - 1) // 4, P)


def _recover_x(y, sign):
    if y >= P:
        return None
    x2 = (y * y - 1) * _inv(D * y * y + 1)
    if x2 == 0:
        return None if sign else 0
    x = pow(x2, (P + 3) // 8, P)
    if (x * x - x2) % P != 0:
        x = x * SQRT_M1 % P
    if (x * x - x2) % P != 0:
        return None
    if (x & 1) != sign:
        x = P - x
    return x


_GY = 4 * _inv(5) % P
_GX = _recover_x(_GY, 0)
G = (_GX, _GY, 1, _GX * _GY % P)


def _add(p, q):
    a = (p[1] - p[0]) * (q[1] - q[0]) % P
    b = (p[1] + p[0]) * (q[1] + q[0]) % P
    c = 2 * p[3] * q[3] * D % P
    d = 2 * p[2] * q[2] % P
    e, f, g, h = b - a, d - c, d + c, b + a
    return (e * f % P, g * h % P, f * g % P, e * h % P)


def _mul(s, p):
    q = (0, 1, 1, 0)
    while s > 0:
        if s & 1:
            q = _add(q, p)
        p = _add(p, p)
        s >>= 1
    return q


def _compress_point(p):
    zinv = _inv(p[2])
    x, y = p[0] * zinv % P, p[1] * zinv % P
    return int.to_bytes(y | ((x & 1) << 255), 32, "little")


def _decompress_point(s):
    y = int.from_bytes(s, "little")
    sign = y >> 255
    y &= (1 << 255) - 1
    x = _recover_x(y, sign)
    if x is None:
        return None
    return (x, y, 1, x * y % P)


def _point_equal(p, q):
    if (p[0] * q[2] - q[0] * p[2]) % P != 0:
        return False
    return (p[1] * q[2] - q[1] * p[2]) % P == 0


def _secret_expand(seed):
    h = _sha512(seed)
    a = int.from_bytes(h[:32], "little")
    a &= (1 << 254) - 8
    a |= 1 << 254
    return a, h[32:]


def ed25519_public(seed):
    a, _ = _secret_expand(seed)
    return _compress_point(_mul(a, G))


def ed25519_sign(seed, msg):
    a, prefix = _secret_expand(seed)
    pub = _compress_point(_mul(a, G))
    r = int.from_bytes(_sha512(prefix + msg), "little") % Q
    rs = _compress_point(_mul(r, G))
    h = int.from_bytes(_sha512(rs + pub + msg), "little") % Q
    s = (r + h * a) % Q
    return rs + int.to_bytes(s, 32, "little")


def ed25519_verify(pub, msg, sig):
    a = _decompress_point(pub)
    r = _decompress_point(sig[:32])
    if a is None or r is None:
        return False
    s = int.from_bytes(sig[32:], "little")
    if s >= Q:
        return False
    h = int.from_bytes(_sha512(sig[:32] + pub + msg), "little") % Q
    return _point_equal(_mul(s, G), _add(r, _mul(h, a)))


def ed25519_selftest():
    # RFC 8032 section 7.1, test 1 (empty message).
    seed = bytes.fromhex("9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60")
    pub = bytes.fromhex("d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a")
    sig = bytes.fromhex(
        "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e06522490155"
        "5fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b")
    if ed25519_public(seed) != pub or ed25519_sign(seed, b"") != sig \
            or not ed25519_verify(pub, b"", sig):
        sys.exit("Ed25519 self-test failed")


# --------------------------------------------------------------------------
# Spec rules: normalisation, objects, identity.
# --------------------------------------------------------------------------
def normalise(raw: bytes) -> bytes:
    """FORMAT.md 2.1: binary (any NUL) verbatim, otherwise drop every CR."""
    if b"\0" in raw:
        return raw
    return raw.replace(b"\r", b"")


def zstd_raw_frame(content: bytes) -> bytes:
    """A valid Zstd frame made only of Raw blocks (RFC 8878). The header
    descriptor 0xE0 is Single_Segment with an 8-byte Frame_Content_Size."""
    out = b"\x28\xb5\x2f\xfd" + b"\xe0" + struct.pack("<Q", len(content))
    max_block = 128 * 1024
    pieces = [content[i:i + max_block] for i in range(0, len(content), max_block)] or [b""]
    for i, piece in enumerate(pieces):
        last = 1 if i == len(pieces) - 1 else 0
        header = last | (0 << 1) | (len(piece) << 3)  # block type 0 = Raw
        out += struct.pack("<I", header)[:3] + piece
    return out


def snapshot_id(tree, parent, merge, message, ts_ms, meta):
    """FORMAT.md 4.2. tree: {path: (hash, mode)}; meta: [(ns, key, value)]."""
    b = b"velo-snapshot-v2\n"
    for path in sorted(tree, key=lambda p: p.encode()):
        h, mode = tree[path]
        b += path.encode() + b"\0" + h.encode() + b"\0" + str(mode).encode() + b"\n"
    b += b"parent\0" + parent.encode()
    b += b"\nmerge\0" + merge.encode()
    b += b"\nmessage\0" + message.encode()
    b += b"\ntime\0" + str(ts_ms).encode()
    b += b"\nmeta\0"
    for ns, key, value in sorted(meta, key=lambda m: (m[0].encode(), m[1].encode())):
        b += ns.encode() + b"\0" + key.encode() + b"\0" + value.encode() + b"\n"
    return blake3_hex(b)


def signed_message(sid):
    return b"velo-signature-v1\n" + sid.encode()


# --------------------------------------------------------------------------
# The history. Entries map path -> (raw working-tree bytes, mode), or None to
# delete. Each step lists only what changed against its first parent.
# --------------------------------------------------------------------------
T0 = 1_800_000_000_000
AUTHOR = [("velo", "author.name", "Format Author"),
          ("velo", "author.email", "format@example.com")]

BLOB = bytes([0, 1, 2, 3, 255, 254, 253, 0, 7, 8, 9])
README1 = b"format v4 fixture\nline two\n"
README2 = b"format v4 fixture\nline two\nmain line\n"
README3 = b"format v4 fixture\nline two\nmain line\nmore main\n"
SCRIPT = b"#!/bin/sh\necho hi\n"

STEPS = [
    dict(n=1, kind="save", branch="main", parent=None, message="initial", dt=0,
         entries={"README.txt": (README1, 0), "src/a.txt": (b"alpha\n", 0),
                  "src/deep/b.txt": (b"bravo\n", 0), "bin/blob.bin": (BLOB, 0),
                  "crlf.txt": (b"one\r\ntwo\r\n", 0), "run.sh": (SCRIPT, 0)}),
    dict(n=2, kind="save", branch="main", parent=1, message="second", dt=1,
         entries={"README.txt": (README2, 0), "src/c.txt": (b"charlie\n", 0)}, tag="v1"),
    dict(n=3, kind="save", branch="main", parent=2, message="make run.sh executable", dt=2,
         entries={"run.sh": (SCRIPT, 1)}, meta=[("corpus", "step", "meta")]),
    dict(n=4, kind="save", branch="main", parent=3, message="rename a to renamed", dt=3,
         entries={"src/a.txt": None, "src/renamed.txt": (b"alpha\n", 0)},
         renames=[("src/a.txt", "src/renamed.txt")], sign=True),
    dict(n=5, kind="save", branch="main", parent=4, message="drop c, edit blob", dt=4,
         entries={"src/c.txt": None, "bin/blob.bin": (BLOB + b"\0\0", 0)}),
    dict(n=6, kind="save", branch="feature", parent=5, message="feature work", dt=5,
         entries={"src/feature.txt": (b"feature one\n", 0)}, new_branch=True),
    dict(n=7, kind="save", branch="main", parent=5, message="main edit", dt=6,
         entries={"README.txt": (README3, 0)}),
    dict(n=8, kind="save", branch="feature", parent=6, message="feature more", dt=7,
         entries={"src/feature.txt": (b"feature one\nfeature two\n", 0)}),
    dict(n=9, kind="merge", branch="main", parent=7, merge=8, message="merge feature", dt=8,
         entries={"src/feature.txt": (b"feature one\nfeature two\n", 0)}),
    dict(n=10, kind="save", branch="main", parent=9, message="swap deep file", dt=9,
         entries={"src/deep/b.txt": None, "src/deep/d.txt": (b"delta\n", 0)}),
    dict(n=11, kind="save", branch="main", parent=10, message="edit renamed", dt=10,
         entries={"src/renamed.txt": (b"alpha\nalpha two\n", 0)}),
    dict(n=12, kind="save", branch="main", parent=11, message="to be undone one", dt=11,
         entries={"extra.txt": (b"extra one\n", 0)}, undone=True),
    dict(n=13, kind="save", branch="main", parent=12, message="to be undone two", dt=12,
         entries={"extra.txt": (b"extra one\nextra two\n", 0)}, undone=True),
]


def build():
    snaps = {}  # n -> record
    for st in STEPS:
        parent_rec = snaps[st["parent"]] if st["parent"] else None
        base_tree = dict(parent_rec["tree"]) if parent_rec else {}
        raw = dict(parent_rec["raw"]) if parent_rec else {}
        tree = dict(base_tree)
        for path, val in st["entries"].items():
            if val is None:
                del tree[path]
                del raw[path]
            else:
                tree[path] = (blake3_hex(normalise(val[0])), val[1])
                raw[path] = val[0]
        meta = AUTHOR + list(st.get("meta", []))
        parent_id = parent_rec["id"] if parent_rec else ""
        merge_id = snaps[st["merge"]]["id"] if st.get("merge") else ""
        ts = T0 + st["dt"] * 1000
        sid = snapshot_id(tree, parent_id, merge_id, st["message"], ts, meta)
        # FORMAT.md 11.6: delta iff the first parent has stored rows and
        # depth(parent) + 1 <= K - 1; otherwise a keyframe.
        if parent_rec is not None and parent_rec["depth"] + 1 <= K - 1:
            depth = parent_rec["depth"] + 1
        else:
            depth = 0
        # Canonical delta: one row per path whose (hash, mode) differs from the base.
        rows = {}
        for path in sorted(set(tree) | set(base_tree), key=lambda p: p.encode()):
            if tree.get(path) != base_tree.get(path):
                rows[path] = tree.get(path)
        snaps[st["n"]] = dict(st=st, id=sid, tree=tree, raw=raw, parent_id=parent_id,
                              merge_id=merge_id, ts=ts, meta=meta, depth=depth,
                              delta_rows=rows)
    return snaps


# --------------------------------------------------------------------------
# Writing.
# --------------------------------------------------------------------------
V4_SCHEMA = """
CREATE TABLE IF NOT EXISTS tree_deltas (
    snapshot_hash TEXT PRIMARY KEY,
    base_hash     TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS tree_delta_rows (
    snapshot_hash TEXT NOT NULL,
    path          TEXT NOT NULL,
    hash          TEXT,
    mode          INTEGER,
    PRIMARY KEY (snapshot_hash, path)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS idx_tree_deltas_base ON tree_deltas (base_hash);
CREATE INDEX IF NOT EXISTS idx_delta_rows_path  ON tree_delta_rows (path);
"""

# Copied from crates/velo-core/src/db.rs (SCHEMA): the v3 tables and indexes.
BASE_SCHEMA = """
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
    CREATE TABLE IF NOT EXISTS stash (
        id            INTEGER PRIMARY KEY AUTOINCREMENT,
        name          TEXT NOT NULL UNIQUE,
        snapshot_hash TEXT NOT NULL,
        branch        TEXT NOT NULL,
        parent_hash   TEXT NOT NULL DEFAULT '',
        created_at_ms INTEGER NOT NULL DEFAULT 0
    );
    CREATE TABLE IF NOT EXISTS conflict_files (
        path          TEXT PRIMARY KEY,
        ancestor_hash TEXT NOT NULL,
        our_hash      TEXT NOT NULL,
        their_hash    TEXT NOT NULL
    );
    CREATE TABLE IF NOT EXISTS hunk_decisions (
        file_path      TEXT    NOT NULL,
        hunk_id        INTEGER NOT NULL,
        decision       TEXT    NOT NULL,
        manual_content TEXT,
        PRIMARY KEY (file_path, hunk_id)
    );
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
    CREATE TABLE IF NOT EXISTS renames (
        snapshot_hash TEXT NOT NULL,
        from_path     TEXT NOT NULL,
        to_path       TEXT NOT NULL,
        PRIMARY KEY (snapshot_hash, to_path)
    );
    CREATE TABLE IF NOT EXISTS pending_renames (
        from_path TEXT NOT NULL,
        to_path   TEXT PRIMARY KEY
    );
    CREATE TABLE IF NOT EXISTS compactions (
        old_hash        TEXT PRIMARY KEY,
        new_hash        TEXT NOT NULL,
        compacted_at_ms INTEGER NOT NULL
    );
    CREATE TABLE IF NOT EXISTS signatures (
        snapshot_hash TEXT NOT NULL,
        algorithm     TEXT NOT NULL,
        public_key    TEXT NOT NULL,
        signature     TEXT NOT NULL,
        PRIMARY KEY (snapshot_hash, public_key)
    );
    CREATE TABLE IF NOT EXISTS settings (
        key   TEXT PRIMARY KEY,
        value TEXT NOT NULL
    );
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
    CREATE INDEX IF NOT EXISTS idx_renames_to     ON renames (to_path);
"""


def write_json(path, obj):
    text = json.dumps(obj, indent=1, sort_keys=True, ensure_ascii=False) + "\n"
    with open(path, "wb") as f:
        f.write(text.encode("utf-8"))


def write_file(path, data):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "wb") as f:
        f.write(data)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", required=True, help="the tests/corpus directory to write into")
    out = ap.parse_args().out

    blake3_selftest()
    ed25519_selftest()

    # The seed file is 64 hex characters (32 bytes), as the Rust test key loader reads it.
    with open(os.path.join(out, "keys", "test.seed"), "rb") as f:
        seed = bytes.fromhex(f.read().decode().strip())
    pub = ed25519_public(seed)

    snaps = build()
    repo = os.path.join(out, "repos", "format-v4")
    if os.path.exists(repo):
        shutil.rmtree(repo)
    velo = os.path.join(repo, ".velo")
    os.makedirs(os.path.join(velo, "objects"))

    # Objects: every content ever stored, one Zstd frame of Raw blocks each.
    contents = {}
    for rec in snaps.values():
        for raw in rec["raw"].values():
            norm = normalise(raw)
            contents[blake3_hex(norm)] = norm
    for h, norm in sorted(contents.items()):
        write_file(os.path.join(velo, "objects", h), zstd_raw_frame(norm))

    db_path = os.path.join(velo, "velo.db")
    conn = sqlite3.connect(db_path, isolation_level=None)
    conn.execute("PRAGMA auto_vacuum = INCREMENTAL")
    conn.execute("PRAGMA journal_mode = WAL")
    conn.executescript(BASE_SCHEMA)
    conn.executescript(V4_SCHEMA)
    conn.execute("PRAGMA user_version = 4")
    conn.execute("BEGIN")

    order = [snaps[n] for n in sorted(snaps)]
    undone = [r for r in order if r["st"].get("undone")]
    live = [r for r in order if not r["st"].get("undone")]
    # Undo removes the newest snapshot first, so deleted_at rises in undo
    # order: the parent (undone last) has the latest deleted_at_ms, which is
    # what redo's ORDER BY deleted_at_ms DESC picks first.
    for i, rec in enumerate(reversed(undone)):
        rec["deleted_at"] = T0 + 100_000 + i
    for rec in order:
        st, sid = rec["st"], rec["id"]
        if st.get("undone"):
            conn.execute("INSERT INTO trash VALUES (?,?,?,?,?,?,?)",
                         (sid, st["message"], st["branch"], rec["parent_id"], rec["merge_id"],
                          rec["ts"], rec["deleted_at"]))
        else:
            conn.execute("INSERT INTO snapshots VALUES (?,?,?,?,?,?)",
                         (sid, st["message"], st["branch"], rec["parent_id"], rec["merge_id"],
                          rec["ts"]))
        if rec["depth"] == 0:
            for path in sorted(rec["tree"], key=lambda p: p.encode()):
                h, mode = rec["tree"][path]
                conn.execute("INSERT INTO file_map VALUES (?,?,?,?)", (sid, path, h, mode))
        else:
            conn.execute("INSERT INTO tree_deltas VALUES (?,?)", (sid, rec["parent_id"]))
            for path, val in rec["delta_rows"].items():
                h, mode = val if val else (None, None)
                conn.execute("INSERT INTO tree_delta_rows VALUES (?,?,?,?)", (sid, path, h, mode))
        for ns, key, value in rec["meta"]:
            conn.execute("INSERT INTO snapshot_meta VALUES (?,?,?,?)", (sid, ns, key, value))
        for frm, to in st.get("renames", []):
            conn.execute("INSERT INTO renames VALUES (?,?,?)", (sid, frm, to))
        if st.get("tag"):
            conn.execute("INSERT INTO tags VALUES (?,?)", (st["tag"], sid))
        if st.get("sign"):
            sig = ed25519_sign(seed, signed_message(sid))
            conn.execute("INSERT INTO signatures VALUES (?,?,?,?)",
                         (sid, "ed25519", pub.hex(), sig.hex()))
            rec["sig"] = sig.hex()

    tips = {}
    for rec in live:
        tips[rec["st"]["branch"]] = rec["id"]
    for name in sorted(tips):
        conn.execute("INSERT INTO branches VALUES (?,?)", (name, tips[name]))
    conn.execute("COMMIT")
    conn.close()
    for suffix in ("-wal", "-shm"):
        if os.path.exists(db_path + suffix):
            os.remove(db_path + suffix)

    # HEAD is main, PARENT its tip, and the working tree equals PARENT's tree.
    head = [r for r in live if r["st"]["branch"] == "main"][-1]
    write_file(os.path.join(velo, "HEAD"), b"main")
    write_file(os.path.join(velo, "PARENT"), head["id"].encode())
    write_file(os.path.join(velo, "lock"), b"")
    for path, raw in head["raw"].items():
        write_file(os.path.join(repo, *path.split("/")), raw)

    self_check(db_path, velo)
    write_expected(out, snaps, live, undone, tips, pub)


def self_check(db_path, velo):
    """Reconstruct every tree from the rows alone, recompute every id, and fail
    on any difference. Reads only the database and the object files."""
    conn = sqlite3.connect(db_path)
    rows = {}
    for table in ("snapshots", "trash"):
        for r in conn.execute(
                f"SELECT hash,message,parent_hash,merge_parent,created_at_ms FROM {table}"):
            rows[r[0]] = r
    deltas = dict(conn.execute("SELECT snapshot_hash, base_hash FROM tree_deltas"))

    def chain_depth(sid):
        d, seen = 0, set()
        while sid in deltas:
            if sid in seen or d > 4096:
                sys.exit("self-check: base cycle")
            seen.add(sid)
            if deltas[sid] != rows[sid][2]:
                sys.exit("self-check: base_hash differs from parent_hash")
            sid, d = deltas[sid], d + 1
        return d

    def tree_of(sid):
        if sid not in deltas:
            return {p: (h, m) for p, h, m in conn.execute(
                "SELECT path,hash,mode FROM file_map WHERE snapshot_hash=?", (sid,))}
        if conn.execute("SELECT 1 FROM file_map WHERE snapshot_hash=?", (sid,)).fetchone():
            sys.exit("self-check: a delta has file_map rows")
        tree = tree_of(deltas[sid])
        for p, h, m in conn.execute(
                "SELECT path,hash,mode FROM tree_delta_rows WHERE snapshot_hash=?", (sid,)):
            if h is None:
                if m is not None or p not in tree:
                    sys.exit("self-check: bad removal row")
                del tree[p]
            else:
                tree[p] = (h, m)
        return tree

    depths = set()
    for sid, (_, message, parent, merge, ts) in rows.items():
        meta = list(conn.execute(
            "SELECT namespace,key,value FROM snapshot_meta WHERE snapshot_id=?", (sid,)))
        tree = tree_of(sid)
        if snapshot_id(tree, parent, merge, message, ts, meta) != sid:
            sys.exit(f"self-check: id mismatch for {sid}")
        d = chain_depth(sid)
        if d > K - 1:
            sys.exit("self-check: chain deeper than K - 1")
        depths.add(d)
        for h, _m in tree.values():
            with open(os.path.join(velo, "objects", h), "rb") as f:
                frame = f.read()
            # Skip 4 magic + 1 descriptor + 8 size + 3 block header bytes.
            if frame[:4] != b"\x28\xb5\x2f\xfd" or blake3_hex(frame[16:]) != h:
                sys.exit(f"self-check: object {h} does not match its name")
    if depths != set(range(K)):
        sys.exit(f"self-check: chain depths {sorted(depths)}, wanted 0..{K - 1}")
    for sid, algo, key, sig in conn.execute(
            "SELECT snapshot_hash,algorithm,public_key,signature FROM signatures"):
        if algo != "ed25519" or not ed25519_verify(
                bytes.fromhex(key), signed_message(sid), bytes.fromhex(sig)):
            sys.exit("self-check: bad signature")
    if conn.execute("PRAGMA user_version").fetchone()[0] != 4:
        sys.exit("self-check: user_version")
    conn.close()


def tree_list(tree):
    return [{"hash": tree[p][0], "mode": tree[p][1], "path": p}
            for p in sorted(tree, key=lambda p: p.encode())]


def write_expected(out, snaps, live, undone, tips, pub):
    recs = [snaps[n] for n in sorted(snaps)]
    ops = []
    for rec in recs:
        st = rec["st"]
        if st.get("new_branch"):
            ops.append({"op": "branch", "name": st["branch"], "from": st["parent"],
                        "from_id": rec["parent_id"]})
        entries = []
        for path in sorted(rec["delta_rows"], key=lambda p: p.encode()):
            val = rec["delta_rows"][path]
            if val is None:
                entries.append({"path": path, "remove": True})
            else:
                entries.append({"path": path, "hash": val[0], "mode": val[1],
                                "content_base64": base64.b64encode(rec["raw"][path]).decode()})
        op = {"op": st["kind"], "n": st["n"], "id": rec["id"], "branch": st["branch"],
              "parent": st["parent"], "parent_id": rec["parent_id"],
              "message": st["message"], "timestamp_ms": rec["ts"], "entries": entries,
              "meta": [list(m) for m in rec["meta"]],
              "renames": [list(r) for r in st.get("renames", [])],
              "depth": rec["depth"]}
        if st.get("merge"):
            op["merge_parent"] = st["merge"]
            op["merge_parent_id"] = rec["merge_id"]
        if st.get("tag"):
            op["tag"] = st["tag"]
        if st.get("sign"):
            op["sign"] = True
        ops.append(op)
    for rec in reversed(undone):
        ops.append({"op": "undo", "n": rec["st"]["n"], "id": rec["id"],
                    "deleted_at_ms": rec["deleted_at"]})

    def snap_row(r):
        return {"branch": r["st"]["branch"], "created_at_ms": r["ts"], "hash": r["id"],
                "merge_parent": r["merge_id"], "message": r["st"]["message"],
                "parent_hash": r["parent_id"]}

    delta_recs = [r for r in recs if r["depth"] > 0]
    exp = {
        "user_version": 4,
        "k": K,
        "snapshots": [snap_row(r) for r in sorted(live, key=lambda r: (r["ts"], r["st"]["n"]))],
        "file_map": {r["id"]: tree_list(r["tree"]) for r in recs},
        "tags": [[r["st"]["tag"], r["id"]] for r in recs if r["st"].get("tag")],
        "snapshot_meta": sorted([r["id"], a, b, c] for r in recs for a, b, c in r["meta"]),
        "renames": [[r["id"], f, t] for r in recs for f, t in r["st"].get("renames", [])],
        "trash_ids": [r["id"] for r in undone],
        "trash": [dict(snap_row(r), deleted_at_ms=r["deleted_at"]) for r in undone],
        "branch_tips": [[b, tips[b]] for b in sorted(tips)],
        "signatures": [[r["id"], "ed25519", pub.hex(), r["sig"]] for r in recs if r.get("sig")],
        "signer_public_key": pub.hex(),
        "tree_deltas": sorted([r["id"], r["parent_id"]] for r in delta_recs),
        "tree_delta_rows": sorted(
            ([r["id"], p, (v[0] if v else None), (v[1] if v else None)]
             for r in delta_recs for p, v in r["delta_rows"].items()),
            key=lambda x: (x[0], x[1].encode())),
        "keyframes": sorted(r["id"] for r in recs if r["depth"] == 0),
        "depths": {r["id"]: r["depth"] for r in recs},
        "ops": ops,
    }
    write_json(os.path.join(out, "expected", "format-v4.json"), exp)


if __name__ == "__main__":
    main()
