# Performance profile: where the time goes

This page is the table that Phase 19 chooses its work from. It holds the
stage timers of `save`, `save_tree` and `restore` at 1,000, 10,000 and 100,000
files with 0, 500 and 2,000 snapshots of history, the agent benchmark's step
time split into engine and binding, the SQLite option experiment, and the
performance targets that follow from the numbers.

Every number here was measured on 2026-10-16 at commit `7da4f96` by the
harnesses described in [`bench/README.md`](../bench/README.md#profile-stage-timers).
The raw JSON lines are in
[`bench/results/perf-2026-10-16-windows.json`](../bench/results/perf-2026-10-16-windows.json) and
[`bench/results/perf-2026-10-16-linux.json`](../bench/results/perf-2026-10-16-linux.json); the agent
runs are in
[`bench/results/agent-velo-only-2026-10-16-windows.json`](../bench/results/agent-velo-only-2026-10-16-windows.json) and
[`bench/results/agent-velo-only-2026-10-16-linux.json`](../bench/results/agent-velo-only-2026-10-16-linux.json).
Earlier scale figures are in [`SCALE.md`](SCALE.md).

## 1. Method and machine

- **Machine.** Intel Core i7-1370P (20 logical CPUs), one laptop for both
  platforms (WSL2 was given 15 GiB of RAM). Windows is Windows 11 Enterprise 10.0.22621 on NTFS. Linux is
  **WSL2** on the same laptop (Ubuntu 22.04, kernel 5.15.153.1, ext4 inside the
  WSL2 virtual disk, work directory `/tmp` on that ext4, source tree copied to
  ext4 so nothing is read through `/mnt/c`), as in [`SCALE.md`](SCALE.md). The two
  columns therefore compare filesystems and scanners on identical hardware, not
  different machines.
- **Build.** Release build with the stage timers compiled in:
  `cargo test -p velo-core --release --features timing`, run with
  `--ignored --nocapture --test-threads=1`. The Windows toolchain is rustc 1.94.0,
  the Linux one cargo 1.97.1. The timers are laps taken around named stages
  (`save.scan`, `save.commit`, ...); a table cell is the median over the samples of
  the sum of that lap inside one operation.
- **What is measured.** `save` is the CLI path (`commands::save::run`): one file
  rewritten on disk, then a save. `save_tree` is the library path from the tip of
  a side branch, one entry replaced, parent entries carried by object id
  (`caller.*` rows are the caller's own `tree_at` and entry building, outside the
  engine). `restore` is `commands::restore::run` alternating between the middle
  of the history and the tip, with `force`. At history 0 there is only one
  snapshot to move between, so `restore` there measures the scan and an almost
  empty diff, not file writes.
- **Histories.** Cells below 10,000 x 500 build their history with real saves.
  Cells at or above 10,000 x 500 use `VELO_PROFILE_BULK=1`: the harness writes
  the `snapshots` and `file_map` rows directly, one transaction per snapshot, so a
  table reaches 50 million rows in hours instead of weeks. Those rows reach the
  same table size as real saves but their ids do not verify (`fsck` fails on
  them), so they are for measuring only. Bulk cells carry a **B** in every
  column heading. The first save and the last-snapshot checkout are real saves in
  every cell.
- **Samples.** One run per cell. Cells up to 10,000 files use 10 samples per
  operation. At 100,000 files a single sample can take an hour on Windows, so
  100,000 x 0 on Windows used 3 samples and 100,000 x 500 used 2 on both platforms
  (`VELO_PROFILE_SAMPLES`); the median of 2 is the lower of the two. Linux
  100,000 x 0 uses 10 samples (a first run with 3 was discarded and repeated).
  The row `samples` in each table says which.
- **Defender.** Windows Defender real-time protection was on and the work
  directory was not excluded (the exclusion list is not readable without admin,
  so assume none). Real-time scanning inflates every small-file number on Windows,
  sometimes by a large factor, and adds heavy tail latency. Do not compare a
  Windows figure with a Linux one without that caveat; the clearest cases are
  `save.scan`, `restore.write_files`, `save_tree.objects` and the read loop below.
- **Noise.** The SQLite experiment runs the unchanged configuration twice in one
  process (`baseline`, `baseline_again`): they differ by 2.9% on Windows and
  6.2% on Linux, so differences under about 6% are noise. The Windows agent run
  recorded 91 s for velo's 500 steps on 2026-10-15 and 128 s today, so a single
  Windows run can move by 40%.

## 2. Stage tables

Medians in ms. Rows are lap names; the last two rows are the median total of the
operation (this includes time outside any lap) and the sample count. A `-` is a
cell that was not run (see section 7). A **B** after a heading is a bulk history.

### Windows (NTFS, Defender on)

#### save - Windows

| stage (ms) | 1k x 0 | 1k x 500 | 1k x 2000 | 10k x 0 | 10k x 500 B | 10k x 2000 B | 100k x 0 | 100k x 500 B | 100k x 2000 n/m |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| `save.begin_tx` | 0.0 | 0.1 | 0.0 | 0.1 | 0.2 | 0.1 | 0.1 | 0.1 | - |
| `save.commit` | 2.5 | 110 | 124 | 68.9 | 1,923 | 1,241 | 704 | 14,780 | - |
| `save.hash_store` | 1.0 | 1.6 | 1.1 | 1.1 | 2.5 | 1.0 | 1.4 | 1.2 | - |
| `save.insert_file_map` | 3.3 | 5.5 | 4.8 | 27.6 | 98.9 | 57.3 | 372 | 3,581 | - |
| `save.insert_rest` | 0.2 | 0.2 | 0.1 | 0.4 | 1.0 | 0.4 | 5.9 | 7.8 | - |
| `save.parent_file` | 1.7 | 1.4 | 0.8 | 1.1 | 2.4 | 1.2 | 1.6 | 1.8 | - |
| `save.parent_tree` | 0.5 | 0.8 | 0.5 | 4.2 | 8.7 | 3.6 | 39.4 | 39.6 | - |
| `save.prepare` | 0.2 | 0.4 | 0.3 | 0.2 | 0.5 | 0.2 | 0.3 | 0.3 | - |
| `save.scan` | 66.2 | 71.3 | 72.0 | 122 | 307 | 157 | 903 | 1,507 | - |
| `save.snapshot_id` | 0.3 | 0.4 | 0.2 | 3.7 | 7.9 | 3.3 | 48.1 | 50.2 | - |
| **total (median)** | 79.5 | 193 | 213 | 230 | 2,374 | 1,506 | 2,144 | 20,086 | - |
| samples | 10 | 10 | 10 | 10 | 10 | 10 | 3 | 2 | - |

| cell | top stage | second stage | total (ms) |
|---|---|---|---:|
| 1k x 0 | `save.scan` 66.2 ms (83%) | `save.insert_file_map` 3.3 ms (4%) | 79.5 |
| 1k x 500 | `save.commit` 110 ms (57%) | `save.scan` 71.3 ms (37%) | 193 |
| 1k x 2000 | `save.commit` 124 ms (58%) | `save.scan` 72.0 ms (34%) | 213 |
| 10k x 0 | `save.scan` 122 ms (53%) | `save.commit` 68.9 ms (30%) | 230 |
| 10k x 500 B | `save.commit` 1,923 ms (81%) | `save.scan` 307 ms (13%) | 2,374 |
| 10k x 2000 B | `save.commit` 1,241 ms (82%) | `save.scan` 157 ms (10%) | 1,506 |
| 100k x 0 | `save.scan` 903 ms (42%) | `save.commit` 704 ms (33%) | 2,144 |
| 100k x 500 B | `save.commit` 14,780 ms (74%) | `save.insert_file_map` 3,581 ms (18%) | 20,086 |

#### save_tree - Windows

| stage (ms) | 1k x 0 | 1k x 500 | 1k x 2000 | 10k x 0 | 10k x 500 B | 10k x 2000 B | 100k x 0 | 100k x 500 B | 100k x 2000 n/m |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| `caller.entries` | 0.0 | 0.0 | 0.0 | 0.4 | 0.1 | 0.1 | 1.9 | 2.5 | - |
| `caller.tree_at` | 1.2 | 1.1 | 0.7 | 10.7 | 11.3 | 10.7 | 94.3 | 120 | - |
| `save_tree.commit` | 3.6 | 139 | 132 | 169 | 1,557 | 1,630 | 643 | 10,740 | - |
| `save_tree.insert_file_map` | 4.0 | 4.8 | 3.9 | 29.8 | 99.1 | 74.1 | 289 | 2,888 | - |
| `save_tree.insert_rest` | 0.0 | 0.0 | 0.0 | 0.0 | 0.1 | 0.1 | 0.0 | 0.0 | - |
| `save_tree.objects` | 57.9 | 42.1 | 31.4 | 371 | 816 | 364 | 3,548 | 7,132 | - |
| `save_tree.snapshot_id` | 0.4 | 0.4 | 0.2 | 2.3 | 5.3 | 2.3 | 34.4 | 25.3 | - |
| `save_tree.validate` | 0.4 | 0.4 | 0.2 | 2.5 | 2.9 | 2.9 | 20.3 | 33.1 | - |
| **total (median)** | 79.1 | 189 | 169 | 604 | 2,565 | 2,089 | 4,749 | 21,124 | - |
| samples | 10 | 10 | 10 | 10 | 10 | 10 | 3 | 2 | - |

#### restore - Windows

| stage (ms) | 1k x 0 | 1k x 500 | 1k x 2000 | 10k x 0 | 10k x 500 B | 10k x 2000 B | 100k x 0 | 100k x 500 B | 100k x 2000 n/m |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| `restore.cache_seed` | 0.2 | 0.7 | 1.5 | 0.2 | 3.0 | 4.1 | 0.2 | 3.9 | - |
| `restore.diff` | 0.9 | 0.7 | 0.8 | 6.7 | 18.1 | 6.8 | 124 | 137 | - |
| `restore.ghosts` | 0.4 | 0.4 | 0.4 | 3.0 | 8.6 | 3.2 | 71.3 | 67.0 | - |
| `restore.parent_file` | 1.0 | 1.0 | 1.1 | 0.8 | 2.4 | 1.1 | 1.1 | 0.9 | - |
| `restore.scan` | 21.7 | 39.6 | 21.8 | 86.5 | 237 | 135 | 910 | 1,422 | - |
| `restore.target_tree` | 0.6 | 0.5 | 0.5 | 3.1 | 7.7 | 3.2 | 33.3 | 32.8 | - |
| `restore.validate` | 0.7 | 10.7 | 35.2 | 1.8 | 41.4 | 50.4 | 22.8 | 44.7 | - |
| `restore.write_files` | 3.2 | 43.0 | 184 | 2.1 | 126 | 264 | 2.2 | 91.6 | - |
| **total (median)** | 32.3 | 101 | 254 | 107 | 450 | 579 | 1,163 | 1,809 | - |
| samples | 10 | 10 | 10 | 10 | 10 | 10 | 3 | 2 | - |

| cell | top stage | second stage | total (ms) |
|---|---|---|---:|
| 1k x 0 | `restore.scan` 21.7 ms (67%) | `restore.write_files` 3.2 ms (10%) | 32.3 |
| 1k x 500 | `restore.write_files` 43.0 ms (43%) | `restore.scan` 39.6 ms (39%) | 101 |
| 1k x 2000 | `restore.write_files` 184 ms (72%) | `restore.validate` 35.2 ms (14%) | 254 |
| 10k x 0 | `restore.scan` 86.5 ms (81%) | `restore.diff` 6.7 ms (6%) | 107 |
| 10k x 500 B | `restore.scan` 237 ms (53%) | `restore.write_files` 126 ms (28%) | 450 |
| 10k x 2000 B | `restore.write_files` 264 ms (46%) | `restore.scan` 135 ms (23%) | 579 |
| 100k x 0 | `restore.scan` 910 ms (78%) | `restore.diff` 124 ms (11%) | 1,163 |
| 100k x 500 B | `restore.scan` 1,422 ms (79%) | `restore.diff` 137 ms (8%) | 1,809 |


### Linux (WSL2, ext4)

#### save - Linux

| stage (ms) | 1k x 0 | 1k x 500 | 1k x 2000 | 10k x 0 | 10k x 500 B | 10k x 2000 B | 100k x 0 | 100k x 500 B | 100k x 2000 n/m |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| `save.begin_tx` | 0.0 | 0.1 | 0.1 | 0.0 | 0.1 | 0.1 | 0.1 | 0.1 | - |
| `save.commit` | 0.4 | 73.5 | 81.0 | 63.4 | 709 | 805 | 1,182 | 11,694 | - |
| `save.hash_store` | 0.1 | 0.2 | 0.2 | 0.3 | 0.4 | 1.4 | 3.1 | 4.0 | - |
| `save.insert_file_map` | 1.6 | 4.7 | 5.3 | 29.9 | 34.9 | 44.4 | 340 | 6,076 | - |
| `save.insert_rest` | 0.1 | 0.2 | 0.2 | 0.5 | 0.4 | 0.5 | 9.1 | 6.2 | - |
| `save.parent_file` | 0.1 | 0.2 | 0.1 | 0.2 | 0.2 | 0.3 | 0.2 | 0.2 | - |
| `save.parent_tree` | 0.3 | 0.7 | 0.7 | 4.8 | 6.0 | 4.8 | 49.0 | 40.2 | - |
| `save.prepare` | 0.0 | 0.0 | 0.0 | 0.0 | 0.1 | 0.0 | 0.1 | 0.1 | - |
| `save.scan` | 7.9 | 15.8 | 15.1 | 41.1 | 52.8 | 46.9 | 356 | 266 | - |
| `save.snapshot_id` | 0.3 | 0.4 | 0.4 | 5.1 | 6.1 | 5.3 | 55.3 | 52.2 | - |
| **total (median)** | 11.4 | 96.5 | 106 | 147 | 803 | 917 | 2,079 | 18,726 | - |
| samples | 10 | 10 | 10 | 10 | 10 | 10 | 10 | 2 | - |

| cell | top stage | second stage | total (ms) |
|---|---|---|---:|
| 1k x 0 | `save.scan` 7.9 ms (70%) | `save.insert_file_map` 1.6 ms (14%) | 11.4 |
| 1k x 500 | `save.commit` 73.5 ms (76%) | `save.scan` 15.8 ms (16%) | 96.5 |
| 1k x 2000 | `save.commit` 81.0 ms (76%) | `save.scan` 15.1 ms (14%) | 106 |
| 10k x 0 | `save.commit` 63.4 ms (43%) | `save.scan` 41.1 ms (28%) | 147 |
| 10k x 500 B | `save.commit` 709 ms (88%) | `save.scan` 52.8 ms (7%) | 803 |
| 10k x 2000 B | `save.commit` 805 ms (88%) | `save.scan` 46.9 ms (5%) | 917 |
| 100k x 0 | `save.commit` 1,182 ms (57%) | `save.scan` 356 ms (17%) | 2,079 |
| 100k x 500 B | `save.commit` 11,694 ms (62%) | `save.insert_file_map` 6,076 ms (32%) | 18,726 |

#### save_tree - Linux

| stage (ms) | 1k x 0 | 1k x 500 | 1k x 2000 | 10k x 0 | 10k x 500 B | 10k x 2000 B | 100k x 0 | 100k x 500 B | 100k x 2000 n/m |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| `caller.entries` | 0.0 | 0.0 | 0.0 | 0.3 | 0.1 | 0.2 | 1.4 | 1.4 | - |
| `caller.tree_at` | 0.8 | 0.9 | 1.1 | 9.9 | 9.3 | 8.4 | 65.4 | 76.6 | - |
| `save_tree.commit` | 0.6 | 78.3 | 98.0 | 97.8 | 761 | 1,009 | 1,892 | 14,128 | - |
| `save_tree.insert_file_map` | 3.1 | 4.6 | 5.1 | 22.7 | 28.2 | 40.4 | 263 | 5,138 | - |
| `save_tree.insert_rest` | 0.0 | 0.0 | 0.0 | 0.0 | 0.0 | 0.0 | 0.0 | 0.1 | - |
| `save_tree.objects` | 1.7 | 2.0 | 1.8 | 16.5 | 14.5 | 19.1 | 186 | 192 | - |
| `save_tree.snapshot_id` | 0.3 | 0.3 | 0.3 | 2.2 | 1.9 | 2.9 | 26.5 | 38.1 | - |
| `save_tree.validate` | 0.3 | 0.3 | 0.3 | 2.5 | 2.6 | 2.6 | 16.7 | 15.6 | - |
| **total (median)** | 7.7 | 86.4 | 105 | 154 | 819 | 1,067 | 2,379 | 19,746 | - |
| samples | 10 | 10 | 10 | 10 | 10 | 10 | 10 | 2 | - |

#### restore - Linux

| stage (ms) | 1k x 0 | 1k x 500 | 1k x 2000 | 10k x 0 | 10k x 500 B | 10k x 2000 B | 100k x 0 | 100k x 500 B | 100k x 2000 n/m |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| `restore.cache_seed` | 0.1 | 0.8 | 1.4 | 0.1 | 1.0 | 2.6 | 0.2 | 1.2 | - |
| `restore.diff` | 1.0 | 0.5 | 0.7 | 7.9 | 5.7 | 5.5 | 93.7 | 81.0 | - |
| `restore.ghosts` | 0.4 | 0.2 | 0.3 | 2.7 | 2.6 | 2.5 | 59.3 | 42.0 | - |
| `restore.parent_file` | 0.2 | 0.2 | 0.2 | 0.1 | 0.2 | 0.2 | 0.1 | 0.1 | - |
| `restore.scan` | 11.2 | 8.9 | 8.7 | 40.3 | 36.9 | 30.4 | 273 | 297 | - |
| `restore.target_tree` | 0.7 | 0.4 | 0.5 | 4.2 | 3.3 | 2.8 | 35.8 | 37.2 | - |
| `restore.validate` | 0.3 | 0.4 | 0.9 | 1.4 | 1.7 | 2.3 | 32.6 | 26.1 | - |
| `restore.write_files` | 1.5 | 5.0 | 12.8 | 1.2 | 4.7 | 12.9 | 2.4 | 27.5 | - |
| **total (median)** | 15.6 | 16.4 | 24.4 | 58.5 | 60.5 | 75.8 | 544 | 568 | - |
| samples | 10 | 10 | 10 | 10 | 10 | 10 | 10 | 2 | - |

| cell | top stage | second stage | total (ms) |
|---|---|---|---:|
| 1k x 0 | `restore.scan` 11.2 ms (71%) | `restore.write_files` 1.5 ms (10%) | 15.6 |
| 1k x 500 | `restore.scan` 8.9 ms (54%) | `restore.write_files` 5.0 ms (30%) | 16.4 |
| 1k x 2000 | `restore.write_files` 12.8 ms (52%) | `restore.scan` 8.7 ms (36%) | 24.4 |
| 10k x 0 | `restore.scan` 40.3 ms (69%) | `restore.diff` 7.9 ms (13%) | 58.5 |
| 10k x 500 B | `restore.scan` 36.9 ms (61%) | `restore.diff` 5.7 ms (9%) | 60.5 |
| 10k x 2000 B | `restore.scan` 30.4 ms (40%) | `restore.write_files` 12.9 ms (17%) | 75.8 |
| 100k x 0 | `restore.scan` 273 ms (50%) | `restore.diff` 93.7 ms (17%) | 544 |
| 100k x 500 B | `restore.scan` 297 ms (52%) | `restore.diff` 81.0 ms (14%) | 568 |


### First save and the library read loop

`first_save` is the initial snapshot of the tree. `read_loop` materialises the
tip into an empty directory the way a library caller without `restore` does:
`tree_at`, then `read_file_at` and a file write per file. It is not a stage of
`save` or `restore`, but it is what the agent benchmark's restore does and it
is the slowest thing measured here.

Windows:

| op (ms, median) | 1k x 0 | 1k x 500 | 1k x 2000 | 10k x 0 | 10k x 500 B | 10k x 2000 B | 100k x 0 | 100k x 500 B |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| `first_save` | 690 | 743 | 1,891 | 20,877 | 21,619 | 17,253 | 510,475 | 173,573 |
| `read_loop` | 649 | 1,994 | 3,801 | 21,579 | 107,771 | 214,954 | 1,820,084 | 2,381,763 |
| `read_loop`: `read_file_at` share | 30% | 62% | 84% | 10% | 87% | 96% | 95% | 96% |

Linux:

| op (ms, median) | 1k x 0 | 1k x 500 | 1k x 2000 | 10k x 0 | 10k x 500 B | 10k x 2000 B | 100k x 0 | 100k x 500 B |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| `first_save` | 77.7 | 81.3 | 73.8 | 677 | 641 | 653 | 6,236 | 6,842 |
| `read_loop` | 30.0 | 216 | 1,148 | 384 | 13,036 | 50,178 | 4,326 | 208,986 |
| `read_loop`: `read_file_at` share | 55% | 91% | 98% | 57% | 97% | 99% | 60% | 98% |

## 3. Top costs

The top two stages by median time, per cell, are in the "top stage" tables in
section 2. Summarised by size (history 0 and deep history differ, so both are
named):

**Save (CLI path)**

| files | Windows | Linux |
|---|---|---|
| 1,000 | history 0: `save.scan`, `save.insert_file_map`. History 500 and 2,000: `save.commit`, `save.scan` | the same |
| 10,000 | history 0: `save.scan`, `save.commit`. History 500 and 2,000: `save.commit`, `save.scan` | history 0 and deep: `save.commit`, `save.scan` |
| 100,000 | history 0: `save.scan`, `save.commit`. History 500: `save.commit`, `save.insert_file_map` | history 0: `save.commit`, `save.scan`. History 500: `save.commit`, `save.insert_file_map` |

The first cost of a save is `save.commit`, and it is the only stage that grows
with history: from 1.2 s at 100,000 x 0 to 11.7 s at 100,000 x 500 on Linux (0.4 ms
to 74 ms at 1,000 files), while the scan does not. `save.scan` is the second cost
and the first on Windows at history 0 (Defender); at 100,000 files and deep history
`save.insert_file_map` (one row per file per snapshot) overtakes it. A save at
100,000 files costs 2.1 s on both platforms at history 0 and 18.7 s (Linux) or
20.1 s (Windows) at history 500.

**Restore**

| files | Windows | Linux |
|---|---|---|
| 1,000 | history 0: `restore.scan`, `restore.write_files`. 500: `restore.write_files`, `restore.scan`. 2,000: `restore.write_files`, `restore.validate` | history 0 and 500: `restore.scan`, `restore.write_files`. 2,000: `restore.write_files`, `restore.scan` |
| 10,000 | history 0: `restore.scan`, `restore.diff`. 500: `restore.scan`, `restore.write_files`. 2,000: `restore.write_files`, `restore.scan` | history 0 and 500: `restore.scan`, `restore.diff`. 2,000: `restore.scan`, `restore.write_files` |
| 100,000 | `restore.scan`, `restore.diff` at both histories | `restore.scan`, `restore.diff` at both histories |

The first cost of a restore is `restore.scan` (a walk of the working directory
comparing it to the target), 50 to 80% of the time at 100,000 files; the second
is `restore.diff` at 10,000 files and up, and `restore.write_files` at 1,000
files with deep history, where more files differ between the middle and the tip.
Restore does not grow with history in the engine: 544 ms at 100,000 x 0 against
568 ms at 100,000 x 500 on Linux.

**The library read path is where the time actually goes.** `read_file_at`, called
once per file, is 91% of the read loop at 1,000 x 500 and 99% at 10,000 x 2,000 on
Linux, and it grows with history (216 ms at 1,000 x 500, 1,148 ms at 1,000 x 2,000;
209 s at 100,000 x 500). On the same snapshot, `restore` takes 16 ms (1,000 x 500)
and 568 ms (100,000 x 500). Windows is the same shape, with the file write
(Defender) a large share at low history.

## 4. The agent workload three ways

The agent benchmark's velo workload (500 steps over 1,000 files, 1 to 20 files
rewritten per step, [`bench/agent`](../bench/agent/README.md)) was run through the
Python binding and through `profile_agent_workload`, which drives the same
workload straight at `velo-core` in three variants. All figures are the sum of
the timed step work over 500 steps, in seconds.

