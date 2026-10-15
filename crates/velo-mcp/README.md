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
{"command": "velo-mcp", "args": ["--repo", "/path", "--lock-wait", "5"]}
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
| `velo_blame` | Per-line origin; `origin.run` is the run that saved the line. `start_line` and `end_line` (1-based, inclusive) return only that slice; an inverted range or one past the end of the file is an invalid-params error. |
| `velo_undo` | Undo the last snapshot on the current branch (shelved, not destroyed). Needs a working tree. |
| `velo_redo` | Bring back what the last `velo_undo` shelved. Needs a working tree. |
| `velo_tag` | `list`, `create` (refuses a taken name) or `delete` tags. Works in store-only mode. |
| `velo_conflicts` | The conflicts of a stopped merge as data: per path the ancestor, ours and theirs objects and each hunk with its decision. Needs a working tree. |
| `velo_find` | Snapshots matching `[namespace, key, value]` metadata filters, optionally on one `branch`. Works in store-only mode. |

There is deliberately no tool for squash, rebase, compact, retention, gc, force
push or anything else that rewrites pushed history, and no tool takes a `force`
argument: the safety stance is the tool surface. There is no force option anywhere: a dirty tree is an error result
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

## Concurrent agents

Two agents may serve one repository through two servers. Write tools wait for
the repository lock for `--lock-wait SECS` (default 5, fractions allowed, 0 to
fail at once). If another process still holds it, the result has `isError: true`
and names the holder in its text and in `structuredContent`:
`{"error": "Locked", "holder": {"pid": ..., "command": ..., "started_at_ms": ...,
"started_at": ...}}`. Retry the call later; nothing was written.

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

A velo error, and bad arguments, are tool results with `isError: true` (the
text of a bad-argument result starts `InvalidParams:` and says what was wrong, so
the model can correct the call); only malformed requests and unknown tool names
are JSON-RPC errors. Only protocol lines go to stdout; diagnostics go to stderr.

## Large results

No tool sends multi-megabyte text by accident. A cut result has
`truncated: true`, a `hint` on how to get the rest and the value to pass for it:

| Tool | Bound | Next page |
| --- | --- | --- |
| `velo_history` | `limit`, default 200 entries | `from` = `next_from` |
| `velo_blame` | `limit`, default 2000 lines | `start_line` = `next_start_line` |
| `velo_tree_at` | `limit`, default 5000 files; result is `{files, total, truncated}` | `offset` = `next_offset` |
| `velo_read_file` | `max_bytes`, default 1 MiB; result carries `size` | `offset` = `next_offset` (text is cut on a character boundary) |

## Protocol versions

`2025-11-25`, `2025-06-18`, `2025-03-26` and `2024-11-05` are accepted; the
client's version is echoed when supported, else the newest (`2025-11-25`).

## Tried clients

Each client below is run against the built server by
[`conformance/`](conformance) (how to run it locally is in its README) and, for
the first three, in CI (`.github/workflows/mcp.yml`).

| Client | Version | Transport | Result | What broke, what was fixed |
| --- | --- | --- | --- | --- |
| MCP Python SDK (`mcp`) | 2.3.0 | stdio | Passes: negotiates `2025-11-25`, every `inputSchema` is valid draft 2020-12, completes the agent-checkpointing scenario store-only and the `velo_save`/`velo_status` working-tree run | Bad arguments were JSON-RPC `-32602` errors, which the SDK raises as an exception and the model never sees: now `isError` results. The server did not offer `2025-11-25`: added. |
| MCP TypeScript SDK (`@modelcontextprotocol/sdk`) | 1.32.1 | stdio | Passes: lists tools, `velo_save_tree`, `velo_history`, a bad call as `isError` | `velo_tree_at` returned a bare array as `structuredContent`, which the spec requires to be an object: now `{files, total, truncated}`. |
| MCP Inspector CLI (`@modelcontextprotocol/inspector`) | 2.11.0 | stdio | Passes: `tools/list` and one `tools/call` exit 0 | Nothing. |
| Claude Desktop, Cursor, VS Code | not exercised in CI | stdio | Cannot be installed headlessly | Not exercised in CI. |

Found while writing the conformance clients, and fixed for every client:
`velo_history`, `velo_blame`, `velo_read_file` and `velo_tree_at` had no bound
and could return multi-megabyte text (see Large results).
