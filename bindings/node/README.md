# velo (Node.js)

napi-rs binding over `velo-core`'s embedder API. It is its own Cargo workspace
so the Node toolchain stays out of the main build.

```
npm install
npm run build
npm test
```

```js
const { Repo } = require('@velo/core')
const repo = await Repo.init('work')
const id = await repo.saveTree({
  branch: 'main',
  message: 'hi',
  entries: [{ path: 'a.txt', data: 'hello' }],
})
await repo.readFileAt(id, 'a.txt') // <Buffer 68 65 6c 6c 6f>
```

Notes:

- Every method returns a `Promise`. The call runs on the libuv thread pool, so
  velo-core stays synchronous and the event loop is never blocked.
- A `Repo` wraps `Arc<Mutex<velo_core::Repo>>`: concurrent calls on one `Repo`
  serialise. That is the binding honouring velo's anti-goal on a `Sync` `Repo`.
- Every rejection is an `Error` whose `code` is the `velo_core::Error` variant
  name (`NotARepo`, `NotFound`, `Compacted`, ...; `Unknown` for a variant added
  later), with the variant's fields as camelCase properties (`err.kind`,
  `err.name`, `err.paths`, `err.id`, `err.into`, ...).
- An `entries` item takes exactly one of `data` (`Buffer` or `string`) or
  `object` (the hash of an object already in the store).
- Ids are plain strings; a malformed one rejects with `InvalidInput`.
- `npm test` uses a glob because newer Node versions reject a bare directory.
- `history({ from, branch, all, paths, limit, meta })` lists snapshots newest
  first; `meta` items are `{ namespace, key, value? }` and omit `value` to mean
  "has this key". With no working tree there is no position to walk back from,
  so pass `from` or `branch`. `findSnapshots(meta)` matches every filter.
- `blame(path, { at, startLine, endLine })` takes 1-based inclusive lines and
  returns per-line `origin` (`id`, `createdAtMs`, `createdAt`, `message`,
  `author`, `branch`, `path`), or `null` when history does not explain a line.
- `mergeBase`, `mergePlan` and `mergeCommit` never touch a working tree.
  `mergeCommit` resolutions map a path to `'ours'`, `'theirs'`, `null` (delete)
  or content (`Buffer`, or a string other than those two words). An unresolved
  conflict rejects with `code: 'Conflicts'` and `err.paths`.
- `branches()`, `createBranch(name, at?)` and `setBranchTip(name, to)` manage
  refs. Timestamps come as `createdAtMs` plus a `createdAt` `Date`.
- Working-tree commands are not yet exposed.

## API reference

The generated API reference for the bindings is `api-python.zip` and
`api-node.zip` on each release page, and the `api-python`/`api-node` artifacts
of the apidocs workflow.

## Change events and merge drivers

```js
repo.onEvent((e) => console.log(e.kind, e.snapshot))
await repo.setMergeDriver('*.json', 'json')           // or 'yaml', 'toml'
await repo.setMergeDriver('*.txt', (ancestor, ours, theirs) => ours + theirs)
await repo.clearMergeDrivers()
```

- `onEvent(fn)` calls `fn(event)` for each change committed through this handle.
  `event` is an object with `kind` (`'Saved'`, `'Merged'`, `'RefMoved'`,
  `'Imported'`) and that event's fields (`snapshot`, `branch`, `parent`,
  `mergeParent`, `ours`, `theirs`, ...). Events are **in-process only**: a handle
  hears its own writes, not those of another handle or process (poll
  `headToken` for those).
- Callbacks run when the promise of the call that made the change resolves:
  after the core call has finished and released the repository, on the JS
  thread. They may use the repository. A callback that throws is rethrown as an
  uncaught exception on the next tick; the write has already committed.
- `setMergeDriver(pattern, driver)` picks a merge driver by path glob; the first
  matching rule wins. `driver` is `'json'`, `'yaml'`, `'toml'` or a function
  `(ancestor, ours, theirs) => string | null`; `null` is a conflict.
- A driver function runs **inside** the merge: the core call runs on a libuv
  worker under the repository lock, and the worker blocks while the JS thread
  runs the function. The function must be synchronous (return the string or
  `null`, not a promise) and must **not** call the repository: any method called
  on the same handle while a driver function is running rejects at once with
  `code: 'Reentrant'` instead of waiting. Calls made at other times are not
  affected. The driver does not keep the process alive.
- A driver that throws, or returns something other than a string or `null`,
  makes `mergePlan` / `mergeCommit` reject with `code: 'DriverFailed'` and the
  driver's message, and nothing is committed. With resolutions given, the merge
  is planned first so the failure surfaces before any write; the driver then
  runs twice.

## Threads

- Every call runs on the libuv pool, so the event loop is never blocked, and
  calls on one handle are serialised: they run one at a time, in the order made.
- A `Repo` belongs to the thread that opened it and cannot be transferred to a
  `worker_thread`. Native objects are not cloneable: `postMessage` either throws
  `DataCloneError` or delivers an empty plain object with none of the methods,
  depending on the Node version. Either way the worker gets no usable handle.
- Open one `Repo` per worker, on the same path. Reads run in parallel. A write
  takes the repository lock without waiting: while another handle (in a worker
  or another process) holds it, the call rejects at once with `code: 'Locked'`,
  so retry after a short delay.
