# Task: MCP client

Documentation given: `README.md` and `docs/LIBRARY.md`. The "MCP server"
section of `docs/LIBRARY.md` is the documentation for this wedge; there is no cookbook.
Installed from: `velo-mcp` built from the release tag, as `docs/LIBRARY.md` says.

## Task (give the agent only this sentence)

Make velo the checkpoint layer for an agent, with every write recorded against
a run id.

Use the run id `coldstart-mcp`. (Give this name to the agent together with the
sentence.)

## Acceptance (for the checker; not shown to the agent)

- The agent drove `velo-mcp` over stdio (a client config or a script it wrote)
  and saved at least one snapshot through `velo_save`.
- `velo fsck` is clean.
- History is non-empty.
- Blame of a file it saved names the actor recorded for run `coldstart-mcp`.
  If the server records the run id under a different actor name, pass that name
  and note it in the friction log as a doc question.

```bash
python ../check.py <repo-dir> --actor <actor-for-run> --file <saved file>
```

Source of the sentence: `docs/LIBRARY.md`'s "MCP server" section ("makes velo the
checkpoint layer for an agent ... with every write recorded against a run id").
