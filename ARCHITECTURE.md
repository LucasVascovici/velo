# Velo as a library — architecture & implementation plan

Goal: make Velo embeddable, so other projects can use it for version history
(editors, registries, document tools, GUIs) instead of only the `velo` CLI.

This is a revision of the original 16-item plan, corrected against the actual
codebase. Every "current state" claim below was verified, not assumed.

**Necessity markers**

| Marker | Meaning |
| :--- | :--- |
| 🔴 **Required** | The goal is unreachable without it. Blocking. |
| 🟡 **Recommended** | Do it unless there's a concrete reason not to. |
| 🟢 **Optional** | Add when a real consumer needs it, not before. |
| ⛔ **Avoid** | Actively don't do this. See [Anti-goals](#anti-goals). |

---

## Starting state (verified before Phase 1)

Kept as the baseline the plan was built against, not as a description of the code
today — Phases 0 through 2 changed every row below. It is here because the
estimate corrections in the next section only make sense against it.

| Claim | Reality then |
| :--- | :--- |
| Library target | **None.** No `src/lib.rs`, no `[lib]`, no `[workspace]`, no `[features]`. Velo is binary-only and cannot be used as a library at all today. |
| Error handling | **`anyhow` is not used anywhere.** A hand-rolled `VeloError` enum already exists — but it is stringly-typed: `InvalidInput(String)` carries divergence, conflicts, dirty tree, and non-fast-forward alike. |
| Output coupling | **238 `println!`, 6 `print!`, 4 `eprintln!`** across 26 files using `console`. |
| Process/env coupling | Contained to **three files**: `resolve.rs` (`Term::stdout`, `read_key`, `$EDITOR`), `main.rs` (`process::exit`, `current_dir` — already CLI-side), `transport.rs` (`current_exe`). |
| Command signatures | **21 of 23** entry points return `Result<()>`. Only `save` returns data (`SaveResult`); `undo` returns a display `String`. |
| Repo handle | None. `get_conn_at_path` is called from **28 files**, re-opening SQLite and re-running all migrations every time. |
| Merge engine purity | `diff3_segments` / `try_auto_merge` contain **zero** `conn`/`db::`/`fs::`/`storage::` references. Already pure `&str → String`. |
| In-memory objects | **Partly present.** `storage::store_raw(&[u8]) -> hash` and `storage::read_object(hash) -> Vec<u8>` are already public. |
| Schema versioning | `PRAGMA user_version` **never used.** Three migrations sniff `pragma_table_info`. No way to detect a *newer* repo. |
| ID semantics | `SnapshotId` etc. are bare `String`. Snapshot IDs are the **16-hex truncation** of BLAKE3 (64 bits) and that truncation *is* the primary key, every `parent_hash`, `merge_parent`, tag/stash/remote-ref target. Object hashes are full 64-hex. |
| Timestamps | `created_at` is a **formatted string** (`%Y-%m-%d %H:%M:%S%.3f`) and is an input to the snapshot hash. |
| Tests | Live **inside the binary** (`src/tests.rs`, `mod tests`) with ~38 repo fixtures. |

### Where the original estimate was wrong

- The refactor is **cheaper** than "80% is untangling": the merge engine is
  already pure, the object primitives already exist, and process/env coupling is
  three files. Budget for *volume* (238 print sites), not archaeology.
- The format break is **bigger**: storing full hashes is not a display change,
  it rewrites every ID. See Phase 0.

---

## Phase 0 — Irreversible decisions ✅ **DONE**

**No code — decisions only**, because each one changes every snapshot ID, and they
must land as **one single format break** rather than four.

**Deliverable: [`docs/FORMAT.md`](docs/FORMAT.md)** — the normative repository
format spec. It documents v1 (implemented) and specifies **v2** (decided, not yet
implemented), precisely enough to implement from.

| # | Decision | Chosen | Cost accepted |
| :--- | :--- | :--- | :--- |
| D1 | App-namespaced snapshot metadata | **Hashed** — part of snapshot identity | Metadata is immutable; changing it makes a new snapshot |
| D2 | Snapshot ID width | **Full 64-hex stored**; 16-char truncation is display-only | Slightly larger DB and wire size |
| D3 | Timestamps | **Epoch milliseconds**; `DateTime<Utc>` in APIs | Lexicographic timestamp ordering no longer holds |
| D4 | Schema versioning | **`PRAGMA user_version`**, refuse-if-newer, `open()` ≠ `open_and_migrate()` | Callers handle migration explicitly |

Also settled in the spec: the domain separator moves to `velo-snapshot-v2\n` (so
v1/v2 ids can never collide), the bundle magic becomes `VELOBND2`, and a
`snapshot_meta` table is added. Objects are **format-stable** — no object is
rewritten by the migration.

