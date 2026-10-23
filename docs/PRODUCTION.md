# Production checklist

Eleven statements that would have to hold before velo drops the
"not production-grade" note from the [README](../README.md). Each is judged
here as **green**, with a link to its proof, or **red**, with the reason. This
page records the evidence; it does not make the decision. Removing or keeping
the note is the owner's call.

Current count: **6 green, 5 red** (lines 6, 7, 9, 10 and 11). Judged again at
the end of phase 23 against the rule below; the count did not move, though the
evidence for lines 6, 7 and 11 did (each section says how far). The README note
stays. The inputs to the decision are in "Decisions for the owner" and "Still
red" at the end of this page.

## How a line turns green

A green line links to a committed result that matches its sentence, at the size,
platform and format the sentence names. A line whose evidence is a reduced run,
a different platform or a prediction is red or marked partial.

Each line that was green was re-read against that rule:

- Line 1: checked, holds. The corpus holds a repository and a bundle from each
  released format, and the `corpus` job runs on Linux, Windows and macOS; the
  `old-release` job covers mixed versions.
- Line 2: checked, holds, with the limits the section states. The seven fuzz
  targets run nightly. The sentence is about what is known, not a proof, and the
  section says so.
- Line 3: checked, holds. The crash harness runs on v3 and v4 storage, on Linux
  and Windows in CI. macOS is not in the `crash` job.
- Line 4: checked, holds. The tests are committed and run in the normal test
  jobs; TLS is out of scope by design.
- Line 5: checked, holds, with one known gap (a later countersignature is not
  re-sent on a later sync).
- Line 6: **did not hold**, now red. Its evidence was a document that disclaims
  the work; see section 6.
- Line 8: checked, holds. The `semver` job compares against the latest released
  tag (`v6.2.0`, `baseline-rev` in `ci.yml`), and the `package`, `parity`, corpus and `old-release` jobs
  exist as listed. The one recorded exception is stated.
- Lines 6, 7, 9, 10 and 11 were judged again at the end of phase 23: all five
  stay red, each with what is now measured and what is still missing in its
  section. Partial evidence is not green.

| # | Line | Verdict |
| --- | --- | --- |
| 1 | Every released format opens, and a newer one is refused | green |
| 2 | No known path from untrusted input to an escape, a panic or an unbounded allocation | green |
| 3 | An interrupted operation leaves a repository `fsck` accepts, on v4 as on v3 | green |
| 4 | A hosted remote cannot be written without authorisation | green |
| 5 | A signed history round-trips and tampering is caught | green |
| 6 | The documented sizes are measured sizes, on the current format | **red (partial)** |
| 7 | The headline workload is within its stated factor, and a regression fails CI | **red (partial)** |
| 8 | The API and format promises are enforced by CI | green |
| 9 | A stranger can follow the README to a working result | **red** |
| 10 | A simulation converges with no open bug above green | **red** |
| 11 | The bindings and the MCP server work from an install, not a checkout | **red (partial)** |

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

## 6. Documented sizes are measured: red (partial)

- [SCALE.md](SCALE.md), "Measured on 6.x (format v4)", is the current envelope.
  Every table there names its platform, velo version and whether the shape is
  full or reduced, and the 5.x tables are kept and labelled as v3. Raw runs:
  [`bench/results`](../bench/results), harness
  [`bench/scale`](../bench/scale).
- Now measured on 6.x, full size, on a Linux GitHub runner (6.2.0): 10,000
  files x 2,000 snapshots (`envelope-a`) and 100,000 files x 200 snapshots
  (`envelope-b`), saves flat in both
  ([`envelope-a`](../bench/results/2026-10-linux-runner-envelope-v4-a.json),
  [`envelope-b`](../bench/results/2026-10-linux-runner-envelope-v4-b.json)).
  Now measured on Windows, full size (6.2.1): 100,000 files x 500 snapshots,
  the ratio shape
  ([result](../bench/results/2026-10-windows-ratio-100k-500.json)).
- Still missing, by SCALE.md's own list "Not run on 6.x":
  - Windows, `envelope-a` (10,000 x 2,000) and `envelope-b` (100,000 x 200):
    not run. Windows has only reduced shapes (10,000 x 200, 30,000 x 20 which
    is partial, 50,000 x 40) plus the one full ratio shape. SCALE.md claims
    both platforms, so the page is not fully measured.
  - Linux, the 100,000 x 500 ratio shape: not run.
  - The retention run on 6.x: not run; its 416 s / 165 s / 193 s figures are
    from before format v4.
  - Linux is a hosted runner only, on two different CPUs; no local Linux.