| path | what is timed | Windows (s) | Linux (s) |
|---|---|---:|---:|
| Python harness (`agent_bench.py --only velo`) | `checkpoint` calls: build entries in Python, `save_tree` through the binding, `tree_at` re-read | 128.1 | 53.8 |
| Rust `as_harness` | the same work, direct on `velo-core`: build entries, `save_tree`, `tree_at` re-read | 114.6 | 50.8 |
| Rust `no_reread` | as above, but the object ids are predicted instead of re-read with `tree_at` | 121.3 | 37.7 |
| Rust `save_tree_only` | only the `save_tree` call | 86.3 | 39.1 |
| Python harness, `save_changes` (`agent_bench.py --only velo`, 2026-10-16 run) | `checkpoint` calls: only the changed files go through the binding with `save_changes`; no entry list, no `tree_at` re-read | 64.4 | 37.4 |

The share of the Python step time that is binding and conversion overhead is
(Python harness - Rust `as_harness`) / Python harness:

- **Windows: (128.1 - 114.6) / 128.1 = 10.5%**
- **Linux: (53.8 - 50.8) / 53.8 = 5.6%**

The four numbers per platform are the Python harness, `as_harness`, `no_reread`
and `save_tree_only` totals in the table. The rest of the Python step is the
engine: `save_tree_only` is 67% of the Python total on Windows and
73% on Linux, and the `tree_at` re-read plus entry building that the
harness does around the call (`as_harness` minus `save_tree_only`) is
22% on Windows and 22% on Linux. `no_reread` is noisy: on Windows it is slower than `as_harness` (121 s against
115 s), which the work cannot explain, so read Windows differences under about
10% as run-to-run variation (section 1). On Linux `no_reread` and
`save_tree_only` agree within 4% (37.7 s and 39.1 s) and both sit about 12 s below
`as_harness`; that gap is the `tree_at` re-read after each save (its p95 step is
213 ms against 112 ms), which `save_changes` is meant to remove.

