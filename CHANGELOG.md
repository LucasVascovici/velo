# Changelog

Notable changes to Velo. Repository **format** changes are called out separately
from API changes, because they are the ones that can make existing data
unreadable. The normative format spec is [`docs/FORMAT.md`](docs/FORMAT.md).

This file starts at the format v2 break. Earlier releases are in the git history.

## Unreleased

### Fixed

- **`velo history --file` now means what it says.** It matched whether a path
  *existed* in a snapshot, and a velo tree is the complete file set — so it
  listed every snapshot since the file was created. It now compares the path
  against the parent, so additions, deletions, edits and mode changes count and
  nothing else does. A directory matches everything beneath it, which the help
  has always advertised and which previously matched nothing at all. With a
  filter set, `--limit` counts matches rather than candidates.

### Added

- **`branches::create` and `branches::set_tip`** — a branch can be created
  pointing at any past snapshot, without switching to it. Previously a branch came
  into existence only via `switch` (which also makes it current and leaves it
  unborn) or as a side effect of `save_tree`.
- **`merge::merge_base`** — the shared ancestor two snapshots merge against, so a
  consumer that merges need not reimplement an indexed recursive query in memory.
- **`Repo::snapshot`** — everything recorded about one snapshot, by the id a
  consumer already holds, instead of formatting it back into text to be resolved
  again.
- **`Repo::head_token`** — one integer that changes whenever snapshots, branches
  or tags change, for an application that has to notice a second window or a
  `pull`.
- **`SaveTree.timestamp_ms`** — a caller can supply the creation time, `None`
  meaning now. The timestamp is part of a snapshot's identity, so with the clock
  read internally an embedder could not control the id: no test could assert one
  against a constant, and importing history from another system would stamp every
  snapshot with the moment of the import. It was also the last ambient dependency
  in a crate that otherwise reads no process environment. Note that a branch tip
  is derived from the newest `created_at_ms`, so a backdated snapshot does not
  become the tip.
- **`SaveTree.merge_parent`** — a snapshot recorded through the embedder API can
  be a merge. The column, the id recipe and `history::Entry::is_merge` already
  supported a second parent; only the write path did not, so an embedder's merges
  were stored as linear history. Beyond the graph, that left the next merge
  computing its base from too old an ancestor and re-raising resolved conflicts.

### Changed — breaking (API only; the repository format is untouched)

- **`history::run` takes an `Options` struct.** It was five positional arguments,
  `run(&repo, false, 0, None, None)`, which said nothing at the call site — the
  same problem `SnapshotIdentity` fixed for `snapshot_id`. `limit` is now
  `Option<usize>`: `None` means all of them and is the default, `Some(n)` the
  newest n. Previously the limit reached SQL as `LIMIT ?`, so `0` — the obvious
  way to ask for everything — returned nothing, and `usize::MAX` worked only
  because the cast to `i64` wrapped to `-1`.
- **`history` results carry typed ids.** `Entry.hash` is a `SnapshotId`,
  `Entry.branch` a `BranchName`, `parent`/`merge_parent` `Option<SnapshotId>`,
  `tag` an `Option<TagName>`; `BranchRef.name` is a `BranchName`,
  `History.current` an `Option<SnapshotId>`, and `refs` is keyed by `SnapshotId`.
  2.3 typed every other result struct and missed this one, so walking history to
  look something up needed a `.parse()` whose error could never fire.
- **`SaveTree.branch` is `&BranchName`**, matching `parent`, so a consumer that
  publishes to one branch no longer clones the name on every save.
- **`tag::create` takes `Option<&SnapshotId>`** rather than a spec. Callers with an
  id in hand no longer hand back text to be resolved again; the CLI resolves at the
  argv boundary.
- **`TreeEntry.content` is a `Content` enum.** `Content::Bytes(Vec<u8>)` is what
  it was; `Content::Stored(ObjectHash)` is new. The `file`/`executable`/`symlink`
  constructors are unchanged, so only code that matched on `.content` is affected.

### Added

- **`Repo::branch_tip` and `Repo::branch_exists`** — ask about a branch directly
  instead of resolving it as a spec and interpreting `NotFound`. The two answers
  differ: a branch created by `switch` exists with no tip.
- **`TreeEntry::stored(path, object, kind)`** — a tree entry that references
  content already in the store. A snapshot is a whole tree, so changing one file
  meant re-supplying the bytes of every other file: each save cost the size of the
  entire tree in decompression, hashing and recompression. Carrying a tree forward
  is now a map with no I/O. A referenced object that the store does not hold is
  refused with `MissingObject`, so the API cannot record a snapshot naming content
  that is not there.

### Fixed

- **Saving an identical snapshot twice is idempotent.** A snapshot's identity
  excludes the branch, so the same tree with the same message and parent recorded
  on two branches inside one millisecond is *one* snapshot — but the second write
  surfaced a raw `UNIQUE constraint failed: snapshots.hash`. The existing rows are
  now left alone and the branch is still registered, so both branches see the tip.
