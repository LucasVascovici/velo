## Use it from your language

The engine is `velody-core`, a synchronous Rust library with no terminal output.
Every other surface below is a thin layer over it. **Nothing is published** to
crates.io, PyPI or npm yet: install from the files attached to each GitHub
release (replace `vX.Y.Z` with a release tag; `docs/INSTALL.md` has the full
list and how to verify the checksums):

```bash
# Python: the wheel for your platform; the import name is `velody`
pip install https://github.com/LucasVascovici/velody/releases/download/vX.Y.Z/<wheel file>
# Node: the tarball for your platform; the package name is `velody-core`
npm install https://github.com/LucasVascovici/velody/releases/download/vX.Y.Z/velody-core-<version>-<platform>.tgz
```

```toml
# Rust: Cargo.toml (the crate is `velody-core`; `cargo install velody-cli` is the command-line tool)
# Use the newest release tag: the releases page, or `git ls-remote --tags <repo URL>`.
velody-core = { git = "https://github.com/LucasVascovici/velody", tag = "vX.Y.Z" }
```

The installed Python package ships type stubs (`velody/__init__.pyi`) and the Node
package ships `index.d.ts`; each lists every method with its types. Below, the
parts people most often have to look up are written out.

### Rust

```rust
use std::path::Path;
use velody_core::commands::{blame, history};
use velody_core::tree::{SaveTree, TreeEntry};
use velody_core::{BranchName, Repo};

let repo = Repo::init(Path::new("work"))?;
let branch: BranchName = "main".parse()?;

// No working tree is read or written: the snapshot is built from memory.
let id = repo.write()?.save_tree(SaveTree {
    branch: &branch,
    parent: None,
    merge_parent: None,
    message: "first",
    entries: vec![TreeEntry::file("notes.md", b"hello\n".to_vec())],
    meta: Default::default(),
    timestamp_ms: None,
    author: None,
    renames: &[],
})?;

let files = repo.tree_at(&id)?;
let log = history::run(&repo, history::Options { from: Some(&id), ..Default::default() })?;
let who = blame::run(&repo, Path::new("notes.md"), blame::Options {
    at: Some(&id),
    ..Default::default()
})?;
```

`save_tree` takes the whole tree. To change a few files of a large tree, use
`save_changes`: the parent's tree is loaded inside the database and only the
changed entries cross the API.

```rust
use velody_core::tree::{Change, SaveChanges};

let mut save = SaveChanges::new(&branch, Some(&id), "edit notes");
save.changes = vec![
    Change::Put(TreeEntry::file("notes.md", b"hello again\n".to_vec())),
    Change::Delete("old.md".to_string()),
];
let next = repo.write()?.save_changes(save)?;
```

What the calls return and take (all in `velody_core`):

- **Imports.** `Repo`, `Author`, `BranchName`, `SnapshotId` and `TagName` are at the
  crate root; `SaveTree`, `SaveChanges`, `Change`, `TreeEntry` and `TreeFile` are in
  `velody_core::tree`; the operations (`history`, `blame`, `branches`, `tag`, `merge`)
  are in `velody_core::commands`.
- **`parent` is the snapshot you build on, and `None` starts a new history.** A
  `save_changes` or `save_tree` with `parent: None` makes a root snapshot holding only
  what you give it and moves the branch to it, even if the branch had commits. Pass the
  previous snapshot every time after the first. (The same holds in Python and Node.)
- `SaveChanges` has public fields `author: Option<&Author>`, `meta`, `merge_parent`,
  `timestamp_ms` and `changes`; rename edges are `renames: &[(PathBuf, PathBuf)]` on
  `SaveTree`.
- `save_tree` returns the new `SnapshotId`; `save_changes` returns `Saved`, so use
  `saved.snapshot` for the id and `saved.written` for the objects it stored.
- An author is `Author::new("ada")?`, which returns a `Result`; `author` takes
  `Option<&Author>`, and the name is read with `author.name()`.
- `history::run` returns a `History`; the snapshots are in `.entries`.
  `from: Some(&id)` lists the whole ancestry of `id`; with no scope it lists the
  checked-out branch, and a repository written only through the library has no
  checked-out position, so it lists the tip of the branch `HEAD` names.
- `blame::run` returns lines whose `origin` is an `Option<LineOrigin>`; its
  `author` is an `Option<Author>`.
- A new branch: save to a new branch name with a `parent`, or
  `branches::create(&guard, &name, Some(&id))`. Tags: `tag::create(&guard,
  &name, Some(&id), false)`.
