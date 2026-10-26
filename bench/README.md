# Benchmarks

## Scale benchmark (`bench/scale/scale.py`)

Measures velody at large file counts and long histories. It drives the real
release CLI (`velody`), so every number includes process start, database open and
disk I/O. Python 3, standard library only.

### Method

1. Generate N files of about 200 bytes in nested directories of about 100 files
   each, in a work directory outside the repository checkout.
2. `velody init` (`--objects files|database` picks the object backend, default
   files, recorded in the result `env`), one first `save`, then M-1 further saves. Each rewrites K files
   (`--change`, default 1) picked by a seeded RNG (`--seed`, default 1).
3. Record every save latency; report the median and p95 of the first and last
   10% of saves, and their ratio (latency growth).
4. Record `.velo` total size, `velo.db` size, and the `file_map` row count
   (read-only `sqlite3`).
5. Record the median of 3 runs of: `status` (clean), `history`, `history --file`,
   `restore` to snapshot M/2 and back to the tip, `gc`, `fsck`.
6. Record open time as the median of 5 runs of `velody branches`.
7. Record the platform, CPU, `velody --version`, work path and, on Windows, the
   `--defender-note` text.

The output is a JSON file (`--out`, default `results.json`, includes every save
latency) and a markdown table on stdout.

### Reproduce

Build first: `cargo build --release -p velody-cli --locked`.

| Preset | Size | Command | Expected time |
|---|---|---|---|
| `small` | 1,000 files x 50 | `python bench/scale/scale.py --preset small` | under a minute |
| `envelope-a` | 10,000 files x 2,000 | `python bench/scale/scale.py --preset envelope-a` | hours (2,000 saves) |
| `envelope-b` | 100,000 files x 200 | `python bench/scale/scale.py --preset envelope-b` | hours (each save scans 100,000 files) |
| `ratio-100k` | 100,000 files x 500 | `python bench/scale/scale.py --preset ratio-100k` | many hours; reports `save_ratio_vs_history0` |

Other options: `--velody PATH`, `--files`, `--snapshots`, `--change`, `--workdir DIR`
(never inside the checkout), `--keep`, `--out`, `--command-timeout SECONDS`
(default: no limit).

`scale.py` rewrites `--out` after every phase with `"partial": true` and
`completed_phases`, and `"partial": false` at the end, so a killed run keeps
what it measured. Run long presets detached: on Windows in a separate terminal
or with `Start-Process -NoNewWindow`, on Linux with `nohup python3 ... &`.
The result carries a `machine` block (OS, platform, CPU, logical cores,
Defender note), `save_history0_median_s` (median of the first 10 one-file saves
after the initial full save) and `save_ratio_vs_history0` (median of the last
10% of saves over that; the target is 1.5x).

On GitHub, run the `bench` workflow (manual dispatch, Ubuntu) with a `preset`
input; it uploads `results.json` as an artifact. It never runs on push or PR.

### Windows Defender caveat

Real-time scanning inflates every small-file number on Windows, sometimes by
a large factor, and adds heavy tail latency (first saves are the worst). Either
exclude the work directory from real-time scanning and say so, or leave it on
and say so, using `--defender-note "<what you checked>"`. Without the flag the
harness records that scanning is assumed to be on. Always also run the Linux
number; do not compare a Windows figure with a Linux one without the caveat.

### Rule for quoting numbers

A number quoted in docs must come from this harness, and the `results.json`
that produced it must be published alongside it (with the commit and the
platform it was measured on).

## Profile (stage timers)

Where `save`, `save_tree` and `restore` spend their time, per named stage, at a
chosen tree size and history depth, plus the agent workload driven straight at
`velody-core`. The results and what they mean are in
[`docs/PERF.md`](../docs/PERF.md). These are `#[ignore]`d tests in `velody-core`,
built in release mode with the `timing` feature (the stage timers are compiled
out otherwise).

### Reproduce

One cell of the stage tables (here 10,000 files, 500 snapshots, bulk history):

```text
VELODY_PROFILE_FILES=10000 VELODY_PROFILE_HISTORY=500 VELODY_PROFILE_BULK=1 VELODY_PROFILE_OUT=perf.json cargo test -p velody-core --release --features timing profile_save_restore     -- --ignored --nocapture --test-threads=1
```

