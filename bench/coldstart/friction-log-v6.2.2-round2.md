# Friction log, round two

Release tag `v6.2.2`, date 2026-10-26. Two runs per wedge, one fresh agent each,
new sandboxes (`prepare.py --tag v6.2.2`), the documentation as fixed after
round one. Same protocol, prompts and threshold as
[`friction-log-v6.2.1.md`](friction-log-v6.2.1.md), which is kept; the rows
below are comparable with it by defect name.

Kind is `guess`, `error` or `missing method` as in round one. Rows come from
the transcripts and the agents' own "what I had to guess" reports. No run hit a
velo runtime error; the two compile-error trails in the Rust runs are logged as
the guesses they came from.

| Run | Wedge | Minute | Kind | Quote | Defect | Doc or API defect | Proposed fix | Deferred or fixed |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 2 | Python | - | guess | "the cookbook links an example script ... which isn't in the docs folder, so I couldn't compare against it" | linked-pages-not-handed-over | doc | Handed-over pages link to files the reader does not have; nothing needed from them was missing, but the links are dead in a copy. Use absolute GitHub URLs for the example scripts | deferred |
| 1 | Node | - | guess | "the cookbook points to `bindings/node/examples/config_registry.mjs`, but that file isn't in the docs I was given" | linked-pages-not-handed-over | doc | as above | deferred |
| 2 | Node | - | guess | "The cookbook points to `bindings/node/examples/config_registry.mjs`, which isn't in the docs I was given" | linked-pages-not-handed-over | doc | as above | deferred |
| 1 | MCP | - | guess | "`work\\docs` only contains `LIBRARY.md`. `docs\\CLI.md`, the cookbook and the velo-mcp README that `LIBRARY.md` links to were not there" | linked-pages-not-handed-over | doc | as above | deferred |
| 1 | MCP | - | guess | "`LIBRARY.md` doesn't list `velo_save_tree`, `velo_tree_at`, `velo_read_file` or `velo_read_files`, but `tools/list` returns them" | mcp-store-tools-not-listed | doc | List the four store-mode tools in the MCP section | fixed after round two (docs) |
| 2 | MCP | - | guess | "`LIBRARY.md`'s tool list omits `velo_save_tree`, `velo_tree_at`, `velo_read_file` and `velo_read_files`" | mcp-store-tools-not-listed | doc | as above | fixed after round two (docs) |
| 2 | MCP | - | guess | "`LIBRARY.md` never says how tool results are shaped. I guessed they arrive in `result.structuredContent`" | mcp-result-shape-undocumented | doc | Say a result is in `result.structuredContent` and as JSON text in `result.content` | fixed after round two (docs) |
| 1 | MCP | - | guess | "`velo_save` ... snapshots the whole working tree, so my script files were swept into the first save" | mcp-save-whole-tree-surprise | doc | Say `velo_save` snapshots the whole tree; keep scripts outside or in `.veloignore` | fixed after round two (docs) |
| 1 | MCP | - | guess | "`velo_branch create` with no `at` gave `at: null`, and `velo_save_tree` onto it then made a parentless root snapshot" | mcp-branch-create-without-at | doc | Say an unborn branch results; pass `at` to start from a snapshot | fixed after round two (docs) |
| 2 | MCP | - | guess | "`velo_branch create`: it returned `at: null`, and I assumed that means the branch starts unborn" | mcp-branch-create-without-at | doc | as above | fixed after round two (docs) |
| 1 | MCP | - | guess | "the CLI `--where` filter listed only `main`, while `velo_find` covered all branches" | history-where-scope | doc | Say `velo history --where` lists the current branch; add `--all` | fixed after round two (docs) |
| 1 | Python | - | guess | "The docs never say whether `save_changes` with no parent works on an empty repo ... I always passed `parent` explicitly after the first call" | save-parent-semantics-undocumented | doc and API | Say `parent` is required after the first save and that none starts a new history (verified: it moves the branch to a root holding only the passed changes) | fixed after round two (docs); a refusal on a branch that already has a tip is deferred |
| 2 | Python | - | guess | "the docs show no way to create one [a base snapshot], so I used `save_tree` on `main` with no parent" | save-parent-semantics-undocumented | doc and API | as above | fixed after round two (docs) |
| 1 | Python | - | guess | "I took 'actor name `coldstart-py` for the run' to mean `run` equals `coldstart-py`" | actor-author-vs-run-unclear | doc | Round one's `Who` bullet is in `LIBRARY.md`; the task sentence itself is ambiguous, so this is the card, not the page | deferred (task card) |
| 2 | Python | - | guess | "the task says `coldstart-py` authors every snapshot, so no snapshot has a separate human author" | actor-author-vs-run-unclear | doc | as above | deferred (task card) |
| 1 | Node | - | guess | "the cookbook never shows an author for Node. I passed `author: { name: 'coldstart-node' }`" | node-author-shape-undocumented | doc | Show `author: { name }` and `meta` on the Node calls | fixed after round two (docs) |
| 2 | Node | - | guess | "I used `author: { name: 'coldstart-node' }` from the `AuthorInput` type. The cookbook only shows the Python `velo.Author(...)`" | node-author-shape-undocumented | doc | as above | fixed after round two (docs) |
| 1 | Node | - | guess | "the cookbook only says to give the rollback `rollback_of` and the approver" (the rollback's own `version` value) | rollback-metadata-undocumented | doc | Give the rollback's metadata in the cookbook's Node snippet | deferred |
| 2 | Node | - | guess | "The rollback's own `version` value (`1.1.0-rollback`) is my invention" | rollback-metadata-undocumented | doc | as above | deferred |
| 1 | Node | - | guess | "to show the forged snapshot's different id, I saved it to a separate `forged` branch" | tamper-demo-branch-unspecified | doc | Say where the tamper demonstration's second snapshot goes (its own branch) | deferred |
| 2 | Node | - | guess | "the tamper check publishes the forged snapshot to a separate `forge` branch, since the docs don't say which branch to use" | tamper-demo-branch-unspecified | doc | as above | deferred |
| 1 | Rust | 0.1 | guess | "The docs only say `tag = \"vX.Y.Z\"`. I ran `git ls-remote --tags` ... and used the latest tag, `v6.2.2`" | rust-version-tag-unspecified | doc | Say to use the newest release tag and how to find it | fixed after round two (docs) |
| 2 | Rust | 0.1 | guess | "`LIBRARY.md` only says `tag = \"vX.Y.Z\"`. I listed the repo's tags with `git ls-remote` and used `v6.2.2`" | rust-version-tag-unspecified | doc | as above | fixed after round two (docs) |
| 1 | Rust | - | error | "Rust wants `&[(PathBuf, PathBuf)]` [for `renames`]. I found that from the compiler error" | rust-save-fields-undocumented | doc | State `renames: &[(PathBuf, PathBuf)]` and the public fields of `SaveChanges` and `SaveTree` | fixed after round two (docs) |
| 2 | Rust | - | error | "the field is `renames: &[(PathBuf, PathBuf)]`, so the `String` pairs I first wrote failed to compile" | rust-save-fields-undocumented | doc | as above | fixed after round two (docs) |
| 1 | Rust | - | guess | "I guessed that `save.author = Some(&author)` is a public field, and it compiled" | rust-save-fields-undocumented | doc | as above | fixed after round two (docs) |
| 2 | Rust | - | guess | "I assumed `SaveChanges` has a public `author: Option<&Author>` field, and it compiled" | rust-save-fields-undocumented | doc | as above | fixed after round two (docs) |
| 1 | Rust | - | guess | "The docs don't say which module exports `Author` or `SaveChanges`" | rust-import-paths-unspecified | doc | List where each type lives | fixed after round two (docs) |
| 2 | Rust | - | guess | "`BranchName`, `Author` and `Repo` come from the crate root, and `branches`, `blame` and `history` come from `velo_core::commands`. This is inferred from the examples" | rust-import-paths-unspecified | doc | as above | fixed after round two (docs) |
| 2 | Rust | - | guess | "The docs don't say whether the write guard must be dropped before the next `repo.write()`" | rust-write-guard-lifetime | doc | Say a guard holds the write lock until dropped, so scope it or let it be a temporary | deferred |
| 1 | Python | - | guess | "The Python stubs list `branch_tip`, but the docs don't say whether `save_changes` picks up the tip by default" | save-parent-semantics-undocumented | doc and API | as above | fixed after round two (docs) |

## Round one defects that did not come back

Round one's ranked list had 20 defects. Judged by the quotes in this round's
transcripts and reports, these did not recur: `api-shapes-from-stubs-not-docs`
(no agent said it read the type stubs for return types; the Python and Node
runs used the page), `no-undo-in-library-docs` (python run 1 built the
documented undo, run 2 reverted by snapshot as the page now says),
`rust-api-beyond-save-undocumented` (both Rust runs built branch, merge, blame
and rename from the page; their compile errors were about field and type
shapes the page did not yet state, which are new rows below), `mcp-run-id-undocumented`, `mcp-author-is-client-name`,
`mcp-stdio-framing-undocumented` and `mcp-init-unclear` (all four MCP reports
mention none of them), `node-rollback-example-missing`,
`cookbook-tags-statement-stale`, `docs-version-stale`,
`one-sqlite-file-wording`, `history-branch-scope-surprise` (node run 1 cites
the page's own history advice) and `merge-meta-undocumented`. Of round one's top five, four are gone and the fifth,
`api-reference-not-reachable`, survives as `linked-pages-not-handed-over`.

## Term check (prepare.py)

| Page | Missing |
| --- | --- |
| `README.md` | `save_changes`, `velo sync` |
| `docs/cookbook/agent-checkpointing.md` | `velo sync` |
| `docs/cookbook/config-registry.md` | `velo sync` |

The README is a one-screen front page and does not carry these terms by design;
`docs/LIBRARY.md`, which every run was given, has all three.

## Per-run summary

`audit.py` was corrected between the rounds: it had flagged every round-one
transcript because it scanned the host's own preamble records and the tool
results (which echo the pages the agents read), and it counted a quoted link in
an agent's prose as a read. It now scans only the agent's own tool calls. Under
it all sixteen runs, both rounds, are valid: no tool call touched a path under a
velo checkout or a planning file, and neither Rust run read the cargo-downloaded
crate. The out-of-sandbox warnings are fragments of sandbox-relative paths and
the two documented URLs.

Minutes are from the prompt to the agent's last record.

| Run | Wedge | Validity | Minutes to first successful save | Timed out at 45 min | check.py result | Notes |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | Python agent checkpointing | valid | under 2 | no | PASS | `velo-repo`, 4 snapshots, file `app.py`; undo recipe followed |
| 2 | Python agent checkpointing | valid | under 2 | no | PASS | `velo-repo`, 7 snapshots; two attempt branches, merge with author and metadata |
| 1 | Node config registry | valid | under 2 | no | PASS (`--at registry`) | `registry-repo`; signed versions, rollback, a forged branch |
| 2 | Node config registry | valid | under 2 | no | PASS (`--at registry`) | `registry-repo`; same |
| 1 | Rust document editor | valid | under 5 | no | PASS | `editor/essay-repo`; branch, a real conflict and its resolution, blame names both authors |
| 2 | Rust document editor | valid | under 5 | no | PASS | `editor/velo-repo`; rename edge, draft branch, merge |
| 1 | MCP client | valid | under 2 | no | PASS (`--actor coldstart-age --run coldstart-mcp`) | `repo`; author is the client name, run id on 4 snapshots |
| 2 | MCP client | valid | under 2 | no | PASS (`--actor coldstart-age --run coldstart-mcp`) | `project`; run id on 3 snapshots |

**Threshold** (fixed 2026-10-23): a wedge passes when two independent runs reach
a green `check.py` inside 45 minutes. **All four wedges pass again.**

**Comparison with round one.** Both rounds pass every wedge. Distinct defects
fell from 20 to 15 and friction rows from 40 to 31; the two lists share one
defect name (`actor-author-vs-run-unclear`, which is the task card's wording, not
a page), and the defects that remain are different in kind: round one's were
missing facts (how to set a run id, what a call returns, how to undo), round
two's are mostly links to pages the test did not hand over, details of the
Node and Rust call shapes, and two behaviours (`parent` semantics, whether a
guard must be dropped). The round-one top-ranked defects are gone except
the unreachable linked pages.

**Limit, stated.** Agents are the only evidence. They read the installed type
stubs and `--help` when a page ran out, as a person would, and "passed"
includes recovering from the gaps above. No outside person has used velo.

## Ranked

`python rank.py friction-log-v6.2.2-round2.md`:

| Defect | Runs hit | Kinds | Wedges |
| --- | --- | --- | --- |
| linked-pages-not-handed-over | 4 | guess | MCP, Node, Python |
| actor-author-vs-run-unclear | 2 | guess | Python |
| mcp-branch-create-without-at | 2 | guess | MCP |
| mcp-store-tools-not-listed | 2 | guess | MCP |
| node-author-shape-undocumented | 2 | guess | Node |
| rollback-metadata-undocumented | 2 | guess | Node |
| rust-import-paths-unspecified | 2 | guess | Rust |
| rust-save-fields-undocumented | 2 | error, guess | Rust |
| rust-version-tag-unspecified | 2 | guess | Rust |
| save-parent-semantics-undocumented | 2 | guess | Python |
| tamper-demo-branch-unspecified | 2 | guess | Node |
| history-where-scope | 1 | guess | MCP |
| mcp-result-shape-undocumented | 1 | guess | MCP |
| mcp-save-whole-tree-surprise | 1 | guess | MCP |
| rust-write-guard-lifetime | 1 | guess | Rust |

## Found by this round and not by the agents

`save_changes` and `save_tree` with no `parent` on a branch that already has
commits do not refuse and do not carry the old tree: they start a new root
holding only what was passed, and the branch tip moves to it. Verified with the
Python binding (a second `save_changes(changes={"m.txt": ...})` produced a tree
of `m.txt` alone, parentless, and `branch_tip` pointed at it). Two Python agents
avoided it by always passing `parent`; the page now says so. Refusing a
parent-less save on a branch that has a tip is an API change and is deferred.

## Adapter decision

- Frameworks, libraries or APIs the agents reached for unprompted: none, again.
- Chosen adapter: none. The rule in `docs/ROADMAP.md` stands.
