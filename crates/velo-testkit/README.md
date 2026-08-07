# velo-testkit

Test fixtures for [Velo](https://github.com/LucasVascovici/velo). A throwaway
repository in a temporary directory, so a downstream test suite doesn't have to
reinvent the setup.

```rust
use velo_testkit::TempRepo;

let repo = TempRepo::new();
repo.write("app.rs", "fn main() {}\n");
let id = repo.save("initial");
assert!(repo.is_clean());
```

The directory is removed when the `TempRepo` is dropped, and the SQLite
connection is closed before that happens — field order in the struct is what
guarantees it, which is the kind of detail worth not rediscovering per project.

Intended as a dev-dependency of anything built on `velo-core`.

MIT licensed.
