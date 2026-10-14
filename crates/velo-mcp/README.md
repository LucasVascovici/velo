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

## Store-only mode

An agent that has no checkout can still use velo. Point the server at a
single-file repository instead of a project directory:

```json
{"command": "velo-mcp", "args": ["--store", "/path/agent.velo", "--create"]}
```

`--create` makes the file when it is missing; without it the file must exist
(and is migrated if it is older). `--store` conflicts with `--repo`. Store mode
is chosen whenever the repository has no working tree, and nothing is ever
written outside the database file (plus its `-wal`, `-shm` and `.lock` files).

Content arrives in the call and trees come back by id:

| Tool | What it does |
| --- | --- |
| `velo_save_tree` | Record a snapshot from `files` (`{path: content}`, `null` deletes a path). By default the files are laid over the tree of `base` (default: the parent), so only changes are sent; `replace: true` makes `files` the whole tree. `encoding` is `utf8` (default) or `base64` and applies to every value. `parent` defaults to the branch tip, a root snapshot on an unborn branch. |
| `velo_tree_at` | The files of a snapshot as `{path, object, kind}`. |
| `velo_read_file` | One file of a snapshot as `{content, encoding}`; non-UTF-8 content comes back base64. |

These three are listed in both modes. In store mode `tools/list` omits the tools
that need a working tree (`velo_save`, `velo_restore`, `velo_status`,
`velo_diff`, `velo_merge_apply` and `velo_branch` `switch`); calling one returns
an `isError: true` result that names `Unsupported` and the store tool to use.
`velo_history`, `velo_metadata`, `velo_blame`, `velo_merge_plan` and branch
`list` and `create` work as usual.

Example session:

1. Create the first snapshot (the whole call is one line):
   `velo_save_tree {"branch": "attempt-1", "message": "start", "files": {"app.py": "print(1)\n", "notes.md": "todo\n"}}`
   returns the snapshot id.
2. Change one file and delete the other:
   `velo_save_tree {"branch": "attempt-1", "message": "cache", "files": {"app.py": "print(2)\n", "notes.md": null}}`
   carries the previous tree forward.
3. `velo_read_file {"snapshot": "attempt-1", "path": "app.py"}` reads it back.
4. `velo_history {"meta": [{"namespace": "mcp", "key": "run"}]}` lists both
   snapshots with this run's identity.

A velo error is a tool result with `isError: true`; protocol faults are JSON-RPC
errors. Only protocol lines go to stdout; diagnostics go to stderr.
