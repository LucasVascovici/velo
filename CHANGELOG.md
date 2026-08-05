# Changelog

Notable changes to Velo. Repository **format** changes are called out separately
from API changes, because they are the ones that can make existing data
unreadable. The normative format spec is [`docs/FORMAT.md`](docs/FORMAT.md).

This file starts at the format v2 break. Earlier releases are in the git history.

## Unreleased

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

- `Display` for the id newtypes uses `f.pad`, so `{:<20}` aligns them. `write_str`
  silently ignores width, which had un-aligned `velo tag`'s table.
