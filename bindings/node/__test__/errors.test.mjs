// Every velo_core::Error variant rejects with its own `code` (18.8-f).
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';

const require = createRequire(import.meta.url);
const { _errorCodes } = require('../index.js');

test('every core variant has a code equal to its name', () => {
  const pairs = _errorCodes();
  assert.ok(pairs.length > 0);
  for (const [variant, code] of pairs) {
    assert.equal(code, variant);
    assert.notEqual(code, 'Unknown');
  }
  assert.equal(new Set(pairs.map(([v]) => v)).size, pairs.length);
});
