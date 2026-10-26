# Scale: how far velo has been tested

This page says how far velo has been measured, how fast it is there, where it
degrades, and what to do about it. Up to 5.3 velo kept every snapshot's whole file
list in the `file_map` table, so its metadata grew as files x snapshots (see
[FORMAT.md](FORMAT.md) sections 11.4 and 11.5); format v4 stores what changed
instead, and the "5.x" sections below are that older behaviour, kept as history.

## Measured on 6.x (format v4)

**Tested on 6.x to 10,000 files x 2,000 snapshots and to 100,000 files x 200
snapshots, on Windows 11 and on a Linux runner, and to 100,000 files x 500
snapshots on both. On every shape the one-file save stays flat against history
(0.92x to 1.12x of the history-0 save on the shapes that report it, and 0.91x to 1.06x growth on all of them) and the repository is under 140 MiB, against
5.3 GiB for the same shapes on 5.3.0.** Beyond those shapes nothing is claimed.

These are the current numbers. Everything below the "5.x" headings is history
from the v3 storage and is kept, losses included. Every table says its
platform, its velo version and whether the shape is **full** (one of the
envelope shapes, 10,000 files x 2,000 snapshots or 100,000 files x 200, or the
100,000 x 500 ratio shape) or **reduced** (smaller than any of them). Nothing
here is extrapolated from another shape: a shape that has no table was not run.

All runs are one run, one file changed per snapshot, the real release CLI
through `bench/scale/scale.py`.

| Shape | Size | Platform | velo | Date | Raw data |
| --- | --- | --- | --- | --- | --- |
| 10,000 x 200 | reduced | Windows 11 | 6.2.0 | 2026-10-22 | `2026-10-22-windows-envelope-v4-a.json` |
| 30,000 x 20 | reduced, partial | Windows 11 | 6.2.0 | 2026-10-22 | `2026-10-22-windows-envelope-v4-b30k.json` |
| 50,000 x 40 | reduced | Windows 11 | 6.2.1 | 2026-10 | `2026-10-windows-ratio-50k-40.json` |
| 100,000 x 500 | full (ratio shape) | Windows 11 | 6.2.1 | 2026-10 | `2026-10-windows-ratio-100k-500.json` |
| 10,000 x 2,000 (`envelope-a`) | full | Windows 11 | 6.2.1 | 2026-10-26 | `2026-10-26-windows-envelope-v4-a-full.json` |
| 100,000 x 200 (`envelope-b`) | full | Windows 11 | 6.2.1 | 2026-10-26 | `2026-10-26-windows-envelope-v4-b-full.json` |
| 100,000 x 500 (ratio shape) | full | Linux, GitHub runner | 6.2.1 | 2026-10-23 | `2026-10-23-linux-runner-ratio-100k-500.json` |
| 10,000 x 2,000, retention | full | Linux, GitHub runner | 6.2.1 | 2026-10-23 | `2026-10-23-linux-runner-retention-10k-2000.txt` |
| 10,000 x 2,000, retention | full | Windows 11 | 6.2.2 (branch build) | 2026-10-26 | `2026-10-26-windows-retention-10k-2000.txt` |
| 10,000 x 2,000 (`envelope-a`) | full | Linux, GitHub runner | 6.2.0 | 2026-10-22 | `2026-10-linux-runner-envelope-v4-a.json` |
| 100,000 x 200 (`envelope-b`) | full | Linux, GitHub runner | 6.2.0 | 2026-10-22 | `2026-10-linux-runner-envelope-v4-b.json` |

Raw data lives under `bench/results/`. Windows machine: Intel Core i7-1370P,
Defender real-time scanning on, no exclusions. Linux runner: a hosted
`ubuntu-latest` runner (kernel 6.17 azure; AMD EPYC 9V45 for `envelope-a`,
EPYC 7763 for `envelope-b`, so the two Linux columns were on different CPUs),
dispatched from `main` at commit `dd15186` (GitHub Actions runs 39868500578
and 39868510865; both succeeded, in 3 m 38 s and 5 m 19 s).

### Windows 11, velo 6.2.0: reduced shapes

| Metric | 10,000 files x 200 snapshots (reduced) |
| --- | --- |
| first save (every file new) | 55.5 s |
| save, first 10% median / p95 | 234 / 329 ms |
| save, last 10% median / p95 | 231 / 324 ms |
| save growth, last / first median | 0.99x |
| `status` (clean) | 170 ms |
| `history` / `history --file` | 50 / 63 ms |
| `restore` to snapshot 100 / back to tip | 263 / 203 ms |
| `gc` / `fsck` | 0.56 s / 0.93 s |
| open (`velo branches`) | 42 ms |
| `.velo` / `velo.db` | 8.6 MiB / 7.0 MiB |
| `file_map` rows | 20,002 |

