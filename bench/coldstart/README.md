# Cold-start test

Can an agent that has never seen this repository finish a real task using only
the README and one cookbook? This directory is the kit for finding out. It
holds the protocol, one task card per wedge, an independent checker and a
friction log template. The first run's results, against v6.2.1, are in
[`friction-log-v6.2.1.md`](friction-log-v6.2.1.md); round two, on v6.2.2 after the
documentation fixes, is [`friction-log-v6.2.2-round2.md`](friction-log-v6.2.2-round2.md).

The run needs fresh agents with no memory of the repository, so it cannot be
done by whoever implements velody. The owner runs it, once per release.

## What is under test

Four wedges, one card each in [`tasks/`](tasks):

| Wedge | Card | Documentation given besides README.md |
| --- | --- | --- |
| Agent checkpointing, Python | [`python-agent-checkpointing.md`](tasks/python-agent-checkpointing.md) | `docs/LIBRARY.md`, `docs/cookbook/agent-checkpointing.md` |
| Config registry, Node | [`node-config-registry.md`](tasks/node-config-registry.md) | `docs/LIBRARY.md`, `docs/cookbook/config-registry.md` |
| Document editor, Rust | [`rust-document-editor.md`](tasks/rust-document-editor.md) | `docs/LIBRARY.md`, `docs/cookbook/document-editor.md` |
| MCP client | [`mcp-client.md`](tasks/mcp-client.md) | `docs/LIBRARY.md`'s "MCP server" section only |

The task is done from an installed wheel or tarball, not from a checkout. The
documents the agent receives must show `save_changes`, `velody report` and
`velody sync`; check that before each run (see "Before you start").

## Pass threshold (fixed 2026-10-23, before the first run)

A wedge counts as passing when **two independent runs reach a green
`check.py` inside the 45-minute limit**. This number was written down before
any run so it cannot be adjusted afterwards to fit the results.

- Every wedge is run at least twice, each time by a different fresh agent with
  no memory of an earlier run.
- A run that times out at 45 minutes is a result and is recorded, not retried.
- A run that read the repository is invalid and is repeated, not counted
  (see "Audit" in the runbook).

## Limits

A software sandbox cannot isolate an agent from its training data: an agent
may know velody from before the run, and the audit only sees what a transcript
shows. Isolation here means an empty directory outside any checkout, the
documents handed over as text and the installed package. State this limit in
every results write-up.

## Runbook

All scripts use only the Python standard library.

1. **Prepare**: `python prepare.py --tag vX.Y.Z --wedge python --run 1 --root ~/coldstart`
   builds `~/coldstart/python-run1/` (refuses a root inside a git work tree),
   downloads and checksum-verifies the release assets, installs the package,
   copies the documents in as text, and writes `PROMPT.txt` and `run.json`. It
   prints the term check; a missing term is a doc-defect row for the log.
2. **Run**: give a fresh agent `PROMPT.txt` and the `work/` directory, and
   nothing else. Start the clock when the prompt is given; stop at 45 minutes.
3. **Audit**: `python audit.py transcript.jsonl --sandbox ~/coldstart/python-run1`.
   Exit 1 means invalid: a read of the project's planning notes, the source or the tests,
   or any path under a velody checkout. Repeat the run with a new agent. Other
   paths outside the sandbox are warnings to read, not to count.
4. **Check**: `python check.py <repo-dir> --actor <actor> --file <file>` with
   the values in `run.json`.
5. **Log**: one row per event in a copy of `friction-log.md`, named
   `friction-log-<tag>.md`, with the run's validity in the summary table.
6. **Rank**: `python rank.py friction-log-<tag>.md` and paste the table into
   the `## Ranked` section. Defects are ranked by distinct runs that hit them.
7. **Fix**: fix the doc defects that need no design, highest rank first.
8. **Round two**: repeat steps 1 to 6 at the fixed tag with new agents, into
   `friction-log-<tag>-round2.md`. Keep both logs.

## Before you start

