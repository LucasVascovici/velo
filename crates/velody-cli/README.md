# velody-cli

The command-line interface for [Velody](https://github.com/LucasVascovici/velody) — a
fast, safe, intuitive version control system. Installs the `velody` binary.

```bash
cargo install --git https://github.com/LucasVascovici/velody velody-cli
```

Git's power without Git's sharp edges: no staging area (what is on disk is what
gets recorded), guards that refuse destructive operations while you have unsaved
work, a hunk-by-hunk conflict resolver that keeps your files valid throughout,
and `velody undo` / `velody redo` instead of a reset-and-reflog dance.

```bash
velody init
velody save "first snapshot"     # no `add` step
velody switch feature            # creates it if new
velody history --all --graph
velody merge feature             # conflicts resolve in a TUI, not with markers
velody undo                      # reversible
```

Push and pull are deliberately boring: `velody push` is fast-forward-only and
refuses divergence with instructions, and `velody pull` either fast-forwards or
stops and tells you — it never silently rewrites history.

The full command reference, comparison with Git, and installation options are in
the [project README](https://github.com/LucasVascovici/velody).

## A note on what this is

Mutating commands wait up to 3 seconds for the repository write lock (retrying
every 10-50 ms) before reporting that another velody operation holds it, naming
its pid, command and age. Set `VELODY_LOCK_WAIT_MS` to change the wait; `0`
fails immediately. A lock dies with its process, so there is nothing to unlock.

Velody was vibe-coded for fun — a real working tool, built as an experiment in a
tight loop with an AI assistant, not as a production-grade Git replacement.

MIT licensed.

## Exit codes

| Code | Meaning |
|------|---------|
| 0 | Success, including "nothing to save", "already up to date" and "nothing to pull" |
| 1 | Error |
| 2 | Command-line usage error |
| 3 | Conflicts left to resolve (`merge`, `cherry-pick` or `rebase` stopped on conflicts) |
| 4 | Histories diverged (`pull`, or a `push` the remote refused) |
| 5 | Repository locked by another velody process |