What this shows: at 200 snapshots the save time does not grow with history
(0.99x, against 1.60x on 5.3.0 at 2,000 snapshots), and a save of 10,000 files
with one change is about 0.23 s, against 1.4 s on 5.3.0 at the same file count.
The sizes are not comparable with the envelope's 5.3 GiB: that was 2,000
snapshots, this is 200, and v4 no longer writes a full `file_map` row set per
snapshot. The 0.99x is over a history ten times shorter than the 5.x one, so
it does not prove the 2,000-snapshot curve is flat on Windows (the Linux
`envelope-a` table below does cover 2,000 snapshots). The first save was
slower than 5.3.0's 31.5 s (55.5 s here); it is one run under Defender, which
moved that number by a factor of two between 5.2.0 and 5.3.0 as well, so it is
not a regression finding either way.

**30,000 files x 20 snapshots (reduced, partial).** The run finished its
saves: saves 4-20 had a median of 0.41 s (range 0.38-0.52 s, flat). The
harness timeout killed it at its `gc` step, so its first-save time, restore,
`fsck` and sizes were not recorded (the JSON is marked partial). Treat it as a
save-latency observation only.

**100,000 files x 20 snapshots did not complete** on 6.2.0: the first save did
not finish inside the 590 s limit (Defender scanning; see
[WINDOWS.md](WINDOWS.md)), so nothing is reported for that size from that
run. The later 100,000 x 500 run below, with no time limit, did finish.

| Metric | 50,000 files x 40 snapshots (reduced), 6.2.1 |
| --- | --- |
| first save (every file new) | 352.8 s |
| save, first 10% median / last 10% median | 1.66 s / 0.86 s |
| save, median / history-0 save | 0.93x |
| `status` (clean) | 0.70 s |
| `restore` to mid / back to tip | 0.97 s / 1.14 s |
| `gc` / `fsck` | 2.7 s / 3.7 s |
| `.velo` / `velo.db` | 28.3 MiB / 20.1 MiB |
| `file_map` rows | 50,001 |

The first-10% median is over 4 saves and its p95 is the first save itself, so
the 0.52x growth figure in the JSON is not a speed-up finding; the ratio to
history-0 (0.93x) is the figure to read.

### Windows 11, velo 6.2.1: the two envelope shapes (full)

Run 2026-10-26 on the published v6.2.1 binary, detached with no time limit, one
shape after the other, Defender real-time protection on with no exclusions. Both
finished complete.

| Metric | `envelope-a`: 10,000 files x 2,000 snapshots | `envelope-b`: 100,000 files x 200 snapshots |
| --- | --- | --- |
| first save (every file new) | 49.9 s | 531 s |
| save, first 10% median / p95 | 0.214 / 0.243 s | 1.44 / 1.66 s |
| save, last 10% median / p95 | 0.226 / 0.271 s | 1.50 / 1.68 s |
| **save, median / history-0 save** | **1.05x** | **1.12x** |
| save growth, last / first median | 1.06x | 1.04x |
| `status` (clean) | 0.15 s | 0.98 s |
| `history` / `history --file` | 67 / 98 ms | 44 / 50 ms |
| `restore` to mid / back to tip | 1.47 s / 0.88 s | 1.49 s / 1.64 s |
| `gc` / `fsck` | 0.79 s / 5.7 s | 7.3 s / 22.9 s |
| open (`velo branches`) | 41 ms | 43 ms |
| `.velo` / `velo.db` | 47.9 MiB / 46.0 MiB | 83.2 MiB / 66.9 MiB |
| `file_map` rows | 160,016 | 200,002 |

The same two shapes on 5.3.0 (format v3) took 1.4 to 2.2 s per save at 10,000
files and left a 5.3 GiB `velo.db`; here a save is 0.2 s, flat over 2,000
snapshots, and the whole repository is 48 MiB. The 100,000-file shape saves in
1.4 to 1.5 s with Defender scanning every touched file and no growth with
history. One run each, one machine.

### Windows 11, velo 6.2.1: 100,000 files x 500 snapshots (full, ratio shape)

| Metric | 100,000 files x 500 snapshots |
| --- | --- |
| first save (every file new) | 589 s |
| save, history-0 median | 1.94 s |
| save, first 10% median / p95 | 2.06 / 5.20 s |
| save, last 10% median / p95 | 1.98 / 3.65 s |
| **save, median / history-0 save** | **1.02x** (target 1.5x) |
| save growth, last / first median | 0.96x |
| `status` (clean) | 2.15 s |
| `history` / `history --file` | 30 / 41 ms |
| `restore` to mid / back to tip | 3.3 s / 2.3 s |
| `gc` / `fsck` | 6.3 s / 17.6 s |
| open (`velo branches`) | 38 ms |
| `.velo` / `velo.db` | 137 MiB / 120 MiB |
| `file_map` rows | 400,004 |

