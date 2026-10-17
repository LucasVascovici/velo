# Velo repository format

Normative specification of Velo's on-disk format: the object store, the snapshot
identity recipe, the SQLite schema, and the bundle wire format.

Anything that reads or writes a `.velo` directory — the CLI, `velo-core`, or a
third-party tool — must conform to this document.

| | |
| :--- | :--- |
| **Current implemented format** | **v3** (repository format version `3`) |
| Status of v3 | **Implemented.** A layout-only change from v2 (chunked large objects, §2.4): no id, tree, object name or bundle byte changed. See [Migration v2 → v3](#migration-v2--v3). |
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
│                 (or a chunk manifest for large objects, §2.4); empty or absent
│                 when objects live in the database (§2.5)
├── chunks/       v3: deduplicated chunks of large objects, named by BLAKE3 hex
├── HEAD          current branch name (text, no trailing newline required)
├── PARENT        snapshot id the working tree is based on ("" if unborn)
├── lock          advisory lock file (fs2); held by mutating operations
├── lock.holder   who holds the lock: pid, start time (ms), command; informational
├── MERGE_HEAD    present only mid-merge/cherry-pick: "<pre-merge-id>:<source>"
├── MERGE_ORIG    present only mid-merge: exact pre-merge bytes of CR-ending files the merge may rewrite
└── REBASE_STATE  present only mid-rebase (with REBASE_ONTO, REBASE_ORIG_HEAD)
```

`HEAD` and `PARENT` are refs written **atomically** (temp file + rename). A reader
must tolerate a missing or empty `PARENT` (a repository with no commits).

`MERGE_ORIG` is working state like `MERGE_HEAD`, not repository format. Stored
text has its `\r` removed (§2.1), so `merge --abort` could only restore the LF
form of a file the merge rewrote; `merge` therefore keeps the original bytes of
every CR-ending, NUL-free text file it may rewrite or delete. It is a sequence
of records: `u32` LE path length, path (UTF-8, repo-relative, `/` separators),
`u32` LE data length, data. Abort writes a record back only if the restored file
equals it with CRs removed. Older builds ignore the file; a stale one is removed
by the next merge, abort or save.

### Single-file layout

A repository may instead be **one SQLite file**, created by `Repo::create_file`. It has the same schema and the same format version as a directory repository, and its `settings` table holds `layout = single-file` and `objects = database`. Objects are rows of the `objects` table (section 2.5).

- There is no `.velo` directory, no working tree, and no `HEAD` or `PARENT`: the default branch is `main` and nothing is checked out.
- Writers lock `<file>.lock`, a sibling of the database, instead of `.velo/lock`, and record the holder in `<file>.lock.holder`.
- Only store-only operations apply; commands that read or write files refuse with `Unsupported`.
- A database without `layout = single-file`, such as a directory repository's `velo.db`, is not opened as a single-file repository.

---

## 2. Object store

An object is the content of a single file, stored at `.velo/objects/<hash>`
where `<hash>` is the **full 64-hex BLAKE3** of the *uncompressed, normalised*
bytes. The file is in one of two forms (a reader must accept both):

- **(a) a Zstd frame** (level 1) of the full content; or
- **(b) a chunk manifest** (v3), used for large objects, laid out in §2.4.

The two cannot be confused: a Zstd frame starts `28 B5 2F FD`, a manifest starts
`VELOCHK1`.

Object naming is unchanged between v1, v2 and v3: the name is always the hash
of the full content, never of what is on disk.

A v3 repository holds its objects in one of two **locations**, recorded in the
`settings` table (§7.2) and fixed when the repository is created: files, as
described above, or the database (§2.5). Everything else in this document is
independent of the location.

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

For every object, `BLAKE3(content) == file_name`, where `content` is the
decompressed frame (form a) or the concatenation of the manifest's chunks (form
b). `velo fsck` verifies this, and any import (bundle or sync) must verify it
**before** trusting received data.

For a chunked object, every chunk must additionally hash to its own name and
have the length the manifest records, and the reassembled length must equal the
manifest's total. A manifest that is truncated, has the wrong length for its
count, or an unknown version is corrupt. A chunk the manifest names but that is
absent is reported as a missing object carrying the chunk's hex name. `velo
fsck` checks each chunk of every referenced manifest (present, decodable,
hashing to its name) and names a missing or corrupt one precisely, then checks
the reassembled content as for any object.

### 2.4 Chunked objects (v3)

Objects of 1 MiB or more are stored below object identity as content-defined
chunks, so a near-duplicate large file costs only its changed chunks. Trees,
snapshot ids and the bundle/pack wire format do not change: on the wire an
object is always one Zstd frame of its full content.

`.velo/objects/<hash>` is then a manifest:

| Field | Size | Value |
| :--- | :--- | :--- |
| magic | 8 bytes | `VELOCHK1` |
| manifest version | u32 LE | `1` |
| total content length | u64 LE | length of the reassembled content |
| chunk count | u32 LE | number of entries that follow |
| per chunk | 32 bytes + u32 LE | the chunk's raw BLAKE3, then its length |

Chunks live at `.velo/chunks/<hex>`. Each is a Zstd frame (level 1) of the
chunk's bytes, named by the BLAKE3 of the uncompressed chunk. A writer stores
every missing chunk **before** the manifest, so a manifest never names a chunk
that was not written. The chunking algorithm and size threshold are not part of
the format; this implementation uses FastCDC (16 KiB / 64 KiB / 256 KiB) for
objects of 1 MiB or more.

A chunk is live only while some object's manifest names it. Unreferenced chunks
are collectable: `velo gc` removes every chunk that no surviving manifest lists
(a chunk shared with a surviving object always stays), and `velo fsck` reports
them as cruft, not corruption, which `--repair` removes.

---

### 2.5 Objects in the database (v3)

When the `settings` row `objects` is `database`, objects are rows of the
`objects` table (§7.2) instead of files: `hash` is the same 64-hex BLAKE3 of
the full normalised content, and `data` is one Zstd frame (level 1) of it, i.e.
exactly form (a) of §2. There is no chunking in this location: chunk
deduplication is a disk-layout optimisation, and in the database the unit of
storage is the SQLite page. `.velo/objects/` and `.velo/chunks/` are unused.

The integrity invariant (§2.3) is unchanged: `data` must decode and hash to its
row's `hash`. `velo gc` deletes rows no snapshot references, `velo fsck`
verifies them, and bundles and sync carry the same frames either way, so a
bundle from one location applies into the other with identical snapshot ids.

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
`file_map`, not as a separate hashed tree object. From v4 a tree may instead be
stored as a delta against its parent's tree (§11.5); what a tree *is* is
unchanged. A snapshot has exactly one
`file_map` row per path; `velo fsck --repair` removes exact duplicate rows (a data
repair, not a format change).

Mode semantics: the executable bit is only observable on Unix. On platforms that
cannot observe it, an implementation must **carry the parent's mode forward**
rather than resetting to `0`. Symlink creation may fall back to writing a regular
file containing the target text where the platform forbids symlinks.

### 3.1 Paths

A snapshot id commits to its rows whatever they say, so the id cannot vouch for
a path. These rules are normative.

**Store rule** (the same on every platform). A path is stored with forward
slashes and is refused if it is empty; starts with `/`; has a first component
of the form `X:` (a drive); has an empty, `.` or `..` component (so no
trailing slash); contains a NUL byte; or has any component equal to `.velo`
ignoring case and trailing dots or spaces.

**Platform rule** (applied when a path is written to a working tree). The store
rule, plus on Windows: the device names `CON`, `PRN`, `AUX`, `NUL`, `COM1`-`COM9`
and `LPT1`-`LPT9` in any case, with or without an extension; the characters
`< > : " | ? *`, backslash and control characters 0x01-0x1F (`:` also covers
alternate data streams); a component ending in a dot or a space; and the 8.3
alias `velo~1`. A repository holding `aux.c` is therefore valid on Linux and is
refused only when written out on Windows.

