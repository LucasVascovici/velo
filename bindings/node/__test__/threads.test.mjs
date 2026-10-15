import { test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { createRequire } from 'node:module';
import { Worker } from 'node:worker_threads';

const require = createRequire(import.meta.url);
const { Repo } = require('../index.js');
const indexPath = fileURLToPath(new URL('../index.js', import.meta.url));

function tmp() {
  return fs.mkdtempSync(path.join(os.tmpdir(), 'velo-node-'));
}

// Each worker opens its own handle and saves; a write lock held by the other
// worker fails at once with code 'Locked', so the worker retries as documented.
const workerSource = `
const { parentPort, workerData } = require('node:worker_threads');
const { Repo } = require(workerData.indexPath);
(async () => {
  const repo = await Repo.open(workerData.dir);
  const ids = [];
  for (let i = 0; i < workerData.saves; i++) {
    for (;;) {
      try {
        ids.push(await repo.saveTree({
          branch: workerData.branch,
          message: workerData.branch + ' ' + i,
          parent: ids.length ? ids[ids.length - 1] : undefined,
          entries: [{ path: 'f.txt', data: workerData.branch + i }],
        }));
        break;
      } catch (e) {
        if (e.code !== 'Locked') throw e;
        await new Promise((r) => setTimeout(r, 5));
      }
    }
  }
  parentPort.postMessage({ ids });
})().catch((e) => { parentPort.postMessage({ error: String(e && e.stack || e) }); });
`;

function runWorker(dir, branch, saves) {
  return new Promise((resolve, reject) => {
    const worker = new Worker(workerSource, {
      eval: true,
      workerData: { indexPath, dir, branch, saves },
    });
    worker.once('message', (m) => (m.error ? reject(new Error(m.error)) : resolve(m)));
    worker.once('error', reject);
  });
}

test('a Repo does not cross to a worker as a usable handle', async () => {
  const repo = await Repo.init(tmp());
  const worker = new Worker(
    `const { parentPort } = require('node:worker_threads');
     parentPort.once('message', (m) => parentPort.postMessage({
       plain: Object.getPrototypeOf(m) === Object.prototype,
       keys: Object.keys(m),
       saveTree: typeof m.saveTree,
     }));`,
    { eval: true },
  );
  try {
    const seen = new Promise((resolve, reject) => {
      worker.once('message', resolve);
      worker.once('error', reject);
    });
    // The native handle is not cloneable. Depending on the Node version the
    // post either throws DataCloneError or delivers an empty plain object;
    // what must never happen is a working handle on the other thread.
    let threw = null;
    try {
      worker.postMessage(repo);
    } catch (e) {
      threw = e;
    }
    if (threw) {
      assert.equal(threw.name, 'DataCloneError');
    } else {
      assert.deepEqual(await seen, { plain: true, keys: [], saveTree: 'undefined' });
    }
  } finally {
    await worker.terminate();
  }
});

test('two workers, each with its own Repo on one path, both save', async () => {
  const dir = tmp();
  await Repo.init(dir);
  const [a, b] = await Promise.all([runWorker(dir, 'a', 5), runWorker(dir, 'b', 5)]);
  assert.equal(a.ids.length, 5);
  assert.equal(b.ids.length, 5);
  const repo = await Repo.open(dir);
  assert.equal(await repo.branchTip('a'), a.ids[4]);
  assert.equal(await repo.branchTip('b'), b.ids[4]);
});
