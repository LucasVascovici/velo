# Is Velody for you?

This page says where velody is slower than Git, where it is faster, what it costs
in disk, and the one thing the alternatives do not give you. Every number comes
from a file committed in this repository, and each one names the file, the
platform and the date of the run. Where nothing was measured the page says
"not measured" and does not estimate.

Read the caveats first, because they apply to every row:

- One machine, one run per cell, unless a row says otherwise. Windows 11 with
  Defender real-time scanning on; "Linux" is WSL2 on ext4, not a bare-metal box.
  Compare systems within a platform and within a run, not across days: the
  machine's speed moved between runs (see [PERF.md](PERF.md), section 1).
- The workload is the agent benchmark: 1,000 files of about 1 KB, 500 steps, each
  step rewriting 1 to 20 files. Method:
  [bench/agent/README.md](../bench/agent/README.md). It is not your workload.
- "Shadow git" is a Git repository that tracks a working directory from the
  outside (`--git-dir` and `--work-tree`), committing after each step.

## Where Velody is faster than Git

| What | velody | shadow git | Source, platform, date |
| --- | ---: | ---: | --- |
| 500 checkpoints | 19.4 s | 210.5 s | [velody](../bench/results/2026-10-20-windows-agent-velody.json) and [git](../bench/results/2026-10-20-windows-agent-git.json), Windows, 2026-10-20 |
| 500 checkpoints, repeat | 9.7 s | 202.3 s | [2026-10-22-windows-agent-who-wrote.json](../bench/results/2026-10-22-windows-agent-who-wrote.json), Windows, 2026-10-22 |
| 8 attempts of 10 steps, then merge | 4.3 s | 38.5 s | same two 2026-10-20 files, Windows |
| Restore step 250, median of 100 | 480 ms | 480 ms (a tie) | same two 2026-10-20 files, Windows |
| Restore step 250, median of 100, repeat | 240 ms | 538 ms | [2026-10-22-windows-agent-who-wrote.json](../bench/results/2026-10-22-windows-agent-who-wrote.json), Windows, 2026-10-22 |

Much of the Windows gap is the cost of starting `git` and writing its loose
files under Defender, one process per step. The Windows runs are the only
post-format-v4 comparison against shadow git: **shadow git was not re-run on
Linux after format v4, so the Linux ratio is not measured.** The next section
has the last Linux ratio that was.

## Where Velody is slower than Git

