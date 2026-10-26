# Task: Python agent checkpointing

Documentation given: `README.md`, `docs/LIBRARY.md`, `docs/cookbook/agent-checkpointing.md`.
Installed from: the Python wheel from the release.

## Task (give the agent only this sentence)

An agent that edits files should leave a trail you can audit and undo.

Use the actor name `coldstart-py` for the run and author of every snapshot.
(Give this name to the agent together with the sentence.)

## Acceptance (for the checker; not shown to the agent)

- The agent's repository is a velody repository with at least one snapshot per
  edit it made, saved through the installed wheel.
- `velody fsck` is clean.
- History is non-empty.
- Blame of a file the agent edited names `coldstart-py`.

```bash
python ../check.py <repo-dir> --actor coldstart-py --file <edited file>
```

Source of the sentence: first sentence of the introduction of
`docs/cookbook/agent-checkpointing.md`.