- **Full-width snapshot ids leaked into terminal output.** Format v2 stores ids at
  64 characters, and eight renderers still printed them verbatim — `status`, `tag`,
  `undo`, `redo`, `show`'s parent line, `restore`, `switch`, `merge` and
  `history --graph`. Under v1's 16-character ids this looked right by accident;
  `velo tag`'s table blew out to a 64-wide column. Every site now goes through one
  abbreviation. The v4.0.0 notes claimed this was already consistent; it was not.
- `resolve_snapshot_id` reports an unresolvable spec as
  `Error::NotFound { kind: RefKind::Any, .. }` rather than `Error::InvalidInput`.
  A consumer asking for a branch that has no snapshots yet needs to tell "nothing
  there" from "malformed request", and matching the message text is what typed
  errors exist to avoid. Found by writing an actual consumer.

### Added

- `examples/prompt-registry` — a worked example of embedding `velo-core` with no
  working tree, plus `FINDINGS.md` recording where the API was awkward.

## 4.0.0

### ⚠️ Repository format v2 — breaking

**Existing repositories cannot be opened.** Format v2 changes every snapshot id,
so a pre-v2 repository is refused rather than migrated: `Repo::open` and
`Repo::open_and_migrate` both fail with `Error::FormatTooOld`, and the CLI
explains what to do instead. To carry work across, copy the working tree into a
fresh repository and save it there — a v1 bundle cannot help, because it carries
v1 ids.

Repositories written by this version and later are stamped with their format
version at creation, so this situation is detectable rather than guessed at.

All four Phase 0 decisions landed together, because splitting them would have
meant four separate id-invalidating migrations:

- **Snapshot ids are stored at full width** — 64 hex characters instead of a
  16-character truncation that was also the primary key. Truncating a BLAKE3
  digest to 64 bits put a ~50% collision risk near 5·10⁹ snapshots, which is thin
  for a store several applications write to. Abbreviation is now display-only.
- **Timestamps are epoch milliseconds** (`created_at_ms INTEGER`), hashed as their
  decimal form. Previously the id committed to formatted text, so a change to a
  format string could have altered snapshot ids.
- **Snapshot metadata**, hashed as part of the identity (see below).
- **Schema versioning** via `PRAGMA user_version`, with refuse-if-newer — this
  part shipped earlier and is what makes the refusal above possible.

The domain separator is now `velo-snapshot-v2\n`, so a v1 and a v2 id can never
collide even given identical inputs. Bundles and sync packs use magic `VELOBND2`;
a reader rejects an unknown version rather than guessing.

Objects are **format-stable** — no object file changed, and object hashes are
unaffected.

### Added

- **Snapshot metadata.** `SnapshotMeta` attaches app-namespaced
  `(namespace, key) → value` pairs to a snapshot via `SaveTree.meta`, read back
  with `Repo::snapshot_meta`. Consumers get somewhere to put provenance instead of
  encoding it into the message or inventing a sidecar file.

  Metadata is **covered by the snapshot hash**, and therefore immutable: changing
  a value produces a different snapshot. That is deliberate — metadata is mostly
  provenance, and provenance that can be silently rewritten is worth nothing.
  `fsck` reports a rewritten metadata row as an id mismatch. It also means
  metadata travels with bundles and sync automatically, because a receiver that
  did not get it would recompute a different id.

  The namespace `velo` is reserved. `velo save` attaches no metadata.

- `commands::timestamp_from_ms` for rendering a stored timestamp as
  `DateTime<Utc>`.

### Changed

- Commands that report a time (`history`, `show`, `blame`, `branches`,
  `stash list`) now return `DateTime<Utc>` rather than a preformatted string, so
  a consumer can format it however it likes.
- `commands::snapshot_id` takes a `SnapshotIdentity` struct instead of six
  positional arguments. Three of them were adjacent `&str`s, where swapping two
  would compile and produce a plausible-looking id.
- Snapshot ids are abbreviated consistently wherever they are printed. Some
  commands previously showed 8 characters and others 16; with full-width ids the
  inconsistency would have produced a 64-character column in `velo history`.
- The schema is one idempotent definition rather than an initial schema plus a
  chain of `ALTER TABLE` migrations. Those existed only to bring v1 repositories
  forward, which v2 refuses to do.

### Fixed

- **A merge resolved entirely in favour of your own side can be recorded.**
  `velo resolve --take ours --all` leaves the working tree unchanged, so `save`
  reported "Nothing to save", exited 0 without recording the merge, and left
  `MERGE_HEAD` in place. That wedged the repository — every later merge, rebase,
  undo, redo and cherry-pick refused with "a merge is already in progress", and
  the only escape was `merge --abort`, which discards the merge. A merge is real
  information regardless of the resulting tree, because it records the second
  parent, so it now gets a snapshot. A clean tree with no merge pending still
  reports nothing to save.
- `Display` for the id newtypes uses `f.pad`, so `{:<20}` aligns them. `write_str`
  silently ignores width, which had un-aligned `velo tag`'s table.
