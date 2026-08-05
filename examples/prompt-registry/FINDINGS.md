# What embedding `velo-core` was actually like

`prompt-registry` is a real consumer: a versioned prompt store with no working
tree, written against nothing but `velo-core`'s public API. Its purpose was to
test a claim the test suite cannot — that the crate is pleasant to embed.

The headline: **it works, and the shape is right.** A registry with publish,
history, tagging, retrieval by tag or hash prefix and durable reopening is about
200 lines. `save_tree` / `tree_at` / `read_object` / `snapshot_meta` are the right
four primitives, snapshot metadata removed the need to encode state into commit
messages, and the typed errors made `create`-or-`open` a clean match rather than
a string comparison.

What follows is the friction, in the order a consumer meets it. **All four were
fixed**; each entry keeps the original complaint, because the reason a fix exists
is worth more than the fix.

---

## 1. `resolve_snapshot_id` reported "not found" as `InvalidInput` — fixed

The first thing the registry does is ask for its branch tip, which does not exist
yet on an empty registry. That is an expected, recoverable condition, so the code
matched on `Error::NotFound`:

```rust
match commands::resolve_snapshot_id(&self.repo, BRANCH) {
    Ok(id) => Ok(Some(id)),
    Err(velo_core::Error::NotFound { .. }) => Ok(None),
    Err(e) => Err(e.into()),
}
```

It never matched. Every unresolvable spec came back as
`InvalidInput { detail: "No snapshot, tag, or branch found matching 'registry'." }`,
so the only way to tell "no such ref" from "you asked something malformed" was to
match on the message — exactly what the typed errors of 1.2 exist to prevent.
`RefKind::Any` already existed, documented as "a ref given by a user that could be
several of the above", and was unused.

**Fixed**: `resolve_snapshot_id` now returns `NotFound { kind: RefKind::Any, name }`.

## 2. `history::Entry.hash` is a `String`

Every other result struct carrying a snapshot id was typed in 2.3 —
`SaveResult.hash`, `SnapshotDetail.hash`, `SearchedSnapshot.hash`,
`branches::Tip.hash`, `cherry_pick::Outcome.snapshot`. `history::Entry` was
missed, and it is the one consumers read most. So walking history to look
something up means:

```rust
let id: SnapshotId = entry.hash.parse()?;      // and this can fail, in theory
let meta = self.repo.snapshot_meta(&id)?;
```

The `parse()` cannot actually fail for a row Velo wrote, but the signature says it
can, so the consumer carries an error path that is dead. `Entry.branch`,
`Entry.parent`, `Entry.merge_parent` and `Entry.tag` are likewise `String` where
`BranchName`, `SnapshotId` and `TagName` exist.

**Fixed**: `Entry.hash` is a `SnapshotId`, `Entry.branch` a `BranchName`,
`parent` and `merge_parent` are `Option<SnapshotId>`, `tag` is `Option<TagName>`,
`BranchRef.name` is a `BranchName`, `History.current` is `Option<SnapshotId>`, and
`refs` is keyed by `SnapshotId`. The registry's lookup lost its reparse:

```rust
let meta = self.repo.snapshot_meta(&entry.hash)?;   // was entry.hash.parse()?
```

## 3. `history::run` takes five positional arguments

```rust
commands::history::run(&self.repo, false, usize::MAX, Some("registry"), None)?
```

Nothing at that call site says what `false` or `usize::MAX` mean. This is the same
problem `SnapshotIdentity` was introduced to solve for `snapshot_id`, and the same
fix applies: an options struct with `Default`.

Worse, the limit has a trap. It reaches SQL as `LIMIT ?`, so:

- `limit: 0` returns **no rows at all** — and 0 is the natural way to ask for
  "everything".
- `limit: usize::MAX` does mean unlimited, but only because `usize::MAX as i64`
  wraps to `-1`, which is SQLite's "no limit". It is right by accident.