This is the one 6.x measurement of a 100,000-file repository over hundreds of
snapshots on Windows: the one-file save is flat against history-0 over 500
snapshots. It is one run under Defender.

### Linux (GitHub runner), velo 6.2.1: 100,000 files x 500 snapshots (full, ratio shape)

| Metric | 100,000 files x 500 snapshots |
| --- | --- |
| first save (every file new) | 7.6 s |
| save, history-0 median | 0.75 s |
| save, first 10% median / p95 | 0.77 / 0.87 s |
| save, last 10% median / p95 | 0.69 / 0.71 s |
| **save, median / history-0 save** | **0.92x** (target 1.5x) |
| save growth, last / first median | 0.91x |
| `status` (clean) | 0.47 s |
| `history` / `history --file` | 7 / 17 ms |
| `restore` to mid / back to tip | 0.98 s / 0.98 s |
| `gc` / `fsck` | 1.6 s / 12.4 s |
| open (`velo branches`) | 3.5 ms |
| `.velo` / `velo.db` | 137 MiB / 121 MiB |
| `file_map` rows | 400,004 |

One run on a hosted runner (AMD EPYC 9V74, 4 logical cores), dispatched from
`main` at `6114905`.

### Linux (GitHub runner), velo 6.2.1: retention, 10,000 files x 2,000 snapshots (full)

The `default_policy_at_ten_thousand_files_and_two_thousand_snapshots` test. It
builds its 2,000 snapshots with whole trees (the 18.10 shape: 20 million
`file_map` rows), so it is the worst case for the delete stage, not the K = 128
default.

| Stage | Time |
| --- | --- |
| build the history | 1,879 s |
| `retention::apply` (default policy; examined 2,000, removed 1,240) | 175 s |
| `gc` | 2.8 s |
| `gc::vacuum` (incremental) | 115 s |
| snapshots / `file_map` rows after | 760 / 7,600,000 |
| `velo.db` after retention and `gc` / after vacuum | 5.56 GB (3.34 GB on the freelist) / 2.21 GB |

For comparison, the pre-v4 run on Windows (19.4) took 416 s for the apply, 165 s
for `gc` and 193 s for the vacuum. Different platforms, so read it as the order
of magnitude, not a ratio.

### Windows 11, velo 6.2.2: retention, 10,000 files x 2,000 snapshots (full)

The same test, run 2026-10-26 on the Windows machine from a build of the 6.2.2
branch (the retention code is unchanged since 6.2.0), detached
with no time limit, Defender on with no exclusions.

| Stage | Time |
| --- | --- |
| build the history | 4,490 s |
| `retention::apply` (examined 2,000, removed 1,240) | 1,323 s |
| of which `compact.delete` / `compact.commit` / `compact.insert` | 940 s / 225 s / 142 s |
| `gc` | 396 s |
| `gc::vacuum` (incremental) | 425 s |
| `velo.db` before / after retention / after vacuum | 5.56 GB / 5.56 GB (3.34 GB on the freelist) / 2.21 GB |
| snapshots / `file_map` rows after | 760 / 7,600,000 |

The size outcome is the same as on Linux (5.56 GB to 2.21 GB, 760 snapshots), but
the times are 7.6x the Linux runner for the apply and 3.2x the pre-v4 Windows run
(1,323 s against 416 s), and 94% of the apply is the delete and its commit. This
machine was also running other work for most of the two hours (agent sessions and
their Rust builds), so the run overstates what an idle machine would take, and one
run cannot say by how much; it is recorded as measured, not as a figure to quote.
The test builds its history with whole trees, so it is the worst case for the delete
stage and says nothing about the K = 128 default.

### Linux (GitHub runner), velo 6.2.0: `envelope-a`, 10,000 files x 2,000 snapshots (full)

| Metric | 10,000 files x 2,000 snapshots |
| --- | --- |
| first save (every file new) | 0.52 s |
| save, first 10% median / p95 | 47 / 72 ms |
| save, last 10% median / p95 | 44 / 50 ms |
| save growth, last / first median | 0.94x |
| `status` (clean) | 35 ms |
| `history` / `history --file` | 13 / 37 ms |
| `restore` to snapshot 1,000 / back to tip | 109 / 113 ms |
| `gc` / `fsck` | 0.17 s / 3.2 s |
| open (`velo branches`) | 2.9 ms |
| `.velo` / `velo.db` | 47.5 MiB / 45.6 MiB |
| `file_map` rows | 160,016 |

### Linux (GitHub runner), velo 6.2.0: `envelope-b`, 100,000 files x 200 snapshots (full)