Inside the engine the step is the commit: of the median `save_tree` lap time on
Linux (about 91 ms per step), `save_tree.commit` is 83 ms (`insert_file_map`
4.5 ms, `objects` 2.7 ms). On Windows `save_tree.commit` is 135 ms and
`save_tree.objects` 101 ms (Defender on object files). Binding overhead is the
smaller problem: it is about a tenth of the step time or less, while the
`save_tree` call alone is two thirds to three quarters of it.

**Did the step time fall by the share 19.1 measured?** 19.1 attributed
10.5% (Windows) and 5.6% (Linux) of the Python step to the binding, and a
further 22% on both platforms to the entry building and `tree_at` re-read that
`save_changes` removes, so the calling convention as a whole was worth about
33% on Windows and 27% on Linux (Python harness against `save_tree_only`).
The 2026-10-16 run measured the old and the new harness back to back, so the
comparison below uses that run's own `velo (5.2 API)` row (87.3 s Windows,
41.2 s Linux), not the 19.1 figures, because every system's absolute times moved
between the two days:

| | 5.2 API (s) | `save_changes` (s) | fall | 19.1 share | missed by |
|---|---:|---:|---:|---:|---:|
| Windows | 87.3 | 64.4 | 26.2% | 32.6% | 6.4 points |
| Linux | 41.2 | 37.4 | 9.2% | 27.3% | 18.1 points |

