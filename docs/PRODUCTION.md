# Production checklist

Eleven statements that would have to hold before velo drops the
"not production-grade" note from the [README](../README.md). Each is judged
here as **green**, with a link to its proof, or **red**, with the reason. This
page records the evidence; it does not make the decision. Removing or keeping
the note is the owner's call.

Current count: **10 green, 1 red** (line 7). Judged again on
2026-10-24 after simulation run six, the v6.2.1 release check, the Linux
benchmark runs, the cold-start test and the Windows envelope runs of 2026-10-26:
lines 6, 9, 10 and 11 turned green (line 9 on agent runs alone, with that limit
stated), and line 7 narrowed (its section says to what). The README note stays. The inputs to the decision are in
"Decisions for the owner" and "Still red" at the end of this page.

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
  tag (`v6.2.1`, `baseline-rev` in `ci.yml`), and the `package`, `parity`, corpus and `old-release` jobs
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
| 6 | The documented sizes are measured sizes, on the current format | green |
| 7 | The headline workload is within its stated factor, and a regression fails CI | **red (partial)** |
| 8 | The API and format promises are enforced by CI | green |
| 9 | A stranger can follow the README to a working result | green (agents only) |
| 10 | A simulation converges with no open bug above green | green |
| 11 | The bindings and the MCP server work from an install, not a checkout | green |

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

- [SCALE.md](SCALE.md), "Measured on 6.x (format v4)", is the current envelope.
  Every table there names its platform, velo version and whether the shape is
  full or reduced, and the 5.x tables are kept and labelled as v3. Raw runs:
  [`bench/results`](../bench/results), harness
  [`bench/scale`](../bench/scale).
- **Both envelope shapes and the ratio shape are measured on 6.x, full size,
  complete, on both platforms:** 10,000 files x 2,000 snapshots and 100,000 x
  200 on Windows 11 (6.2.1, 2026-10-26;
  [`a`](../bench/results/2026-10-26-windows-envelope-v4-a-full.json),
  [`b`](../bench/results/2026-10-26-windows-envelope-v4-b-full.json)) and on a
  Linux runner (6.2.0;
  [`a`](../bench/results/2026-10-linux-runner-envelope-v4-a.json),
  [`b`](../bench/results/2026-10-linux-runner-envelope-v4-b.json)); and 100,000
  x 500 on Windows
  ([result](../bench/results/2026-10-windows-ratio-100k-500.json)) and on the
  Linux runner
  ([result](../bench/results/2026-10-23-linux-runner-ratio-100k-500.json)). The
  full retention run, 10,000 x 2,000, ran on the Linux runner on 6.2.1
  ([log](../bench/results/2026-10-23-linux-runner-retention-10k-2000.txt)).
- **The full retention run also ran on Windows** (2026-10-26, a build of the
  6.2.2 branch;
  [log](../bench/results/2026-10-26-windows-retention-10k-2000.txt)): the same
  size outcome as Linux, and 1,323 s for the apply against 175 s on Linux and
  416 s before format v4. The machine was busy with other work, so that figure
  is a measurement and not a number to quote; the delete stage is 94% of it.
- **What the sentence does not claim, and SCALE.md says so:** Linux is a hosted
  runner and not a local machine, the two Linux envelope runs were on different CPUs, and the
  Linux envelope runs are on 6.2.0 and the rest on 6.2.1 (no format or save-path
  change between them). Every number is one run.

## 7. Headline workload against its stated factor: red (partial)

The verdicts below are as [PERF.md](PERF.md) records them, misses included.

| Target | Verdict | Where |
| --- | --- | --- |
| One-file save at 100,000 files, under 1 s (22.3) | partial: median 246 ms, slowest 496 ms, through the library `save_tree` on Windows. Through the CLI it is 0.69 s on the Linux runner (6.2.1, [raw](../bench/results/2026-10-23-linux-runner-ratio-100k-500.json)) and 1.8 to 1.9 s on Windows with Defender on, so the CLI misses on Windows only | [PERF.md](PERF.md), "After the parent-membership rule (22.3)" and section 6, "One-file save at 100,000 files, 500 snapshots (CLI, Windows)" |
| Who wrote this line, within 1.5x shadow git (22.4) | met: 0.87x on Windows (22.4); 1.47x on the Linux runner with 6.2.1 (0.258 s against 0.175 s), met by a hair | [PERF.md](PERF.md) section 11, [raw](../bench/results/2026-10-23-linux-runner-agent-500-6.2.1.json) |
| Disk after 500 steps, at most 3x git (22.5) | met on both platforms. Windows: 2.10x at rest, 2.02x with the handle open after the WAL bound (23.5). Linux runner, 6.2.1 with the WAL bound: **2.23x** with the handle open (6.86 MB against 3.08 MB) and 2.05x at rest (7.62 MB against 3.71 MB); it was 3.20x before the bound | [PERF.md](PERF.md) section 12, [raw](../bench/results/2026-10-23-linux-runner-agent-500-6.2.1.json) |
| 500 steps, at most 3x shadow git on Linux | **met on a Linux runner**: 1.45 s against 13.15 s, 0.11x, 6.2.1, one run, one seed (1.26 s against 12.15 s on 6.2.0) | [raw](../bench/results/2026-10-23-linux-runner-agent-500-6.2.1.json), [6.2.0 raw](../bench/results/2026-10-linux-runner-agent-500-v4.json) |
| One-file save at 100,000 files, at most 1.5x the history-0 save | met on both: 1.02x on Windows and 0.92x on the Linux runner, 100,000 files x 500 snapshots, through the CLI | [Windows raw](../bench/results/2026-10-windows-ratio-100k-500.json), [Linux raw](../bench/results/2026-10-23-linux-runner-ratio-100k-500.json) |