1. Pick the release tag to test (`vX.Y.Z`).
2. Confirm the pages handed over at that tag (`README.md`, `docs/LIBRARY.md`,
   `docs/CLI.md`, `docs/COLLABORATION.md` and the cookbooks) mention `save_changes`,
   `velody report` and `velody sync`:
   `for f in README.md docs/LIBRARY.md docs/CLI.md docs/COLLABORATION.md docs/cookbook/*.md; do git show vX.Y.Z:$f | grep -c -E "save_changes|velody report|velody sync"; done`
   and check that the pages the wedge uses are among those that match. A missing term is itself a
   finding: write it in the friction log as a doc defect.
3. Make one fresh directory per wedge, outside any checkout, for example
   `~/coldstart/<wedge>/work`. Do not run an agent inside a clone of this
   repository.

## Install from release assets

Install what a user would: the files attached to the GitHub release. Nothing is
on PyPI or npm.

```bash
mkdir -p ~/coldstart/assets && cd ~/coldstart/assets
gh release download vX.Y.Z --repo LucasVascovici/velody \
  --pattern 'SHA256SUMS-bindings.txt' \
  --pattern '*.whl' --pattern 'velody-core-*.tgz'

# Verify before installing. A failure on any listed file stops the run.
sha256sum --check --ignore-missing SHA256SUMS-bindings.txt   # shasum -a 256 -c on macOS
```

Then, in an isolated environment per wedge:

```bash
# Python: use the wheel for your platform
python -m venv ~/coldstart/python/venv
~/coldstart/python/venv/bin/pip install ./velody_core-<version>-cp39-abi3-<platform>.whl

# Node: use the tarball for your platform (linux, macos or windows)
mkdir -p ~/coldstart/node/work && cd ~/coldstart/node/work && npm init -y
npm install ~/coldstart/assets/velody-core-<version>-<platform>.tgz
```

The Rust wedge uses the crate the agent adds to its own `Cargo.toml`, as the
README and `docs/INSTALL.md` say to; do not hand it a path into a checkout. The CLI the checker
needs comes from `cargo install --git https://github.com/LucasVascovici/velody
--tag vX.Y.Z velody-cli`, or the release binary. The MCP wedge installs the
`velody-mcp-<target>` release archive (and the `velody-<target>` one for the CLI):
download both with `gh release download`, then verify them against
`SHA256SUMS.txt` (`sha256sum --check --ignore-missing SHA256SUMS.txt`) before
extracting. Do not build `velody-mcp` from source.

Install the checker's `velody` outside the agent's reach if you can, so the agent
is not helped by `--help` on a binary it was not given. If the agent was given
the CLI (the Rust and MCP wedges may need it), say so in the friction log.

## What the agent receives

Exactly these, copied into its empty working directory:

- `README.md` and `docs/LIBRARY.md` at the tag,
- the one cookbook for its wedge (the MCP wedge gets no cookbook; it gets only
  README.md and docs/LIBRARY.md, whose MCP section is the documentation),
- the task card's one-sentence task, pasted as the prompt. The card's
  acceptance section is for the checker and is not shown to the agent.

It does not receive: the source tree, `docs/` other than `docs/LIBRARY.md` and its cookbook,
the source or the tests, example scripts, or any hint beyond the sentence. Linked
pages beyond those are not available to it; if it asks for one, that is a
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
`velody`. It does not import the bindings and does not trust the agent's own
output:

```bash
python check.py <repo-dir> --actor <actor-name> --file <path-in-repo> [--velody /path/to/velody]
```

It asserts that `velody fsck` exits zero, that history shows at least one
snapshot, and that `velody blame` of the file the task produced names the actor.
It prints PASS or FAIL for each and exits non-zero on any failure. The card
says which file and actor to pass.

## After the run

1. Fill one row per event in the friction log. The "doc or API defect" column
   says which it is; "proposed fix" is a sentence; "deferred or fixed" starts
   as `deferred` and changes when a commit fixes it (name the commit).
2. Fix the doc defects that need no design, in the next release.
3. Fill in "Adapter decision" below.

## Adapter decision

The adapter decision in `docs/ROADMAP.md` chooses the first framework adapter
from what the cold-start agents reach for, not from a guess. Owner: fill this in after the run.

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
`velody report` and `velody sync` in what it is given.
