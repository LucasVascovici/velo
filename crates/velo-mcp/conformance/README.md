# MCP conformance clients

Real MCP client implementations driving the built `velo-mcp` end to end. CI
runs all three in `.github/workflows/mcp.yml`; the results and what each one
found are in [`../README.md`](../README.md#tried-clients).

From the repository root, build the server and the CLI (the CLI only runs
`velo init` for the working-tree run):

```bash
cargo build -p velo-mcp -p velo-cli
```

## Python SDK

```bash
pip install mcp jsonschema
python crates/velo-mcp/conformance/python_sdk_client.py \
  --velo-mcp target/debug/velo-mcp --velo target/debug/velo
```

Checks the negotiated protocol version, validates every tool `inputSchema`
as JSON Schema draft 2020-12, then plays the agent-checkpointing cookbook
scenario against `--store <tmp>/repo.db --create` (a snapshot per tool call, a
branch per attempt, `velo_find` the passing attempt, `velo_merge_plan`, the
two-parent `velo_save_tree` merge, history, blame), checks that bad input and
a bad snapshot id are `isError` results and that a file over 1 MiB is
truncated. A second run with `--repo <dir>` covers `velo_save` and
`velo_status`.

## TypeScript SDK

```bash
cd crates/velo-mcp/conformance
npm ci
node ts_sdk_client.mjs ../../../target/debug/velo-mcp
```

Uses `@modelcontextprotocol/sdk` `Client` with `StdioClientTransport`: lists
the tools, one `velo_save_tree`, one `velo_history`, one bad call.

## MCP Inspector CLI

```bash
npx -y @modelcontextprotocol/inspector --cli target/debug/velo-mcp \
  --store /tmp/repo.db --create --method tools/list
npx -y @modelcontextprotocol/inspector --cli target/debug/velo-mcp \
  --store /tmp/repo.db --create --method tools/call --tool-name velo_save_tree \
  --tool-arg branch=main --tool-arg message=hi --tool-arg 'files={"a.txt":"one\n"}'
```

Both must exit 0.