| Metric | 100,000 files x 200 snapshots |
| --- | --- |
| first save (every file new) | 8.2 s |
| save, first 10% median / p95 | 0.61 / 0.68 s |
| save, last 10% median / p95 | 0.56 / 0.57 s |
| save growth, last / first median | 0.91x |
| `status` (clean) | 0.39 s |
| `history` / `history --file` | 4 / 8 ms |
| `restore` to snapshot 100 / back to tip | 0.78 / 0.82 s |
| `gc` / `fsck` | 1.3 s / 5.7 s |
| open (`velo branches`) | 2.9 ms |
| `.velo` / `velo.db` | 83.0 MiB / 66.7 MiB |
| `file_map` rows | 200,002 |

These are not comparable cell for cell with the Windows tables: the
platforms, filesystems and scanning differ, and the Linux first saves (0.5 s,
8 s) against Windows (55 s, 589 s) mostly show what Defender costs, not what
velo costs. The saves stay flat over both full envelope shapes on Linux.

### Not run on 6.x

A shape or platform below has no 6.x number and none is implied. Every full shape
now has one on Windows and on the Linux runner.

- **Linux on a local machine or WSL2** (as opposed to a hosted runner): not
  run; the runner tables above are the only Linux data.

## 5.x (format v3): the envelope


**On 5.x, tested to 10,000 files x 2,000 snapshots and to 100,000 files x 200
snapshots: 20 million `file_map` rows (a 5.3 GiB `velo.db`) in both cases, on
Windows 11 and on Linux, one file changed per snapshot.**

Inside that envelope every command completed. Beyond it nothing is claimed:
velo has not been measured above 20 million rows, 100,000 files or 2,000
snapshots. The limits that show up inside it are below.

## 5.x (format v3): results

Every number is a run of the real release CLI, so it includes process start,
database open and disk I/O. Each cell has two runs side by side: **5.2.0**
(2026-10-15, the first publication of this page) and **5.3.0** (2026-10-17, the
Phase 19 re-run: same `bench/scale/scale.py` presets, same seed, same machine,
no change to the harness). One run per cell, one file changed per snapshot, each
file about 200 bytes, so these are metadata and scanning costs, not
large-content costs. The per-command rows are the median of 3 runs (5 for open).
Raw data, including every save latency, is in
[`bench/results/`](../bench/results/): `2026-10-15-*` is 5.2.0, `2026-10-17-*` is
5.3.0.

Machine for both platforms: Intel Core i7-1370P, 20 logical CPUs. The
Linux runs are WSL2 (Ubuntu 22.04, kernel 5.15, ext4 inside the WSL2 virtual
disk) on the same laptop, not a separate Linux box, so the two columns compare
filesystems and scanners on identical hardware, not different hardware. The 5.3.0
Windows runs had Defender real-time protection on with no exclusions, as before.
The two days are not a controlled comparison of code: the same machine gave
shadow git a different time on the two days with no code change (see the
cookbook), and no repeat was run to separate the code from the day.

### 5.x (format v3): 10,000 files x 2,000 snapshots (`envelope-a`)

| Metric | Win 5.2.0 | Win 5.3.0 | Linux 5.2.0 | Linux 5.3.0 |
| --- | --- | --- | --- | --- |
| first save (every file new) | 49.6 s | 31.5 s | 0.68 s | 0.78 s |
| save, first 10% median / p95 | 1,208 / 2,456 ms | 1,407 / 2,076 ms | 861 / 1,832 ms | 763 / 1,498 ms |
| save, last 10% median / p95 | 1,854 / 2,111 ms | 2,249 / 2,881 ms | 1,976 / 2,835 ms | 1,473 / 2,345 ms |
| **save growth, last / first median** | 1.53x | **1.60x** | 2.30x | **1.93x** |
| `status` (clean) | 180 ms | 117 ms | 101 ms | 47 ms |
| `history` (all 2,000) | 50 ms | 39 ms | 28 ms | 13 ms |
| `history --file` | 111 ms | 94 ms | 74 ms | 42 ms |
| `restore` to snapshot 1,000 / back to tip | 965 / 508 ms | 479 / 885 ms | 329 / 277 ms | 222 / 208 ms |
| `gc` | 6.5 s | 6.5 s | 5.1 s | 3.2 s |
| `fsck` | 31.3 s | 29.3 s | 25.6 s | 17.5 s |
| open (`velo branches`) | 36 ms | 29 ms | 3 ms | 4 ms |
| `velo.db` | 5,338.6 MiB | 5,343.7 MiB | 5,353.9 MiB | 5,354.8 MiB |
| `file_map` rows | 20,002,000 | 20,002,000 | 20,002,000 | 20,002,000 |

### 5.x (format v3): 100,000 files x 200 snapshots (`envelope-b`)

