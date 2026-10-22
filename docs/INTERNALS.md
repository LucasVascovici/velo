# Internals

## What velo is

Velo is a timeline engine. A program (or a person) records whole-tree snapshots,
branches them, merges them and asks questions of the result.

- **Verifiable whole-tree snapshots** — content-addressed, `fsck` recomputes every id.
- **Tamper-evident provenance** — hashed, namespaced metadata; authorship; parent-aware, rename-aware blame with author and branch per line.
- **No working tree required** — `save_tree` / `tree_at`, caller-supplied timestamps, `Repo::scoped`.
- **Explicit divergence** — branches, a pure merge engine, resumable resolutions, undo/redo, sync that refuses rather than guesses.
- **Trivial to deploy** — one SQLite file, synchronous core, typed errors, per-call progress and cancellation.

Anything that has to answer *"what did this look like at T, who changed it, why,
and can I prove it?"* — while letting people or programs work on alternatives in
parallel — is a candidate.

To see who wrote each line of an agent run, open the [sample `velo report`](report-sample.html): a static page that works offline.

---

## Where it fits

| Use | What velo gives it | Fit |
| :--- | :--- | :--- |
| AI agent workspaces / checkpointing | Snapshot per tool call, branch per attempt, undo/redo, merge the winner; blame answers "which run wrote this line"; metadata (`eval_run`, `model`, `tool_call_id`) is trustworthy after the fact | Excellent — the wedge |
| Prompt, config and policy registries | Publish / tag / rollback, audit trail, reviewable diffs | Excellent — [`examples/prompt-registry`](../examples/prompt-registry) is the proof |
| Document and knowledge editors | Drafts as branches, per-line provenance, rename-aware history | Very good for text; structured formats via merge drivers |
| Regulated audit trails | Recomputable content addressing; Ed25519-signed snapshots (`velo sign`, `velo verify --trust`) whose signatures travel in bundles; `velo fsck` recomputes every id, so altered content or history is caught, and a signature from a key you do not trust fails verification | Good; no key storage, revocation or web of trust (see [Signing snapshots](INTEGRITY.md#signing-snapshots)) |
| Local-first / offline field apps | Bundles need no server; explicit merges; HTTP sync; WASM single-file repos | Medium |
| Headless CMS / low-code | Draft → preview branch → publish → revert; JSON/YAML/TOML drivers | Medium |
| Reproducible research, assessment | "What exactly was run / submitted", provably; chunked storage for large files | Medium |

**Not a fit:** real-time co-editing (CRDT territory), high-frequency event
sourcing, large binary assets (game art, CAD, datasets), and use as a database.

---

## New in v4

| | |
| :--- | :--- |
| **A new snapshot format** | Ids are stored whole rather than truncated to 16 characters, timestamps are integers so no format-string change can shift an id, and metadata is hashed in — which is what made tamper-evident authorship possible afterwards without a second break. **Repositories written by 3.x cannot be opened**; copy the working tree into a fresh repository and save it there. |
| **Merges remember what you resolved** | Merging the same branch twice used to re-raise every conflict settled the first time. Ancestry now follows both parents, so the base is the tip the previous merge absorbed. |
| **History shows merged-in work** | Same cause: a branch that had merged another listed none of the merged snapshots. |
| **Authorship** | `VELO_AUTHOR_NAME` / `VELO_AUTHOR_EMAIL` are recorded into the snapshot's identity, and `velo blame` shows who wrote each line. |
| **A library a program can hold** | `velo-core` produces no terminal output at all, takes options structs rather than positional flags, and reports progress and accepts cancellation per call. A consumer with no working tree — an editor, a registry, a server — is a first-class caller. |

The full list is in [CHANGELOG.md](../CHANGELOG.md); the design record, including
what was deliberately not done, is kept with the maintainers.

---

## Architecture

Velo is a Cargo workspace. Everything that matters lives in the engine; the
other crates are consumers of it.

| Crate / directory | Role |
| :--- | :--- |
| `crates/velo-core` | The engine: repository, snapshots, history, blame, sync. Synchronous, no terminal output |
| `crates/velo-merge` | The pure three-way merge engine and merge drivers |
| `crates/velo-tui` | The interactive conflict resolver used by the CLI |
| `crates/velo-cli` | The `velo` command, one consumer of the engine |
| `crates/velo-ffi` | A C ABI (`include/velo.h`) over the embeddable half of the engine |
| `crates/velo-mcp` | A stdio MCP server that makes velo a checkpoint layer for agents |
| `crates/velo-testkit` | Shared test fixtures |
| `bindings/python` | PyO3 binding, built with maturin |
| `bindings/node` | napi-rs binding (`velo-core`) |
| `bindings/wasm` | wasm-bindgen binding over single-file repositories |

The bindings are separate Cargo workspaces so their toolchains stay out of the
main build.

| Layer | Technology | Role |
| :--- | :--- | :--- |
| Hashing | BLAKE3 | Collision-proof, 10× faster than SHA-1; `rayon` parallelises large files |
| Compression | Zstd level 1 | Fast compression on save; transparent decompression on restore |
| Metadata | SQLite (WAL mode) | Snapshots, branches, tags, ancestry, conflicts, stash, remotes — indexed queries |
| Mtime cache | `index_cache` table | `(path, mtime_ns, size, hash)` — skips rehashing unchanged files |
| Concurrency | Rayon | Parallel filesystem walk, hash-and-compress, and file writes on restore |
| I/O | memmap2 | Memory-maps files ≥256 KB to avoid kernel→userspace copy |
| Locking | fs2 advisory lock | Serialises mutating commands across processes (`.velo/lock`) |
| Transport | Filesystem · SSH · bundle | Direct path access, a pack protocol over `ssh`, or a self-contained file |

**Content-addressed snapshots.** A snapshot's ID is `BLAKE3(tree ‖ parents ‖ message ‖ timestamp)`, truncated to 16 hex characters (64 bits), where *tree* is every `(path, object-hash, mode)` triple in sorted order. Because the ID commits to the tree, it can be verified against its contents — the property `fsck` checks and that makes accepting history from another machine safe. The branch is deliberately **not** hashed, so renaming or deleting a branch never changes the identity of its commits.

**File model.** Each tree entry records a mode: regular, executable, or symlink. The executable bit survives save→restore (on Unix), and symlinks are stored as their target and recreated as real links rather than being followed and flattened into copies. On Windows the executable bit isn't observable from the filesystem, so it is carried through history unchanged, and symlink creation falls back to a regular file when the platform disallows it.

**Delta storage.** Each snapshot records only changed files. Unchanged files are stored as references to the same object from the parent. A 1000-file project where 10 files change creates 10 new objects, not 1000.

**Object store.** Content-addressed storage under `.velo/objects/`. Each object is Zstd-compressed file content named by its BLAKE3 hash. Identical content across branches and snapshots is stored exactly once.

**Merge parents.** Merge commits record both their primary parent and their merge-source parent (`merge_parent` column in the `snapshots` table). This is what enables the two-parent topology in `velo history --graph`.

**Schema migrations.** The database schema is versioned via `pragma_table_info` checks. New columns are added automatically on first use — existing repositories are upgraded in place with no manual intervention.

---

## Repository layout

The source tree:

```
crates/
├── velo-core/      # the engine
├── velo-merge/     # three-way merge engine and drivers
├── velo-tui/       # interactive conflict resolver
├── velo-cli/       # the `velo` binary
├── velo-ffi/       # C ABI, include/velo.h
├── velo-mcp/       # MCP server
└── velo-testkit/   # shared test fixtures
bindings/
├── python/         # PyO3 + maturin
├── node/           # napi-rs
└── wasm/           # wasm-bindgen, single-file repositories
examples/prompt-registry/   # a registry with no working tree
docs/                       # FORMAT.md and the cookbook
```

A repository on disk:

```
.velo/
├── velo.db       # SQLite: snapshots, file trees, branches, tags, stash,
│                 #         conflicts, remotes, remote-tracking refs
├── objects/      # Content-addressed object store (Zstd-compressed, named by BLAKE3 hash)
├── HEAD          # Current branch name
├── PARENT        # Hash of the current snapshot
├── lock          # Advisory lock held by mutating commands
├── MERGE_HEAD    # Present only during an in-progress merge or cherry-pick
│                 # Format: "<pre-merge-hash>:<source-branch>"
└── REBASE_STATE  # Present only during an in-progress rebase
                  # (alongside REBASE_ONTO and REBASE_ORIG_HEAD)
```

Remote-tracking history is stored on internal branches named
`remotes/<remote>/<branch>`, with each remote's last-known tips in the
`remote_refs` table. Your local branches are never touched by `fetch`.
