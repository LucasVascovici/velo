# Friction log

Release tag `v6.2.1`, date 2026-10-24. Two runs per wedge, one fresh agent each.

Each run was a separate agent session started in its own sandbox folder
(`prepare.py` layout), outside any checkout, with no project memory and no
project instructions. It received the task card's sentence and actor, one
line saying where the installed package (or the release binaries) and the
documents were, a request to stay inside the sandbox, the 45-minute limit, and
a request to report the repository path, a changed file and what it had to
guess. The agents ran concurrently; none read or messaged another (checked in
the transcripts).

Kind is one of `guess` (the agent had to guess at something the docs did not
say), `error` (a command or call failed) or `missing method` (it reached for a
method, flag or tool that does not exist). Rows come from the transcripts and
the agents' own "what I had to guess" reports; Rust compile errors are logged
as the guesses they came from. No run hit a velo runtime error.

| Run | Wedge | Minute | Kind | Quote | Defect | Doc or API defect | Proposed fix | Deferred or fixed |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 1 | Python | - | guess | "the docs never say how to undo" | no-undo-in-library-docs | doc | Show undo in the library: a snapshot of the parent's tree, or `set_branch_tip`, with when to use which | deferred |
| 2 | Python | - | guess | "the docs say 'undo' but show no undo call for the Python library" | no-undo-in-library-docs | doc | as above | deferred |
| 1 | Python | - | guess | "I found these by inspecting the installed package, not the docs" | api-shapes-from-stubs-not-docs | doc | Give return types in the library page (`TreeFile`, `Entry`, `Saved`) and link the API reference | deferred |
| 2 | Python | - | guess | "I read signatures and docstrings from the installed package's `__init__.pyi`" | api-shapes-from-stubs-not-docs | doc | as above | deferred |
| 1 | Node | - | guess | "I relied on the installed package's README and type declarations" | api-shapes-from-stubs-not-docs | doc | as above, for Node | deferred |
| 2 | Node | - | guess | "I used the installed package's README and `index.d.ts` as the API reference" | api-shapes-from-stubs-not-docs | doc | as above, for Node | deferred |
| 1 | Rust | 2.1 | missing method | `error[E0599]: no method named create_branch found for struct WriteGuard` | rust-api-beyond-save-undocumented | doc | A Rust section per cookbook operation: branch, merge (`commands::merge::plan` and `merge::commit`), authors | deferred |
| 2 | Rust | 2.2 | guess | `expected &Author, found &Result<Author, Error>` | rust-api-beyond-save-undocumented | doc | as above | deferred |
| 1 | Python | - | guess | "the Python binding's README and the API reference that the docs point to aren't in `work`" | api-reference-not-reachable | doc | Publish the API reference somewhere a reader can open (the docs site is built but its deploy is off) | deferred: owner decision (Pages) |
| 2 | Python | - | guess | "the cookbook links to an example script and a binding README that aren't in the docs folder" | api-reference-not-reachable | doc | as above | deferred |
| 1 | Node | - | guess | "Many linked files aren't in the docs folder: the Node port of the cookbook script, STABILITY.md, INTEGRITY.md and the API reference zips" | api-reference-not-reachable | doc | as above | deferred |
| 2 | Node | - | guess | "the docs refer to several pages (CLI.md, INSTALL.md, INTEGRITY.md, STABILITY.md and others) that aren't in the folder" | api-reference-not-reachable | doc | as above | deferred |
| 1 | MCP | - | guess | "The docs link to the velo-mcp README for the tool list, but that file wasn't in the folder" | mcp-tools-not-in-docs | doc | List the tools and their arguments in the MCP section itself | deferred |
| 2 | MCP | - | guess | "These point to `crates/velo-mcp/README.md#tools`, which isn't in the docs folder" | mcp-tools-not-in-docs | doc | as above | deferred |
| 1 | MCP | - | guess | "`docs/LIBRARY.md` says writes are 'recorded against a run id' but not how to set it" | mcp-run-id-undocumented | doc | Say: `--run ID` or `VELO_MCP_RUN`, else generated; stored as metadata `mcp/run` | deferred |
| 2 | MCP | - | guess | "I found the `--run` flag (or `$VELO_MCP_RUN`) only in `velo-mcp --help`" | mcp-run-id-undocumented | doc | as above | deferred |
| 1 | MCP | - | guess | "I guessed `mcp`/`run` as the metadata key to search on" | mcp-run-id-undocumented | doc | as above, with `history --where mcp:run=ID` | deferred |
| 2 | MCP | - | guess | "The filter key for finding a run's snapshots. I guessed it was `mcp`/`run`" | mcp-run-id-undocumented | doc | as above | deferred |
| 1 | MCP | - | guess | "The server records the name my client sends at startup ... as the author. The docs never mention this" | mcp-author-is-client-name | doc | Say the author is `clientInfo.name` and the run id is metadata, so blame shows the client and `--where` finds the run | deferred |
| 2 | MCP | - | guess | "The snapshot author is taken from the `clientInfo.name` my client sends" | mcp-author-is-client-name | doc | as above | deferred |
| 1 | MCP | - | guess | "The docs don't say how messages are framed" | mcp-stdio-framing-undocumented | doc | One line: newline-delimited JSON-RPC over stdio, and the protocol versions accepted | deferred |
| 2 | MCP | - | guess | "I assumed one JSON message per line and sent protocolVersion `2025-06-18`" | mcp-stdio-framing-undocumented | doc | as above | deferred |
| 1 | Python | - | guess | "the docs don't fix the metadata shape for an actor. I used `coldstart-py` as both the `run` value and the author name" | actor-author-vs-run-unclear | doc | Say which of author and metadata answers "who" and which answers "which run", with one example of each | deferred |
| 2 | Python | - | guess | "I read it as both `Author(\"coldstart-py\")` and the metadata `run` value" | actor-author-vs-run-unclear | doc | as above | deferred |
| 1 | Node | - | guess | "The cookbook shows rollback only in Python (`TreeEntry.stored`)" | node-rollback-example-missing | doc | Give the Node form of rollback: `saveTree` entries of `{path, object, kind}` | deferred |
| 2 | Node | - | guess | "the cookbook shows rollback only in Python ... I guessed it maps to `saveTree` entries of `{path, object, kind}`" | node-rollback-example-missing | doc | as above | deferred |
| 1 | Node | - | guess | "The cookbook says the Python binding has no tags. The Node binding has `createTag`" | cookbook-tags-statement-stale | doc | Say which bindings have tags, and use them in the registry example | deferred |
| 2 | Node | - | guess | "The cookbook says to store the version in metadata because Python has no tags. The Node binding does have tags" | cookbook-tags-statement-stale | doc | as above | deferred |
| 1 | Rust | - | guess | "the docs only show `cargo install … velo-cli` ... For the library I used `cargo add velo-core --git ...`" | rust-dependency-line-missing | doc | Give the `Cargo.toml` line for `velo-core` from git with a tag | deferred |
| 1 | Rust | 2.8 | guess | "`&History` is not an iterator" | rust-api-beyond-save-undocumented | doc | Show `.entries` in the history example | deferred |
| 2 | Rust | 3.3 | guess | "expected `&SnapshotId`, found `&Saved`" | rust-api-beyond-save-undocumented | doc | Show `save_changes(...)?.snapshot` | deferred |
| 2 | Python | - | guess | "the docs describe 6.2.0 and `api-python.zip`, but neither was available" | docs-version-stale | doc | Generate version strings in the docs from the workspace version | deferred |
| 2 | MCP | - | guess | "`LIBRARY.md` also gives the download name as `.tar.gz` while the files I was given are `.zip`, and the README still calls it 'v4'" | docs-version-stale | doc | as above; name the Windows `.zip` | deferred |
| 2 | Node | - | guess | "The docs describe velo as 'one SQLite file', so I didn't expect those" (`.velo/` and `.veloignore`) | one-sqlite-file-wording | doc | Say "a `.velo` directory" (or `--objects database` for one file) | deferred |
| 1 | Python | - | guess | "`history(branch=...)` lists only the snapshots made since the branch forked" | history-branch-scope-surprise | doc | In the library page, say `branch` lists what was recorded on the branch and `from_` gives its ancestry | deferred |
| 2 | Python | - | guess | "the docs never show [metadata on merges]. I passed `author=` and `meta=` to `merge_commit` based on the installed package's type hints" | merge-meta-undocumented | doc | Show `merge_commit` with author and metadata | deferred |
| 1 | Node | - | guess | "The Node snapshot info has no author field" | node-snapshot-info-no-author | API | Add the author to the Node snapshot info, as Python's `Entry` has it, or document where it is | deferred |
| 2 | Node | - | guess | "blame credits the current lines to the 'rollback to 1.1.0' snapshot, not the original 1.1.0 publish" | blame-after-rollback | doc | Say that a rollback is a new snapshot, so the approver of the restored content is in the rollback's metadata | deferred |
| 1 | MCP | - | guess | "Switching to a new branch reports `started_unborn` with `inherits`, which isn't explained" | mcp-switch-unborn-unexplained | doc | Explain `started_unborn` and `inherits` in the tool's result description | deferred |
| 1 | MCP | - | guess | "'Run `velo init` in the project first' didn't say whether `velo-mcp` creates the repository itself" | mcp-init-unclear | doc | Say that `velo-mcp` needs an existing repository (or `--store FILE --create`) | deferred |

