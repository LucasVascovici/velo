# MCP conformance clients

Real MCP client implementations driving the built `velody-mcp` end to end. CI
runs all three in `.github/workflows/mcp.yml`; the results and what each one
found are in [`../README.md`](../README.md#tried-clients).

From the repository root, build the server and the CLI (the CLI only runs
`velody init` for the working-tree run):

```bash
cargo build -p velody-mcp -p velody-cli
```

## Python SDK

```bash
pip install mcp jsonschema
python crates/velody-mcp/conformance/python_sdk_client.py \
  --velody-mcp target/debug/velody-mcp --velody target/debug/velody
```

Checks the negotiated protocol version, validates every tool `inputSchema`
as JSON Schema draft 2020-12, then plays the agent-checkpointing cookbook
scenario against `--store <tmp>/repo.db --create` (a snapshot per tool call, a
branch per attempt, `velody_find` the passing attempt, `velody_merge_plan`, the
two-parent `velody_save_tree` merge, history, blame), checks that bad input and
a bad snapshot id are `isError` results and that a file over 1 MiB is
truncated. A second run with `--repo <dir>` covers `velody_save` and
`velody_status`.

## TypeScript SDK

```bash
cd crates/velody-mcp/conformance
npm ci
node ts_sdk_client.mjs ../../../target/debug/velody-mcp
```

Uses `@modelcontextprotocol/sdk` `Client` with `StdioClientTransport`: lists
the tools, one `velody_save_tree`, one `velody_history`, one bad call.

## MCP Inspector CLI

```bash
npx -y @modelcontextprotocol/inspector --cli target/debug/velody-mcp \
  --store /tmp/repo.db --create --method tools/list
npx -y @modelcontextprotocol/inspector --cli target/debug/velody-mcp \
  --store /tmp/repo.db --create --method tools/call --tool-name velody_save_tree \
  --tool-arg branch=main --tool-arg message=hi --tool-arg 'files={"a.txt":"one\n"}'
```

Both must exit 0.

## Against a release archive

`.github/workflows/release-smoke.yml` runs the Python and TypeScript clients,
the Inspector calls and `check_configs.py` against the `velody-mcp` extracted
from the release archive (Linux and Windows, an empty directory, no Rust
toolchain). `release.yml` calls it with the run's own artifacts before
publishing; `quickstarts.yml` can be dispatched with `release_tag` to run it
against a published release. The `uvx` and `npx` forms are not tested: no
registry package exists yet.
