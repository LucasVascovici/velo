import { test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { createRequire } from 'node:module';

const require = createRequire(import.meta.url);
const { Repo } = require('../index.js');

function tmp() {
  return fs.mkdtempSync(path.join(os.tmpdir(), 'velo-node-'));
}

async function fresh() {
  return Repo.init(tmp());
}

test('init, then open', async () => {
  const dir = tmp();
  await Repo.init(dir);
  const repo = await Repo.open(dir);
  assert.equal(await repo.branchTip('main'), null);
});

test('open on an empty dir rejects with NotARepo', async () => {
  await assert.rejects(Repo.open(tmp()), (err) => {
    assert.ok(err instanceof Error);
    assert.equal(err.code, 'NotARepo');
    assert.equal(typeof err.searchedFrom, 'string');
    return true;
  });
});

test('saveTree, then treeAt and readFileAt round-trip', async () => {
  const repo = await fresh();
  const id = await repo.saveTree({
    branch: 'main',
    message: 'one',
    entries: [
      { path: 'a.txt', data: 'hello' },
      { path: 'dir/b.bin', data: Buffer.from([0, 1, 2, 255]) },
      { path: 'run.sh', data: '#!/bin/sh\n', kind: 'executable' },
    ],
  });
  assert.equal(typeof id, 'string');
  const tree = await repo.treeAt(id);
  assert.deepEqual(
    tree.map((f) => [f.path, f.kind]).sort(),
    [
      ['a.txt', 'regular'],
      ['dir/b.bin', 'regular'],
      ['run.sh', 'executable'],
    ],
  );
  assert.equal((await repo.readFileAt(id, 'a.txt')).toString(), 'hello');
  assert.deepEqual([...(await repo.readFileAt(id, 'dir/b.bin'))], [0, 1, 2, 255]);
  const a = tree.find((f) => f.path === 'a.txt');
  assert.equal((await repo.readObject(a.object)).toString(), 'hello');
  assert.equal(await repo.resolve(id.slice(0, 8)), id);
  assert.equal(await repo.branchTip('main'), id);
  const head = await repo.snapshot(id);
  assert.equal(head.id, id);
  assert.equal(head.message, 'one');
  assert.equal(head.branch, 'main');
  assert.equal(head.parent, null);
  assert.ok(head.createdAt instanceof Date);
  assert.equal(head.createdAt.getTime(), head.createdAtMs);
  assert.equal(typeof (await repo.headToken()), 'bigint');
});

test('a stored-object entry carries a file forward', async () => {
  const repo = await fresh();
  const first = await repo.saveTree({
    branch: 'main',
    message: 'one',
    entries: [{ path: 'keep.txt', data: 'kept' }],
  });
  const [file] = await repo.treeAt(first);
  const second = await repo.saveTree({
    branch: 'main',
    message: 'two',
    parent: first,
    entries: [
      { path: 'keep.txt', object: file.object },
      { path: 'new.txt', data: 'new' },
    ],
  });
  assert.equal((await repo.readFileAt(second, 'keep.txt')).toString(), 'kept');
  assert.equal((await repo.snapshot(second)).parent, first);
});

test('exactly one of data or object is required', async () => {
  const repo = await fresh();
  await assert.rejects(
    repo.saveTree({ branch: 'main', message: 'x', entries: [{ path: 'a' }] }),
    { code: 'InvalidInput' },
  );
});

test('meta and author round-trip via snapshotMeta', async () => {
  const repo = await fresh();
  const id = await repo.saveTree({
    branch: 'main',
    message: 'm',
    entries: [{ path: 'a', data: 'x' }],
    meta: { app: { run: '42', note: 'hi' } },
    author: { name: 'Ada', email: 'ada@example.com' },
  });
  const meta = await repo.snapshotMeta(id);
  assert.equal(meta.app.run, '42');
  assert.equal(meta.app.note, 'hi');
  assert.equal(meta.velo['author.name'], 'Ada');
  assert.equal(meta.velo['author.email'], 'ada@example.com');
});

test('a fixed timestampMs gives identical ids in two repos', async () => {
  const save = async () => {
    const repo = await fresh();
    return repo.saveTree({
      branch: 'main',
      message: 'same',
      entries: [{ path: 'a', data: 'x' }],
      meta: { app: { k: 'v' } },
      author: { name: 'Ada' },
      timestampMs: 1_700_000_000_000,
    });
  };
  assert.equal(await save(), await save());
});

test('two concurrent saveTree calls on one Repo both resolve', async () => {
  const repo = await fresh();
  const [a, b] = await Promise.all([
    repo.saveTree({ branch: 'one', message: 'a', entries: [{ path: 'a', data: '1' }] }),
    repo.saveTree({ branch: 'two', message: 'b', entries: [{ path: 'b', data: '2' }] }),
  ]);
  assert.notEqual(a, b);
  assert.equal(await repo.branchTip('one'), a);
  assert.equal(await repo.branchTip('two'), b);
});

test('an unknown parent rejects with NotFound', async () => {
  const repo = await fresh();
  const parent = 'f'.repeat(64);
  await assert.rejects(
    repo.saveTree({
      branch: 'main',
      message: 'x',
      parent,
      entries: [{ path: 'a', data: 'x' }],
    }),
    (err) => {
      assert.equal(err.code, 'NotFound');
      assert.equal(typeof err.kind, 'string');
      assert.equal(typeof err.name, 'string');
      return true;
    },
  );
});

test('branchTip on an unborn branch is null', async () => {
  const repo = await fresh();
  assert.equal(await repo.branchTip('nothing'), null);
});

test('a malformed id rejects with InvalidInput', async () => {
  const repo = await fresh();
  await assert.rejects(repo.treeAt('not an id'), { code: 'InvalidInput' });
});
