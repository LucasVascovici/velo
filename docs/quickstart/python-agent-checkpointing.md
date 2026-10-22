# Python: checkpoint an agent in five minutes

Save a snapshot after every tool call, look the run up in the history, and
restore an earlier state. No Rust toolchain is needed: the wheel is a
prebuilt abi3 wheel.

## Install

Each [release](../INSTALL.md) attaches one wheel per platform, named
`velo_core-<version>-cp39-abi3-<platform>.whl`:

```sh
pip install https://github.com/LucasVascovici/velo/releases/download/vX.Y.Z/<wheel file>
```

The distribution is `velo-core`; you import it as `velo`.

## Save, read the history, restore

```python quickstart
import shutil
import tempfile

import velo

# On Windows the database file can stay locked until the handle is collected,
# so cleaning up the scratch directory is best effort.
tmp = tempfile.mkdtemp()
try:
    repo = velo.Repo.init(tmp)

    # One snapshot per tool call; send only the files the call changed.
    first = repo.save_changes(
        branch="main",
        message="write the first draft",
        changes={"app.py": "print('v1')\n", "notes.md": "plan\n"},
        author=velo.Author("agent-1"),
        meta={"agent": {"run": "run-1", "tool_call_id": "call-1"}},
    ).snapshot
    second = repo.save_changes(
        branch="main",
        message="break it",
        changes={"app.py": "print('oops')\n"},
        parent=first,
        author=velo.Author("agent-1"),
        meta={"agent": {"run": "run-1", "tool_call_id": "call-2"}},
    ).snapshot

    # History, newest first, filtered by the metadata the agent wrote.
    log = repo.history(branch="main", meta=[("agent", "run", "run-1")])
    assert [e.id for e in log] == [second, first]

    # Restore: the earlier file is still there, byte for byte.
    assert repo.read_file_at(first, "app.py") == b"print('v1')\n"
    out = tmp + "-restored"
    repo.materialize(first, out)
    with open(out + "/app.py", encoding="utf-8", newline="") as f:
        assert f.read() == "print('v1')\n"
    shutil.rmtree(out, ignore_errors=True)
    print("ok:", first[:8], second[:8])
finally:
    shutil.rmtree(tmp, ignore_errors=True)
```

## See who wrote what

When the run is done, `velo report` writes one self-contained HTML page: an
integrity verdict, a timeline per branch and the origin of every line. It opens
offline; see the [sample](../report-sample.html).

```sh
velo report -o report.html
```

The same recipe with a branch per attempt and a merge of the winner is in the
[cookbook](../cookbook/agent-checkpointing.md).
