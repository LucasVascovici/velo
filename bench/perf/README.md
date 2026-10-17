# Performance guard

CI runs a small guard on every pull request (`perf` job in
`.github/workflows/ci.yml`, Ubuntu and Windows). It measures two workloads:

- `agent_steps`: 500 `WriteGuard::save_changes` steps over a 1,000-file project.
- `save_10k`: a one-file `save` in a 10,000-file working tree (median of 5).

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
