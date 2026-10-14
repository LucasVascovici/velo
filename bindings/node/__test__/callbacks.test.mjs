import { test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { createRequire } from 'node:module';

const require = createRequire(import.meta.url);
const { Repo } = require('../index.js');

// A deadlock must fail the test rather than hang the run.
const opts = { timeout: 30000 };

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

async function fork(repo, file, base, ours, theirs) {
  const a = await save(repo, 'main', 'a', { [file]: base });
  const b = await save(repo, 'side', 'b', { [file]: theirs }, a);
  const c = await save(repo, 'main', 'c', { [file]: ours }, a);
  return { a, b, c };
}

function conflicts(plan) {
  return plan.files.filter((f) => f.action === 'conflicted').length;
}

const JSON_BASE = '{\n  "a": 1,\n  "b": 2\n}\n';
const JSON_OURS = '{\n  "a": 10,\n  "b": 2\n}\n';
const JSON_THEIRS = '{\n  "a": 1,\n  "b": 20\n}\n';

test('onEvent hears Saved after the call, outside the core call', opts, async () => {
  const repo = await fresh();
  const seen = [];
  const follow = [];
  repo.onEvent((e) => {
    seen.push(e);
    // The callback may use the repository: it runs outside the core call.
    follow.push(repo.branchTip('main'));
  });
  assert.equal(seen.length, 0);
  const snap = await save(repo, 'main', 'm', { f: '1' });
  assert.equal(seen.length, 1);
  assert.equal(seen[0].kind, 'Saved');
  assert.equal(seen[0].snapshot, snap);
  assert.equal(seen[0].branch, 'main');
  assert.equal(seen[0].parent, null);
  assert.equal(await follow[0], snap);
});

test('onEvent hears Saved then Merged for a merge', opts, async () => {
  const repo = await fresh();
  const { b, c } = await fork(repo, 'f', '1\n', '1\n', '1\n');
  const seen = [];
  repo.onEvent((e) => seen.push(e));
  const m = await repo.mergeCommit({ branch: 'main', ours: c, theirs: b, message: 'merge' });
  assert.deepEqual(seen.map((e) => e.kind), ['Saved', 'Merged']);
  assert.equal(seen[1].snapshot, m);
  assert.equal(seen[1].theirs, b);
});

test('the built-in json driver merges what lines cannot', opts, async () => {
  const repo = await fresh();
  const { b, c } = await fork(repo, 'c.json', JSON_BASE, JSON_OURS, JSON_THEIRS);
  const before = await repo.mergePlan(c, b);
  assert.ok(conflicts(before) > 0);
  await repo.setMergeDriver('*.json', 'json');
  const plan = await repo.mergePlan(c, b);
  assert.equal(conflicts(plan), 0);
  assert.equal(plan.files[0].action, 'autoMerged');
  const text = plan.files[0].content.toString();
  assert.ok(text.includes('"a": 10') && text.includes('"b": 20'));
  await repo.clearMergeDrivers();
  assert.ok(conflicts(await repo.mergePlan(c, b)) > 0);
});

test('an unknown driver name or bad pattern rejects', opts, async () => {
  const repo = await fresh();
  await assert.rejects(repo.setMergeDriver('*.json', 'xml'), { code: 'InvalidInput' });
  await assert.rejects(repo.setMergeDriver('[', 'json'), { code: 'InvalidInput' });
});

test('a custom JS driver merges, and null is a conflict', opts, async () => {
  const repo = await fresh();
  const { b, c } = await fork(repo, 'f.txt', 'base\n', 'ours\n', 'theirs\n');
  const calls = [];
  await repo.setMergeDriver('*.txt', (ancestor, ours, theirs) => {
    calls.push([ancestor, ours, theirs]);
    return `${ours.trim()}+${theirs.trim()}\n`;
  });
  const plan = await repo.mergePlan(c, b);
  assert.equal(conflicts(plan), 0);
  assert.equal(plan.files[0].content.toString(), 'ours+theirs\n');
  assert.deepEqual(calls, [['base\n', 'ours\n', 'theirs\n']]);
  const m = await repo.mergeCommit({ branch: 'main', ours: c, theirs: b, message: 'merge' });
  assert.equal((await repo.readFileAt(m, 'f.txt')).toString(), 'ours+theirs\n');

  await repo.clearMergeDrivers();
  await repo.setMergeDriver('*.txt', () => null);
  assert.equal(conflicts(await repo.mergePlan(c, b)), 1);
  await assert.rejects(
    repo.mergeCommit({ branch: 'main', ours: c, theirs: b, message: 'merge' }),
    { code: 'Conflicts' },
  );
});

test('a throwing driver rejects and commits nothing', opts, async () => {
  const repo = await fresh();
  const { b, c } = await fork(repo, 'f.txt', 'base\n', 'ours\n', 'theirs\n');
  const before = (await repo.history({ all: true })).map((e) => e.id);
  await repo.setMergeDriver('*.txt', () => {
    throw new Error('driver bug');
  });
  await assert.rejects(repo.mergePlan(c, b), { code: 'DriverFailed', message: /driver bug/ });
  await assert.rejects(
    repo.mergeCommit({ branch: 'main', ours: c, theirs: b, message: 'merge' }),
    { code: 'DriverFailed', message: /driver bug/ },
  );
  // Even with a resolution that would settle the conflict.
  await assert.rejects(
    repo.mergeCommit({
      branch: 'main',
      ours: c,
      theirs: b,
      message: 'merge',
      resolutions: { 'f.txt': 'ours' },
    }),
    { code: 'DriverFailed' },
  );
  assert.deepEqual((await repo.history({ all: true })).map((e) => e.id), before);
  assert.equal(await repo.branchTip('main'), c);

  await repo.clearMergeDrivers();
  await repo.setMergeDriver('*.txt', () => 5);
  await assert.rejects(repo.mergePlan(c, b), { code: 'DriverFailed' });
});

test('a driver calling back into the repo is refused, not deadlocked', opts, async () => {
  const repo = await fresh();
  const { b, c } = await fork(repo, 'f.txt', 'base\n', 'ours\n', 'theirs\n');
  let nested;
  await repo.setMergeDriver('*.txt', () => {
    nested = repo.branches().then(
      () => null,
      (e) => e,
    );
    return null;
  });
  const plan = await repo.mergePlan(c, b);
  assert.equal(conflicts(plan), 1);
  const err = await nested;
  assert.equal(err.code, 'Reentrant');
  assert.match(err.message, /must not call back into the repository/);
  // The handle is intact, and calls made outside a driver are not refused.
  assert.equal(await repo.branchTip('main'), c);
  await repo.clearMergeDrivers();
  assert.equal(conflicts(await repo.mergePlan(c, b)), 1);
});