**Exclusivity.** No path may be both a file in one entry and a directory
prefix of another entry of the same tree (`a` and `a/b`).

**Symlinks.** A symlink entry is refused if its target is absolute (`/x`,
`C:/x`, a UNC path) or, resolved lexically from the link's own directory,
climbs above the root. A working-tree write is refused if any existing
directory on the way to the target is a symlink.

**Where it is enforced.** Importers (`save_tree`, git import, bundle apply,
fetch, pull, clone and the push receive side) must refuse an offending path
with `UnsafePath` *before committing or storing anything*. Every writer of a
working tree (restore, switch, undo, redo, merge, cherry-pick, rebase, stash,
resolve, clone checkout) validates the whole target tree before writing the
first file. A repository that already holds a refused path stays readable;
only writing it out is refused.

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
| **v3** | Same marker, stamped `3`. Adds the `.velo/chunks/` directory and the additive `settings` and `objects` tables (§7.2); a v3 repository may keep its objects in files or in the database. |
| **v4** | *Planned, 6.0, not implemented.* Adds the `delta_base` and `file_delta` tables for snapshot storage (§11.5); a snapshot with no `delta_base` row is a keyframe, so existing `file_map` rows stay valid; `user_version = 4` makes older builds refuse the repository. |

The v1 `ALTER TABLE` sniffing migrations are gone. They existed only to bring a v1
repository forward, and v2 refuses to open one, so keeping them would have meant
maintaining a chain of migrations nothing could reach. The schema is now one
idempotent definition, which is also the migration for a v2 repository written by
an earlier build of v2.

**v2 rules — normative:**

- `user_version = 3` for this specification.
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
| `renames` | **v2** — rename edges: `(snapshot_hash, to_path)`(PK), `from_path` (§7.3) |
| `pending_renames` | **v2** — working-tree moves awaiting a save: `to_path`(PK), `from_path`; **derived**, safe to delete |
| `settings` | **v3, additive** — `key`(PK) → `value`. The one key today is `objects`: `database` when objects live in the `objects` table (§2.5). A missing row, or a missing table, means files, which is every repository created before the setting existed. Written once at creation |
| `objects` | **v3, additive** — `hash`(PK), `data` BLOB: one Zstd frame of the full object content (§2.5). Empty unless `settings.objects` is `database` |
| `compactions` | **v3, additive** — `old_hash`(PK) → `new_hash`, `compacted_at_ms`: one row per snapshot id that compaction removed or re-minted (§11.3); **local only**, never collected by `gc`, not in bundles |
| `signatures` | **v3, additive** — `(snapshot_hash, public_key)`(PK), `algorithm`, `signature`: Ed25519 signatures over snapshot ids, hex (§11.1). Outside identity; a missing table means no signatures. `fsck` verifies every row |
| `delta_base` | **v4** — `snapshot_hash`(PK) → `base_hash`: one row per snapshot stored as a delta against its first parent; no row means keyframe (§11.5). *Planned, not implemented* |
| `file_delta` | **v4** — `(snapshot_hash, path)`(PK, `WITHOUT ROWID`), `hash`, `mode`: the changes a delta applies to its base's tree; `hash = ''` removes the path (§11.5). *Planned, not implemented* |

Indexes are performance-only and may be rebuilt: `idx_filemap_snap`,
`idx_filemap_path`, `idx_snap_branch`, `idx_trash_branch`, `idx_stash_name`,
`idx_renames_to`, `idx_meta_lookup` (`snapshot_meta (namespace, key, value)`,
for metadata queries), and from v4 `idx_delta_base_base` (`delta_base
(base_hash)`) and `idx_file_delta_path` (`file_delta (path)`).

**Reserved branch names.** `_stash` is internal. `remotes/<remote>/<branch>` is
remote-tracking. `_deleted_<name>` is a soft-deleted branch. Consumers must not
create branches matching these patterns.

`remote_refs` is authoritative for remote-tracking tips: `remotes/<remote>/<branch>`
and `<remote>/<branch>` both resolve through it. The `remotes/<remote>/<branch>`
value in `snapshots.branch` records where a fetched snapshot arrived; it is
display context only and is never used to resolve a ref.

### 7.3 Rename edges

A snapshot is a whole tree, so a move is indistinguishable from a delete plus an
add once it has happened. `renames` records the move at the moment it is made,
and is the only place that fact exists.

- **Not part of snapshot identity.** Identity answers what a tree *is*, and two
  identical trees are the same snapshot however each was arrived at. Adding
  edges to the recipe would also mean an identical re-save could collide with a
  different claim about how it came to be.
- **A consequence:** nothing about the id proves an edge is true. `fsck` checks
  them structurally instead — `to_path` must be in the snapshot, and `from_path`
  in one of its parents.
- **Never inferred.** Velo does not guess at moves from content similarity. A
  file moved without an edge reads as a delete and an add, which is what the
  stored trees actually say.
- **Optional throughout.** A repository with no edges is well-formed, and is
  what every repository written before this existed looks like.

`pending_renames` is the working-tree half: `velo mv` writes it, the next `velo
save` moves the applicable rows into `renames`, and any command that rewrites
the tree wholesale clears it. Nothing in history depends on it, so a consumer
may ignore or delete it.

