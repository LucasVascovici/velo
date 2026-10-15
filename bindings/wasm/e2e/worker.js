// Dedicated worker: opens the persistent repository and answers the page.
// Build the package first: `wasm-pack build bindings/wasm --target web`.
import init, { Repo } from '../pkg/velo_wasm.js';

const NAME = 'reload-test.db';
const ready = init();

async function handle({ op, text }) {
  await ready;
  const repo = await Repo.openPersistent(NAME);
  if (op === 'save') {
    const id = repo.saveTree({
      branch: 'main',
      message: 'saved before reload',
      entries: [{ path: 'note.txt', data: text }],
    });
    return { id, ...read(repo, id) };
  }
  if (op === 'read') {
    const ids = repo.history({ all: true }).map((e) => e.id);
    return { ids, ...(ids.length ? read(repo, ids[0]) : { text: null }) };
  }
  throw new Error(`unknown op ${op}`);
}

function read(repo, id) {
  const ids = repo.history({ all: true }).map((e) => e.id);
  const text = new TextDecoder().decode(repo.readFileAt(id, 'note.txt'));
  return { ids, text };
}

self.onmessage = async (e) => {
  try {
    self.postMessage({ ok: true, ...(await handle(e.data)) });
  } catch (err) {
    self.postMessage({ ok: false, error: String(err && err.message ? err.message : err), code: err && err.code });
  }
};
