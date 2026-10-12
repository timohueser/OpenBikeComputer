import { defineConfig } from '@playwright/test';

// The shipped operator page, served from loopback. Each test answers `/api/cli` in the page.
const PORT = Number(process.env.OBC_DATA_BROWSER_PORT || 4182);
if (!Number.isInteger(PORT) || PORT < 1 || PORT > 65535) {
  throw new Error('OBC_DATA_BROWSER_PORT must be an integer from 1 to 65535');
}

export default defineConfig({
  testDir: 'data',
  testMatch: '*.test.js',
  workers: 1,
  retries: 0,
  forbidOnly: true,
  outputDir: '../../../../.artifacts/web-builder/data',
  reporter: [
    ['list'],
    ['junit', { outputFile: '../../../../.artifacts/web-builder/data-junit.xml' }],
  ],
  use: {
    browserName: 'chromium',
    baseURL: `http://127.0.0.1:${PORT}`,
    screenshot: 'only-on-failure',
    trace: 'retain-on-failure',
  },
  webServer: {
    command: `python3 -m http.server ${PORT} --bind 127.0.0.1 --directory ../../dist/data`,
    url: `http://127.0.0.1:${PORT}/data.html`,
    reuseExistingServer: false,
    timeout: 10_000,
  },
});