| Metric | Win 5.2.0 | Win 5.3.0 | Linux 5.2.0 | Linux 5.3.0 |
| --- | --- | --- | --- | --- |
| first save (every file new) | 519.9 s | 319.4 s | 21.4 s | 7.5 s |
| save, first 10% median / p95 | 3,621 / 7,081 ms | 2,818 / 4,260 ms | 2,971 / 4,493 ms | 2,941 / 4,983 ms |
| save, last 10% median / p95 | 14,470 / 16,551 ms | 16,324 / 19,195 ms | 20,187 / 21,821 ms | 12,562 / 15,688 ms |
| **save growth, last / first median** | 4.00x | **5.79x** | 6.80x | **4.27x** |
| `status` (clean) | 2,406 ms | 880 ms | 720 ms | 341 ms |
| `history` (all 200) | 28 ms | 24 ms | 7 ms | 6 ms |
| `history --file` | 44 ms | 40 ms | 13 ms | 9 ms |
| `restore` to snapshot 100 / back to tip | 2,720 / 2,751 ms | 1,213 / 1,246 ms | 1,280 / 1,298 ms | 776 / 754 ms |
| `gc` | 392.3 s | 149.8 s | 8.2 s | 4.8 s |
| `fsck` | 127.5 s | 36.7 s | 37.6 s | 25.2 s |
| open (`velo branches`) | 96 ms | 30 ms | 3 ms | 3 ms |
| `velo.db` | 5,329.2 MiB | 5,344.3 MiB | 5,350.9 MiB | 5,350.6 MiB |
| `file_map` rows | 20,000,200 | 20,000,200 | 20,000,200 | 20,000,200 |

**Save growth is not fixed.** One changed file at 100,000 files still costs 2.8 s
(Windows) and 2.9 s (Linux) over the first 20 saves and 16.3 s and 12.6 s over the
last 20: 5.79x and 4.27x. That is lower than 5.2.0 on Linux (6.80x) and higher on
Windows (4.00x), but the movement is in the first-10% baseline as much as in the
last-10% time, and one run per cell cannot tell it from day-to-day variation.
Phase 19 changed what a step costs the caller (`save_changes`, `materialize`,
retention, vacuum) but not how a save writes its rows, so a save still writes
one `file_map` row per file and still grows with the table. At 10,000 files the
growth is 1.60x (Windows) and 1.93x (Linux) and the save settles near 2.2 s and
1.5 s. The targets this feeds are in [`PERF.md`](PERF.md) section 6.

Reading them:

- **Reading history is cheap everywhere.** `history`, `history --file` and open
  stay under 120 ms at 20 million rows in both runs. `restore` is a second or three, in
  proportion to the files it rewrites.
- **Save is the cost, and it grows with the history.** At 10,000 files a save
  settles at about 1.5-2.2 s after the first few hundred snapshots and stays flat
  to 2,000. At 100,000 files it keeps climbing, from 3-4 s to 13-20 s over 200
  snapshots, and had not flattened when the run ended. Each save writes one
  row per file (100,000 rows for a one-file change) into a database that keeps
  getting larger. The runs measured that latency grows with the table; they did
  not isolate which part of SQLite's work (page cache, index maintenance, the
  WAL) is responsible.
- **The first save of a large tree is a scan-and-hash of every file.** On
  Windows it took 520 s (5.2.0) and 319 s (5.3.0) for 100,000 files against 21 s
  and 7.5 s on Linux, and the same
  gap shows in `gc` (392 s and 150 s against 8 s and 5 s) and in clean `status`. That gap is
  per-file open cost with Defender scanning on, not the database. See the
  caveats.
- **`fsck` and `gc` read every row.** At 20 million rows `fsck` is 18-38 s on
  Linux and 29-127 s on Windows. Treat it as a maintenance command, not a
  prompt.

## Where it degrades, and why

Rows in `file_map` grow as files x snapshots. Everything that is not a point
lookup either reads or writes in proportion to that:

| What grows | Where it showed | Consequence |
| --- | --- | --- |
| rows written per save = files in the tree | `save` at 100,000 files: 2.8-3.6 s rising to 13-20 s | at 100,000 files a save takes 13-20 s once the history is 200 snapshots long |
| total rows = files x snapshots | `fsck` 18-127 s, `gc` 3-392 s, `velo.db` 5.3 GiB | maintenance commands take minutes at 20 million rows |
| per-file open cost | Windows first save, `gc`, `status` at 100,000 files | Defender scanning (see caveats); not a velo limit |

What stays fast: `history`, `history --file`, open, and `restore` of a tree
(proportional to the files in one tree, not to the history). Storage costs about
290 bytes per `file_map` row, so 20 million rows is 5.3 GiB however small the
files are.

Two things keep the cost down without changing the format:

