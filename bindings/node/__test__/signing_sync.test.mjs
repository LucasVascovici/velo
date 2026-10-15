// Snapshot signing and one-step sync (18.8-d).
//
// Signing needs only the binding. Sync needs a working tree, a remote and a
// clone, none of which the binding creates, so those scenarios set up their
// repositories with the `velo` CLI and skip when no binary is built.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { createRequire } from 'node:module';

const require = createRequire(import.meta.url);
const { Repo, SigningKey } = require('../index.js');

const opts = { timeout: 60000 };
const SEED = Buffer.from([...Array(32).keys()]);
const OTHER = Buffer.from([...Array(32).keys()].map((i) => i + 1));

function tmp() {
  return fs.mkdtempSync(path.join(os.tmpdir(), 'velo-node-'));
}

async function fresh() {
  return Repo.init(tmp());
}

function snapshot(repo) {
  return repo.saveTree({ branch: 'main', message: 'm', entries: [{ path: 'f', data: 'x' }] });
}

test('a signing key from a seed or hex has a public key', opts, () => {
  const key = SigningKey.fromSeed(SEED);
  assert.equal(key.publicKey.length, 64);
  assert.equal(SigningKey.fromHex(SEED.toString('hex')).publicKey, key.publicKey);
});

test('a bad seed or hex is refused', opts, () => {
  assert.throws(() => SigningKey.fromSeed(Buffer.from('short')), { code: 'InvalidInput' });
  assert.throws(() => SigningKey.fromHex('nothex'), { code: 'InvalidInput' });
});

test('sign, list, verify trusted and untrusted, list signed', opts, async () => {
  const repo = await fresh();
  const snap = await snapshot(repo);
  const key = SigningKey.fromSeed(SEED);
  const other = SigningKey.fromSeed(OTHER);
  assert.deepEqual(await repo.signatures(snap), []);
  assert.deepEqual(await repo.signedSnapshots(), []);

  const sig = await repo.sign(snap, key);
  assert.equal(sig.snapshot, snap);
  assert.equal(sig.publicKey, key.publicKey);
  assert.equal(sig.algorithm, 'ed25519');
  assert.deepEqual((await repo.signatures(snap)).map((s) => s.publicKey), [key.publicKey]);
  assert.deepEqual(await repo.signedSnapshots(), [snap]);

  const trusted = await repo.verify(snap, [key.publicKey]);
  assert.equal(trusted.isTrusted, true);
  assert.deepEqual(trusted.trusted, [key.publicKey]);
  assert.deepEqual(trusted.invalid, []);

  const untrusted = await repo.verify(snap, [other.publicKey]);
  assert.equal(untrusted.isTrusted, false);
  assert.deepEqual(untrusted.untrusted, [key.publicKey]);
  assert.deepEqual(untrusted.trusted, []);
  assert.equal((await repo.verify(snap, [])).isTrusted, false);
});

test('verify rejects a malformed key', opts, async () => {
  const repo = await fresh();
  const snap = await snapshot(repo);
  await assert.rejects(repo.verify(snap, ['zz']), { code: 'InvalidInput' });
});

// --- sync ---------------------------------------------------------------

function findVelo() {
  if (process.env.VELO_BIN) return process.env.VELO_BIN;
  const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..', '..');
  for (const profile of ['debug', 'release']) {
    for (const name of ['velo.exe', 'velo']) {
      const p = path.join(root, 'target', profile, name);
      if (fs.existsSync(p)) return p;
    }
  }
  return null;
}

const VELO = findVelo();
const skip = VELO ? false : 'no velo binary to set up repositories';
const syncOpts = { ...opts, skip };

function cli(cwd, ...args) {
  execFileSync(VELO, args, { cwd, stdio: 'pipe' });
}

function put(root, name, text) {
  fs.writeFileSync(path.join(root, name), text);
}

async function tip(root) {
  return (await Repo.open(root)).branchTip('main');
}

/** An origin with one snapshot and a clone of it. */
function pair() {
  const holder = tmp();
  const origin = path.join(holder, 'origin');
  fs.mkdirSync(origin);
  cli(origin, 'init');
  put(origin, 'a.txt', 'shared\n');
  cli(origin, 'save', 'base');
  cli(holder, 'clone', origin, 'copy');
  return { origin, copy: path.join(holder, 'copy') };
}

test('sync with nothing to do', syncOpts, async () => {
  const { copy } = pair();
  const out = await (await Repo.open(copy)).sync();
  assert.equal(out.kind, 'nothing');
  assert.equal(out.branch, 'main');
  assert.equal(out.remote, 'origin');
});

test('sync behind fast-forwards', syncOpts, async () => {
  const { origin, copy } = pair();
  const repo = await Repo.open(copy);
  put(origin, 'b.txt', 'new\n');
  cli(origin, 'save', 'theirs');
  const out = await repo.sync();
  assert.equal(out.kind, 'fast_forwarded');
  assert.equal(out.to, await tip(origin));
  assert.equal(fs.readFileSync(path.join(copy, 'b.txt'), 'utf8'), 'new\n');
});

test('sync ahead pushes', syncOpts, async () => {
  const { origin, copy } = pair();
  put(copy, 'b.txt', 'mine\n');
  cli(copy, 'save', 'mine');
  const out = await (await Repo.open(copy)).sync({ remote: 'origin', branch: 'main' });
  assert.equal(out.kind, 'pushed');
  assert.equal(out.report.outcome, 'sent');
  assert.equal(out.report.snapshots, 1);
  assert.equal(await tip(origin), await tip(copy));
});

test('sync diverged clean merges and pushes', syncOpts, async () => {
  const { origin, copy } = pair();
  put(origin, 'theirs.txt', 't\n');
  cli(origin, 'save', 'theirs');
  put(copy, 'mine.txt', 'm\n');
  cli(copy, 'save', 'mine');
  const out = await (await Repo.open(copy)).sync({
    author: { name: 'Ann', email: 'ann@example.com' },
    message: 'joined',
  });
  assert.equal(out.kind, 'merged');
  assert.equal(out.merge, await tip(origin));
  assert.equal(out.merge, await tip(copy));
  assert.equal(out.report.outcome, 'sent');
  assert.ok(fs.existsSync(path.join(copy, 'theirs.txt')));
  assert.ok(fs.existsSync(path.join(copy, 'mine.txt')));
});

test('sync diverged conflict stops and pushes nothing', syncOpts, async () => {
  const { origin, copy } = pair();
  put(origin, 'a.txt', 'theirs\n');
  cli(origin, 'save', 'theirs');
  put(copy, 'a.txt', 'mine\n');
  cli(copy, 'save', 'mine');
  const before = await tip(origin);
  const out = await (await Repo.open(copy)).sync();
  assert.equal(out.kind, 'conflicted');
  assert.deepEqual(out.paths, ['a.txt']);
  assert.equal(await tip(origin), before);
});

test('a sync token is accepted and never appears in an error', syncOpts, async () => {
  const { copy } = pair();
  const secret = 's3cr3t-bearer-token';
  const out = await (await Repo.open(copy)).sync({ token: secret });
  assert.equal(out.kind, 'nothing');
  put(copy, 'dirty.txt', 'unsaved\n');
  await assert.rejects(
    (await Repo.open(copy)).sync({ token: secret }),
    (e) => e.code === 'DirtyWorkingTree' && !String(e.message).includes(secret),
  );
  await assert.rejects(
    (await Repo.open(copy)).sync({ remote: 'nowhere', token: secret }),
    (e) => !String(e.message).includes(secret),
  );
});
