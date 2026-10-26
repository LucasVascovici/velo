# velody-mcp

A synchronous stdio [Model Context Protocol](https://modelcontextprotocol.io)
server that makes velody the checkpoint layer for any agent, with no integration
code on the agent author's side. There is no async runtime: the core has none,
and one request at a time is all a stdio client sends.

## Which half of the API

An agent edits files in a project directory, so `velody-mcp` serves a repository
*with* a working tree (`--repo <dir>`, default the current directory, opened with
`Repo::discover`). It therefore uses the working-tree commands `save`, `restore`,
`status` and `diff`. That is legitimate: `velody-mcp` is a consumer like the CLI,
not a language binding, so it is not limited to the embeddable half.

## Client configuration

Download the `velody-mcp` archive for your platform from the latest release and
extract it:

```sh
curl -L -o velody-mcp.tar.gz https://github.com/LucasVascovici/velody/releases/latest/download/velody-mcp-x86_64-linux.tar.gz && tar -xzf velody-mcp.tar.gz
```

Assets are `velody-mcp-<arch>-<os>.tar.gz` (`x86_64-linux`, `aarch64-linux`,
`x86_64-macos`, `aarch64-macos`) and `velody-mcp-x86_64-windows.zip`. In every
config below, replace `/path/to/velody-mcp` with the extracted binary and
`/path/to/project` with the project directory (run `velody init` in it first).
Other flags: `--run <id>`, `--lock-wait <secs>`, and `--store <file> --create`
for store-only mode.

Claude Desktop (`claude_desktop_config.json`):

<!-- config: claude-desktop.json -->
```json
{
  "mcpServers": {
    "velody": {
      "command": "/path/to/velody-mcp",
      "args": ["--repo", "/path/to/project"]
    }
  }
}
```

Cursor (`.cursor/mcp.json`):

<!-- config: cursor.json -->
```json
{
  "mcpServers": {
    "velody": {
      "command": "/path/to/velody-mcp",
      "args": ["--repo", "/path/to/project"]
    }
  }
}
```

Claude Code (`.mcp.json`, or on one line
`claude mcp add velody -- /path/to/velody-mcp --repo /path/to/project`):

<!-- config: claude-code.json -->
```json
{
  "mcpServers": {
    "velody": {
      "type": "stdio",
      "command": "/path/to/velody-mcp",
      "args": ["--repo", "/path/to/project"]
    }
  }
}
```

VS Code (`.vscode/mcp.json`):

<!-- config: vscode.json -->
```json
{
  "servers": {
    "velody": {
      "type": "stdio",
      "command": "/path/to/velody-mcp",
      "args": ["--repo", "/path/to/project"]
    }
  }
}
```

`uvx` and `npx` forms will be added when a registry package ships `velody-mcp`;
none does today. [`server.json`](server.json) is a prepared entry for the MCP
registry; submitting it is the owner's to do. Every config above is launched
against the built binary by `conformance/check_configs.py` (run in
`.github/workflows/mcp.yml`), which also checks these snippets equal the files
in `conformance/configs/`. The same check, the conformance clients and the
quickstart pages also run in CI against the release archive for Linux and
Windows (`.github/workflows/release-smoke.yml`), extracted into an empty
directory on a runner with no Rust toolchain; on a tag run that happens before
the release is published. The `uvx` and `npx` forms stay untested until a
registry package exists.

## Run identity

The run id is `--run <id>`, else `$VELODY_MCP_RUN`, else `run-<epoch_ms>`. Every
write tool records metadata in namespace `mcp`: `run`, `tool`, `client` and,
when the call supplies a `tool_call_id`, `tool_call_id`. Agents cannot write to
the `velo` or `mcp` namespaces themselves. The author is the client name, or
`$VELODY_AUTHOR_NAME` / `$VELODY_AUTHOR_EMAIL` when set.

## Tools

| Tool | What it does |
| --- | --- |
| `velody_save` | Save a snapshot; records the run in `mcp` metadata. |
| `velody_restore` | Restore the tree or some paths; refuses on unsaved changes. |
| `velody_status` | Branch, position and unsaved changes. |
| `velody_diff` | Diff two snapshots, or a snapshot against the tree. |
| `velody_history` | Snapshots, newest first, filterable by metadata (such as run). |
| `velody_metadata` | Author and all metadata of a snapshot. |
| `velody_branch` | `list`, `create` (does not switch) or `switch` (creates the branch if missing, as the CLI does; refuses on a dirty tree). |
| `velody_merge_plan` | What merging `source` into the current branch would do, with conflict texts and an `apply_hint`. Writes nothing. |
| `velody_merge_apply` | Record the merge as a two-parent snapshot and move the tree to it. Needs a clean tree and a resolution for every conflict. |
| `velody_blame` | Per-line origin; `origin.run` is the run that saved the line. `start_line` and `end_line` (1-based, inclusive) return only that slice; an inverted range or one past the end of the file is an invalid-params error. |
| `velody_undo` | Undo the last snapshot on the current branch (shelved, not destroyed). Needs a working tree. |
| `velody_redo` | Bring back what the last `velody_undo` shelved. Needs a working tree. |
| `velody_tag` | `list`, `create` (refuses a taken name) or `delete` tags. Works in store-only mode. |
| `velody_conflicts` | The conflicts of a stopped merge as data: per path the ancestor, ours and theirs objects and each hunk with its decision. Needs a working tree. |
| `velody_repack` | Convert stored full snapshots into deltas so the repository shrinks; rewrites no history and changes no id. `dry_run` (default false) reports the saving, with `bytes_after` an estimate, and writes nothing. Works in store-only mode. |
| `velody_find` | Snapshots matching `[namespace, key, value]` metadata filters, optionally on one `branch`. Works in store-only mode. |

There is deliberately no tool for squash, rebase, compact, retention, gc, force
push or anything else that rewrites pushed history, and no tool takes a `force`
argument: the safety stance is the tool surface. There is no force option anywhere: a dirty tree is an error result
(`DirtyWorkingTree`) and nothing is overwritten. Merging is always plan, then
apply; there is no single-call merge.

### Example agent session

1. `velody_branch {"action": "create", "name": "attempt-1"}` then
   `{"action": "switch", "name": "attempt-1"}`.
2. Edit files, then `velody_save {"message": "try a cache"}`.
3. `velody_branch {"action": "switch", "name": "main"}`.
4. `velody_merge_plan {"source": "attempt-1"}` shows each file's action, and for a
   conflict its base, ours and theirs text.
5. `velody_merge_apply {"source": "attempt-1", "message": "take the cache",
   "resolutions": {"app.py": {"content": "..."}}}`. Without resolutions a
   conflict returns `Conflicts` listing the paths.
6. `velody_blame {"path": "app.py"}`: each line's `origin.run` names the run that
   wrote it.

## Concurrent agents

Two agents may serve one repository through two servers. Write tools wait for
the repository lock for `--lock-wait SECS` (default 5, fractions allowed, 0 to
fail at once). If another process still holds it, the result has `isError: true`
and names the holder in its text and in `structuredContent`:
`{"error": "Locked", "holder": {"pid": ..., "command": ..., "started_at_ms": ...,
"started_at": ...}}`. Retry the call later; nothing was written.

## HTTP transport

`velody-mcp --http ADDR` serves MCP's Streamable HTTP transport instead of stdio,
so one server can serve agents on other machines. It works with `--repo` and
`--store` alike. The bound address is printed to stderr as
`velody-mcp: listening on http://ADDR/mcp`, so `--http 127.0.0.1:0` reveals the
port it chose.

- One endpoint, `/mcp`. `POST` carries one JSON-RPC message and is answered with
  `Content-Type: application/json` (no SSE stream); notifications and client
  responses get `202 Accepted`. `GET` is `405`. `DELETE` with `Mcp-Session-Id`
  ends the session (`204`).
- `initialize` returns an `Mcp-Session-Id` header; every later request must send
  it (`400` without, `404` if unknown or ended). `MCP-Protocol-Version`, when
  sent, must match the version negotiated at `initialize`. Each session keeps
  its own `clientInfo`, so saves are stamped with the right client.
- Tokens: `--token-file FILE` (one token per line; blank lines and `#` comments
  are skipped) and/or the `VELODY_MCP_TOKEN` environment variable. With any
  token configured, every request needs `Authorization: Bearer <token>`;
  otherwise `401` with `WWW-Authenticate: Bearer`. Comparison is constant-time.
  All tokens are equal; there is no read-only tier.
- Origin rule (DNS-rebinding protection): a request with an `Origin` header is
  accepted only from a loopback origin (`localhost`, `127.0.0.0/8`, `::1`) or
  one given exactly with `--allow-origin ORIGIN` (repeatable); otherwise `403`.
  Requests with no `Origin`, as from non-browser agents, pass this check.
- Binding a non-loopback address without a token is refused at start-up.
- Requests run one at a time against one repository handle; bodies are limited
  to 8 MiB.

The server speaks plain HTTP. To expose it beyond a trusted network, put a
reverse proxy that terminates TLS in front of it (as for `velody serve-http`), and
keep the bearer token out of logs.

Bearer tokens are the only supported authorization scheme. SSE streaming,
resumable streams and OAuth authorization-server metadata are not implemented.

## Store-only mode

An agent that has no checkout can still use velody. Point the server at a
single-file repository instead of a project directory:

```json
{"command": "velody-mcp", "args": ["--store", "/path/agent.velo", "--create"]}
```

`--create` makes the file when it is missing; without it the file must exist
(and is migrated if it is older). `--store` conflicts with `--repo`. Store mode
is chosen whenever the repository has no working tree, and nothing is ever
written outside the database file (plus its `-wal`, `-shm` and `.lock` files).

Content arrives in the call and trees come back by id:

| Tool | What it does |
| --- | --- |
| `velody_save_tree` | Record a snapshot from `files` (`{path: content}`, `null` deletes a path) or from `changes`, an ordered array of `{"op": "put", "path", "content"}`, `{"op": "delete", "path"}` and `{"op": "rename", "from", "to"}` applied to the parent's tree (a rename records an edge, so history and blame follow it). Give exactly one of the two; `changes` cannot be combined with `replace` or a `base` other than the parent. The result lists `written: {path: object}` for every file given content. By default the files are laid over the tree of `base` (default: the parent), so only changes are sent; `replace: true` makes `files` the whole tree. `encoding` is `utf8` (default) or `base64` and applies to every value. `parent` defaults to the branch tip, a root snapshot on an unborn branch. |
| `velody_tree_at` | The files of a snapshot as `{path, object, kind}`. |
| `velody_read_file` | One file of a snapshot as `{content, encoding}`; non-UTF-8 content comes back base64. |
| `velody_read_files` | Several files of one snapshot in one call: `{files: [{path, content, encoding, size}], omitted, truncated}`. `max_bytes` (default 4 MiB) is the total; files that do not fit are listed in `omitted` with a hint to call again with them. Each file falls back to base64 on its own. |

These four are listed in both modes. In store mode `tools/list` omits the tools
that need a working tree (`velody_save`, `velody_restore`, `velody_status`,
`velody_diff`, `velody_merge_apply` and `velody_branch` `switch`); calling one returns
an `isError: true` result that names `Unsupported` and the store tool to use.
`velody_history`, `velody_metadata`, `velody_blame`, `velody_merge_plan` and branch
`list` and `create` work as usual.

There is no MCP counterpart of the library's `materialize` (write a snapshot
into any directory). Store-only mode has no directory to write to, and a client
that has a checkout already has `velody_restore`. A store-only client reads a
whole set of files with `velody_read_files` instead of one `velody_read_file` call
per path.

