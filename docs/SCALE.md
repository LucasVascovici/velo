# Scale: how far velo has been tested

velo keeps every snapshot's whole file list in the `file_map` table, so its
metadata grows as files x snapshots even when one file changes per snapshot
(see [FORMAT.md](FORMAT.md) section 11.4). This page says how far that has been
measured, how fast velo is there, where it degrades, and what to do about it.

## The envelope

**Tested to 10,000 files x 2,000 snapshots and to 100,000 files x 200
snapshots: 20 million `file_map` rows (a 5.3 GiB `velo.db`) in both cases, on
Windows 11 and on Linux, one file changed per snapshot.**

Inside that envelope every command completed. Beyond it nothing is claimed:
velo has not been measured above 20 million rows, 100,000 files or 2,000
snapshots. The limits that show up inside it are below.

## Results

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

### 10,000 files x 2,000 snapshots (`envelope-a`)

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

### 100,000 files x 200 snapshots (`envelope-b`)

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
