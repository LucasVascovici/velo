# Fuzz targets

`cargo-fuzz` targets for every reader of untrusted bytes in `velody-core`. They
call the `velody_core::fuzzing` wrappers (feature `fuzzing`, hidden from the docs),
which discard every `Err`: a refusal is the right answer to hostile input. Only a
panic, an abort, an out-of-memory or a timeout is a finding.

| Target | Reads |
|---|---|
| `bundle_decode` | bundle bytes (v2, v3, v4), then imports them into a scratch repository |
| `wire` | ref advertisements and length-prefixed strings |
| `http_service` | `method`, `path`, a credential flag and a body, against `serve::http::Service` |
| `git_import` | a `git fast-export` stream |
| `tree_paths` | tree paths, one per line, through the path rule |
| `snapshot_ids` | snapshot ids, object hashes, branch and tag names, public keys |
| `delta_table` | records overwriting the stored delta tables of a small repository, then every tree read |

Seeds live in `fuzz/seeds/<target>/`. The bundle seeds are the golden corpus in
`tests/corpus/bundles`.

## Run

Needs a nightly toolchain and Linux or macOS (cargo-fuzz is unreliable on
Windows MSVC; use WSL).

```sh
cargo install cargo-fuzz --locked
cargo +nightly fuzz run bundle_decode fuzz/seeds/bundle_decode -- -max_total_time=60 -rss_limit_mb=2048
```

New inputs the fuzzer finds go to `fuzz/corpus/<target>/`, crashes to
`fuzz/artifacts/<target>/`. CI (`.github/workflows/fuzz.yml`) runs every target
for 60 s on each push and pull request and for 600 s nightly, and uploads
`fuzz/artifacts/` when one fails.

The workflow can also be dispatched by hand with a chosen budget (the `seconds`
input); GitHub only offers that once the workflow file is on the default
branch.

## Minimise a crash

```sh
cargo +nightly fuzz tmin bundle_decode fuzz/artifacts/bundle_decode/crash-<hash>
```

## Add a regression test

Fix the bug in `velody-core`, then add a test to
`crates/velody-core/src/tests/fuzz_regressions.rs` that feeds the minimised input
(inline bytes, kept small) to the same underlying `pub(crate)` function the
wrapper calls, without the `fuzzing` feature, and asserts it returns instead of
panicking. Name it after the bug, not the hash.