What changed since the last judgement: the Linux 500-step ratio now exists and
passes, and the 100,000 x 500 ratio is measured and passes on Windows. The CI
guard now covers all four workloads on both runners: the `perf` job in
[`ci.yml`](../.github/workflows/ci.yml) blocks on Linux (its Windows leg runs and reports but is advisory since 6.2.3), against
baselines in [`baseline.json`](../bench/perf/baseline.json) taken as medians of
three CI runs ([`bench/perf/README.md`](../bench/perf/README.md), "CI
calibration"). On 2026-10-23 the Linux disk target was met (2.23x with the
handle open, 2.05x at rest), so every target is now met on Linux. Red because
two things are still open, and both are partly a question of what the line
should demand, which is the owner's:

- The CLI one-file save at 100,000 files is 1.8 to 1.9 s on Windows with
  Defender scanning on, against "under 1 s". The target was set for the
  library `save_tree` (22.3), which meets it on Windows, and the CLI meets it
  on Linux; whether the CLI on a scanned Windows disk is part of the line is
  not settled.
- The benchmark workflow [`bench.yml`](../.github/workflows/bench.yml) is
  manual. The regression guard is the `perf` job, which blocks on Linux and reports on Windows; the manual workflow is the
  measurement, not the guard.

## 8. API and format promises enforced by CI: green

- [STABILITY.md](STABILITY.md) has a table of each promise and the check that
  enforces it: the `semver` job (cargo-semver-checks on `velo-core` and
  `velo-merge`), the `package` job, the `parity` job, and the golden corpus and
  `old-release` jobs, all in [`ci.yml`](../.github/workflows/ci.yml). A golden
  output example:
  [`compaction_in_place.golden`](../crates/velo-core/src/tests/compaction_in_place.golden).
- One recorded exception, `Bundle.signatures`, is described in
  [STABILITY.md](STABILITY.md) under "Not stable".

## 9. A stranger follows the README: green, on agent runs alone

- **The cold-start test ran on v6.2.1** (2026-10-24): four wedges, two runs
  each, every run a fresh session in its own sandbox outside any checkout, with
  the release assets installed and only `README.md`, `docs/LIBRARY.md` and the
  wedge's cookbook handed over. All eight runs reached a green `check.py`, each
  in under five minutes against the 45-minute limit, so **all four wedges pass
  the threshold written before the runs**. The log, the audit reading and the
  ranked defects are in
  [`friction-log-v6.2.1.md`](../bench/coldstart/friction-log-v6.2.1.md).
- **What round one found.** No run hit a velo error. The friction is documentation:
  the API reference the pages link to is not reachable (four runs), API shapes
  had to be read from type stubs (four runs), the Rust page shows little beyond
  save, history and blame, and the MCP section does not say how to set the run
  id, which tools exist, how messages are framed, or that the author is the
  client's name. The checker also found a velo bug the agents did not run into:
  `velo history` showed nothing in a repository written through the library,
  fixed in 6.2.2.