1. **Retention** (next section) removes whole snapshots and their rows.
2. **Carry-forward** (`TreeEntry::stored` in `WriteGuard::save_tree`, built from
   the previous tree) avoids re-reading and re-hashing files, which is where
   first-save cost goes. It does not reduce rows.

## Retention for agent checkpointing

An agent that snapshots after every tool call produces thousands of
near-identical snapshots. The recommended policy is the default of
`velo_core::commands::retention::Policy::default()`:

- every snapshot for the last day,
- one per hour for the last 30 days,
- one per day after that.

`velo retention` applies it (`--dry-run` first to see what would go), then runs
`gc` and an incremental vacuum. Run it on a schedule (daily is enough). An
embedder calls `retention::apply`, `gc::run` and `gc::vacuum`.

**Measured** (`crates/velo-core/src/tests/retention_scale.rs`, an ignored
test): a 10,000-file tree and 2,000 snapshots spread evenly over 60 synthetic
days (one about every 43 minutes, three files changed per snapshot), saved with
`save_tree` and explicit timestamps, then `retention::apply` with the default
policy at day 60, then `gc`, then `gc::vacuum(Incremental)`. Windows 11,
release build, Defender on. The 18.10 column is the first run, before
compaction was re-minted in place and before the vacuum existed; the 19.4
column is the repeat (raw output in
[`bench/results/2026-10-16-windows-retention-scale.txt`](../bench/results/2026-10-16-windows-retention-scale.txt)).

| | Before | 18.10: after retention and `gc` | 19.4: after retention, `gc` and vacuum |
| --- | --- | --- | --- |
| snapshots | 2,000 | 760 (-62%) | 760 (-62%) |
| `file_map` rows | 20,000,000 | 7,600,000 (-62%) | 7,600,000 (-62%) |
| live database content | 5.17 GiB | 1.99 GiB (-62%) | 1.99 GiB (-62%) |
| `velo.db` file size | 5.17 GiB | 7.14 GiB | 2.06 GiB (-60%) |
| `retention::apply` | | 1,219.5 s | 415.6 s |
| `gc` | | 226.1 s | 165.2 s |
| vacuum | | none | 192.5 s |
| building the 2,000 snapshots | 3,808 s | | 3,751 s |

The file is smaller after than before: 5,559,283,712 bytes to 2,211,880,960.
Neither `retention::apply` nor `gc` shrinks it on its own (the file stays at
5.18 GiB with 3.11 GiB on the freelist); the vacuum is what returns the pages,
and leaves no free pages behind.

Stage breakdown of the 415.6 s in `retention::apply` (the `timing` feature's
laps):

| Stage | Time |
| --- | --- |
| `retention.chain`, `.dependants`, `.classify` | 0.05 s |
| `compact.chain`, `.eligibility`, `.refs` | 0.07 s |
| `compact.plan` | 8.2 s |
| `compact.delete` | 246.3 s |
| `compact.insert` | 39.4 s |
| `compact.commit` | 121.5 s |

Deleting the dropped snapshots' `file_map` rows (6.1 of the 6.9 minutes, with the
commit that makes them durable) dominates; choosing what to drop is
negligible. `gc` has no stage laps. An earlier run of the same code with a
`gc::vacuum` that freed one page instead of all of them is kept as
`bench/results/2026-10-16-windows-retention-scale-vacuum-single-step.txt`; it
is why the first run left the file at 5.18 GiB.

Only the Windows run of the retention test is published. The Linux (WSL2) run
was not repeated for this release, so there are still no Linux retention
numbers; the row and snapshot counts are properties of the policy and do not
depend on the platform, the timings do.

What to take from it:

- **Retention bounds the rows.** The policy keeps everything from the last day
  and at most 24 per day for the 29 days after, then one a day, so the retained
  history is set by the policy, not by how long the agent has run. Rows fall in
  step with snapshots, because every snapshot owns its whole tree.
- **The database file shrinks, on repositories created from 5.3.0.** They use
  incremental auto-vacuum, and `velo retention` (or `velo gc --vacuum`) returns
  the freed pages to the operating system: 5.18 GiB to 2.06 GiB here. A
  repository created before 5.3.0 is not incremental; run one
  `velo gc --vacuum` on it to convert it (a full `VACUUM`, which needs free
  disk about the size of `velo.db` and holds the write lock), and later passes
  are cheap. Until then retention bounds growth but the file keeps its size.
- **Run it early and often.** Applying the policy once to 20 million rows took
  about 7 minutes, plus 3 for the vacuum. Run daily, a pass thins about 30
  snapshots and costs a small fraction of that. Do not let 2,000 snapshots of a
  10,000-file tree pile up before the first pass.

## Repacking an existing repository

