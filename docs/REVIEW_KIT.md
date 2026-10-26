# External review kit

Everything a reviewer needs to audit velody's handling of untrusted input, in one
place. Start with the [threat model](THREAT_MODEL.md); report findings as
described in [SECURITY.md](../SECURITY.md). Stability promises are in
[STABILITY.md](STABILITY.md).

Paths below are relative to the repository root.

## Untrusted surfaces

| Surface | Where bytes enter | File and function |
|---|---|---|
| Bundle decoding (v2, v3, v4) | `velody import` and every pull | `crates/velody-core/src/commands/bundle.rs`: `decode`, then `import` and `import_pack` |
| Wire strings and ref advertisements | ssh and HTTP transport, both directions | `crates/velody-core/src/transport.rs`: `read_string`, `read_string_opt`, `read_refs` |
| ssh server side | `velody serve` over stdio | `crates/velody-core/src/serve.rs`: `upload`, `receive` |
| HTTP server | `velody serve-http` | `crates/velody-core/src/serve.rs`, `mod http`: `Service::handle` (`refs`, `upload`, `receive`; bearer parsing in `parse_bearer`) |
| Git import | `git fast-export` stream | `crates/velody-core/src/commands/import.rs`: `parse_path`, `parse_ident`, `parse_trailer`, `parse_mark` |
| Path rule | every ingested or materialised path | `crates/velody-core/src/safe_path.rs`: `check`, `check_for_this_platform`, `check_tree`, `join_checked`, `check_symlink_target` |
| Stored trees | rows read back from the database, including delta tables | `crates/velody-core/src/tree_store.rs`: `tree_of`, `keyframe_rows`, `check_path_limits` |
| Signatures | snapshot signatures received with a bundle | `crates/velody-core/src/sign.rs`: `check_row`, `verify` |
| MCP server | stdio and HTTP | `crates/velody-mcp/src/lib.rs`: `handle_as`; `crates/velody-mcp/src/http.rs`: `serve`, `check_bind`, `origin_is_loopback` |

## Known gaps

Quoted from [THREAT_MODEL.md](THREAT_MODEL.md), "What velody does not defend
against":

- **A compromised host.** Anyone who can write the repository directory or run
  as the user can do anything the user can.
- **A trusted key that signs something wrong.** A signature proves who signed,
  not that the content is good.
- **Key management and revocation.** Distributing, rotating and revoking keys
  is left to the user.
- **Side channels** such as timing or size leaks.
- **Availability against a hostile peer on an unauthenticated listener**,
  beyond the configured limits.
- **TLS.** `serve-http` speaks plain HTTP; terminating TLS is the proxy's job.
- **Git-level threats in exported histories.**

And "Residual risks":

- The known crash windows, listed in [CRASH_SAFETY.md](CRASH_SAFETY.md).
- Later countersignatures are not re-sent: a countersignature added to an
  already-shared snapshot does not travel on a later sync.

## Fuzz targets

Details in [fuzz/README.md](../fuzz/README.md). It needs a nightly toolchain
and Linux or macOS (use WSL on Windows):

```sh
cargo install cargo-fuzz --locked
cargo +nightly fuzz run bundle_decode  fuzz/seeds/bundle_decode  -- -max_total_time=60 -rss_limit_mb=2048
cargo +nightly fuzz run wire           fuzz/seeds/wire           -- -max_total_time=60 -rss_limit_mb=2048
cargo +nightly fuzz run http_service   fuzz/seeds/http_service   -- -max_total_time=60 -rss_limit_mb=2048
cargo +nightly fuzz run git_import     fuzz/seeds/git_import     -- -max_total_time=60 -rss_limit_mb=2048
cargo +nightly fuzz run tree_paths     fuzz/seeds/tree_paths     -- -max_total_time=60 -rss_limit_mb=2048
cargo +nightly fuzz run snapshot_ids   fuzz/seeds/snapshot_ids   -- -max_total_time=60 -rss_limit_mb=2048
cargo +nightly fuzz run delta_table    fuzz/seeds/delta_table    -- -max_total_time=60 -rss_limit_mb=2048
```

Raise `-max_total_time` for a longer run. Crashes land in
`fuzz/artifacts/<target>/`; minimise one with
`cargo +nightly fuzz tmin <target> fuzz/artifacts/<target>/crash-<hash>`.

## Crash harness

Every operation is killed at every failpoint, then four checks run
([CRASH_SAFETY.md](CRASH_SAFETY.md)). It takes about a minute:

```sh
cargo test -p velody-core --features failpoints --test crash
```

## Golden corpus