### 7.4 Pragmas

`journal_mode=WAL`, `synchronous=NORMAL`, `foreign_keys=ON`. WAL is required:
readers must not block a writer.

`auto_vacuum=INCREMENTAL` is set on repositories created from 5.3.0, before the
first table exists, so freed pages can be handed back with
`PRAGMA incremental_vacuum` (`gc::vacuum`). A repository created earlier stays
`auto_vacuum=NONE` until a full `VACUUM` converts it, once. Opening never
rewrites a repository, `user_version` does not change, and every SQLite build
reads both modes, so older velo builds open new repositories unchanged. Any
later schema migration needs no `VACUUM` for this reason.

**Durability.** With `synchronous=NORMAL` in WAL mode a committed transaction is
not forced to disk at commit, so a power loss may roll back the most recent
commits; the database stays consistent. Files outside the database
(`.velo/PARENT`, `.velo/HEAD`, objects) are written by temp-file-and-rename
without an `fsync`. What a crash or power loss can leave, and how it is tested,
is in [CRASH_SAFETY.md](CRASH_SAFETY.md).

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
magic      : 8 bytes  "VELOBND1"   (v1)  /  "VELOBND2"  (v2, v3 and v4)
version    : u32                    1 (v1) / 2 / 3 / 4
snapshots  : u32 count, then per row:
               hash, message, branch, parent_hash, merge_parent,
               created_at            (v1: string / v2: i64 epoch ms)
file_map   : u32 count, then per row: snapshot_hash, path, hash, i64 mode
meta       : u32 count, then per row: snapshot_id, namespace, key, value   (v2+)
tags       : u32 count, then per row: name, snapshot_hash
objects    : u32 count, then per row: hash, u32 len, len bytes
               (the raw, already-Zstd-compressed object, verbatim)
renames    : u32 count, then per row: snapshot_hash, from_path, to_path    (v3+)
signatures : u32 count, then per row: snapshot_hash, algorithm,
               public_key, signature                                      (v4 only)