- It turns green when the four missing runs above exist, each committed as a
  complete run (the detached commands are in SCALE.md under "Commands for the
  owner"). The sentence does not change.

## 7. Headline workload against its stated factor: red (partial)

The verdicts below are as [PERF.md](PERF.md) records them, misses included.

| Target | Verdict | Where |
| --- | --- | --- |
| One-file save at 100,000 files, under 1 s (22.3) | partial: median 246 ms, slowest 496 ms, through the library `save_tree` on Windows. Through the CLI at 100,000 files the save is 1.8 to 1.9 s with Defender on, so the sub-second figure is the library's, not the CLI's | [PERF.md](PERF.md), "After the parent-membership rule (22.3)" and section 6, "One-file save at 100,000 files, 500 snapshots (CLI, Windows)" |
| Who wrote this line, within 1.5x shadow git (22.4) | met: 0.87x on Windows (22.4); 1.49x on the Linux runner, met by a hair | [PERF.md](PERF.md) section 11 and section 6, "Result on a Linux GitHub runner" |
| Disk after 500 steps, at most 3x git (22.5) | Windows: met at rest (2.10x) and, after the WAL bound (23.5), 2.02x with the handle open. Linux runner: **missed**, 3.20x with the handle open (11.1 MB against 3.5 MB); the at-rest figure was not measured there | [PERF.md](PERF.md) section 12 and section 6 |
| 500 steps, at most 3x shadow git on Linux | **met on a Linux runner**: 1.26 s against 12.15 s, 0.10x, format v4, one run, one seed | [PERF.md](PERF.md) section 6, "Result on a Linux GitHub runner" ([raw](../bench/results/2026-10-linux-runner-agent-500-v4.json)) |
| One-file save at 100,000 files, at most 1.5x the history-0 save | met on Windows, 1.02x at 100,000 files x 500 snapshots through the CLI; not measured on Linux | [PERF.md](PERF.md) section 6, [raw](../bench/results/2026-10-windows-ratio-100k-500.json) |

What changed since the last judgement: the Linux 500-step ratio now exists and
passes, and the 100,000 x 500 ratio is measured and passes on Windows. Red
because three things are still open:

- The disk target misses on the only Linux measurement (3.20x, open handle).
  The WAL bound that brings Windows to 2.02x was measured on Windows only, and
  the Linux at-rest disk was not measured.
- The CI guard does not yet cover everything above. The `perf` job in
  [`ci.yml`](../.github/workflows/ci.yml) fails a regression on Linux, but its
  Windows leg is advisory (`continue-on-error` for `windows-latest`), and
  `save_tree_100k` and `blame_walk` have no baseline entry in
  [`baseline.json`](../bench/perf/baseline.json) yet, so they are reported and
  pass. [`bench/perf/README.md`](../bench/perf/README.md) holds the steps to
  calibrate them on the runners; that was not done.
- The benchmark workflow [`bench.yml`](../.github/workflows/bench.yml) is
  manual.

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

- **No agent run has been done.** There is no `friction-log-<tag>.md` in
  [`bench/coldstart`](../bench/coldstart);
  [`friction-log.md`](../bench/coldstart/friction-log.md) is still the empty
  template, so the pre-written threshold in
  [`README.md`](../bench/coldstart/README.md) was not applied to any wedge. The
  kit (tasks, `prepare.py`, `audit.py`, `check.py`, `rank.py`) is committed and
  ready to run.
- **No outside person has used velo.**
  [`bench/outside/findings.md`](../bench/outside/findings.md) says so and has
  no rows: no reader and no consumer. Finding someone is the owner's task (see
  [`bench/outside/README.md`](../bench/outside/README.md)).
- **The limit, stated.** Even when the agent runs are done, agents are the only
  evidence this line would have. An agent that reads the README and succeeds
  shows that the steps are findable and followable by a model; it does not show
  that a person who has never seen velo understands what it is for or gets to a
  result without help. Until an outside reader or consumer is recorded, the line
  is judged on the agent runs alone and that limit goes with the verdict.
- The five-minute quickstarts run in CI
  ([`quickstarts.yml`](../.github/workflows/quickstarts.yml)), which shows the
  steps work, not that anyone can find and follow them.

## 10. A simulation converges: red

[`SIMULATION.md`](SIMULATION.md) records five runs and the run-six plan. Run
six, the one meant for the 6.2.x release, is "planned, not yet run": its row in
the summary table is "planned" throughout. Run five's findings are all fixed in
6.1.0, and "Open after this run: none of the findings above", but that run was
against 6.0.1 and left blind spots (`restore <path>`, `stash drop`, `mcp`, HTTP
remotes, large files, `.veloignore` and others) unexercised, and the oracle's
attribution warnings were unexplained when it ran. The line needs a run on the
current release with nothing open above green; none is recorded.

## 11. Bindings and the MCP server from an install: red (partial)

- The Python and Node packages are built, installed into a clean environment
  with no Rust toolchain, and exercised on every change:
  [`packages.yml`](../.github/workflows/packages.yml) (`wheel-install`,
  `node-install`). That half is green.