So it fell on both platforms but did not reach the 19.1 estimate on either:
Windows came close (it beats the 10.5% binding share by itself and misses the
combined share by about a fifth), and Linux fell about a third of the way
(9.2% against 27.3%). Against the 19.1 table directly the drop looks larger
(128.1 s to 64.4 s on Windows, 53.8 s to 37.4 s on Linux), but most of that is
the machine, not the API: the 5.2 row in the same new run is already 87.3 s and
41.2 s. On Linux the new step total (37.4 s) is near 19.1's `save_tree_only`
(39.1 s), but that is a cross-day comparison and only indicative. What is left
is plausibly the engine's commit. A possible, unmeasured, explanation for the
estimate being too high on Linux is that the 19.1 `as_harness` and
`save_tree_only` totals were single runs. Restore, which 19.1 did not break out,
fell from 844 ms to 216 ms median on Windows and from 145 ms to 13 ms on Linux
(`materialize` against `tree_at` plus `read_file_at` per file).

The Python harness's other workloads, from the same two runs, were: Windows
branches 25.3 s, restore median 2,230 ms, who-wrote 9.5 s, disk 174 MB; Linux
branches 7.4 s, restore median 180 ms, who-wrote 0.4 s, disk 174 MB.

## 5. SQLite options

`sqlite_options_one_file_save` at 100,000 files x 200 snapshots (a **B** history:
bulk rows; the one repository is shared by all variants, which add 55 snapshots
to the 200, negligible). Each row changes one thing against the unchanged
baseline and measures a one-file `save_tree`, median of 5. Delta is against the
first `baseline`. Nothing here changed production settings.

