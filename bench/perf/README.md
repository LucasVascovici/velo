# Performance guard

CI runs a small guard on every pull request (`perf` job in
`.github/workflows/ci.yml`, Ubuntu and Windows). It measures four workloads:

- `agent_steps`: 500 `WriteGuard::save_changes` steps over a 1,000-file project.
- `save_10k`: a one-file `save` in a 10,000-file working tree (median of 5).
- `save_tree_100k`: a one-file-change `WriteGuard::save_tree` on a 100,000-entry
  tree, previous snapshot as parent (median of 5; setup untimed; first round only).
- `blame_walk`: `blame` of a 200-line file at the tip of a 200-snapshot,
  two-author history (median of 5; setup untimed).

Environment overrides, for runners too slow for the full size:

- `VELO_PERF_AGENT_STEPS` (default 500): steps in `agent_steps`. CI sets 100 on Windows.
- `VELO_PERF_SAVE_TREE_FILES` (default 100000): files in `save_tree_100k`. CI sets
  10000 on Windows. A shortened run is not comparable to a full-size baseline, so
  the Windows baseline must be taken with the same value set.

Measured runtime (developer Windows machine, release build, a busy machine so
timings vary by tens of percent): with the Windows CI values
(`VELO_PERF_AGENT_STEPS=100`, `VELO_PERF_SAVE_TREE_FILES=10000`) the local
release run of the guard took 146 s of test time, well inside the Windows leg's
20 minute limit. An earlier measurement with 30,000 files (before the timed
saves reused the parent's stored objects) took 203 s, so the untimed setup adds
roughly 2.3 s per 1,000 files on Windows, an upper bound of about 6 minutes for
a full 100,000-file run there. The `save_tree_100k` workload runs in the first round only (its
100,000-file setup dominates); within that one run it takes the median of 5
one-file-change saves. No full-size run has been measured, on Windows or Linux,
so the Ubuntu leg keeps a 60 minute limit for now; the first Ubuntu CI run
should be read for the real figure and the limit tightened afterwards.

The two newest workloads have no baseline entry yet: they are reported and pass
until a baseline is calibrated on the runner.

Absolute milliseconds on a shared runner measure the runner, so each workload
is divided by a calibration run on the same machine: 64 MiB of blake3 hashing,
1,000 small files written through temp-file-plus-rename and read back, and
100,000 SQLite inserts into a table with two indexes. The three are run three
times, interleaved, and medians are taken. The ratio is `workload / calibration`.

`baseline.json` holds the committed ratios per operating system
(`linux`, `windows`). A measured ratio more than 1.6x its baseline fails the
job; one below baseline / 1.6 is reported as "faster: update the baseline" and
does not fail; a workload with no baseline entry is reported and passes. A
change that doubles a save's cost therefore turns the check red.

The first baselines were taken on a developer machine (Windows) and WSL2
(Linux). Calibrated ratios are meant to travel between machines, but the first
CI runs confirm or move them: each run uploads its measurements as the
`perf-<os>` artifact, and a baseline can be taken from it.

## Updating a baseline

Do it deliberately, in the same pull request as the change that moved the ratio,
so a regression is a red check and an improvement is a diff someone reads:

```sh
VELO_PERF_UPDATE=1 cargo test -p velo-core --release perf_guard -- --ignored
```

This rewrites the entry for the current OS only; run it on the other OS (or copy
the numbers from the CI artifact) for the other entry.

## v4 re-baseline

The Windows entry was re-baselined for the v4 engine (agent_steps 53.17 -> 8.23,
save_10k 0.2045 -> 0.0860). No metric got worse. The Linux entry is unchanged
because it was not measured here; copy it from the CI artifact of the first
green run on this branch.

## Calibrating the CI baselines (phase 23, not yet done)

`save_tree_100k` and `blame_walk` have no baseline entry, and the Windows leg
of the `perf` job is still advisory (`continue-on-error` in `ci.yml`). The
workload only reached its final form (save_tree_100k timed as a one-file
change, tree workload in the first round only) in the last commits of the
`phase-23` branch, so no finished CI run measured it yet. Baselines are never
taken from a developer machine, so none were invented. To finish:

1. Let `ci.yml` finish twice on `phase-23` after the last workload commit,
   with both `Perf` legs green.
2. For each run id: `gh run download <id> -n perf-Linux` and
   `gh run download <id> -n perf-Windows`.
3. Per OS and workload (`agent_steps`, `save_10k`, `save_tree_100k`,
   `blame_walk`), take the median as the baseline and compute max/min.
4. If every Windows spread is under 1.6x, write the medians into
   `baseline.json` (per-OS maps, see `write_entry` in `perf_guard.rs`), remove
   `continue-on-error` from the `perf` job and keep the Windows
   `VELO_PERF_AGENT_STEPS` and `VELO_PERF_SAVE_TREE_FILES` overrides. If a
   workload is at or above 1.6x, drop it from the Windows leg (or the leg) and
   record the measured spread here.
5. List the run ids and spreads in this section, in the same commit.
