import { defineConfig } from '@playwright/test';
import { loopbackPort } from '../../../../tools/loopback_port.mjs';

// The shipped operator page, served from loopback. Each test answers `/api/cli` in the page.
const PORT = await loopbackPort('OBC_DATA_BROWSER_PORT');

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
    command: `python3 serve.py ${PORT} ../../dist/data`,
    url: `http://127.0.0.1:${PORT}/data.html`,
    reuseExistingServer: false,
    timeout: 10_000,
  },
});
