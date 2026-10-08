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

function save(repo, branch, message, text, extra = {}) {
  return repo.saveTree({
    branch,
    message,
    entries: [{ path: 'a.txt', data: text }],
    ...extra,
  });
}

test('history from a merge tip includes both sides', async () => {
  const repo = await fresh();
  const base = await save(repo, 'main', 'base', 'one\ntwo\n');
  const ours = await save(repo, 'main', 'ours', 'ONE\ntwo\n', { parent: base });
  const theirs = await save(repo, 'side', 'theirs', 'one\nTWO\n', { parent: base });
  const merged = await save(repo, 'main', 'merge', 'ONE\nTWO\n', {
    parent: ours,
    mergeParent: theirs,
  });
  const entries = await repo.history({ from: merged });
  const ids = entries.map((e) => e.id);
  assert.equal(ids[0], merged);
  for (const id of [base, ours, theirs]) assert.ok(ids.includes(id));
  assert.equal(entries[0].mergeParent, theirs);
  assert.ok(entries[0].createdAt instanceof Date);
  assert.equal(entries[0].createdAt.getTime(), entries[0].createdAtMs);

  const onSide = await repo.history({ branch: 'side' });
  assert.deepEqual(onSide.map((e) => e.id), [theirs]);
  const touching = await repo.history({ from: merged, paths: ['a.txt'], limit: 2 });
  assert.equal(touching.length, 2);
});

test('a meta filter composes with limit', async () => {
  const repo = await fresh();
  let parent;
  const tagged = [];
  for (let i = 0; i < 4; i++) {
    const meta = i % 2 === 0 ? { review: { state: 'ok' } } : { other: { x: '1' } };
    parent = await save(repo, 'main', `s${i}`, `v${i}\n`, { parent, meta });
    if (i % 2 === 0) tagged.push(parent);
  }
  const all = await repo.history({ from: parent, meta: [{ namespace: 'review', key: 'state' }] });
  assert.deepEqual(all.map((e) => e.id), tagged.reverse());
  const one = await repo.history({
    from: parent,
    meta: [{ namespace: 'review', key: 'state', value: 'ok' }],
    limit: 1,
  });
  assert.deepEqual(one.map((e) => e.id), [all[0].id]);
  const none = await repo.history({
    from: parent,
    meta: [{ namespace: 'review', key: 'state', value: 'bad' }],
  });
  assert.deepEqual(none, []);
});

test('findSnapshots matches every filter', async () => {
  const repo = await fresh();
  const a = await save(repo, 'main', 'a', 'a\n', { meta: { t: { k: 'v', j: 'w' } } });
  await save(repo, 'main', 'b', 'b\n', { meta: { t: { k: 'v' } } });
  const found = await repo.findSnapshots([
    { namespace: 't', key: 'k', value: 'v' },
    { namespace: 't', key: 'j' },
  ]);
  assert.deepEqual(found.map((e) => e.id), [a]);
  await assert.rejects(repo.findSnapshots([]), (err) => err.code === 'InvalidInput');
});

test('blame attributes lines with author and honours the line window', async () => {
  const repo = await fresh();
  const ada = { name: 'Ada', email: 'ada@example.com' };
  const first = await save(repo, 'main', 'first', 'one\ntwo\nthree\n', { author: ada });
  const second = await save(repo, 'main', 'second', 'one\nTWO\nthree\n', {
    parent: first,
    author: { name: 'Bob' },
  });
  const b = await repo.blame('a.txt', { at: second });
  assert.equal(b.snapshot, second);
  assert.deepEqual(b.lines.map((l) => l.text), ['one', 'TWO', 'three']);
  assert.equal(b.lines[0].lineNo, 1);
  assert.equal(b.lines[0].lineCount, 1);
  assert.equal(b.lines[0].origin.id, first);
  assert.deepEqual(b.lines[0].origin.author, { name: 'Ada', email: 'ada@example.com' });
  assert.equal(b.lines[0].origin.branch, 'main');
  assert.ok(b.lines[0].origin.createdAt instanceof Date);
  assert.equal(b.lines[1].origin.id, second);
  assert.deepEqual(b.lines[1].origin.author, { name: 'Bob', email: null });

  const window = await repo.blame('a.txt', { at: second, startLine: 2, endLine: 2 });
  assert.deepEqual(window.lines.map((l) => l.lineNo), [2]);
});
