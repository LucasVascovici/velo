# MCP: give any agent a checkpoint layer in five minutes

`velo-mcp` is a stdio [Model Context Protocol](https://modelcontextprotocol.io)
server. Any MCP client gets save, history and read tools without integration
code. Every tool is listed in the
[server README](../../crates/velo-mcp/README.md).

## Install

Build the server from a release tag (see [INSTALL.md](../INSTALL.md)):

```sh
cargo install --git https://github.com/LucasVascovici/velo --tag vX.Y.Z velo-mcp
```

Then point your client at it. This is the working-tree mode, serving a
project directory:

```json
{"command": "velo-mcp", "args": ["--repo", "/path/to/project"]}
```

## Drive it by hand

The script below is a minimal client in the store-only mode, which needs no
project directory: one file holds the history. It starts the server, runs the
`initialize` handshake, lists the tools, saves a snapshot, reads the history
and reads the file back. A real client (Claude Desktop, the official SDKs)
does the same over the same pipe. The script takes the server's path from
`$VELO_MCP` and falls back to `velo-mcp` on `PATH`.

```python quickstart
import json
import os
import subprocess
import tempfile

server = os.environ.get("VELO_MCP", "velo-mcp")

with tempfile.TemporaryDirectory() as tmp:
    proc = subprocess.Popen(
        [server, "--store", os.path.join(tmp, "agent.velo"), "--create"],
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True,
    )
    next_id = 0

    def send(method, params=None, notify=False):
        global next_id
        msg = {"jsonrpc": "2.0", "method": method}
        if params is not None:
            msg["params"] = params
        if not notify:
            next_id += 1
            msg["id"] = next_id
        proc.stdin.write(json.dumps(msg) + "\n")
        proc.stdin.flush()
        if notify:
            return None
        reply = json.loads(proc.stdout.readline())
        assert "error" not in reply, reply
        return reply["result"]

    def call(tool, **arguments):
        result = send("tools/call", {"name": tool, "arguments": arguments})
        assert not result.get("isError"), result
        if result.get("structuredContent") is not None:
            return result["structuredContent"]
        return json.loads(result["content"][0]["text"])

    init = send("initialize", {
        "protocolVersion": "2025-11-25",
        "capabilities": {},
        "clientInfo": {"name": "quickstart", "version": "0"},
    })
    assert init["serverInfo"]["name"] == "velo-mcp"
    send("notifications/initialized", notify=True)

    names = {t["name"] for t in send("tools/list")["tools"]}
    assert {"velo_save_tree", "velo_history", "velo_read_file"} <= names, names

    saved = call("velo_save_tree", branch="main", message="first checkpoint",
                 files={"app.py": "print('v1')\n"})
    snapshot = saved["snapshot"]

    assert "first checkpoint" in json.dumps(call("velo_history", branch="main"))
    back = call("velo_read_file", snapshot=snapshot, path="app.py")
    assert "print('v1')" in json.dumps(back), back
    print("ok:", snapshot[:8])

    proc.stdin.close()
    proc.wait(timeout=10)
```