- A release now has a check that installs from release archives:
  [`release-verify.yml`](../.github/workflows/release-verify.yml) (called by
  [`release.yml`](../.github/workflows/release.yml) after `publish`) and
  [`release-smoke.yml`](../.github/workflows/release-smoke.yml), which runs
  `velo-mcp` and the quickstarts from the archives before publish. v6.2.0 is
  published with all 23 assets, and its checksums and three attestations verify
  ([record](../bench/results/2026-10-23-release-check-v6.2.0.md)).
- It is not green because no `release-verify` or `release-smoke` run has
  succeeded on a release. Both workflows exist on the `phase-23` branch only, so
  `gh run list --workflow release-smoke.yml` finds nothing on `main` and
  `release-verify` cannot be dispatched. The only runs are push runs on
  `phase-23`, and both failed:
  [run 39870618921](https://github.com/LucasVascovici/velo/actions/runs/39870618921)
  and [run 39870976394](https://github.com/LucasVascovici/velo/actions/runs/39870976394).
  In the second, every job passed except `velo-mcp` on `ubuntu-latest`
  ("Initialize and list tools over stdio"); the first also failed the Windows
  wheel, which the Python pin in the second appears to have fixed. The cause of
  the MCP failure is not recorded.
- It turns green when `velo-mcp` passes in `release-verify` on every runner and
  a run on a published release succeeds end to end after the branch merges.

## Decisions for the owner

These five are the owner's; none was taken here, and the README note was not
touched.

1. **The npm scope and the trademark check.** [`NAMES.md`](NAMES.md) shows
   `velo` taken on PyPI, npm and crates.io by unrelated projects, the unscoped
   `velo-core` and `velo-timeline` free on PyPI and npm, and the `@velo` scope
   as "owner's to check" because it needs a login. The trademark and prior-use
   search is a legal check and has not been done. Act on `docs/NAMES.md`.
2. **Whether the project keeps the name `velo`.** The evidence is the same
   table: the bare name is not available on any registry, and the recommended
   `velo-core` names are free. NAMES.md prices the options; renaming the
   project is its largest-cost row, and keeping `velo` with the `velo-core`
   distribution names is the cheap one. Act on `docs/NAMES.md`.
3. **Whether to commission the external review.** Line 2 says "no known path",
   and no independent review has been done; the fuzz targets and tests are not a
   proof. [`REVIEW_KIT.md`](REVIEW_KIT.md) is the pack for a reviewer and is
   ready. Act on `docs/REVIEW_KIT.md`.
4. **Which distribution channels to switch on.** v6.2.0 is on GitHub releases
   with checksums and attestations. crates.io, PyPI, npm, Homebrew, scoop,
   winget, containers, the MCP directory and the GitHub Action are the owner's;
   `distribution.yml` has never run on a release and the manifest generator has
   not been run against the real release. Act on `docs/RELEASING.md`, under
   "Channels", and settle the name decision above first, since the package
   names depend on it.
5. **Whether to enable the Pages deploy.** The docs site builds, and the deploy
   job in `docs-site.yml` stays off until the repository variable `DOCS_PAGES`
   is `true` and Pages is set to GitHub Actions; the Pages API answers 404 for
   the repository today. Act on `.github/workflows/docs-site.yml` and
   `docs/RELEASING.md`, "Docs site".

## Still red

Each red line, and what turns it green. This list is the next phase's input.

- **Line 6** (sizes): run Windows `envelope-a` and `envelope-b`, Linux
  `ratio-100k`, and the retention run on 6.x, each complete and committed to
  `bench/results`, and label the cells in [SCALE.md](SCALE.md).
- **Line 7** (headline workload): bring the Linux disk ratio under 3x on a
  runner, or have the owner restate that target; measure the at-rest Linux
  disk; calibrate the `save_tree_100k` and `blame_walk` baselines from a
  finished CI run and make the Windows leg of `perf` blocking
  ([`bench/perf/README.md`](../bench/perf/README.md)).
- **Line 9** (a stranger): run the cold-start protocol in
  [`bench/coldstart`](../bench/coldstart) on a release, commit
  `friction-log-<tag>.md` and apply the threshold; record at least one outside
  reader or consumer in
  [`bench/outside/findings.md`](../bench/outside/findings.md), or keep the line
  judged on agent runs alone with that limit stated.
- **Line 10** (simulation): run six against the 6.2.x release with the blind
  spots exercised, record it in [SIMULATION.md](SIMULATION.md), and repeat until
  nothing above green is open.
- **Line 11** (installs): merge the release workflows to `main`, find and fix
  the `velo-mcp` stdio failure on ubuntu, and get a successful `release-verify`
  (and `release-smoke`) run on a published release; link it here.
