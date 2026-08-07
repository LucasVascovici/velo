# velo-cli

The command-line interface for [Velo](https://github.com/LucasVascovici/velo) — a
fast, safe, intuitive version control system. Installs the `velo` binary.

```bash
cargo install --git https://github.com/LucasVascovici/velo velo-cli
```

Git's power without Git's sharp edges: no staging area (what is on disk is what
gets recorded), guards that refuse destructive operations while you have unsaved
work, a hunk-by-hunk conflict resolver that keeps your files valid throughout,
and `velo undo` / `velo redo` instead of a reset-and-reflog dance.

```bash
velo init
velo save "first snapshot"     # no `add` step
velo switch feature            # creates it if new
velo history --all --graph
velo merge feature             # conflicts resolve in a TUI, not with markers
velo undo                      # reversible
```

Push and pull are deliberately boring: `velo push` is fast-forward-only and
refuses divergence with instructions, and `velo pull` either fast-forwards or
stops and tells you — it never silently rewrites history.

The full command reference, comparison with Git, and installation options are in
the [project README](https://github.com/LucasVascovici/velo).

## A note on what this is

Velo was vibe-coded for fun — a real working tool, built as an experiment in a
tight loop with an AI assistant, not as a production-grade Git replacement.

MIT licensed.
