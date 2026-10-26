import { test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { createRequire } from 'node:module';

const require = createRequire(import.meta.url);
const { Repo } = require('../index.js');

const opts = { timeout: 30000 };

async function fresh() {
  const repo = await Repo.init(fs.mkdtempSync(path.join(os.tmpdir(), 'velody-node-')));
  const files = {
    'a.txt': 'one',
    'src/b.py': 'two',
    'src/c.bin': Buffer.from([0, 255, 1]),
    'docs/d.md': 'four',
  };
  const id = await repo.saveTree({
    branch: 'main',
    message: 'base',
    entries: Object.entries(files).map(([p, data]) => ({ path: p, data })),
  });
  return { repo, id };
}

const tmp = () => fs.mkdtempSync(path.join(os.tmpdir(), 'velody-out-'));

test('readFilesAt keeps order and bytes', opts, async () => {
  const { repo, id } = await fresh();
  const paths = ['src/c.bin', 'a.txt', 'src/b.py', 'a.txt'];
  const got = await repo.readFilesAt(id, paths);
  assert.deepEqual(got.map((f) => f.path), paths);
  for (const f of got) {
    assert.ok(Buffer.isBuffer(f.data));
    assert.deepEqual(f.data, await repo.readFileAt(id, f.path));
  }
});

test('readFilesAt rejects a missing path', opts, async () => {
  const { repo, id } = await fresh();
  await assert.rejects(repo.readFilesAt(id, ['a.txt', 'nope.txt']));
});

test('materialize writes the whole tree', opts, async () => {
  const { repo, id } = await fresh();
  const dest = path.join(tmp(), 'out');
  assert.equal(await repo.materialize(id, dest), 4);
  assert.equal(fs.readFileSync(path.join(dest, 'a.txt'), 'utf8'), 'one');
  assert.deepEqual(fs.readFileSync(path.join(dest, 'src', 'c.bin')), Buffer.from([0, 255, 1]));
});

test('materialize honours only and ignore', opts, async () => {
  const { repo, id } = await fresh();
  const a = path.join(tmp(), 'a');
  assert.equal(await repo.materialize(id, a, { only: ['src/**'] }), 2);
  assert.ok(!fs.existsSync(path.join(a, 'a.txt')));
  assert.ok(fs.existsSync(path.join(a, 'src', 'b.py')));
  const b = path.join(tmp(), 'b');
  assert.equal(await repo.materialize(id, b, { ignore: ['*.bin', 'docs/'] }), 2);
  assert.ok(!fs.existsSync(path.join(b, 'src', 'c.bin')));
  assert.ok(fs.existsSync(path.join(b, 'a.txt')));
});