- **Linux checkpoints, last measured ratio.** On 2026-10-17 (before format v4),
  500 steps took 37.4 s in velody and 8.4 s in shadow git: Velody was 4.5 times
  slower ([agent-2026-10-16-linux.json](../bench/results/agent-2026-10-16-linux.json)).
  Format v4 then cut velody's Linux step from about 93 ms to a 6.78 ms median
  (6.3 s for 500 steps, [2026-10-linux-agent-500.json](../bench/results/2026-10-linux-agent-500.json),
  Linux, 2026-10), but Git was not run alongside it, so whether the 4.5x is now
  closed is **not measured** ([PERF.md](PERF.md), section 9, "Linux 500-step
  re-run"). That run also shows the median step 36% slower at 500 steps than at
  200, so step time does grow with history there.
- **Who wrote this line.** On 2026-10-20 (Windows) 20 queries took 9.30 s in velody
  and 3.01 s in shadow git, 3.1 times slower. A blame skip then brought it to
  3.29 s against 3.77 s, 0.87x, on 2026-10-22
  ([2026-10-22-windows-agent-who-wrote.json](../bench/results/2026-10-22-windows-agent-who-wrote.json)).
  A second velody-only run of the same repository gave 1.32 s, so the figure is
  noisy; the committed one is the slower of the two. The Linux figure after the
  skip is **not measured**. Before it (2026-10-17, Linux) the two were close:
  0.35 s against 0.23 s ([agent-2026-10-16-linux.json](../bench/results/agent-2026-10-16-linux.json)).
- **Disk.** See the next section.
- **First save on Windows.** The first save of 10,000 new files took 55.5 s
  (Windows, 2026-10-22,
  [2026-10-22-windows-envelope-v4-a.json](../bench/results/2026-10-22-windows-envelope-v4-a.json)),
  one run under Defender, which moved that number by a factor of two between
  releases ([SCALE.md](SCALE.md)). A first save of 100,000 files did not finish
  inside the 590 s limit on 6.x; a 30,000-file run saved at a flat 0.41 s median
  from the fourth save on. Git's first add of the same tree was **not measured**.
- **A one-file save at 100,000 files.** Median 5.28 s over 400 saves, about 2.5x
  the 2.1 s floor from an earlier build, and not a like-for-like pair
  ([PERF.md](PERF.md), section 6, Windows, 2026-10-20). Git at that size was
  **not measured**. Velody checks that every file's object is present on each save.
- **Missed budgets.** The Python binding's overhead was 10.5% of the step on
  Windows when last measured, against a 10% budget, and was not re-measured
  ([PERF.md](PERF.md), section 6). The `save_changes` call cut the step time by
  26.2% on Windows and 9.2% on Linux, short of the 32.6% and 27.3% that were
  expected ([PERF.md](PERF.md), section 4).

## What it costs in disk

On the 1,000-file, 500-step workload on Windows:

| Store | After 500 steps | Source |
| --- | ---: | --- |
| velody, handle open | 11.03 MB, 3.37x shadow git | [2026-10-20-windows-agent-velody.json](../bench/results/2026-10-20-windows-agent-velody.json), 2026-10-20 |
| velody, handle open, seed-0 breakdown run | 11.16 MB, 3.41x | [2026-10-22-agent-disk-breakdown.txt](../bench/results/2026-10-22-agent-disk-breakdown.txt), Windows, 2026-10-22 |
| velody, handle dropped | 6.88 MB, 2.10x | same breakdown file |
| shadow git | 3.27 MB | [2026-10-20-windows-agent-git.json](../bench/results/2026-10-20-windows-agent-git.json), 2026-10-20 |

The 3x disk budget was **missed** as measured: 3.37x. About 4.28 MB of the open
figure is SQLite's write-ahead log, which is checkpointed and removed when the
last handle closes (every CLI call does), so a repository at rest is 2.10x Git.
Velody keeps every snapshot's tree queryable in SQLite, which is the price of the
speed above; the breakdown is in [PERF.md](PERF.md), section 12. Before format
v4 the same workload took 152 MB
([agent-2026-10-16-windows.json](../bench/results/agent-2026-10-16-windows.json),
Windows, 2026-10-16). The size at 10,000 files by 200 snapshots is in
[SCALE.md](SCALE.md): `.velo` 8.6 MiB (Windows, 2026-10-22). A comparison of
disk at that size with Git was **not measured**.

## The one thing no alternative has

Velody hashes the author and the snapshot's metadata (for example an agent name,
a run id, a tool call) into the snapshot id. `velody fsck` recomputes every id, so
a changed author or metadata value is detected the same way a changed file is,
and a snapshot can carry a signature that covers the whole tree. Blame can be
asked by run, because the run is in the id. Git hashes content and commit
fields, but who or what made a commit is whatever the committer typed; a log, a
copy or a SQLite table records nothing that can be checked later (the
provenance column of
[agent-2026-10-16-windows.json](../bench/results/agent-2026-10-16-windows.json)
and [bench/agent/README.md](../bench/agent/README.md)). The details and limits
are in [INTEGRITY.md](INTEGRITY.md) and [THREAT_MODEL.md](THREAT_MODEL.md):
Velody does not store private keys or revoke them.

If you do not need that, a plain Git repository is a smaller store, and for
code you already review by pull request it is the right tool.

## Not a fit

Real-time co-editing, high-frequency event sourcing, large binary assets and use
as a database ([Internals](INTERNALS.md#where-it-fits)).

## Comparisons by audience

- [Agent checkpointing](compare/agent-checkpointing.md): against a shadow Git
  repository and `git worktree`.
- [Registries](compare/registries.md): against an append-only log or a SQLite
  store.
- [Document history](compare/document-history.md): against Git.
