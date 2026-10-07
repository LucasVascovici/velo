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
├── MERGE_HEAD    present only mid-merge/cherry-pick: "<pre-merge-id>:<source>"
└── REBASE_STATE  present only mid-rebase (with REBASE_ONTO, REBASE_ORIG_HEAD)
```

`HEAD` and `PARENT` are refs written **atomically** (temp file + rename). A reader
must tolerate a missing or empty `PARENT` (a repository with no commits).

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
| **v3** | Same marker, stamped `3`. Adds the `.velo/chunks/` directory and the additive `settings` and `objects` tables (§7.2); a v3 repository may keep its objects in files or in the database. |

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

Indexes are performance-only and may be rebuilt: `idx_filemap_snap`,
`idx_filemap_path`, `idx_snap_branch`, `idx_trash_branch`, `idx_stash_name`,
`idx_renames_to`, `idx_meta_lookup` (`snapshot_meta (namespace, key, value)`,
for metadata queries).

**Reserved branch names.** `_stash` is internal. `remotes/<remote>/<branch>` is
remote-tracking. `_deleted_<name>` is a soft-deleted branch. Consumers must not
create branches matching these patterns.

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
magic      : 8 bytes  "VELOBND1"   (v1)  /  "VELOBND2"  (v2 and v3)
version    : u32                    1 (v1) / 2 / 3
snapshots  : u32 count, then per row:
               hash, message, branch, parent_hash, merge_parent,
               created_at            (v1: string / v2: i64 epoch ms)
file_map   : u32 count, then per row: snapshot_hash, path, hash, i64 mode
meta       : u32 count, then per row: snapshot_id, namespace, key, value   (v2+)
tags       : u32 count, then per row: name, snapshot_hash
objects    : u32 count, then per row: hash, u32 len, len bytes
               (the raw, already-Zstd-compressed object, verbatim)
renames    : u32 count, then per row: snapshot_hash, from_path, to_path    (v3 only)
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

### 9.1 Packs over HTTP

The HTTP transport carries packs with exactly this encoding; nothing about the
bundle changes. Under `<base>/velo/v1/`: `GET refs` returns a refs block;
`POST upload` takes the client's have-ids (length-prefixed strings) and returns
a refs block followed by the pack; `POST receive` takes branch and new tip
(length-prefixed) followed by the pack and returns a length-prefixed status
(`OK <snapshots> <objects>` or `REJECT <reason>`). A refused push is a `200`
carrying `REJECT`, as over ssh; other non-2xx codes are failures.
Fast-forward-only semantics are unchanged.

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

Apart from D6 (§11.2) and D7 (§11.3), both now implemented, nothing in this
section is written or read by the current code. These are decisions
taken before any code exists, because each is cheap now and a format break
later. Later work implements them to the letter.

### 11.1 Signatures (D5)

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
- **Storage, when implemented:**

  ```sql
  signatures (
      snapshot_hash TEXT NOT NULL,
      algorithm     TEXT NOT NULL,
      public_key    TEXT NOT NULL,
      signature     TEXT NOT NULL,
      PRIMARY KEY (snapshot_hash, public_key)
  )
  ```

  The table is additive and changes no id.
- **Transport:** bundle wire version 4 appends a `signatures` section after
  `renames` (u32 count; per row: snapshot_hash, algorithm, public_key,
  signature). Every writer that supports signatures writes version 4, so a
  v3-only reader *refuses* the bundle instead of importing it and silently
  dropping the signatures. Readers verify every signature before committing an
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
  - Never squashed **or** re-minted: once signatures exist, signed snapshots,
    because re-minting invalidates the signature.
  - Tagged snapshots *may* be re-minted as descendants. The tag is retargeted to
    the new id and the move is recorded.
- **The squashed snapshot** takes the newest member's tree, message, timestamp
  and metadata (including the author). Its parent is the oldest member's
  parent. Rename edges are composed across the range.
- `gc` never collects `compactions` rows.

---

## Non-goals

- **No separate hashed tree object** (à la Git's tree objects). Trees are rows in
  `file_map`. Revisit only if a real need for shared subtree identity appears.
- **No delta/packfile encoding.** Objects stay whole-content addressed: a name
  is always the hash of the full content. Storage-level chunking *below*
  identity is decided as D6 (§11.2); it is not delta encoding and changes no id.
- **No signing yet.** Content addressing gives tamper-*evidence*, not
  authentication. Signatures are decided as D5 (§11.1), outside identity, and
  not yet implemented.
- **No downgrade path.** Migrations are forward-only.
