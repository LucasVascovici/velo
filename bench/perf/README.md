# Performance guard

CI runs a small guard on every pull request (`perf` job in
`.github/workflows/ci.yml`, Ubuntu and Windows). It measures four workloads:

- `agent_steps`: 500 `WriteGuard::save_changes` steps over a 1,000-file project.
- `save_10k`: a one-file `save` in a 10,000-file working tree (median of 5).
- `save_tree_100k`: a one-file-change `WriteGuard::save_tree` on a 100,000-entry
  tree, previous snapshot as parent (median of 5; setup untimed).
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
release run of the guard took 199 s, well inside the Windows leg's 20 minute
limit. With 30,000 files it took 352 s, so the tree adds roughly 7.6 s per 1,000
files on Windows and the full 100,000 files would take about 13 minutes for the
tree alone, plus the other workloads; that is why the Windows leg shrinks the
tree. The `save_tree_100k` workload runs in the first round only (its
100,000-file setup dominates) and is a single sample, not a median.
No full-size run has been measured: WSL was not available to the agent that
wrote this, and Windows does not finish it within the 590 s a single
foreground call allows. Because that is unmeasured, the Ubuntu leg has a 60
minute limit rather than risking a timeout; the first Ubuntu CI run should be
read for the real figure and the limit tightened afterwards.

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