Example session:

1. Create the first snapshot (the whole call is one line):
   `velody_save_tree {"branch": "attempt-1", "message": "start", "files": {"app.py": "print(1)\n", "notes.md": "todo\n"}}`
   returns the snapshot id.
2. Change one file and delete the other:
   `velody_save_tree {"branch": "attempt-1", "message": "cache", "files": {"app.py": "print(2)\n", "notes.md": null}}`
   carries the previous tree forward.
   The same with ordered edits, including a rename:
   `velody_save_tree {"branch": "attempt-1", "message": "move", "changes": [{"op": "put", "path": "app.py", "content": "print(3)\n"}, {"op": "rename", "from": "notes.md", "to": "docs/notes.md"}]}`
   returns `written: {"app.py": "<object id>"}` and `renamed: [["notes.md", "docs/notes.md"]]`.
3. `velody_read_file {"snapshot": "attempt-1", "path": "app.py"}` reads it back.
4. `velody_history {"meta": [{"namespace": "mcp", "key": "run"}]}` lists both
   snapshots with this run's identity.

A velody error, and bad arguments, are tool results with `isError: true` (the
text of a bad-argument result starts `InvalidParams:` and says what was wrong, so
the model can correct the call); only malformed requests and unknown tool names
are JSON-RPC errors. Only protocol lines go to stdout; diagnostics go to stderr.