- Rollback or undo is a new snapshot that carries the old objects forward:
  `TreeEntry::stored(file.path, file.object, file.kind)` for each `TreeFile` of
  `repo.tree_at(&old)?`. History stays append-only.
- Merge in two steps, with no working tree:

```rust
use velody_core::commands::merge::{self, MergeCommit, Resolution};

let plan = merge::plan(&repo, &ours, &theirs)?;          // writes nothing
let conflicts: Vec<_> = plan.conflicts().map(|f| f.path.clone()).collect();
let merged = merge::commit(&repo.write()?, MergeCommit {
    branch: &branch,
    ours: &ours,
    theirs: &theirs,
    // One resolution per conflicted path; `Content` writes exactly these bytes.
    resolutions: &[("notes.md".to_string(), Resolution::Content(b"both\n".to_vec()))],
    message: "merge",
    meta: Default::default(),
    author: None,
    timestamp_ms: None,
})?;
```

  A conflict with no resolution is `Error::Conflicts` listing the paths. The
  merged snapshot has `ours` as parent and `theirs` as merge parent.

See [`crates/velody-core`](../crates/velody-core) and the runnable
[`examples/prompt-registry`](../examples/prompt-registry).

### Python

[`bindings/python`](../bindings/python) is a PyO3 binding, built with maturin:

```bash
cd bindings/python
python -m venv .venv && .venv/bin/python -m pip install maturin pytest   # Scripts/ on Windows
.venv/bin/maturin develop
```

```python
import velody
repo = velody.Repo.init("work")
a = repo.save_tree(branch="main", message="a", entries={"n.txt": "one\n"},
                   author=velody.Author("ada"))
c = repo.save_tree(branch="main", message="c", parent=a, entries={"n.txt": "two\n"})
repo.history(from_=c, limit=5)
# Only the changed files; the rest of the tree carries forward from `parent`.
d = repo.save_changes(branch="main", message="d", parent=c,
                      changes=[velody.Change.put("n.txt", "three\n"),
                               velody.Change.delete("old.txt")]).snapshot
repo.blame("n.txt", at=c).lines[0].origin.author.name
```

What the calls return and take:

- **`parent` is required after the first save.** `save_tree` and `save_changes`
  with no `parent` start a new history holding only what you pass, and move the branch
  to it, even if the branch already had commits. Pass the previous snapshot id each
  time. `Repo.init(path)` takes just a path and creates a `.velo` folder in it.
- `save_tree` returns the snapshot id (`str`); `save_changes` returns `Saved`
  (`.snapshot`, `.written`). `tree_at` returns `TreeFile` objects (`.path`,
  `.object`, `.kind`); `history` returns `Entry` objects (`.id`, `.message`,
  `.branch`, `.parent`, `.merge_parent`, `.tag`, `.is_merge`); `read_file_at`
  returns `bytes`.
- **History scope.** `history(branch="b")` lists the snapshots *recorded on* `b`,
  which stops where `b` forked; `history(from_=id)` lists everything that led to
  `id`. Use `from_` for a full trail.
- **Who.** `Entry` has no author. A line's author is
  `blame(...).lines[i].origin.author`; a snapshot's is
  `snapshot_meta(id)["velo"]["author.name"]`. The author is part of the snapshot
  id, so it cannot be changed afterwards. Keep "which run" or "who approved" in
  your own metadata namespace (`meta={"app": {...}}`), as the cookbooks do.
- **Undo and rollback.** Save a new snapshot whose whole tree is the old one, carrying
  its objects forward: `save_tree(entries=[velody.TreeEntry.stored(f.path, f.object,
  f.kind) for f in repo.tree_at(old)], parent=current, ...)`. Using `save_tree`
  rather than `save_changes` drops files the bad snapshot added. History stays
  complete. A branch's tip is its newest snapshot, so
  `set_branch_tip(name, to)` moves only a branch that has no snapshots of its own;
  it does not take an existing branch back. `create_branch(name, at=id)` starts a
  new branch from an earlier snapshot.
- **Branches and tags.** `create_branch(name, at=id)`, `branches()`,
  `create_tag(name, at=id)`, `tags()`, `delete_tag(name)`.
- **Merging.** `merge_plan(ours, theirs)` writes nothing; `merge_commit(branch=...,
  ours=..., theirs=..., message=..., resolutions={path: b"text"}, author=...,
  meta={...})` records a two-parent snapshot. A resolution is `b"bytes"` to write,
  `"ours"`, `"theirs"`, or `None` to delete the path; a conflicted path with none
  raises `Conflicts` (its `.paths` lists them).

