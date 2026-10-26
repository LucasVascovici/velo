# velody-core (Python)

PyO3 binding over `velody-core`'s embedder API. It is its own Cargo workspace so
the Python toolchain stays out of the main build.

```
python -m venv .venv
.venv/Scripts/python -m pip install maturin pytest   # bin/ on Unix
.venv/Scripts/maturin develop
.venv/Scripts/python -m pytest -q
```

```python
import velody
repo = velody.Repo.init("work")
sid = repo.save_tree(branch="main", message="hi", entries={"a.txt": "hello"})
repo.read_file_at(sid, "a.txt")
# Several files of one snapshot in one call, in the order asked:
repo.read_files_at(sid, ["a.txt"])      # [("a.txt", b"hello")]
# Write a snapshot into any directory; only/ignore are gitignore patterns:
repo.materialize(sid, "out", only=["src/**"], ignore=["*.log"])  # files written
```

Save only what changed, and get object ids back:

```python
saved = repo.save_changes(branch="main", message="edit", parent=sid,
                          changes={"a.txt": "hi", "old.txt": None})
saved.snapshot, saved.written   # id, {"a.txt": object id}
# or a list: velody.Change.put / stored / delete / rename
```

Notes:

- `Repo` is `unsendable`: using it from another thread raises `RuntimeError`.
  That is the binding enforcing velody's anti-goal on `Sync`.
- Calls hold the GIL, except `merge_plan` and `merge_commit`, which release it
  so a merge driver can run on any thread (see below).
- Every `velody_core::Error` variant is an exception subclass of `velody.VelodyError`
  named after the variant (`Io` is `VelodyIOError`, `Db` is `DatabaseError`), with
  the variant's fields as attributes (`NotFound.kind`, `Compacted.into`, ...).
- Ids are plain `str`; a malformed one raises `InvalidInput`.
- A repository with no working tree has no current position, so pass `from_`
  or `branch` to `history`.

History, blame and a store-only merge:

```python
a = repo.save_tree(branch="main", message="a", entries={"n.txt": "one\n"},
                   author=velody.Author("ada"), meta={"app": {"kind": "draft"}})
b = repo.save_tree(branch="side", message="b", parent=a, entries={"n.txt": "two\n"})
c = repo.save_tree(branch="main", message="c", parent=a, entries={"n.txt": "three\n"})

repo.history(from_=c, limit=5, meta=[("app", "kind", "draft")])
repo.blame("n.txt", at=c, lines=(1, 1)).lines[0].origin.author.name
plan = repo.merge_plan(c, b)            # plan.is_clean is False
repo.merge_commit(branch="main", ours=c, theirs=b, message="merge",
                  resolutions={"n.txt": b"two\nthree\n"})
repo.create_branch("release", at=c)
```

## API reference

The generated API reference for the bindings is `api-python.zip` and
`api-node.zip` on each release page, and the `api-python`/`api-node` artifacts
of the apidocs workflow.

## Change events and merge drivers

```python
repo.on_event(lambda e: print(e["kind"], e.get("snapshot")))
repo.set_merge_driver("*.json", "json")            # or "yaml", "toml"
repo.set_merge_driver("*.txt", lambda ancestor, ours, theirs: ours + theirs)
repo.clear_merge_drivers()
```

- `on_event(callback)` calls `callback(event)` for each change committed through
  this handle. `event` is a dict with `kind` (`"Saved"`, `"Merged"`,
  `"RefMoved"`, `"Imported"`) and that event's fields (`snapshot`, `branch`,
  `parent`, `merge_parent`, `ours`, `theirs`, ...). Events are **in-process
  only**: a handle hears its own writes, not those of another handle or
  process (poll `head_token` for those).
- Callbacks run **after** the call that made the change has returned, with the
  repository lock released, on the calling thread. They may use the repository.
  A callback that raises is reported through `sys.unraisablehook`; the write
  has already committed and does not fail.
- `set_merge_driver(pattern, driver)` picks a merge driver by path glob; the
  first matching rule wins. `driver` is `"json"`, `"yaml"`, `"toml"` or a
  function `(ancestor, ours, theirs) -> str | None`; `None` is a conflict.
- A driver function runs **inside** the merge, under the repository lock, and
  the GIL is released around `merge_plan` / `merge_commit` so it can run on any
  worker thread. It must **not** call back into the repository: any method
  called on the same handle while it is busy raises `velody.ReentrantCall`
  (a `VelodyError`) at once, rather than deadlocking.
- An exception raised by a driver makes `merge_plan` / `merge_commit` raise that
  same exception, and nothing is committed. With resolutions given, the merge
  is planned first so the exception surfaces before any write; the driver then
  runs twice.

## Threads

- A `Repo` belongs to the thread that created it. Using it from another thread
  raises `RuntimeError` ("velody.Repo is unsendable: it can only be used on the
  thread that created it"); the handle is never `Sync` and never shared.
- Use one `Repo` per thread: open another `Repo` on the same path in each
  thread.
- The GIL is released during the long calls (`save_tree`, `tree_at`, `history`,
  `find_snapshots`, `blame`, `merge_base`, `merge_plan`, `merge_commit`,
  `retention`, `compact`, `fsck`, `sign`, `verify`, `signed_snapshots`, `sync`),
  so threads that each have their own handle run in parallel, and other Python
  threads keep running during a long call. A write takes the repository lock
  without waiting: while another handle holds it, the call raises `velody.Locked`
  at once, so retry after a short delay.
