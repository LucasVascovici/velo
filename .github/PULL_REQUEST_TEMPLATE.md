## What and why

<!-- What changes, and why. Link the issue if there is one. -->

## Checklist

- [ ] `cargo fmt --all`, `cargo clippy --workspace --all-targets --all-features -- -D warnings` and `cargo test --workspace --locked` pass
- [ ] New behaviour has a test that fails without the change
- [ ] A change to stored or bundled bytes (including a golden file) updates `docs/FORMAT.md` and `docs/STABILITY.md`
- [ ] A user-visible change has a `CHANGELOG.md` entry
- [ ] One coherent change per commit, with an imperative subject

See [CONTRIBUTING.md](../CONTRIBUTING.md).