A repository written before deltas stores every snapshot's whole tree, and
migrating it to the current layout does not shrink it, because migration
rewrites nothing. `velo repack` does the rewrite: it converts those full
snapshots into deltas a few at a time, each batch in its own short transaction,
so another `velo save` waits only a moment. It changes no snapshot id and
rewrites no history, and it can be interrupted and run again to finish.

```
velo repack --dry-run   # what it would convert, with an estimated saving
velo repack
```

The dry run's byte figure is an estimate, a lower bound on the saving, because
it ignores index pages and page slack. The space is returned to the operating
system by a vacuum at the end of a real run. The same operation is available as
`Store.repack(dry_run=False)` in Python, `repack(dryRun?)` in Node and the
`velo_repack` MCP tool. 

Measured on Windows (NTFS, Defender on) on a reduced envelope-style repository:
10,000 files by 150 snapshots, one file changed per snapshot, written by 5.3.0
(`scale.py --files 10000 --snapshots 150`), so every snapshot held its whole
tree: 1,500,150 rows, 398.7 MiB of `velo.db`. A copy was made before 6.0.0
touched it.

| step | time | result |
|---|---:|---|
| migrate (first open by 6.0.0) | 165 ms | nothing rewritten, size unchanged |
| `repack --dry-run` | 9.0 s | 148 snapshots to convert, estimated 398.7 MB to 270.3 MB (a lower bound) |
| `repack` | 41.2 s | 1,500,150 rows to 20,150; 398.7 MB to 7.5 MB; `.velo` 419.8 MB to 9.6 MB |
| `fsck` after | 1.0 s | healthy |

The dry run's estimate (270 MB) is far below what the real run returned, as it
says it is: the real figure includes the index pages and slack the vacuum
reclaims. Ids and trees were verified against the pre-migration copy: all 150
snapshot ids on `main` are identical, `tree_at` of every snapshot matches path
by path (1,500,150 entries), and `velo fsck` is healthy on both
([`2026-10-20-windows-repack.json`](../bench/results/2026-10-20-windows-repack.json)).
This is a reduced shape; the 5.18 GiB envelope repository (10,000 by 2,000) was
not repacked in this run.

## Commands for the owner

Run from the repository root with `cargo build --release -p velo-cli` done.
Each full run takes hours on Windows. `scale.py` has no time limit of its own
(`--command-timeout SECONDS` is optional and bounds one velo command), and it
rewrites `--out` after every phase with `"partial": true` and the
`completed_phases`, so a killed run keeps what it measured. Run it detached,
not under a time-limited tool:

```text
# Windows: a separate terminal, or detached from this one
Start-Process -NoNewWindow -FilePath python -ArgumentList 'bench/scale/scale.py','--preset','ratio-100k','--out','bench/results/<date>-windows-ratio-100k.json' -RedirectStandardError scale.log

# Linux
nohup python3 bench/scale/scale.py --preset ratio-100k --out bench/results/<date>-linux-ratio-100k.json > scale.out 2> scale.log &
```

### Detached full runs

```text
# Windows, envelope-a (10,000 x 2,000)
Start-Process -NoNewWindow -FilePath python -ArgumentList 'bench/scale/scale.py','--preset','envelope-a','--out','bench/results/<date>-windows-envelope-v4-a-full.json' -RedirectStandardError scale-a.log

# Windows, envelope-b (100,000 x 200)
Start-Process -NoNewWindow -FilePath python -ArgumentList 'bench/scale/scale.py','--preset','envelope-b','--out','bench/results/<date>-windows-envelope-v4-b-full.json' -RedirectStandardError scale-b.log

# Windows, full retention run
Start-Process -NoNewWindow -FilePath cargo -ArgumentList 'test','--release','-p','velo-core','--features','timing','--lib','default_policy_at_ten_thousand_files_and_two_thousand_snapshots','--','--ignored','--nocapture' -RedirectStandardOutput retention.out -RedirectStandardError retention.log

# Linux, full retention run
nohup cargo test --release -p velo-core --features timing --lib default_policy_at_ten_thousand_files_and_two_thousand_snapshots -- --ignored --nocapture > retention.out 2> retention.log &
```

The foreground forms below are for reference; do not run the full shapes
under a time-limited tool.

