"""Drive velody-mcp with the official `mcp` Python SDK.

    pip install mcp jsonschema
    python python_sdk_client.py --velody-mcp target/debug/velody-mcp --velody target/debug/velody

Two runs: store-only (`--store <tmp>/repo.db --create`), where it plays the
agent-checkpointing cookbook scenario (mirroring
bindings/python/examples/agent_checkpointing.py), and working-tree
(`--repo <dir>`), where it exercises `velody_save` and `velody_status`. Every
assertion is a claim about what a real client sees.
"""
import argparse
import asyncio
import json
import subprocess
import sys
import tempfile
from pathlib import Path

import jsonschema
from mcp import ClientSession, StdioServerParameters
from mcp.client.stdio import stdio_client

EXPECTED_VERSION = "2025-11-25"

BASE = "def total(xs):\n    return sum(xs)\n\nprint(total([1, 2]))\n"
GENERATOR = BASE.replace("sum(xs)", "sum(x for x in xs)")


def payload(result):
    """The structured content of a tool result, from either channel."""
    if getattr(result, "structured_content", None) is not None:
        return result.structured_content
    return json.loads(result.content[0].text)


async def ok(session, tool, **args):
    result = await session.call_tool(tool, args)
    assert not result.is_error, f"{tool} failed: {result.content[0].text}"
    return payload(result)


async def start(session):
    init = await session.initialize()
    assert init.protocol_version == EXPECTED_VERSION, init.protocol_version
    assert init.server_info.name == "velody-mcp"
    tools = (await session.list_tools()).tools
    assert tools, "no tools listed"
    for t in tools:
        schema = t.input_schema
        jsonschema.Draft202012Validator.check_schema(schema)
        assert schema["type"] == "object" and "properties" in schema, t.name
    return {t.name: t for t in tools}


async def store_run(session):
    tools = await start(session)
    for name in ("velody_save_tree", "velody_find", "velody_merge_plan", "velody_history"):
        assert name in tools, name
    # The working-tree tools are not offered when there is no working tree.
    assert "velody_save" not in tools

    async def tool_call(branch, parent, run, call, files, message, tests=None):
        meta = {"agent": {"run": run, "tool_call_id": call, "model": "m"}}
        if tests is not None:
            meta["agent"]["tests"] = tests
        args = dict(branch=branch, message=message, files=files, meta=meta,
                    tool_call_id=call)
        if parent:
            args["parent"] = parent
        return (await ok(session, "velody_save_tree", **args))["snapshot"]

    base = await tool_call("main", None, "human", "c0", {"app.py": BASE}, "base")
    for name in ("attempt-1", "attempt-2"):
        await ok(session, "velody_branch", action="create", name=name, at=base)

    a1 = await tool_call("attempt-1", base, "run-1", "call-1",
                         {"app.py": GENERATOR}, "rewrite total as a generator")
    a2 = await tool_call("attempt-1", a1, "run-1", "call-2",
                         {"app.py": GENERATOR + "print(total([]))\n"},
                         "add an empty-list check", tests="pass")
    b1 = await tool_call("attempt-2", base, "run-2", "call-1",
                         {"app.py": "def total(xs):\n    return 0\n"},
                         "stub total", tests="fail")

    # Pick the winner by metadata, not by remembering ids.
    found = await ok(session, "velody_find", filters=[["agent", "tests", "pass"]])
    assert [s["snapshot"] for s in found["snapshots"]] == [a2], found
    fails = await ok(session, "velody_find", filters=[["agent", "tests", "fail"]])
    assert [s["snapshot"] for s in fails["snapshots"]] == [b1], fails
    meta = await ok(session, "velody_metadata", snapshot=a2)
    assert meta["meta"]["agent"]["run"] == "run-1"
    assert meta["meta"]["mcp"]["tool_call_id"] == "call-2"

    run1 = await ok(session, "velody_history", branch="attempt-1",
                    meta=[{"namespace": "agent", "key": "run", "value": "run-1"}])
    assert [e["snapshot"] for e in run1["entries"]] == [a2, a1], run1
    assert run1["truncated"] is False

    # Merge the winner into main: plan, then record the two-parent snapshot
    # with save_tree (the store-mode merge path).
    plan = await ok(session, "velody_merge_plan", source=a2)
    assert plan["clean"] is True, plan
    merged = (await ok(
        session, "velody_save_tree", branch="main", message="take attempt-1",
        parent=base, merge_parent=a2, base=a2,
        files={}, meta={"agent": {"picked": "run-1"}}))["snapshot"]
    read = await ok(session, "velody_read_file", snapshot=merged, path="app.py")
    assert read["content"] == GENERATOR + "print(total([]))\n", read
    tip = await ok(session, "velody_history", limit=1)
    assert tip["entries"][0]["snapshot"] == merged
    assert tip["entries"][0]["merge_parent"] == a2

    # Blame: each line's origin names the run that wrote it.
    blame = await ok(session, "velody_blame", path="app.py", at=merged)
    # `origin.run` is the server's run (one per process); the cookbook's own
    # run ids live in the metadata of the origin snapshot.
    runs = {}
    for l in blame["lines"]:
        origin = await ok(session, "velody_metadata", snapshot=l["origin"]["snapshot"])
        runs[l["text"].strip()] = origin["meta"]["agent"]["run"]
    assert runs["return sum(x for x in xs)"] == "run-1", runs
    assert runs["print(total([]))"] == "run-1", runs
    assert runs["def total(xs):"] == "human", runs

    # Errors are results, not transport failures.
    bad = await session.call_tool("velody_read_file", {"snapshot": "zz-not-a-snapshot",
                                                     "path": "app.py"})
    assert bad.is_error and bad.content[0].text, bad
    bad = await session.call_tool("velody_save_tree", {"branch": "main"})
    assert bad.is_error and "InvalidParams" in bad.content[0].text, bad

    # A large file is truncated, with a hint on how to page.
    big = "x" * (1024 * 1024 + 100)
    snap = (await ok(session, "velody_save_tree", branch="big", message="big",
                     files={"big.txt": big}))["snapshot"]
    page = await ok(session, "velody_read_file", snapshot=snap, path="big.txt")
    assert page["truncated"] is True and len(page["content"]) == 1024 * 1024, page.keys()
    assert "offset" in page["hint"]
    rest = await ok(session, "velody_read_file", snapshot=snap, path="big.txt",
                    offset=page["next_offset"])
    assert rest["truncated"] is False and len(rest["content"]) == 100
    print("store-only run ok: winner", a2[:12], "merged", merged[:12])


