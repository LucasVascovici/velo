# Crash safety

What velody promises when a process is killed part-way through a command, how
that is tested, and the windows that are known to leave something to clean up.

The promise: after a kill at any durable step of any operation below, the
repository opens, `fsck` reports no problem (or exactly the one listed below),
the working tree is the old tree or the new one (or a listed window), no lock is
left held, and `undo`/`redo` still work or refuse with a typed error.

## Method

- **Failpoints.** Every durable step of an operation (a transaction commit, a
  write of `.velo/PARENT` or `.velo/HEAD`, the first file of a multi-file
  working-tree write, the first object a `gc` deletes) is preceded or followed
  by a call to `velody_core::failpoint::hit("<op>.<stage>")`. Without the
  `failpoints` cargo feature the call is an empty inline function and compiles
  to nothing; no release build carries it.
- **Self-exec abort.** The test (`crates/velody-core/tests/crash.rs`) builds a
  fixture repository, runs the operation once without a handler to learn the new
  state, and once with a handler that logs every point it hits (discovery). Then,
  for each point, it copies the fixture and spawns the same test binary as a
  child with a handler that calls `std::process::abort()` at that point. An abort
  runs no destructors and no `Drop`, which is what a kill looks like to the
  files on disk. The parent reopens the damaged copy and checks it.
- **The four checks**, run after every kill:
  1. `fsck` finds no problem; `fsck --repair` then leaves nothing, not even
     cruft.
  2. The working tree is the pre-operation tree or the post-operation tree. Any
     other state must be a known window (below), and the test asserts the exact
     state that window leaves and that it can be finished.
  3. A clean `status` means the tree equals the tree of the snapshot
     `.velo/PARENT` names.
  4. The write lock is not held, and `undo` then `redo` succeed or fail with a
     typed error that is not `Corrupt`.
- **Per-operation checks** on top of those:
  - merge and rebase: if `MERGE_HEAD` / `REBASE_STATE` survives, `--abort` puts
    the pre-operation tree and `PARENT` back byte for byte, and `rebase
    --continue` finishes or refuses with a typed error;
  - `gc`: every file of every live snapshot still reads back (chunked files
    included), a second `gc` completes, and `fsck` is clean after it;
  - `compact` and retention: an old id resolves to `Compacted { into }` only if
    the rewrite committed, `into` is live, and a rolled-back rewrite leaves
    history exactly as it was;
  - `bundle apply`: applying the bundle again succeeds, yields exactly the source
    repository's snapshots with identical trees, and a further apply imports
    nothing (idempotence);
  - `clone`: the exact leftover state is asserted per point, a second clone into
    it is refused, and deleting the directory and cloning again reproduces the
    source tree.
  - delta storage (format 4): every scenario above runs twice, once at the
    production keyframe interval and once with the fixture and the operation at
    K = 3, so delta writes, keyframes and folds are crossed. The points
    `tree_store.after_delta_rows` (delta rows written, base record not yet),
    `tree_store.after_keyframe_rows` (a whole tree written) and
    `tree_store.fold.after_rewrite` (a dependant rewritten, the removed rows not
    yet deleted) all sit inside the operation's one transaction, so a kill at any
    of them rolls the whole operation back and adds no window. `compact_fold`
    is a compaction whose removed snapshot has a delta dependant;
  - migration: `migrate_v3` builds a v3-shaped repository (whole trees, no delta
    tables, `user_version` 3) and kills `open_and_migrate` at
    `migrate.before_stamp`, between the schema script and the version stamp.
    The schema script and the stamp run in one transaction, so the kill leaves a
    complete v3 repository (no v4 table, `integrity_check` ok, `MigrationRequired`
    on open) that migrates cleanly on the next open. There is no window.
- The test also requires that each scenario actually hit the points listed for
  it, so a failpoint that is moved or stops being reachable fails the test
  instead of quietly shrinking the coverage.

## Results

Every point passes the four checks, except the ones marked **window**, which are
described in the next section. Retention has no points of its own: it plans the
ranges and calls `compact::run`, so it crosses the same `compact.*` points, and
the test drives it separately.

