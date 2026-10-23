## Use it from your language

The engine is `velo-core`, a synchronous Rust library with no terminal output.
Every other surface below is a thin layer over it. **Everything is built from
source; nothing is published** to crates.io, PyPI or npm yet.

### Rust

```rust
use std::path::Path;
use velo_core::commands::{blame, history};
use velo_core::tree::{SaveTree, TreeEntry};
use velo_core::{BranchName, Repo};

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
use velo_core::tree::{Change, SaveChanges};

let mut save = SaveChanges::new(&branch, Some(&id), "edit notes");
save.changes = vec![
    Change::Put(TreeEntry::file("notes.md", b"hello again\n".to_vec())),
    Change::Delete("old.md".to_string()),
];
let next = repo.write()?.save_changes(save)?;
```

See [`crates/velo-core`](../crates/velo-core) and the runnable
[`examples/prompt-registry`](../examples/prompt-registry).

### Python

[`bindings/python`](../bindings/python) is a PyO3 binding, built with maturin:

```bash
cd bindings/python
python -m venv .venv && .venv/bin/python -m pip install maturin pytest   # Scripts/ on Windows
.venv/bin/maturin develop
```

```python
import velo
repo = velo.Repo.init("work")
a = repo.save_tree(branch="main", message="a", entries={"n.txt": "one\n"},
                   author=velo.Author("ada"))
c = repo.save_tree(branch="main", message="c", parent=a, entries={"n.txt": "two\n"})
repo.history(from_=c, limit=5)
# Only the changed files; the rest of the tree carries forward from `parent`.
d = repo.save_changes(branch="main", message="d", parent=c,
                      changes=[velo.Change.put("n.txt", "three\n"),
                               velo.Change.delete("old.txt")]).snapshot
repo.blame("n.txt", at=c).lines[0].origin.author.name
```

It also exposes `merge_plan`, `merge_commit`, `create_branch` and more; see its README.

### Node.js

[`bindings/node`](../bindings/node) is a napi-rs binding (`velo-core`); every method
returns a `Promise`:

```bash
cd bindings/node && npm install && npm run build
```

```js
const { Repo } = require('velo-core')
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

### C

[`crates/velo-ffi`](../crates/velo-ffi) is a C ABI (opaque handles, JSON results);
the declarations are in `crates/velo-ffi/include/velo.h`:

```bash
cargo build -p velo-ffi --release     # cdylib and staticlib in target/release/
```

```c
VeloRepo *repo; VeloTree *tree; char *id;
velo_repo_init("/tmp/r", &repo);
velo_tree_new(&tree);
velo_tree_add_file(tree, "a.txt", (const uint8_t *)"hi\n", 3, VELO_KIND_REGULAR);
if (velo_save_tree(repo, tree, "{\"branch\":\"main\",\"message\":\"first\"}", &id) != 0)
    fprintf(stderr, "%s\n", velo_last_error_message());
velo_string_free(id);
velo_tree_free(tree);
velo_repo_free(repo);
```

`velo_history`, `velo_blame`, `velo_merge_plan` and `velo_branches` are declared in the same header.

### WebAssembly

[`bindings/wasm`](../bindings/wasm) (`velo-wasm`) exposes the embedder API over
**single-file repositories** — the whole repository, objects included, is one
SQLite database — for browsers and Node. Build it with
[wasm-pack](https://rustwasm.github.io/wasm-pack/):

```bash
wasm-pack build bindings/wasm --target web      # or --target nodejs / bundler
```

```js
import { Repo } from './pkg/velo_wasm.js';

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

[`crates/velo-mcp`](../crates/velo-mcp) is a stdio Model Context Protocol server
that makes velo the checkpoint layer for an agent: `velo_save`, `velo_restore`,
`velo_history`, `velo_branch`, `velo_merge_plan`, `velo_merge_apply`,
`velo_blame` and more, with every write recorded against a run id.

Install the `velo-mcp` archive for your platform from the latest release
(assets `velo-mcp-<arch>-<os>.tar.gz`, and `velo-mcp-x86_64-windows.zip`):

```bash
curl -L -o velo-mcp.tar.gz https://github.com/LucasVascovici/velo/releases/latest/download/velo-mcp-x86_64-linux.tar.gz && tar -xzf velo-mcp.tar.gz
```

Or build it from source:

```bash
cargo build -p velo-mcp --release     # target/release/velo-mcp
```

Run `velo init` in the project first. The tools are listed in the
[velo-mcp README](../crates/velo-mcp/README.md#tools).

Client configuration (the surrounding file format depends on the client):

```json
{"command": "velo-mcp", "args": ["--repo", "/path/to/project"]}
```

Which real clients were tried, and what broke, is in the
[tried clients table](../crates/velo-mcp/README.md#tried-clients).

### From the command line

The CLI reads the same repository. `velo report -o run.html` writes a
self-contained HTML audit report (fsck verdict, a timeline per branch, line
provenance; flags `--branch`, `--since`, `--json`, `--trust FILE`, `--no-fsck`).
`velo sync [remote] [branch]` fetches, then fast-forwards, pushes, or merges and
pushes in one step; a conflict stops with exit 3 and never pushes. See
[CLI.md](CLI.md).

### Further reading

Worked walkthroughs live in [`docs/cookbook`](cookbook):
[agent checkpointing](cookbook/agent-checkpointing.md),
[config registry](cookbook/config-registry.md) and
[document editor](cookbook/document-editor.md).

What is promised to adopters, and which CI job enforces each promise, is in
[`docs/STABILITY.md`](STABILITY.md).

The Python and Node API reference is `api-python.zip` and `api-node.zip` on each
release page, and the `api-python`/`api-node` artifacts of the apidocs workflow.