Windows (NTFS, Defender on):

| variant | median save_tree (ms) | delta (ms) | delta (%) | `commit` (ms) |
|---|---:|---:|---:|---:|
| `baseline` | 17,428 | +0 | +0.0% | 11,964 |
| `synchronous=OFF` | 8,203 | -9,225 | -52.9% | 3,128 |
| `synchronous=FULL` | 20,054 | +2,626 | +15.1% | 14,517 |
| `wal_autocheckpoint=0` | 7,779 | -9,649 | -55.4% | 1,363 |
| `wal_autocheckpoint=10000` | 17,894 | +466 | +2.7% | 12,885 |
| `cache_size=-16384` | 17,023 | -405 | -2.3% | 11,821 |
| `cache_size=-262144` | 17,317 | -111 | -0.6% | 12,609 |
| `mmap_size=0` | 18,169 | +741 | +4.3% | 13,119 |
| `mmap_size=1073741824` | 17,436 | +8 | +0.0% | 12,564 |
| `baseline_again` | 17,927 | +499 | +2.9% | 12,793 |
| `drop_idx_filemap_path` | 4,139 | -13,289 | -76.3% | 595 |

| schema variant (row-insert stage, 100,000 rows) | insert (ms) | delta (ms) | bytes/row | delta bytes |
|---|---:|---:|---:|---:|
| `schema_rowid_two_indexes` | 569 | +0.0% | 291 | +0.0% |
| `schema_without_rowid` | 487 | -14.4% | 190 | -34.6% |
| `schema_blob_hashes` | 370 | -34.9% | 186 | -36.3% |