## Large results

No tool sends multi-megabyte text by accident. A cut result has
`truncated: true`, a `hint` on how to get the rest and the value to pass for it:

| Tool | Bound | Next page |
| --- | --- | --- |
| `velody_history` | `limit`, default 200 entries | `offset` = `next_offset`, same filters |
| `velody_blame` | `limit`, default 2000 lines | `start_line` = `next_start_line` |
| `velody_tree_at` | `limit`, default 5000 files; result is `{files, total, truncated}` | `offset` = `next_offset` |
| `velody_read_file` | `max_bytes`, default 1 MiB; result carries `size` | `offset` = `next_offset` (text is cut on a character boundary) |
| `velody_read_files` | `max_bytes`, default 4 MiB in total; later files go to `omitted` | call again with the paths in `omitted` |

## Protocol versions

`2025-11-25`, `2025-06-18`, `2025-03-26` and `2024-11-05` are accepted; the
client's version is echoed when supported, else the newest (`2025-11-25`).

## Tried clients

Each client below is run against the built server by
[`conformance/`](conformance) (how to run it locally is in its README) and, for
the first three, in CI (`.github/workflows/mcp.yml`).

| Client | Version | Transport | Result | What broke, what was fixed |
| --- | --- | --- | --- | --- |
| MCP Python SDK (`mcp`) | 2.3.0 | stdio | Passes: negotiates `2025-11-25`, every `inputSchema` is valid draft 2020-12, completes the agent-checkpointing scenario store-only and the `velody_save`/`velody_status` working-tree run | Bad arguments were JSON-RPC `-32602` errors, which the SDK raises as an exception and the model never sees: now `isError` results. The server did not offer `2025-11-25`: added. |
| MCP TypeScript SDK (`@modelcontextprotocol/sdk`) | 1.32.1 | stdio | Passes: lists tools, `velody_save_tree`, `velody_history`, a bad call as `isError` | `velody_tree_at` returned a bare array as `structuredContent`, which the spec requires to be an object: now `{files, total, truncated}`. |
| MCP Inspector CLI (`@modelcontextprotocol/inspector`) | 2.11.0 | stdio | Passes: `tools/list` and one `tools/call` exit 0 | Nothing. |
| Claude Desktop, Cursor, VS Code | not exercised in CI | stdio | Cannot be installed headlessly | Not exercised in CI. |

Found while writing the conformance clients, and fixed for every client:
`velody_history`, `velody_blame`, `velody_read_file` and `velody_tree_at` had no bound
and could return multi-megabyte text (see Large results).
