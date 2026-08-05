# Velo repository format

Normative specification of Velo's on-disk format: the object store, the snapshot
identity recipe, the SQLite schema, and the bundle wire format.

Anything that reads or writes a `.velo` directory — the CLI, `velo-core`, or a
third-party tool — must conform to this document.

| | |
| :--- | :--- |
| **Current implemented format** | **v2** (repository format version `2`) |
| Status of v2 | **Implemented.** All four decisions landed in one commit, as required. |
| Status of v1 | **Refused.** A pre-v2 repository cannot be opened; see [Migration](#migration-v1--v2). |

> ⚠️ **v2 was a deliberate, one-time breaking change.** It changed every snapshot
> ID. It was specified and implemented before any external consumer existed,
> precisely so it never has to happen again. Sections below mark **v1** and **v2**
> explicitly wherever they differ — do not read an unmarked statement as applying
> to both. v1 is retained here as documentation of what existing data looks like,
> not as something this implementation can read.

---

## 1. Repository layout

```
.velo/
├── velo.db       SQLite (WAL): snapshots, trees, refs, remotes, stash, conflicts
├── objects/      content-addressed blobs, Zstd-compressed, named by BLAKE3 hex
├── HEAD          current branch name (text, no trailing newline required)
├── PARENT        snapshot id the working tree is based on ("" if unborn)
├── lock          advisory lock file (fs2); held by mutating operations
├── MERGE_HEAD    present only mid-merge/cherry-pick: "<pre-merge-id>:<source>"
└── REBASE_STATE  present only mid-rebase (with REBASE_ONTO, REBASE_ORIG_HEAD)
```

`HEAD` and `PARENT` are refs written **atomically** (temp file + rename). A reader
must tolerate a missing or empty `PARENT` (a repository with no commits).

---

## 2. Object store

An object is the **Zstd-compressed** (level 1) content of a single file, stored at
`.velo/objects/<hash>` where `<hash>` is the **full 64-hex BLAKE3** of the
*uncompressed, normalised* bytes.

Object naming is unchanged between v1 and v2.

### 2.1 Content normalisation

Before hashing or storing, content is normalised:

1. If the byte sequence contains a `0x00` byte it is treated as **binary** and
   stored verbatim — no normalisation.
2. Otherwise every `\r` byte is removed (`\r\n` → `\n`, lone `\r` dropped).

The same normalisation is applied when computing a file's hash for change
detection, so a file's stored hash always equals the hash of what is stored.

> Consequence: text files round-trip as LF. This is intentional and platform
> independent; line-ending restoration is a working-tree concern, not a storage
> concern.

### 2.2 Symlinks

A symlink's object content is its **target path** as UTF-8 bytes, with `\`
normalised to `/`. It is stored raw (no CRLF normalisation).

### 2.3 Integrity invariant

For every object, `BLAKE3(zstd_decompress(file)) == file_name`. `velo fsck`
verifies this, and any import (bundle or sync) must verify it **before** trusting
received data.

---

## 3. Trees

A tree is the complete set of files in a snapshot — not a delta. Each entry is:

| Field | Type | Meaning |
| :--- | :--- | :--- |
| `path` | text | repo-relative, **forward slashes**, no leading `./` |
| `hash` | text | object hash (full 64-hex) |
| `mode` | int | `0` regular, `1` executable, `2` symlink |

Storage is deduplicated at the object level: unchanged files across snapshots
reference the same object. Trees themselves are stored row-per-entry in
`file_map`, not as a separate hashed tree object.

Mode semantics: the executable bit is only observable on Unix. On platforms that
cannot observe it, an implementation must **carry the parent's mode forward**
rather than resetting to `0`. Symlink creation may fall back to writing a regular
file containing the target text where the platform forbids symlinks.

---

## 4. Snapshot identity

A snapshot's id is a BLAKE3 hash over a domain-separated serialisation of its
**full tree**, its parents, its message, and its timestamp. Because the id commits
to the tree, a snapshot can be verified against its own contents.

The **branch is deliberately excluded**: renaming or deleting a branch must not
change the identity of its commits, and the same commit reachable from two
branches must have one id.

### 4.1 v1 recipe (historical — no longer read or written)

```
BLAKE3(
  "velo-snapshot-v1\n"
  for each tree entry, sorted by path ascending (byte order):
      path "\0" hash "\0" mode(decimal) "\n"
  "parent\0"  parent_id
  "\nmerge\0" merge_parent_id      (empty string when not a merge)
  "\nmessage\0" message
  "\ntime\0"  timestamp            (format "%Y-%m-%d %H:%M:%S%.3f")
)  →  hex, truncated to the first 16 characters
```

Absent parents/merge-parents are encoded as the **empty string**, not omitted.

### 4.2 v2 recipe (current)

Three changes from v1, all decided in [Decisions](#decisions):

```
BLAKE3(
  "velo-snapshot-v2\n"
  for each tree entry, sorted by path ascending (byte order):
      path "\0" hash "\0" mode(decimal) "\n"
  "parent\0"  parent_id
  "\nmerge\0" merge_parent_id
  "\nmessage\0" message
  "\ntime\0"  timestamp_ms(decimal)        ← epoch milliseconds, not text
  "\nmeta\0"
  for each metadata pair, sorted by (namespace, key) ascending:
      namespace "\0" key "\0" value "\n"   ← app metadata is hashed
)  →  full 64-hex, stored in full
```

- The domain separator changes to `velo-snapshot-v2\n`, so a v1 and v2 snapshot
  can never collide even with identical inputs.
- Ids are **stored at full width**. Truncation to 16 characters is a *display*
  concern only (see §8).
- Metadata participates in the hash. An empty metadata set still emits the
  `"\nmeta\0"` marker, so "no metadata" and "metadata absent" are the same thing.

---

## 5. Snapshot metadata (v2)

Structured, app-namespaced key/values attached to a snapshot — so consumers stop
encoding state into the message string or inventing sidecar files.

```sql
CREATE TABLE snapshot_meta (
    snapshot_id TEXT NOT NULL,
    namespace   TEXT NOT NULL,   -- e.g. 'promptreg', reverse-DNS also fine
    key         TEXT NOT NULL,
    value       TEXT NOT NULL,
    PRIMARY KEY (snapshot_id, namespace, key)
);
```

Rules:

- `namespace` must be non-empty and must not contain `\0`. The namespace `velo`
  is **reserved** for this project.
- `key` and `value` are opaque UTF-8 to Velo. Consumers own their meaning.
- **Metadata is covered by the snapshot hash** and is therefore **immutable**.
  Changing metadata produces a new snapshot. There is no in-place edit.
- Metadata travels with bundles and sync (it is part of snapshot identity, so it
  must, or ids would fail verification on the receiving side).

> Rationale for hashing: metadata is frequently provenance (`author_tool_version`,
> `eval_run`). Provenance that can be silently rewritten is worthless, and
> tamper-evidence is the whole point of content addressing. The cost is
> immutability, which is the correct trade.

---

## 6. Timestamps

| | |
| :--- | :--- |
| **v1** | text, `"%Y-%m-%d %H:%M:%S%.3f"` UTC, and hashed as that text |
| **v2** | integer **epoch milliseconds** (UTC), hashed as its decimal representation |

v2 removes string formatting from the identity recipe: a locale, precision, or
formatting change can no longer alter a snapshot id. APIs expose
`DateTime<Utc>`; the integer is a storage detail.

Ordering: `created_at_ms` ascending is chronological. Implementations must not
rely on lexicographic ordering of timestamps in v2 (it no longer holds), and must
tie-break on a stable secondary key (`rowid`) when timestamps collide.

---

## 7. SQLite schema

### 7.1 Versioning

| | |
| :--- | :--- |
| **v1** | **No version marker.** Migrations sniff `pragma_table_info(...)` and add missing columns. There is no way to detect a repository written by a *newer* implementation. |
| **v2** | `PRAGMA user_version` holds the repository format version, stamped when the database is created. |

The v1 `ALTER TABLE` sniffing migrations are gone. They existed only to bring a v1
repository forward, and v2 refuses to open one, so keeping them would have meant
maintaining a chain of migrations nothing could reach. The schema is now one
idempotent definition, which is also the migration for a v2 repository written by
an earlier build of v2.

**v2 rules — normative:**

- `user_version = 2` for this specification.
- An implementation **must refuse to open** a repository whose `user_version`
  exceeds the highest version it understands, with a distinct, catchable error
  (`SchemaTooNew { found, supported }`). Silently proceeding risks half-migration
  and data loss when several independent applications share a repository.
- Opening and migrating are **separate operations**. `open()` must not migrate;
  `open_and_migrate()` performs the upgrade. The caller decides when a
  potentially-destructive upgrade happens — a background daemon must not silently
  migrate a repo a user's other tool is mid-use.
- Migrations are forward-only. Downgrade is not supported.

A `user_version` of `0` means "v1, unversioned" — see [Migration](#migration-v1--v2).

### 7.2 Tables

Present in v1 and v2 (v2 additions marked):

| Table | Purpose |
| :--- | :--- |
| `snapshots` | `hash`(PK), `message`, `branch`, `parent_hash`, `merge_parent`, `created_at` — in v2 `created_at_ms INTEGER` |
| `file_map` | tree rows: `snapshot_hash`, `path`, `hash`, `mode` |
| `snapshot_meta` | **v2** — app-namespaced metadata (§5) |
| `branches` | `name`(PK) → `tip`; `tip = ''` means the branch exists but is unborn |
| `tags` | `name`(PK) → `snapshot_hash` |
| `trash` | undone snapshots retained for `redo`, incl. `merge_parent` |
| `trash_tags` | tags shelved by `undo`, restored by `redo` |
| `stash` | named shelves; each points at a snapshot on the internal `_stash` branch |
| `conflict_files` | active merge conflicts: `path`, `ancestor_hash`, `our_hash`, `their_hash` |
| `hunk_decisions` | per-hunk resolutions for a resumable conflict session |
| `index_cache` | `(path, mtime_ns, size, hash)` — change-detection cache, **derived**; safe to delete |
| `remotes` | `name`(PK) → `url` |
| `remote_refs` | last-known remote tips: `(remote, branch)` → `hash` |

Indexes are performance-only and may be rebuilt: `idx_filemap_snap`,
`idx_filemap_path`, `idx_snap_branch`, `idx_trash_branch`, `idx_stash_name`.

**Reserved branch names.** `_stash` is internal. `remotes/<remote>/<branch>` is
remote-tracking. `_deleted_<name>` is a soft-deleted branch. Consumers must not
create branches matching these patterns.

### 7.3 Pragmas

`journal_mode=WAL`, `synchronous=NORMAL`, `foreign_keys=ON`. WAL is required:
readers must not block a writer.

---

## 8. Display vs identity

Ids are stored and compared at **full width**. Truncation exists only for human
output.

- Canonical form: full hex (v2: 64 chars for snapshots and objects).
- Display: implementations may truncate — 12 or 16 characters is conventional.
  This implementation uses **16** (`commands::SNAP_HASH_LEN`) everywhere an id is
  printed, via one shared helper. Under v1, where the stored width was also 16,
  several renderers truncated to 8 instead and the inconsistency was invisible;
  with full-width ids it would have produced a 64-character column.
- **Lookup by prefix is supported**, and an ambiguous prefix must be an error,
  never a silent pick.
- Never persist a truncated id, and never use one as a key, in a bundle, or on
  the wire.

---

## 9. Bundle wire format

Little-endian. Strings are `u32` byte-length followed by UTF-8 bytes.

```
magic      : 8 bytes  "VELOBND1"   (v1)  /  "VELOBND2"  (v2)
version    : u32                    1 (v1) / 2 (v2)
snapshots  : u32 count, then per row:
               hash, message, branch, parent_hash, merge_parent,
               created_at            (v1: string / v2: i64 epoch ms)
file_map   : u32 count, then per row: snapshot_hash, path, hash, i64 mode
meta       : u32 count, then per row: snapshot_id, namespace, key, value   (v2 only)
tags       : u32 count, then per row: name, snapshot_hash
objects    : u32 count, then per row: hash, u32 len, len bytes
               (the raw, already-Zstd-compressed object, verbatim)
```

Rules:

- A reader **must** reject an unknown `version` with a clear error rather than
  guessing.
- A bundle must be **self-contained**: reachability is walked to the root, so
  every included snapshot's parents are included.
- A reader **must** verify every object (§2.3) and recompute every snapshot id
  (§4) before committing the import, and the import must be **idempotent** and
  **transactional**.
- Packs used for sync share this encoding but may legitimately omit objects the
  peer already holds. Only `bundle create` guarantees self-containment.

---

## 10. Decisions

Locked for v2. Recorded with rationale so they are not silently revisited.

| # | Decision | Chosen | Rationale | Cost accepted |
| :--- | :--- | :--- | :--- | :--- |
| D1 | App metadata | **Hashed** (part of snapshot identity) | Metadata is mostly provenance; rewritable provenance is worthless. Keeps `fsck` able to verify it. | Metadata is immutable — changing it makes a new snapshot. |
| D2 | Snapshot id width | **Full 64-hex stored**; truncation is display-only | 64-bit truncation is ~50% collision risk near 5·10⁹ snapshots — thin for a store many apps write to. | Slightly larger DB and wire size. |
| D3 | Timestamps | **Epoch milliseconds (int)**, `DateTime<Utc>` in APIs | Removes text formatting from the identity recipe; no locale/precision can shift an id. | Lexicographic timestamp ordering no longer holds. |
| D4 | Schema versioning | **`PRAGMA user_version`**, refuse-if-newer, `open()` ≠ `open_and_migrate()` | The only thing preventing half-migration and corruption once independent apps share a repo. | Callers must handle a migration step explicitly. |

All four **change snapshot ids** and therefore **land as one atomic format
break**. Splitting them means four id-invalidating migrations.

---

## Migration v1 → v2

Snapshot ids change, so this is not an in-place row rewrite: every id, and every
reference to an id (`parent_hash`, `merge_parent`, tags, stash, remote refs,
branch tips, `PARENT`), must be recomputed.

**Strategy A (re-init) is what shipped.** Opening a pre-v2 repository fails with
`Error::FormatTooOld` from both `open()` and `open_and_migrate()`, and the
refusal leaves `user_version` untouched, so a failed open is never a partial
upgrade. `velo bundle create` cannot help — a v1 bundle carries v1 ids — so
preserve work by copying the working tree into a fresh v2 repository and saving
it there.

A repository written before versioning existed reports `user_version = 0`, which
is why `0` is treated as v1 rather than as "current": before v2 there was nothing
to distinguish the two, and a fresh repository is now stamped at creation so it
can never be mistaken for one.

**B. Rewrite migration** — specified but **not implemented**. Only worth building
if a v1 repository with real history turns up:

1. Refuse if any operation is in progress (`MERGE_HEAD`, `REBASE_STATE`) or the
   working tree is dirty.
2. Topologically order snapshots parents-first.
3. For each, recompute its id under §4.2 (tree unchanged, objects untouched —
   object hashes do not change), building a v1→v2 id map.
4. Rewrite `snapshots`, `file_map`, `branches`, `tags`, `trash`, `trash_tags`,
   `stash`, `remote_refs`, and `PARENT` through the map.
5. Convert `created_at` text → `created_at_ms`; `snapshot_meta` starts empty.
6. Set `user_version = 2`. Whole thing in one transaction.
7. Run `fsck` and refuse to commit if it does not pass.

Objects are format-stable: **no object is rewritten by this migration.**

Remotes are not automatically compatible: a v2 repository cannot sync with a v1
peer, because ids differ. All participants must migrate together.

---

## Non-goals

- **No separate hashed tree object** (à la Git's tree objects). Trees are rows in
  `file_map`. Revisit only if a real need for shared subtree identity appears.
- **No delta/packfile encoding.** Objects are whole-file Zstd. Simplicity beats
  storage efficiency at this scale.
- **No signing.** Content addressing gives tamper-*evidence*, not
  authentication. Signed snapshots would be a v3 discussion.
- **No downgrade path.** Migrations are forward-only.