The agent workload three ways (`as_harness`, `no_reread`, `save_tree_only`) and
the SQLite option experiment (100,000 files x 200 snapshots):

```text
cargo test -p velody-core --release --features timing profile_agent_workload     -- --ignored --nocapture --test-threads=1
VELODY_PROFILE_BULK=1 cargo test -p velody-core --release --features timing     sqlite_options_one_file_save -- --ignored --nocapture --test-threads=1
```

The binding's side of the agent comparison (build the Python binding in release
first, in a virtual environment):

```text
maturin develop --release --manifest-path bindings/python/Cargo.toml
python bench/agent/agent_bench.py --only velody --out agent-velody-only.json
```

Knobs: `VELODY_PROFILE_FILES` (1000), `VELODY_PROFILE_HISTORY` (0),
`VELODY_PROFILE_SAMPLES` (10; 5 for the SQLite experiment), `VELODY_PROFILE_BULK`
(0), `VELODY_PROFILE_DIR` (the system temp directory; put it on the filesystem you
mean to measure), `VELODY_PROFILE_STEPS` (500) and `VELODY_PROFILE_OUT` (append the
JSON lines to this file). Each op prints one JSON object to stderr. Run with
`--test-threads=1`.

`VELODY_PROFILE_BULK=1` writes the history rows directly instead of saving, which
reaches large table sizes in hours instead of weeks; the ids it makes do not
verify (`fsck` fails), so it is for measuring only, and a result from it must be
labelled bulk. A 100,000 x 500 cell takes about two hours to build and its read
loop takes about 40 minutes per sample on Windows with Defender on, so lower
`VELODY_PROFILE_SAMPLES` for the 100,000-file cells and say so. Plan for disk: a
history of N snapshots over F files is about N x F x 290 bytes (100,000 x 2,000
is about 55 GiB).

The Windows Defender caveat above applies unchanged; say whether the work
directory was excluded. The committed results are
`bench/results/perf-<date>-windows.json` and `perf-<date>-linux.json` (JSON
lines, one per op, as printed).

## For the owner

Runs the Phase 20 re-run (task 20.9-b) did not make, because they take hours.
Run each one detached (see above); none needs a time limit. Build first: `cargo build --release -p velody-cli --locked`. Say whether Defender
was excluded (`--defender-note`), and publish the JSON under `bench/results/`.

1. Envelope A (10,000 files x 2,000 snapshots, hours), Windows and then Linux:

   ```text
   python bench/scale/scale.py --preset envelope-a --keep --workdir <dir outside the checkout> --out bench/results/<date>-windows-envelope-a.json
   ```

2. Envelope B (100,000 files x 200 snapshots, hours):

   ```text
   python bench/scale/scale.py --preset envelope-b --out bench/results/<date>-windows-envelope-b.json
   ```

   The 100,000 x 500 ratio run:

   ```text
   python bench/scale/scale.py --preset ratio-100k --out bench/results/<date>-windows-ratio-100k.json
   ```

   The full retention test (10,000 files x 2,000 snapshots):

   ```text
   cargo test --release -p velody-core --features timing --lib default_policy_at_ten_thousand_files_and_two_thousand_snapshots -- --ignored --nocapture
   ```

3. Migrate and repack the real 5.18 GiB envelope repository (the 2026-10-16 retention
   run's repository, written by 5.3.0 or earlier). Copy it first, then check
   ids and trees against the copy:

   ```text
   cp -r <envelope repo> <envelope repo>-copy
   cd <envelope repo>
   time velody branches            # first open by the new engine: the migration
   time velody repack --dry-run
   time velody repack
   velody fsck
   cd <envelope repo>-copy && velody fsck
   ```

   For the tree comparison, open both with the Python binding and compare
   `history(branch="main")` ids and `tree_at(id)` for each snapshot (the
   script used for the reduced run is described in
   `bench/results/2026-10-20-windows-repack.json`). Record migration time,
   repack time, and the size of `.velo` before and after.

4. Agent benchmark on Linux/WSL2 (500 steps, run from an ext4 path), and the
   binding share of the Python step: `python bench/agent/agent_bench.py --only velody --out <file>`
   and `--only "shadow git"` as two runs, since the full run with the 5.2-API
   row takes over 10 minutes on Windows.