async def tree_run(session, directory):
    tools = await start(session)
    assert "velody_save" in tools and "velody_status" in tools
    (directory / "a.txt").write_text("one\n")
    st = await ok(session, "velody_status")
    assert "a.txt" in st["new_files"], st
    saved = await ok(session, "velody_save", message="first", tool_call_id="t1")
    assert len(saved["snapshot"]) == 64, saved
    st = await ok(session, "velody_status")
    assert not st["modified"] and not st["new_files"], st
    (directory / "a.txt").write_text("two\n")
    st = await ok(session, "velody_status")
    assert st["modified"] == ["a.txt"], st
    bad = await session.call_tool("velody_save", {"message": 3})
    assert bad.is_error, bad
    print("working-tree run ok:", saved["snapshot"][:12])


async def run(args):
    with tempfile.TemporaryDirectory() as tmp:
        tmp = Path(tmp)
        store = StdioServerParameters(
            command=args.velody_mcp,
            args=["--store", str(tmp / "repo.db"), "--create", "--run", "py-sdk"])
        async with stdio_client(store) as (r, w):
            async with ClientSession(r, w) as session:
                await store_run(session)

        work = tmp / "work"
        work.mkdir()
        subprocess.run([args.velody, "init"], cwd=work, check=True,
                       capture_output=True)
        tree = StdioServerParameters(
            command=args.velody_mcp, args=["--repo", str(work), "--run", "py-sdk"])
        async with stdio_client(tree) as (r, w):
            async with ClientSession(r, w) as session:
                await tree_run(session, work)


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--velody-mcp", required=True)
    p.add_argument("--velody", required=True, help="the velody CLI, to init the working-tree repo")
    args = p.parse_args()
    args.velody_mcp = str(Path(args.velody_mcp).resolve())
    args.velody = str(Path(args.velody).resolve())
    asyncio.run(run(args))
    return 0


if __name__ == "__main__":
    sys.exit(main())
