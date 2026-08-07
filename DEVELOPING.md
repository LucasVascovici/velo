# Developing Velo

A short, practical guide to building, testing, and releasing Velo.

## Prerequisites

- **Rust 1.88+** (stable). Install via [rustup](https://rustup.rs).
  1.88 is the published `rust-version`, derived from the highest MSRV in the
  dependency tree; CI builds on exactly that toolchain so it stays true.
- No system dependencies for local dev — SQLite is bundled (compiled from source
  by `rusqlite`), so a working C compiler is all `cargo` needs.

---

## Build

```bash
cargo build            # debug build   → target/debug/velo
cargo build --release  # optimised     → target/release/velo   (velo.exe on Windows)
```

Run the freshly built binary directly:

```bash
./target/release/velo --version   # should match the version in Cargo.toml
```

> The `--version` string is wired to `CARGO_PKG_VERSION`, so it always matches
> `Cargo.toml` automatically — never hard-code it.

---

## Test

There are **two** kinds of tests. Run both before pushing.

### 1. Unit / integration tests (fast, in-process)

```bash
cargo test            # ~155 tests, all in src/tests.rs
cargo test --release  # same, optimised (what CI-adjacent runs use)
```

CI runs `cargo test --locked` on Linux, Windows, and macOS. This job **blocks**
merges — keep it green.

### 2. Workflow simulation (end-to-end, black-box)

`workflow_sim.sh` drives the real binary through a long, two-developer project
and asserts every outcome. It's the best way to catch cross-command regressions
that unit tests miss.

```bash
./workflow_sim.sh
```

It builds the release binary automatically if one isn't present, runs everything
in a throwaway temp sandbox, and exits non-zero on the first failed check.

> ⚠️ **WSL / Linux note.** The script uses the binary that matches the shell it
> runs in — the native `velo` on WSL/Linux/macOS, `velo.exe` only under a Windows
> shell (Git Bash/MSYS). **Do not run the Windows `velo.exe` from WSL:** SQLite's
> file locking breaks across the Windows↔WSL filesystem boundary and every
> command fails with `database is locked`. If you built on Windows and then open
> WSL, just run `cargo build --release` inside WSL once (it drops a native `velo`
> next to the `.exe` in `target/release/`) and re-run the script.

> The workflow sim is **not** part of CI — it's a local smoke test. Run it
> yourself before a release.

### Lint & format

```bash
cargo clippy --all-targets --all-features -- -D warnings   # BLOCKING in CI
cargo fmt --all                                            # auto-format
cargo fmt --all -- --check                                 # advisory in CI
```

These are **two separate CI jobs**, on purpose:

- **Clippy blocks**, and runs on Linux, Windows, *and* macOS. It has caught real
  defects here, so a new warning fails the build. Keep it at zero.
- **Format is advisory** (`continue-on-error`). Style alone should never stop a
  legitimate fix from landing; run `cargo fmt --all` to clear it.

> ⚠️ **Clippy on one OS only checks half the code.** This project has
> `#[cfg(unix)]` / `#[cfg(not(unix))]` branches (file modes, symlinks), so a
> clean run on Windows says nothing about the Unix branch — an `unused_mut` once
> slipped through exactly that way, because the mutation lived in the non-Unix
> block. That's why the clippy job is a 3-OS matrix. To check the Unix side
> locally from Windows, use WSL with a separate target dir so it doesn't fight
> your Windows build:
>
> ```bash
> wsl
> cd /mnt/c/Users/lvi/Documents/velo
> CARGO_TARGET_DIR=/tmp/velo-target cargo clippy --all-targets --all-features -- -D warnings
> ```

They used to share one job, which meant a formatting nit turned the whole thing
red — and a permanently-red job hides clippy's signal entirely. Splitting them
keeps the useful failure visible.

---

## Push & pull-request flow

1. Work on a branch (not `main`).
2. Before pushing, locally run:
   ```bash
   cargo test && cargo clippy --all-targets --all-features -- -D warnings && ./workflow_sim.sh
   ```
3. Push and open a PR against `main`. CI runs the test matrix (Linux/Windows/macOS)
   and lint on every push and PR.
4. Merge once the **Test** job is green.

---

## Cutting a release

Releases are **tag-driven**. Pushing a tag `vX.Y.Z` triggers `.github/workflows/release.yml`,
which verifies the version, builds binaries for all five platforms, runs the
tests, and publishes a GitHub Release with the packaged artifacts.

The release workflow **fails unless `Cargo.toml`'s version exactly matches the tag**
(minus the leading `v`). Because everything uses `--locked`, `Cargo.lock` must
also be in sync. So:

```bash
# 1. Bump the version in Cargo.toml, e.g. version = "2.5.0"
#    (main.rs picks it up automatically via CARGO_PKG_VERSION)

# 2. Refresh Cargo.lock so --locked builds succeed
cargo build

# 3. Commit the bump (both files) and push to main
git add Cargo.toml Cargo.lock
git commit -m "Release v2.5.0"
git push origin main            # let CI go green first

# 4. Tag it — the tag MUST match Cargo.toml — and push the tag
git tag v2.5.0
git push origin v2.5.0          # → triggers the release build + publish
```

### Fixing a botched tag

```bash
git tag -d v2.5.0                    # delete locally
git push --delete origin v2.5.0     # delete on the remote (re-run the release after fixing)
```

This only works while nothing depends on the tag. It does **not** work for a
version that has been published to crates.io — see below.

---

## Publishing to crates.io

Five crates are published: `velo-merge`, `velo-core`, `velo-tui`,
`velo-testkit`, `velo-cli`. (`prompt-registry` carries `publish = false` — it is
a worked example, not a product.)

**A version on crates.io is permanent.** It cannot be replaced, re-uploaded, or
deleted; `cargo yank` only stops *new* dependants from resolving it, and the
files stay downloadable forever. So unlike a git tag, a publish is not something
to redo. Get the version right first.

CI already runs everything the publish will run — the `package` job is
`cargo publish` without the upload — so a green build is the go-ahead.

```bash
# Dry run: resolves, packages and verifies every crate without uploading.
cargo publish --workspace --locked --dry-run
```

```bash
# The real thing. Cargo works out the dependency order and waits for each crate
# to appear on the index before publishing the next.
cargo publish --workspace --locked
```

Needs an API token — `cargo login` once, from
[crates.io/settings/tokens](https://crates.io/settings/tokens). Publishing is
irreversible, so it is a deliberate manual step rather than something the
release workflow does off a tag.

Afterwards, `cargo install velo-cli` installs the binary, and an embedder can
depend on `velo-core = "4"` instead of pinning a git revision.

### Bumping after a publish

`Cargo.toml`'s `[workspace.package] version` and the `[workspace.dependencies]`
entries for the five crates are separate strings and must move together — a
mismatch is only caught at publish time, which is the worst place to find it.

---

## Repository layout

```
src/
├── main.rs          # CLI definition (clap) + command dispatch
├── db.rs            # SQLite schema, migrations, connection setup
├── storage.rs       # object store: BLAKE3 hashing, Zstd, CRLF normalisation
├── error.rs         # error types
├── tests.rs         # unit/integration test suite
└── commands/        # one module per subcommand (save, merge, resolve, rebase, …)
.github/workflows/
├── ci.yml           # test matrix (blocking) + lint (advisory) on push/PR
└── release.yml      # tag-driven cross-platform build + GitHub Release
workflow_sim.sh      # end-to-end, multi-developer black-box simulation
```

---

## Gotchas

- **`target/` is shared between Windows and WSL builds.** They coexist (`velo.exe`
  vs `velo`) but switching environments may trigger a rebuild. Harmless.
- **Schema migrations are additive and idempotent** (`src/db.rs`) — they run on
  every connection open, so old repos upgrade in place. When adding a column,
  add both the `CREATE TABLE` default *and* an `ALTER TABLE` migration guard.
- **`--locked` everywhere.** If you change dependencies, commit the updated
  `Cargo.lock`, or CI's `--locked` builds will fail.
