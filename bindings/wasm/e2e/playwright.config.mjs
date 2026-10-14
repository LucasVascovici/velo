import { defineConfig, devices } from '@playwright/test';

// Static server rooted at bindings/wasm so the page can reach ../pkg. OPFS
// sahpool needs no COOP/COEP headers.
export default defineConfig({
  testDir: '.',
  testMatch: 'reload.spec.mjs',
  reporter: 'list',
  use: { baseURL: 'http://127.0.0.1:8123' },
  webServer: {
    command: 'python -m http.server 8123 --bind 127.0.0.1',
    cwd: '..',
    url: 'http://127.0.0.1:8123/e2e/index.html',
    reuseExistingServer: !process.env.CI,
  },
  projects: [
    { name: 'chromium', use: { ...devices['Desktop Chrome'] } },
    // Not run in CI; see the README for their status.
    { name: 'firefox', use: { ...devices['Desktop Firefox'] } },
    { name: 'webkit', use: { ...devices['Desktop Safari'] } },
  ],
});