```text
# Full envelope, Windows (Defender on; say so with --defender-note if excluded)
python bench/scale/scale.py --preset envelope-a --out bench/results/<date>-windows-envelope-v4-a-full.json
python bench/scale/scale.py --preset envelope-b --out bench/results/<date>-windows-envelope-v4-b-full.json

# Full envelope, Linux (WSL2 from an ext4 path, or a Linux box)
python3 bench/scale/scale.py --velo target/release/velo --preset envelope-a --out bench/results/<date>-linux-envelope-v4-a-full.json
python3 bench/scale/scale.py --velo target/release/velo --preset envelope-b --out bench/results/<date>-linux-envelope-v4-b-full.json

# 100,000 files x 500 snapshots: reports save_ratio_vs_history0 (target 1.5x)
python bench/scale/scale.py --preset ratio-100k --out bench/results/<date>-windows-ratio-100k.json

# Reduced 100,000-file shape that did not finish here; run it detached
python bench/scale/scale.py --files 100000 --snapshots 20 --out bench/results/<date>-windows-envelope-v4-b-reduced.json

# Full retention run (10,000 files x 2,000 snapshots, the 18.10/19.4 shape)
cargo test --release -p velo-core --features timing --lib default_policy_at_ten_thousand_files_and_two_thousand_snapshots -- --ignored --nocapture

# Reduced retention run (2,000 files x 400 snapshots)
cargo test --release -p velo-core --features timing --lib default_policy_at_a_reduced_18_10_shape -- --ignored --nocapture
```

The test lives in `crates/velo-core/src/tests/retention_scale.rs`.

Migrate and repack the 5.18 GiB envelope repository, then compare ids and
trees with a copy taken before (copy first; this is the 5.x-written
repository from the 2026-10-16 retention run):

```text
cp -r <envelope repo> <envelope repo>-copy
cd <envelope repo>
time velo branches            # first open by the new engine: the migration
time velo repack --dry-run
time velo repack
velo fsck
cd <envelope repo>-copy && velo fsck
```

Then open both with the Python binding and compare `history(branch="main")`
ids and `tree_at(id)` for every snapshot; they must be identical. Record the
migration time, the repack time and the `.velo` size before and after. The same
steps are in `bench/README.md`.

## Method

`bench/scale/scale.py` drives the release CLI and uses only the Python standard
library. For N files of about 200 bytes in directories of about 100 files, it
runs `velo init`, one first `save`, then M-1 saves that each rewrite one file
chosen by a seeded RNG (seed 1). It records every save latency, then the
`.velo` and `velo.db` sizes and the `file_map` row count (read-only `sqlite3`),
then the median of 3 runs of `status`, `history`, `history --file`, `restore`,
`gc` and `fsck`, and of 5 runs of `velo branches` for open time. Full method:
[`bench/README.md`](../bench/README.md).

Build and run:

```
cargo build --release -p velo-cli --locked
python bench/scale/scale.py --preset envelope-a --out a.json   # 10,000 x 2,000
python bench/scale/scale.py --preset envelope-b --out b.json   # 100,000 x 200
```

Both take hours. On the laptop above, `envelope-a` took about 1 hour 10 minutes
and `envelope-b` about 2 hours 15 minutes on Windows in the 5.2.0 run; the 5.3.0
run took about 1 hour 10 minutes and about 1 hour 15 minutes on Windows, and
about 50 minutes and 35 minutes on Linux. The `bench` workflow
(`.github/workflows/bench.yml`, manual dispatch) runs the same presets on a
GitHub-hosted Linux runner. Retention:

```
cargo test -p velo-core --release retention_scale -- --ignored --nocapture
```

The retention test builds 2,000 snapshots of a 10,000-file tree: about 1 hour
30 minutes on Windows, and it needs about 8 GiB of temporary disk.

Results are committed as `bench/results/<date>-<platform>-<preset>.json`:
`2026-10-15-*-envelope-{a,b}.json` (5.2.0) and `2026-10-17-*-envelope-{a,b}.json`
(5.3.0, for Windows and Linux), with the retention output as `<date>-<platform>-retention-scale.txt`.

## Caveats

- **Windows Defender.** The Windows runs had Defender real-time protection on,
  with no exclusions, and the work directory under `%TEMP%`. Defender scans
  files as they are opened and written, and velo writes one object file per new
  file, so file-heavy steps (the first save, `gc`, clean `status` at 100,000
  files) are many times slower than on Linux: 520 s against 21 s for the first
  100,000-file save. Excluding the work directory from real-time scanning is
  expected to remove most of that gap; that was not measured here. See [WINDOWS.md](WINDOWS.md) for the measured Windows numbers and settings. The
  committed Windows numbers are the default configuration an adopter sees, not
  the best case. Each JSON records the Defender state in `env.defender_note`.
- **One machine, one run per cell.** The Linux runs are WSL2 on the same laptop
  as the Windows runs, not native Linux and not a separate machine. There is no
  repeat to give a confidence interval; the p95 columns show the spread within
  a run.
- **Small files.** About 200 bytes each, so content hashing and compression are
  negligible and the numbers describe metadata scale. Large files add their own
  hashing and write cost on top.
- **One changed file per snapshot** in the benchmark and three in the retention
  test. The rows written are the same either way: one per file in the tree.
