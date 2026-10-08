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
- Working-tree commands and events are not yet exposed.