Linux (WSL2, ext4):

| variant | median save_tree (ms) | delta (ms) | delta (%) | `commit` (ms) |
|---|---:|---:|---:|---:|
| `baseline` | 10,420 | +0 | +0.0% | 9,041 |
| `synchronous=OFF` | 1,776 | -8,645 | -83.0% | 491 |
| `synchronous=FULL` | 13,546 | +3,126 | +30.0% | 12,332 |
| `wal_autocheckpoint=0` | 2,769 | -7,651 | -73.4% | 235 |
| `wal_autocheckpoint=10000` | 16,069 | +5,648 | +54.2% | 13,750 |
| `cache_size=-16384` | 11,954 | +1,534 | +14.7% | 10,655 |
| `cache_size=-262144` | 10,227 | -194 | -1.9% | 9,260 |
| `mmap_size=0` | 10,893 | +473 | +4.5% | 9,843 |
| `mmap_size=1073741824` | 10,809 | +389 | +3.7% | 9,641 |
| `baseline_again` | 11,066 | +645 | +6.2% | 10,035 |
| `drop_idx_filemap_path` | 1,173 | -9,248 | -88.7% | 838 |

| schema variant (row-insert stage, 100,000 rows) | insert (ms) | delta (ms) | bytes/row | delta bytes |
|---|---:|---:|---:|---:|
| `schema_rowid_two_indexes` | 1,365 | +0.0% | 291 | +0.0% |
| `schema_without_rowid` | 697 | -48.9% | 190 | -34.6% |
| `schema_blob_hashes` | 365 | -73.2% | 186 | -36.3% |

