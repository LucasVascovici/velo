# Cold-start test

Can an agent that has never seen this repository finish a real task using only
the README and one cookbook? This directory is the kit for finding out. It
holds the protocol, one task card per wedge, an independent checker and a
friction log template. It holds no results: nothing here has been run, and
every table is empty until the owner fills it in.

The run needs fresh agents with no memory of the repository, so it cannot be
done by whoever implements velo. The owner runs it, once per release.

## What is under test

Four wedges, one card each in [`tasks/`](tasks):

| Wedge | Card | Documentation given besides README.md |
| --- | --- | --- |
| Agent checkpointing, Python | [`python-agent-checkpointing.md`](tasks/python-agent-checkpointing.md) | `docs/cookbook/agent-checkpointing.md` |
| Config registry, Node | [`node-config-registry.md`](tasks/node-config-registry.md) | `docs/cookbook/config-registry.md` |
| Document editor, Rust | [`rust-document-editor.md`](tasks/rust-document-editor.md) | `docs/cookbook/document-editor.md` |
| MCP client | [`mcp-client.md`](tasks/mcp-client.md) | the README's "MCP server" section only |

The task is done from an installed wheel or tarball, not from a checkout. The
documents the agent receives must show `save_changes`, `velo report` and
`velo sync`; check that before each run (see "Before you start").

## Before you start

1. Pick the release tag to test (`vX.Y.Z`).
2. Confirm the README and cookbooks at that tag mention `save_changes`,
   `velo report` and `velo sync`:
   `git show vX.Y.Z:README.md | grep -c -E "save_changes|velo report|velo sync"`
   and the same for the cookbook the wedge uses. A missing term is itself a
   finding: write it in the friction log as a doc defect.
3. Make one fresh directory per wedge, outside any checkout, for example
   `~/coldstart/<wedge>/work`. Do not run an agent inside a clone of this
   repository.

## Install from release assets

Install what a user would: the files attached to the GitHub release. Nothing is
on PyPI or npm.

```bash
mkdir -p ~/coldstart/assets && cd ~/coldstart/assets
gh release download vX.Y.Z --repo LucasVascovici/velo \
  --pattern 'SHA256SUMS-bindings.txt' \
  --pattern '*.whl' --pattern 'velo-core-*.tgz'

# Verify before installing. A failure on any listed file stops the run.
sha256sum --check --ignore-missing SHA256SUMS-bindings.txt   # shasum -a 256 -c on macOS
```

Then, in an isolated environment per wedge:

```bash
# Python: use the wheel for your platform
python -m venv ~/coldstart/python/venv
~/coldstart/python/venv/bin/pip install ./velo_core-<version>-cp39-abi3-<platform>.whl

# Node: use the tarball for your platform (linux, macos or windows)
mkdir -p ~/coldstart/node/work && cd ~/coldstart/node/work && npm init -y
npm install ~/coldstart/assets/velo-core-<version>-<platform>.tgz
```

The Rust wedge uses the crate the agent adds to its own `Cargo.toml`, as the
README says to; do not hand it a path into a checkout. The CLI the checker
needs comes from `cargo install --git https://github.com/LucasVascovici/velo
--tag vX.Y.Z velo-cli`, or the release binary. The MCP wedge needs `velo-mcp`
built from the same tag, as the README says.

Install the checker's `velo` outside the agent's reach if you can, so the agent
is not helped by `--help` on a binary it was not given. If the agent was given
the CLI (the Rust and MCP wedges may need it), say so in the friction log.

## What the agent receives

Exactly these, copied into its empty working directory:

- `README.md` at the tag,
- the one cookbook for its wedge (the MCP wedge gets no cookbook; it gets only
  the README, whose MCP section is the documentation),
- the task card's one-sentence task, pasted as the prompt. The card's
  acceptance section is for the checker and is not shown to the agent.

It does not receive: the source tree, `docs/` other than its cookbook,
`ARCHITECTURE.md`, example scripts, or any hint beyond the sentence. Linked
pages in the README are not available to it; if it asks for one, that is a
finding. Let it use the web only if you are testing that on purpose, and record
it.

Give it a fixed actor name to use for the work, from the card, so the checker
can look for it in blame.

## Time bound

45 minutes per wedge, wall clock, from the moment the prompt is given. Stop the
agent at the bound and run the checker on what exists. A run that hits the
bound is recorded as "timed out", not retried.

## What to record

Per wedge, in [`friction-log.md`](friction-log.md):

- time to first successful `save` (minutes from the prompt),
- every question the agent had to guess at,
- every error it hit, quoted,
- every place it reached for a method, flag or tool that does not exist,
- whether the independent check passes.

Watch the agent's transcript rather than asking it afterwards: a guess it did
not mention is still a guess. Note the minute of each event.

## Independent check

`check.py` uses only the Python standard library and shells out to an installed
`velo`. It does not import the bindings and does not trust the agent's own
output:

```bash
python check.py <repo-dir> --actor <actor-name> --file <path-in-repo> [--velo /path/to/velo]
```

It asserts that `velo fsck` exits zero, that history shows at least one
snapshot, and that `velo blame` of the file the task produced names the actor.
It prints PASS or FAIL for each and exits non-zero on any failure. The card
says which file and actor to pass.

## After the run

1. Fill one row per event in the friction log. The "doc or API defect" column
   says which it is; "proposed fix" is a sentence; "deferred or fixed" starts
   as `deferred` and changes when a commit fixes it (name the commit).
2. Fix the doc defects that need no design, in the next release.
3. Fill in "Adapter decision" below.

## Adapter decision

ARCHITECTURE 19.9 chooses the first framework adapter from what the cold-start
agents reach for, not from a guess. Owner: fill this in after the run.

- Frameworks, libraries or APIs the agents reached for unprompted:
- Count of agents that reached for each:
- Chosen adapter, and the evidence for it:
- Date and release tag of the run:

## Re-running on 6.0

Run the whole protocol again against the 6.0 release tag, with fresh agents
and fresh directories: nothing from the first run's agents or working
directories is reused. Keep each run's friction log as its own file
(`friction-log-<tag>.md`, copied from the template) so the two can be compared
row by row. A defect from the earlier run that reappears is a regression in the
docs; mark it as such. For 6.0 also confirm the agent sees `save_changes`,
`velo report` and `velo sync` in what it is given.