## Found by the checker, not by the agents

The agents did not run the CLI against their repositories (the Python, Node and
Rust wedges were not given it), so these came from `check.py`:

| Finding | Kind | Disposition |
| --- | --- | --- |
| `velo history` on a directory repository written only through the library printed "No snapshots yet on branch 'main'" while `velo branches` showed `main` with a tip. Library writes never set `.velo/PARENT`, and the default scope read only that; core even reported the branch as unborn | velo bug | fixed in 6.2.2 (the default history now walks from the checked-out branch's tip when there is no position), with a regression test |
| `check.py` read `history` from the checked-out position, blamed only on the checked-out branch, and looked for the MCP run id in blame | checker | fixed: history across all branches, `--at BRANCH`, and `--run ID` checked through `history --where mcp:run=ID` |
| `velo blame` truncates the author column at 14 characters (`coldstart-cli…`), and `velo show` prints no author at all | CLI output | deferred |

## Term check (prepare.py)

The pages handed over were checked for `save_changes`, `velo report` and
`velo sync` before the runs:

| Page | Missing |
| --- | --- |
| `README.md` | `save_changes`, `velo sync` |
| `docs/cookbook/agent-checkpointing.md` | `velo sync` |
| `docs/cookbook/config-registry.md` | `velo sync` |

## Per-run summary

Validity is `valid`, or `invalid: <audit reason>` when `audit.py` exited 1;
an invalid run is repeated and not counted toward the threshold.

`audit.py` exited 1 on every transcript, and on inspection every match was a
false positive: the planning-file name it flagged appears only in the agent
host's own preamble records (its skill list and memory instructions), and `crates/velo-` only inside the
handed-over `docs/LIBRARY.md`, which links to `crates/velo-core`. No tool call
touched a path under a velo checkout or a planning file; the only paths outside
a sandbox in tool calls were the git dependency URL the Rust docs give, the
app's own notes folder (written, never read) and string fragments such as
`"\n"`. The runs are judged valid on that reading; the audit heuristic needs to
learn the preamble record types (deferred).

Minutes are from the prompt to the agent's last record; the check is the
corrected `check.py` above.

| Run | Wedge | Validity | Minutes to first successful save | Timed out at 45 min | check.py result | Notes |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | Python agent checkpointing | valid (see above) | under 1.5 | no | PASS | repo `work/agent-repo`, 6 snapshots, file `app.py` |
| 2 | Python agent checkpointing | valid (see above) | under 1.2 | no | PASS | repo `agent-repo`, 8 snapshots, file `app.py` |
| 1 | Node config registry | valid (see above) | under 1.0 | no | PASS (`--at registry`) | repo `registry-repo`, published on branch `registry`, signed versions, a forged branch to show verification fail |
| 2 | Node config registry | valid (see above) | under 0.9 | no | PASS (`--at registry`) | repo `registry`, signed versions with a trusted-keys file |
| 1 | Rust document editor | valid (see above) | under 3.7 | no | PASS | repo `editor/essay-repo`, file `essay.txt`; branch and merge done with `save_tree` and `merge_parent` because the Rust merge API was not found |
| 2 | Rust document editor | valid (see above) | under 4.7 | no | PASS | as run 1; eight compile errors on the way, all API-shape guesses |
| 1 | MCP client | valid (see above) | under 1.3 | no | PASS (`--actor coldstart-cli --run coldstart-mcp`) | repo `project`; run id on 3 snapshots; author is the client's name |
| 2 | MCP client | valid (see above) | under 1.5 | no | PASS (`--actor coldstart-cli --run coldstart-mcp`) | repo `project`; run id on 4 snapshots |

**Threshold** (fixed 2026-10-23, before the first run): a wedge passes when two
independent runs reach a green `check.py` inside 45 minutes. **All four wedges
pass.**

**Limit, stated.** Agents are the only evidence here. They show the steps are
findable and followable by a model given the README and one cookbook; they do
not show that a person who has never seen velo understands what it is for. The
agents also read the installed package's type stubs and `--help` when the docs
ran out, which a person would do too, so "passed" includes recovering from the
gaps listed above.

## Ranked

`python rank.py friction-log-v6.2.1.md`:

| Defect | Runs hit | Kinds | Wedges |
| --- | --- | --- | --- |
| api-reference-not-reachable | 4 | guess | Node, Python |
| api-shapes-from-stubs-not-docs | 4 | guess | Node, Python |
| actor-author-vs-run-unclear | 2 | guess | Python |
| cookbook-tags-statement-stale | 2 | guess | Node |
| docs-version-stale | 2 | guess | MCP, Python |
| mcp-author-is-client-name | 2 | guess | MCP |
| mcp-run-id-undocumented | 2 | guess | MCP |
| mcp-stdio-framing-undocumented | 2 | guess | MCP |
| mcp-tools-not-in-docs | 2 | guess | MCP |
| no-undo-in-library-docs | 2 | guess | Python |
| node-rollback-example-missing | 2 | guess | Node |
| rust-api-beyond-save-undocumented | 2 | guess, missing method | Rust |
| blame-after-rollback | 1 | guess | Node |
| history-branch-scope-surprise | 1 | guess | Python |
| mcp-init-unclear | 1 | guess | MCP |
| mcp-switch-unborn-unexplained | 1 | guess | MCP |
| merge-meta-undocumented | 1 | guess | Python |
| node-snapshot-info-no-author | 1 | guess | Node |
| one-sqlite-file-wording | 1 | guess | Node |
| rust-dependency-line-missing | 1 | guess | Rust |

## Adapter decision

- Frameworks, libraries or APIs the agents reached for unprompted: none. Every
  agent built directly on the velo binding or the MCP server it was given; no
  agent framework was mentioned in any transcript.
- Count of agents that reached for each: 0.
- Chosen adapter, and the evidence for it: none. The evidence does not pick one,
  so by the rule in `docs/ROADMAP.md` no adapter is built.
- Date and release tag of the run: 2026-10-24, v6.2.1.
