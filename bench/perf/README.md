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

- `VELODY_PERF_AGENT_STEPS` (default 500): steps in `agent_steps`. CI sets 100 on Windows.
- `VELODY_PERF_SAVE_TREE_FILES` (default 100000): files in `save_tree_100k`. CI sets
  10000 on Windows. A shortened run is not comparable to a full-size baseline, so
  the Windows baseline must be taken with the same value set.

Measured runtime (developer Windows machine, release build, a busy machine so
timings vary by tens of percent): with the Windows CI values
(`VELODY_PERF_AGENT_STEPS=100`, `VELODY_PERF_SAVE_TREE_FILES=10000`) the local
release run of the guard took 146 s of test time, well inside the Windows leg's
20 minute limit. An earlier measurement with 30,000 files (before the timed
saves reused the parent's stored objects) took 203 s, so the untimed setup adds
roughly 2.3 s per 1,000 files on Windows, an upper bound of about 6 minutes for
a full 100,000-file run there. The `save_tree_100k` workload runs in the first
round only (its 100,000-file setup dominates); within that one run it takes the
median of 5 one-file-change saves. On the CI runners the whole perf job,
including the full-size 100,000-file tree on Ubuntu, took under 2 minutes on
Ubuntu and 4-11 minutes on Windows, so both legs have a 20 minute limit.

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

The baselines are medians of CI runs on the GitHub runners themselves (see
"CI calibration" below). Each run uploads its measurements as the `perf-<os>`
artifact, and a baseline is taken from those, not from a developer machine.

## Updating a baseline

Do it deliberately, in the same pull request as the change that moved the ratio,
so a regression is a red check and an improvement is a diff someone reads:

```sh
VELODY_PERF_UPDATE=1 cargo test -p velody-core --release perf_guard -- --ignored
```

This rewrites the entry for the current OS only; run it on the other OS (or copy
the numbers from the CI artifact) for the other entry.

## v4 re-baseline

The Windows entry was re-baselined for the v4 engine (agent_steps 53.17 -> 8.23,
save_10k 0.2045 -> 0.0860). No metric got worse. The Linux entry is unchanged
because it was not measured here; copy it from the CI artifact of the first
green run on this branch.

## CI calibration (phase 23)

Both entries of `baseline.json` are the per-workload medians of three green
`ci.yml` runs on `phase-23`, all on the final workload code: 39932862928
(f33925f), 39933199028 (6016770) and 39934899978 (b4d01e3). The Windows leg
runs with `VELODY_PERF_AGENT_STEPS=100` and `VELODY_PERF_SAVE_TREE_FILES=10000`,
as its baseline does. Spread is max/min over the three runs:

| Workload | Linux median | Linux spread | Windows median | Windows spread |
| :--- | ---: | ---: | ---: | ---: |
| `agent_steps` | 3.116 | 1.09x | 1.180 | 1.38x |
| `save_10k` | 0.1924 | 1.14x | 0.0908 | 1.20x |
| `save_tree_100k` | 0.4680 | 1.11x | 0.00680 | 1.16x |
| `blame_walk` | 0.1585 | 1.20x | 0.0378 | 1.24x |

Every spread is under the 1.6x tolerance, so the Windows leg was made
blocking; since 6.2.3 it is advisory again (`continue-on-error`: a failure shows red
but holds nothing), because Defender and a busy runner made it random. The old Linux
`agent_steps` entry (79.96) predated the v4 engine and the bounded WAL and was
never re-taken; the old Windows entry (8.23) was taken locally at 500 steps,
not the 100 the Windows leg runs. Both moved down, so no workload got slower.
Three runs is a small sample: if a leg goes red without a code change, add
runs and re-take the median rather than raising the tolerance.