- **Round two ran on v6.2.2** (2026-10-26), after the documentation fixes: the
  same eight-run protocol with fresh agents and new sandboxes. All eight runs
  passed again. Distinct defects fell from 20 to 15 and friction rows from 40 to
  31; four of round one's top five are gone and the fifth survives only as links
  to pages the test does not hand over. What remains is Node and Rust call shapes
  and two `parent` behaviours, all now written into `docs/LIBRARY.md`. See
  [`friction-log-v6.2.2-round2.md`](../bench/coldstart/friction-log-v6.2.2-round2.md).
  The audit script was corrected between the rounds (it had flagged every
  transcript on the host's own records and on quoted links); under it all
  sixteen runs are valid.
- **The limit, stated.** Agents are the only evidence this line has. An agent
  that reads the README and succeeds shows that the steps are findable and
  followable by a model; it does not show that a person who has never seen velo
  understands what it is for or gets to a result without help. No outside
  reader or consumer is recorded in
  [`bench/outside/findings.md`](../bench/outside/findings.md). The line is green
  on the agent runs alone, as this page's earlier verdict allowed, and the limit
  goes with it.

## 10. A simulation converges: green

Run six ran against 6.2.1 on 2026-10-23 and is recorded in
[`SIMULATION.md`](SIMULATION.md), "Run six".

- **Convergence.** Every clone ended at the remote's tip and `fsck` was clean
  on the remote and on all nine clones. The full run (9 developers, 4 rounds,
  2,354 commands) was reported "not converged" by the oracle over five work
  markers missing from main; each was traced through the remote's history, and
  none is a velo loss: four were removed when a developer rewrote the lines
  that carried them (snapshot `3a97c25`, a mission to edit those lines), one
  when a developer forced `resolve --mark --force` past velo's warning, which
  named the line it dropped. The second pass (rounds 1 and 2 again, 933
  commands) converged with no oracle problem.
- **Blind spots.** The full run's blind-spot missions did not reach the
  developers, so the second pass and a scripted check covered them: twelve
  checks against 6.2.1, all passing
  ([results](../bench/results/2026-10-23-run-six-blind-spots.json)): stash
  `-m`, show and drop; remote add and remove; `.veloignore`; the exit-code
  rule; the mid-merge refusals; the HTTP token message; a 32 MB file over HTTP;
  tags reaching a fresh clone; a 5.3.0 client against a 6.2.1 remote; 5.3.0
  refusing a v4 repository; a 6.2.1 bundle applied by 5.3.0; `velo-mcp` over
  stdio.
- **Bugs.** Three confirmed, none data-losing, all fixed in 6.2.2 with a
  regression test each: `velo conflicts` showing lines when both sides moved
  the same line (medium, display only), a path restore counting unsaved changes
  outside its paths (low), and `undo` of a local merge warning about pushed
  history (low). Nothing above green is open.
- **Limits.** The blind spots were exercised by a script, not by simulated
  developers, so they show the behaviour is right, not that a developer finds
  it. UX findings remain open (below); they are not bugs, and the line is about
  bugs.
- **Not met, recorded:** commands per refused push were 38.6 in the second
  pass against a target of under 4 (21.5). `velo sync` went from one use to 35
  and worked whenever it was used; developers still read the manual recipe
  printed after it as the instructions.

## 11. Bindings and the MCP server from an install: green

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
- **Green on the published v6.2.1.** In the v6.2.1 release run,
  `release-smoke` ran `velo-mcp` and the quickstarts from the archives on
  Linux and Windows and passed before publish. `release-verify` then failed
  two jobs, and both failures were the workflow's own: on macOS, a Homebrew
  `cargo` left on `PATH` stopped the "no toolchain" step, and on Ubuntu the
  `velo-mcp` binary was picked with an `ls` of a missing `.exe`, which failed
  under `pipefail` before the server started. Both were fixed in the workflow
  (pull request #15), and a dispatched `release-verify` of the published v6.2.1
  passed every job: the CLI on four platforms, `velo-mcp` on Linux and Windows,
  the wheels and Node tarballs on three platforms, checksums and attestations
  ([run 39965842878](https://github.com/LucasVascovici/velo/actions/runs/39965842878)).
- The release run itself still shows red, because it ran the workflow before
  the fix. The next tagged release runs the fixed one end to end.

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
4. **Which distribution channels to switch on.** v6.2.1 is on GitHub releases
   with checksums and attestations, and its release run generated the
   Homebrew, scoop and winget manifests from the real release (none was
   submitted). crates.io, PyPI, npm, Homebrew, scoop, winget, containers, the
   MCP directory and the GitHub Action are the owner's. Act on `docs/RELEASING.md`, under
   "Channels", and settle the name decision above first, since the package
   names depend on it.
5. **Whether to enable the Pages deploy.** The docs site builds, and the deploy
   job in `docs-site.yml` stays off until the repository variable `DOCS_PAGES`
   is `true` and Pages is set to GitHub Actions; the Pages API answers 404 for
   the repository today. Act on `.github/workflows/docs-site.yml` and
   `docs/RELEASING.md`, "Docs site".

## Still red

Each red line, and what turns it green. This list is the next phase's input.

- **Line 7** (headline workload): every target is met on Linux. Open: the CLI
  one-file save at 100,000 files on Windows with Defender on (1.8 to 1.9 s
  against "under 1 s"), and whether the manual `bench.yml` counts against a
  line whose regression guard is the blocking `perf` job. Both are partly the
  owner's call on what the line demands.

Open UX findings from run six, not bugs and not counted by any line: the
refused-push loop (above); an unborn branch forgets its start point after
switching away; `velo push <branch>` with no remote reads the branch as a
remote; refs are positional in `show` and `diff` but flags in `blame`, `grep`
and `history`; `push` and `sync` publish branches and tags nobody named.
