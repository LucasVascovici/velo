# Cold-start test

Can an agent that has never seen velo finish a real task from the public docs
and an installed release? This kit makes that runnable in an afternoon. The
run itself needs fresh agents and published release assets.

## Protocol

For each wedge, give a fresh agent only the docs below, an empty directory and
an installed wheel, package or binary, plus the one-sentence task in `tasks/`.
No checkout, no source tree, no hints, no answers to questions: where the agent
would ask, it guesses, and the guess is recorded.

| Wedge | Task | Cookbook to hand over | Install |
|-------|------|-----------------------|---------|
| Agent checkpointing, Python | `tasks/agent-checkpointing-python.md` | `docs/cookbook/agent-checkpointing.md` | wheel |
| Config registry, Node | `tasks/config-registry-node.md` | `docs/cookbook/config-registry.md` | tarball |
| Document editor, Rust | `tasks/document-editor-rust.md` | `docs/cookbook/document-editor.md` | `cargo install` |
| MCP client | `tasks/mcp-client.md` | none (the README's MCP section) | `cargo install` |

The task must be completed from the installed artefact, not a checkout. The
docs the agent sees must show `save_changes`, `velo report` and `velo sync`; if
they do not, that is the first friction entry.

## Exactly which files to hand over

`README.md` and the one cookbook for the wedge (`docs/cookbook/*.md`). Nothing
else: not `docs/FORMAT.md`, not the examples under `bindings/`, not this
directory. Copy the files from the tag under test, so the agent reads what the
release shipped.

## Install from release assets

Work in a scratch directory, never in a checkout. Download the assets of the
release under test and verify them first:

```bash
gh release download <tag> --repo LucasVascovici/velo -p '*.whl' -p '*.tgz' -p SHA256SUMS-bindings.txt
sha256sum --check --ignore-missing SHA256SUMS-bindings.txt

python -m venv venv && venv/bin/pip install <wheel URL or downloaded .whl>
npm install <tarball URL or downloaded .tgz>
cargo install --git https://github.com/LucasVascovici/velo --tag <tag> velo-cli
cargo install --git https://github.com/LucasVascovici/velo --tag <tag> velo-mcp
```

The `velo` binary used by `check.py` must come from the same release.

## Time bound

60 minutes per wedge, wall clock, from handing over the task. At the bound the
run stops and is recorded as unfinished. If the agent runs `save` by minute 60
the check below still runs.

## What to record

Copy `friction-template.md` and fill it in as the run goes:

- time to first successful `save`;
- every question the agent had to guess at;
- every error it hit;
- every place it reached for a method that does not exist;
- whether the result passes the independent check.

The independent check does not trust the agent's report:

```bash
python bench/coldstart/check.py --repo <dir> --velo <velo> --actor <actor name>
```

It prints a JSON verdict (fsck ok, at least one snapshot in history, the
expected actor named in blame for each file) and exits 0 or 1. Pass
`--file PATH` (repeatable) to check named files instead of every tracked one.
`test_check.py` tests it (`python -m unittest test_check`, skipped without
`velo` on PATH).

## Where results go

`bench/results/<date>-coldstart-<wedge>.md`, one file per wedge and run, for
example `2026-11-02-coldstart-agent-checkpointing-python.md`. The friction
table is the doc backlog: each row is fixed or marked consciously deferred.
Re-run the whole kit on the 6.0 release and compare.