```

Version 3 appends `renames` after `objects` and changes nothing before it, so a
version-2 bundle is a version-3 bundle with no edges. Readers accept both; the
section is appended rather than slotted in beside the other tables precisely so
that stays true.

Rename edges travel even though they are not part of a snapshot's identity —
the opposite of the reason metadata does. A receiver without metadata recomputes
a different id and rejects the import; a receiver without edges recomputes the
*same* ids, accepts happily, and then reports a file's whole history as
belonging to whoever moved it. Silence, not an error, which is why they are
carried. Edges naming snapshots the receiver does not hold are dropped on
import: storing one would manufacture exactly the problem §7.4 has `fsck`
report.

Version 4 appends `signatures` (§11.1) after `renames` and changes nothing
before it. It is written **exactly when the bundle carries at least one
signature**; a bundle with none is version 3, byte for byte as before, so an
unsigned repository's bundles stay readable by 5.1.x. A v3-only reader refuses a
version-4 bundle, which is the point: it would otherwise import the history and
silently drop the signatures. Rows for snapshots the receiver does not hold are
dropped on import, as for renames. Sync packs send only the snapshots the peer
lacks, so a countersignature added later to an already-shared snapshot is not
re-sent by sync; a `bundle create` of that history carries it.

Rules:

- A reader **must** reject a `version` it does not understand with a clear error
  rather than guessing. Accepting an *older* version it does understand is
  correct and expected.
- A bundle must be **self-contained**: reachability is walked to the root, so
  every included snapshot's parents are included.
- A reader **must** verify every object (§2.3) and recompute every snapshot id
  (§4) before committing the import, and the import must be **idempotent** and
  **transactional**.
- Packs used for sync share this encoding but may legitimately omit objects the
  peer already holds. Only `bundle create` guarantees self-containment.
- A reader **must** bound every allocation it makes on the strength of a number
  in the data: a section count must fit in the bytes that remain (at least 28,
  20, 16, 8, 8 and 12 bytes per snapshot, file_map, meta, tag, object and rename
  row), a string length and a decompressed object size must stay under explicit
  ceilings, and the whole bundle must too. `velo-core` defaults to 16 MiB per
  string, 1 GiB per decompressed object and 16 GiB per bundle, all configurable
  on the handle (`Repo::limited`). Refusal is the typed `LimitExceeded` error
  (a count that cannot fit is reported as a truncated bundle). The ceilings are
  a reader's policy and are **not part of the format**: another implementation
  may choose other values.

### 9.1 Packs over HTTP

The HTTP transport carries packs with exactly this encoding; nothing about the
bundle changes. Under `<base>/velo/v1/`: `GET refs` returns a refs block;
`POST upload` takes the client's have-ids (length-prefixed strings) and returns
a refs block followed by the pack; `POST receive` takes branch and new tip
(length-prefixed) followed by the pack and returns a length-prefixed status
(`OK <snapshots> <objects>` or `REJECT <reason>`). A refused push is a `200`
carrying `REJECT`, as over ssh; other non-2xx codes are failures.
Fast-forward-only semantics are unchanged.

**Addressing.** A server holding one repository answers at `<any prefix>/velo/v1/`.
A server holding a directory of repositories answers at
`/<repo path>/velo/v1/`, where `<repo path>` is a relative path under the
served directory. The repository path follows the tree-path rule: no `..`,
no `.velo` component, no empty segment, no absolute or backslash form, and no
`%` at all (the server never percent-decodes). A violation is `400 bad-path`;
a path that is safe but names no repository is `404 not-found`.

**Authentication.** A request may carry `Authorization: Bearer <token>`; the
scheme is case-insensitive. A server with tokens configured compares the token
in constant time against every configured token. A read token permits `refs`
and `upload`; a write token also permits `receive`. A host may replace this
check with its own, deciding per repository and operation.

**Refusals.** Every refusal is a one-line `text/plain` body,
`velo: <kind>: <detail>\n`:

| Status | Kind | Meaning |
| :--- | :--- | :--- |
| 400 | `bad-path` | unsafe or malformed repository path |
| 401 | `unauthenticated` | no valid token; carries `WWW-Authenticate: Bearer` |
| 403 | `forbidden` | valid token that does not permit the operation |
| 403 | `read-only` | `receive` on a server that only serves fetch |
| 404 | `not-found` | unknown endpoint or repository |
| 405 | `method-not-allowed` | wrong method for the endpoint (`GET refs`, `POST upload`/`receive`) |
| 413 | `too-large` | request body or a received object exceeds a limit |
| 500 | `error` | any other server-side failure |
| 503 | `busy` | the repository is locked by another writer |

Checks run in the order route, repository, authorization, read-only, size, so
an unauthenticated caller learns nothing about size limits. A refused push never
changes the repository.

---

## 10. Decisions

Locked for v2. Recorded with rationale so they are not silently revisited.

| # | Decision | Chosen | Rationale | Cost accepted |
| :--- | :--- | :--- | :--- | :--- |
| D1 | App metadata | **Hashed** (part of snapshot identity) | Metadata is mostly provenance; rewritable provenance is worthless. Keeps `fsck` able to verify it. | Metadata is immutable — changing it makes a new snapshot. |
| D2 | Snapshot id width | **Full 64-hex stored**; truncation is display-only | 64-bit truncation is ~50% collision risk near 5·10⁹ snapshots — thin for a store many apps write to. | Slightly larger DB and wire size. |
| D3 | Timestamps | **Epoch milliseconds (int)**, `DateTime<Utc>` in APIs | Removes text formatting from the identity recipe; no locale/precision can shift an id. | Lexicographic timestamp ordering no longer holds. |
| D4 | Schema versioning | **`PRAGMA user_version`**, refuse-if-newer, `open()` ≠ `open_and_migrate()` | The only thing preventing half-migration and corruption once independent apps share a repo. | Callers must handle a migration step explicitly. |
| D5 | Signatures | **Ed25519 over the snapshot id**, stored **outside identity** (§11.1) | The id already commits to everything worth signing; outside identity, a countersignature never mints a snapshot. | A dropped signature cannot be detected from the id, so bundle version 4 must make older readers refuse. |
| D6 | Chunked objects | **Chunking below object identity** (§11.2) | An object's hash stays the hash of its full content, so no tree, snapshot id or wire reader changes; "objects are format-stable" survives. | A new on-disk form, so repository format v3 (older builds refuse it). |
| D7 | Compaction record | **Local `compactions` table, old id → new id** (§11.3) | A consumer holding an old id gets `Compacted { into }` rather than `NotFound`. | A table `gc` must never collect; not carried by sync. |

D1-D4 all **change snapshot ids** and therefore **land as one atomic format
break**. Splitting them means four id-invalidating migrations. D5-D7 change no
id: they are decided here, ahead of any code, and specified in §11.

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

## Migration v2 → v3

Additive and in place. `open_and_migrate` creates `.velo/chunks/` and stamps
`user_version = 3`; `open` on a v2 repository returns `MigrationRequired`.
**No snapshot id, tree, row or object is rewritten**: existing objects stay
valid as form (a) frames, and only objects stored from then on may be chunked.
Peers need not migrate together, since the bundle and sync wire formats are
unchanged. Older builds refuse a v3 repository with `SchemaTooNew`, per §7.1.

A v3 repository may hold its objects in files or in the database (§2.5); the
choice is recorded in `settings` when it is created. Migration creates the
`settings` and `objects` tables (`CREATE TABLE IF NOT EXISTS`, so it is
idempotent) but never writes a setting row: an existing repository has none and
stays in the files location.

---

## 11. Decided, not yet implemented

Apart from D6 (§11.2) and D7 (§11.3), both implemented, and the storage half
of D5 (§11.1), nothing in this section is written or read by the current code. These are decisions
taken before any code exists, because each is cheap now and a format break
later. Later work implements them to the letter.

### 11.1 Signatures (D5) — implemented

- **What is signed:** the snapshot id. The signed message is the ASCII bytes
  `velo-signature-v1\n` followed by the 64-char lowercase hex id. Nothing else
  is needed: the id already commits to the tree, parents, message, timestamp,
  metadata and author.
- **Algorithm:** Ed25519 (RFC 8032, pure, no prehash). The public key (32 bytes)
  and the signature (64 bytes) are stored as lowercase hex. An `algorithm`
  column holds `ed25519`, so another algorithm can be added later without a
  break. A reader must treat an unknown algorithm as *unverifiable*, never as
  valid.
- **Outside identity, like rename edges (§7.3).** A countersignature adds a row
  and never mints a new snapshot. The consequence is the same as for rename
  edges: a dropped signature cannot be detected from the id.
- **Storage (implemented, `velo_core::sign`):**

  ```sql
  signatures (
      snapshot_hash TEXT NOT NULL,
      algorithm     TEXT NOT NULL,
      public_key    TEXT NOT NULL,
      signature     TEXT NOT NULL,
      PRIMARY KEY (snapshot_hash, public_key)
  )
  ```

  The table is additive and changes no id. A reader of a repository without
  the table treats it as empty; `fsck` reports a row that fails strict
  verification as corruption, ignores unknown-algorithm rows, and treats a row
  naming a snapshot in neither `snapshots` nor `trash` as cruft.
- **Transport:** bundle wire version 4 appends a `signatures` section after
  `renames` (u32 count; per row: snapshot_hash, algorithm, public_key,
  signature). Version 4 is written **only when the bundle carries at least one
  signature**, so an unsigned repository's bundles stay version 3. A v3-only
  reader still refuses any bundle that carries signatures, instead of importing
  it and silently dropping them. Readers verify every signature before committing an
  import, and one invalid signature rejects the whole import (`UntrustedData`).
  Rows for snapshots the receiver does not hold are dropped, as for renames.
  Sync packs share the encoding.
- **Trust:** velo verifies signatures; which keys to trust is the caller's
  policy. Velo stores no private keys, and signing takes the key per call.

### 11.2 Chunked objects, below object identity (D6) — implemented in v3

Implemented; the normative layout is §2 and §2.4, and the migration is
[v2 → v3](#migration-v2--v3). The decision stands as recorded in D6: an
object's name stays the BLAKE3 of its full content, chunking is a storage
detail, and the bundle wire format does not change.

### 11.3 What compaction leaves behind (D7) — implemented

Implemented by `commands::compact` (store only). The `compactions` table is
created by the schema script like any other, with no version bump: it is
additive and an older v3 build simply never reads it. Compaction writes every
removed and re-minted id in the same transaction as the rewrite, and refuses
(`InvalidInput`) rather than rewriting anything the eligibility rules below
protect; signed-snapshot protection arrives with signatures. No `Saved` events
are emitted for re-mints; `RefMoved` is emitted for the branch and for each
retargeted tag.

Compaction squashes a range of snapshots into one. Every descendant on the
rewritten chain gets a new parent and therefore a new id (it is *re-minted*).

- **Record:**

  ```sql
  compactions (
      old_hash        TEXT PRIMARY KEY,
      new_hash        TEXT NOT NULL,
      compacted_at_ms INTEGER NOT NULL
  )
  ```

  One row per id that stopped existing. Squashed members map to the snapshot
  they became; re-minted descendants map to their new id. Lookups follow
  chains: if `new_hash` was itself compacted later, follow on to the live id.
- **Lookup contract:** an exact id, or a unique prefix that matches no live
  snapshot, that appears as `old_hash` fails with
  `Error::Compacted { id, into }`, where `into` is the live id at the end of
  the chain. It never fails with `NotFound`.
- **Local only.** The record is not part of identity and is not carried in
  bundles or sync. Compaction refuses to rewrite anything reachable from a
  remote-tracking ref, so a rewritten id has never travelled by velo sync. A
  bundle made earlier may still carry old ids; the receiver simply holds them as
  live snapshots.
- **Eligibility:**
  - Never *squashed away*: tagged snapshots; merge snapshots (the second parent
    must survive); a snapshot that is the parent or merge parent of a snapshot
    outside the rewrite; anything reachable from a remote-tracking ref; the
    snapshot `.velo/PARENT` names, unless it is the newest member of its range.
  - Never squashed **or** re-minted: signed snapshots, because re-minting
    invalidates the signature. A compaction whose range contains a signed
    snapshot, or whose rewrite would re-mint a signed descendant, is refused
    and changes nothing. Retention treats a signed snapshot as a barrier like a
    published one: only snapshots newer than the newest signed snapshot on the
    chain are eligible, and it is reported as protected (`signed`).
  - Other id-rewriting commands keep the same rule and refuse with
    `InvalidInput` before writing anything: `squash` when a signed snapshot is
    in the squashed range, `rebase` when it would replay a signed snapshot,
    and `save --amend` when the tip is signed. `undo` does not rewrite, so it
    stays allowed; `redo` restores the snapshot.
  - `gc` never deletes `signatures` rows, and does not expire a trash entry
    whose snapshot is signed, so an undone signed snapshot stays recoverable by
    `redo`.
  - Tagged snapshots *may* be re-minted as descendants. The tag is retargeted to
    the new id and the move is recorded.
- **The squashed snapshot** takes the newest member's tree, message, timestamp
  and metadata (including the author). Its parent is the oldest member's
  parent. Rename edges are composed across the range.
- `gc` never collects `compactions` rows.

### 11.4 Decided: tree metadata growth

Every snapshot stores its whole tree in `file_map`, so rows grow as
snapshots x files even when one file changed. Phase 15.9 measured this and
changed nothing; the storage is as §7.2 describes. Reproduce with
`cargo test -p velo-core metadata_growth -- --ignored --nocapture`.

**Measured** (1000 files, 200 snapshots, one file changed per snapshot; Intel
Core i7-1370P, Windows 11, release-less `cargo test` profile, SQLite via
rusqlite, tempdir on NTFS; two runs):

| Quantity | Run 1 | Run 2 |
| --- | --- | --- |
| `file_map` rows | 200,000 | 200,000 |
| database size | 55,390,208 bytes | 55,390,208 bytes |
| save, first 50 snapshots | 134.7 ms/save | 294.5 ms/save |
| save, last 50 snapshots | 305.3 ms/save | 137.7 ms/save |
| 1000-row `INSERT` loop (rolled back) | 15.3 ms | 5.4 ms |
| `INSERT ... SELECT` of 999 rows + 1 insert (rolled back) | 46.1 ms | 13.8 ms |

Reading the numbers: save latency shows no growth trend (the two runs disagree
about which end is slower, so the spread is machine noise, not table size). A
save is dominated by hashing and writing objects, not by the row insert. The
row insert is 5-15 ms of a ~140-300 ms save. About 277 bytes of database per
row, all in. The `INSERT ... SELECT` carry-forward was **not** faster here
(2.5-3x slower): at 1000 rows the per-row Rust crossing is not the cost, and
the select reads through `idx_filemap_snap` in non-insertion order. Its case
rests on avoiding the in-Rust tree at larger sizes, which this run does not
show.

**Who uses `idx_filemap_path`** (from `EXPLAIN QUERY PLAN`; the plan depends on
the planner's choice with no `ANALYZE` statistics, so re-run it before relying
on this):

- `blame.rs`: `WHERE snapshot_hash = ? AND path = ?` plans as `SEARCH ... USING
  INDEX idx_filemap_path (path=?)`.
- `tree.rs` `Repo::read_file_at` (public API): the same statement as
  `blame.rs`, same plan.
- `diff.rs` (stored hash of a path against `PARENT`): `WHERE path = ? AND
  snapshot_hash = ?`, same plan.
- `fsck.rs` (does a snapshot hold a path): same plan.
- `gc.rs` stale `index_cache` prune: `NOT EXISTS (... WHERE file_map.path =
  index_cache.path ...)` is path-keyed on purpose and probes
  `idx_filemap_path`.

Everything else uses `idx_filemap_snap` or scans: tree loads (`apply`,
`compact`, `merge`, `restore`, `save`, `commands::mod`, `grep`,
`diff` `load_file_map`, and `tree.rs` `Repo::tree_at`, whose `ORDER BY path`
still plans through `idx_filemap_snap` plus a temp B-tree sort), `tracks_path`,
`history --file` (both the parent comparison and the root-snapshot check, which
plans through `idx_filemap_snap`), the `gc` orphan
sweep. `fsck` reads the whole table in one `ORDER BY snapshot_hash` scan
(grouped per snapshot in Rust), `gc` runs `SELECT hash` and dedups in Rust, and
the bundle export scans the whole table. `mv.rs` and `paths.rs` do not read `file_map`.
Dropping `idx_filemap_path` would therefore turn those four point lookups into
a scan of the snapshot's rows via `idx_filemap_snap` (about one tree), not of
the table; a composite `(snapshot_hash, path)` index would serve them better
and is the `WITHOUT ROWID` option below.

**Cheap, format-compatible options**, in order:

1. Carry unchanged paths forward with one `INSERT ... SELECT`, so those rows
   never cross into Rust. Measured gain at this size: none (see above).
2. Store hashes as 32-byte blobs rather than 64-character hex.
3. `WITHOUT ROWID` keyed on `(snapshot_hash, path)`.

**Structural option:** a tree-object or delta model, where a snapshot names a
tree object and unchanged subtrees are shared. That changes the format and
bundles.

**Round three** (release build, 2,000 files x 300 snapshots, one file changed
per snapshot, so about 600,000 `file_map` rows; run through the CLI on an idle
machine):

| Quantity | Round three |
| --- | --- |
| `file_map` rows | about 600,000 (2,000 x 300) |
| `.velo` size | 174 MB, against 304 MB for the equivalent git repository |
| `.velo` per snapshot | 0.58 MB, about twice round one's per-snapshot figure |
| `fsck` before the 17.11 change | 2.65 s: objects 0.62 s, snapshot ids 1.0 s, duplicate-row count 0.95 s |
| `fsck` after | 0.8-1.0 s: objects 0.34 s, snapshot ids 0.35 s, duplicate-row count 0 s |
| `gc` before | 0.89 s: database statements 0.64 s |
| `gc` after | 0.31 s: database statements 0.11 s |

This run did not break the 174 MB down by table. The `fsck` and
`gc` cost scales with the same rows: both read every one. Their fix was in how
the rows are read (one ordered scan, parallel id hashing, no `DISTINCT` over
the whole table), not in how they are stored.

**Round three left the decision open and changed nothing in the format.** No column,
index or row layout was altered by this measurement; the schema is as section 7.2
describes. A change that stored a tree once and shared it between snapshots
that did not touch it would cut the rows and the size, and would also change
`fsck`, `gc`, bundles and sync, so it needs its own phase.

**Round four: pack import (17.8-a).** `bundle apply` and `clone` at 2,000 files
x 300 snapshots (about 600,000 rows, a 165 MB `velo.db`) spent their time as
follows in `bundle::import`, release build, one run each, NTFS with Defender on:

| Phase | Before | After |
| --- | --- | --- |
| objects: verify and write 2,600 files | 14-16 s | 5.6-6.7 s |
| snapshot id verification | 0.1 s | 0.1 s |
| `file_map` inserts with both indexes | 6.4-8 s | 6.4-8 s |
| commit (WAL write and checkpoint) | 4-5 s | 4-5 s |

The object phase was the avoidable part: many rayon threads creating small
files at once took about 100 ms per file where one thread takes 2.5 ms, so the
writes now run on the calling thread after the parallel verification. Four
writer threads were measured and were no better than the full pool. That is
about a 1.4x improvement of `bundle apply` (roughly 27 s to 20 s on a noisy
machine), not the 2x aimed at.

**The floor is SQLite.** Dropping `idx_filemap_snap` and `idx_filemap_path`
for the bulk insert and recreating them in the same transaction made the
inserts take 1.1 s but the index build 4-5 s, a wash (11 s against 11.5 s), so
it was not kept. A 512 MB page cache saved about one second. What remains is
writing 600,000 rows and two indexes over long repeated path strings, then
checkpointing 165 MB. Going below it means fewer rows or smaller rows, which is
the decision below: share trees between snapshots, store hashes as blobs, or
key `file_map` on `(snapshot_hash, path)` `WITHOUT ROWID`. Nothing in the
format changed.

**Round five (18.10): the format-compatible options at envelope sizes.**
Measured with the `#[ignore]`d `round_five_options_at_*_rows` tests in
`crates/velo-core/src/tests/metadata_growth.rs` (`cargo test -p velo-core
--release metadata_growth -- --ignored --nocapture --test-threads=1`) and the
`bench/scale/scale.py` harness. Machine: Intel Core i7-1370P (Family 6 Model
186), Windows 11, NTFS with Defender real-time scanning on, release build,
SQLite via rusqlite. Two runs each. Options 2 and 3 are measured on scratch
tables, never the real schema; one parent snapshot of N rows, 10 snapshots for
the scratch tables.

