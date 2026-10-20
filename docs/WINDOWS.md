# Velo on Windows

The first save of a large tree is much slower on Windows than on Linux. This
page gives the measured cost, why it happens, and the settings that change it.

## The numbers

From the 18.10 scale run (same laptop, one file changed per snapshot after the
first save):

| Files | First save, Windows | First save, Linux |
|---|---|---|
| 10,000 | 49.6 s | 0.68 s |
| 100,000 | 520 s | 21 s |

In that run `gc` at 100,000 files took 392 s on Windows against 8 s on Linux.

Since then, 19.6-a made a save open each new file once instead of several
times, and 19.6-b added `velo init --objects database`. Both were measured on
Windows 11 (NTFS), release build, Defender real-time protection on with no
exclusions, 5 snapshots, one file changed per snapshot, with
`bench/scale/scale.py`. The raw results are in
`bench/results/2026-10-16-windows-firstrun.json`.

| Files | Configuration | First save | `gc` | Clean `status` |
|---|---|---|---|---|
| 10,000 | defaults (object files) | 47.0 s | 0.41 s | 0.10 s |
| 10,000 | `--objects database` | 33.1 s | 0.05 s | 0.12 s |
| 10,000 | Defender exclusion | not measured: no permission | not measured | not measured |
| 100,000 | defaults (object files) | 448 s | 4.1 s | 0.86 s |
| 100,000 | `--objects database` | 348 s | 0.38 s | 0.85 s |
| 100,000 | Defender exclusion | not measured: no permission | not measured | not measured |

The exclusion rows were not measured because the account that ran the
benchmark is not an administrator and cannot add a Defender exclusion. No
workaround was attempted. Anyone with the permission can fill the rows in with
`python bench/scale/scale.py --files 100000 --snapshots 5 --defender-note "work
directory excluded" --out results.json`.

What the numbers say:

- Fewer opens (19.6-a) barely moved the first save: 49.6 s to 47.0 s at 10,000
  files and 520 s to 448 s at 100,000 (different runs, so treat the
  difference as small).
- `--objects database` cut the first save by about 30 percent and `gc` by an
  order of magnitude, but the first save is still about 50 times the Linux
  figure at 100,000 files. Writing no object files removes only part of the
  cost; every source file is still opened and read once.
- Clean `status` is under a second at both sizes in every configuration.

## Why it happens

Windows Defender real-time protection scans a file when it is opened. Velo
opens every file once to hash it, and, with the default object store, creates
one new object file for every new file, which is scanned again on write. Each
scan has a fixed per-file cost, so time grows with the file count rather than
the byte count. Linux has no such per-open hook. This is a property of the
platform configuration, not of velo's data model.

## Excluding the work directory

Excluding the project directory from real-time scanning is the setting expected
to remove most of the gap. From an elevated PowerShell:

```powershell
Add-MpPreference -ExclusionPath "C:\path\to\project"
```

The trade-off is real: files in an excluded directory are not scanned when they
are opened or written, so malware placed there is not caught by real-time
protection. Exclude only directories you control, such as an agent's scratch
workspace, and prefer a narrow path over a drive. On a managed machine the
exclusion may need your administrator. The effect of the exclusion on velo has
not been measured yet (see the table).

## The single-file option

`velo init --objects database` stores objects inside `velo.db` instead of one
file each under `.velo/objects`. Choose it for agent workloads and trees with
many small files: it needs no per-object file creation, and it made `gc`
roughly ten times faster here. What it costs: one large `velo.db` (about 188 MB
at 100,000 files against 171 MB of `.velo` for the default), and the choice is
fixed at init; an existing repository cannot be switched.

## Recommendation

- Small and medium trees (a few thousand files): the defaults are fine.
- Large trees or agent workloads on Windows: initialise with
  `velo init --objects database`, and, where your policy allows, exclude the
  work directory from Defender. The first save is paid once; later saves
  were 0.2-0.4 s at 10,000 files.
- Whether `--objects database` should become the default at `init` on Windows
  is open. It saves about 30 percent of the first save (47 s to 33 s, 448 s to
  348 s) and most of `gc`, and does not change clean `status`, which is already
  fast. That is not enough on its own to justify a platform-specific default,
  so the default is unchanged and the decision is recorded for the owner,
  ideally after the exclusion rows are measured.

A detector that notices a slow filesystem and suggests these settings is not
built.

## First save: more reader threads than cores

The first save of a large tree is one open per file under Defender. If that
cost were per-open latency rather than throughput, a rayon pool with more
threads than cores might overlap it. `first_save_threads` (an ignored,
Windows-only test in `velo-core`) writes the files into a fresh temporary
directory for each thread count, so Defender scans every file, then runs the
real first save inside a local rayon pool of that size. The production pool is
untouched.

Measured at 10,000 files on a 20-logical-CPU machine, default (loose object)
store, release build:

| threads | first save |
|--------:|-----------:|
|      20 |     22.8 s |
|      40 |     26.2 s |
|      80 |     23.8 s |

Measured, no gain: 40 and 80 threads were 15 percent and 4 percent slower
than 20. The cost under Defender is throughput, not per-open latency that
threads can overlap, so the pool size is unchanged. Raw results are in
`bench/results/2026-10-20-windows-first-save-threads.json`. One run per thread
count; 30,000 and 100,000 files were not measured.

To repeat it at 100,000 files (the first save alone was 348 to 448 s):

```text
VELO_PROFILE_FILES=100000 VELO_PROFILE_THREADS=20,40,80 VELO_PROFILE_OUT=out.json \
    cargo test -p velo-core --release first_save_threads \
    -- --ignored --nocapture --test-threads=1
```