| Operation | Point | Result |
| --- | --- | --- |
| save | `save.objects_stored` | old state; objects are unreferenced until `gc` |
| save | `save.before_commit`, `save.after_commit` | old / new snapshot; PARENT unmoved |
| save | `save.after_parent` | new state |
| restore | `restore.before_write` | old tree |
| restore | `restore.after_first_file` | **window**: mixed tree |
| restore | `restore.before_parent`, `restore.after_parent` | new tree; PARENT moves last |
| switch | `switch.before_head`, `switch.after_head` | tree is the target's; HEAD moves last |
| undo | `undo.before_parent`, `undo.after_parent`, `undo.before_commit`, `undo.after_commit` | each is the old or the new state |
| redo | `redo.before_commit`, `redo.after_commit` | old / new state |
| merge | `merge.before_state`, `merge.after_state`, `merge.after_conflicts` | `merge --abort` restores the pre-merge tree byte for byte |
| merge --abort | `merge_abort.after_restore`, `.before_clear_state`, `.after_clear_state` | re-running the abort reaches the aborted tree |
| rebase | `rebase.before_state`, `.after_onto`, `.after_state`, `.after_restore`, `.after_apply`, `.after_replay`, `.before_clear_state` | `--abort` restores the original; `--continue` finishes or refuses typed |
| rebase --abort | `rebase_abort.after_restore`, `rebase_abort.before_clear_state` | re-running finishes the abort |
| apply (files) | `apply.after_files`, `apply.after_first_file` | see restore |
| gc | `gc.after_prune` | database pruned, garbage objects still on disk; the next `gc` removes them |
| gc | `gc.after_object`, `gc.after_chunk` | some garbage gone; every live object present |
| compact, retention | `compact.before_commit` | nothing changed; no `compactions` row |
| compact, retention | `compact.after_commit`, `compact.before_parent` | **window**: PARENT names an old id |
| bundle apply | `bundle.after_objects`, `bundle.before_commit` | objects stored, no rows; re-apply completes |
| bundle apply | `bundle.after_commit` | fully imported |
| clone | `clone.after_init` | **window**: empty repository |
| clone | `bundle.after_objects`, `bundle.before_commit` | **window**: objects, no snapshots |
| clone | `bundle.after_commit`, `clone.after_import` | **window**: history in, nothing checked out |
| clone | `restore.before_write` | **window**: history in, no files |
| clone | `restore.after_first_file` | **window**: some files |
| clone | `restore.before_parent`, `restore.after_parent` | **window**: all files, PARENT not or just written |

Bundle signature verification (wire v4) happens before any object is written, and
the failpoints sit after it, so a bundle with a bad signature is refused with
nothing stored.

## Known windows

Each is asserted exactly by the test (the `KNOWN_WINDOWS` and `CLONE_WINDOWS`
tables in `crash.rs`).

- **`restore.after_first_file`.** A whole-tree restore writes its files one by
  one. A kill after the first leaves a mix of old and new files. `PARENT` still
  names the old snapshot and `status` lists the changed files as unsaved.
  Recovery: `velody restore --force <target>` finishes it.
- **`compact.after_commit` and `compact.before_parent`.** The rewrite
  transaction is committed (new snapshots, old rows deleted, `compactions` rows
  written) and then `.velo/PARENT` is rewritten as a separate file write. A kill
  between them leaves `PARENT` naming an old id. The working tree is untouched
  and equals the new snapshot's tree. The old id resolves to
  `Compacted { into }`, `fsck` reports exactly one `DanglingPosition` for it, and
  `status` lists every file as unsaved because there is no snapshot to compare
  against. Recovery: `velody restore --force <into>` (the id in the `Compacted`
  message; the tree is identical, so nothing is lost). Moving the `PARENT` write
  before the commit would only move the window: a kill then would leave `PARENT`
  naming an id that does not exist yet.
