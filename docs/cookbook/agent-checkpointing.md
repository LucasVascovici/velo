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
2,000 snapshots over 60 days a pass cut snapshots, rows and live content by 62%.
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

**Windows 11** (i7-1370P, Python 3.12.8, git 2.42.0.windows.2, Defender
real-time protection ON, exclusions not readable, assumed none):

| system | 500 steps (s) | 8 attempts + merge (s) | restore step 250, median (ms) | 100 restores (s) | who wrote x20 (s) | disk (MB) |
|---|---|---|---|---|---|---|
| velo | 91.1 | 17.5 | 878 | 116.5 | 4.81 | 174 |
| shadow git | 221.3 | 38.7 | 470 | 49.5 | 3.24 | 5 |
| git worktree | 214.8 | 44.8 | 501 | 53.1 | 3.45 | 13 |
| dir copy | 567.9 | 63.8 | 659 | 68.8 | 19.45 | 571 |
| jsonl log | 1.3 | 1.1 | 688 | 69.3 | 0.63 | 7 |
| sqlite blobs | 9.5 | 1.9 | 732 | 75.0 | 0.00 | 33 |

**Linux** (Ubuntu 22.04 under WSL2 on the same machine, ext4, Python 3.10.12,
git 2.34.1; not a bare-metal or CI Linux box, and the `agent` job in
`bench.yml` reruns it on a GitHub runner):

| system | 500 steps (s) | 8 attempts + merge (s) | restore step 250, median (ms) | 100 restores (s) | who wrote x20 (s) | disk (MB) |
|---|---|---|---|---|---|---|
| velo | 58.1 | 8.4 | 222 | 22.6 | 0.55 | 174 |
| shadow git | 11.3 | 3.0 | 46 | 4.5 | 0.32 | 5 |
| git worktree | 11.2 | 3.3 | 43 | 4.3 | 0.31 | 13 |
| dir copy | 30.1 | 5.2 | 45 | 4.8 | 0.19 | 571 |
| jsonl log | 14.5 | 0.9 | 59 | 6.0 | 0.39 | 7 |
| sqlite blobs | 18.3 | 2.5 | 41 | 4.2 | 0.00 | 33 |

Raw results: `bench/results/agent-2026-10-15-windows.json` and
`bench/results/agent-2026-10-15-linux.json`. Disk is the store only, in
apparent bytes (the live working directory is not counted); for git worktree it
includes every attempt's checked-out tree. SQLite's `who wrote` is near zero
because an index lookup finds the file's versions; it answers "which step", not
a line-level blame.

### What velo is, and is not, here

velo loses most rows. Read the table that way:

- **Speed on Linux.** Every other system checkpoints 500 steps faster than velo:
  git does it in 11 s against velo's 58 s, and restores are about five times slower than git, a
  directory copy or SQLite. The harness hands velo a 1,000-entry tree each step
  (999 carried forward) and re-reads the tree after every save to learn the new
  object ids, and both are inside the timed region. A binding that returned the
  ids of the files it just wrote would cut that; this benchmark measures the API
  as it is today. Restore through `tree_at` plus one `read_file_at` per file
  costs a call per file.
- **Speed on Windows.** velo is the fastest of the git and copy options for
  steps (91 s against git's 215 to 221 s and the copy's 568 s) because it does
  not spawn processes or touch the working tree, but it is the slowest at
  restore and the slowest of all but the directory copy at line-level blame, and the log and SQLite stores beat it by one
  to two orders of magnitude on steps.
- **Disk.** velo used 174 MB against git's 5 MB, most likely because every
  snapshot records the whole 1,000-file tree even when 999 files were carried
  forward by id (carry-forward saves the object bytes, not the per-snapshot tree
  record); this run did not break the 174 MB down. A
  directory copy is the only thing bigger (571 MB). If disk is the constraint on
  a long agent run over a big tree, git (or the log) wins clearly.

Where each alternative is better:

- **Shadow git and git worktree.** Smallest store, fastest blame and restore on
  Linux, a toolchain every developer already has, and `git diff`, `git log` and
  hosting for free. A worktree per attempt costs a full checked-out tree on disk
  per attempt (13 MB against 5 MB) but lets an attempt run its tests in place.
  Git has no per-step provenance that cannot be edited: authorship is whatever
  was passed to `commit`.
- **Directory copy.** Simplest thing that works and, on Linux, as fast to restore
  as git (45 ms): a restore is a `copytree`. It costs the most disk by far and gets
  slow on Windows, where 500 copies of 1,000 files took 568 s.
- **Append-only log.** Fastest to write on Windows by two orders of magnitude and
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
