"""Agent checkpointing: a snapshot per tool call, a branch per attempt.

Runs against the velo Python binding and asserts every claim it prints.
"""
import tempfile

import velo

BASE = "def total(xs):\n    return sum(xs)\n\nprint(total([1, 2]))\n"
GENERATOR = BASE.replace("sum(xs)", "sum(x for x in xs)")


def tool_call(repo, branch, parent, run, call, model, files, message, tests=None):
    """Save one tool call as one snapshot, recording the run in metadata."""
    agent = {"run": run, "tool_call_id": call, "model": model}
    if tests is not None:
        agent["tests"] = tests
    return repo.save_tree(
        branch=branch,
        message=message,
        entries=files,
        parent=parent,
        author=velo.Author(f"agent-{run}"),
        meta={"agent": agent},
    )


with tempfile.TemporaryDirectory() as tmp:
    repo = velo.Repo.init(tmp)

    # A base snapshot written by a human; both attempts fork from it.
    base = repo.save_tree(
        branch="main", message="base", entries={"app.py": BASE},
        author=velo.Author("human"),
    )
    repo.create_branch("attempt-1", at=base)
    repo.create_branch("attempt-2", at=base)

    # Attempt 1: two tool calls, the last one passes its tests.
    a1 = tool_call(repo, "attempt-1", base, "run-1", "call-1", "model-a",
                   {"app.py": GENERATOR}, "rewrite total as a generator")
    a2 = tool_call(repo, "attempt-1", a1, "run-1", "call-2", "model-a",
                   {"app.py": GENERATOR + "print(total([]))\n"},
                   "add an empty-list check", tests="pass")

    # Attempt 2: one tool call that fails its tests.
    b1 = tool_call(repo, "attempt-2", base, "run-2", "call-1", "model-b",
                   {"app.py": "def total(xs):\n    return 0\n"},
                   "stub total", tests="fail")

    assert repo.branch_tip("attempt-1") == a2
    assert repo.branch_tip("attempt-2") == b1
    assert repo.branch_tip("main") == base

    # Pick the winner by metadata, not by remembering ids.
    winners = repo.find_snapshots([("agent", "tests", "pass")])
    assert [e.id for e in winners] == [a2]
    winner = winners[0]
    assert repo.snapshot_meta(winner.id)["agent"]["run"] == "run-1"
    run1 = repo.history(from_=a2, meta=[("agent", "run", "run-1")])
    assert [e.id for e in run1] == [a2, a1]

    # Merge the winner into main: plan first, then commit.
    plan = repo.merge_plan(base, winner.id)
    assert plan.is_clean
    merged = repo.merge_commit(
        branch="main", ours=base, theirs=winner.id,
        message="take attempt-1",
        author=velo.Author("human"),
        meta={"agent": {"picked": "run-1"}},
    )
    entry = repo.snapshot(merged)
    assert entry.is_merge and entry.parent == base and entry.merge_parent == a2
    assert repo.read_file_at(merged, "app.py") == repo.read_file_at(a2, "app.py")
    assert repo.branch_tip("main") == merged

    # Blame: each line's origin names the run that wrote it.
    blame = repo.blame("app.py", at=merged)
    runs = {}
    for line in blame.lines:
        origin_meta = repo.snapshot_meta(line.origin.id)
        run = origin_meta.get("agent", {}).get("run", "human")
        runs[line.text.strip()] = (run, line.origin.author.name)
        print(f"{line.line_no}: {run:<6} {line.text.rstrip()}")
    assert runs["return sum(x for x in xs)"] == ("run-1", "agent-run-1")
    assert runs["print(total([]))"] == ("run-1", "agent-run-1")
    assert runs["def total(xs):"] == ("human", "human")

    print("winner:", winner.message, "->", merged[:12])

    # Release the database before the directory is removed (Windows).
    del repo