> ⚠️ All four change snapshot IDs, so they landed as **one atomic change**. D4
> (schema versioning) arrived in 1.5; D1, D2 and D3 followed in a single commit —
> see [Format v2](#format-v2--the-phase-0-break-). Strategy **A (re-init)** is
> what shipped: a pre-v2 repository is refused, not migrated.

---

## Phase 1 — Make it a library at all 🔴 **DONE**

| Sub-task | Status |
| :--- | :--- |
| 1.1 Workspace split | ✅ `velo-core`, `velo-merge`, `velo-tui`, `velo-cli`, `velo-testkit` |
| 1.2 Typed errors | ✅ 20-variant `#[non_exhaustive] Error` with classifier helpers |
| 1.3 Commands return data | ✅ **Done** — every command returns data; `velo-core` has zero print sites and the boundary lints are enabled |
| 1.4 Repo handle + write guard | ✅ `init`/`open`/`discover`, `write`/`try_write`/`write_timeout` |
| 1.5 Schema versioning | ✅ `PRAGMA user_version`, refuse-if-newer, `open()` ≠ `open_and_migrate()` |
| 1.6 Injectable transport | ✅ `transport::Spawn` passed in; `velo-core` reads no environment at all |
| 1.7 Tests + testkit | ✅ tests relocated, `velo-testkit::TempRepo` seeded |

**Remaining before a consumer can be written: the rest of 1.3, and 1.6.**

1.3 is the bulk of Phase 1 — originally 238 print sites across 23 command
modules, each needing a data struct in core and a renderer in `velo-cli`. It is
being done in batches so the tree stays green throughout:

| Batch | Modules | Status |
|---|---|---|
| 1 — inspection | `status`, `history`, `diff`, `show`, `blame`, `grep` | ✅ **done** (+ `stash show`, which shares the diff model) |
| 2 — listings | `branches`, `tag`, `fsck`, `remote` | ✅ **done** |
| 3 — mutating | ✅ all of them | ✅ **done** |

Batch 1 established the pattern the rest follow: core returns a documented
struct, `velo-cli/src/render/<cmd>.rs` owns every byte of output, and the flags
that only pick a presentation (`--oneline`, `--graph`) never reach core at all.
It also collapsed three drifted hunk printers into one shared diff model used by
`diff`, `show` and `stash show`.

Batch 2 added two more rules. **Split conflated entry points**: `branches::run`
and `tag::run` each took a bag of `Option`s and dispatched internally on which
were set; they are now `list`/`create`/`delete`, one per operation, with the
dispatch in the CLI where the flags live. **Type the findings, don't
pre-format them**: `fsck` returns `Problem` / `Cruft` / `Section` values with
`Display` and `describe()` for text, so a consumer can ask "which objects are
missing?" without parsing lines. Deciding the exit code is the CLI's job, so
`fsck::run` no longer returns `Err` to signal corruption — callers check
`Report::is_healthy()`.

Batch 3 is the mutating commands, where output is interleaved with the work
rather than produced at the end. Two more rules came out of `merge` and
`restore`:

**Model what happened, not what to print.** `merge::Outcome` distinguishes the
five ways a merge can end (aborted, unborn branch adopted, already up to date,
fast-forwarded, three-way), and `ThreeWay` records a `FileAction` per file. The
summary counts and the "here's how to resolve it" block are derived in the
renderer, so nothing about the wording is baked into core.

**Convert callees before callers.** `restore::run` is a step inside eleven other
commands; while it printed, every one of them leaked its progress lines into the
middle of their own output. Converting `merge` without `restore` left
`merge --abort` printing restore's lines *before* its own. Where a command calls
another, the callee has to go first. The same forced `save` to be converted with
cherry-pick and rebase, which both commit through it.

**Extract the shared work, then share its renderer too.** `merge`, `cherry-pick`
and `rebase` all answer "given an ancestor, ours and theirs, what should the tree
become?", and all three carried their own copy of the loop. They now share
[`commands::apply`], which owns the loop and the `FileAction` vocabulary, and
`velo-cli/src/render/apply.rs`, which owns how it reads. That is what surfaced
the drift between the copies — see the fixes list.

**A no-op needs a reason, not just a `None`.** `save::run` returned
`Option<SaveResult>` for three outcomes: saved, nothing-to-save, and
nothing-to-amend. Callers couldn't tell the last two apart, so `save` printed the
explanation itself — which is exactly what leaked into cherry-pick's output. It
returns a three-way `Outcome` now.

**Never print an error and return `Ok`.** `switch` refused a dirty switch by
printing a message and returning success, so it exited 0 and a script had no way
to know the switch hadn't happened. Returning data made this obvious: there was
no outcome variant that honestly described "didn't switch". A refusal is an
error.

**Guidance that depends on the command belongs at the dispatch point.** The
generic dirty-tree hint can suggest `save` and `stash push`, but only `switch`
and `restore` accept `--force`. `main()` derives the force form from the parsed
command and passes it to `hint_for`, so no command is ever told about a flag it
would reject.

**Known gap: no progress reporting.** A command that returns data can only be
rendered once it finishes, so a long `rebase` or a `pull` over `ssh://` prints
nothing until it completes. Designed in [2.0](#20-progress-reporting-) below;
not yet implemented.

**Batch 3 completion also closed three long-standing TODOs:**

- The boundary lints are **on**: `#![deny(clippy::print_stdout,
  clippy::print_stderr)]` in `velo-core/src/lib.rs`. The discipline is now
  enforced by the compiler, not by convention.
- `console` is **gone** from `velo-core`'s dependencies — it has no way to
  produce terminal output at all any more.
- Every command's outcome is a documented type, so a consumer can act on results
  instead of scraping text.

**The `&Connection` half of 1.3 is done too.** Every command now takes `&Repo`
(reads) or `&WriteGuard` (mutations):

- **`rusqlite` is no longer re-exported.** SQLite appears nowhere in the public
  API, so it is a genuine implementation detail — the storage engine could change
  without breaking a consumer.
- **One connection per process.** Commands used to call `get_conn_at_path` on
  every invocation, reopening the database (and re-running migration checks) each
  time; they now share the `Repo`'s connection. The only remaining direct open is
  `init`, which creates the repository before a `Repo` can exist.
- **Refuse-if-newer finally covers the CLI.** It was implemented and unit-tested
  on `Repo::open`, but the binary opened SQLite directly and so bypassed it. The
  CLI now goes through `Repo::open_and_migrate`, and a repository written by a
  newer Velo is refused on both read and write paths. `LocalRemote` checks the
  *far* repository the same way, so a newer remote is refused rather than misread.
- **Mutation requires proof of the lock.** `&WriteGuard` is unforgeable without
  `Repo::write()`, so "did we take the lock?" is answered by the compiler. The CLI
  already locked exactly these commands, so runtime behaviour is unchanged.
- **`velo-tui` no longer knows SQLite exists.** `resolve_interactive` takes a
  `&WriteGuard`.

`Connection::transaction` needs `&mut Connection`, which a shared connection
can't hand out, so mutating code goes through `WriteGuard::transaction` —
`unchecked_transaction` under the hood, whose one rule is "don't nest". Holding
the guard is what makes that safe: one connection per `Repo`, one guard per lock,
and no other route to a transaction.

**1.6 is done too.** `transport::Spawn` carries the three things core used to
discover for itself:

| Was | Now |
|---|---|
| `std::env::current_exe()` for `child:` | `Spawn::local_bin`, supplied by the caller |
| `std::env::var("VELO_SSH")` | `Spawn::ssh`, defaulting to `ssh` |
| `std::env::var("VELO_REMOTE_BIN")` | `Spawn::remote_bin`, defaulting to `velo` |

The CLI assembles it in `spawn_config()` — still honouring `VELO_SSH` and
`VELO_REMOTE_BIN`, so nothing changes for a user — and threads it through
`sync::{clone,fetch,push,pull}`. `velo-core` now contains **no** `env::` read.

That matters beyond tidiness: an embedding consumer may have no `velo` executable
on disk at all, so `current_exe()` would have pointed at *their* binary. Making it
a parameter turns a silent misfire into a value they choose. It also made the
transport unit tests exact — they assert the full command line instead of
`assert!(prog.contains("velo") || !prog.is_empty())`.

**Phase 1 is complete.** `velo-core` is a library: data in, data out, no output,
no environment, no SQLite in its API, and mutation gated by a lock the type system
enforces.

Remaining:

- `velo-core` still depends on `console` and prints (marked `TODO(P1.5)` in
  `Cargo.toml` and `lib.rs`).
- The boundary lints (`deny(clippy::print_stdout, print_stderr)`) are **commented
  out** in `lib.rs` — turning them on is the gate that proves 1.3 is complete.
- `velo-core` re-exports `rusqlite` because command signatures still take
  `&Connection` rather than `&Repo`/`&WriteGuard`.
- The CLI still opens repositories with `db::get_conn_at_path`, so the
  refuse-if-newer guard is **not yet on the CLI path** (it is implemented and
  unit-tested on `Repo::open`).

### 1.1 Workspace split
```
velo-core     lib: repo, storage, db, merge glue, sync   (no output, no env, no exit)
velo-merge    lib: pure diff3 + merge drivers            (no db, no fs)
velo-cli      bin: clap, rendering, colour, exit codes
velo-tui      lib: the interactive conflict resolver
velo-testkit  lib: temp-repo fixtures (dev-dependency)
```
**The discipline boundary — enforced, not aspirational.** `velo-core` must
contain no `println!`/`eprintln!`/`print!`, no `process::exit`, no
`env::current_dir()`, no `env::current_exe()`, no `$EDITOR` spawning, no
terminal detection, no colour codes.

Enforce it mechanically in CI, or it will rot:
```toml
# velo-core/src/lib.rs
#![deny(clippy::print_stdout, clippy::print_stderr, clippy::exit)]
```

### 1.2 Typed errors — **before** 1.3
Split `InvalidInput(String)` into structured variants so consumers branch on
outcomes instead of string-matching:
```rust
#[non_exhaustive]
pub enum Error {
    Diverged { ahead: usize, behind: usize },
    Conflicts(Vec<PathBuf>),
    DirtyWorkingTree(Vec<PathBuf>),
    NotFastForward { branch: BranchName },
    NotFound(RefSpec),
    Locked { held_by: Option<u32> },
    Corrupt { detail: String },
    SchemaTooNew { found: u32, supported: u32 },
    Io(std::io::Error),
    Db(rusqlite::Error),
}
```
`#[non_exhaustive]` from day one so new variants aren't semver breaks.
`thiserror` is 🟡 convenience — the existing hand-rolled `Display` already works.

> Ordering matters: return types and error variants get designed together.
> Doing 1.3 first means touching all 23 commands twice.

### 1.3 Commands return data
`repo.status() -> Status`, `repo.history(Query) -> Vec<Snapshot>`,
`repo.diff(a, b) -> Vec<FileDiff>` with real hunk structs. All 238 print sites
move to `velo-cli`.

### 1.4 Repo handle + write guard
```rust
Repo::init(&Path)      Repo::open(&Path)      Repo::discover(&Path)   // explicit, not implicit
let w = repo.write()?;                 // one lock + one transaction for N mutations
repo.try_write()?;  repo.write_timeout(Duration)?;
```
Discovery becomes a *distinct call*, never an implicit upward search inside
other operations. The timeout matters: a GUI that takes the write lock and then
opens a modal would otherwise wedge every other process.

### 1.5 Schema versioning — **promoted from Tier 2**
`PRAGMA user_version`; separate `open()` from `open_and_migrate()` so the caller
chooses when migration happens; refuse newer-than-supported with
`Error::SchemaTooNew`.

*Promoted because it is cheap and it is the only thing protecting users from
corruption the moment two apps share a repo. Shipping without it is the riskiest
ordering in the plan.*

### 1.6 Injectable sync server command — **not in the original plan**
[`transport.rs`](src/transport.rs) resolves the `child:` scheme via
`std::env::current_exe()`. Embedded in a host app, that spawns **the host app**
as a sync server. Transport must accept an injectable server command, or `child:`
must be CLI-only. This is an API-shape decision, so it belongs here.

### 1.7 Relocate tests + seed `velo-testkit` — **not in the original plan**
`src/tests.rs` lives inside the binary. Tests must be split per crate as part of
the move — they are the safety net for this entire refactor, so this is not a
follow-up task. The existing fixture helpers *are* the first version of
`velo-testkit`.

---

## Phase 2 — Make it useful to non-CLI consumers ✅ **DONE**

Every required item has landed. The one thing still open is a 🟡 recommended
extra: pluggable merge drivers in [2.2](#22-extract-velo-merge--done).

The capability work. Without 2.1 every consumer marshals through temp files.

### 2.0 Progress reporting ✅ **DONE**

The one thing Phase 1 made worse: returning data means nothing can be rendered
until the operation finishes. Fixing it is the last piece of "core reports,
caller presents".

**The trait.** Every method defaults to a no-op, so an implementation overrides
only what it uses.

```rust
/// Where a long operation reports its progress.
///
/// Calls may arrive from several threads at once — hashing and writing files are
/// parallel — and arrive once per item. An implementation must therefore be
/// cheap and do its own rate limiting: core does not throttle, because how often
/// to redraw is a presentation decision.
pub trait Observer: Send + Sync {
    /// A phase began. `total` is `None` when the size isn't known in advance.
    fn begin(&self, _phase: Phase, _total: Option<u64>) {}

    /// `by` more items of `phase` are done.
    ///
    /// A delta rather than a running total: deltas are race-free when several
    /// rayon workers report at once, a cumulative count is not.
    fn advance(&self, _phase: Phase, _by: u64) {}

    /// The phase finished. Always called, including on the error path.
    fn finish(&self, _phase: Phase) {}
}

/// What a long operation is currently doing.
///
/// `#[non_exhaustive]`: a new phase is not a breaking change, and an observer
/// that doesn't recognise one can fall back on `Display`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Phase {
    /// Hashing and compressing working-tree files.
    Hashing,
    /// Writing files into the working tree.
    Writing,
    /// Three-way reconciling files (merge, cherry-pick, rebase).
    Reconciling,
    /// Replaying commits onto a new base.
    Replaying,
    /// Assembling a pack to send.
    Packing,
    /// Moving a pack over the wire. Indeterminate — see below.
    Transferring,
    /// Verifying and inserting received objects.
    Importing,
    /// Re-hashing stored objects to check integrity.
    Verifying,
    /// Scanning the object store for unreachable objects.
    Collecting,
}
```

`Phase` carries a `Display` giving a default plain-English label, the same way
[`Error`] and [`fsck::Cruft`] already own their wording — an observer gets
something printable for free and can override any of it.

**How it is supplied.** Configured on the `Repo`, consumed by value so there is
no mutating setter and no interior mutability:

```rust
let repo = Repo::open(&root)?.observing(bar);   // or `.observing_none()` implicitly
let guard = repo.write()?;
sync::pull(&guard, "origin", &spawn)?;          // signature unchanged
```

Commands reach it through the `&Repo` / `&WriteGuard` they already take, so **no
command signature changes**. `sync::clone` is the exception — it has no
repository yet, so it takes the observer as a parameter alongside `Spawn`:

```rust
sync::clone(url, dir, &spawn, Some(&bar))?;
```

That asymmetry is real but honest: clone genuinely has nothing to configure until
it has created the repository.

**Three deliberate exclusions.**

*Phases are flat, never nested.* A rebase reports `Replaying` once per commit and
stays silent about the `Reconciling` happening inside each one. Nesting roughly
doubles the trait surface and forces every consumer to handle depth; git's
progress is flat for the same reason.

*No cancellation.* Returning `ControlFlow` from `advance` would make it nearly
free, but "what state is left behind when a fetch is cancelled halfway" is a
correctness question that deserves its own design — and the answer depends on
2.1. Adding it later is not a breaking change.

*`Transferring` reports bytes but no total.* The transport now moves a pack in
64 KiB chunks in both directions, so a slow link shows a live byte count instead
of a static marker. It stays **indeterminate** because the pack is framed by EOF,
not a length prefix — the reader cannot know the size in advance. Giving it one
means length-prefixing the pack, and since `serve-upload` / `serve-receive` run on
the far host (possibly an older build) that is a wire-format break needing a
negotiated protocol version. Deliberately not done.

**Sites to wire.** Each already has a loop; none needs restructuring.

| Site | Phase | Total known | Parallel |
|---|---|---|---|
| `save` | `Hashing` | yes | **yes** |
| `restore` (write, ghosts) | `Writing` | yes | **yes** |
| `stash` push/pop | `Hashing` / `Writing` | yes | **yes** |
| `merge`, `cherry-pick`, `rebase` via `apply` | `Reconciling` | yes | no |
| `rebase` | `Replaying` | yes | no |
| `bundle`/`fetch`/`push` import | `Importing` | yes | no |
| `bundle create`, `push` | `Packing` | yes | no |
| `fsck` | `Verifying` | yes | no |
| `gc` | `Collecting` | no (streams a dir) | no |

The three parallel sites are what force `Send + Sync` on the trait.

**CLI side.** `velo-cli` implements `Observer` over a progress bar, rate-limited
in the implementation rather than in core. It must respect the existing rule that
`serve-upload`/`serve-receive` emit nothing but protocol on stdout — the server
paths get no observer, and a non-TTY stdout gets a silent one so piped output
stays clean.

### 2.1 In-memory trees ✅ **DONE**

```rust
guard.save_tree(SaveTree { branch, parent, message, entries }) -> Result<String>
repo.tree_at(&snapshot)              -> Result<Vec<TreeFile>>
repo.read_file_at(&snapshot, path)   -> Result<Vec<u8>>
repo.read_object(&hash)              -> Result<Vec<u8>>
```

Velo's model is "disk = snapshot", which is right for a version control tool and
wrong for an editor or a registry. `velo_core::tree` is the second adapter onto
the same store, with the filesystem walk as the first.

**Two findings shaped the API.**

*A branch tip is derived, not stored.* `branch_tip` takes the newest snapshot
carrying that branch name, so merely inserting a row moves the branch — there is
no ref to update or withhold. `save_tree` therefore takes the branch explicitly:
a consumer using its own name cannot disturb a human's branches, and one passing
`"main"` has said so. Nothing touches `.velo/PARENT` or the working tree, because
a headless consumer has no working tree and moving the position under one that
does would leave every file reading as modified.

*Content must be CRLF-normalised.* The filesystem path normalises before hashing;
`store_raw` does not. Storing raw `
` would mean the same logical content
landing in a different object depending on which adapter wrote it — and a file
restored with its CRLFs intact would re-hash to something else and read as
permanently modified, the same failure as the old `hash_mmap` bug.

Both are load-bearing and tested: one test asserts a disk save and an in-memory
save of identical bytes produce the *identical object*, and another builds a
repository entirely from memory and runs `fsck`, which recomputes every
content-addressed id — so the snapshot recipe provably matches `save`'s.

`FileKind` (regular / executable / symlink) is the public vocabulary rather than
`storage::MODE_*`, so a caller never handles a bare `i64`. Hashes are wrapped as
of [2.3](#23-newtypes--done): `SaveTree.branch` is a `BranchName`, `parent` a
`SnapshotId`, and `TreeFile.object` an `ObjectHash`.

### 2.2 Extract `velo-merge` ✅ **DONE**
The crate exists and `diff3(ancestor, ours, theirs) -> MergeResult` lives in it,
re-exported as `velo_core::merge` so a caller needs one dependency. That much
happened during the Phase 1 crate split rather than here.

The tests came with it. `crates/velo-merge/tests/props.rs` drives `diff3` from
outside the crate — unchanged-side symmetry, identical edits on both sides,
disjoint edits preserving both, and determinism — and `diff3.rs` alongside it
covers the same surface by worked example, including the `compute_conflict_hunks`
→ `Decision` → `build_resolved_content` round trip a resolver actually walks. The
engine is verified at its own boundary rather than through `velo-core`'s suite.

**One algorithm, two shapes of answer.** `diff3` returns a `MergeResult`;
`try_auto_merge` returns `Option<String>` for the callers that only act on the
clean case — `reconcile` among them. These were parallel copies of the same walk
(compute the hunks, bail if any, otherwise `build_resolved_content` with
identical arguments), which meant either could drift silently. `try_auto_merge`
is now a wrapper that flattens `diff3`'s outcome, so the duplication is gone and
the equivalence is structural.

Writing the tests is what surfaced it, and a property still asserts the
agreement: across conflicting input and each clean shape (random triples nearly
always conflict, so the clean side has to be constructed deliberately). It holds
by construction now, but that is a fact about the implementation rather than the
signatures — re-inlining the wrapper would reopen a blind spot nothing else
covers, since every other property enters through `diff3` while `reconcile` calls
only `try_auto_merge`. The gap was verified real before the wrapper landed:
dropping `try_auto_merge`'s trailing-newline argument to `false` failed this
property and nothing else in the workspace.

`velo-core` no longer duplicates any of it. It never wrapped the engine
(`reconcile` calls `velo_merge::try_auto_merge` directly), so what its suite
tests is the other side of the boundary: deciding *when* a line merge is even
attempted (binary content and symlinks can't be, a mode-only change needs no
merge) and the merge/resolve commands on top. Those need a repository, which is
the line between the two suites.

**Pluggable merge drivers** registered by path glob or content type (so a
spreadsheet or document tool can supply structure-aware merging, falling back to
the line engine) — 🟡 recommended, but design the registration point now even if
only the default driver ships.

### 2.3 Newtypes ✅ **DONE**
`SnapshotId`, `ObjectHash`, `BranchName` and `TagName` are newtypes over `String`
in `velo_core::ids`. Everything they share — `Deref<Target = str>`, `Display`,
`FromStr`, `ToSql`/`FromSql`, comparison against text — comes from one macro; the
per-type part is the validation.

**Ids are typed; specs are not.** A *spec* is what a person types: `HEAD`, `v1.0`,
`a1b2c3`, `main`, `origin/main`. Any string is a plausible attempt at one, so
specs stay `&str` — wrapping them would add ceremony and catch nothing. An *id* is
what resolving a spec produces, so `resolve_snapshot_id(&repo, "v1.0")` takes
`&str` and returns `SnapshotId`. "Resolve before you look something up" is now a
signature rather than a convention: `repo.tree_at("v1.0")` does not compile.

Where the invariant is sharpest is `velo_core::tree`, whose two id-shaped fields
were previously interchangeable `String`s — `save_tree` could be handed a hash
where a branch belonged and would cheerfully create a branch named after it.

The CLI parses names at the argv boundary (`let name: TagName = name.parse()?`),
so a malformed one is a clear error before anything touches the repository:

```
$ velo tag "bad name  "
error: 'bad name  ' is not a valid tag name.
```

**Two things learned wrapping them.**

*`Display` must use `f.pad`, not `f.write_str`.* `write_str` silently ignores
width and alignment, so `{:<20}` formats correctly for a `String` and does
nothing for a newtype — which quietly un-aligned `velo tag`'s table. Caught by
running the binary, not by the suite; there is now a test asserting `{:<8}`,
`{:>8}` and `{:^8}` all match what the equivalent `&str` produces.

*Comparison against text has to be implemented in both directions.* Without
`PartialEq<str>`/`PartialEq<String>` and their mirrors, every `name == "main"` and
every assertion against a value read from SQL needs an `.as_str()`. That gives up
nothing: comparing to a literal is not the same as *passing* one where an id
belongs.

`from_stored` is `pub(crate)` and unvalidated, for values coming out of the
repository — validating on the way out would turn a corrupt row into a panic
somewhere unhelpful, which is `fsck`'s job to report. `FromStr` is the only
public way in.

**Deferred from here** and landed separately as
[format v2](#format-v2--the-phase-0-break-): the full-hash and typed-timestamp
decisions, which change the on-disk format and had to go in together.

---

## Format v2 — the Phase 0 break ✅ **DONE**

The four decisions locked in Phase 0 landed as **one commit**, because each one
changes every snapshot id and shipping them separately would mean four
id-invalidating migrations. D4 (`PRAGMA user_version`) arrived early in 1.5; D1,
D2 and D3 landed here. `docs/FORMAT.md` is the normative spec and now describes
v2 as implemented, with v1 retained only as documentation of what old data looks
like.

| | v1 | v2 |
| :--- | :--- | :--- |
| Snapshot id | BLAKE3 truncated to 16 hex, **and that truncation was the primary key** | full 64 hex stored; 16 is display only |
| Timestamp | text `%Y-%m-%d %H:%M:%S%.3f`, hashed as that text | `created_at_ms INTEGER`, hashed as decimal epoch ms |
| Metadata | nowhere to put it | `snapshot_meta` table, **covered by the id** |
| Domain separator | `velo-snapshot-v1\n` | `velo-snapshot-v2\n` |
| Bundle magic | `VELOBND1` | `VELOBND2`, with a metadata section |

### Snapshot metadata is hashed

`velo_core::SnapshotMeta` holds app-namespaced `(namespace, key) → value` pairs,
attached through `SaveTree.meta` and read back with `Repo::snapshot_meta`. Being
part of the id makes it **immutable** — changing a value produces a different
snapshot — which is the point: metadata is mostly provenance, and provenance that
can be silently rewritten is worth nothing. A test asserts that rewriting a
metadata row makes `fsck` report an id mismatch.

It also means metadata *must* travel with bundles and sync, or a receiver's
recomputation would disagree. That is exactly how the wire format is tested:
the round-trip test asserts the receiver passes `fsck`, which recomputes every id,
so a dropped metadata section fails loudly rather than quietly.

Two things fell out of the design. Entries live in a `BTreeMap` keyed on
`(namespace, key)`, so canonical ordering is structural rather than a rule callers
must remember — two callers inserting the same pairs in either order get the same
id. And NUL is rejected in all three fields, because NUL is the recipe's
separator, so allowing it would let two distinct metadata sets hash identically.

### v1 repositories are refused, not migrated

Strategy A from the spec. `open()` and `open_and_migrate()` both fail with
`Error::FormatTooOld`, distinct from `MigrationRequired` because there is nothing
the caller can call to fix it. Stamping v2 over v1 rows would leave 16-character
v1-recipe ids in a database claiming to be v2, and `fsck` could then only report
the damage after the fact. The refusal leaves `user_version` untouched, so a
failed open is never a partial upgrade.

A subtlety worth recording: **`user_version = 0` means v1**, because v1 wrote no
marker at all. But `init_db_at_path` never stamped the version either, so a
freshly created repository also sat at 0 — harmless while `0` meant "current",
and fatal the moment it meant "v1". Fresh repositories are now stamped at
creation. Without that, every `velo init` would have produced a repository the
next command refused to open.

The v1 `ALTER TABLE` sniffing migrations are gone with it. They existed only to
bring a v1 repository forward, so keeping them would have meant maintaining a
migration chain nothing could reach. The schema is one idempotent definition that
doubles as the migration for a v2 repository written by an earlier build.

### What the break exposed in the renderers

Full-width ids made two latent inconsistencies visible immediately, both of the
same shape as the drifted hunk printers from 1.3:

*Five `short_date` helpers* each sliced the stored timestamp text at a
hand-picked width — 10, 16, 19 characters — which only worked because the stored
form happened to be `YYYY-MM-DD HH:MM:SS.mmm`. With an integer column there is no
text to slice. They are now three named formats in `render/when.rs`, and the
widths that were implicit are asserted.

*Three local `short` helpers and four inline `[..8]` slices* abbreviated ids
differently in different commands. Invisible when ids were 16 characters; with
64-character ids `velo history` printed a 64-wide column. One shared
`render/id::short` now abbreviates everywhere, and the history column is the
display width rather than the stored length.

Neither was caught by the suite. Both were caught by running the binary — which
is the second time in this phase that formatting bugs got through green tests
(see the `f.pad` note in [2.3](#23-newtypes--done)).

---

## Phase 3 — Make consumers pleasant 🟡/🟢

Nothing here blocks a first consumer.

| Item | Marker | Notes |
| :--- | :--- | :--- |
| **Feature flags** — `default = []`, opt-in `ssh`/`tui`/`cli`/`bundle` | 🟡 **Recommended** | A GUI shouldn't compile clap and a TUI to save a file. Do it while the crate graph is being drawn in Phase 1 — retrofitting flags means re-auditing every `use`. |
| **Programmatic ignore rules + scoped roots** | 🟡 **Recommended** | "Only track `prompts/**`" without writing `.veloignore` to a user's disk. Real consumers hit this immediately. |
| **Progress + cancellation** on `clone`/`restore`/`gc`/large `save` | 🟢 Optional | `Options` struct with `Option<&mut dyn FnMut(Progress)>` + cancellation token. **Never a global.** |
| **Change notification** — `repo.head_token()` to poll | 🟢 Optional | Enough for a GUI to notice a background commit. A full watcher is over-engineering until asked for. |
| **`velo-testkit` published** | 🟢 Optional | Seeded in 1.7; formalise and publish only when a second project needs it. |
| **`velo-render`** — shared pretty output | 🟢 Optional | Only if a second tool genuinely wants Velo's tables/graph. Otherwise rendering stays in `velo-cli`. |

---

## Phase 4 — Ecosystem 🟢

| Item | Marker | Notes |
| :--- | :--- | :--- |
| `docs/FORMAT.md` + `CHANGELOG.md` | ✅ **done** | Both exist. `CHANGELOG.md` starts at the format v2 break, as planned. |
| Publish `velo-core` to crates.io | 🟢 Optional timing | See ⛔ below on placeholder releases. |

---

---

# Consumer feedback — Phases 5 to 9

Two real consumers now exist. `examples/prompt-registry` (no working tree) drove
the fixes recorded in its `FINDINGS.md`. **Velum** — an editor using velo as its
history backbone, with a working tree it manages itself — is the second, and its
feedback is the source of the phases below.

Every claim here was re-verified against the code before being written down; a
plan built on unchecked assertions is worth nothing. Where a defect is asserted,
it was reproduced.

The ordering is deliberate and differs from the order the feedback arrived in:
**wrong answers, then the irreversible decision, then missing capability, then
consistency, then ecosystem.** That is the same order Phases 0–4 used, and for
the same reason — the cost of doing an item later rises fastest for the ones
nearest the top.

> **Sequencing constraint that governs all of it:** `velo-core` is not on
> crates.io. Every breaking change below is free today and a major version after
> Phase 9. **Phase 8 must land before Phase 9**, or the consistency work becomes
> a semver event instead of an afternoon.

---

## Phase 5 — Wrong answers ✅ **DONE**

Not ergonomics. These produce results that are incorrect.

### 5.1 `history --file` matched presence, not change ✅

`touched()` asks whether the path exists in that snapshot's tree:

```sql
SELECT EXISTS(SELECT 1 FROM file_map WHERE snapshot_hash = ? AND path = ?)
```

A velo tree is the *complete* file set, so this is true for **every snapshot
since the file was created**. Reproduced: a repository with one snapshot touching
`README.md` and three touching other files lists all four.

```
$ velo history --file README.md
→ bd5488bc  unrelated change 3
  3188dfe8  unrelated change 2
  bb5e7622  unrelated change 1
  660dbae9  add README
```

The CLI help promises "snapshots that touched this file". This is wrong for CLI
users, not only embedders. The fix is to compare the path's object hash against
the same path in the parent — one extra lookup per candidate — and treat
"absent in one, present in the other" as a change.

**Second defect in the same code path:** the limit was applied *before* the
filter. `run` fetched `limit` rows and then `entries.retain(…)`, so "the last 20
snapshots touching this chapter" returned whatever subset of the last 20 overall
happened to touch it.

**Third, found while fixing the first two:** a directory matched *nothing*. The
query compared the path for equality, while the CLI has always advertised "file
or directory".

**Fixed.** The predicate compares `(path, hash, mode)` rows under the filter
against the parent's, in both directions — so additions, deletions, edits and
mode flips all count, and a deletion (which appears only in the parent) is not
missed. A directory matches by prefix, with `%` and `_` escaped since both are
legal in filenames. When a filter is set the query runs unbounded and the limit
is applied afterwards, so it counts matches rather than candidates.

The existing test asserted the buggy behaviour and its comment described it as
the contract — "the filter is *was this path in the snapshot*, not *did this
snapshot modify it*" — while the help text said the opposite. The help text was
right. Four tests now cover change-not-presence, deletions and mode flips,
directories, and limit-after-filter.

### 5.2 `SaveTree` could not record a merge parent ✅

The `snapshots` table has `merge_parent`. `SnapshotIdentity` takes a
`merge_parent`. `history::Entry` exposes it and offers `is_merge()`. Every layer
supports it **except the one an embedder writes through**, so a snapshot created
via `save_tree` can never be a merge:

```rust
pub struct SaveTree<'a> {
    pub branch: &'a BranchName,
    pub parent: Option<&'a SnapshotId>,
    pub message: &'a str,
    pub entries: Vec<TreeEntry>,
    pub meta: SnapshotMeta,
}
```

The consequence is not cosmetic. A merge recorded as a linear commit makes
`history --graph` draw velo's headline two-parent topology as a straight line,
and — worse — merge-base computation returns an ancestor that is too old, so the
*next* merge re-raises conflicts the author already resolved. "It keeps asking me
the same question" is the classic symptom of a lost merge parent.

**Not a format break:** the column, the recipe and the reader all exist. It is
one field, `merge_parent: Option<&'a SnapshotId>`.

**Fixed.** `merge_parent: Option<&'a SnapshotId>`, validated like the first
parent — naming one that does not exist is refused rather than left for `fsck`.
Two tests: a merge recorded through `save_tree` reports both parents and answers
`is_merge()`, with `fsck` recomputing the id to prove the second parent was
*hashed in* and not merely written to a column; and an unknown merge parent is
`NotFound`.

[7.1](#71-savetreetimestamp_ms) adds another field to `SaveTree`. Doing it next
keeps the number of breaking changes to that struct at two rather than three.

---

## Phase 6 — The last irreversible decision ✅ **DONE**

### 6.1 Authorship ✅

`snapshots` is `hash, message, branch, parent_hash, merge_parent, created_at_ms`.
Nothing records **who**. velo has `clone`, `push`, `pull` and `bundle` — it is
built for more than one person and cannot answer the first question anyone asks
of shared history. Every consumer will otherwise invent its own convention, and
they will all differ.

The feedback framed this as a choice between a hashed field (breaks the format),
a plain column (rewritable, which for authorship in a synced repository is close
to worthless), or a documented convention (not really a decision).

**There is a fourth option, and it is better than all three: the reserved
metadata namespace already built for exactly this.**

- `snapshot_meta` rows are **already hashed into the snapshot id** (decision D1),
  so an author stored there is tamper-evident — `fsck` reports a rewritten one as
  an id mismatch. That is the entire argument the hashed-field option was making.
- `RESERVED_NAMESPACE = "velo"` already exists and `SnapshotMeta::set` already
  **rejects it for application callers**, so no consumer can squat the key.
- The table already exists, already travels with bundles and sync, and is already
  covered by `fsck`. **No format break. No `FORMAT_VERSION` bump.**

What is missing is only the typed way in — and it must be typed, not a documented
string convention, or the point is lost:

```rust
pub struct Author { pub name: String, pub email: Option<String> }

pub struct SaveTree<'a> {
    pub author: Option<&'a Author>,   // written to the reserved `velo` namespace
}
```

Honest costs, so this is decided with eyes open:

| Cost | Assessment |
| :--- | :--- |
| Snapshots written before it have no author | Unavoidable under every option, including a format break |
| Authorship is per-snapshot, not per-repository identity | Correct — it is what git does |
| `velo save` needs a source for it | The CLI reads config/env and passes it in; core still reads no environment |
| Slightly more metadata per snapshot | Two short rows |

**Done, and it cost no format change.** `Author` carries a name and an optional
email, `SaveTree.author` records it, `SnapshotMeta::author()` reads it back, and
`SnapshotMeta::set_author` is `pub(crate)` so the reserved namespace stays the one
sanctioned route — an application still cannot forge an author through
`SnapshotMeta::set`, which a test asserts.

The proof that hashing was the right call is also a test: forging
`snapshot_meta.value` directly in the database makes `fsck` report the snapshot,
because the id commits to it. A plain column would have accepted the forgery
silently.

`velo save` reads `VELO_AUTHOR_NAME` and `VELO_AUTHOR_EMAIL` in `velo-cli`, since
core reads no environment. An absent author is not an error — velo has always
recorded snapshots without one, and refusing to save until a variable is set
would be a poor trade for a tool that is useful single-player. A *malformed* one
is an error rather than being dropped, because whoever set it meant it.

One thing this exposed: `save` had never written to `snapshot_meta` at all, since
it always passed an empty set. It does now — an id that commits to metadata the
repository does not hold fails its own `fsck`, so the rows and the id have to land
in the same transaction.

**Still to wire:** `cherry_pick` and `rebase` create snapshots through
`save::run`, which takes no author, so those record none. Both signatures are
being reshaped in [8.1](#81-refs-are-still-strings-in-most-write-commands)
anyway, and threading it there is one change rather than two. This costs nothing
irreversible — authorship is metadata, so adding it later needs no migration,
which is precisely the property that made the reserved namespace the right
choice.

⛔ **Do not add a plain `author` column.** Rewritable provenance in a synced
repository is worse than none, because it looks trustworthy. This is the same
argument `meta.rs` already makes for hashing metadata.

---

## Phase 7 — Finish the embedder API 🔴/🟡 *(7.5 and 7.6 remain)*

Capability an embedder cannot supply for itself. Each of these forces a consumer
to reimplement logic velo already has, or blocks something outright.

### 7.1 `SaveTree.timestamp_ms` ✅ **DONE**

The clock is read *inside* `save_tree`, and the timestamp is part of identity, so
a caller cannot control it. Two consequences, both larger than they look.

**Embedders cannot write reproducible tests.** An id changes every millisecond,
so no test can assert one against a constant. Velum's history tests compare ids
to each other because anything else would be flaky — which rules out precisely
the golden-file testing a storage format most wants: build this exact tree,
assert this exact repository.

**History cannot be imported.** A git→velo importer is the most obvious ecosystem
tool velo could have, and it cannot be written: every commit would be stamped
with the moment of the import. Same for any migration, and for restoring a backup
with its dates intact.

It also contradicts velo's own stated boundary. `lib.rs` says the crate "reads no
process environment" and that "anything that depends on the surrounding process is
passed in". **The wall clock is the surrounding process** — the last ambient
dependency, and the one baked into content-addressed identity.

```rust
/// When this snapshot was made, or `None` for now.
pub timestamp_ms: Option<i64>,
```

`None` preserves every existing caller. Not a format break.

**Done.** A golden test now asserts an id against a constant — the thing that was
impossible before — and a second replays three 2021 dates through `save_tree` and
reads them back, which is the importer case in miniature.

One consequence is pinned rather than prevented: a branch tip is *derived* as the
newest snapshot on the branch by `created_at_ms`, so a snapshot saved with a
timestamp **older than the current tip does not become the tip**. That follows
from the design and an importer replaying in order never meets it, but anything
supplying out-of-order timestamps needs to know, so it is a test rather than a
sentence.

`save_tree` is now the only place in `velo-core` that reads a clock, and it does
so only when the caller declines to.

### 7.2 A branch cannot be pointed at a past snapshot ✅ **DONE**

`branches::list` and `branches::delete` are public; `register_branch` is
`pub(crate)`. So a branch is created either by `switch::run` — which also makes it
current, and leaves it unborn — or as a side effect of `save_tree` recording a
snapshot on it. Neither offers what `git branch <name> <commit>` does.

```rust
pub fn create(guard: &WriteGuard, name: &BranchName, at: Option<&SnapshotId>) -> Result<()>;
pub fn set_tip(guard: &WriteGuard, name: &BranchName, to: &SnapshotId) -> Result<()>;
```

Related and smaller: creating a branch and switching to it are one operation, and
a consumer that wants the first without the second cannot ask.

### 7.3 Merge-base is private ✅ **DONE**

`lowest_common_ancestor` is a private recursive CTE in `commands::merge` —
indexed, fast, correct. Any consumer that merges must reimplement a subtle
algorithm over `history::Entry` rows in memory.

```rust
pub fn merge_base(repo: &Repo, a: &SnapshotId, b: &SnapshotId) -> Result<Option<SnapshotId>>;
```

### 7.4 A snapshot cannot be inspected by id ✅ **DONE**

`Repo::snapshot_meta(&SnapshotId)` exists, but there is no way to get a snapshot's
message, timestamp, parents or branch from an id. `show::run` takes a `&str` and
resolves it again; `history::run` walks ancestry. A consumer holding an id — which
is what every consumer holds — must format it back into text for re-resolution.

```rust
pub fn snapshot(&self, id: &SnapshotId) -> Result<history::Entry>;
```

**Done**, returning `history::Entry` rather than a near-duplicate type — it
already carries the message, timestamp, both parents, branch and tag, already
typed. `show::run` stays the way to get the diff as well, since computing that is
not free.

### 7.5 `merge::run` is unusable by anything with an interface ✅ **DONE**

It requires a clean working tree, writes files to disk, and persists conflict
state. An editor can use none of that: buffers are dirty by definition, and
nothing may touch disk before the author has seen the merge. The pure
`velo_merge` functions are the right seam — but every consumer that merges will
first write the same tree-classification pass.

```rust
/// Classify every path. No side effects.
pub fn plan(repo: &Repo, ours: &SnapshotId, theirs: &SnapshotId) -> Result<MergePlan>;
```

**Done** as `merge::plan(repo, ours, theirs) -> MergePlan`. It takes a `&Repo`,
not a `&WriteGuard`, which is the API saying it cannot write.

A conflict carries its three sides *by object*, so a caller can read and present
them, and an auto-merge carries the merged content — deciding "auto-merged rather
than conflicted" computes it, and throwing it away would make every caller redo
the merge. `PlannedChange::action()` reports in the same vocabulary the
working-tree merge uses.

The test asserts the absence of side effects directly: file contents unchanged,
the dirty set identical, no `MERGE_HEAD`, no active merge, `fsck` healthy.

Folding `merge::run` into `plan` plus a working-tree write is the obvious
follow-on and is left for [8.2](#82-positional-arguments-and-booleans-that-are-really-enums),
where that signature is being reshaped anyway.

### 7.6 Per-call progress and cancellation 🟡 *(restore done)*

This project's own anti-goals say, in bold: *"Don't use globals for progress or
cancellation. Pass them per call."* `Repo::observing` consumes and returns `Self`,
so the observer is set once per handle and fires for everything — a consumer
wanting a bar for one `restore` gets callbacks from every operation, with no way
to tell which a `Phase` belongs to. **The anti-goal was right and simply was not
implemented.**

Cancellation does not exist at all — nothing in `velo-core` matches `cancel`. For
a GUI, "stop this" on a workspace-wide restore or a large clone is table stakes.

Per-call options carrying an observer and a cancellation token on `restore`,
`clone`, `gc` and large `save`. Keep `Repo::observing` as the convenience default.

> **Deliberately sequenced with [8.2](#82-positional-arguments-and-booleans-that-are-really-enums),
> not done here.** Both an observer and a cancellation token have to *reach* the
> command, and there is no way to pass them per call without changing those
> signatures — which is exactly what 8.2 does. `restore::run` has 42 call sites
> and `save::run` 35; reshaping them twice, once to add progress and again to add
> an options struct, is churn for no gain. When 8.2 introduces
> `restore::Options`, `observer` and `cancel` are two more fields on it.
>
> This is the one Phase 7 item that is *not* additive, which is why it is the one
> that waits.

**Done for `restore`**, alongside its options struct in 8.2 exactly as planned.
`progress::Cancel` is a cloneable flag checked between files, so cancelling never
interrupts a write; `Error::Cancelled` is the answer. `restore::Options.observer`
overrides the handle's for that call, and a test asserts the handle's observer is
**not** consulted — otherwise it is still a global with extra steps.

Cancellation is deliberately not a rollback: files already written stay written,
which is the same position a killed process leaves and which `status` describes
accurately. Promising atomicity over a working tree would be a lie.

`gc`, `clone` and large `save` still take no options; they follow the same shape
when theirs land.

### 7.7 `Repo::head_token` ✅ **DONE**

A second window, a `pull`, or the user running `velo` in the same folder all
change the repository under a running application. Listed as 🟢 optional in Phase
3; for any GUI it is the difference between polling one integer and polling every
branch tip.

```rust
pub fn head_token(&self) -> Result<u64>;
```

**Done.** Hashed over the snapshot count and newest rowid, plus every branch and
tag row — because a branch moving or a tag being added changes nothing a count
would notice. Deliberately does not cover the working tree.

---

## Phase 8 — Finish the consistency passes ✅ **DONE**

`FINDINGS.md` made two arguments — typed ids beat strings, options structs beat
positional arguments — and both were applied to the commands that example
happened to use, then stopped. These are the same fixes, applied to the rest.

**Do this before Phase 9.** All of it is breaking, and all of it is free until
`velo-core` is published.

### 8.1 Refs are still strings in most write commands ✅ **DONE**

`tag::create` was changed to take `Option<&SnapshotId>` because "a caller holding
an id handed back text to be resolved a second time". Unchanged since:

| Command | Today |
| :--- | :--- |
| `cherry_pick::run` | `target: &str` |
| `rebase::run` | `target: &str` |
| `merge::run` | `target_branch: Option<&str>` |
| `show::run` | `target: &str` |
| `blame::run` | `at: Option<&str>` |
| `restore::run` | `snapshot_hash: &str` |
| `bundle::create` | `target: Option<&str>` |
| `grep::run` | `snapshot: Option<&str>` |

The double resolution is not only inelegant: a consumer can be handed
`AmbiguousPrefix` for an id it already holds unambiguously.

**Done — with one correction to the table.** `merge::run`'s target is
*legitimately a spec* and stays `&str`. An exact local-branch tip must win over a
hash prefix so a short branch name is never mis-read as one, tags and remote refs
fall out of the fallback, and the branch **name** is what `MERGE_HEAD` records for
the eventual save to resolve into a second parent. A `SnapshotId` would discard
that — the same reasoning `resolve_snapshot_id` applies to its own input.
Everything else in the table takes an id, and the CLI resolves at the argv
boundary.

### 8.2 Positional arguments, and booleans that are really enums ✅ **DONE**

```rust
grep::run(repo, pattern, snapshot, case_insensitive, names_only, context)
restore::run(guard, snapshot_hash, force, paths)
rebase::run(guard, target, abort, cont)      // two bools, three real modes
merge::run(guard, target_branch, abort)
```

`rebase::run` deserves singling out: `abort` and `cont` encode three states in two
booleans and the fourth combination is meaningless. That is an enum.

**Mode enums done** for `rebase` and `merge`, which also let the CLI's hand-rolled
"specify a target" check and its `process::exit` go away — the enum makes the
missing case something the caller has to handle when constructing it.
`grep::Options` landed with [8.1](#81-refs-are-still-strings-in-most-write-commands--done).

`restore::Options` and `save::Options` followed, each carrying the observer and
cancellation token [7.6](#76-per-call-progress-and-cancellation--restore-done)
owed them — doing both at once is why 7.6 waited. `save::run_with_paths` is gone;
there is one entry point.

`cherry_pick::run` and `rebase::run` gained the author that
[6.1](#61-authorship-) could not thread until their signatures moved, so the
snapshots they create on a user's behalf are attributed like any other.

### 8.3 Filesystem paths are `&str`

`bundle::create(repo, file: &str, …)`, `bundle::apply(guard, file: &str)` and
`restore::run(…, paths: &[String])` take text where they mean paths, while
`Repo::init` and `discover` already take `&Path`. On Windows especially this is a
class of quoting and separator bugs that only appear on someone else's machine.

### 8.4 The core parses argv ✅ **DONE**

`diff::dispatch(repo, args: &[String], paths: &[String])` interprets raw
command-line arguments — `a..b` range syntax included — inside `velo-core`. A
consumer holding two ids must format them into a string so velo can parse them
back.

```rust
pub fn between(repo: &Repo, a: &SnapshotId, b: &SnapshotId, paths: &[&Path]) -> Result<Diff>;
```

…with `dispatch` moving to `velo-cli`, where argv comes from.

**Done.** `between` takes ids and `&[&Path]`; `dispatch`, `split_range` and
`is_path_like` now live in `velo-cli/src/diffargs.rs`. Core kept one piece —
`diff::tracks_path` — because "did this snapshot track this path" is a repository
question, and it is what stops a since-deleted file being mistaken for a ref.

Labels moved too, and that is the more interesting half. `run_range` built
`main (a1b2c3d4)` from the spec it was handed, so core knew what the user typed.
It no longer does: it labels with the abbreviated id and the CLI prepends the
spec — the rule 1.3 established, applied to the last place still breaking it.

The two tests asserting argv interpretation moved with the code, and gained a
case: more than two refs is refused rather than silently ignoring the extras.

### 8.5 Shrink the public surface — **gates Phase 9** ✅ **DONE**

`commands` exports `remove_empty_parents`, `decision_to_db`, `decision_from_db`,
`read_text`, `is_binary`, `get_tracked_files`, `reconcile_file`, `find_repo_root`.
These read as internals that became `pub` because two modules needed them.

The moment velo is on crates.io, everything public is a semver commitment. One
pass asking of each item, *"would I promise this for five years?"*

**Done.** Nine items are `pub(crate)` now — `get_tracked_files`, `is_binary`,
`FileRef`, `Reconcile`, `reconcile_file`, `remove_empty_parents`,
`decision_to_db`, `decision_from_db` and `read_text`. Checked before demoting:
none was used outside `velo-core`. `commands`' own surface went from 16 items to
10.

Four were kept public against the feedback's list, each for a reason:

| Kept | Why |
| :--- | :--- |
| `find_repo_root` | The CLI needs the root *before* deciding whether to open, to phrase "not a repository" usefully |
| `snapshot_id` / `SnapshotIdentity` | The id recipe is the format's contract; a consumer verifying ids needs it |
| `SNAP_HASH_LEN` / `SNAP_ID_LEN` | Display width and stored width; renderers use the first |
| `get_dirty_files` / `FileStatus` | "What is unsaved" is a real consumer question |

`snapshot_timestamp_ms` and `timestamp_from_ms` stay too — the second is used by
every renderer that prints a date.

### 8.6 Say which half of the API owns the working tree ✅ **DONE**

velo has two APIs that look like one. `save`, `status`, `restore`, `switch`,
`merge`, `resolve` and `stash` read and write **files on disk**. `save_tree`,
`tree_at`, `read_file_at`, `read_object` and `snapshot_meta` never touch it.

Both consumers so far needed the second column, and neither could tell which was
which without reading implementations. One table in the crate docs fixes this
permanently, and it is the cheapest item in this entire plan.

**Done**, as three rows in the crate docs rather than two: *writes your files*,
*reads your files*, and *store only*. The middle row matters — `save`, `status`,
`diff`, `grep` and `squash` do not write, but their answers depend on what is on
disk, which is just as surprising to an embedder that has none. Classified by
reading what each module actually touches, not by reputation.

### 8.7 Smaller items ✅ **DONE**

- **`save_tree` has no "nothing changed" guard.** `velo save` refuses an empty
  save; the primitive does not, so the same tree handed to it twice records a
  second snapshot. Defensible for a primitive — but every embedder needs the
  guard, and the failure mode is a history full of duplicates rather than an
  error. ✅ **Documented** on `save_tree`, with the comparison a consumer wants —
  `repo.tree_at(&parent)? == proposed` — rather than adding a guard. Refusing
  would make it impossible to record "checked at this moment, unchanged", which a
  consumer may genuinely want; a policy like that belongs at the call site, not
  baked into the primitive.
- **`history::Options.file` is a single path.** A document plus its assets is 2+
  paths, so a consumer runs the query twice. `paths: &[&Path]`. ✅ **Done** — a
  snapshot qualifies if it changed *any* of them, the limit still counts matches
  across the whole set (merging two capped result sets does not give the newest N
  across both), and `velo history --file` is repeatable to match.
- **Programmatic ignore rules + scoped roots** (already 🟡 in Phase 3). Velum
  writes `.veloignore` into the *user's* workspace to exclude its own cache
  directory — an application putting a file in someone's folder for its own
  benefit. ✅ **Done** as [`Scope`](crate::Scope), configured with
  `Repo::scoped`. `ignore()` subtracts, `only()` restricts, both in gitignore
  syntax so there is one syntax rather than two.

  It belongs to the handle rather than to each call — unlike an observer, which
  describes *one operation*, a scope describes what the repository contains as
  far as this application is concerned, and that does not change between
  operations.

  **A scope narrows and can never widen**, which took two attempts to get right.
  The obvious implementation hands both halves to the walker as `ignore`-crate
  overrides — but a matching *positive* pattern is a whitelist there, and
  outranks `.veloignore`, so `only("**")` quietly re-included a file the user had
  excluded. Exclusions go to the walker (a purely negated override composes
  correctly); the restriction filters the walk's results instead, where it can
  only ever remove. A test pins it.

---

## Phase 9 — Ecosystem 🟢

| Item | Gated on | Notes |
| :--- | :--- | :--- |
| Publish `velo-core` to crates.io | **8.5** | Consumers currently pin a git revision, which `cargo deny` needs an explicit `allow-git` for. Publishing freezes whatever the surface is on that day. |
| **git → velo importer** | **7.1** | Impossible until a caller can supply timestamps. The most obvious ecosystem tool velo could have. |
| Publish `velo-testkit` | — | Formalise when a third project wants it |

---

## What not to change

The feedback was explicit that these are right, and both consumers independently
relied on them. Recorded so a future pass does not "improve" them:

- **The whole-tree snapshot model.** It is what makes an id verifiable, and
  `TreeEntry::stored` already removes the cost. ⛔ Do not make `save_tree` take a
  diff.
- **The synchronous core.** Velum wraps it in one actor thread per workspace.
  Async would have been pure cost.
- **No `--force`, fast-forward-only push, a `pull` that stops rather than
  guessing.** Velum adopted the same stance at the document level, directly from
  reading velo's README.
- **Hashed metadata.** The reason a consumer can record why a checkpoint exists
  and trust the answer — and, per [6.1](#61-authorship), the mechanism that makes
  authorship possible without a format break.
- **Refusing to open a v1 repository** rather than half-migrating it.
- **`fsck` recomputing every id.** Both consumers end their integration tests with
  it, which is how an embedder gets an end-to-end check on its own use of the
  format for free.

---

# Velum punch list — Phases 10 to 12

Velum's second round, after `4d5871b`. Every item was re-verified against the
source, and the one called a blocker was **reproduced** rather than inferred —
twice, once through the API and once on the command line.

**Nothing here touches the format.** Every item is an API addition or a query
fix, so no repository needs migrating whenever any of it lands.

## What the audit changed about the priority

Velum's list is accurate on every point. One thing it raised as a suspicion turns
out to be true and makes item 1 more urgent than "an embedder blocker":

> *"`merge::run` and `merge::plan` both call `merge_base`, so `velo merge`
> between the same pair of branches should have the same amnesia. Worth
> reproducing on the command line."*

Reproduced. `do_merge` calls `lowest_common_ancestor` directly
(`merge.rs:260`), so the CLI shares the defect exactly:

```
$ velo merge side          # conflict on line 2, resolved to RESOLVED, saved
$ velo switch side; …      # side changes only line 3
$ velo merge side          # line 2 conflicts again
  [Conflict] f.txt
```

The side branch changed line 3. Line 2 had been settled. It comes back, because
the base is the shared root rather than the tip the first merge absorbed. **This
is a correctness bug in velo's headline feature for every user who merges the
same branch twice**, not only for embedders.

`merge_base_follows_the_second_parent` in `crates/velo-core/src/tests.rs` is
`#[ignore]`d against this and asserts the correct answer, so it flips to passing
when 10.1 lands:

```bash
cargo test -p velo-core --lib -- --ignored merge_base_follows
```

---

## Phase 10 — Ancestry ✅ **Done**

Items 1 and 2 of the punch list, together, because **they want the same recursive
CTE**. A walk that follows both parents is what merge-base needs and what an
ancestry-scoped history needs; writing it twice would be the mistake.

### 10.1 `merge_base` must follow `merge_parent`

`lowest_common_ancestor` joins `s.hash = a.parent_hash` in both branches of both
CTEs. `merge_parent` is stored, drawn, reported and verified by `fsck` — and
never read by the code that consumes ancestry.

Three things the fix has to get right, all three correctly identified by Velum:

1. **Recurse through both parents.** `merge_parent` is `TEXT NOT NULL DEFAULT ''`,
   so the join must exclude the empty string rather than treat it as a hash.
2. **Guard the walk.** Two parents means nodes are revisited, so `UNION ALL` can
   blow up on a merge-heavy history. Use `UNION`, or a visited set. Note `anc_tgt`
   has **no depth guard at all** today — safe for a single-parent chain, not once
   it branches.
3. **"Lowest" is not "minimum depth" in a DAG.** `ORDER BY ac.depth ASC LIMIT 1`
   is right for a tree. With criss-cross merges the true merge base is a common
   ancestor with no *other* common ancestor reachable from it. Either compute
   that, or state the approximation in a comment — an approximation that is
   documented is fine; one that is assumed exact is a bug waiting for a history
   that criss-crosses.

Both callers benefit at once: `merge::plan` (Velum's path) and `do_merge` (the
CLI's).

### 10.2 `history` needs an ancestry scope

`Options` offers *recorded on a branch* or *everything*. The third scope — the
ancestry of a given snapshot — is what a timeline needs, and the walk already
exists as `Scope::CurrentBranch`, reachable only via `.velo/PARENT`, which
`save_tree` deliberately never sets.

```rust
pub struct Options<'a> {
    /// Ancestry of this snapshot, following both parents through merges.
    /// Mutually exclusive with `branch` and `all`.
    pub from: Option<&'a SnapshotId>,
}
```

Velum's symptom is a draft timeline that stops dead at the branch point, because
shared history carries `branch = "main"` and is filtered out. The workaround is
two queries plus a hand-rolled graph walk plus an intersection plus manual
truncation — and it puts the limit-after-filtering problem back into application
code, which is exactly what `paths` just took out of it.

Do this *with* 10.1, sharing the walk.

### What landed

One walk, `commands::ancestors`, used by both. It follows `parent_hash` and a
non-empty `merge_parent`, de-duplicates with `UNION` rather than `UNION ALL`,
stops at `MAX_ANCESTRY_DEPTH`, and returns each reachable snapshot with its
minimum depth. `lowest_common_ancestor` intersects two of these and takes the
shallowest, breaking ties by hash so the answer does not depend on row order.
Point 3 above was answered by documenting the approximation rather than
implementing the full DAG merge base: the comment says where it differs from
the true answer, so the next person to hit a criss-cross history finds the
reason rather than a surprise.

`ancestry_of` in `history` uses the same walk and sorts by time, since two
parents leave no single chain to follow.

Verified on the binary, not only in tests: the two-merge scenario from the punch
list now reports the absorbed tip as the ancestor and merges clean, and
`velo history` lists the snapshots the merge brought in.
`merge_base_follows_the_second_parent` is no longer `#[ignore]`d, and the suite
has no ignored tests left.

---

## Phase 11 — Finish the passes that stopped early 🟡 **Recommended**

Three items, each one a place where a pass was applied to some commands and not
others. None blocks Velum; each is a visible gap at a known Velum phase.

### 11.1 `blame` was missed by the typed-ids pass, and has nowhere for an author

```rust
pub struct LineOrigin {
    pub hash: String,          // → SnapshotId
    pub created_at: DateTime<Utc>,
    pub message: String,
    pub author: Option<Author>,   // ← new
}
```

`Blame.path: String` → `PathBuf`, `Blame.snapshot: String` → `SnapshotId`.

The author matters more than the typing here: Velum stores documents one sentence
per line *specifically* so per-line blame becomes per-sentence provenance, and the
gutter's question is literally "who wrote this sentence". A consumer can answer it
with a `snapshot_meta` lookup per distinct snapshot — but blame already walks that
history, and every consumer will otherwise write the same dedup.

### 11.2 `gc` needs options

`run(guard, keep_days)` — no observer override, no cancel, while `restore` and
`save` got both. `Phase::Collecting` already reports through the handle observer,
so a GUI cannot tell one operation's progress from another's, and cannot stop the
longest local operation velo has.

Cancelling is safe to define exactly as `restore` does: objects already collected
stay collected, which is an earlier stopping point rather than a broken state.

### 11.3 Sync needs cancellation, and `clone` needs a `&Path`

```rust
pub fn clone(
    url: &str,
    dir: Option<&str>,                    // → Option<&Path>, per 8.3
    spawn: &transport::Spawn,
    observer: Option<Box<dyn Observer>>,  // per-call already
) -> Result<Cloned>                       // and no Cancel
```

`clone` is the longest operation velo has — a whole history over a network — and
the only one that cannot be stopped. `fetch`, `push` and `pull` report through
`guard.phase(…)`, so they inherit the handle observer and take no `Cancel`
either.

With 11.2 and 11.3, [7.6](#76-per-call-progress-and-cancellation--restore-done) is
finally complete for all four commands it named.

---

## Phase 12 — crates.io 🟢

Unchanged from [Phase 9](#phase-9--ecosystem-), with the ordering now explicit:
**after Phases 10 and 11**. Those are most of what would otherwise force a
breaking release immediately after publishing, and 8.5 already shrank the surface
that publishing freezes.

Velum pins `rev = "4d5871b"`, which needs `cargo deny`'s `allow-git` and blocks a
release whose dependencies all resolve from a registry.

---

## Deliberately not doing

From Velum's own "what Velum does not need", plus the reasoning already recorded
elsewhere in this plan:

| Not doing | Why |
| :--- | :--- |
| Folding `merge::run` onto `merge::plan` | Good hygiene — one classification rather than two — but zero consumer impact, and 10.1 fixes the shared defect anyway |
| A git→velo importer | `timestamp_ms` made it possible; nothing needs it |
| Removing `Repo::observing` | Once 11.2 and 11.3 land, a handle-level default is a reasonable fallback rather than the global the anti-goal warned about |
| Per-call progress on `status` / `history` | Both are inside a GUI's latency budget already |

## Anti-goals

⛔ **Don't async-ify core.** SQLite is synchronous. Guarantee `Repo: Send` so
callers can `spawn_blocking`, and *document* that `rusqlite::Connection` is not
`Sync` — one `Repo` per thread, or `Arc<Mutex<Repo>>`. No work needed; the
current code is already fully sync. Adding async would be pure cost.

⛔ **Don't make `Repo` `Sync` or share one `Connection` across threads.** It
will appear to work and corrupt under load.

⛔ **Don't add `anyhow` to core.** It isn't there today — keep it that way.
`anyhow` in `velo-cli` is correct (flattening to a message is the CLI's job).

⛔ **Don't publish a placeholder release to reserve the name.** A stub `0.1.0`
means the first real release is `0.2`+ with an API you immediately break. Either
publish `0.0.1` explicitly marked unstable, or check availability and publish
when Phase 1–2 land.

⛔ **Don't use globals for progress or cancellation.** Pass them per call.

⛔ **Don't defer the Phase 0 decisions.** Hashed metadata, full-width IDs, and
timestamp representation are cheap now and format-breaking later.

⛔ **Don't abstract storage behind a trait "for flexibility" yet.** There is one
backend (SQLite + file objects). Add the seam when a second backend actually
exists, not on speculation.

⛔ **Don't split `velo-render` up front.** Rendering lives in `velo-cli` until a
second consumer wants the same output.

⛔ **Don't let the core discipline boundary be a convention.** Enforce it with
`#![deny(clippy::print_stdout, clippy::print_stderr, clippy::exit)]` in CI, or it
regresses within a month.

---

## Summary

| Phase | Content | Marker |
| :--- | :--- | :--- |
| **0** | Format decisions: metadata hashing, ID width, timestamps, schema versioning + `FORMAT.md` | ✅ **done** |
| **1** | Workspace split, typed errors, data-returning commands, repo handle + write guard, schema versioning, injectable transport, test relocation | ✅ **done** |
| **2** | Progress reporting, in-memory trees, `velo-merge` extraction, newtypes | ✅ **done** |
| **v2** | The Phase 0 format break implemented in one commit: full-width ids, epoch-ms timestamps, hashed snapshot metadata | ✅ **done** |
| **3** | Feature flags & scoped ignores 🟡; notifications, testkit, render 🟢 | 🟡/🟢 |
| **4** | Changelog, publishing | 🟡/🟢 |
| **5** | Wrong answers: `history --file` filters by presence; `SaveTree` cannot record a merge parent | 🔴 |
| **6** | Authorship — resolvable via the reserved metadata namespace, with no format break | 🔴 |
| **7** | Embedder API: caller-supplied timestamps, branch refs, merge-base, snapshot-by-id, side-effect-free merge plan, per-call progress + cancellation, head token | 🔴/🟡 |
| **8** | Finish the typed-ref and options-struct passes; shrink the public surface; document which half owns the working tree | 🟡 |
| **9** | crates.io, git importer, testkit | 🟢 |
| **10** | Ancestry: `merge_base` must follow `merge_parent` (a live `velo merge` bug), and `history` needs an ancestry scope — one walk serves both | ✅ |
| **11** | Finish the passes that stopped early: `blame` types + author, `gc` options, sync cancellation | 🟡 |
| **12** | crates.io, after 10 and 11 | 🟢 |

**12 of the original 16 items confirmed as-written.** Four premises corrected
(no `anyhow`; coupling is 3 files not pervasive; merge engine already pure;
object primitives already exist). Three items added (injectable transport, test
relocation, typed timestamps). Two items re-tiered (schema versioning promoted
to Phase 1; full-hash storage recognised as a format break, not display).