What the table says:

- Two things move a save at this size by more than noise: skipping the sync
  (`synchronous=OFF`, -53% Windows, -83% Linux, and not acceptable as a default
  because it gives up durability) and the `file_map` path index
  (`drop_idx_filemap_path`, -76% Windows, -89% Linux, which removes maintenance
  of a second b-tree per row but the index serves lookups by path). Both attack the
  commit stage, which falls from 12.0 s to 0.6 s (Windows) and 9.0 s to 0.8 s
  (Linux) with the index gone.
- `wal_autocheckpoint=0` also shows -55% and -73% but only postpones the work:
  the deferred passive checkpoint costs 16.5 s (Windows) and 51.1 s (Linux).
- `synchronous=FULL` is slower (+15%, +30%); the page cache and mmap settings are
  within noise, except `wal_autocheckpoint=10000` and the small cache on Linux
  (+54% and +15%), which are single runs beside a 6% baseline spread and go the
  wrong way for a tuning.
- Row layout matters for size as much as for time: `WITHOUT ROWID` stores 190
  bytes per row against 291 (-35%) and a blob hash column 186 (-36%).

## 6. Targets

**Proposed targets** (from the Phase 19 plan, on the 18.13 workload on Linux:
500 steps within 2x of shadow git, restore within 2x of shadow git, disk within
2x of shadow git, and a one-file save at 100,000 files flat across 200
snapshots), against what was measured. Shadow-git figures are from the 18.13 run
([`agent-2026-10-15-linux.json`](../bench/results/agent-2026-10-15-linux.json),
[`-windows.json`](../bench/results/agent-2026-10-15-windows.json)), not repeated
today; velo is today's run.

| | shadow git, Linux (18.13) | velo, Linux today | ratio | velo, Windows today | shadow git, Windows (18.13) |
|---|---:|---:|---:|---:|---:|
| 500 steps (s) | 11.3 | 53.8 | 4.8x | 128.1 | 221.3 |
| restore median (ms) | 46 | 180 | 3.9x | 2,230 | 470 |
| disk after steps (MB) | 3.8 | 152 | 40x | 152 | 3.3 |
| one-file save, 100,000 files (s) | - | 2.1 at history 0, 18.7 at 500 (8.9x) | not flat | 2.1 at history 0, 20.1 at 500 (9.4x) | - |

**Derived targets.** Each replaces the proposal and says how it follows from the
numbers above.