Repositories and bundles written by tagged release builds, with expectations
read from the databases those builds wrote
([tests/corpus/README.md](../tests/corpus/README.md)). The bundle goldens in
`tests/corpus/bundles/` also seed the `bundle_decode` fuzz target.

```sh
cargo test -p velody-core --test corpus
```

## Reproducible build

CI builds the musl release binary twice from separate directories and compares
hashes ([.github/workflows/reproducible.yml](../.github/workflows/reproducible.yml)).
To repeat it locally on Linux (needs `musl-tools` and the musl Rust target),
run the same build in two checkouts at different paths and compare:

```sh
export CC=musl-gcc CARGO_INCREMENTAL=0
export SOURCE_DATE_EPOCH="$(git log -1 --format=%ct)"
export RUSTFLAGS="--remap-path-prefix=$PWD=/build --remap-path-prefix=$HOME/.cargo=/cargo"
cargo build -p velody-cli --release --locked --target x86_64-unknown-linux-musl
sha256sum target/x86_64-unknown-linux-musl/release/velody
```

`$PWD` must be the checkout root in each run; the two hashes must match.

## Automated passes

The static and dynamic analysis passes below run weekly and on every change to
`crates/velody-ffi/` in
[.github/workflows/static-analysis.yml](../.github/workflows/static-analysis.yml),
one job per pass. Alongside them, the dependency audit and `cargo deny` jobs
run in CI. Results are as of 2026-10-22 (Rust 1.94.0, cargo 1.94.0).

The claim that `unsafe` is confined to `velody-ffi` is nearly, but not exactly,
true. Outside `velody-ffi` there are three sites, each under a crate-level
`deny(unsafe_code)` with a narrow `allow`:

| Site | Why |
|---|---|
| `crates/velody-core/src/storage.rs` | read-only `memmap2` map of a pack file, justified in place |
| `crates/velody-cli/src/lockwait.rs` | `kill(pid, 0)` liveness probe for a lock holder (Unix) and its Windows counterpart |
| `crates/velody-cli/src/main.rs` | `SetConsoleOutputCP(65001)` on Windows so UTF-8 output renders |

`velody-mcp`, `velody-merge`, `velody-testkit` and `velody-tui` are
`#![forbid(unsafe_code)]`. The `unsafe-policy` CI job checks the crate-root
attributes and fails if a new file outside `velody-ffi` gains an unsafe block.

### Why Miri is not used

Miri interprets Rust MIR and cannot execute foreign code. `velody-ffi`'s tests
open real repositories, which calls into the bundled C SQLite through
`rusqlite`, so Miri stops at the first foreign call. AddressSanitizer and
`cargo careful` run the real code natively instead.

### Passes

| Pass | Tool and version | Command | Ran locally | Findings |
|---|---|---|---|---|
| Unsafe policy | grep over crate roots | the `unsafe-policy` job | yes (by hand, same patterns) | 0 |
| Clippy unsafe lints | clippy 1.94.0 | `cargo clippy -p velody-ffi -- -W clippy::undocumented_unsafe_blocks -W clippy::multiple_unsafe_ops_per_block -W clippy::ptr_as_ptr` | yes | 1, fixed |
| Unsafe inventory | cargo-geiger (latest at run time) | `cargo geiger --forbid-only` | no | pending first CI run |
| AddressSanitizer | nightly rustc | `RUSTFLAGS=-Zsanitizer=address cargo +nightly test -p velody-ffi --target x86_64-unknown-linux-gnu` | no | pending first CI run |
| cargo careful | cargo-careful (latest at run time) | `cargo +nightly careful test -p velody-ffi` | no | pending first CI run |
| Unused dependencies | cargo-machete (latest at run time) | `cargo machete` | no | pending first CI run |

The author's machine could not run the Linux-only and cargo-installed passes
(WSL was not reachable from the environment that produced this change, and
`cargo-geiger` is not installed there), so those four rows are recorded as
pending. Their CI jobs are advisory (`continue-on-error: true`) until a first
clean run is recorded here, at which point the `continue-on-error` line is
removed so they block. The unsafe-policy and clippy jobs are clean now and
block.

### Findings and dispositions

| Pass | Finding | Disposition |
|---|---|---|
| Clippy `ptr_as_ptr` | `crates/velody-ffi/src/tree.rs`: `Box::into_raw(bytes) as *mut u8` | fixed: now `.cast::<u8>()`, same behaviour |
| Clippy `undocumented_unsafe_blocks` | none | clean |
| Clippy `multiple_unsafe_ops_per_block` | none | clean |
| Unsafe policy | `velody-core`, `velody-cli` contain three unsafe sites despite the "confined to velody-ffi" description | documented above; not a defect, the sites are minimal and each has an `allow` |
| geiger, ASan, careful, machete | not yet run | open: record the first CI results here |
