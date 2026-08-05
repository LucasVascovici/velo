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

## Verified current state

| Claim | Reality |
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

> ⚠️ All four change snapshot IDs, so they land as **one atomic change**.
> Implementation is scheduled in Phase 1.5 (schema/versioning) and Phase 2.3
> (IDs, timestamps, metadata) — but they must be **committed together**, not
> shipped a phase apart. See
> [Migration v1 → v2](docs/FORMAT.md#migration-v1--v2); strategy **A (re-init)**
> is recommended while no repository has archival value.

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

## Phase 2 — Make it useful to non-CLI consumers 🔴

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

### 2.1 In-memory trees 🔴
```rust
repo.save_tree(entries: impl IntoIterator<Item = TreeEntry>, msg: &str) -> Result<SnapshotId>
repo.read_file_at(&SnapshotId, &Path) -> Result<Vec<u8>>
repo.read_object(&ObjectHash)          -> Result<Vec<u8>>
repo.tree_at(&SnapshotId)              -> Result<Tree>
```
Velo's model is "disk = snapshot", but an editor or registry holds content in
memory. This turns Velo into a general-purpose versioned content store, with the
filesystem walk as *one adapter* on top.

**Cheaper than it looks:** `storage::store_raw` and `storage::read_object`
already exist and are public. Only `save_tree` is genuinely new; the three read
functions are thin wrappers.

### 2.2 Extract `velo-merge` 🔴
`diff3(base, ours, theirs) -> MergeResult` over `&[u8]`/`&str`. Already pure —
close to a file move. Bring the existing proptest properties with it.

**Pluggable merge drivers** registered by path glob or content type (so a
spreadsheet or document tool can supply structure-aware merging, falling back to
the line engine) — 🟡 recommended, but design the registration point now even if
only the default driver ships.

### 2.3 Newtypes 🔴
`SnapshotId`, `ObjectHash`, `BranchName`, `TagName` as newtypes over `String`.
Cheap now, miserable to retrofit once consumers pattern-match on `String`.
Implement the Phase 0 full-hash + typed-timestamp decisions here.

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
| `docs/FORMAT.md` + `CHANGELOG.md` | 🟡 **Recommended** | Format changes now break N applications. Start the changelog with the Phase 0 break. |
| Publish `velo-core` to crates.io | 🟢 Optional timing | See ⛔ below on placeholder releases. |

---

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
| **0** | Format decisions: metadata hashing, ID width, timestamps, schema versioning + `FORMAT.md` | 🔴 |
| **1** | Workspace split, typed errors, data-returning commands, repo handle + write guard, schema versioning, injectable transport, test relocation | 🔴 |
| **2** | In-memory trees, `velo-merge` extraction, newtypes | 🔴 |
| **3** | Feature flags & scoped ignores 🟡; progress, notifications, testkit, render 🟢 | 🟡/🟢 |
| **4** | Format spec, changelog, publishing | 🟡/🟢 |

**12 of the original 16 items confirmed as-written.** Four premises corrected
(no `anyhow`; coupling is 3 files not pervasive; merge engine already pure;
object primitives already exist). Three items added (injectable transport, test
relocation, typed timestamps). Two items re-tiered (schema versioning promoted
to Phase 1; full-hash storage recognised as a format break, not display).