| Option | Rows | Run 1 | Run 2 |
| --- | --- | --- | --- |
| 1. read tree + per-row insert loop | 10,000 | 25.1 ms | 25.3 ms |
| 1. `INSERT ... SELECT` + 1 insert | 10,000 | 21.8 ms | 21.2 ms |
| 1. read tree + per-row insert loop | 100,000 | 324.7 ms | 345.0 ms |
| 1. `INSERT ... SELECT` + 1 insert | 100,000 | 286.1 ms | 254.3 ms |
| 2. hex TEXT, bytes per row / insert | 100,000 | 290.1 B / 1.05 s | 290.1 B / 1.07 s |
| 2. 32-byte BLOB, bytes per row / insert | 100,000 | 184.0 B / 0.85 s | 184.0 B / 0.81 s |
| 2. hex TEXT, bytes per row / insert | 10,000 | 287.1 B / 78 ms | 287.1 B / 80 ms |
| 2. 32-byte BLOB, bytes per row / insert | 10,000 | 178.6 B / 48 ms | 178.6 B / 47 ms |
| 3. rowid + 2 indexes: B/row, insert, lookup | 100,000 | 290.1 B, 1.05 s, 88 us | 290.1 B, 1.00 s, 104 us |
| 3. `WITHOUT ROWID`: B/row, insert, lookup | 100,000 | 191.5 B, 0.40 s, 47 us | 191.5 B, 0.37 s, 46 us |
| 3. rowid + 2 indexes: B/row, insert, lookup | 10,000 | 287.1 B, 73 ms, 55 us | 287.1 B, 74 ms, 52 us |
| 3. `WITHOUT ROWID`: B/row, insert, lookup | 10,000 | 186.4 B, 51 ms, 47 us | 186.4 B, 65 ms, 48 us |

