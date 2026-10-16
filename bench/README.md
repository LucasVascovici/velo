# Benchmarks

## Scale benchmark (`bench/scale/scale.py`)

Measures velo at large file counts and long histories. It drives the real
release CLI (`velo`), so every number includes process start, database open and
disk I/O. Python 3, standard library only.

### Method

1. Generate N files of about 200 bytes in nested directories of about 100 files
   each, in a work directory outside the repository checkout.
2. `velo init` (`--objects files|database` picks the object backend, default
   files, recorded in the result `env`), one first `save`, then M-1 further saves. Each rewrites K files
   (`--change`, default 1) picked by a seeded RNG (`--seed`, default 1).
3. Record every save latency; report the median and p95 of the first and last
   10% of saves, and their ratio (latency growth).
4. Record `.velo` total size, `velo.db` size, and the `file_map` row count
   (read-only `sqlite3`).
5. Record the median of 3 runs of: `status` (clean), `history`, `history --file`,
   `restore` to snapshot M/2 and back to the tip, `gc`, `fsck`.
6. Record open time as the median of 5 runs of `velo branches`.
7. Record the platform, CPU, `velo --version`, work path and, on Windows, the
   `--defender-note` text.

The output is a JSON file (`--out`, default `results.json`, includes every save
latency) and a markdown table on stdout.

### Reproduce

Build first: `cargo build --release -p velo-cli --locked`.

| Preset | Size | Command | Expected time |
|---|---|---|---|
| `small` | 1,000 files x 50 | `python bench/scale/scale.py --preset small` | under a minute |
| `envelope-a` | 10,000 files x 2,000 | `python bench/scale/scale.py --preset envelope-a` | hours (2,000 saves) |
| `envelope-b` | 100,000 files x 200 | `python bench/scale/scale.py --preset envelope-b` | hours (each save scans 100,000 files) |

Other options: `--velo PATH`, `--files`, `--snapshots`, `--change`, `--workdir DIR`
(never inside the checkout), `--keep`, `--out`.

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
