# Production checklist

Eleven statements that would have to hold before velo drops the
"not production-grade" note from the [README](../README.md). Each is judged
here as **green**, with a link to its proof, or **red**, with the reason. This
page records the evidence; it does not make the decision. Removing or keeping
the note is the owner's call.

Current count: **7 green, 4 red** (lines 7, 9, 10 and 11).

| # | Line | Verdict |
| --- | --- | --- |
| 1 | Every released format opens, and a newer one is refused | green |
| 2 | No known path from untrusted input to an escape, a panic or an unbounded allocation | green |
| 3 | An interrupted operation leaves a repository `fsck` accepts, on v4 as on v3 | green |
| 4 | A hosted remote cannot be written without authorisation | green |
| 5 | A signed history round-trips and tampering is caught | green |
| 6 | The documented sizes are measured sizes, on the current format | green |
| 7 | The headline workload is within its stated factor, and a regression fails CI | **red** |
| 8 | The API and format promises are enforced by CI | green |
| 9 | A stranger can follow the README to a working result | **red** |
| 10 | A simulation converges with no open bug above green | **red** |
| 11 | The bindings and the MCP server work from an install, not a checkout | **red** |

## 1. Released formats open, newer ones are refused: green

- The golden corpus holds a repository and a bundle from each released format;
  [`crates/velo-core/tests/corpus.rs`](../crates/velo-core/tests/corpus.rs)
  opens, migrates or refuses each as [FORMAT.md](FORMAT.md) says, and
  `a_repository_written_by_a_newer_version_is_refused_and_left_alone` covers the
  refusal. Fixtures and how they are generated:
  [`tests/corpus/README.md`](../tests/corpus/README.md).
- Mixed-version checks: [`tests/compat/README.md`](../tests/compat/README.md).
- CI runs the `corpus` and `old-release` jobs in
  [`ci.yml`](../.github/workflows/ci.yml).

## 2. Untrusted input: green, with limits on what that means

- One path rule and read limits at ingestion: [FORMAT.md](FORMAT.md) sections
  3.1 and 9. Tests:
  [`hostile_paths.rs`](../crates/velo-core/src/tests/hostile_paths.rs),
  [`hostile_deltas.rs`](../crates/velo-core/src/tests/hostile_deltas.rs),
  [`fuzz_regressions.rs`](../crates/velo-core/src/tests/fuzz_regressions.rs).
- Seven fuzz targets in [`fuzz/fuzz_targets`](../fuzz/fuzz_targets), run nightly
  by [`fuzz.yml`](../.github/workflows/fuzz.yml).
- [THREAT_MODEL.md](THREAT_MODEL.md) lists what is and is not defended, and
  [REVIEW_KIT.md](REVIEW_KIT.md) is the pack for an outside reviewer.
- "No known path" is a statement about what is known. Fuzzing and tests are not
  a proof, and no independent review has been done.

## 3. Interrupted operations: green

- The crash harness kills every operation at every step and runs `fsck` on what
  is left: [`crates/velo-core/tests/crash.rs`](../crates/velo-core/tests/crash.rs),
  run as `cargo test -p velo-core --features failpoints --test crash` by the
  `crash` job in [`ci.yml`](../.github/workflows/ci.yml).
- Every scenario runs on v3 and on v4 storage, and the known windows are
  listed: [CRASH_SAFETY.md](CRASH_SAFETY.md).

## 4. Hosted remote authorisation: green

- Push is refused until a token is configured; an anonymous fetch is allowed by
  default and can be turned off. Tests:
  [`serve_http_auth.rs`](../crates/velo-cli/tests/serve_http_auth.rs) and
  [`http_auth_client.rs`](../crates/velo-core/src/tests/http_auth_client.rs),
  with the server in
  [`http_service.rs`](../crates/velo-core/src/tests/http_service.rs).
- `serve-http` speaks plain HTTP; TLS is the proxy's job
  ([THREAT_MODEL.md](THREAT_MODEL.md)).

## 5. Signed history: green