Whole-save latency, `scale.py --files 100000 --snapshots 20` (one file changed
per snapshot; 2,000,020 `file_map` rows afterwards): the first save (every file
new) took 483 s and 491 s; the median of saves 2-20 was 3.67 s and 3.66 s, and
the median of the last 10% (two saves) 5.4 s and 5.3 s.

**Carry-forward decision.** The rule was: adopt `INSERT ... SELECT` in `save`
only if it makes a one-file-changed save at 100,000 files at least 20% faster
end to end. It saves at most about 60-90 ms of a 3.7-5.4 s save, under 3%, so
it does not clear the rule and `commands/save.rs` and `tree.rs` are unchanged.
The whole row insert is about 0.3 s of the save; the rest is scanning, hashing
and writing. Carry-forward would also give up the in-Rust tree the save needs
anyway.

Options 2 and 3 change column types or the table definition, so they are format
changes and are **recorded, not implemented**: 32-byte blobs cut about 36% of
the bytes per row and `WITHOUT ROWID` about 34%, with `WITHOUT ROWID` also
inserting 2.6x faster and halving point lookups. Whether that is worth a format
change is the overall decision of 18.10-c.

**Round six (18.10-c): the envelope and the retention policy.** Published with
its method in [SCALE.md](SCALE.md). The harness (`bench/scale/scale.py`, release
CLI, one file changed per snapshot) reached 10,000 files x 2,000 snapshots and
100,000 files x 200 snapshots, 20 million `file_map` rows (a 5.3 GiB database)
each, on Windows 11 and on Linux (WSL2). What the numbers say:

