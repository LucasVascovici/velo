import { test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { createRequire } from 'node:module';

const require = createRequire(import.meta.url);
const { Repo } = require('../index.js');

const DAY = 86_400_000;
const NOW = 100 * DAY;
// Everything older than a day keeps one snapshot per 10 days.
const TIERS = [{ olderThanMs: DAY, keepOnePerMs: 10 * DAY }];

async function fresh() {
  return Repo.init(fs.mkdtempSync(path.join(os.tmpdir(), 'velo-node-')));
}

/** One snapshot a day for `count` days, oldest first. */
async function spread(repo, count = 40) {
  const ids = [];
  let parent;
  for (let i = 0; i < count; i++) {
    parent = await repo.saveTree({
      branch: 'main',
      message: `s${i}`,
      entries: [{ path: 'f', data: String(i) }],
      parent,
      timestampMs: NOW - (count - i) * DAY,
    });
    ids.push(parent);
  }
  return ids;
}

test('retention dry run reports ranges and changes nothing', async () => {
  const repo = await fresh();
  const ids = await spread(repo);
  const report = await repo.retention('main', { tiers: TIERS, nowMs: NOW, dryRun: true });
  assert.equal(report.dryRun, true);
  assert.ok(report.removed > 0);
  assert.ok(report.squashed.length > 0);
  assert.equal(report.examined, 40);
  assert.equal(await repo.branchTip('main'), ids[39]);
  assert.equal((await repo.history({ branch: 'main' })).length, 40);
});

test('retention squashes and old ids reject with Compacted', async () => {
  const repo = await fresh();
  await spread(repo);
  const report = await repo.retention('main', { tiers: TIERS, nowMs: NOW });
  assert.ok(report.removed > 0);
  assert.equal((await repo.history({ branch: 'main' })).length, 40 - report.removed);
  const { into, removed } = report.squashed[0];
  await assert.rejects(repo.snapshot(removed[0]), (e) => {
    assert.equal(e.code, 'Compacted');
    assert.equal(e.into, into);
    return true;
  });
  assert.equal((await repo.snapshot(into)).id, into);
});

test('the default policy reports tagged snapshots as protected', async () => {
  const repo = await fresh();
  const ids = await spread(repo);
  await repo.createTag('keep', ids[5]);
  const report = await repo.retention('main', { nowMs: NOW, dryRun: true });
  assert.deepEqual(
    report.protected.find((p) => p.id === ids[5]),
    { id: ids[5], reason: 'tagged' },
  );
});

test('compact squashes a range and re-mints its descendants', async () => {
  const repo = await fresh();
  const ids = await spread(repo, 6);
  const dry = await repo.compact('main', [{ oldest: ids[1], newest: ids[3] }], true);
  assert.equal(dry.dryRun, true);
  assert.equal((await repo.snapshot(ids[2])).id, ids[2]);
  const done = await repo.compact('main', [{ oldest: ids[1], newest: ids[3] }]);
  assert.deepEqual(done.squashed[0].removed, ids.slice(1, 4));
  assert.ok(done.rewritten.length > 0);
  const { old, new: renamed } = done.rewritten[0];
  await assert.rejects(repo.snapshot(old), (e) => {
    assert.equal(e.code, 'Compacted');
    assert.equal(e.into, renamed);
    return true;
  });
});

test('bad tiers reject as InvalidInput', async () => {
  const repo = await fresh();
  await spread(repo, 3);
  await assert.rejects(
    repo.retention('main', { tiers: [{ olderThanMs: 0, keepOnePerMs: DAY }], nowMs: NOW }),
    (e) => e.code === 'InvalidInput',
  );
});

test('tags: create, force, list, delete', async () => {
  const repo = await fresh();
  const ids = await spread(repo, 3);
  assert.deepEqual(await repo.tags(), []);
  assert.equal(await repo.createTag('v1', ids[0]), ids[0]);
  await repo.createTag('v2', ids[1].slice(0, 12));
  const tags = await repo.tags();
  assert.deepEqual(tags.map((t) => [t.name, t.snapshot]), [['v1', ids[0]], ['v2', ids[1]]]);
  assert.equal(tags[0].message, 's0');
  await assert.rejects(repo.createTag('v1', ids[2]), (e) => e.code === 'InvalidInput');
  await repo.createTag('v1', ids[2], true);
  assert.equal((await repo.tags())[0].snapshot, ids[2]);
  await repo.deleteTag('v1');
  assert.deepEqual((await repo.tags()).map((t) => t.name), ['v2']);
  await assert.rejects(repo.deleteTag('v1'), (e) => e.code === 'NotFound');
});

test('fsck on a healthy repository finds no problems', async () => {
  const repo = await fresh();
  await spread(repo, 3);
  const report = await repo.fsck();
  assert.deepEqual(report, { problems: [], cruft: [], repaired: [] });
  assert.deepEqual((await repo.fsck(true)).problems, []);
});
