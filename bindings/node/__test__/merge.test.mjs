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

function save(repo, branch, message, files, parent) {
  return repo.saveTree({
    branch,
    message,
    entries: Object.entries(files).map(([p, data]) => ({ path: p, data })),
    parent,
  });
}

test('a clean plan, then mergeCommit', async () => {
  const repo = await fresh();
  const base = await save(repo, 'main', 'base', { 'a.txt': 'a\n' });
  const ours = await save(repo, 'main', 'ours', { 'a.txt': 'a\n', 'o.txt': 'o\n' }, base);
  const theirs = await save(repo, 'side', 'theirs', { 'a.txt': 'a\n', 't.txt': 't\n' }, base);
  assert.equal(await repo.mergeBase(ours, theirs), base);

  const plan = await repo.mergePlan(ours, theirs);
  assert.equal(plan.base, base);
  assert.equal(plan.files.length, 1);
  assert.equal(plan.files[0].path, 't.txt');
  assert.equal(plan.files[0].action, 'added');
  assert.equal(typeof plan.files[0].object, 'string');

  const merged = await repo.mergeCommit({
    branch: 'main',
    ours,
    theirs,
    message: 'merge',
    author: { name: 'Ada' },
  });
  const info = await repo.snapshot(merged);
  assert.equal(info.parent, ours);
  assert.equal(info.mergeParent, theirs);
  assert.equal((await repo.readFileAt(merged, 't.txt')).toString(), 't\n');
  assert.equal((await repo.readFileAt(merged, 'o.txt')).toString(), 'o\n');
});

test('a conflict rejects without resolutions and merges with a Buffer', async () => {
  const repo = await fresh();
  const base = await save(repo, 'main', 'base', { 'a.txt': 'x\n' });
  const ours = await save(repo, 'main', 'ours', { 'a.txt': 'ours\n' }, base);
  const theirs = await save(repo, 'side', 'theirs', { 'a.txt': 'theirs\n' }, base);

  const plan = await repo.mergePlan(ours, theirs);
  assert.equal(plan.files[0].action, 'conflicted');
  assert.equal((await repo.readObject(plan.files[0].base)).toString(), 'x\n');
  assert.equal((await repo.readObject(plan.files[0].ours)).toString(), 'ours\n');
  assert.equal((await repo.readObject(plan.files[0].theirs)).toString(), 'theirs\n');

  const input = { branch: 'main', ours, theirs, message: 'merge' };
  await assert.rejects(repo.mergeCommit(input), (err) => {
    assert.equal(err.code, 'Conflicts');
    assert.deepEqual(err.paths, ['a.txt']);
    return true;
  });

  const merged = await repo.mergeCommit({
    ...input,
    resolutions: { 'a.txt': Buffer.from('both\n') },
  });
  assert.equal((await repo.readFileAt(merged, 'a.txt')).toString(), 'both\n');

  const viaKeyword = await repo.mergeCommit({
    ...input,
    resolutions: { 'a.txt': 'theirs' },
  });
  assert.equal((await repo.readFileAt(viaKeyword, 'a.txt')).toString(), 'theirs\n');
});

test('createBranch, branches and setBranchTip', async () => {
  const repo = await fresh();
  const a = await save(repo, 'main', 'a', { 'a.txt': 'a\n' });
  const b = await save(repo, 'main', 'b', { 'a.txt': 'b\n' }, a);
  await repo.createBranch('topic', a);
  assert.equal(await repo.branchTip('topic'), a);
  await repo.createBranch('unborn');
  assert.equal(await repo.branchTip('unborn'), null);
  await assert.rejects(repo.createBranch('topic', a));

  await repo.setBranchTip('topic', b);
  assert.equal(await repo.branchTip('topic'), b);

  const list = await repo.branches();
  assert.deepEqual(list.map((x) => x.name), ['main', 'topic', 'unborn']);
  assert.equal(list.find((x) => x.name === 'main').tip, b);
  assert.equal(list.find((x) => x.name === 'unborn').tip, null);
});
