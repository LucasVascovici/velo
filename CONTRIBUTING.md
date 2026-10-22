# Contributing to velo

Thank you for considering a contribution. This page says how to build velo,
which test suites exist and when to run them, and the conventions a change is
expected to follow. By taking part you agree to the
[Code of Conduct](CODE_OF_CONDUCT.md). Questions go through [SUPPORT.md](SUPPORT.md);
vulnerabilities go through [SECURITY.md](SECURITY.md), never a public issue.

## Build

Requires Rust 1.88 or later (the minimum supported version, checked in CI).

```bash
git clone https://github.com/LucasVascovici/velo.git
cd velo
cargo build --workspace      # debug build of every crate
cargo build --release        # binary at target/release/velo
```

## Test suites

The CI jobs in [`.github/workflows/ci.yml`](.github/workflows/ci.yml) are the
full gate; run the first three locally before opening a pull request.

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --locked
```

| Suite | Where | Run |
|---|---|---|
| Unit and property tests (including randomised 3-way-merge and object-store round-trips) | `crates/*/src`, `crates/velo-core/src/tests/` | `cargo test --workspace` |
| CLI integration tests, driving the real binary and asserting its output | `crates/velo-cli/tests/cli.rs` | `cargo test -p velo-cli` |
| Corpus: repositories written by older releases must still open and migrate | `crates/velo-core/tests/corpus.rs` | `cargo test -p velo-core --test corpus` |
| Crash harness (see below) | `crates/velo-core/tests/crash.rs` | `cargo test -p velo-core --features failpoints --test crash` |
| Python binding | `bindings/python/tests` | see [`bindings/python/README.md`](bindings/python/README.md) |
| Node binding | `bindings/node/__test__` | see [`bindings/node/README.md`](bindings/node/README.md) |
| Binding parity: every public embedder entry point is bound or exempted with a reason | `bindings/parity/check.py` | `python bindings/parity/check.py` |
| MCP conformance | `crates/velo-mcp/conformance/` | see its [README](crates/velo-mcp/conformance/README.md) |
| Fuzz targets for every reader of untrusted bytes | `fuzz/` | see [`fuzz/README.md`](fuzz/README.md) |

The Python and npm distributions are both named `velo-core`. The bindings are
separate Cargo workspaces, so `--workspace` does not reach them: format and lint
them with `--manifest-path bindings/<name>/Cargo.toml`.

CI also checks the minimum Rust version, `cargo package` for the whole
workspace, public API compatibility against the latest release
(`cargo-semver-checks`), and dependency advisories and licences
(`cargo audit`, `cargo deny`). The fuzz, benchmark, nightly, Python, Node, WASM
and MCP workflows live next to `ci.yml` in
[`.github/workflows/`](.github/workflows/).

### The differential harness

[`crates/velo-core/src/tests/differential.rs`](crates/velo-core/src/tests/differential.rs)
runs the same operations through two paths and asserts they agree, and replays
a checked-in corpus of repositories that is migrated on open. Add a case there
when a change introduces a second way of doing something that must stay
equivalent to the first.

### The crash harness

[`crates/velo-core/tests/crash.rs`](crates/velo-core/tests/crash.rs) injects
failures at named points inside a write (behind the `failpoints` feature) and
checks that the repository still opens, `velo fsck` is clean and no committed
snapshot is lost. A new write path needs a failpoint and a test here. The
guarantees are described in [`docs/CRASH_SAFETY.md`](docs/CRASH_SAFETY.md).

### The performance guard

A small guard runs on every pull request and fails a change that makes a save
about 1.6 times slower than its calibrated baseline. How it measures, and how
to update a baseline deliberately in the same pull request as the change that
moved it, is in [`bench/perf/README.md`](bench/perf/README.md). The test is
`crates/velo-core/src/tests/perf_guard.rs`; longer benchmarks run from
[`.github/workflows/bench.yml`](.github/workflows/bench.yml).

## The format-change rule

A changed golden output is a format change. The golden file
`crates/velo-core/src/tests/compaction_in_place.golden` pins the ids and rows
that in-place compaction produces; if a change makes it differ, then what is
stored on disk or in bundles changed. Do not just regenerate the file. Update
[`docs/FORMAT.md`](docs/FORMAT.md) and [`docs/STABILITY.md`](docs/STABILITY.md)
in the same commit, add a migration if existing repositories need one, and
explain the change in the changelog. The same applies to anything else that
changes bytes on disk, in a bundle or on the wire.

## Conventions

- `velo-core` is a library: it denies `clippy::print_stdout`,
  `clippy::print_stderr` and `clippy::exit`. Report through return values and
  typed `VeloError` variants, not strings or printing.
- Public enums and option structs are `#[non_exhaustive]`, and option structs
  have a `Default`, so adding a field or variant is not a breaking change.
- A new feature gets its own self-contained test module in
  `crates/velo-core/src/tests/`, declared at the end of `tests.rs`. Tests must
  fail without the change they cover.
- Comments explain why, in full sentences.
- Mind `#[cfg(unix)]` and `#[cfg(not(unix))]` pairs: keep both sides symmetric,
  and run the affected tests on Linux when you change one.
- One coherent change per commit. The subject is imperative, plain English and
  at most 72 characters ("Add clone stage laps"), with no prefixes; the body
  says why and records any non-obvious decision.
- A user-visible change adds an entry to [`CHANGELOG.md`](CHANGELOG.md).
- Open a pull request against `main` with the checks above green; the template
  lists what a reviewer will ask. Address review with new commits, then
  squash or tidy on request.
