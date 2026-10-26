# velody-wasm

WebAssembly binding for [velody-core](../../crates/velody-core), for browser-based
local-first apps and for Node. It exposes the embedder API over **single-file
repositories**: the whole repository, objects included, is one SQLite database.

Everything is synchronous once the repository is open. The core is synchronous
and one tab (or worker) owns the database. Result shapes and field names mirror
the [Node binding](../node); errors are thrown as `Error` objects whose `code` is
the velody `Error` variant name (`'NotFound'`, `'Conflicts'`, ...), with the
variant's fields (for example `paths`) attached.

## Build

```bash
cargo install wasm-pack
wasm-pack build bindings/wasm --target web      # or --target nodejs / bundler
wasm-pack test --node bindings/wasm -- --test node   # in-memory suite under Node
wasm-pack test --headless --chrome bindings/wasm -- --test browser --test browser_memory
wasm-pack test --headless --firefox bindings/wasm -- --test browser --test browser_memory
```

The browser tests run the cookbook scenario (three snapshots, a branch,
history, a conflicting merge plan and resolved merge, `fsck`) inside a
dedicated worker: once in memory, and once against a persistent OPFS
repository that is dropped and reopened in the same worker to check the history
survived. The CI `bindings` job runs them in headless Chrome and headless
Firefox; they have not been run locally, so the result of that job is the
evidence, and a browser that fails there (for example Firefox if the OPFS
`sahpool` VFS is unsupported) is to be removed from the job and documented
here. `--test`
keeps the worker-configured files out of the Node run.

Page-reload persistence is tested end to end in `e2e/` with Playwright: a page
starts a module dedicated worker that opens `reload-test.db` with
`openPersistent`, saves a snapshot, the page is reloaded, and a fresh worker
reads the same snapshot id and file text back.

```bash
wasm-pack build bindings/wasm --target web
cd bindings/wasm/e2e && npm ci && npx playwright install --with-deps chromium
npx playwright test --project chromium    # also: --project firefox | webkit
```

| Browser  | Reload test                                              |
| -------- | -------------------------------------------------------- |
| Chromium | run in the CI `reload` job (the result of that job is the evidence) |
| Firefox  | project configured, not run in CI, not verified          |
| WebKit   | project configured, not run in CI, not verified          |

Only Chromium is claimed. Firefox and WebKit are listed in
`e2e/playwright.config.mjs` so they can be tried with `--project`, but no result
is recorded for them here.

`zstd-sys` and the bundled SQLite compile C for wasm, so `clang` must be on
`PATH` (see DEVELOPING.md).

## In memory (Node, tests, ephemeral use)

```js
import { Repo } from './pkg/velody_wasm.js';

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
import init, { Repo } from './pkg/velody_wasm.js';

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

### No IndexedDB fallback

Persistence requires OPFS in a dedicated worker; there is no IndexedDB
fallback, and `openPersistent` takes no backend option. `sqlite-wasm-vfs` does
ship a `relaxed_idb` VFS, but it is not usable for a velody repository: it
supports only `PRAGMA synchronous=off` while velody-core sets `WAL` and
`synchronous=NORMAL`, it writes to IndexedDB asynchronously after SQLite's sync
with no per-write completion the binding could await (so a reload right after a
save can lose it), and its own documentation warns of corruption when more than
one page opens the same database. Making it safe would need changes to velody-core
and the single-file seam, which is out of scope. Where OPFS is unavailable,
`openPersistent` rejects with `'Unsupported'`; use `createInMemory` and export
through your own means.

## Size

Measured 2026-10-14 on the release package, built with
`wasm-pack build bindings/wasm --release --target web`:

| File | Raw | `gzip -9` | `brotli -q 11` |
| :--- | ---: | ---: | ---: |
| `pkg/velody_wasm_bg.wasm` | 2,414,425 | 863,812 | 663,391 |
| `pkg/velody_wasm.js` (glue) | 48,339 | | |

That is about 0.85 MB over the wire with gzip and 0.65 MB with brotli. The
engine is SQLite plus zstd plus velody-core; the largest single items are the
SQLite VDBE, zstd's compressors and the `fsck` inspector.

What was done about it, and what was measured:

- Before the release profile settings (opt-level 3, no LTO, names kept):
  3,748,254 raw, 1,267,189 gzip, 889,811 brotli.
- `[profile.release]` in `Cargo.toml` now sets `opt-level = "z"`, `lto = true`,
  `codegen-units = 1`, `panic = "abort"` and `strip = true`. `"s"` was tried
  and came out larger under gzip (1,014,689 against 1,004,646 at that point),
  so `"z"` stays. Stripping the 472 KB function-name section was the largest
  single win. The tests use the dev profile and are not affected.
- Merge drivers are not in the build: velody-core is used with its default
  features, which are none.
- Git import and export are **not** gated out. With LTO nothing reachable from
  the binding calls them, and `twiggy` shows no `commands::import` or
  `commands::export` code in the binary, so a target cfg would save 0 bytes.
- wasm-pack's bundled `wasm-opt` (binaryen 117) aborts on this module, so it is
  disabled in `Cargo.toml`. A newer binaryen (npm `binaryen`, `wasm-opt -Oz
  --all-features`) took the pre-LTO build from 1,267,189 to 1,147,159 gzip,
  about 9.5%. It is not wired into the build yet.

The numbers were taken with `zig cc` standing in for clang as the C compiler
(the C parts are SQLite and zstd), so CI may differ by a little. CI prints the
three sizes on every run and fails if the gzip size passes the budget set in
`.github/workflows/wasm.yml`; the comment there says how to raise it.

## API

`Repo`: `createInMemory`, `openInMemory`, `openPersistent` (async, static);
`saveTree`, `treeAt`, `readFileAt`, `snapshot`, `snapshotMeta`, `history`,
`blame`, `mergeBase`, `mergePlan`, `mergeCommit`, `fsck`, `branchTip`,
`headToken` (a `bigint`). `fsck()` verifies the repository read-only and returns
`{ok, problems: [{kind, detail}], cruft: [{kind, detail}]}`; `ok` is false only
for integrity problems. The input and result types are in the generated `.d.ts`.

## Limits

- **Store-only.** There is no working tree: nothing reads or writes files, so
  commands that need one (`add`, `checkout`, `status`, ...) are not exposed, and
  velody-core returns `'Unsupported'` for them. Content goes in through `saveTree`
  and comes out through `readFileAt` and `treeAt`.
- **Single file.** One SQLite database holds everything; there are no `.velo`
  directory, no loose objects and no sidecar files.
- **No sync.** Remotes, push/pull, bundles and events are not available in the
  browser.
- Not published to npm.
