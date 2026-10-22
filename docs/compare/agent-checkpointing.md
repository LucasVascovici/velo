# Agent checkpointing: velo against a shadow Git repository and `git worktree`

The audience: someone building an agent that wants a trail of what it did and a
way back. Method, systems and workloads:
[bench/agent/README.md](../../bench/agent/README.md). Summary page:
[Is velo for you?](../IS_VELO_FOR_YOU.md).

Every table cites its file. Workload throughout: 1,000 files, 500 steps, 1 to 20
files rewritten per step, seed 1, then 8 attempts of 10 steps with a merge, then
100 restores of step 250. Windows 11 with Defender real-time scanning on; the
Linux run is WSL2 on ext4. Speeds moved between days, so compare columns of one
file, not across files.

## Windows, 2026-10-20, velo on format v4

Files: [velo](../../bench/results/2026-10-20-windows-agent-velo.json) and
[shadow git](../../bench/results/2026-10-20-windows-agent-git.json). The two
were run separately. `git worktree` was not run in these.

| | velo | shadow git | git worktree |
| --- | ---: | ---: | --- |
| 500 steps | 19.4 s | 210.5 s | not measured on v4 |
| 8 attempts and merge | 4.3 s | 38.5 s | not measured on v4 |
| restore step 250, median | 480 ms | 480 ms | not measured on v4 |
| who wrote this line, 20 queries | 9.30 s | 3.01 s | not measured on v4 |
| disk after steps | 11.03 MB | 3.27 MB | not measured on v4 |
| disk, final | 11.84 MB | 4.54 MB | not measured on v4 |

Velo is faster at checkpoints and branches and **slower at who-wrote and in
disk**.

## Windows, 2026-10-22, after the blame skip

File: [2026-10-22-windows-agent-who-wrote.json](../../bench/results/2026-10-22-windows-agent-who-wrote.json).
Only velo and shadow git were run.

| | velo | shadow git |
| --- | ---: | ---: |
| 500 steps | 9.7 s | 202.3 s |
| 8 attempts and merge | 1.8 s | 37.8 s |
| restore step 250, median | 240 ms | 538 ms |
| who wrote this line, 20 queries | 3.29 s | 3.77 s |
| disk after steps | 11.07 MB | 3.69 MB |

Who-wrote is now level with Git, within noise: a second velo-only run took
1.32 s ([PERF.md](../PERF.md), section 11). Disk is still about 3.0x as the file
recorded it, with the write-ahead log of an open handle counted; at rest it is
2.10x ([2026-10-22-agent-disk-breakdown.txt](../../bench/results/2026-10-22-agent-disk-breakdown.txt),
Windows, 2026-10-22, a different seed from the table above).

## Windows, 2026-10-16, all seven systems, velo before format v4

File: [agent-2026-10-16-windows.json](../../bench/results/agent-2026-10-16-windows.json).
Velo here is the old storage (152 MB on disk), so do not read its disk or step
column as current; the other systems are the point of this table.

| system | 500 steps | 8 attempts and merge | restore median | who wrote x20 | disk after steps |
| --- | ---: | ---: | ---: | ---: | ---: |
| velo | 64.4 s | 13.2 s | 216 ms | 2.95 s | 152.3 MB |
| velo (5.2 API) | 87.3 s | 16.4 s | 844 ms | 5.21 s | 152.0 MB |
| shadow git | 190.9 s | 31.0 s | 422 ms | 2.42 s | 3.27 MB |
| git worktree | 174.2 s | 39.5 s | 466 ms | 2.49 s | 3.27 MB |
| dir copy | 434.0 s | 79.8 s | 729 ms | 10.68 s | 488.5 MB |
| jsonl log | 1.39 s | 0.38 s | 705 ms | 0.55 s | 6.35 MB |
| sqlite blobs | 8.84 s | 2.09 s | 738 ms | 0.002 s | 28.5 MB |

`git worktree` here is an ordinary repository with a worktree per attempt; after
the branch workload it holds 12.6 MB in total, against 4.5 MB for shadow git,
because each attempt's checked-out tree counts. The log and SQLite rows are
faster than velo at writing a step and, for the log, smaller on disk; they record
nothing that can be verified afterwards (their provenance column reads "none"),
and neither has a merge. That is the reason to choose velo over them, not speed.

## Linux (WSL2, ext4), 2026-10-17, velo before format v4

File: [agent-2026-10-16-linux.json](../../bench/results/agent-2026-10-16-linux.json)
(the file's date field reads 2026-10-17).

| system | 500 steps | restore median | who wrote x20 | disk after steps |
| --- | ---: | ---: | ---: | ---: |
| velo | 37.4 s | 12.9 ms | 0.35 s | 151.9 MB |
| shadow git | 8.4 s | 28.5 ms | 0.23 s | 3.74 MB |
| git worktree | 8.1 s | 37.1 ms | 0.23 s | 3.31 MB |

On Linux velo was **4.5 times slower than shadow git at checkpoints** on that
run. After format v4 velo's Linux step is a 6.78 ms median and 6.3 s for 500
steps ([2026-10-linux-agent-500.json](../../bench/results/2026-10-linux-agent-500.json),
Linux, 2026-10, the Rust profiler, not the same harness), but Git was not run
with it: the current Linux ratio is **not measured**. Disk on Linux with v4 is
**not measured**.

## The difference that is not in the tables

A shadow Git repository records a commit per step; the author on it is whatever
the agent set. In velo the author and metadata (agent name, run, tool call) are
part of the snapshot id, `velo fsck` recomputes them, and `velo blame` and
`velo report` answer "which run wrote this line". See
[cookbook/agent-checkpointing.md](../cookbook/agent-checkpointing.md) and the
[sample report](../report-sample.html).

Choose Git instead if the agent already works inside a Git repository and you
only need the files back, or if disk is the constraint: it is smaller in every
row above.
