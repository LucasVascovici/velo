# velo-wasm

WebAssembly binding for [velo-core](../../crates/velo-core), for browser-based
local-first apps and for Node. It exposes the embedder API over **single-file
repositories**: the whole repository, objects included, is one SQLite database.

Everything is synchronous once the repository is open. The core is synchronous
and one tab (or worker) owns the database. Result shapes and field names mirror
the [Node binding](../node); errors are thrown as `Error` objects whose `code` is
the velo `Error` variant name (`'NotFound'`, `'Conflicts'`, ...), with the
variant's fields (for example `paths`) attached.

## Build

```bash
cargo install wasm-pack
wasm-pack build bindings/wasm --target web      # or --target nodejs / bundler
wasm-pack test --node bindings/wasm             # the test suite
```

`zstd-sys` and the bundled SQLite compile C for wasm, so `clang` must be on
`PATH` (see DEVELOPING.md).

## In memory (Node, tests, ephemeral use)

```js
import { Repo } from './pkg/velo_wasm.js';

const repo = Repo.createInMemory('notes.db');   // a distinct name per repository
const id = repo.saveTree({
  branch: 'main',
  message: 'first',
  entries: [
    { path: 'a.txt', data: 'hello\n' },
    { path: 'img/logo.bin', data: new Uint8Array([1, 2, 3]) },
  ],
});
repo.treeAt(id);                  // [{ path, object, kind }, ...]
repo.readFileAt(id, 'a.txt');     // Uint8Array
repo.history({ from: id });
repo.blame('a.txt', { at: id });
```

The in-memory repository lives as long as the module instance. `openInMemory(name)`
reopens one created earlier in the same instance.

## Persistent, in a browser worker

```js
// worker.js -- must be a *dedicated* Web Worker
import init, { Repo } from './pkg/velo_wasm.js';

await init();
const repo = await Repo.openPersistent('notes.db');   // creates or opens
```

`openPersistent` installs the OPFS `sahpool` VFS (from `sqlite-wasm-vfs`) the
first time it is called, then creates or opens `name` in it. OPFS's synchronous
access handles exist only in a dedicated worker, so anywhere else (the main
thread, a shared worker, Node) it rejects with code `'Unsupported'`. Exactly one
context can hold the pool at a time; a second tab's call fails until the first
releases it. After the VFS is installed it is the default, so do not mix
`createInMemory` with `openPersistent` in the same worker.

## API

`Repo`: `createInMemory`, `openInMemory`, `openPersistent` (async, static);
`saveTree`, `treeAt`, `readFileAt`, `snapshot`, `snapshotMeta`, `history`,
`blame`, `mergeBase`, `mergePlan`, `mergeCommit`, `branchTip`, `headToken`
(a `bigint`). The input and result types are in the generated `.d.ts`.

## Limits

- **Store-only.** There is no working tree: nothing reads or writes files, so
  commands that need one (`add`, `checkout`, `status`, ...) are not exposed, and
  velo-core returns `'Unsupported'` for them. Content goes in through `saveTree`
  and comes out through `readFileAt` and `treeAt`.
- **Single file.** One SQLite database holds everything; there are no `.velo`
  directory, no loose objects and no sidecar files.
- **No sync.** Remotes, push/pull, bundles and events are not available in the
  browser.
- Not published to npm.
