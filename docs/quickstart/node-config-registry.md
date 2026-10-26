# Node: a versioned config registry in five minutes

Publish each version of a config as a snapshot, list the history, and roll
back to an earlier version. The package carries the native addon, so no
toolchain is needed.

## Install

Each [release](../INSTALL.md) attaches one tarball per platform, named
`velody-core-<version>-<platform>.tgz` (platform is `linux`, `macos` or
`windows`):

```sh
npm install https://github.com/LucasVascovici/velody/releases/download/vX.Y.Z/velody-core-<version>-<platform>.tgz
```

## Publish, read the history, roll back

Save the script as `registry.mjs` (ES modules, so top-level `await` works).

```js quickstart
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import velody from 'velody-core';

const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'velody-quickstart-'));
try {
  const repo = await velody.Repo.init(tmp);

  const v1 = await repo.saveChanges({
    branch: 'registry',
    message: 'publish 1.0.0',
    changes: [{ op: 'put', path: 'service.yaml', data: 'replicas: 2\n' }],
    author: { name: 'release-bot' },
  });
  const v2 = await repo.saveChanges({
    branch: 'registry',
    message: 'publish 1.1.0 (a bad change)',
    changes: [{ op: 'put', path: 'service.yaml', data: 'replicas: 0\n' }],
    parent: v1.snapshot,
    author: { name: 'release-bot' },
  });

  const log = await repo.history({ branch: 'registry' });
  assert.deepEqual(log.map((e) => e.id), [v2.snapshot, v1.snapshot]);

  // Roll back: read the old content and save it as a new snapshot, so the
  // bad version stays in the history.
  const old = await repo.readFileAt(v1.snapshot, 'service.yaml');
  const back = await repo.saveChanges({
    branch: 'registry',
    message: 'roll back to 1.0.0',
    changes: [{ op: 'put', path: 'service.yaml', data: old }],
    parent: v2.snapshot,
    author: { name: 'release-bot' },
  });
  assert.equal(
    (await repo.readFileAt(back.snapshot, 'service.yaml')).toString(),
    'replicas: 2\n',
  );
  console.log('ok:', back.snapshot.slice(0, 8));
} finally {
  // Node cannot close a handle, so on Windows the database stays locked until
  // the process exits. Cleanup is best effort.
  try {
    fs.rmSync(tmp, { recursive: true, force: true });
  } catch {}
}
```

The cookbook has the [longer recipe](../cookbook/config-registry.md) with
metadata lookups and tamper evidence.
