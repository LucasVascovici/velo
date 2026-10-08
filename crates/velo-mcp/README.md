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

`velo_save`, `velo_restore`, `velo_status`, `velo_diff`, `velo_history`,
`velo_metadata`. There is no force option anywhere: `velo_restore` on a dirty
tree returns an error result (`DirtyWorkingTree`) and overwrites nothing.

A velo error is a tool result with `isError: true`; protocol faults are JSON-RPC
errors. Only protocol lines go to stdout; diagnostics go to stderr.
