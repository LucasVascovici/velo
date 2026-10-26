import { test, expect } from '@playwright/test';

async function call(page, message) {
  await page.waitForFunction(() => window.velodyReady === true);
  const reply = await page.evaluate((m) => window.velodyCall(m), message);
  expect(reply.error, JSON.stringify(reply)).toBeUndefined();
  return reply;
}

test('a repository written in a worker survives a page reload', async ({ page }) => {
  await page.goto('/e2e/index.html');
  const text = `written at ${Date.now()}\n`;
  const saved = await call(page, { op: 'save', text });
  expect(saved.ids).toEqual([saved.id]);
  expect(saved.text).toBe(text);

  await page.reload();

  const read = await call(page, { op: 'read' });
  expect(read.ids).toEqual([saved.id]);
  expect(read.text).toBe(text);
});