- **Clone, every point.** `clone` creates the directory, initialises the
  repository, imports the history in one transaction, then checks out. It does not
  clone into a temporary directory and rename, because it accepts an existing
  directory (and `.` ) and changing that is a change of its semantics. A kill
  therefore leaves a directory with a `.velo` in it, in one of these states:
  - after `init`: no snapshots, no files, no `origin` remote;
  - after the objects are stored, before the import commits: no snapshots, no
    files, unreferenced objects (a later `gc` collects them);
  - after the import: all snapshots, no `origin` remote row, no files;
  - during the checkout: all snapshots, and some files, each complete and equal to
    the source's (files are written whole);
  - after the last file, before `PARENT`: all files, no position.

  A second `velody clone` into the directory is refused ("already contains a Velody
  repository"). Recovery: delete the directory and clone again.
- **Objects without rows.** A kill between storing objects and committing the
  transaction (`save.objects_stored`, `bundle.after_objects`) leaves objects that
  nothing references. They are harmless and are removed by `gc` (and `fsck
  --repair` reports unreferenced chunks as cruft).

## Power loss

The harness kills the process, not the machine. A process kill cannot lose data
the kernel already accepted; a power cut can. That variant cannot be tested
portably (it needs a filesystem that drops unflushed writes, and the behaviour
differs by filesystem and mount options), so it is documented here instead.

- `write_atomic` writes a temporary sibling file and renames it over the target.
  It does not `fsync` the file or the directory. After a power loss the rename may
  not have reached the disk (the old `PARENT`, `HEAD` or object remains), or, on
  filesystems that do not order data before the rename, the new file may exist but
  be empty or short.
- SQLite runs with `journal_mode=WAL` and `synchronous=NORMAL` (FORMAT.md §7.4).
  In WAL mode with `NORMAL`, a committed transaction is not forced to disk at
  commit time, so after power loss it may roll back, but the database remains
  consistent: it comes back at some earlier commit, never half-applied. Setting
  `synchronous=FULL` would make every commit durable at the cost of a sync per
  commit; velody does not.
- Objects are written before the transaction that refers to them. A rolled-back
  import (or save) therefore leaves only objects nothing references, which `gc`
  removes. The reverse, rows pointing at objects that were lost, is the case
  `fsck` is for: it re-hashes every referenced object and reports a missing or
  short one as a problem.
- `.velo/PARENT` is a separate file from the database. If the transaction that
  created the snapshot rolled back but the `PARENT` rename survived, `PARENT`
  names a snapshot that does not exist. That is the same state as the compact
  window above, minus a `Compacted` answer: `fsck` reports one `DanglingPosition`,
  `status` lists every file as unsaved, and the id resolves to nothing. No data is
  lost: the working tree is intact. Recovery: `velody restore --force` to a snapshot
  that exists (for instance the branch tip). An empty `PARENT` reads as no
  position; a truncated one names an id that does not exist, as above.

## The WAL bound

A `Repo` held open for a long run would otherwise carry its WAL at the
high-water mark of the 1000-page autocheckpoint, about 4 MB. Each connection
sets `journal_size_limit` to 1 MiB, and when a write guard is released (after
its transactions have committed, with the repository lock still held) the
library truncates the WAL with `PRAGMA wal_checkpoint(TRUNCATE)` if the file
has grown past 2 MiB. The checkpoint is never inside a transaction, gives up at
once when a reader holds the log, and ignores any error: a WAL that is not
truncated is only larger. A crash before the checkpoint leaves a WAL that
SQLite replays on the next open; a crash during or after it leaves either that
WAL or a database that already holds the pages. The crash scenarios above run
unchanged and pass with the bound in place. Nothing is stored differently, so
FORMAT.md is untouched.

## Running the harness

```bash
cargo test -p velody-core --features failpoints --test crash
```

The run spawns one child process per point, so it takes about a minute. Without
`--features failpoints` the test target is not built (it is declared with `required-features`), because the
points compile to nothing. To add an operation, add a `Scenario` (a fixture
builder and the operation) to `crash.rs`; the harness discovers its points by
running it once, and the four checks run on each.

## Undo-heavy differential run

The differential harness (`differential_long` in `crates/velody-core/src/tests/differential.rs`)
also runs an undo-heavy mix, and the nightly run uses it too. The first measured run
(Windows, release, commit 78afa7a, seed 1000000) started 18 sequences and stopped at
the 18th: 17 were clean, and seed 1000002 from the `format-v4` start (subject K=3)
left the K=1 reference with a `DeltaBaseMissing` fsck finding after the shrunk ops
`Redo, Undo, Edit, Save`. The cause was a `save` that discarded the redo stack
(the trash rows) but left their stored trees behind, a delta chain whose base was
itself an unreferenced tree; `fsck` read that base as missing. `save` now removes
the discarded trees with the fold, and `fsck` treats an unreachable chain as cruft
rather than corruption. The regression is `tests/orphan_delta_repro.rs`. The run is
recorded in `bench/results/2026-10-undo-heavy-differential.txt`. To reach 10,000
sequences the owner (about 9 seconds per sequence
at the rate seen here, so roughly 25 hours) by repeating the one-hour slice below
about 25 times, each with a distinct `VELODY_DIFF_SEED` so the runs do not overlap
(each slice covers about 400 sequences). Run it across machines if you prefer:

```bash
for run in $(seq 0 24); do
  VELODY_DIFF_SEED=$((1000000 + run*100000)) VELODY_DIFF_MIX=undo VELODY_DIFF_SECS=3600 \
    VELODY_DIFF_FAILURE_OUT=fail-$run.txt \
    cargo test -p velody-core --release --locked differential_long -- --ignored --nocapture \
    2>&1 | tee run-$run.log
done
cat run-*.log | grep -c 'differential_long: seed'   # total sequences
```

Every seed and start pair the run prints (`differential_long: seed ...`) is one sequence.
