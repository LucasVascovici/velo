// Config registry: versioned configs, rollback by carry-forward, tamper
// evidence. The Node port of bindings/python/examples/config_registry.py.
//
// The package is imported by name, so the script runs against an installed
// tarball. To run it against a local build:
//
//   cd bindings/node && npm install && npm run build && npm pack
//   mkdir /tmp/try && cd /tmp/try && npm init -y && npm install <path to the .tgz>
//   node <path to velody>/bindings/node/examples/config_registry.mjs
//
// Node resolves `velody-core` from the directory of the script, so copy the
// script next to the install (or into it) when it is run from elsewhere.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import velody from 'velody-core';

const { Repo } = velody;

const PATH = 'configs/service.yaml';
const VERSIONS = {
  '1.0.0': 'replicas: 2\ntimeout: 30\n',
  '1.1.0': 'replicas: 3\ntimeout: 30\n',
  '1.2.0': 'replicas: 3\ntimeout: 5\n', // a bad change
};

const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'velody-config-registry-'));
try {
  let repo = await Repo.init(tmp);

  // Publish: one snapshot per version on the `registry` branch. The version
  // lives in metadata (the binding exposes branches, not tags).
  const ids = {};
  let parent;
  for (const [version, body] of Object.entries(VERSIONS)) {
    parent = await repo.saveTree({
      branch: 'registry',
      message: `publish ${version}`,
      entries: [
        { path: PATH, data: body },
        { path: 'configs/limits.yaml', data: 'cpu: 1\n' },
      ],
      parent,
      author: { name: 'release-bot' },
      meta: { registry: { version, approved_by: 'alice' } },
    });
    ids[version] = parent;
  }
  assert.equal(await repo.branchTip('registry'), ids['1.2.0']);

  // Look a release up by metadata.
  const hits = await repo.findSnapshots([
    { namespace: 'registry', key: 'version', value: '1.1.0' },
  ]);
  assert.equal(hits.length, 1);
  assert.equal(hits[0].id, ids['1.1.0']);

  // Roll back to 1.1.0: carry forward the stored objects, no copying.
  const old = await repo.treeAt(ids['1.1.0']);
  const entries = old.map((f) => ({ path: f.path, object: f.object, kind: f.kind }));
  const rolled = await repo.saveTree({
    branch: 'registry',
    message: 'rollback to 1.1.0',
    entries,
    parent: ids['1.2.0'],
    author: { name: 'oncall' },
    meta: { registry: { version: '1.1.0', approved_by: 'bob', rollback_of: '1.2.0' } },
  });
  const at = async (id) => (await repo.readFileAt(id, PATH)).toString();
  assert.equal(await at(rolled), VERSIONS['1.1.0']);
  assert.equal(await at(rolled), await at(ids['1.1.0']));
  // History is append-only: the bad 1.2.0 is still there to audit.
  assert.equal(await at(ids['1.2.0']), VERSIONS['1.2.0']);

  // Audit: who approved each state, newest first.
  const trail = [];
  for (const e of await repo.history({ branch: 'registry' })) {
    const m = (await repo.snapshotMeta(e.id)).registry;
    trail.push([m.version, m.approved_by]);
    console.log(e.id.slice(0, 12), m.version, m.approved_by);
  }
  assert.deepEqual(trail, [
    ['1.1.0', 'bob'],
    ['1.2.0', 'alice'],
    ['1.1.0', 'alice'],
    ['1.0.0', 'alice'],
  ]);

  // Tamper evidence: the same content with a different approver is a
  // different snapshot id, so history cannot be quietly rewritten.
  const forged = await repo.saveTree({
    branch: 'forged',
    message: 'publish 1.2.0',
    entries: [
      { path: PATH, data: VERSIONS['1.2.0'] },
      { path: 'configs/limits.yaml', data: 'cpu: 1\n' },
    ],
    parent: ids['1.1.0'],
    author: { name: 'release-bot' },
    meta: { registry: { version: '1.2.0', approved_by: 'mallory' } },
  });
  assert.notEqual(forged, ids['1.2.0']);
  assert.equal((await repo.snapshotMeta(ids['1.2.0'])).registry.approved_by, 'alice');
  console.log('tamper detected: ids differ', forged.slice(0, 12), ids['1.2.0'].slice(0, 12));

  repo = null;
} finally {
  // Node has no way to close a handle, so on Windows the database file can
  // stay locked until the process exits. Cleanup is best effort.
  try {
    fs.rmSync(tmp, { recursive: true, force: true });
  } catch {
    console.log('left the temp directory behind:', tmp);
  }
}
