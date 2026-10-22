# Cookbook: agent checkpointing

Script: [`bindings/python/examples/agent_checkpointing.py`](../../bindings/python/examples/agent_checkpointing.py)

An agent that edits files should leave a trail you can audit and undo. velo
gives that without a working tree: one snapshot per tool call, one branch per
attempt, and provenance that is part of the snapshot's hash.

## One snapshot per tool call

Every tool call is saved with the run, the call id and the model in `agent`
metadata, and an author naming the run:

```python
saved = repo.save_changes(branch=branch, message=message, changes=files,
                          parent=parent, author=velo.Author(f"agent-{run}"),
                          meta={"agent": {"run": run, "tool_call_id": call,
                                          "model": model}})
saved.snapshot  # the new snapshot id; saved.written maps path -> object id
```

`changes` holds only what the tool call touched: a value is written and `None`
deletes the path. The rest of the parent's tree is loaded inside the database,
so the call sends no tree and gets ids back. A list of `velo.Change` values
adds `Change.rename` and `Change.stored`.

Metadata is hashed into the snapshot id, so provenance cannot be edited after
the fact.

## A branch per attempt

Both attempts fork from the same base snapshot, so they are directly
comparable and cannot interfere:

```python
repo.create_branch("attempt-1", at=base)
repo.create_branch("attempt-2", at=base)
```

The final snapshot of each attempt records `tests` (`pass` or `fail`) in the
same `agent` namespace.

## Pick the winner by metadata

```python
winners = repo.find_snapshots([("agent", "tests", "pass")])
repo.history(from_=a2, meta=[("agent", "run", "run-1")])  # one run's calls
```

Filters are `(namespace, key, value)` tuples. The script asserts that exactly
the passing snapshot is found and that the run's history lists both of its tool
calls.

## Merge into main

Merging is always plan, then commit:

```python
plan = repo.merge_plan(base, winner.id)       # plan.is_clean
merged = repo.merge_commit(branch="main", ours=base, theirs=winner.id,
                           message="take attempt-1")
```

The result is a two-parent snapshot (`is_merge`, `merge_parent` is the
winner), and the discarded attempt stays reachable on `attempt-2` for review.

## Blame names the run

`blame` gives each line's origin snapshot; read the run from that snapshot's
metadata:

```python
for line in repo.blame("app.py", at=merged).lines:
    run = repo.snapshot_meta(line.origin.id).get("agent", {}).get("run", "human")
```

The script asserts that the lines the agent wrote are attributed to `run-1`
and the human's lines to the human.

## The same thing through MCP

This section is prose only; it is not executed. `velo-mcp` serves a repository
with a working tree to any MCP client, so an agent gets this behaviour with no
integration code. Client configuration:

```json
{"command": "velo-mcp", "args": ["--repo", "/path/to/project", "--run", "run-1"]}
```

The tools map onto the steps above:

1. `velo_branch {"action": "create", "name": "attempt-1"}`, then
   `{"action": "switch", "name": "attempt-1"}`.
2. After each edit, `velo_save {"message": "..."}`. The run, tool and client
   are recorded in `mcp` metadata for you (`--run` or `$VELO_MCP_RUN` sets the
   run id).
3. `velo_history` filters by metadata such as the run, and `velo_metadata`
   shows a snapshot's author and metadata.
4. `velo_branch {"action": "switch", "name": "main"}`, then
   `velo_merge_plan {"source": "attempt-1"}` to see what would change.
5. `velo_merge_apply {"source": "attempt-1", "message": "take attempt-1"}`;
   conflicts need an explicit resolution for each path.
6. `velo_blame {"path": "app.py"}`: `origin.run` names the run that wrote each
   line.

An agent with no checkout can run `velo-mcp --store agent.velo --create`
instead: content goes in the tool call (`velo_save_tree`), trees come back by
id, and no working tree is ever written. See the "Store-only mode" section of
the velo-mcp README.

There is no force option: a dirty tree is an error result and nothing is
overwritten. See [`crates/velo-mcp/README.md`](../../crates/velo-mcp/README.md).

## Keep the history bounded

An agent that saves after every tool call piles up snapshots, and each one owns
a row per file. Thin the history on a schedule rather than letting it
accumulate:

```bash
velo retention --dry-run          # what would go, and what is protected
velo retention                    # thin, then gc and return the freed space
velo retention --keep 1d=1h --keep 30d=1d   # the same tiers, spelled out
```

With no `--keep`, everything stays for a day, then one snapshot per hour for a
month, then one per day. Signed and tagged snapshots always stay. The command
runs `gc` and an incremental vacuum afterwards, so the database file shrinks on
repositories created from 5.3.0.