It also exposes more (events, merge drivers, signing, `repack`, `fsck`); see its README.

### Node.js

[`bindings/node`](../bindings/node) is a napi-rs binding (`velody-core`); every method
returns a `Promise`:

```bash
cd bindings/node && npm install && npm run build
```

```js
const { Repo } = require('velody-core')
const repo = await Repo.init('work')
const id = await repo.saveTree({
  branch: 'main',
  message: 'hi',
  entries: [{ path: 'a.txt', data: 'hello\n' }],
})
// saveChanges (camelCase) sends only the changed files, not the whole tree.
const next = await repo.saveChanges({
  branch: 'main',
  message: 'edit',
  parent: id,
  changes: [{ op: 'put', path: 'a.txt', data: 'hello again\n' }],
})
await repo.history({ from: id, limit: 5 })
await repo.blame('a.txt', { at: id })
```

Authors and metadata: `author: { name: 'ada' }` (and optional `email`) and
`meta: { app: { key: 'value' } }` go on `saveTree`, `saveChanges` and `mergeCommit`. As in
Python, `parent` is required after the first save: with none, the call starts a new
history that holds only what you pass and moves the branch to it.

What the calls return and take: `saveTree` resolves to the snapshot id (a
string); `saveChanges` to `{ snapshot, written }`; `treeAt` to
`{ path, object, kind }` objects; `history` to `SnapshotInfo` objects (`id`,
`message`, `branch`, `parent`, `mergeParent`, `tag`, `createdAt`). They carry no
author: read a line's with `blame(...)` (`lines[i].origin.author`) or a snapshot's
with `snapshotMeta(id)` (`velo['author.name']`), and keep "who approved" in your
own metadata namespace. `history({ branch })` lists only what was recorded on
that branch since it forked; `history({ from })` gives the whole ancestry.

Rollback is a new snapshot that carries the old objects forward, so no content is
read or copied: pass `{ path, object, kind }` from `treeAt(old)` as the entries of
`saveTree`. A branch's tip is its newest snapshot, so `setBranchTip` does not take an
existing branch back; `createBranch(name, at)` starts a new branch from an earlier
snapshot. Branches and tags are `createBranch`, `branches`, `createTag(name, at)`,
`tags` and `deleteTag`.

### C

[`crates/velody-ffi`](../crates/velody-ffi) is a C ABI (opaque handles, JSON results);
the declarations are in `crates/velody-ffi/include/velody.h`:

```bash
cargo build -p velody-ffi --release     # cdylib and staticlib in target/release/
```

```c
VelodyRepo *repo; VelodyTree *tree; char *id;
velody_repo_init("/tmp/r", &repo);
velody_tree_new(&tree);
velody_tree_add_file(tree, "a.txt", (const uint8_t *)"hi\n", 3, VELODY_KIND_REGULAR);
if (velody_save_tree(repo, tree, "{\"branch\":\"main\",\"message\":\"first\"}", &id) != 0)
    fprintf(stderr, "%s\n", velody_last_error_message());
velody_string_free(id);
velody_tree_free(tree);
velody_repo_free(repo);
```

`velody_history`, `velody_blame`, `velody_merge_plan` and `velody_branches` are declared in the same header.

### WebAssembly

