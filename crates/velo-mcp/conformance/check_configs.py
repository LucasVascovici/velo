#!/usr/bin/env python3
"""Check the documented MCP host configurations against a built velo-mcp.

For every file in configs/ this script
  1. checks that the README documents exactly that JSON (a fenced block
     preceded by `<!-- config: <file> -->`),
  2. substitutes the real binary and a temporary repository for the
     `/path/to/...` placeholders, launches the command, and runs the
     initialize / tools/list exchange over stdio.

Usage: check_configs.py <velo-mcp> <velo>
"""
import json
import pathlib
import re
import subprocess
import sys
import tempfile

HERE = pathlib.Path(__file__).resolve().parent
README = HERE.parent / "README.md"
CONFIGS = HERE / "configs"


def readme_blocks():
    text = README.read_text(encoding="utf-8")
    pat = re.compile(r"<!-- config: (\S+) -->\s*```json\r?\n(.*?)```", re.S)
    return {m.group(1): json.loads(m.group(2)) for m in pat.finditer(text)}


def entry(cfg):
    servers = cfg.get("mcpServers") or cfg.get("servers")
    assert servers and "velo" in servers, "config has no velo server"
    return servers["velo"]


def rpc(proc, msg):
    proc.stdin.write(json.dumps(msg) + "\n")
    proc.stdin.flush()
    if "id" not in msg:
        return None
    line = proc.stdout.readline()
    assert line, "server closed stdout"
    resp = json.loads(line)
    assert resp.get("id") == msg["id"] and "error" not in resp, resp
    return resp["result"]


def launch(server, binary, repo):
    args = [repo if a == "/path/to/project" else a for a in server.get("args", [])]
    proc = subprocess.Popen(
        [binary] + args,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
        text=True,
    )
    try:
        init = rpc(
            proc,
            {
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2025-11-25",
                    "capabilities": {},
                    "clientInfo": {"name": "check_configs", "version": "0"},
                },
            },
        )
        assert init["serverInfo"]["name"], init
        rpc(proc, {"jsonrpc": "2.0", "method": "notifications/initialized"})
        tools = rpc(proc, {"jsonrpc": "2.0", "id": 2, "method": "tools/list"})
        names = {t["name"] for t in tools["tools"]}
        assert "velo_save" in names and "velo_status" in names, names
    finally:
        proc.stdin.close()
        proc.wait(timeout=10)


def main():
    # Absolute, because both programs are started with a temporary repository
    # as their working directory, where a relative path no longer resolves.
    binary = str(pathlib.Path(sys.argv[1]).resolve())
    velo = str(pathlib.Path(sys.argv[2]).resolve())
    docs = readme_blocks()
    files = sorted(CONFIGS.glob("*.json"))
    assert files, "no configs"
    for f in files:
        cfg = json.loads(f.read_text(encoding="utf-8"))
        assert docs.get(f.name) == cfg, f"README snippet for {f.name} differs from the file"
        server = entry(cfg)
        assert server["command"] == "/path/to/velo-mcp", f.name
        with tempfile.TemporaryDirectory() as repo:
            subprocess.run([velo, "init"], cwd=repo, check=True, stdout=subprocess.DEVNULL)
            launch(server, binary, repo)
        print(f"ok {f.name}")
    extra = set(docs) - {f.name for f in files}
    assert not extra, f"README documents configs with no file: {extra}"


if __name__ == "__main__":
    main()