An embedder does the same through the library:
`velo_core::commands::retention::apply` with a `Policy`, then
`commands::gc::run` and `commands::gc::vacuum`. On a 10,000-file tree with
2,000 snapshots over 60 days a pass cut snapshots, rows and live content by
62% and the database file from 5.18 GiB to 2.06 GiB, in about 7 minutes for
retention and 3 for the vacuum. A repository created before 5.3.0 needs one
`velo gc --vacuum` first to switch to incremental vacuum.
The measurement, its method and the envelope velo was tested to are in
[docs/SCALE.md](../SCALE.md).

## Against the alternatives

Numbers, not adjectives: [`bench/agent/agent_bench.py`](../../bench/agent/agent_bench.py)
runs the same agent workload against velo and five things an agent author would
reach for. Method and caveats are in [`bench/agent/README.md`](../../bench/agent/README.md).

Workloads: 500 steps each editing 1 to 20 files (1 to 3 lines each) in a
1,000-file project of about 1 KB files; 8 attempts of 10 steps from the final
state, the last one merged back; restore step 250, 100 times; "who wrote this
line" for 20 lines at the end of the run. Seeded, so every system sees the same
edits. Each system's answers are checked against the generator's record.

**Linux GitHub runner, velo 6.2.0, format v4, 2026-10-22** (`ubuntu-24.04`
hosted runner, image 20261012.334.1, Python 3.12.15, git 2.55.0, commit
`dd15186`, `bench.yml` run 39866892547; the runner is not the laptop, so these
numbers are not comparable with the tables below; disk is the store after the
whole run, open handle):

| system | 500 steps (s) | 8 attempts + merge (s) | restore step 250, median (ms) | 100 restores (s) | who wrote x20 (s) | disk (MB) |
|---|---|---|---|---|---|---|
| velo | 1.3 | 0.3 | 24 | 2.4 | 0.26 | 12 |
| velo (5.2 API) | 2.5 | 0.6 | 1,665 | 166.9 | 0.26 | 12 |
| shadow git | 12.1 | 3.2 | 46 | 4.6 | 0.17 | 4 |
| git worktree | 11.8 | 3.3 | 46 | 4.7 | 0.17 | 12 |
| dir copy | 51.8 | 9.4 | 101 | 10.5 | 0.42 | 571 |
| jsonl log | 0.2 | 0.04 | 96 | 9.6 | 0.36 | 7 |
| sqlite blobs | 1.5 | 0.3 | 83 | 8.4 | 0.00 | 33 |

On this run velo is 0.10x shadow git for 500 steps and 0.53x for restore, 1.49x
for "who wrote" (slower), and 3.1x for disk, which still misses the 3x target;
see [`PERF.md`](../PERF.md) section 6. Raw:
`bench/results/2026-10-linux-runner-agent-500-v4.json`.

The tables below are earlier versions (5.3 era, format v3) on other machines,
kept as they were.

**Windows 11, earlier version** (i7-1370P, Python 3.12.8, git 2.42.0.windows.2, Defender
real-time protection assumed ON, exclusions not checked):

| system | 500 steps (s) | 8 attempts + merge (s) | restore step 250, median (ms) | 100 restores (s) | who wrote x20 (s) | disk (MB) |
|---|---|---|---|---|---|---|
| velo | 64.4 | 13.2 | 216 | 23.2 | 2.95 | 174 |
| velo (5.2 API) | 87.3 | 16.4 | 844 | 113.3 | 5.21 | 174 |
| shadow git | 190.9 | 31.0 | 422 | 43.6 | 2.42 | 5 |
| git worktree | 174.2 | 39.5 | 466 | 48.4 | 2.49 | 13 |
| dir copy | 434.0 | 79.8 | 729 | 74.1 | 10.68 | 571 |
| jsonl log | 1.4 | 0.4 | 705 | 71.8 | 0.55 | 7 |
| sqlite blobs | 8.8 | 2.1 | 738 | 74.1 | 0.00 | 33 |

**Linux, earlier version** (Ubuntu 22.04 under WSL2 on the same machine, ext4, Python 3.10.12,
git 2.34.1; not a bare-metal or CI Linux box, and the `agent` job in
`bench.yml` reruns it on a GitHub runner):

| system | 500 steps (s) | 8 attempts + merge (s) | restore step 250, median (ms) | 100 restores (s) | who wrote x20 (s) | disk (MB) |
|---|---|---|---|---|---|---|
| velo | 37.4 | 7.3 | 13 | 1.3 | 0.35 | 174 |
| velo (5.2 API) | 41.2 | 6.7 | 145 | 14.6 | 0.31 | 174 |
| shadow git | 8.4 | 2.2 | 29 | 3.0 | 0.23 | 5 |
| git worktree | 8.1 | 2.9 | 37 | 3.8 | 0.23 | 13 |
| dir copy | 23.4 | 3.4 | 37 | 3.8 | 0.13 | 571 |
| jsonl log | 3.2 | 0.5 | 45 | 4.8 | 0.31 | 7 |
| sqlite blobs | 15.4 | 1.8 | 30 | 3.1 | 0.00 | 33 |