1. **500 steps: at most 3x shadow git on Linux (34 s against git's 11.3 s);
   keep 2x as a stretch, not a target.** Today a step is about 107 ms on Linux,
   of which `save_tree.commit` is 83 ms and everything else (engine remainder,
   `tree_at` re-read, binding) about 25 ms, which is 12.5 s per 500 steps and
   already above the 11.3 s that git takes in total. Cutting the commit to a third
   (the size of effect the options in section 5 show on the commit stage) gives
   about 28 ms + 25 ms = 53 ms per step, 27 s, 2.4x git. Hitting 2x (22.6 s) needs
   the commit under 20 ms, which no safe option measured reaches
   (`synchronous=OFF` would, and is excluded). Windows already runs the workload
   in less time than shadow git (128 s against 221 s); the target there is not to
   regress against it. Windows figures vary by up to 40% between runs.
2. **Restore (materialise a step into an empty directory): at most 2x shadow git
   on Linux (92 ms against 46 ms), at most about 1 s on Windows (git 470 ms).**
   The engine's own `restore` is 16 to 24 ms at 1,000 files and 500 to 2,000
   snapshots, so the work is not the target-defining cost: the per-file
   `read_file_at` is 91% of the read loop. Without it the loop is a
   `tree_at` plus file writes, about 17 ms on Linux at 1,000 x 500, plus binding
   cost, so 2x git is reachable by batching the reads. On Windows the floor is the
   file write under Defender (756 ms at 1,000 x 500 in the read loop), so 2x git
   (940 ms) is only just above what the writes alone cost; the target is
   "about 1 s".
3. **Disk after 500 steps: at most 100 MB with a schema change, and at most 3x
   shadow git (about 11 MB) if snapshots store only the files that changed.**
   2x git (7.6 MB) cannot be reached by any option measured: 500 snapshots x 1,000
   files is 500,000 `file_map` rows at 291 bytes, 145 of the 152 MB. The two schema
   variants save about 35% (to 190 and 186 bytes per row, roughly 95 MB). Storing
   only changed rows (an average of 10.5 files per step) plus a full tree every
   50 steps is 500 x 10.5 + 10 x 1,000 = 15,250 rows, about 4.4 MB, plus the 7 MB
   of everything else, about 11 MB, which is where 3x comes from. The deltas are
   a design question for 19.5; the 100 MB bound does not depend on it.
4. **One-file save at 100,000 files: at 500 snapshots, at most 1.5x the cost at
   history 0 (Linux 3.1 s, Windows 3.2 s; today 18.7 s and 20.1 s), and the same
   ratio at 200 snapshots.** "Flat" is not measurable, so it is made a ratio: the
   history-0 save is the floor, 2.1 s on both platforms, of which the part that
   does not grow (scan, parent tree, snapshot id) is 0.46 s on Linux, and the
   parts that grow are `save.commit` and the row insert. The index
   experiment (-89% on Linux, -76% on Windows at 200 snapshots, commit 0.8 s
   against 9.0 s) shows that most of the growth is index maintenance, so 1.5x is
   inside what the data allows; flat (1.0x) is not, because the row insert alone
   is 0.34 s at history 0 and 6 s at 500.
5. **Binding overhead: at most 10% of the Python step time.** Measured: 10.5% on
   Windows and 5.6% on Linux. This is a new target; it needs no engine work and is
   there to catch a regression in the binding. It must be read with the engine
   target 1: if the engine step drops to 50 ms the same absolute binding cost is a
   larger share.

## 7. Not measured

- **100,000 files x 2,000 snapshots (both platforms).** About 200 million
  `file_map` rows, roughly 55 GiB at the measured 291 bytes per row. It fits the
  free disk on Linux (about 880 GB free) and, with little margin, on Windows
  (about 83 GiB free on `C:`), so it was **not skipped for disk but for time**:
  building 100,000 x 500 with bulk rows took about 2 hours on each platform, so
  the 2,000 build alone extrapolates to about 8 hours each before one sample is
  taken, and the read loop at 100,000 x 500 already took 40
  minutes per sample on Windows (209 s on Linux) and grows with history. The
  100,000 x 500 cells, whose `save.commit` and `insert_file_map` growth is the
  evidence, stand in for it.
- **Fewer samples.** Windows 100,000 x 0 (3 samples), Windows and Linux
  100,000 x 500 (2 samples). Medians of 2 or 3 are weak; read those columns as
  magnitude, not as a precise figure.
- **Bulk histories.** 10,000 x 500, 10,000 x 2,000 and 100,000 x 500 (both
  platforms) and the whole SQLite experiment (100,000 x 200) use synthetic rows,
  not real saves; they match real histories in table size and shape, not in
  object-store contents.
- **Real-save histories at 10,000 x 500 and above.** Not run: a real save at that
  size takes seconds, so 500 of them would take longer than the bulk build.
- **Restore at history 0** measures the scan only (see section 1).
- **Other systems in the agent benchmark.** Only velo was re-run
  (`--only velo`); the shadow git figures in section 6 are from the 18.13 run.
- **Changes-only agent workload.** The row in section 4 is empty until the
  `save_changes` measurement fills it.
- **One run per cell, one machine,** with WSL2 standing in for Linux.