- Reading is not the problem: `history`, `history --file`, open and `restore`
  stay at or under 3 s and mostly under 120 ms at 20 million rows.
- Save grows with the table: at 10,000 files it levels at about 1.9 s; at
  100,000 files it rose from 3.6 s to 14.5 s (Windows) and from 3.0 s to
  20.2 s (Linux) over 200 snapshots and had not levelled.
- `fsck` takes 26-38 s on Linux and 31-127 s on Windows at 20 million rows;
  `gc` 5-8 s on Linux, 6.5 s and 392 s on Windows (Defender scanning, not rows).
- The default retention policy (everything for a day, hourly for a month, daily
  after) applied to 2,000 snapshots of a 10,000-file tree over 60 days cut
  snapshots, rows and live content by 62% (20.0 million to 7.6 million rows,
  5.17 GiB to 1.99 GiB live). The database file did not shrink, because SQLite
  keeps the freed pages and velo has no `VACUUM`.

**Decision.** Metadata growth is bounded by retention, not by a format change,
for the agent-checkpointing workload that produces the long histories:

1. **Retention bounds the rows.** The default policy is the documented
   recommendation, and rows are then limited by the policy (a day of
   snapshots, at most 24 a day for a month, one a day beyond) rather than by
   how long the agent has run. Run it daily; one pass over 20 million rows took
   20 minutes.
2. **Carry-forward `INSERT ... SELECT` is not adopted** (18.10-b: under 3% of a
   100,000-file save). `save` keeps the per-row insert loop.
3. **32-byte blob hashes and `WITHOUT ROWID` are deferred.** They would cut about
   a third of the bytes per row (36% and 34%) and `WITHOUT ROWID` would also
   insert 2.6x faster, but both change the on-disk format and the bundle
   layout, and neither changes the shape of the growth: a snapshot still owns
   one row per file. They are recorded here, not scheduled.
4. **A tree-object model is a 6.0 conversation**, held in §11.5 (which chose
   parent deltas with keyframes, not a tree object), to be had with the golden
   corpus (Phase 18.3) in hand so that a migration can be proven against every
   released format. It is the only option that removes the files x snapshots
   product itself, and the 100,000-file save latency above is the case for it.
   Until then the stated envelope in SCALE.md is the claim: beyond 20 million
   rows or 100,000 files, velo has not been measured.

### 11.5 Decided: what a snapshot stores (Phase 19.5)

Decision: **option A, parent deltas with a full keyframe every K = 128
snapshots, identity unchanged.** It ships as format v4 in 6.0. This section is
normative for that change; nothing here is implemented yet.

**The candidates, with the prototype numbers** (100,000 files, one file
changed per save, 200 saves per K; Windows / Linux; raw lines in the
repository's benchmark results and the "Snapshot storage prototype" section of
the performance notes).

| | save, median | disk per snapshot | `tree_at` | point read at worst depth |
| --- | ---: | ---: | ---: | ---: |
| today (full rows) | 5,336 / 1,750 ms | 27,868,365 B | 83.9 / 100.2 ms | 0.35 / 0.10 ms |
| A, K=8 | 94.8 / 83.0 ms | 2,306,314 B | 59 / 51 ms | 0.059 / 0.023 ms |
| A, K=32 | 110.5 / 95.9 ms | 559,985 B | 85 / 60 ms | 0.28 / 0.13 ms |
| **A, K=128** | **110.2 / 89.6 ms** | **93,655 B** | **83 / 62 ms** | **1.43 / 0.47 ms** |
| A, K=512 | 110.2 / 100.5 ms | 35,739 B | 98 / 75 ms | 4.05 / 1.75 ms |

- **A** removes the row writes (0.07 ms) and the files x snapshots growth: a
  save is 16x (Linux) to 48x (Windows) faster at K=128 and a snapshot is 300x
  smaller on disk. `tree_at` does not grow with K up to 512, because the
  keyframe read (50 to 80 ms) dominates the one-row deltas. A point read walks
  at most K-1 deltas, so it grows linearly with depth.
- **B** (a tree object per directory) rewrites the entries of the changed
  directory plus one per ancestor: 52 rows for the agent benchmark's 1,000
  files, 102 for the scale layout, 5,002 for the agent layout grown to 100,000
  files, and **100,000 for one flat directory**, no better than today. Its cost
  depends on directory shape, not file count, and it changes identity. Not
  chosen.
- **C** (relational only: 32-byte blob hashes, `WITHOUT ROWID`) is the
  round-five result in §11.4: 36% and 34% fewer bytes per row and 2.6x faster
  inserts. It keeps one row per file per snapshot, so it does not change the
  growth or the O(files) save. Not chosen on its own; it stays compatible with
  A and can be taken in the same 6.0 migration or later.

**Remaining cost under A.** With today's id recipe a save still reads the
parent tree and hashes the full sorted tree, 80 to 110 ms at 100,000 files
(parent-tree read 54 to 74 ms, hash 27 to 37 ms). That is the price of keeping
identity unchanged, and it was weighed against a new recipe whose id over
(parent id, sorted changes) would cost about 2 microseconds.

**K and the keyframe spike.** K = 128 holds disk at about 94 KB per snapshot
and the worst point read at about 1.4 ms (Windows). The keyframe save writes a
full tree, 0.4 to 0.6 s at 100,000 files (the slowest keyframe in each run was
a cold-cache outlier, so this is an upper bound), once every 128 saves. That
spike is below today's every-save cost (1.75 s Linux, 5.3 s Windows), so the
keyframe is **amortised inside the save's own transaction**: no background
writer, no second code path, no state between processes. A background keyframe
is a later optimisation if a measured need appears. K is a writer-side
constant: a reader treats any snapshot with a keyframe marker as a keyframe, so
K can be retuned without migrating.

**Identity rule: unchanged.** A snapshot's id is the v2 recipe of §4.2: the
BLAKE3 of the **full sorted tree** plus parent, merge parent, message, time and
metadata. A delta or a keyframe is a storage detail below identity, as
chunking is (§11.2). A save reconstructs the parent tree in memory (the 54 to
74 ms read above), applies the changes and hashes the result, so two saves
that reach the same tree by different paths get the same id input, and `fsck`
verifies a delta snapshot by reconstructing its tree and recomputing the id.
Every existing id stays valid and nothing is re-minted by the migration. A
recipe over (parent id, changes) is rejected for now, knowingly: it would make
two identical trees reached differently two different snapshots, break the
bundle convergence of §9 and leave a second identity recipe to carry forever,
to save the 80 to 110 ms above.

