# Cookbook: agent checkpointing

Script: [`bindings/python/examples/agent_checkpointing.py`](../../bindings/python/examples/agent_checkpointing.py)

An agent that edits files should leave a trail you can audit and undo. velo
gives that without a working tree: one snapshot per tool call, one branch per
attempt, and provenance that is part of the snapshot's hash.

## One snapshot per tool call

Every tool call is saved with the run, the call id and the model in `agent`
metadata, and an author naming the run:

```python
repo.save_tree(branch=branch, message=message, entries=files, parent=parent,
               author=velo.Author(f"agent-{run}"),
               meta={"agent": {"run": run, "tool_call_id": call, "model": model}})
```

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

There is no force option: a dirty tree is an error result and nothing is
overwritten. See [`crates/velo-mcp/README.md`](../../crates/velo-mcp/README.md).
