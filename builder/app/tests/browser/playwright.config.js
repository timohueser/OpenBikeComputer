import { defineConfig } from '@playwright/test';

// One origin serves the shipped `dist/web` build and the pinned fixture catalog, so the tab fetches
// cells the way a same-origin deployment does — no CORS, and nothing outside loopback to reach.
// `pretest` builds `dist/web` with `VITE_CATALOG_URL=/catalog/catalog.json`, which is root-relative,
// so the port lives here and only here.
const PORT = Number(process.env.OBC_BROWSER_PORT || 4180);
if (!Number.isInteger(PORT) || PORT < 1 || PORT > 65535) {
  throw new Error('OBC_BROWSER_PORT must be an integer from 1 to 65535');
}

export default defineConfig({
  testDir: '.',
  testMatch: '*.test.js',
  // The journey downloads five cells and three terrain squares, assembles them in wasm and reads
  // the map back out of OPFS. Generous, because a timeout here is a failure, never a retry.
  timeout: 120_000,
  expect: { timeout: 15_000 },
  workers: 1,
  retries: 0,
  forbidOnly: true,
  outputDir: '../../../../.artifacts/web-builder/browser',
  reporter: [
    ['list'],
    ['junit', { outputFile: '../../../../.artifacts/web-builder/junit.xml' }],
  ],
  use: {
    browserName: 'chromium',
    baseURL: `http://127.0.0.1:${PORT}`,
    viewport: { width: 1440, height: 1100 },
    reducedMotion: 'no-preference',
    actionTimeout: 15_000,
    navigationTimeout: 15_000,
    screenshot: 'only-on-failure',
    trace: 'retain-on-failure',
  },
  webServer: {
    command:
      `python3 ../../../../tools/fixture_catalog.py --catalog web-assemble --port ${PORT}` +
      ' --static ../../dist/web --log ../../../../.artifacts/web-builder/catalog.jsonl',
    url: `http://127.0.0.1:${PORT}/catalog/catalog.json`,
    reuseExistingServer: false,
    timeout: 10_000,
    stdout: 'pipe',
    stderr: 'pipe',
  },
});