**Fixed**: `run(repo, Options { .. })`, with `Default`. `limit` is now
`Option<usize>` — `None` means all of them and is the default, `Some(n)` means the
newest n. The unlimited case reaches SQL as an explicit `-1` rather than through a
wrapping cast. The call site says what it does:

```rust
commands::history::run(&self.repo, commands::history::Options {
    branch: Some(&self.branch),
    ..Default::default()
})?
```

The test that pinned the old trap was written to fail loudly if it was ever
fixed. It did, and now asserts the new contract instead.

## 4. A snapshot is a whole tree, so publishing one file rewrites all of them

`SaveTree.entries` is the complete contents of the snapshot. To publish one prompt
without dropping the others, a consumer must read every other file back out and
hand it in again:

```rust
for file in self.repo.tree_at(&tip)? {
    let content = self.repo.read_object(&file.object)?;   // decompress
    carried.insert(file.path, content);                   // hold in memory
}
carried.insert(path, body.as_bytes().to_vec());
```

Every publish therefore decompresses, holds in memory, re-hashes and re-compresses
the entire registry. For a store meant to hold many small documents this is the
one thing that will not scale, and it is the most likely reason a real consumer
would give up.

The fix is not to make `save_tree` take a diff — a whole tree is the right model,
and it is what makes the id verifiable. It is to let an entry *reference content
already in the store* instead of carrying its bytes:

```rust
// what exists
TreeEntry::file(path, content: Vec<u8>)
// what is missing
TreeEntry::stored(path, object: ObjectHash)
```

`TreeFile` already hands back an `ObjectHash`, so the carry-forward loop becomes a
map with no I/O and no rehashing. This looks like the single highest-value addition
for embedders.

**Fixed**: `TreeEntry.content` is now a `Content` enum — `Bytes(Vec<u8>)` or
`Stored(ObjectHash)` — and `TreeEntry::stored(path, object, kind)` builds the
second. The registry's publish became:

```rust
let mut entries: BTreeMap<String, TreeEntry> = self
    .repo
    .tree_at(tip)?
    .into_iter()
    .map(|f| (f.path.clone(), TreeEntry::stored(f.path, f.object, f.kind)))
    .collect();
entries.insert(path.clone(), TreeEntry::file(path, body.as_bytes().to_vec()));
```

No decompression, no rehashing, and memory proportional to the one document being
published rather than to the whole registry.

Two properties are tested, because both could silently break a consumer:
carrying a tree forward by reference produces a tree *identical* to carrying it
forward by value (modes, executable bit and symlinks included) — otherwise a
consumer would fork its own history the moment it took the cheap path; and a
`Stored` entry naming an object the store does not hold is refused with
`MissingObject`, so the API cannot manufacture corruption that only `fsck` would
find later.

## 5. Smaller things

- **`SaveTree.branch` is owned but taken per call.** A consumer that publishes to
  one branch forever clones the same `BranchName` on every save. Harmless, but
  `save_tree` could take `&BranchName` now that the ergonomic argument for owning
  it (inline `"x".parse()?`) is served just as well by a borrow of a stored field.
- **No way to ask "does this branch exist?"** other than resolving its tip and
  interpreting the error. `branches::list` returns everything, which is fine but
  heavier than the question.
- **`tag::create` takes `Option<&str>` for the snapshot** while everything else
  now takes `&SnapshotId`. The registry has an id in hand and has to hand over
  `Some(version.as_str())`, going back through resolution that already happened.

## What was genuinely good

- `Repo::init` returning `Error::AlreadyInitialized` made create-or-open three
  lines with no `exists()` race.
- `SnapshotMeta` is the reason `versions("alpha")` can filter on real data instead
  of parsing commit messages. Hashing it means a version's provenance cannot drift
  from the version.
- `fsck` recomputing every id is a genuine end-to-end check for a consumer: the
  registry's test suite asserts it, which proves the metadata is hashed the same
  way a bundle receiver would recompute it.
- The `WriteGuard` made the "when am I holding the lock" question disappear.
- Not one panic, and no need to reach for anything outside the public API.