- Round trip, tampering and trust:
  [`signatures.rs`](../crates/velo-core/src/tests/signatures.rs),
  [`signed_bundles.rs`](../crates/velo-core/src/tests/signed_bundles.rs),
  [`signed_protection.rs`](../crates/velo-core/src/tests/signed_protection.rs)
  and [`signing.rs`](../crates/velo-cli/tests/signing.rs).
- Known gap: a later countersignature is not re-sent on a later sync
  ([THREAT_MODEL.md](THREAT_MODEL.md)).

## 6. Documented sizes are measured: green

- [SCALE.md](SCALE.md), "Measured on 6.x (format v4)", holds the measured
  envelope on the current format; the 5.x tables are kept and labelled as v3.
  The runs are in [`bench/results`](../bench/results) and
  [`bench/scale`](../bench/scale).

## 7. Headline workload against its stated factor: red

The verdicts below are as [PERF.md](PERF.md) records them, misses included.

| Target | Verdict | Where |
| --- | --- | --- |
| One-file save at 100,000 files, under 1 s (22.3) | met: median 246 ms, slowest 496 ms, Windows | [PERF.md](PERF.md), "After the parent-membership rule (22.3)" |
| Who wrote this line, within 1.5x shadow git (22.4) | met: 0.87x | [PERF.md](PERF.md) section 11 |
| Disk after 500 steps, at most 3x git (22.5) | met at rest (2.10x); 3.37x with the handle held open, as the benchmark does, and section 6 keeps that figure | [PERF.md](PERF.md) section 12 |
| 500 steps, at most 3x shadow git on Linux | **not measured** since storage v4; the last measurement was 4.5x, a miss | [PERF.md](PERF.md) sections 5 and 6 |
| One-file save at 100,000 files, at most 1.5x the history-0 save | section 6 recorded a miss (about 2.5x); the 22.3 result is under 1 s but is not restated as that ratio | [PERF.md](PERF.md) section 6 |

Red because the Linux ratio of the headline agent workload has no measurement on
the current format, and because the CI guard does not cover everything above:
the `perf` job in [`ci.yml`](../.github/workflows/ci.yml) fails a regression on
Linux, but its Windows leg is advisory (`continue-on-error`) until its baseline
is calibrated on the runner. The benchmark workflow
[`bench.yml`](../.github/workflows/bench.yml) is manual.

## 8. API and format promises enforced by CI: green

- [STABILITY.md](STABILITY.md) has a table of each promise and the check that
  enforces it: the `semver` job (cargo-semver-checks on `velo-core` and
  `velo-merge`), the `package` job, the `parity` job, and the golden corpus and
  `old-release` jobs, all in [`ci.yml`](../.github/workflows/ci.yml). A golden
  output example:
  [`compaction_in_place.golden`](../crates/velo-core/src/tests/compaction_in_place.golden).
- One recorded exception, `Bundle.signatures`, is described in
  [STABILITY.md](STABILITY.md) under "Not stable".

## 9. A stranger follows the README: red

Not yet run. The protocol is in
[`bench/coldstart/README.md`](../bench/coldstart/README.md), and the friction log
[`friction-log.md`](../bench/coldstart/friction-log.md) is still the empty
template. The five-minute quickstarts are executed in CI
([`quickstarts.yml`](../.github/workflows/quickstarts.yml)), which shows the
steps work, not that a person or an agent can find and follow them.

## 10. A simulation converges: red

Not yet run, and no findings record is tracked in the repository. Turning this
green needs a run whose findings are committed, with nothing open above green.

## 11. Bindings and the MCP server from an install: red

- The Python and Node packages are built, installed into a clean environment
  with no Rust toolchain, and exercised:
  [`packages.yml`](../.github/workflows/packages.yml) (`wheel-install`,
  `node-install`). That half is green.
- The MCP server is not: [`mcp.yml`](../.github/workflows/mcp.yml) and
  [`quickstarts.yml`](../.github/workflows/quickstarts.yml) run a `velo-mcp`
  built from the checkout with `cargo build`. A release build is produced by
  [`release.yml`](../.github/workflows/release.yml), but nothing installs it and
  runs a client against it.
