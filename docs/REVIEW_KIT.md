# External review kit

Everything a reviewer needs to audit velo's handling of untrusted input, in one
place. Start with the [threat model](THREAT_MODEL.md); report findings as
described in [SECURITY.md](../SECURITY.md). Stability promises are in
[STABILITY.md](STABILITY.md).

Paths below are relative to the repository root.

## Untrusted surfaces

| Surface | Where bytes enter | File and function |
|---|---|---|
| Bundle decoding (v2, v3, v4) | `velo import` and every pull | `crates/velo-core/src/commands/bundle.rs`: `decode`, then `import` and `import_pack` |
| Wire strings and ref advertisements | ssh and HTTP transport, both directions | `crates/velo-core/src/transport.rs`: `read_string`, `read_string_opt`, `read_refs` |
| ssh server side | `velo serve` over stdio | `crates/velo-core/src/serve.rs`: `upload`, `receive` |
| HTTP server | `velo serve-http` | `crates/velo-core/src/serve.rs`, `mod http`: `Service::handle` (`refs`, `upload`, `receive`; bearer parsing in `parse_bearer`) |
| Git import | `git fast-export` stream | `crates/velo-core/src/commands/import.rs`: `parse_path`, `parse_ident`, `parse_trailer`, `parse_mark` |
| Path rule | every ingested or materialised path | `crates/velo-core/src/safe_path.rs`: `check`, `check_for_this_platform`, `check_tree`, `join_checked`, `check_symlink_target` |
| Stored trees | rows read back from the database, including delta tables | `crates/velo-core/src/tree_store.rs`: `tree_of`, `keyframe_rows`, `check_path_limits` |
| Signatures | snapshot signatures received with a bundle | `crates/velo-core/src/sign.rs`: `check_row`, `verify` |
| MCP server | stdio and HTTP | `crates/velo-mcp/src/lib.rs`: `handle_as`; `crates/velo-mcp/src/http.rs`: `serve`, `check_bind`, `origin_is_loopback` |

## Known gaps

Quoted from [THREAT_MODEL.md](THREAT_MODEL.md), "What velo does not defend
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
cargo test -p velo-core --features failpoints --test crash
```

## Golden corpus

Repositories and bundles written by tagged release builds, with expectations
read from the databases those builds wrote
([tests/corpus/README.md](../tests/corpus/README.md)). The bundle goldens in
`tests/corpus/bundles/` also seed the `bundle_decode` fuzz target.

```sh
cargo test -p velo-core --test corpus
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
cargo build -p velo-cli --release --locked --target x86_64-unknown-linux-musl
sha256sum target/x86_64-unknown-linux-musl/release/velo
```

`$PWD` must be the checkout root in each run; the two hashes must match.