**Migration (v3 to v4).** The existing `file_map` rows are keyframes. The
migration adds the two tables below and rewrites nothing, so it
is O(1) in repository size. It still bumps `user_version` to 4, which is what
makes older builds refuse the repository (`SchemaTooNew`) instead of reading a
delta snapshot as an empty tree. That refusal is the whole reason this is a
6.0. As with every migration it runs only from `open_and_migrate()` (§7.1).
Before the code lands, the golden corpus (Phase 18.3) must gain a repository
in the v4 format, so the migration and the reader are proven against it and
against every released format.

**Wire: unchanged.** Export keeps emitting full tree lists and import keeps
converting them into whatever the receiving repository stores, so the bundle
format does not change and 5.x peers can still clone from a 6.0 repository. A
delta wire (v5) is a later optimisation.

**Compaction and retention.** Removing a snapshot must first fold its changes
into its child, or promote the child to a keyframe. Without that, deleting a
delta's rows would silently change every descendant's tree. This composes with
the in-place re-mint that compaction now does (one transaction: a snapshot
whose parent was removed is re-minted with the same tree and a new parent, and
`PARENT` and tags follow). A re-mint keeps the tree, so under A the removed
snapshot's delta is folded into the first surviving child, or that child
becomes a keyframe when the fold would cost more than a keyframe write.
Retention follows the same rule. After a fold no chain may exceed K-1 deltas (rule 7).

**Storage schema (v4) — normative.** Two tables and two indexes, created by
the migration:

```sql
-- One row per snapshot stored as a delta. A stored snapshot with no row here is a keyframe.
CREATE TABLE IF NOT EXISTS delta_base (
    snapshot_hash TEXT PRIMARY KEY,
    base_hash     TEXT NOT NULL
);
-- What a delta snapshot changes in its base's tree. hash = '' removes the path.
CREATE TABLE IF NOT EXISTS file_delta (
    snapshot_hash TEXT NOT NULL,
    path          TEXT NOT NULL,
    hash          TEXT NOT NULL,
    mode          INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (snapshot_hash, path)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS idx_delta_base_base ON delta_base (base_hash);
CREATE INDEX IF NOT EXISTS idx_file_delta_path ON file_delta (path);
```

**Why the marker is the absence of a `delta_base` row, not a column on
`snapshots`.** Tree rows are keyed by snapshot hash whether the snapshot row
sits in `snapshots`, in `trash`, or in both (the fetch case of 17.1). A marker
keyed the same way travels with the rows: `undo`, `redo` and import never copy
a column, so they cannot lose or invent a marker. "No row means keyframe" is
what every v3 snapshot already is, so the migration writes nothing, and any
writer that still writes full `file_map` rows produces a valid keyframe.

**Rules — normative.** A *stored tree* is one whose hash is in `snapshots` or
`trash`.

1. **Exclusivity.** Every stored tree is exactly one of these. A **keyframe**:
   no `delta_base` row, a full tree in `file_map` (one row per path), no
   `file_delta` rows. A **delta**: one `delta_base` row, zero `file_map` rows,
   zero or more `file_delta` rows.
2. **Base.** `base_hash` must equal the snapshot's first parent
   (`parent_hash`), merges included; the chain is the first-parent chain. A
   root (`parent_hash = ''`) is always a keyframe. The base must itself be a
   stored tree.
3. **Reconstruction.** Tree(delta) is Tree(base) with each `file_delta` row
   applied: `hash = ''` removes `path`; anything else sets `(hash, mode)`.
   Writers emit only rows that change the base tree. An empty delta (the same
   tree as the base, a different message) is legal.
4. **Chain length and K.** The *chain length* of a snapshot is the number of
   delta links to its keyframe; a keyframe's is 0. A writer stores a snapshot
   as a keyframe when its parent's chain length + 1 would reach K, so chains
   are at most K-1 links. K = 128. The optional `settings` row
   `keyframe_interval` (decimal, at least 1) overrides K **for writers only**.
   It is a tuning and test knob, not format; readers ignore it. K = 1 means
   every snapshot is a keyframe, the v3 storage shape.
5. **Reader bound.** Readers accept any chain length up to a hard walk bound
   of 4,096 links. A longer chain, a cycle, or a base that is not stored is
   corruption (`Corrupt`), never a loop, a panic or an empty tree.
6. **Identity** is unchanged (§4.2): the id hashes the reconstructed full
   sorted tree.
7. **Removal.** A stored tree that is the base of another stored delta may be
   removed only in the same transaction that first either (a) **folds** it,
   rewriting the dependent's delta against the removed tree's base, which is
   allowed only when the dependent's first parent becomes that base
   (compaction, retention, squash), or (b) **promotes** the dependent to a
   keyframe. If the dependent's new first parent is not the removed tree's
   base, only promotion is allowed. After a fold, no chain exceeds K-1.
8. **Import.** The wire is unchanged: bundles and packs carry full trees. The
   receiver stores a snapshot as a delta only if its first parent is stored
   there (or earlier in the same import) and the chain stays within K-1;
   otherwise as a keyframe. If the hash already has stored rows (a trashed
   snapshot), they are **adopted** when they reconstruct exactly the received
   tree, and otherwise **replaced** by a keyframe. They are never duplicated.
9. **Unchanged tables.** `renames`, `snapshot_meta`, `signatures` and
   `compactions` are unchanged and stay keyed by snapshot hash.
10. **Migration v3 to v4.** Create the two tables and their indexes, then
    stamp `user_version = 4`. No row is rewritten.

**auto_vacuum.** New repositories are already `auto_vacuum=INCREMENTAL` (§7.4),
so the migration needs no `VACUUM`; a repository created earlier stays as it is
until a full `VACUUM` converts it.

---

## Non-goals

- **No separate hashed tree object** (à la Git's tree objects). Trees are rows in
  `file_map`. Revisit only if a real need for shared subtree identity appears. The parent
  deltas of §11.5 are not a tree object: they change how rows are stored, not
  what a snapshot id hashes, and they do not touch object identity.
- **No delta/packfile encoding.** Objects stay whole-content addressed: a name
  is always the hash of the full content. Storage-level chunking *below*
  identity is decided as D6 (§11.2); it is not delta encoding and changes no id.
- **No built-in trust.** Content addressing gives tamper-*evidence*, not
  authentication. Signatures (§11.1) are stored outside identity and verified
  against keys the caller trusts; velo makes no trust decision of its own.
- **No downgrade path.** Migrations are forward-only.
