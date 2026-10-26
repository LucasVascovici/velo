// Drive velody-mcp with the official MCP TypeScript SDK.
//
//   npm install && node ts_sdk_client.mjs <path to velody-mcp>
//
// Lists the tools, then does one velody_save_tree and one velody_history.
import assert from "node:assert/strict";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { StdioClientTransport } from "@modelcontextprotocol/sdk/client/stdio.js";

const bin = resolve(process.argv[2] ?? "../../../target/debug/velody-mcp");
const tmp = mkdtempSync(join(tmpdir(), "velody-ts-"));
const transport = new StdioClientTransport({
  command: bin,
  args: ["--store", join(tmp, "repo.db"), "--create", "--run", "ts-sdk"],
});
const client = new Client({ name: "velody-conformance-ts", version: "1.0.0" });

try {
  await client.connect(transport);
  const { tools } = await client.listTools();
  const names = tools.map((t) => t.name);
  for (const want of ["velody_save_tree", "velody_history", "velody_read_file"]) {
    assert.ok(names.includes(want), `${want} not listed: ${names}`);
  }
  for (const t of tools) {
    assert.equal(t.inputSchema.type, "object", t.name);
    assert.ok(t.inputSchema.properties, t.name);
  }

  const saved = await client.callTool({
    name: "velody_save_tree",
    arguments: {
      branch: "main",
      message: "from the ts sdk",
      files: { "a.txt": "one\n" },
      meta: { agent: { run: "ts-1" } },
      tool_call_id: "ts-call-1",
    },
  });
  assert.ok(!saved.isError, JSON.stringify(saved));
  const id = saved.structuredContent.snapshot;
  assert.equal(id.length, 64);

  const history = await client.callTool({ name: "velody_history", arguments: {} });
  assert.ok(!history.isError, JSON.stringify(history));
  assert.equal(history.structuredContent.entries[0].snapshot, id);
  assert.equal(history.structuredContent.entries[0].run, "ts-sdk");
  assert.equal(history.structuredContent.truncated, false);

  // A bad argument is a result the model can read, not a thrown error.
  const bad = await client.callTool({ name: "velody_history", arguments: { limit: 0 } });
  assert.equal(bad.isError, true);
  assert.match(bad.content[0].text, /InvalidParams/);

  console.log("ts sdk run ok:", id.slice(0, 12));
} finally {
  await client.close();
  rmSync(tmp, { recursive: true, force: true });
}