[`bindings/wasm`](../bindings/wasm) (`velody-wasm`) exposes the embedder API over
**single-file repositories** — the whole repository, objects included, is one
SQLite database — for browsers and Node. Build it with
[wasm-pack](https://rustwasm.github.io/wasm-pack/):

```bash
wasm-pack build bindings/wasm --target web      # or --target nodejs / bundler
```

```js
import { Repo } from './pkg/velody_wasm.js';

const repo = Repo.createInMemory('notes.db');
const id = repo.saveTree({
  branch: 'main',
  message: 'first',
  entries: [{ path: 'a.txt', data: 'hello\n' }],
});
repo.history({ from: id });
repo.blame('a.txt', { at: id });
```

`Repo.openPersistent` stores the file in OPFS from a dedicated worker. See its
README for the details and limits (no sync in the browser). The save, history,
merge and `fsck` scenario, in memory and against a reopened OPFS repository,
is run in headless Chrome and Firefox by the CI `bindings` job; see the
binding README for what is and is not yet confirmed.

The release `.wasm` is about 2.4 MB raw, 0.86 MB with gzip and 0.66 MB with
brotli (measured 2026-10-14; method in the binding README, budget enforced in CI).

### MCP server

[`crates/velody-mcp`](../crates/velody-mcp) is a stdio Model Context Protocol server
that makes velody the checkpoint layer for an agent, with every write recorded
against a run id.

**Tools.** `velody_save`, `velody_restore`, `velody_status`, `velody_diff`,
`velody_history`, `velody_metadata`, `velody_blame`, `velody_branch` (`list`, `create`,
`switch`), `velody_merge_plan` then `velody_merge_apply` (a merge is always a plan
first, with a resolution for every conflict), `velody_undo`, `velody_redo`,
`velody_tag`, `velody_conflicts`, `velody_find` and `velody_repack`; and, for an agent with no
checkout (`--store`, below), `velody_save_tree`, `velody_tree_at`, `velody_read_file` and
`velody_read_files`. Ask the server for the exact arguments with `tools/list`. A result
is in `result.structuredContent` (and as JSON text in `result.content`). There is no tool
that forces anything or rewrites pushed history.

**Run identity.** Start the server with `--run <id>` (or set `VELODY_MCP_RUN`);
without one it generates `run-<epoch_ms>`. Every write records metadata in the
`mcp` namespace: `run`, `tool`, `client` and, when the call passes a
`tool_call_id`, `tool_call_id`. Find a run's snapshots with `velody_find` (or
`velody history --all --where mcp:run=<id>`), and `velody_blame` reports the run of each
line. The snapshot **author** is the `clientInfo.name` your client sends at
`initialize` (or `VELODY_AUTHOR_NAME` when set); it is not the run id.

**Speaking to it.** Messages are JSON-RPC 2.0, one per line, on stdin and stdout.
Send `initialize` (the server accepts protocol versions 2025-11-25, 2025-06-18,
2025-03-26 and 2024-11-05 and answers with one it supports), then the
`notifications/initialized` notification, then `tools/list` and `tools/call`.

**Behaviours worth knowing.** `velody_save` snapshots the whole working tree, so a script
you keep inside the project is saved too (keep it outside, or in `.veloignore`).
`velody_branch` `create` with no `at` makes an unborn branch; pass `at` to start it from a
snapshot. `velody_find` searches every branch, while `velody history --where ...` lists
only the current branch unless you add `--all`.

**Setup.** The server needs an existing repository: run `velody init` in the
project first (the CLI is in the `velody-<arch>-<os>` release archive), or point it
at a single-file store with `--store FILE --create`. A `velody_branch` `switch` to a
branch that does not exist yet answers `started_unborn`, with `inherits` naming the
snapshot the branch will start from at its first save.

Install the `velody-mcp` archive for your platform from the latest release
(assets `velody-mcp-<arch>-<os>.tar.gz` for Linux and macOS, and
`velody-mcp-x86_64-windows.zip` for Windows; the line below is the Linux one):

```bash
curl -L -o velody-mcp.tar.gz https://github.com/LucasVascovici/velody/releases/latest/download/velody-mcp-x86_64-linux.tar.gz && tar -xzf velody-mcp.tar.gz
```

Or build it from source:

```bash
cargo build -p velody-mcp --release     # target/release/velody-mcp
```

The full tool table, the concurrent-agents rules and the HTTP transport are in the
[velody-mcp README](../crates/velody-mcp/README.md).

Client configuration (the surrounding file format depends on the client):

```json
{"command": "velody-mcp", "args": ["--repo", "/path/to/project"]}
```

Which real clients were tried, and what broke, is in the
[tried clients table](../crates/velody-mcp/README.md#tried-clients).

### From the command line

The CLI reads the same repository. `velody report -o run.html` writes a
self-contained HTML audit report (fsck verdict, a timeline per branch, line
provenance; flags `--branch`, `--since`, `--json`, `--trust FILE`, `--no-fsck`).
`velody sync [remote] [branch]` fetches, then fast-forwards, pushes, or merges and
pushes in one step; a conflict stops with exit 3 and never pushes. See
[CLI.md](CLI.md).

### Further reading

Worked walkthroughs live in [`docs/cookbook`](cookbook):
[agent checkpointing](cookbook/agent-checkpointing.md),
[config registry](cookbook/config-registry.md) and
[document editor](cookbook/document-editor.md).

What is promised to adopters, and which CI job enforces each promise, is in
[`docs/STABILITY.md`](STABILITY.md).

The generated Python and Node API reference is `api-python.zip` and `api-node.zip`
on each release page, and the `api-python`/`api-node` artifacts of the apidocs
workflow. With only the installed package, read `velody/__init__.pyi` (Python) or
`index.d.ts` (Node).