`velo` is the 5.3 calling convention: `save_changes` with only the changed files
per step (no entry list, no `tree_at` re-read) and `materialize` for restore.
`velo (5.2 API)` is the old harness, kept as it was: `save_tree` with the whole
tree carried forward by object id, a `tree_at` re-read after every save, and a
`read_file_at` per file on restore. Both rows are from the same run, and both
pass the same answer checks.

Raw results: `bench/results/agent-2026-10-16-windows.json` and
`bench/results/agent-2026-10-16-linux.json`. The earlier run, with only the 5.2
API, is in
`bench/results/agent-2026-10-15-windows.json` and
`bench/results/agent-2026-10-15-linux.json`; absolute times for every system
moved between the two days (shadow git steps on Linux went from 11.3 s to 8.4 s
with no code change), so compare rows within one table, not across days. Disk is
the store only, in apparent bytes (the live working directory is not counted);
for git worktree it includes every attempt's checked-out tree. SQLite's `who
wrote` is near zero because an index lookup finds the file's versions; it
answers "which step", not a line-level blame.

**Phase 19 re-run.** The tables above are still the latest agent runs: the code
has not changed since the 2026-10-16 run, so it was not repeated. The scale
envelope was re-run on 2026-10-17 (see [`SCALE.md`](../SCALE.md)). Against the
targets in [`PERF.md`](../PERF.md) section 6, velo still loses three rows, and
each loss has a measured reason there:

- **500 steps on Linux: 4.5x shadow git** (37.4 s against 8.4 s; the target was
  3x, the proposal 2x). One SQLite commit per step is most of it.
- **Disk after 500 steps: 41x shadow git on Linux and 47x on Windows** (152 MB
  against 3.7 MB and 3.3 MB; the target was 100 MB, then 3x). Final disk after
  the restores and branches is 174 MB, 35x and 38x. Every snapshot stores its whole 1,000-file
  tree, and nothing that shrinks that has shipped.
- **A one-file save at 100,000 files is not flat:** 4.27x slower after 200
  snapshots than after the first few on Linux and 5.79x on Windows (target
  1.5x), because each save still writes one row per file.

Restore meets its target on both platforms, and 500 steps on Windows meets it
(velo is 3x faster than shadow git there).

**Phase 20 re-run (Windows only, 2026-10-20, format v4).** velo and shadow git
were re-run on Windows
(`bench/results/2026-10-20-windows-agent-velo.json` and
`-agent-git.json`); the tables above are the earlier runs and were not
overwritten, and Linux was not re-run. No harness parameter was changed. On
Windows, velo took 19.4 s for the 500 steps against 210.5 s for shadow git
(0.09x), 4.29 s against 38.5 s for the 8 attempts plus merge, 480 ms against
480 ms for the restore median, and 11.0 MB of disk after the steps against 3.3
MB (final disk 11.8 MB against 4.5 MB), where 5.3.0 used 152 MB. The rows still
lost, each with its reason (full detail in [`PERF.md`](../PERF.md) section 6,
"Result of the Phase 20 re-run"):

- **Disk after 500 steps: 3.37x shadow git** (11.0 MB against 3.27 MB), 12% over
  the 3x target, though it is the 11 MB the target was derived from. What is
  stored is a keyframe every 128 snapshots plus the changed rows. Of the
  11.2 MB, 4.3 MB is the live write-ahead log, which SQLite removes when the
  last handle closes: at rest the repository is 6.9 MB, 2.1x git (3.1 MB of
  objects, 3.8 MB of database; breakdown in PERF.md section 12).
- **"Who wrote this line": 3.1x shadow git** (9.30 s against 3.01 s; was 1.2x).
  Measured cause: the blame walk visits every ancestor snapshot (about 500)
  although the file changed in about 5, and at each visit it reads the text of
  the snapshot and its parent from the object store without a cache. Those
  reads are 78% of the blame time; diffing is 2.4% and the history walk is
  small (`PERF.md` section 6, Phase 20 re-run, and
  `bench/results/2026-10-20-windows-who-wrote-profile.txt`).
- **500 steps on Linux: 4.5x shadow git** when last measured (not re-run).
- **A one-file save at 100,000 files:** about 2.5x the history-0 save (5.3 s
  median against a 2.1 s floor; target 1.5x) because each save checks the
  existence of every object (`save_tree.objects`, 95% of the save).

### What velo is, and is not, here

velo still loses most rows on Linux. Read the tables that way:

- **Speed on Linux.** The new calling convention took the 500 steps from 41.2 s
  to 37.4 s (9% less) and restore from 145 ms to 13 ms (11 times faster, 14.6 s
  to 1.3 s over 100 restores). Restore is now the one row velo wins outright on
  Linux: 13 ms against 29 ms for shadow git, the next best. Steps still lose to
  every other system: 4.5x shadow git (8.4 s), 4.6x git worktree (8.1 s), 1.6x
  directory copy (23.4 s), 11.7x the log (3.2 s) and 2.4x SQLite (15.4 s); the
  engine's commit (see [`PERF.md`](../PERF.md)) is what remains. Branches lose
  too: 3.4x shadow git, 2.5x git worktree, 2.1x directory copy, 13.8x the log
  and 4.0x SQLite. "Who wrote this line" loses to git by 1.5x (0.35 s against
  0.23 s), to the copy by 2.6x, and to the log by 1.1x; SQLite answers a
  different question.
- **Speed on Windows.** velo is now the fastest of the git, copy and replay
  options at steps (64 s against git's 174 to 191 s and the copy's 434 s),
  branches (13.2 s against 31 to 80 s) and restore (216 ms against 422 ms for
  shadow git), but the log and SQLite stores still beat it on steps by 46x and
  7.3x, and on branches by 35x and 6.3x. Blame loses to git by 1.2x (2.95 s
  against 2.42 s) and to the log by 5.3x.
- **Disk.** velo used 174 MB of final disk against git's 5 MB (35x on Linux, 38x on Windows; after the 500 steps alone it is 41x and 47x), git
  worktree's 13 MB (14x), the log's 7 MB (24x) and SQLite's 33 MB (5.3x), most likely because every
  snapshot records the whole 1,000-file tree even when 999 files were carried
  forward by id (carry-forward saves the object bytes, not the per-snapshot tree
  record); this run did not break the 174 MB down. A
  directory copy is the only thing bigger (571 MB). If disk is the constraint on
  a long agent run over a big tree, git (or the log) wins clearly.

Where each alternative is better:

- **Shadow git and git worktree.** Smallest store, fast blame and restore on
  Linux, a toolchain every developer already has, and `git diff`, `git log` and
  hosting for free. A worktree per attempt costs a full checked-out tree on disk
  per attempt (13 MB against 5 MB) but lets an attempt run its tests in place.
  Git has no per-step provenance that cannot be edited: authorship is whatever
  was passed to `commit`.
- **Directory copy.** Simplest thing that works and, on Linux, as fast to restore
  as git (37 ms): a restore is a `copytree`. It costs the most disk by far and gets
  slow on Windows, where 500 copies of 1,000 files took 434 s.
- **Append-only log.** Fastest to write by a wide margin (46x velo on Windows, 12x on Linux) and
  tiny on disk when edits are small. Restore replays everything, so it gets
  slower with run length, and there is no branching or merging, only appending a
  winner's changes.
- **SQLite blobs.** Fast to write and restore, one file to ship, and the best
  lookup of "which step touched this file". No line-level blame, no merge, no
  content addressing, and the manifest per step repeats the tree.

What no alternative has is **verifiable provenance for free.** velo hashes the
author and the `agent` metadata (run, tool call, model) into every snapshot id,
so "which run wrote this line" is answered by `blame` plus `snapshot_meta`, and
`fsck` recomputes the ids. git hashes content and commit fields but cannot tell
you that a commit really came from run 1; the directory copy, log and SQLite
store record nothing that can be checked. If you only need to undo, pick the
cheaper tool above; if you must show afterwards what an agent did and that the
record was not altered, that is the case for velo.

### Reproduce

```
python -m venv .venv                 # .venv/Scripts on Windows
.venv/bin/pip install maturin
.venv/bin/maturin develop --release --manifest-path bindings/python/Cargo.toml
.venv/bin/python bench/agent/agent_bench.py --out bench/results/agent-<date>-<platform>.json
```

`--quick` runs a small version in about a minute. The full run takes roughly 25
minutes on Linux and 40 on Windows, mostly git and the directory copy.

## See what the agent did: `velo report`

Blame answers one line at a time. To hand someone the whole run, write a report
from the repository the run used:

```
velo report -o run.html
```

The page holds the timeline of every branch (both attempts and the merge), each
snapshot's metadata (the run, tool call, model and test result), signature
badges, the author of every line of every file at each branch tip, and the
`fsck` verdict that every snapshot id was recomputed. The author column is the
cookbook's answer made visible: the generator line is by `agent-run-1`, the
lines it did not touch are by `human`.

The file is static and works offline: no script, no network, a content-security
policy that forbids both, and every message, author, metadata value and file
line escaped, so a hostile agent cannot put markup in your audit page. `velo
report --json` prints the same data for tooling.

A generated sample of this exact scenario is in
[`report-sample.html`](../report-sample.html).
