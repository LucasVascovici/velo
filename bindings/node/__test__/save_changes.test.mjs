import { test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { createRequire } from 'node:module';

const require = createRequire(import.meta.url);
const { Repo } = require('../index.js');

async function fresh() {
  return Repo.init(fs.mkdtempSync(path.join(os.tmpdir(), 'velo-node-')));
}

async function objects(repo, snap) {
  const out = {};
  for (const f of await repo.treeAt(snap)) out[f.path] = f.object;
  return out;
}

test('put and delete: written equals treeAt objects', async () => {
  const repo = await fresh();
  const base = await repo.saveTree({
    branch: 'main',
    message: 'base',
    entries: [
      { path: 'a.txt', data: 'one' },
      { path: 'b.txt', data: 'two' },
    ],
  });
  const saved = await repo.saveChanges({
    branch: 'main',
    message: 'edit',
    parent: base,
    changes: [
      { op: 'put', path: 'a.txt', data: 'ONE' },
      { op: 'put', path: 'c.bin', data: Buffer.from([0, 1]) },
      { op: 'delete', path: 'b.txt' },
    ],
  });
  const tree = await objects(repo, saved.snapshot);
  assert.deepEqual(Object.keys(tree).sort(), ['a.txt', 'c.bin']);
  assert.deepEqual(saved.written, [
    { path: 'a.txt', object: tree['a.txt'] },
    { path: 'c.bin', object: tree['c.bin'] },
  ]);
  assert.equal(await repo.branchTip('main'), saved.snapshot);
});

test('rename, stored put and executable kind', async () => {
  const repo = await fresh();
  const base = await repo.saveTree({
    branch: 'main',
    message: 'base',
    entries: [{ path: 'old.txt', data: 'keep' }],
  });
  const oldObj = (await objects(repo, base))['old.txt'];
  const saved = await repo.saveChanges({
    branch: 'main',
    message: 'm',
    parent: base,
    changes: [
      { op: 'rename', from: 'old.txt', to: 'new.txt' },
      { op: 'put', path: 'alias.txt', object: oldObj },
      { op: 'put', path: 'run.sh', data: '#!/bin/sh\n', kind: 'executable' },
    ],
  });
  const tree = await objects(repo, saved.snapshot);
  assert.deepEqual(Object.keys(tree).sort(), ['alias.txt', 'new.txt', 'run.sh']);
  assert.equal(tree['new.txt'], oldObj);
  for (const w of saved.written) assert.equal(tree[w.path], w.object);
});

test('same id as saveTree of the same tree', async () => {
  const one = await fresh();
  const two = await fresh();
  const common = {
    branch: 'main',
    timestampMs: 1_700_000_000_000,
    author: { name: 'ada' },
  };
  const entries = [
    { path: 'a', data: '1' },
    { path: 'b', data: '2' },
  ];
  const base = await one.saveTree({ ...common, message: 'base', entries });
  const base2 = await two.saveTree({ ...common, message: 'base', entries });
  assert.equal(base, base2);
  const saved = await one.saveChanges({
    ...common,
    message: 'next',
    parent: base,
    changes: [
      { op: 'put', path: 'a', data: '9' },
      { op: 'delete', path: 'b' },
      { op: 'put', path: 'c', data: '3' },
    ],
  });
  const direct = await two.saveTree({
    ...common,
    message: 'next',
    parent: base2,
    entries: [
      { path: 'a', data: '9' },
      { path: 'c', data: '3' },
    ],
  });
  assert.equal(saved.snapshot, direct);
});

test('deleting an absent path rejects with InvalidInput', async () => {
  const repo = await fresh();
  const base = await repo.saveTree({
    branch: 'main',
    message: 'b',
    entries: [{ path: 'a', data: '1' }],
  });
  await assert.rejects(
    repo.saveChanges({
      branch: 'main',
      message: 'm',
      parent: base,
      changes: [{ op: 'delete', path: 'nope' }],
    }),
    (err) => err.code === 'InvalidInput',
  );
});

test('a malformed op rejects with InvalidInput', async () => {
  const repo = await fresh();
  for (const change of [
    { op: 'frobnicate', path: 'a' },
    { op: 'put', data: 'x' },
    { op: 'delete' },
    { op: 'rename', from: 'a' },
    { op: 'put', path: 'a', data: 'x', object: 'y' },
  ]) {
    await assert.rejects(
      repo.saveChanges({ branch: 'main', message: 'm', changes: [change] }),
      (err) => err.code === 'InvalidInput',
      JSON.stringify(change),
    );
  }
});
