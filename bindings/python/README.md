# velo (Python)

PyO3 binding over `velo-core`'s embedder API. It is its own Cargo workspace so
the Python toolchain stays out of the main build.

```
python -m venv .venv
.venv/Scripts/python -m pip install maturin pytest   # bin/ on Unix
.venv/Scripts/maturin develop
.venv/Scripts/python -m pytest -q
```

```python
import velo
repo = velo.Repo.init("work")
sid = repo.save_tree(branch="main", message="hi", entries={"a.txt": "hello"})
repo.read_file_at(sid, "a.txt")
```

Notes:

- `Repo` is `unsendable`: using it from another thread raises `RuntimeError`.
  That is the binding enforcing velo's anti-goal on `Sync`.
- Calls hold the GIL; `allow_threads` cannot borrow an unsendable `Repo`.
- Every `velo_core::Error` variant is an exception subclass of `velo.VeloError`
  named after the variant (`Io` is `VeloIOError`, `Db` is `DatabaseError`), with
  the variant's fields as attributes (`NotFound.kind`, `Compacted.into`, ...).
- Ids are plain `str`; a malformed one raises `InvalidInput`.
- A repository with no working tree has no current position, so pass `from_`
  or `branch` to `history`.

History, blame and a store-only merge:

```python
a = repo.save_tree(branch="main", message="a", entries={"n.txt": "one\n"},
                   author=velo.Author("ada"), meta={"app": {"kind": "draft"}})
b = repo.save_tree(branch="side", message="b", parent=a, entries={"n.txt": "two\n"})
c = repo.save_tree(branch="main", message="c", parent=a, entries={"n.txt": "three\n"})

repo.history(from_=c, limit=5, meta=[("app", "kind", "draft")])
repo.blame("n.txt", at=c, lines=(1, 1)).lines[0].origin.author.name
plan = repo.merge_plan(c, b)            # plan.is_clean is False
repo.merge_commit(branch="main", ours=c, theirs=b, message="merge",
                  resolutions={"n.txt": b"two\nthree\n"})
repo.create_branch("release", at=c)
```
