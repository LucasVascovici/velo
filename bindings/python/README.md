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
- History, blame, merge and branches are not yet exposed.
