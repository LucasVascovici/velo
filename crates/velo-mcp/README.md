# velo-mcp

A synchronous stdio [Model Context Protocol](https://modelcontextprotocol.io)
server that makes velo the checkpoint layer for any agent, with no integration
code on the agent author's side. There is no async runtime: the core has none,
and one request at a time is all a stdio client sends.

## Which half of the API

An agent edits files in a project directory, so `velo-mcp` serves a repository
*with* a working tree (`--repo <dir>`, default the current directory, opened with
`Repo::discover`). It therefore uses the working-tree commands `save`, `restore`,
`status` and `diff`. That is legitimate: `velo-mcp` is a consumer like the CLI,
not a language binding, so it is not limited to the embeddable half.

## Client configuration

```json
{"command": "velo-mcp", "args": ["--repo", "/path"]}
```

## Run identity

The run id is `--run <id>`, else `$VELO_MCP_RUN`, else `run-<epoch_ms>`. Every
write tool records metadata in namespace `mcp`: `run`, `tool`, `client` and,
when the call supplies a `tool_call_id`, `tool_call_id`. Agents cannot write to
the `velo` or `mcp` namespaces themselves. The author is the client name, or
`$VELO_AUTHOR_NAME` / `$VELO_AUTHOR_EMAIL` when set.

## Tools

| Tool | What it does |
| --- | --- |
| `velo_save` | Save a snapshot; records the run in `mcp` metadata. |
| `velo_restore` | Restore the tree or some paths; refuses on unsaved changes. |
| `velo_status` | Branch, position and unsaved changes. |
| `velo_diff` | Diff two snapshots, or a snapshot against the tree. |
| `velo_history` | Snapshots, newest first, filterable by metadata (such as run). |
| `velo_metadata` | Author and all metadata of a snapshot. |
| `velo_branch` | `list`, `create` (does not switch) or `switch` (creates the branch if missing, as the CLI does; refuses on a dirty tree). |
| `velo_merge_plan` | What merging `source` into the current branch would do, with conflict texts and an `apply_hint`. Writes nothing. |
| `velo_merge_apply` | Record the merge as a two-parent snapshot and move the tree to it. Needs a clean tree and a resolution for every conflict. |
| `velo_blame` | Per-line origin; `origin.run` is the run that saved the line. |

There is no force option anywhere: a dirty tree is an error result
(`DirtyWorkingTree`) and nothing is overwritten. Merging is always plan, then
apply; there is no single-call merge.

### Example agent session

1. `velo_branch {"action": "create", "name": "attempt-1"}` then
   `{"action": "switch", "name": "attempt-1"}`.
2. Edit files, then `velo_save {"message": "try a cache"}`.
3. `velo_branch {"action": "switch", "name": "main"}`.
4. `velo_merge_plan {"source": "attempt-1"}` shows each file's action, and for a
   conflict its base, ours and theirs text.
5. `velo_merge_apply {"source": "attempt-1", "message": "take the cache",
   "resolutions": {"app.py": {"content": "..."}}}`. Without resolutions a
   conflict returns `Conflicts` listing the paths.
6. `velo_blame {"path": "app.py"}`: each line's `origin.run` names the run that
   wrote it.

A velo error is a tool result with `isError: true`; protocol faults are JSON-RPC
errors. Only protocol lines go to stdout; diagnostics go to stderr.
